//! The **`.epoch` file** — a generated world captured as authored data, JSON, the
//! same shape as a `.flight` / `.pack` / `.rig`: a `format`/`version` header, an
//! optional `_comment`, then the payload. Here the payload is a world's full
//! **input recipe** ([`WorldConfig`]) plus the immutable per-epoch
//! [`EpochSnapshot`]s it produced — so a finished planet can be handed off,
//! diffed, or reloaded without re-running the sim, and (because the recipe rides
//! along) regenerated bit-for-bit if you'd rather.
//!
//! ```json
//! {
//!   "format": "flicker.epoch", "version": 1,
//!   "_comment": "Earth-like sample world, freq 6.",
//!   "config": { "values": { "e3_plates": 8.0, ... }, "freq": 6, "seed": 247276033 },
//!   "snapshots": [ { "epoch": 1, "cells": [ ... ], "provenance": { ... } }, ... ]
//! }
//! ```
//!
//! Load/parse/validate mirror `flicker_flight::Flight` so the two file types feel
//! the same. `#[serde(default)]` on the header + snapshot fields keeps it
//! forward-tolerant: an older `.epoch` missing a field the schema has since grown
//! still loads.

use std::path::Path;

use flicker_core::compression;
use serde::{Deserialize, Serialize};

use crate::config::{WorldConfig, WORLD_EPOCHS};
use crate::snapshot::EpochSnapshot;

/// The `format` tag every `.epoch` carries (mirrors `"flicker.flight"`).
pub const EPOCH_FORMAT: &str = "flicker.epoch";
/// The current `.epoch` schema version.
pub const EPOCH_VERSION: u32 = 1;

/// A load/parse/validate failure — names the file where it can.
#[derive(Debug, thiserror::Error)]
pub enum EpochFileError {
    #[error("reading .epoch {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("parsing .epoch {path}: {source}")]
    Parse {
        path: String,
        source: serde_json::Error,
    },
    #[error("serialising .epoch: {0}")]
    Serialize(serde_json::Error),
    #[error("invalid .epoch: {0}")]
    Invalid(String),
}

/// A parsed `.epoch` — a captured world (recipe + per-epoch snapshots).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochFile {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    /// Human note (ignored by loading) — the `_comment` convention.
    #[serde(default, rename = "_comment")]
    pub comment: String,
    /// The input recipe that produced the snapshots.
    pub config: WorldConfig,
    /// The captured per-epoch snapshots, in epoch order.
    pub snapshots: Vec<EpochSnapshot>,
}

impl EpochFile {
    /// Wrap a recipe + snapshots with the current header.
    pub fn new(
        config: WorldConfig,
        snapshots: Vec<EpochSnapshot>,
        comment: impl Into<String>,
    ) -> Self {
        Self {
            format: EPOCH_FORMAT.to_string(),
            version: EPOCH_VERSION,
            comment: comment.into(),
            config,
            snapshots,
        }
    }

    /// Parse and validate a `.epoch` from JSON text.
    pub fn from_json(json: &str) -> Result<Self, EpochFileError> {
        let file: EpochFile =
            serde_json::from_str(json).map_err(|source| EpochFileError::Parse {
                path: "<memory>".to_string(),
                source,
            })?;
        file.validate()?;
        Ok(file)
    }

    /// Compact JSON text for this `.epoch` — what [`save`](Self::save) writes. A
    /// world is machine-generated (not hand-edited like a `.flight`) and the per-cell
    /// snapshots are large, so compact keeps the file small; use
    /// [`to_json_pretty`](Self::to_json_pretty) when a human needs to read it.
    pub fn to_json(&self) -> Result<String, EpochFileError> {
        serde_json::to_string(self).map_err(EpochFileError::Serialize)
    }

    /// Indented JSON text — readable, but several times larger than [`to_json`].
    pub fn to_json_pretty(&self) -> Result<String, EpochFileError> {
        serde_json::to_string_pretty(self).map_err(EpochFileError::Serialize)
    }

    /// Read and parse a `.epoch` file through the shared gz-at-rest seam
    /// (`flicker_core::compression`): a `.gz` path is transparently gunzipped,
    /// a logical `.epoch` path resolves its `.gz` twin first (the package
    /// convention, same as `bakes/*.json.gz`), and a loose file still reads.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, EpochFileError> {
        let path = path.as_ref();
        let text = compression::read_text(path).map_err(|source| EpochFileError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let file: EpochFile =
            serde_json::from_str(&text).map_err(|source| EpochFileError::Parse {
                path: path.display().to_string(),
                source,
            })?;
        file.validate()?;
        Ok(file)
    }

    /// Write this `.epoch` to `path` as compact JSON (creating parent dirs). A `.gz`
    /// extension gzips it.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), EpochFileError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| EpochFileError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let json = self.to_json()?;
        let bytes = if compression::names_gz(path) {
            compression::compress_gzip(json.as_bytes())
        } else {
            json.into_bytes()
        };
        std::fs::write(path, bytes).map_err(|source| EpochFileError::Io {
            path: path.display().to_string(),
            source,
        })
    }

    fn validate(&self) -> Result<(), EpochFileError> {
        if self.snapshots.is_empty() {
            return Err(EpochFileError::Invalid("no snapshots".into()));
        }
        // Every snapshot spans the same planet (same cell count) and names a valid
        // epoch; snapshots are stored in ascending epoch order.
        let cells = self.snapshots[0].len();
        let mut last = 0u8;
        for s in &self.snapshots {
            if s.epoch < 1 || s.epoch as usize > WORLD_EPOCHS {
                return Err(EpochFileError::Invalid(format!(
                    "epoch {} out of range",
                    s.epoch
                )));
            }
            if s.len() != cells {
                return Err(EpochFileError::Invalid(format!(
                    "epoch {} has {} cells, expected {cells}",
                    s.epoch,
                    s.len()
                )));
            }
            if s.epoch <= last {
                return Err(EpochFileError::Invalid(
                    "snapshots out of epoch order".into(),
                ));
            }
            last = s.epoch;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorldEngine;

    #[test]
    fn captures_round_trip_through_json_and_disk() {
        // A small world is enough to exercise the format end to end.
        let mut engine = WorldEngine::from_repo().expect("engine from repo content");
        engine.set_freq(6);
        let file = engine.capture("test world");
        assert_eq!(file.format, EPOCH_FORMAT);
        assert_eq!(
            file.snapshots.len(),
            WORLD_EPOCHS,
            "all nine epochs captured"
        );

        // JSON round-trip preserves the recipe and per-cell state.
        let json = file.to_json().unwrap();
        let back = EpochFile::from_json(&json).unwrap();
        assert_eq!(back.config, file.config);
        assert_eq!(back.snapshots.len(), file.snapshots.len());
        assert_eq!(back.snapshots[0].cells, file.snapshots[0].cells);
        assert_eq!(
            back.snapshots[5].provenance.conserved_mass,
            file.snapshots[5].provenance.conserved_mass
        );

        // Disk round-trip, plain and gzipped — both restore identical cells, and
        // the gzip is markedly smaller.
        let dir = std::env::temp_dir().join("flicker_epochfile_test");
        let plain = dir.join("sample.epoch");
        let gz = dir.join("sample.epoch.gz");
        file.save(&plain).unwrap();
        file.save(&gz).unwrap();
        let from_plain = EpochFile::load(&plain).unwrap();
        let from_gz = EpochFile::load(&gz).unwrap();
        assert_eq!(from_plain.snapshots[0].cells, file.snapshots[0].cells);
        assert_eq!(from_gz.snapshots[0].cells, file.snapshots[0].cells);
        let (sz_plain, sz_gz) = (
            std::fs::metadata(&plain).unwrap().len(),
            std::fs::metadata(&gz).unwrap().len(),
        );
        assert!(
            sz_gz < sz_plain,
            "gzip ({sz_gz}) should be smaller than plain ({sz_plain})"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_empty_and_out_of_range() {
        assert!(EpochFile::from_json(
            r#"{ "config": { "values": {}, "freq": 6, "seed": 1 }, "snapshots": [] }"#
        )
        .is_err());
    }
}
