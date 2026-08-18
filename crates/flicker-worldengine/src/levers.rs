//! Generator **levers** — the authored, data-driven knobs of world generation,
//! loaded from `Alpha/content/data` behind a swappable [`GeneratorParamsSource`]
//! (JSON now, a DB/web source later) exactly like [`flicker_materials::Tables`]
//! loads the vocabulary behind `TableSource`.
//!
//! Two tables:
//! - **`epoch_defaults.json`** — one [`LeverDef`] per tunable knob: its owning
//!   epoch, its stable string id (the same id the Lua HUD and the viewer's slider
//!   layout key on), its Earth-like `default`, and the `[min, max]` band a slider
//!   spans / a reseed wanders within. This is the old hardcoded `PARAM_DEFS` table
//!   lifted out of Rust source into content.
//! - **`abundance.json`** — one [`AbundanceDef`] per Epoch-1 element: its symbol and
//!   starting relative abundance weight (the old hardcoded `ABUNDANCE_DEFS`).
//!
//! Both files lead with a `_meta` object (provenance/notes) that the loader
//! ignores, and both tolerate unknown fields, so the schema can grow — the same
//! forward-tolerance convention the material tables use.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The two content-data filenames (siblings of `periodic_table.json`).
const EPOCH_DEFAULTS_FILE: &str = "epoch_defaults.json";
const ABUNDANCE_FILE: &str = "abundance.json";

/// One tunable generator knob. `id` is the contract shared by the config
/// ([`crate::WorldConfig`] reads it), the viewer HUD (publishes/edits it), and the
/// `ui_theme.json` slider layout (whose range must match `[min, max]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeverDef {
    /// Owning epoch (1..=9). Epoch 1 also owns the element mix (see [`AbundanceDef`]).
    pub epoch: u8,
    /// Stable knob id, e.g. `"e3_mountain_uplift"`.
    pub id: String,
    /// Earth-like starting value (drives [`crate::WorldConfig::default`]).
    pub default: f64,
    /// Low end of the reasonable band (slider min / reseed floor).
    pub min: f64,
    /// High end of the reasonable band (slider max / reseed ceiling).
    pub max: f64,
    /// Optional human label / note — ignored by generation, handy in the file.
    #[serde(default)]
    pub note: String,
}

/// One Epoch-1 element's starting relative abundance. Weights are relative
/// (renormalised per cell), so the value is just "how much this element competes".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbundanceDef {
    /// Element symbol (must exist in `periodic_table.json`).
    pub symbol: String,
    /// Starting relative abundance weight.
    pub weight: f64,
}

/// A load/parse failure that names the offending file — mirrors
/// [`flicker_materials::MaterialError`].
#[derive(Debug, thiserror::Error)]
pub enum GeneratorError {
    #[error("reading generator data {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing generator data {path}: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
}

/// The swappable data-source seam for generator levers — implement this for a new
/// backend (DB / web service) and no caller changes, exactly as [`TableSource`]
/// does for the material vocabulary.
///
/// [`TableSource`]: flicker_materials::TableSource
pub trait GeneratorParamsSource {
    fn load_levers(&self) -> Result<Vec<LeverDef>, GeneratorError>;
    fn load_abundance(&self) -> Result<Vec<AbundanceDef>, GeneratorError>;
}

/// Wrapper structs matching the top-level JSON object; `_meta` and any unknown
/// sibling keys are ignored (forward-tolerant, like the material tables).
#[derive(Deserialize)]
struct EpochDefaultsFile {
    levers: Vec<LeverDef>,
}

#[derive(Deserialize)]
struct AbundanceFile {
    elements: Vec<AbundanceDef>,
}

/// Today's source: reads the two JSON tables from `dir` (e.g. `Alpha/content/data`).
pub struct JsonGeneratorSource {
    dir: PathBuf,
}

impl JsonGeneratorSource {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, file: &str) -> Result<T, GeneratorError> {
        let path = self.dir.join(file);
        let text = std::fs::read_to_string(&path).map_err(|source| GeneratorError::Io {
            path: path.display().to_string(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| GeneratorError::Parse {
            path: path.display().to_string(),
            source,
        })
    }
}

impl GeneratorParamsSource for JsonGeneratorSource {
    fn load_levers(&self) -> Result<Vec<LeverDef>, GeneratorError> {
        let f: EpochDefaultsFile = self.read(EPOCH_DEFAULTS_FILE)?;
        Ok(f.levers)
    }

    fn load_abundance(&self) -> Result<Vec<AbundanceDef>, GeneratorError> {
        let f: AbundanceFile = self.read(ABUNDANCE_FILE)?;
        Ok(f.elements)
    }
}

/// The loaded, indexed lever tables — the in-memory form of the two content files,
/// built once from a source and read-only after. The engine holds one and consults
/// it to seed defaults, clamp edits, and drive per-epoch reseed jitter.
#[derive(Clone, Debug)]
pub struct GeneratorParams {
    levers: Vec<LeverDef>,
    abundance: Vec<AbundanceDef>,
}

impl GeneratorParams {
    /// Build from any source (the one construction path).
    pub fn from_source(source: &impl GeneratorParamsSource) -> Result<Self, GeneratorError> {
        Ok(Self {
            levers: source.load_levers()?,
            abundance: source.load_abundance()?,
        })
    }

    /// Build from an explicit content directory (e.g. `Alpha/content/data`).
    pub fn from_dir(dir: impl Into<PathBuf>) -> Result<Self, GeneratorError> {
        Self::from_source(&JsonGeneratorSource::new(dir))
    }

    /// Build from the repo's `Alpha/content/data` (relative to this crate) — the
    /// convenience the tests and the headless bake tool use. Real callers pass a
    /// source so the data home stays swappable.
    pub fn from_repo() -> Result<Self, GeneratorError> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Self::from_dir(dir)
    }

    /// Build directly from rows (tests / non-JSON), bypassing a source.
    pub fn from_rows(levers: Vec<LeverDef>, abundance: Vec<AbundanceDef>) -> Self {
        Self { levers, abundance }
    }

    /// Every lever, in file order.
    pub fn levers(&self) -> &[LeverDef] {
        &self.levers
    }

    /// Every abundance entry, in file order (the Epoch-1 element competition list).
    pub fn abundance(&self) -> &[AbundanceDef] {
        &self.abundance
    }

    /// The lever with this id, if defined.
    pub fn lever(&self, id: &str) -> Option<&LeverDef> {
        self.levers.iter().find(|l| l.id == id)
    }
}

/// The content dir path used by [`GeneratorParams::from_repo`] and the material
/// tables alike — exposed so consumers needn't re-spell the `/../../` prefix.
pub fn repo_content_dir() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../Alpha/content/data"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_tables_load_and_index() {
        let p = GeneratorParams::from_repo().expect("Alpha/content/data generator tables load");
        assert!(!p.levers().is_empty(), "no levers loaded");
        assert!(!p.abundance().is_empty(), "no abundance entries loaded");
        // The canonical Epoch-1 mix leads with oxygen.
        assert!(p.abundance().iter().any(|a| a.symbol == "O"));
        // A known lever resolves with a sane band.
        let l = p
            .lever("e3_mountain_uplift")
            .expect("e3_mountain_uplift defined");
        assert!(l.min <= l.default && l.default <= l.max);
        assert_eq!(l.epoch, 3);
    }
}
