//! The loader **seam**: [`TableSource`] abstracts *where* the vocabulary rows
//! come from. The simulation asks a source for elements and materials; it never
//! hardcodes a path. [`JsonTableSource`] is the today implementation, reading
//! `data/materials/*.json`. Later, a flicker-net → web service → DB source
//! implements the same trait and the callers don't change — the JSON-now /
//! network-later seam from the handoff (§2, §8).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::element::Element;
use crate::material::MaterialDef;

/// Filename of the periodic table within a [`JsonTableSource`] directory.
pub const PERIODIC_TABLE_FILE: &str = "periodic_table.json";
/// Filename of the material index within a [`JsonTableSource`] directory.
pub const MATERIALS_FILE: &str = "materials.json";

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
}

/// Where the vocabulary rows come from. Implementors return the raw row lists;
/// [`crate::Tables::from_source`] indexes them into the queryable vocabulary.
pub trait TableSource {
    /// The periodic-table rows.
    fn load_elements(&self) -> Result<Vec<Element>, MaterialError>;
    /// The material-index rows.
    fn load_materials(&self) -> Result<Vec<MaterialDef>, MaterialError>;
}

/// A [`TableSource`] backed by a directory of JSON files — the today seam.
/// Holds only the directory; the filenames are [`PERIODIC_TABLE_FILE`] and
/// [`MATERIALS_FILE`].
#[derive(Clone, Debug)]
pub struct JsonTableSource {
    dir: PathBuf,
}

impl JsonTableSource {
    /// A source reading the two JSON tables from `dir` (e.g. `data/materials`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
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

impl TableSource for JsonTableSource {
    fn load_elements(&self) -> Result<Vec<Element>, MaterialError> {
        let file: PeriodicTableFile = read_json(&self.dir.join(PERIODIC_TABLE_FILE))?;
        Ok(file.elements)
    }

    fn load_materials(&self) -> Result<Vec<MaterialDef>, MaterialError> {
        let file: MaterialsFile = read_json(&self.dir.join(MATERIALS_FILE))?;
        Ok(file.materials)
    }
}
