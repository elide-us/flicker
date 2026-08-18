//! **Content file operations** — the reversible moves the Content Manager makes.
//!
//! [`package`](crate::package) owns how content is ENCODED at rest (gz, one
//! way, one implementation). This module owns how it is REARRANGED: moving a
//! file or folder within a tree, renaming, creating a folder, and promoting
//! out of `staging/` into `package/`.
//!
//! # Moving does not re-encode
//!
//! Staging and package are both gz-at-rest, so a move relocates the physical
//! file **byte for byte** — no decompress/recompress step, and no second gz
//! path (the encoding rule in [`package`] is about not writing a rival
//! compressor; moving compressed bytes touches neither compressor).
//! [`physical_path`] resolves a LOGICAL path (`…/Foo.json`) to whatever is
//! actually on disk (`…/Foo.json.gz`, or the raw file for a dev-loose tree),
//! and the destination keeps the same form. Normalising a raw file to gz is
//! `content-tool gzify`'s job, and it is idempotent.
//!
//! # Everything is reversible
//!
//! Every operation records what it needs to undo itself, because the bench
//! promises *one mutation, one Ctrl+Z*. That is why a Replace-resolved
//! conflict never unlinks the file it displaces: the displaced bytes move to
//! [`TRASH_DIR`] under the batch's id, and reverting puts them back. A batch
//! reverts in reverse order, so overlapping operations unwind correctly.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::package::{gz_sibling, names_gz};

/// Where a Replace-resolved conflict parks the file it displaced, under the
/// staging root. Not a recycle bin for the user — it exists so that a batch
/// containing a Replace is still revertible.
pub const TRASH_DIR: &str = ".trash";

/// The physical file backing a LOGICAL content path: the gz twin when present,
/// else the raw file, else `None` when nothing is there.
///
/// The read-side companion is `package::read_bytes`; this is what a MOVE needs,
/// because it must relocate the actual bytes on disk rather than a name that
/// may not exist in that exact form.
#[must_use]
pub fn physical_path(logical: &Path) -> Option<PathBuf> {
    if names_gz(logical) {
        return logical.is_file().then(|| logical.to_path_buf());
    }
    let gz = gz_sibling(logical);
    if gz.is_file() {
        return Some(gz);
    }
    logical.is_file().then(|| logical.to_path_buf())
}

/// Give `dst` the same at-rest form as `src_physical` — if the source is the gz
/// twin, the destination is too. Keeps a move from silently changing a file's
/// encoding.
#[must_use]
fn matching_form(dst_logical: &Path, src_physical: &Path) -> PathBuf {
    if names_gz(src_physical) && !names_gz(dst_logical) {
        gz_sibling(dst_logical)
    } else {
        dst_logical.to_path_buf()
    }
}

/// True when a logical path already has something at it — file or directory,
/// in either at-rest form. What a conflict probe asks.
#[must_use]
pub fn occupied(logical: &Path) -> bool {
    logical.is_dir() || physical_path(logical).is_some()
}

/// What the conflict prompt shows about one side of a collision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFacts {
    /// The physical file (so the UI can show the real at-rest name).
    pub path: PathBuf,
    /// Size on disk, in bytes, of the physical file.
    pub size: u64,
    /// `true` when this side is a directory rather than a file.
    pub is_dir: bool,
}

impl FileFacts {
    /// Facts for a logical path, or `None` when nothing is there.
    #[must_use]
    pub fn of(logical: &Path) -> Option<Self> {
        if logical.is_dir() {
            return Some(Self {
                path: logical.to_path_buf(),
                size: 0,
                is_dir: true,
            });
        }
        let physical = physical_path(logical)?;
        let size = std::fs::metadata(&physical).map(|m| m.len()).unwrap_or(0);
        Some(Self {
            path: physical,
            size,
            is_dir: false,
        })
    }
}

/// One collision found before a batch runs, so the UI can prompt ONCE per
/// conflict instead of discovering them halfway through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// The item being moved.
    pub src: PathBuf,
    /// The logical destination it collides at.
    pub dst: PathBuf,
    /// What is already there.
    pub existing: FileFacts,
    /// What would land on it.
    pub incoming: FileFacts,
}

/// How the user chose to resolve a collision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolution {
    /// Displace the existing file into [`TRASH_DIR`] and move on top of it.
    Replace,
    /// Land beside it under a `_NN`-suffixed name.
    #[default]
    KeepBoth,
    /// Leave both alone — this item is not moved.
    Skip,
}

/// Probe `moves` (logical src → logical dst) for collisions, without touching
/// anything. Destinations that are free produce no entry.
#[must_use]
pub fn probe_conflicts(moves: &[(PathBuf, PathBuf)]) -> Vec<Conflict> {
    moves
        .iter()
        .filter_map(|(src, dst)| {
            let existing = FileFacts::of(dst)?;
            let incoming = FileFacts::of(src)?;
            Some(Conflict {
                src: src.clone(),
                dst: dst.clone(),
                existing,
                incoming,
            })
        })
        .collect()
}

/// The first free `<stem>_NN.<ext>` beside `dst` — the "Keep both" name.
///
/// Counts from `_01` and skips anything occupied in either at-rest form, so it
/// never hands back a name that would collide a second time. Gives up after 99
/// rather than spinning.
pub fn keep_both_name(dst: &Path) -> Result<PathBuf> {
    if !occupied(dst) {
        return Ok(dst.to_path_buf());
    }
    let parent = dst.parent().unwrap_or(Path::new(""));
    // Split on the FIRST dot so a compound extension survives: `Foo.pack.json`
    // must become `Foo_01.pack.json`, not `Foo.pack_01.json`.
    let name = dst
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("un-nameable destination {}", dst.display()))?;
    let (stem, ext) = match name.find('.') {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    };
    for n in 1..=99 {
        let candidate = parent.join(format!("{stem}_{n:02}{ext}"));
        if !occupied(&candidate) {
            return Ok(candidate);
        }
    }
    bail!("no free name beside {} after 99 tries", dst.display())
}

/// Relocate a physical file, preferring a rename and falling back to
/// copy+delete across filesystems. Never re-encodes.
fn relocate(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        // A rename across devices fails; copy+delete is the portable fallback.
        Err(_) => {
            std::fs::copy(from, to)
                .with_context(|| format!("copying {} → {}", from.display(), to.display()))?;
            std::fs::remove_file(from)
                .with_context(|| format!("removing {} after copy", from.display()))
        }
    }
}

/// One reversible change to the content tree.
///
/// Each variant records enough to undo itself; a [`FileOp`] is only meaningful
/// inside a [`BatchFileOp`], which is what the history stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// Move a file or directory from one logical path to another. Covers
    /// rename too — a rename is a move whose parent does not change.
    Move {
        src: PathBuf,
        dst: PathBuf,
        /// Set once applied: where the displaced occupant went, if any.
        displaced: Option<PathBuf>,
        /// Set once applied: the physical destination actually written.
        landed: Option<PathBuf>,
    },
    /// Create a directory.
    Mkdir { path: PathBuf },
}

impl FileOp {
    /// Move `src` to `dst` (both LOGICAL paths).
    #[must_use]
    pub fn mv(src: impl Into<PathBuf>, dst: impl Into<PathBuf>) -> Self {
        Self::Move {
            src: src.into(),
            dst: dst.into(),
            displaced: None,
            landed: None,
        }
    }

    /// Rename `path` to `new_name`, keeping it in place.
    #[must_use]
    pub fn rename(path: impl Into<PathBuf>, new_name: &str) -> Self {
        let path = path.into();
        let dst = path.parent().unwrap_or(Path::new("")).join(new_name);
        Self::mv(path, dst)
    }

    /// Create `path` as a directory.
    #[must_use]
    pub fn mkdir(path: impl Into<PathBuf>) -> Self {
        Self::Mkdir { path: path.into() }
    }
}

/// A group of [`FileOp`]s that succeed or unwind together, and that the history
/// stores as exactly ONE undoable entry.
///
/// `trash_root` is where Replace-displaced files are parked (normally the
/// staging root); `batch_id` namespaces this batch's displacements so two
/// batches can never reclaim each other's.
#[derive(Clone, Debug)]
pub struct BatchFileOp {
    ops: Vec<FileOp>,
    trash_root: PathBuf,
    batch_id: String,
    applied: bool,
}

impl BatchFileOp {
    /// A batch of `ops`, parking any displaced files under
    /// `<trash_root>/.trash/<batch_id>/`.
    pub fn new(
        ops: Vec<FileOp>,
        trash_root: impl Into<PathBuf>,
        batch_id: impl Into<String>,
    ) -> Self {
        Self {
            ops,
            trash_root: trash_root.into(),
            batch_id: batch_id.into(),
            applied: false,
        }
    }

    /// How many operations this batch carries (still ONE history entry).
    #[must_use]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// No operations — nothing to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    fn trash_dir(&self) -> PathBuf {
        self.trash_root.join(TRASH_DIR).join(&self.batch_id)
    }

    /// Run every operation in order.
    ///
    /// On failure the operations that already ran are **unwound** before the
    /// error is returned, so a partially-applied batch never survives — which
    /// is what lets the history treat the batch as atomic.
    pub fn apply(&mut self) -> Result<()> {
        for i in 0..self.ops.len() {
            if let Err(e) = self.apply_one(i) {
                // Unwind what landed, newest first, so overlapping moves undo cleanly.
                for j in (0..i).rev() {
                    let _ = self.revert_one(j);
                }
                self.applied = false;
                return Err(e);
            }
        }
        self.applied = true;
        Ok(())
    }

    /// Undo every operation, newest first.
    pub fn revert(&mut self) -> Result<()> {
        for i in (0..self.ops.len()).rev() {
            self.revert_one(i)?;
        }
        self.applied = false;
        Ok(())
    }

    /// Has this batch been applied?
    #[must_use]
    pub fn is_applied(&self) -> bool {
        self.applied
    }

    fn apply_one(&mut self, i: usize) -> Result<()> {
        let trash = self.trash_dir();
        match &mut self.ops[i] {
            FileOp::Mkdir { path } => std::fs::create_dir_all(&*path)
                .with_context(|| format!("creating {}", path.display())),
            FileOp::Move {
                src,
                dst,
                displaced,
                landed,
            } => {
                if src.is_dir() {
                    if dst.exists() {
                        bail!("{} already exists", dst.display());
                    }
                    relocate(src, dst)?;
                    *landed = Some(dst.clone());
                    *displaced = None;
                    return Ok(());
                }
                let physical_src =
                    physical_path(src).with_context(|| format!("nothing at {}", src.display()))?;
                let physical_dst = matching_form(dst, &physical_src);

                // Never unlink: an occupant is PARKED so the batch stays revertible.
                *displaced = match physical_path(dst) {
                    Some(occupant) => {
                        let parked = trash.join(format!("{i}")).join(
                            occupant
                                .file_name()
                                .unwrap_or_else(|| std::ffi::OsStr::new("file")),
                        );
                        relocate(&occupant, &parked)?;
                        Some(parked)
                    }
                    None => None,
                };
                relocate(&physical_src, &physical_dst)?;
                *landed = Some(physical_dst);
                Ok(())
            }
        }
    }

    fn revert_one(&mut self, i: usize) -> Result<()> {
        match &mut self.ops[i] {
            FileOp::Mkdir { path } => {
                // Only remove a directory WE created and that is still empty —
                // never take content down with it.
                if path.is_dir()
                    && std::fs::read_dir(&*path)
                        .map(|mut d| d.next().is_none())
                        .unwrap_or(false)
                {
                    std::fs::remove_dir(&*path)
                        .with_context(|| format!("removing {}", path.display()))?;
                }
                Ok(())
            }
            FileOp::Move {
                src,
                dst,
                displaced,
                landed,
            } => {
                // Send the mover home FIRST, which frees `dst` for its original
                // occupant; then un-park whatever Replace displaced.
                if let Some(there) = landed.take() {
                    let back = matching_form(src, &there);
                    relocate(&there, &back)?;
                }
                if let Some(parked) = displaced.take() {
                    let home = matching_form(dst, &parked);
                    relocate(&parked, &home)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flicker_ops_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write a gz-at-rest content file at a LOGICAL path, the way every
    /// processed-content writer does.
    fn write_content(logical: &Path, text: &str) {
        crate::package::write_text(logical, text).unwrap();
    }

    fn read_content(logical: &Path) -> String {
        crate::package::read_text(logical).unwrap()
    }

    #[test]
    fn physical_path_finds_the_gz_twin_then_the_raw_file() {
        let d = scratch("physical");
        let logical = d.join("Foo.json");
        assert_eq!(physical_path(&logical), None, "nothing there yet");

        write_content(&logical, "{}");
        assert_eq!(
            physical_path(&logical),
            Some(d.join("Foo.json.gz")),
            "the at-rest twin backs the logical name"
        );

        let raw = d.join("Bare.json");
        std::fs::write(&raw, b"{}").unwrap();
        assert_eq!(
            physical_path(&raw),
            Some(raw.clone()),
            "dev-loose raw file still resolves"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A move relocates the COMPRESSED file as-is — the content survives and
    /// the at-rest form is unchanged.
    #[test]
    fn moving_a_gz_file_preserves_its_at_rest_form_and_content() {
        let d = scratch("move_gz");
        let src = d.join("staging/Foo.json");
        let dst = d.join("package/Bar.json");
        write_content(&src, r#"{"format":"flicker.rig"}"#);

        let mut b = BatchFileOp::new(vec![FileOp::mv(&src, &dst)], &d, "b1");
        b.apply().unwrap();

        assert!(d.join("package/Bar.json.gz").is_file(), "landed in gz form");
        assert!(physical_path(&src).is_none(), "source is gone");
        assert_eq!(read_content(&dst), r#"{"format":"flicker.rig"}"#);

        b.revert().unwrap();
        assert_eq!(
            read_content(&src),
            r#"{"format":"flicker.rig"}"#,
            "back byte-for-byte"
        );
        assert!(physical_path(&dst).is_none(), "destination cleared");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rename_keeps_the_parent_and_round_trips() {
        let d = scratch("rename");
        let src = d.join("chars/Old.json");
        write_content(&src, "1");

        let mut b = BatchFileOp::new(vec![FileOp::rename(&src, "New.json")], &d, "b1");
        b.apply().unwrap();
        assert_eq!(read_content(&d.join("chars/New.json")), "1");
        assert!(physical_path(&src).is_none());

        b.revert().unwrap();
        assert_eq!(read_content(&src), "1");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn directories_move_whole_and_come_back() {
        let d = scratch("move_dir");
        write_content(&d.join("staging/Kit/A.json"), "a");
        write_content(&d.join("staging/Kit/sub/B.json"), "b");

        let mut b = BatchFileOp::new(
            vec![FileOp::mv(d.join("staging/Kit"), d.join("package/Kit"))],
            &d,
            "b1",
        );
        b.apply().unwrap();
        assert_eq!(read_content(&d.join("package/Kit/A.json")), "a");
        assert_eq!(
            read_content(&d.join("package/Kit/sub/B.json")),
            "b",
            "nested content came too"
        );
        assert!(!d.join("staging/Kit").exists());

        b.revert().unwrap();
        assert_eq!(read_content(&d.join("staging/Kit/A.json")), "a");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// THE reason `.trash` exists: Replace must not destroy the file it
    /// displaces, or the batch could not be undone.
    #[test]
    fn replace_parks_the_displaced_file_and_revert_restores_it() {
        let d = scratch("replace");
        let src = d.join("staging/Foo.json");
        let dst = d.join("package/Foo.json");
        write_content(&src, "incoming");
        write_content(&dst, "existing");

        let mut b = BatchFileOp::new(vec![FileOp::mv(&src, &dst)], &d, "batch7");
        b.apply().unwrap();
        assert_eq!(read_content(&dst), "incoming", "the incoming file won");
        assert!(
            d.join(TRASH_DIR).join("batch7").exists(),
            "the displaced file was parked, not unlinked"
        );

        b.revert().unwrap();
        assert_eq!(
            read_content(&dst),
            "existing",
            "the displaced file came home"
        );
        assert_eq!(read_content(&src), "incoming", "and the mover went back");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keep_both_counts_from_01_and_survives_a_compound_extension() {
        let d = scratch("keepboth");
        let free = d.join("Nothing.json");
        assert_eq!(
            keep_both_name(&free).unwrap(),
            free,
            "a free name is used as-is"
        );

        let taken = d.join("Foo.json");
        write_content(&taken, "x");
        assert_eq!(keep_both_name(&taken).unwrap(), d.join("Foo_01.json"));

        write_content(&d.join("Foo_01.json"), "x");
        assert_eq!(
            keep_both_name(&taken).unwrap(),
            d.join("Foo_02.json"),
            "skips the taken suffix"
        );

        // The compound extension must survive intact.
        let pack = d.join("Katanami.pack.json");
        write_content(&pack, "x");
        assert_eq!(
            keep_both_name(&pack).unwrap(),
            d.join("Katanami_01.pack.json")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn probe_reports_only_real_collisions_with_both_sides_facts() {
        let d = scratch("probe");
        let a = d.join("staging/A.json");
        let b = d.join("staging/B.json");
        let taken = d.join("package/A.json");
        write_content(&a, "aaaa");
        write_content(&b, "b");
        write_content(&taken, "old");

        let moves = vec![
            (a.clone(), taken.clone()),
            (b.clone(), d.join("package/B.json")),
        ];
        let conflicts = probe_conflicts(&moves);
        assert_eq!(conflicts.len(), 1, "only A collides");
        assert_eq!(conflicts[0].src, a);
        assert_eq!(conflicts[0].dst, taken);
        assert!(conflicts[0].existing.size > 0 && conflicts[0].incoming.size > 0);
        assert!(
            conflicts[0].existing.path.ends_with("A.json.gz"),
            "shows the real at-rest name"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A 40-item batch is one apply and one revert — the history's contract.
    #[test]
    fn a_forty_item_batch_applies_and_unwinds_whole() {
        let d = scratch("batch40");
        let mut ops = Vec::new();
        for i in 0..40 {
            let src = d.join(format!("staging/f{i}.json"));
            write_content(&src, &format!("v{i}"));
            ops.push(FileOp::mv(src, d.join(format!("package/f{i}.json"))));
        }
        let mut b = BatchFileOp::new(ops, &d, "big");
        assert_eq!(b.len(), 40);
        b.apply().unwrap();
        for i in 0..40 {
            assert_eq!(
                read_content(&d.join(format!("package/f{i}.json"))),
                format!("v{i}")
            );
        }

        b.revert().unwrap();
        for i in 0..40 {
            assert_eq!(
                read_content(&d.join(format!("staging/f{i}.json"))),
                format!("v{i}")
            );
            assert!(physical_path(&d.join(format!("package/f{i}.json"))).is_none());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A batch that fails partway must not leave half its work behind.
    #[test]
    fn a_failed_batch_unwinds_what_already_landed() {
        let d = scratch("partial");
        let ok = d.join("staging/Good.json");
        write_content(&ok, "good");

        let ops = vec![
            FileOp::mv(&ok, d.join("package/Good.json")),
            // Nothing at this source — the batch must fail here.
            FileOp::mv(
                d.join("staging/Missing.json"),
                d.join("package/Missing.json"),
            ),
        ];
        let mut b = BatchFileOp::new(ops, &d, "b1");
        assert!(b.apply().is_err(), "the batch fails");
        assert!(!b.is_applied());
        assert_eq!(read_content(&ok), "good", "the first move was rolled back");
        assert!(physical_path(&d.join("package/Good.json")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mkdir_reverts_only_when_still_empty() {
        let d = scratch("mkdir");
        let made = d.join("package/NewKit");
        let mut b = BatchFileOp::new(vec![FileOp::mkdir(&made)], &d, "b1");
        b.apply().unwrap();
        assert!(made.is_dir());
        b.revert().unwrap();
        assert!(!made.exists(), "an empty folder we created is removed");

        // With content in it, revert must NOT take the content down.
        let mut b = BatchFileOp::new(vec![FileOp::mkdir(&made)], &d, "b2");
        b.apply().unwrap();
        write_content(&made.join("Later.json"), "kept");
        b.revert().unwrap();
        assert!(made.is_dir(), "a folder someone filled is left alone");
        assert_eq!(read_content(&made.join("Later.json")), "kept");
        let _ = std::fs::remove_dir_all(&d);
    }
}
