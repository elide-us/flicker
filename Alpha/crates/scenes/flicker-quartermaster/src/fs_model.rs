//! What the Quartermaster knows about STAGING — the promotable-asset queue.
//!
//! The generic half of this module (listing · sort · folder tree · breadcrumb · the
//! root ceiling · the at-rest `.gz` seam) MOVED to the engine crate as
//! [`flicker_content::browse`] (rule `8B511B67` — shared functionality lives in an
//! engine crate; rule `DDD070C7` — no parallel copy). The re-export below is the
//! bench's door onto that ONE implementation: `fs_model::list_dir` and friends still
//! resolve here, and there is no second listing in this crate for them to drift from.
//!
//! What stays is what is genuinely the Quartermaster's: the two content ROOTS it
//! browses, and the CM5 staging queue (the promotable items, their class and bulk).

use std::path::PathBuf;

use flicker_content::{classify_package, PackageClass};

pub use flicker_content::browse::{
    breadcrumb, display_name, files_under, list_dir, logical, parent_within_roots, tree_rows, Row,
    SortKey, TreeRow,
};

/// The two roots the bench browses, in the order the design shows them.
///
/// The engine's browsing model takes roots as a plain SLICE (a file dialog's roots are
/// whatever its caller allows), so this named pair is the bench's own shape and
/// [`Roots::list`] is the one adapter onto it.
#[derive(Clone, Debug)]
pub struct Roots {
    pub package: PathBuf,
    pub staging: PathBuf,
}

impl Roots {
    /// From the executable's declared content root.
    #[must_use]
    pub fn from_config() -> Self {
        let r = flicker_content::roots();
        Self {
            package: r.package(),
            staging: r.staging(),
        }
    }

    /// The pair as the engine browsing model wants it — the ONE adapter, so every
    /// surface that browses this bench's trees navigates through identical code.
    #[must_use]
    pub fn list(&self) -> Vec<PathBuf> {
        vec![self.package.clone(), self.staging.clone()]
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
                    name: display_name(&path),
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

    /// DEVELOPMENT-TIER GATES (Aaron 2026-09-05, ruling 977B4D38): the hard-coded handoff
    /// conditions of a refactor — tests that read this crate's own source and assert a
    /// transition holds. `cargo test -- --skip gates::` is the production tier (every OS);
    /// `cargo test -- gates::` runs only these (one OS in CI). A gate names the transition
    /// it enforces and is deleted when that transition closes.
    mod gates {
        use super::*;

        /// THE NO-DUPLICATE GATE: the browsing model lives in the ENGINE crate now
        /// ([`flicker_content::browse`]) and this bench re-exports it. A private copy
        /// growing back here is the drift rule `DDD070C7` exists to stop, so the
        /// module's own source is asserted to hold no second listing.
        #[test]
        fn the_bench_keeps_no_private_listing_of_its_own() {
            let src = include_str!("fs_model.rs");
            // The needles are ASSEMBLED, not written: a gate that names the thing it forbids
            // trips over its own text.
            for stem in ["list_dir", "tree_rows", "breadcrumb", "files_under"] {
                let banned = ["fn ", stem].concat();
                assert!(
                    !src.contains(&banned),
                    "fs_model.rs re-implements `{banned}` — the browsing model lives in \
                     the engine crate, and a second copy is exactly the drift the move removed"
                );
            }
            assert!(
                src.contains("pub use flicker_content::browse::"),
                "the bench reaches the engine model through the ONE re-export"
            );
        }
    }
}
