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
use crate::material::{MaterialDef, MaterialId, RenderClass, EMISSIVE_FIRST};
use crate::rock::RockDef;

/// Filename of the periodic table within a [`JsonTableSource`] directory.
pub const PERIODIC_TABLE_FILE: &str = "periodic_table.json";
/// Filename of the material index within a [`JsonTableSource`] directory.
pub const MATERIALS_FILE: &str = "materials.json";
/// Filename of the compound catalog within a [`JsonTableSource`] directory.
pub const COMPOUNDS_FILE: &str = "compounds.json";
/// Sibling of [`COMPOUNDS_FILE`] holding the mantle + world-formation-simulation
/// compounds (ids 79-96, split out 2026-08-19). Loaded and appended into the SAME
/// catalog: the two files share one id space, so a consumer resolves any compound
/// by name/id regardless of which file it came from.
pub const CRUST_COMPOUNDS_FILE: &str = "crust_compounds.json";
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
    /// A material row sits in a slot that may not hold its render class
    /// ([`crate::slot_accepts`]): slot 0 is Air and holds nothing, and the emissive
    /// slots ([`EMISSIVE_FIRST`]`..=255`) hold emissive materials only.
    #[error(
        "material {id} ({name}) is {class:?}, which slot {id} may not hold — slot 0 is Air, \
         and slots {first}-255 hold only emissive materials",
        first = EMISSIVE_FIRST
    )]
    SlotRefused {
        id: MaterialId,
        name: String,
        class: RenderClass,
    },
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

/// One AUTHORING BAND of the material byte — an entry of `materials.json`'s
/// `_meta.bands`: the run of ids a kind of material is filed under
/// (`"10-19": "10-19 ROCKS: igneous + the common three"`), so a new row lands beside its
/// kin and a catalog reads in rows. The index's own documentation, not a loader gate (the
/// file says so): nothing refuses a row outside its band.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialBand {
    /// The band's first and last id, inclusive (`"0"` is the one-id band `0..=0`).
    pub first: MaterialId,
    pub last: MaterialId,
    /// The band's label exactly as `materials.json` words it — content, never rewritten.
    pub label: String,
}

impl MaterialBand {
    /// Whether `id` falls in this band.
    #[must_use]
    pub fn holds(&self, id: MaterialId) -> bool {
        (self.first..=self.last).contains(&id)
    }
}

/// The band whose range holds `id`, if any band does.
#[must_use]
pub fn band_of(bands: &[MaterialBand], id: MaterialId) -> Option<&MaterialBand> {
    bands.iter().find(|b| b.holds(id))
}

impl JsonTableSource {
    /// The authoring bands of the material byte — `materials.json` `_meta.bands`, sorted by
    /// first id. `_`-prefixed keys are the block's own comments and are skipped.
    ///
    /// Strict, because a band is read to TELL someone where an id is filed: a key that is
    /// not `"<id>"` or `"<first>-<last>"` over `0..=255`, a label that is not text, or two
    /// bands claiming one id is a [`MaterialError::Schema`] naming it — never a band
    /// silently dropped or an id silently filed twice. A file with no `_meta.bands` has no
    /// bands (an empty list), which is not an error.
    pub fn load_material_bands(&self) -> Result<Vec<MaterialBand>, MaterialError> {
        let path = self.dir.join(MATERIALS_FILE);
        let display = path.display().to_string();
        let root: serde_json::Value = read_json(&path)?;
        let Some(bands) = root.get("_meta").and_then(|m| m.get("bands")) else {
            return Ok(Vec::new());
        };
        let schema = |detail: String| MaterialError::Schema {
            path: display.clone(),
            detail,
        };
        let bands = bands
            .as_object()
            .ok_or_else(|| schema("`_meta.bands` is not an object".into()))?;
        let id = |s: &str| s.trim().parse::<MaterialId>().ok();
        let mut out = Vec::new();
        for (key, label) in bands {
            if key.starts_with('_') {
                continue;
            }
            let range = match key.split_once('-') {
                Some((a, b)) => id(a).zip(id(b)),
                None => id(key).map(|a| (a, a)),
            };
            let Some((first, last)) = range.filter(|(a, b)| a <= b) else {
                return Err(schema(format!(
                    "`_meta.bands` key `{key}` is not an id or an id range over 0-255"
                )));
            };
            let label = label
                .as_str()
                .ok_or_else(|| schema(format!("`_meta.bands.{key}` is not text")))?;
            out.push(MaterialBand {
                first,
                last,
                label: label.to_string(),
            });
        }
        out.sort_by_key(|b| b.first);
        if let Some(pair) = out.windows(2).find(|w| w[1].first <= w[0].last) {
            return Err(schema(format!(
                "`_meta.bands` files id {} under both `{}` and `{}`",
                pair[1].first, pair[0].label, pair[1].label
            )));
        }
        Ok(out)
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

/// Top-level shape of the compound-catalog files (`compounds.json` and its
/// sibling `crust_compounds.json`) — `_meta` is ignored; only `compounds` is read.
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
        // The catalog is split across two sibling files that share ONE id space:
        // `compounds.json` (gameplay rows 1-78) and `crust_compounds.json` (the
        // mantle + world-formation-sim rows 79-96). Both are appended into the same
        // catalog so every consumer resolves any compound by name/id, wherever it
        // lives. Each is tolerant of absence — a content dir that predates either
        // file still loads (a network/DB source later merges the same way).
        let mut compounds = Vec::new();
        for file in [COMPOUNDS_FILE, CRUST_COMPOUNDS_FILE] {
            let path = self.dir.join(file);
            if !path.exists() {
                continue;
            }
            let parsed: CompoundsFile = read_json(&path)?;
            compounds.extend(parsed.compounds);
        }
        Ok(compounds)
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

    /// The SHIPPED bands file every id of the byte exactly once, in order, each with its
    /// label as `materials.json` words it — so "which band is slot N filed under" always has
    /// exactly one answer.
    #[test]
    fn the_shipped_bands_file_every_id_exactly_once() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/data");
        let bands = JsonTableSource::new(dir)
            .load_material_bands()
            .expect("the shipped bands read");
        assert!(!bands.is_empty(), "materials.json authors its bands");
        for id in 0..=MaterialId::MAX {
            let holding = bands.iter().filter(|b| b.holds(id)).count();
            assert_eq!(holding, 1, "id {id} is filed under {holding} bands");
        }
        let rocks = band_of(&bands, 10).expect("10 is filed");
        assert!(
            rocks.label.starts_with("10-19"),
            "the label is the file's own: {}",
            rocks.label
        );
        assert_eq!((rocks.first, rocks.last), (10, 19));
    }

    /// A malformed bands block is LOUD: a key that is not an id range, an overlap, a label
    /// that is not text — each a schema error naming it. A file with no bands has none.
    #[test]
    fn a_malformed_bands_block_is_refused_loudly() {
        let dir =
            std::env::temp_dir().join(format!("flicker_mat_bands_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = JsonTableSource::new(&dir);
        let with = |bands: &str| {
            std::fs::write(
                dir.join(MATERIALS_FILE),
                format!(r#"{{ "_meta": {{ "bands": {bands} }}, "materials": [] }}"#),
            )
            .unwrap();
            src.load_material_bands()
        };
        assert!(with(r#"{ "1-9": "a", "_note": "skipped" }"#).is_ok());
        assert!(with(r#"{ "ten": "a" }"#).is_err(), "not an id");
        assert!(with(r#"{ "9-1": "a" }"#).is_err(), "a backwards range");
        assert!(with(r#"{ "0-300": "a" }"#).is_err(), "past the byte");
        assert!(
            with(r#"{ "1-9": "a", "5-12": "b" }"#).is_err(),
            "an overlap"
        );
        assert!(with(r#"{ "1-9": 3 }"#).is_err(), "a label that is not text");
        std::fs::write(dir.join(MATERIALS_FILE), r#"{ "materials": [] }"#).unwrap();
        assert_eq!(
            src.load_material_bands().expect("no bands is not an error"),
            Vec::new()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
