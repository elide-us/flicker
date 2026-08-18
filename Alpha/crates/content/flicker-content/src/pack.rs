//! `content-tool pack` — the store-only package container (`package.flk`).
//!
//! The tree under `content/package/` is already gz-at-rest (every text file
//! individually compressed — see [`crate::package`]), so the container adds
//! NO compression of its own: a zip whose entries are all **Stored**. The
//! central directory is the index (name → offset/size/crc32) over verbatim
//! byte ranges — full random access, openable with any zip tool. The `zip`
//! dependency is built with no compression features, so this module *cannot*
//! deflate even by accident.
//!
//! **Deterministic**: the same tree packs to byte-identical output — entries
//! are sorted by their forward-slash relative path, timestamps are fixed at
//! the zip epoch, permissions are fixed at 0644. A repack that changes no
//! file changes no bytes, so a content release diff is exactly a content diff.
//!
//! The walk is the ground truth for what ships: every regular file under the
//! package dir (dot-files skipped). `package/manifest.json.gz` — the
//! Quartermaster's promotion ledger — is packed as ordinary content like
//! everything else; it is provenance, never the index.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

/// What one [`pack_dir`] / [`verify_pack`] pass covered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackStats {
    /// Entries written / verified.
    pub files: usize,
    /// Total content bytes (uncompressed == stored size; the container adds
    /// only its index and headers).
    pub bytes: u64,
}

/// Entry options shared by every write: Stored (the container never
/// compresses), the fixed zip-epoch timestamp, fixed 0644 permissions —
/// the determinism spine, in one place.
fn stored_options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644)
}

/// Walk `dir` and return every packable file as its forward-slash relative
/// path, sorted by that path's bytes (the entry order contract). Refusals are
/// loud: symlinks and non-UTF-8 names are drift in a package tree, not data.
fn collect_entries(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    fn walk(root: &Path, cur: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
        for entry in
            std::fs::read_dir(cur).with_context(|| format!("pack: reading {}", cur.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue; // .DS_Store and friends — never content
            }
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                bail!(
                    "pack: {} is a symlink — a package tree holds plain files only",
                    path.display()
                );
            }
            if meta.is_dir() {
                walk(root, &path, out)?;
                continue;
            }
            let rel = path.strip_prefix(root).expect("walk stays under root");
            let Some(rel) = rel.to_str() else {
                bail!(
                    "pack: {} has a non-UTF-8 name — authored names must be nameable",
                    path.display()
                );
            };
            out.push((rel.replace('\\', "/"), path));
        }
        Ok(())
    }

    let mut entries = Vec::new();
    walk(dir, dir, &mut entries)?;
    ensure!(
        !entries.is_empty(),
        "pack: {} holds no files — an empty package is drift, not a package",
        dir.display()
    );
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(entries)
}

/// Pack the tree under `dir` into the store-only container at `out`
/// (conventionally `package.flk`). Writes `<out>.tmp`, verifies every entry
/// against the source tree (the gzify verify-before-delete ethos: nothing is
/// trusted until it reads back byte-identical), then atomically renames into
/// place. Refuses an `out` located inside `dir` — a package must never pack
/// itself.
pub fn pack_dir(dir: &Path, out: &Path) -> Result<PackStats> {
    ensure!(dir.is_dir(), "pack: {} is not a directory", dir.display());
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("pack: creating {}", parent.display()))?;
        }
    }
    let dir_canon = dir.canonicalize()?;
    if let Ok(out_parent) = out.parent().unwrap_or(Path::new(".")).canonicalize() {
        ensure!(
            !out_parent.starts_with(&dir_canon),
            "pack: output {} is inside the package tree — refusing to pack the pack",
            out.display()
        );
    }

    let entries = collect_entries(dir)?;

    let file_name = out
        .file_name()
        .with_context(|| format!("pack: {} has no file name", out.display()))?
        .to_string_lossy()
        .into_owned();
    let tmp = out.with_file_name(format!("{file_name}.tmp"));

    let mut stats = PackStats::default();
    {
        let mut zip = ZipWriter::new(BufWriter::new(
            File::create(&tmp).with_context(|| format!("pack: creating {}", tmp.display()))?,
        ));
        for (rel, path) in &entries {
            let mut src =
                File::open(path).with_context(|| format!("pack: opening {}", path.display()))?;
            let large = src.metadata()?.len() >= zip::ZIP64_BYTES_THR;
            zip.start_file(rel, stored_options().large_file(large))
                .with_context(|| format!("pack: starting entry {rel}"))?;
            stats.bytes += std::io::copy(&mut src, &mut zip)
                .with_context(|| format!("pack: writing entry {rel}"))?;
            stats.files += 1;
        }
        zip.finish()
            .context("pack: finishing the archive")?
            .flush()?;
    }

    // Verify: every entry reads back byte-identical, and the name sets match
    // exactly, before the tmp file may claim the real name.
    verify_against(&tmp, &entries).with_context(|| {
        format!(
            "pack: verification of {} FAILED — output left as .tmp",
            tmp.display()
        )
    })?;
    std::fs::rename(&tmp, out)
        .with_context(|| format!("pack: renaming {} -> {}", tmp.display(), out.display()))?;
    Ok(stats)
}

/// CRC-read every entry of `flk`; with `tree` given, also byte-compare each
/// entry against the tree file AND require the walked tree to contain exactly
/// the archive's names (both directions — a file missing from either side
/// fails loud).
pub fn verify_pack(flk: &Path, tree: Option<&Path>) -> Result<PackStats> {
    match tree {
        Some(dir) => {
            ensure!(dir.is_dir(), "verify: {} is not a directory", dir.display());
            verify_against(flk, &collect_entries(dir)?)
        }
        None => {
            let mut zip = open_archive(flk)?;
            let mut stats = PackStats::default();
            for i in 0..zip.len() {
                let mut entry = zip
                    .by_index(i)
                    .with_context(|| format!("verify: entry #{i}"))?;
                let name = entry.name().to_string();
                let mut sink = Vec::with_capacity(entry.size() as usize);
                entry
                    .read_to_end(&mut sink) // the reader checks the stored CRC at EOF
                    .with_context(|| format!("verify: reading {name}"))?;
                stats.files += 1;
                stats.bytes += sink.len() as u64;
            }
            Ok(stats)
        }
    }
}

fn open_archive(flk: &Path) -> Result<ZipArchive<BufReader<File>>> {
    ZipArchive::new(BufReader::new(
        File::open(flk).with_context(|| format!("opening {}", flk.display()))?,
    ))
    .with_context(|| format!("{} is not a readable package", flk.display()))
}

/// The shared verification core: `flk`'s entries ≡ `entries` (names AND bytes).
fn verify_against(flk: &Path, entries: &[(String, PathBuf)]) -> Result<PackStats> {
    let mut zip = open_archive(flk)?;
    ensure!(
        zip.len() == entries.len(),
        "verify: {} holds {} entries, the tree holds {} files",
        flk.display(),
        zip.len(),
        entries.len()
    );
    let mut stats = PackStats::default();
    for (rel, path) in entries {
        let mut entry = zip
            .by_name(rel)
            .with_context(|| format!("verify: {rel} is in the tree but not the package"))?;
        let mut packed = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut packed)
            .with_context(|| format!("verify: reading {rel}"))?;
        let source =
            std::fs::read(path).with_context(|| format!("verify: reading {}", path.display()))?;
        ensure!(
            packed == source,
            "verify: {rel} differs between package and tree"
        );
        stats.files += 1;
        stats.bytes += packed.len() as u64;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flicker_pack_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let tree = dir.join("package");
        std::fs::create_dir_all(tree.join("characters/golem")).unwrap();
        std::fs::create_dir_all(tree.join("flights")).unwrap();
        std::fs::write(tree.join("characters/golem/rig.json.gz"), b"gz-bytes-1").unwrap();
        std::fs::write(tree.join("characters/golem/skin.png"), b"png-bytes").unwrap();
        std::fs::write(tree.join("flights/intro.flight.gz"), b"gz-bytes-2").unwrap();
        std::fs::write(tree.join("manifest.json.gz"), b"ledger").unwrap();
        std::fs::write(tree.join(".DS_Store"), b"junk").unwrap();
        dir
    }

    /// Same tree, two packs, identical bytes — the determinism contract, gated
    /// on the actual output channel.
    #[test]
    fn repack_is_byte_identical() {
        let dir = fixture("determinism");
        let tree = dir.join("package");
        let a = dir.join("a.flk");
        let b = dir.join("b.flk");
        let s1 = pack_dir(&tree, &a).unwrap();
        let s2 = pack_dir(&tree, &b).unwrap();
        assert_eq!(s1, s2);
        assert_eq!(s1.files, 4, "dot-file must be skipped");
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The full pack → verify round trip, both archive-only and against-tree.
    #[test]
    fn verify_round_trips_and_catches_corruption() {
        let dir = fixture("verify");
        let tree = dir.join("package");
        let flk = dir.join("package.flk");
        pack_dir(&tree, &flk).unwrap();
        verify_pack(&flk, None).unwrap();
        verify_pack(&flk, Some(&tree)).unwrap();

        // A tree file that changes after packing must fail the against-tree pass.
        std::fs::write(tree.join("manifest.json.gz"), b"ledger-CHANGED").unwrap();
        assert!(verify_pack(&flk, Some(&tree)).is_err());

        // Flipping one content byte must fail even the archive-only CRC pass.
        let mut bytes = std::fs::read(&flk).unwrap();
        let idx = 60; // inside the first stored entry's body
        bytes[idx] ^= 0xFF;
        std::fs::write(&flk, &bytes).unwrap();
        assert!(verify_pack(&flk, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The loud refusals: missing tree, empty tree, output inside the tree.
    #[test]
    fn refusals_fail_loud() {
        let dir = fixture("refusals");
        let tree = dir.join("package");
        assert!(pack_dir(&dir.join("nope"), &dir.join("x.flk")).is_err());
        assert!(
            pack_dir(&tree, &tree.join("package.flk")).is_err(),
            "output inside the tree must be refused"
        );

        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(pack_dir(&empty, &dir.join("y.flk")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The writer↔reader drift gate, closed across the REAL seam: pack a tree,
    /// mount the result at a root that does NOT exist on disk, and read every
    /// logical path through `flicker_core::compression` — bytes must equal the
    /// source tree's. Ghost root ⇒ every read provably came from the archive.
    #[test]
    fn mounted_package_serves_the_seam() {
        use flicker_core::compression as seam;

        let dir = fixture("mount");
        let tree = dir.join("package");
        // One REALLY gzipped file so the mount → sniff → decompress path runs.
        let gz_payload = flicker_core::compression::compress_gzip(b"real inflated payload");
        std::fs::write(tree.join("flights/intro.flight.gz"), &gz_payload).unwrap();

        let flk = dir.join("package.flk");
        pack_dir(&tree, &flk).unwrap();

        let ghost = dir.join("ghost-install/package");
        assert!(!ghost.exists());
        flicker_core::mount::mount_package(&flk, &ghost).unwrap();

        // Logical-name read resolves the .gz twin inside the mount + inflates.
        assert_eq!(
            seam::read_text(&ghost.join("flights/intro.flight")).unwrap(),
            "real inflated payload"
        );
        // Raw binary entries pass through unchanged.
        assert_eq!(
            seam::read_bytes(&ghost.join("characters/golem/skin.png")).unwrap(),
            b"png-bytes"
        );
        // Existence + capped prefix reads work by logical name too.
        assert!(seam::file_exists(&ghost.join("manifest.json")));
        assert!(!seam::file_exists(&ghost.join("nope.json")));
        assert_eq!(
            seam::read_bytes_prefix(&ghost.join("flights/intro.flight"), 4).unwrap(),
            b"real"
        );
        // Directory enumeration comes from the mount (nothing on disk here).
        let names: Vec<String> = seam::list_dir(&ghost)
            .unwrap()
            .into_iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["characters", "flights", "manifest.json.gz"]);
        let sub = seam::list_dir(&ghost.join("characters/golem")).unwrap();
        assert_eq!(sub.len(), 2, "rig.json.gz + skin.png: {sub:?}");
        // Outside the mount, a missing dir is still NotFound.
        assert!(seam::list_dir(&dir.join("elsewhere")).is_err());

        flicker_core::mount::unmount();
        // Unmounted, the ghost root has nothing — reads must fail again.
        assert!(seam::read_bytes(&ghost.join("characters/golem/skin.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlink in the tree is drift, not data.
    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let dir = fixture("symlink");
        let tree = dir.join("package");
        std::os::unix::fs::symlink(tree.join("manifest.json.gz"), tree.join("link.gz")).unwrap();
        let err = pack_dir(&tree, &dir.join("z.flk")).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
