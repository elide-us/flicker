//! The PACKAGE MANIFEST — the record of promotion INTENT (`package/manifest.json`).
//!
//! Ruled 2026-08-02 (Content Manager redesign): the manifest is **not an index**. An
//! index is an observation, rebuilt by scanning; the manifest is authored BY the act
//! of promoting — one row per promote, appended by the Quartermaster. Undoing a
//! promote removes its row: the manifest never claims something staging took back.
//! `content-tool pack` packs it as ordinary content (provenance riding along in the
//! shipped package) — the packer's index comes from walking the TREE, never from here.
//!
//! v1 shape: `{ "version": 1, "entries": [ { name, class, path, promoted_from } ] }`,
//! written through the shared gz-at-rest seam like every text file in the package.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One promotion — what landed, what it is, where it lives now, where it came from.
/// `path` and `promoted_from` are LOGICAL paths relative to the content root
/// (`package/…`, `staging/…`), so the manifest survives the tree moving hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub name: String,
    pub class: String,
    pub path: String,
    pub promoted_from: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ManifestFile {
    #[serde(default = "one")]
    version: u32,
    #[serde(default)]
    entries: Vec<ManifestEntry>,
}

fn one() -> u32 {
    1
}

/// Read the manifest at `path` (logical; gz-transparent). Absent → empty v1.
pub fn read(path: &Path) -> Result<Vec<ManifestEntry>> {
    if !crate::package::file_exists(path) {
        return Ok(Vec::new());
    }
    let text = crate::package::read_text(path)
        .with_context(|| format!("reading manifest {}", path.display()))?;
    let f: ManifestFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing manifest {}", path.display()))?;
    Ok(f.entries)
}

fn write(path: &Path, entries: Vec<ManifestEntry>) -> Result<()> {
    let f = ManifestFile {
        version: 1,
        entries,
    };
    crate::package::write_text(path, &serde_json::to_string_pretty(&f)?)
        .with_context(|| format!("writing manifest {}", path.display()))?;
    Ok(())
}

/// LOGICAL paths are forward-slash on every platform (the manifest survives the
/// tree moving hosts). Callers build rows from `PathBuf`s, which spell `\` on
/// Windows — normalize at THIS seam, so a backslashed row can neither be
/// written by [`append`] nor missed by [`remove`], whoever constructed it.
fn normalized(entry: &ManifestEntry) -> ManifestEntry {
    ManifestEntry {
        name: entry.name.clone(),
        class: entry.class.clone(),
        path: entry.path.replace('\\', "/"),
        promoted_from: entry.promoted_from.replace('\\', "/"),
    }
}

/// Append one promotion row (logical paths normalized to forward slashes).
/// Creates the manifest on first promote.
pub fn append(path: &Path, entry: ManifestEntry) -> Result<()> {
    let mut entries = read(path)?;
    entries.push(normalized(&entry));
    write(path, entries)
}

/// Remove the LAST row equal to `entry` — the undo of [`append`]. Removing a row
/// that is not there is an error, never a silent no-op: an undo that cannot find
/// what it recorded is a corrupted ledger, and the caller must hear about it.
pub fn remove(path: &Path, entry: &ManifestEntry) -> Result<()> {
    let entry = normalized(entry);
    let mut entries = read(path)?;
    let Some(i) = entries.iter().rposition(|e| *e == entry) else {
        anyhow::bail!(
            "manifest {} has no row for `{}`",
            path.display(),
            entry.name
        );
    };
    entries.remove(i);
    write(path, entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: &str) -> ManifestEntry {
        ManifestEntry {
            name: n.to_string(),
            class: "Rig".to_string(),
            path: format!("package/characters/{n}"),
            promoted_from: format!("staging/characters/{n}"),
        }
    }

    /// Append → read → remove round-trips through the gz seam, preserves order,
    /// and an undo removes exactly its own row (the LAST equal one).
    #[test]
    fn the_manifest_appends_reads_and_removes_rows() {
        let d = std::env::temp_dir().join("flicker_manifest_test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let m = d.join("manifest.json");

        assert!(read(&m).unwrap().is_empty(), "absent manifest reads empty");
        append(&m, entry("GolemBase_Low")).unwrap();
        append(&m, entry("Tree")).unwrap();
        append(&m, entry("GolemBase_Low")).unwrap(); // re-promote: a second row
        let rows = read(&m).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "GolemBase_Low");
        assert_eq!(rows[1].name, "Tree");

        // Undo removes the LAST matching row, leaving the earlier promotion's.
        remove(&m, &entry("GolemBase_Low")).unwrap();
        let rows = read(&m).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "GolemBase_Low");
        assert_eq!(rows[1].name, "Tree");

        // Removing a row that is not recorded is a LOUD error.
        assert!(remove(&m, &entry("Nope")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// LOGICAL paths are forward-slash on EVERY platform. A row built from
    /// `PathBuf` strings (backslashed on Windows) must land normalized, and an
    /// undo must find its row by either spelling — this seam is the gate, so
    /// the guarantee is tested with literal backslashes on all platforms.
    #[test]
    fn rows_normalize_to_forward_slash_logical_paths() {
        let d = std::env::temp_dir().join("flicker_manifest_seps_test");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let m = d.join("manifest.json");

        let backslashed = ManifestEntry {
            name: "NewThing".into(),
            class: "Rig".into(),
            path: r"package\characters\NewThing".into(),
            promoted_from: r"staging\characters\NewThing".into(),
        };
        append(&m, backslashed.clone()).unwrap();
        let rows = read(&m).unwrap();
        assert_eq!(rows[0].path, "package/characters/NewThing");
        assert_eq!(rows[0].promoted_from, "staging/characters/NewThing");

        // The undo finds the row through the same normalization.
        remove(&m, &backslashed).unwrap();
        assert!(read(&m).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
