//! **The content-tree BROWSING model** — a directory listing, a folder tree, a
//! breadcrumb and the root ceiling, with no UI in sight.
//!
//! Lifted out of the Quartermaster (its `fs_model.rs`): shared functionality lives in
//! an ENGINE crate (rule `8B511B67`), and this is the engine crate that already owns
//! the roots service, the at-rest `.gz` seam and the package classifier — so a browsing
//! surface speaks the same names as every loader with nothing restated. The bench
//! reaches it through ONE re-export, so no second listing can grow beside it.
//!
//! Scope note: the interactive-breadcrumb (name + jump target) and free-roots-ceiling
//! halves went out with the in-engine `file_browser` modal Aaron reverted on
//! 2026-09-04 (ruling `AAD0DC4B` — file selection is the OS dialog through `rfd`).
//! What is here is what the Quartermaster consumes; nothing is kept warm for a
//! consumer that no longer exists.
//!
//! Everything here is pure `std` + this crate's own classifier, so the whole
//! navigation model is testable without a window: a test walks a scratch tree and
//! asserts what the panes would show.
//!
//! ROOTS are a SLICE, not a struct: the Quartermaster browses `package` + `staging`,
//! and the ceiling rule ([`parent_within_roots`]) reads whatever slice it is handed —
//! navigation never climbs out of the trees it was given.

use std::path::{Path, PathBuf};

use crate::{classify_package, PackageClass, TRASH_DIR};

/// One row in a file list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The LOGICAL path — the at-rest `.gz` is an encoding detail the browser
    /// never spells out, exactly as every loader addresses content.
    pub path: PathBuf,
    /// What the row shows in the Name column.
    pub name: String,
    /// Derived, never authored — drives both the Type column and the row colour.
    pub class: PackageClass,
    /// Size of the physical file on disk (0 for folders).
    pub size: u64,
    /// Directories sort first and open on Confirm.
    pub is_dir: bool,
}

/// The logical display name: `Foo.json.gz` on disk is `Foo.json` here.
#[must_use]
pub fn display_name(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    raw.strip_suffix(".gz").unwrap_or(&raw).to_string()
}

/// The LOGICAL path for a physical one — the at-rest `.gz` dropped, so callers speak
/// the same names as the loaders and the file operations.
///
/// Carrying the physical `.gz` around instead makes `dst == src` comparisons silently
/// fail — a rename to the same name then reads as "name taken".
#[must_use]
pub fn logical(physical: &Path) -> PathBuf {
    match physical.file_name().and_then(|n| n.to_str()) {
        Some(n) => match n.strip_suffix(".gz") {
            Some(stem) => physical.with_file_name(stem),
            None => physical.to_path_buf(),
        },
        None => physical.to_path_buf(),
    }
}

/// How a listing is ordered. Folders always precede files whatever the key —
/// a folder is a place, not a peer of the files beside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Name,
    Type,
    Size,
}

impl SortKey {
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Type => "type",
            Self::Size => "size",
        }
    }
}

/// List one directory, classified and sorted. A path that cannot be read lists
/// EMPTY rather than failing — a browser must survive a folder disappearing
/// under it.
#[must_use]
pub fn list_dir(dir: &Path, sort: SortKey, descending: bool) -> Vec<Row> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        // `.trash` is the undo machinery's parking space, not content.
        .filter(|p| p.file_name().is_none_or(|n| n != TRASH_DIR))
        .map(|physical| {
            let is_dir = physical.is_dir();
            let size = if is_dir {
                0
            } else {
                std::fs::metadata(&physical).map(|m| m.len()).unwrap_or(0)
            };
            Row {
                name: display_name(&physical),
                class: classify_package(&physical),
                size,
                is_dir,
                path: logical(&physical),
            }
        })
        .collect();

    rows.sort_by(|a, b| {
        // Folders first, always — the one ordering rule no sort key overrides.
        a.is_dir.cmp(&b.is_dir).reverse().then_with(|| match sort {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Type => a
                .class
                .id()
                .cmp(b.class.id())
                .then_with(|| a.name.cmp(&b.name)),
            SortKey::Size => a.size.cmp(&b.size).then_with(|| a.name.cmp(&b.name)),
        })
    });
    if descending {
        // Reverse WITHIN each group so folders stay on top.
        let split = rows.iter().position(|r| !r.is_dir).unwrap_or(rows.len());
        rows[..split].reverse();
        rows[split..].reverse();
    }
    rows
}

/// One row of the folder tree: a directory, its depth, and whether it is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: String,
    /// Indent level; the roots are 0.
    pub depth: usize,
    pub expanded: bool,
    /// Whether it has any subdirectory to expand into (drives the caret).
    pub has_children: bool,
}

fn has_subdir(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_dir())))
        .unwrap_or(false)
}

/// Flatten the folder tree into display rows, descending only into directories
/// the user has opened. Never walks the whole tree — an unopened branch costs
/// one `has_subdir` probe, which is what keeps a 600-file tree cheap to show.
#[must_use]
pub fn tree_rows(roots: &[PathBuf], expanded: &[PathBuf]) -> Vec<TreeRow> {
    let mut out = Vec::new();
    for root in roots {
        push_tree_row(root, 0, expanded, &mut out);
    }
    out
}

fn push_tree_row(dir: &Path, depth: usize, expanded: &[PathBuf], out: &mut Vec<TreeRow>) {
    if !dir.is_dir() {
        return;
    }
    let is_open = expanded.iter().any(|e| e == dir);
    out.push(TreeRow {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        depth,
        expanded: is_open,
        has_children: has_subdir(dir),
        path: dir.to_path_buf(),
    });
    if !is_open {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subs: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_none_or(|n| n != TRASH_DIR))
        .collect();
    subs.sort();
    for sub in subs {
        push_tree_row(&sub, depth + 1, expanded, out);
    }
}

/// The breadcrumb NAMES for `dir` — the root's own name first, then each segment
/// below it. A path outside every root yields just its own file name, so the bar is
/// never empty.
#[must_use]
pub fn breadcrumb(roots: &[PathBuf], dir: &Path) -> Vec<String> {
    for root in roots {
        let Ok(rel) = dir.strip_prefix(root) else {
            continue;
        };
        let mut out = vec![name_of(root)];
        out.extend(
            rel.components()
                .map(|seg| seg.as_os_str().to_string_lossy().to_string()),
        );
        return out;
    }
    vec![name_of(dir)]
}

/// A path's own last segment as display text (empty for a root with none).
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Whether `path` sits inside one of `roots` (a root itself counts) — the containment
/// test [`parent_within_roots`] is built from. Private: the free-roaming per-step guard
/// left with the `file_browser` modal, and this crate's one caller is right below.
fn within_roots(roots: &[PathBuf], path: &Path) -> bool {
    roots.iter().any(|r| path == r || path.starts_with(r))
}

/// The parent to go "up" to, unless `dir` is already one of the roots — a listing
/// never climbs out of the content trees.
#[must_use]
pub fn parent_within_roots(roots: &[PathBuf], dir: &Path) -> Option<PathBuf> {
    if roots.iter().any(|r| r == dir) {
        return None;
    }
    let parent = dir.parent()?;
    within_roots(roots, parent).then(|| parent.to_path_buf())
}

/// Every file under `dir`, recursively (sorted for stable batches).
///
/// A FILE path is one file — a single-file asset IS its own content, so a promote and
/// a facts read reach it through this one seam instead of forking on the item's shape.
#[must_use]
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    if dir.is_file() {
        return vec![dir.to_path_buf()];
    }
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        package: PathBuf,
        staging: PathBuf,
    }

    impl Scratch {
        fn roots(&self) -> Vec<PathBuf> {
            vec![self.package.clone(), self.staging.clone()]
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            if let Some(parent) = self.package.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    fn scratch(name: &str) -> Scratch {
        let d = std::env::temp_dir().join(format!("flicker_browse_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        let s = Scratch {
            package: d.join("package"),
            staging: d.join("staging"),
        };
        std::fs::create_dir_all(&s.package).unwrap();
        std::fs::create_dir_all(&s.staging).unwrap();
        s
    }

    fn write(path: &Path, text: &str) {
        crate::package::write_text(path, text).unwrap();
    }

    #[test]
    fn a_listing_shows_logical_names_not_the_at_rest_gz() {
        let r = scratch("names");
        write(
            &r.package.join("Foo.json"),
            r#"{"format":"flicker.rig","mesh":{"indices":[1]}}"#,
        );
        let rows = list_dir(&r.package, SortKey::Name, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].name, "Foo.json",
            "the .gz is an encoding detail, not a name"
        );
        assert_eq!(
            rows[0].path,
            r.package.join("Foo.json"),
            "and the PATH is logical too, matching the name — carrying the physical .gz here \
             makes dst==src comparisons silently fail"
        );
        assert_eq!(
            rows[0].class,
            PackageClass::Rig,
            "class is derived from content"
        );
        assert!(rows[0].size > 0);
    }

    #[test]
    fn folders_sort_first_under_every_key_and_direction() {
        let r = scratch("sort");
        std::fs::create_dir_all(r.package.join("zzz_folder")).unwrap();
        write(&r.package.join("aaa.json"), r#"{"clips":[{"name":"x"}]}"#);
        write(&r.package.join("mmm.json"), r#"{"clips":[{"name":"y"}]}"#);

        for (key, desc) in [
            (SortKey::Name, false),
            (SortKey::Name, true),
            (SortKey::Size, true),
            (SortKey::Type, true),
        ] {
            let rows = list_dir(&r.package, key, desc);
            assert!(rows[0].is_dir, "a folder leads under {key:?} desc={desc}");
        }
        // Descending reverses within the file group.
        let asc = list_dir(&r.package, SortKey::Name, false);
        let desc = list_dir(&r.package, SortKey::Name, true);
        assert_eq!(asc[1].name, "aaa.json");
        assert_eq!(desc[1].name, "mmm.json");
    }

    /// `.trash` is the undo machinery's parking space — it must never appear as
    /// content, or a user could navigate into it and move the bytes an undo needs.
    #[test]
    fn the_trash_is_never_listed_or_walked() {
        let r = scratch("trash");
        write(&r.staging.join(".trash/b1/Old.json"), "x");
        write(&r.staging.join("Real.json"), r#"{"clips":[{"name":"y"}]}"#);

        let rows = list_dir(&r.staging, SortKey::Name, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Real.json");

        let tree = tree_rows(&r.roots(), std::slice::from_ref(&r.staging));
        assert!(!tree.iter().any(|t| t.name == TRASH_DIR));
    }

    #[test]
    fn the_tree_shows_both_roots_and_descends_only_where_opened() {
        let r = scratch("tree");
        std::fs::create_dir_all(r.package.join("characters/katanami")).unwrap();
        std::fs::create_dir_all(r.staging.join("characters")).unwrap();

        let closed = tree_rows(&r.roots(), &[]);
        assert_eq!(closed.len(), 2, "just the two roots");
        assert_eq!(closed[0].name, "package");
        assert_eq!(closed[1].name, "staging");
        assert!(
            closed[0].has_children,
            "caret shows there is something inside"
        );
        assert!(!closed[0].expanded);

        let open = tree_rows(&r.roots(), std::slice::from_ref(&r.package));
        assert_eq!(open.len(), 3, "package opened one level");
        assert_eq!(open[1].name, "characters");
        assert_eq!(open[1].depth, 1);
        assert!(
            !open[1].expanded,
            "a child is not opened just because its parent is"
        );

        let deep = tree_rows(
            &r.roots(),
            &[r.package.clone(), r.package.join("characters")],
        );
        assert_eq!(deep.len(), 4);
        assert_eq!(deep[2].name, "katanami");
        assert_eq!(deep[2].depth, 2);
    }

    #[test]
    fn breadcrumbs_start_at_the_root_that_contains_the_folder() {
        let r = scratch("crumbs");
        let deep = r.package.join("characters/katanami");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            breadcrumb(&r.roots(), &deep),
            vec!["package", "characters", "katanami"]
        );
        assert_eq!(breadcrumb(&r.roots(), &r.staging), vec!["staging"]);
    }

    /// Backspace at a root must not climb out of the content tree: the ceiling is
    /// what `parent_within_roots` refuses to answer, which is all a caller can see.
    #[test]
    fn up_stops_at_the_roots() {
        let r = scratch("up");
        let deep = r.package.join("characters/katanami");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            parent_within_roots(&r.roots(), &deep),
            Some(r.package.join("characters"))
        );
        assert_eq!(
            parent_within_roots(&r.roots(), &r.package),
            None,
            "a root is the ceiling"
        );
        assert_eq!(parent_within_roots(&r.roots(), &r.staging), None);
        assert_eq!(
            parent_within_roots(&r.roots(), r.package.parent().expect("scratch parent")),
            None,
            "a folder ABOVE every root has no `up` inside them either"
        );
    }

    #[test]
    fn an_unreadable_folder_lists_empty_rather_than_failing() {
        let r = scratch("missing");
        assert!(list_dir(&r.package.join("nope"), SortKey::Name, false).is_empty());
    }

    /// `files_under` is the ONE seam a promote and a facts read reach an item's
    /// content through, so a single-file asset has to flow through it too —
    /// otherwise every consumer needs its own shape test.
    #[test]
    fn files_under_a_single_file_is_that_file() {
        let r = scratch("filesunder");
        let f = r.staging.join("worlds/planet_seam.epoch.gz");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, b"bytes").unwrap();
        assert_eq!(files_under(&f), vec![f.clone()]);
        assert_eq!(
            files_under(&r.staging.join("worlds")),
            vec![f],
            "and a directory still walks to its files"
        );
    }
}
