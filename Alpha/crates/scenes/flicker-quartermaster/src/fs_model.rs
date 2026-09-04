//! What the browser knows about the content trees — a directory listing and a
//! folder tree, with no UI in sight.
//!
//! Split from the scene so the whole navigation model is testable without a
//! window: a test walks a scratch tree and asserts what the panes would show.

use std::path::{Path, PathBuf};

use flicker_content::{classify_package, PackageClass};

/// One row in the file list.
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

impl Row {
    /// The logical display name: `Foo.json.gz` on disk is `Foo.json` here.
    fn display_name(path: &Path) -> String {
        let raw = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        raw.strip_suffix(".gz").unwrap_or(&raw).to_string()
    }

    /// The logical path for a physical one — the at-rest `.gz` dropped, so the
    /// browser speaks the same names as the loaders and the file operations.
    fn logical_path(physical: &Path) -> PathBuf {
        match physical.file_name().and_then(|n| n.to_str()) {
            Some(n) => match n.strip_suffix(".gz") {
                Some(logical) => physical.with_file_name(logical),
                None => physical.to_path_buf(),
            },
            None => physical.to_path_buf(),
        }
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
pub fn list_dir(dir: &Path, sort: SortKey, descending: bool) -> Vec<Row> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<Row> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        // `.trash` is the undo machinery's parking space, not content.
        .filter(|p| {
            p.file_name()
                .is_none_or(|n| n != flicker_content::TRASH_DIR)
        })
        .map(|physical| {
            let is_dir = physical.is_dir();
            let size = if is_dir {
                0
            } else {
                std::fs::metadata(&physical).map(|m| m.len()).unwrap_or(0)
            };
            Row {
                name: Row::display_name(&physical),
                class: classify_package(&physical),
                size,
                is_dir,
                // LOGICAL, to match `name` and to match how every loader and
                // every file operation addresses content. Carrying the physical
                // `.gz` here instead makes `dst == src` comparisons silently
                // fail — a rename to the same name then reads as "name taken".
                path: Row::logical_path(&physical),
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

/// The two roots the bench browses, in the order the design shows them.
#[derive(Clone, Debug)]
pub struct Roots {
    pub package: PathBuf,
    pub staging: PathBuf,
}

impl Roots {
    /// From the executable's declared content root.
    pub fn from_config() -> Self {
        let r = flicker_content::roots();
        Self {
            package: r.package(),
            staging: r.staging(),
        }
    }

    pub fn all(&self) -> [&Path; 2] {
        [self.package.as_path(), self.staging.as_path()]
    }
}

fn has_subdir(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_dir())))
        .unwrap_or(false)
}

/// Flatten the folder tree into display rows, descending only into directories
/// the user has opened. Never walks the whole tree — an unopened branch costs
/// one `has_subdir` probe, which is what keeps a 600-file tree cheap to show.
pub fn tree_rows(roots: &Roots, expanded: &[PathBuf]) -> Vec<TreeRow> {
    let mut out = Vec::new();
    for root in roots.all() {
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
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .is_none_or(|n| n != flicker_content::TRASH_DIR)
        })
        .collect();
    subs.sort();
    for sub in subs {
        push_tree_row(&sub, depth + 1, expanded, out);
    }
}

/// The breadcrumb for `dir`, relative to whichever root contains it: the root's
/// own name first, then each segment below it. A path outside both roots yields
/// just its file name, so the bar is never empty.
pub fn breadcrumb(roots: &Roots, dir: &Path) -> Vec<String> {
    for root in roots.all() {
        if let Ok(rel) = dir.strip_prefix(root) {
            let mut crumbs = vec![root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()];
            crumbs.extend(
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string()),
            );
            return crumbs;
        }
    }
    vec![dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()]
}

/// The parent to go "up" to, unless `dir` is already one of the roots — the
/// browser never climbs out of the content trees.
pub fn parent_within_roots(roots: &Roots, dir: &Path) -> Option<PathBuf> {
    if roots.all().contains(&dir) {
        return None;
    }
    let parent = dir.parent()?;
    roots
        .all()
        .iter()
        .any(|r| parent.starts_with(r) || parent == *r)
        .then(|| parent.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Roots {
        let d = std::env::temp_dir().join(format!("flicker_qm_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        let roots = Roots {
            package: d.join("package"),
            staging: d.join("staging"),
        };
        std::fs::create_dir_all(&roots.package).unwrap();
        std::fs::create_dir_all(&roots.staging).unwrap();
        roots
    }

    fn write(path: &Path, text: &str) {
        flicker_content::package::write_text(path, text).unwrap();
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
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
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
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
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

        let tree = tree_rows(&r, std::slice::from_ref(&r.staging));
        assert!(!tree.iter().any(|t| t.name == flicker_content::TRASH_DIR));
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }

    #[test]
    fn the_tree_shows_both_roots_and_descends_only_where_opened() {
        let r = scratch("tree");
        std::fs::create_dir_all(r.package.join("characters/katanami")).unwrap();
        std::fs::create_dir_all(r.staging.join("characters")).unwrap();

        let closed = tree_rows(&r, &[]);
        assert_eq!(closed.len(), 2, "just the two roots");
        assert_eq!(closed[0].name, "package");
        assert_eq!(closed[1].name, "staging");
        assert!(
            closed[0].has_children,
            "caret shows there is something inside"
        );
        assert!(!closed[0].expanded);

        let open = tree_rows(&r, std::slice::from_ref(&r.package));
        assert_eq!(open.len(), 3, "package opened one level");
        assert_eq!(open[1].name, "characters");
        assert_eq!(open[1].depth, 1);
        assert!(
            !open[1].expanded,
            "a child is not opened just because its parent is"
        );

        let deep = tree_rows(&r, &[r.package.clone(), r.package.join("characters")]);
        assert_eq!(deep.len(), 4);
        assert_eq!(deep[2].name, "katanami");
        assert_eq!(deep[2].depth, 2);
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }

    #[test]
    fn breadcrumbs_start_at_the_root_that_contains_the_folder() {
        let r = scratch("crumbs");
        let deep = r.package.join("characters/katanami");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            breadcrumb(&r, &deep),
            vec!["package", "characters", "katanami"]
        );
        assert_eq!(breadcrumb(&r, &r.staging), vec!["staging"]);
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }

    /// Backspace at a root must not climb out of the content tree.
    #[test]
    fn up_stops_at_the_roots() {
        let r = scratch("up");
        let deep = r.package.join("characters/katanami");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            parent_within_roots(&r, &deep),
            Some(r.package.join("characters"))
        );
        assert_eq!(
            parent_within_roots(&r, &r.package),
            None,
            "a root is the ceiling"
        );
        assert_eq!(parent_within_roots(&r, &r.staging), None);
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }

    #[test]
    fn an_unreadable_folder_lists_empty_rather_than_failing() {
        let r = scratch("missing");
        assert!(list_dir(&r.package.join("nope"), SortKey::Name, false).is_empty());
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }

    /// `files_under` is the ONE seam the promote and the review facts reach an
    /// item's content through, so a single-file asset has to flow through it
    /// too — otherwise every consumer needs its own shape test.
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
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }

    /// A committed world is a SINGLE self-describing FILE — no folder, no
    /// primary json — so it has to enter the queue on its own. And the
    /// classifier is the admission test: a stray `.DS_Store` beside it must
    /// NOT become something a reviewer can promote.
    #[test]
    fn a_staged_world_file_is_a_single_file_queue_item() {
        let r = scratch("worldqueue");
        let physical = r.staging.join("worlds/planet_test.epoch.gz");
        std::fs::create_dir_all(physical.parent().unwrap()).unwrap();
        std::fs::write(&physical, b"a planet's bytes").unwrap();
        std::fs::write(r.staging.join("worlds/.DS_Store"), b"junk").unwrap();

        let q = staging_queue(&r);
        assert_eq!(q.len(), 1, "only the world is an asset: {q:?}");
        assert_eq!(q[0].class, PackageClass::Epoch);
        assert_eq!(q[0].files, 1);
        assert_eq!(q[0].bytes, 16);
        assert_eq!(q[0].dir, physical, "the file itself IS the item");
        assert_eq!(
            q[0].name, "planet_test.epoch",
            "the at-rest .gz is not part of the name"
        );
        assert_eq!(
            q[0].rel,
            PathBuf::from("worlds/planet_test.epoch.gz"),
            "but rel carries the PHYSICAL name — it addresses the bytes that move"
        );
        let _ = std::fs::remove_dir_all(r.package.parent().unwrap());
    }
}

// ─── the CM5 staging queue ───────────────────────────────────────────────────

/// Where staged ASSETS live, relative to the staging root — one entry per ingest
/// tier, mirroring exactly what the benches' commit roots write. An item is a
/// DIRECT CHILD of one of these: either the asset FOLDER (its files are the
/// dependencies that travel with it on promote) or, for a self-describing
/// SINGLE-FILE artifact like a Populous world's `.epoch`, the file itself.
pub const ITEM_ROOTS: [&str; 5] = [
    "characters",
    "props",
    "materials",
    "retarget/clips",
    "worlds",
];

/// One promotable asset sitting in staging: what it is, where it goes, its bulk.
#[derive(Debug, Clone)]
pub struct QueueItem {
    /// The asset directory (physical) — or the FILE itself for a single-file
    /// asset, which is its own whole content.
    pub dir: PathBuf,
    /// The asset's name on screen: the folder's name, or a single file's
    /// LOGICAL name (the at-rest `.gz` dropped, like every other display name).
    pub name: String,
    /// Path relative to the staging root (`characters/GolemBase_Low`,
    /// `worlds/planet_f96_s….epoch.gz`) — mirrored under `package/` by promote.
    /// A single file's segment is its PHYSICAL name, so the promote's
    /// destination, the occupancy probe and the manifest row all address the
    /// bytes that actually move; `name` is the display twin.
    pub rel: PathBuf,
    /// Derived from the asset's primary json — or, for a single-file asset,
    /// from the file itself. Never authored.
    pub class: PackageClass,
    /// Files inside (the dependencies that travel), and their total bytes.
    /// A single-file asset is one file.
    pub files: usize,
    pub bytes: u64,
}

/// The LOGICAL path for a physical one — the at-rest `.gz` dropped, so callers
/// speak the same names as the loaders (the public twin of `Row::logical_path`).
pub fn logical(physical: &Path) -> PathBuf {
    Row::logical_path(physical)
}

/// Every file under `dir`, recursively (sorted for stable batches).
///
/// A FILE path is one file — a single-file asset IS its own content, so the
/// promote and the review facts reach it through this one seam instead of
/// forking on the item's shape.
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

/// Scan the staging tiers for promotable assets, stable-ordered by tier then name.
pub fn staging_queue(roots: &Roots) -> Vec<QueueItem> {
    let mut out = Vec::new();
    for tier in ITEM_ROOTS {
        let root = roots.staging.join(tier);
        let Ok(rd) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut kids: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        kids.sort();
        for path in kids {
            // The on-disk segment: what `rel` must carry, so the promote moves
            // the bytes that are actually there.
            let physical = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let item = if path.is_dir() {
                let files = files_under(&path);
                if files.is_empty() {
                    continue; // an empty folder is not an asset
                }
                // Classify off the primary json (the asset's own file where present).
                let primary = files
                    .iter()
                    .find(|f| {
                        f.to_string_lossy().contains(&physical)
                            && f.to_string_lossy().contains(".json")
                    })
                    .or_else(|| files.iter().find(|f| f.to_string_lossy().contains(".json")));
                QueueItem {
                    rel: PathBuf::from(tier).join(&physical),
                    class: primary
                        .map(|p| classify_package(p))
                        .unwrap_or(PackageClass::Unknown),
                    files: files.len(),
                    bytes: files
                        .iter()
                        .filter_map(|f| f.metadata().ok())
                        .map(|m| m.len())
                        .sum(),
                    name: physical,
                    dir: path,
                }
            } else {
                // A SINGLE-FILE asset — self-describing, with no folder and no
                // primary json to probe (a Populous world's `.epoch`). The
                // classifier is the whole admission test: it reads the LOGICAL
                // name internally, so the at-rest `.gz` is no obstacle, and a
                // stray `.DS_Store` classifies Unknown and stays out of the
                // queue rather than becoming a reviewable asset.
                let class = classify_package(&path);
                if class == PackageClass::Unknown {
                    continue;
                }
                QueueItem {
                    rel: PathBuf::from(tier).join(&physical),
                    name: Row::display_name(&path),
                    class,
                    files: 1,
                    bytes: path.metadata().map(|m| m.len()).unwrap_or(0),
                    dir: path,
                }
            };
            out.push(item);
        }
    }
    out
}
