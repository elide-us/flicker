//! The PACKAGE MANIFEST — the record of promotion INTENT (`package/manifest.json`).
//!
//! Ruled 2026-08-02 (Content Manager redesign): the manifest is **not an index**. An
//! index is an observation, rebuilt by scanning; the manifest is authored BY the act
//! of promoting — one row per promote, appended by the Quartermaster and consumed
//! later by `content-tool pack`'s reserved subcommand. Undoing a promote removes its
//! row: the manifest never claims something staging took back.
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
    let f = ManifestFile { version: 1, entries };
    crate::package::write_text(path, &serde_json::to_string_pretty(&f)?)
        .with_context(|| format!("writing manifest {}", path.display()))?;
    Ok(())
}

/// Append one promotion row. Creates the manifest on first promote.
pub fn append(path: &Path, entry: ManifestEntry) -> Result<()> {
    let mut entries = read(path)?;
    entries.push(entry);
    write(path, entries)
}

/// Remove the LAST row equal to `entry` — the undo of [`append`]. Removing a row
/// that is not there is an error, never a silent no-op: an undo that cannot find
/// what it recorded is a corrupted ledger, and the caller must hear about it.
pub fn remove(path: &Path, entry: &ManifestEntry) -> Result<()> {
    let mut entries = read(path)?;
    let Some(i) = entries.iter().rposition(|e| e == entry) else {
        anyhow::bail!("manifest {} has no row for `{}`", path.display(), entry.name);
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
}
