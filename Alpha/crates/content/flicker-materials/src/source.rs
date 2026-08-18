//! The loader **seam**: [`TableSource`] abstracts *where* the vocabulary rows
//! come from. The simulation asks a source for elements and materials; it never
//! hardcodes a path. [`JsonTableSource`] is the today implementation, reading
//! `Alpha/content/data/*.json`. Later, a flicker-net → web service → DB source
//! implements the same trait and the callers don't change — the JSON-now /
//! network-later seam from the handoff (§2, §8).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::compound::CompoundDef;
use crate::element::Element;
use crate::material::{MaterialDef, MaterialId};
use crate::rock::RockDef;

/// Filename of the periodic table within a [`JsonTableSource`] directory.
pub const PERIODIC_TABLE_FILE: &str = "periodic_table.json";
/// Filename of the material index within a [`JsonTableSource`] directory.
pub const MATERIALS_FILE: &str = "materials.json";
/// Filename of the compound catalog within a [`JsonTableSource`] directory.
pub const COMPOUNDS_FILE: &str = "compounds.json";
/// Filename of the rock catalog within a [`JsonTableSource`] directory.
pub const ROCKS_FILE: &str = "rocks.json";

/// An error loading the vocabulary. Both variants name the offending file so a
/// missing or malformed table is diagnosable without guessing which one failed.
#[derive(Debug, thiserror::Error)]
pub enum MaterialError {
    /// Reading the file failed (missing, permissions, …).
    #[error("reading material table `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The file was read but did not parse as the expected schema.
    #[error("parsing material table `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    /// The file parsed but did not carry the expected structure — no `materials` array, an
    /// id not present, or a value that would not serialise back. A write target that is not
    /// what the reader validated.
    #[error("material table `{path}`: {detail}")]
    Schema { path: String, detail: String },
}

/// Where the vocabulary rows come from. Implementors return the raw row lists;
/// [`crate::Tables::from_source`] indexes them into the queryable vocabulary.
pub trait TableSource {
    /// The periodic-table rows.
    fn load_elements(&self) -> Result<Vec<Element>, MaterialError>;
    /// The material-index rows.
    fn load_materials(&self) -> Result<Vec<MaterialDef>, MaterialError>;
    /// The compound-catalog rows. Defaults to empty so a source that predates the
    /// catalog (or a content dir without `compounds.json`) still loads.
    fn load_compounds(&self) -> Result<Vec<CompoundDef>, MaterialError> {
        Ok(Vec::new())
    }
    /// The rock-catalog rows. Defaults to empty for the same reason as compounds:
    /// a source that predates the tier still loads, it just has no rocks to offer.
    fn load_rocks(&self) -> Result<Vec<RockDef>, MaterialError> {
        Ok(Vec::new())
    }
}

/// A [`TableSource`] backed by a directory of JSON files — the today seam.
/// Holds only the directory; the filenames are [`PERIODIC_TABLE_FILE`],
/// [`MATERIALS_FILE`], and [`COMPOUNDS_FILE`].
#[derive(Clone, Debug)]
pub struct JsonTableSource {
    dir: PathBuf,
}

impl JsonTableSource {
    /// A source reading the JSON tables from `dir` (e.g. `Alpha/content/data`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Relabel the material with byte id `id` in `materials.json`, keyed STRICTLY by the id —
    /// the `u8` wire value a voxel carries — while preserving the file's `_meta` and every
    /// other row and field. A rename changes one byte's LABEL; it never moves the mapping.
    /// The read seam ([`TableSource::load_materials`]) is unchanged, and a network/DB source
    /// later implements the same write behind its own type.
    pub fn save_material_name(&self, id: MaterialId, name: &str) -> Result<(), MaterialError> {
        let path = self.dir.join(MATERIALS_FILE);
        let display = path.display().to_string();
        // Edit the parsed VALUE in place — only the one row's `name` — so `_meta`, field
        // order, and every other material survive the round-trip untouched.
        let mut root: serde_json::Value = read_json(&path)?;
        let rows = root
            .get_mut("materials")
            .and_then(|m| m.as_array_mut())
            .ok_or_else(|| MaterialError::Schema {
                path: display.clone(),
                detail: "no `materials` array".into(),
            })?;
        let hit = rows.iter_mut().any(|row| {
            if row.get("id").and_then(|v| v.as_u64()) == Some(id as u64) {
                row["name"] = serde_json::Value::String(name.to_string());
                true
            } else {
                false
            }
        });
        if !hit {
            return Err(MaterialError::Schema {
                path: display,
                detail: format!("no material with id {id}"),
            });
        }
        let text = serde_json::to_string_pretty(&root).map_err(|e| MaterialError::Schema {
            path: display.clone(),
            detail: format!("serialising: {e}"),
        })?;
        std::fs::write(&path, text).map_err(|source| MaterialError::Io {
            path: display,
            source,
        })
    }
}

/// Read and parse one JSON table file into its top-level wrapper `T`.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, MaterialError> {
    let bytes = std::fs::read(path).map_err(|source| MaterialError::Io {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| MaterialError::Parse {
        path: path.display().to_string(),
        source,
    })
}

/// Top-level shape of `periodic_table.json` — `_meta` and any other sibling
/// keys are ignored; only `elements` is read.
#[derive(Deserialize)]
struct PeriodicTableFile {
    elements: Vec<Element>,
}

/// Top-level shape of `materials.json` — `_meta` is ignored; only `materials`
/// is read.
#[derive(Deserialize)]
struct MaterialsFile {
    materials: Vec<MaterialDef>,
}

/// Top-level shape of `compounds.json` — `_meta` is ignored; only `compounds`
/// is read.
#[derive(Deserialize)]
struct CompoundsFile {
    compounds: Vec<CompoundDef>,
}

/// Top-level shape of `rocks.json` — `_meta` is ignored; only `rocks` is read.
#[derive(Deserialize)]
struct RocksFile {
    rocks: Vec<RockDef>,
}

impl TableSource for JsonTableSource {
    fn load_elements(&self) -> Result<Vec<Element>, MaterialError> {
        let file: PeriodicTableFile = read_json(&self.dir.join(PERIODIC_TABLE_FILE))?;
        Ok(file.elements)
    }

    fn load_materials(&self) -> Result<Vec<MaterialDef>, MaterialError> {
        let file: MaterialsFile = read_json(&self.dir.join(MATERIALS_FILE))?;
        Ok(file.materials)
    }

    fn load_compounds(&self) -> Result<Vec<CompoundDef>, MaterialError> {
        // Tolerant of a content dir that has no compound catalog yet.
        let path = self.dir.join(COMPOUNDS_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file: CompoundsFile = read_json(&path)?;
        Ok(file.compounds)
    }

    fn load_rocks(&self) -> Result<Vec<RockDef>, MaterialError> {
        // Tolerant of a content dir that has no rock catalog yet.
        let path = self.dir.join(ROCKS_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file: RocksFile = read_json(&path)?;
        Ok(file.rocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The write seam relabels STRICTLY by byte id, preserves `_meta` and every other row,
    /// and a miss is a loud error — never a silent no-op that would drift a wire byte's label.
    #[test]
    fn save_material_name_relabels_by_id_and_preserves_the_rest() {
        let dir = std::env::temp_dir().join("flicker_mat_rename_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(MATERIALS_FILE);
        std::fs::write(
            &path,
            r#"{ "_meta": { "purpose": "keepme" },
                "materials": [
                    { "id": 10, "name": "Granite", "category": "rock", "hardness": 6.5, "brittleness": 0.5, "water_capacity": 0.04, "viscosity": 1.0, "density_g_cm3": 2.7, "color": [0.7,0.68,0.66] },
                    { "id": 11, "name": "Basalt", "category": "rock", "hardness": 6.0, "brittleness": 0.5, "water_capacity": 0.04, "viscosity": 1.0, "density_g_cm3": 3.0, "color": [0.24,0.24,0.26] }
                ] }"#,
        )
        .unwrap();

        let src = JsonTableSource::new(&dir);
        src.save_material_name(10, "PinkGranite").unwrap();

        // The read seam sees the new name on id 10; id 11 and id 10's other fields survive.
        let mats = src.load_materials().unwrap();
        let g = mats.iter().find(|m| m.id == 10).unwrap();
        assert_eq!(g.name, "PinkGranite", "id 10 relabelled");
        assert_eq!(
            g.hardness, 6.5,
            "id 10's other fields survive the round-trip"
        );
        assert_eq!(
            mats.iter().find(|m| m.id == 11).unwrap().name,
            "Basalt",
            "a different id is untouched"
        );
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("keepme"),
            "_meta is preserved"
        );

        // A strict miss is an error, not a silent no-op.
        assert!(
            src.save_material_name(200, "Nope").is_err(),
            "an absent id is a strict error"
        );
    }
}
