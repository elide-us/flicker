//! The **`.epoch` file** — a planet captured as authored data, JSON, the same
//! shape as a `.flight` / `.pack` / `.rig`: a `format`/`version` header, an
//! optional `_comment`, then the payload.
//!
//! **Version 2 — the PLANET EPOCH ([`PlanetEpoch`]) — is the live format**: the
//! Populous Bench's output, the base planet the planet-scale erosion-cycle
//! simulation starts from (the hex-stack ledger, Aaron 2026-08-25/28). Its
//! payload is a **recipe** (the seeds and counts that regenerate the static
//! context — map, molten seams, crust — bit-for-bit) plus the **era ledger**:
//! the path-dependent per-hex state the evolution ticks accumulated, which no
//! seed can replay. Load a v2 and the bench (or any successor sim) stands the
//! same world back up without re-running the era.
//!
//! ```json
//! {
//!   "format": "flicker.epoch", "version": 2,
//!   "_comment": "Populous world, freq 96, 3600 ticks.",
//!   "recipe": { "freq": 96, "seed": 247276033, "cells": 12, "spots": 12 },
//!   "era":    { "ticks": 3600, "water_volume": 1.94, ... },
//!   "ledger": { "base": [ ... ], "rock": [ ... ], ... },
//!   "air":    [ [ ... ], [ ... ], [ ... ] ],
//!   "emitted": [ ... ],
//!   "veins":  [ { "center": 88, "kind": 3, "size": 12, "budget": 30 }, ... ]
//! }
//! ```
//!
//! **Version 1 ([`EpochFile`]) is the LEGACY shape** — the retired chemistry-sim
//! capture (`WorldConfig` + per-epoch [`EpochSnapshot`]s). It stays only because
//! its producer (the retired `WorldEngine` capture path) is kept for the sim-crate
//! review (decision 05F626F8); nothing live writes it. Each version's loader
//! REJECTS the other's files loud (rule 7C46FAC4: an old reader must fail loud,
//! never mis-read).
//!
//! Load/parse/validate mirror `flicker_flight::Flight` so the file types feel
//! the same, and both ride the shared gz-at-rest seam.

use std::path::Path;

use flicker_core::compression;
use serde::{Deserialize, Serialize};

use crate::config::{WorldConfig, WORLD_EPOCHS};
use crate::snapshot::EpochSnapshot;

/// The `format` tag every `.epoch` carries (mirrors `"flicker.flight"`).
pub const EPOCH_FORMAT: &str = "flicker.epoch";
/// The current `.epoch` schema version — the PLANET EPOCH ([`PlanetEpoch`]).
pub const EPOCH_VERSION: u32 = 2;
/// The legacy chemistry-sim capture's version ([`EpochFile`]).
pub const LEGACY_EPOCH_VERSION: u32 = 1;

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

/// A parsed LEGACY v1 `.epoch` — the retired chemistry sim's captured world
/// (recipe + per-epoch snapshots). Kept only while that sim awaits its review;
/// the live planet format is [`PlanetEpoch`] (v2), and this loader rejects v2
/// files loud.
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
            version: LEGACY_EPOCH_VERSION,
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
        // Version-fenced BOTH ways (rule 7C46FAC4): this legacy loader must
        // refuse a v2 planet epoch loud, not mis-read it.
        if self.version > LEGACY_EPOCH_VERSION {
            return Err(EpochFileError::Invalid(format!(
                "version {} is a planet epoch — read it with PlanetEpoch",
                self.version
            )));
        }
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

// ─────────────────────────── THE PLANET EPOCH (v2) ───────────────────────────

/// **The recipe** — what regenerates the planet's STATIC context bit-for-bit:
/// the hex map at `freq`, the molten seam field rolled with `seed` at `cells`
/// convection cells and `spots` plumes, and everything derived from those (the
/// crust's vents, the winds, the microclimate jitter — all seeded streams off
/// the same roll). The era's dials ride in [`PlanetEra`]; only identity lives
/// here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanetRecipe {
    /// The hex grid's subdivision frequency (`10·freq² + 2` tiles).
    pub freq: u32,
    /// The roll that placed the seam field's seeds — the world's one seed.
    pub seed: u64,
    /// How many convection cells the molten field was rolled with.
    pub cells: u32,
    /// How many mantle plumes.
    pub spots: u32,
}

/// **The era's global state** — every planet-wide scalar the evolution ticks
/// accumulated: the clocks, the conserved water, the climate machine and the
/// dial settings. One struct so a reader sees the planet's globals in one
/// place instead of scattered through the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanetEra {
    /// Completed procedure cycles.
    pub ticks: u64,
    /// Eruptions fired, plate steps advanced, safety-net heals — the era's
    /// event counters.
    pub eruptions: u64,
    pub steps: u64,
    pub heals: u64,
    /// The CONSERVED water volume (area-weighted height units) and the share
    /// of it locked in the ice caps.
    pub water_volume: f32,
    pub ice_locked: f32,
    /// The climate machine: the dial baseline, the live temperature, the one
    /// deep-ocean reservoir and the volcanic-greenhouse lift.
    pub climate_base: f32,
    pub temp: f32,
    pub deep_temp: f32,
    pub greenhouse: f32,
    /// The coverage target the in-fall pursues and the green target the flora
    /// pursues, with the flora's adapted thirst and last measured share.
    pub water_target: f32,
    pub veg_target: f32,
    pub veg_thirst: f32,
    pub green_share: f32,
    /// Whether the bootstrap-horizon resource guarantee has run.
    pub resources_ensured: bool,
    /// **THE SUBDUCTION WELL** — the aggregate volume the crust's
    /// convergences have sunk and the vents have not yet spent back, with
    /// the cumulative take and the basal ceiling's fire count beside it.
    /// Durable: a planet restored without it would resume minting.
    #[serde(default)]
    pub well: f32,
    #[serde(default)]
    pub sunk: f32,
    #[serde(default)]
    pub delaminations: u64,
}

/// **The per-hex ledger** — parallel arrays, index = tile id, length =
/// `cell_count(freq)` for every one. The path-dependent truth the era built:
/// the solid stack, the crust-edge state, the standing fluids and the veins.
/// Derived fields (the push, the winds, cell areas, seeded jitters) are NOT
/// here — the recipe regenerates them exactly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanetLedger {
    // ── the solid stack, bottom up (heights in tile-width units) ──
    /// The era's welded base above the shell (L2).
    pub base: Vec<f32>,
    /// The formed strata: L3 (vein/marine layer) and L4 (volcanic layer),
    /// each with the hardness grade it consolidated at.
    pub l3_h: Vec<f32>,
    pub l3_hard: Vec<f32>,
    pub l4_h: Vec<f32>,
    pub l4_hard: Vec<f32>,
    /// **THE STRATA'S FABRIC** — the bedding attitude the era's deformation
    /// recorded: the column's strike azimuth (radians in the tile's
    /// east/north tangent frame, an undirected `[0, π)` line) and each formed
    /// slot's dip. ADDITIVE: an epoch written before the fabric existed omits
    /// these entirely, and loads as a NULL-FABRIC world — dip 0 everywhere,
    /// which erodes exactly as that world always did. Present arrays span the
    /// recipe's tiles like every other; empty is the only other legal length.
    #[serde(default)]
    pub strike: Vec<f32>,
    #[serde(default)]
    pub l3_dip: Vec<f32>,
    #[serde(default)]
    pub l4_dip: Vec<f32>,
    /// **THE GRADED LEVEL** — per column, the outlet elevation it has
    /// adjusted to (Phase 2, mobile base levels). A CONTROL variable, not a
    /// store: it holds no material and the conservation ledger never counts
    /// it, but it is PATH-DEPENDENT — a knickzone still climbing the
    /// drainage is exactly the gap between this and the live outlet, and a
    /// planet restored without it would silently arrive at grade.
    /// ADDITIVE, like the fabric: an epoch written before the front existed
    /// omits it and loads UN-MET, adopting each column's live outlet on the
    /// first stream pass — a world at grade, which is what that planet was.
    #[serde(default)]
    pub graded: Vec<f32>,
    /// **THE DISSOLVED LOAD** — per column, bed that has gone into solution
    /// and has not come back out (area-weighted volume, no height). Water's
    /// second denomination (Phase 3, dissolution): part of the conserved
    /// material ledger, so a planet restored without it would silently lose
    /// whatever was in transit. ADDITIVE, like the fabric and the graded
    /// level: an epoch written before the channel existed omits it and loads
    /// with an EMPTY store — which is exactly what that planet had.
    #[serde(default)]
    pub dissolved: Vec<f32>,
    /// Loose volcanic rock and its mass-blended hardness.
    pub rock: Vec<f32>,
    pub rock_hard: Vec<f32>,
    /// Loose sediment in transit — the softest thing on any column.
    pub sediment: Vec<f32>,
    /// The marine-compaction grade of the consolidated bed (never relaxes).
    pub bed_hard: Vec<f32>,
    // ── crust-edge and transport state ──
    /// Collision pressure, the tracked collision-edge intensity (EMA) and its
    /// consecutive-persistence age.
    pub pressure: Vec<f32>,
    pub edge: Vec<f32>,
    pub edge_age: Vec<u32>,
    /// Accumulated travel along the push (the local step ratchet).
    pub drift: Vec<f32>,
    /// Water-borne suspended load the ground refused (no height, re-enters
    /// the flow next tick).
    pub suspend: Vec<f32>,
    // ── the standing fluids ──
    /// The ice cap on this column and the ocean surface band's temperature.
    pub ice: Vec<f32>,
    pub sst: Vec<f32>,
    /// Last tick's discharge down the stream network (state for the carving).
    pub discharge: Vec<f32>,
    // ── the vapor/biosphere skin ──
    /// Near-ground moisture, the rain ledger (EMA) and vegetation cover.
    pub moist: Vec<f32>,
    pub rain: Vec<f32>,
    pub veg: Vec<f32>,
    // ── the veins ──
    /// Per-tile vein: 0 = none, else `1 + index` into the compound registry's
    /// vein kinds. `vein_node_of`: 0 = none, else `1 + index` into `veins`.
    pub vein: Vec<u8>,
    pub vein_node_of: Vec<u16>,
}

/// One nucleated vein BODY — centre tile, registry kind, grown size and the
/// seeded budget that bounds it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VeinBody {
    pub center: u32,
    pub kind: u8,
    pub size: u16,
    pub budget: u16,
}

/// A parsed v2 `.epoch` — **the planet epoch**: the Populous Bench's output,
/// the base planet the erosion-cycle simulation starts from. Recipe + era
/// globals + per-hex ledger + the air column + per-vent emission phases + the
/// vein bodies. Save/load ride the same gz-at-rest seam as everything else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanetEpoch {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    /// Human note (ignored by loading) — the `_comment` convention.
    #[serde(default, rename = "_comment")]
    pub comment: String,
    pub recipe: PlanetRecipe,
    pub era: PlanetEra,
    pub ledger: PlanetLedger,
    /// In-flight moisture per atmospheric layer per tile (layer count is the
    /// writer's — every layer must span the planet).
    pub air: Vec<Vec<f32>>,
    /// Cumulative emission per crust vent, in the crust's own vent order —
    /// the phase of each vent's slow output drift (length = the vent count
    /// the recipe derives; validated on restore, not here).
    pub emitted: Vec<f32>,
    /// Every nucleated vein body, in nucleation order.
    pub veins: Vec<VeinBody>,
}

impl PlanetEpoch {
    /// Wrap a captured planet with the current header.
    pub fn new(
        recipe: PlanetRecipe,
        era: PlanetEra,
        ledger: PlanetLedger,
        air: Vec<Vec<f32>>,
        emitted: Vec<f32>,
        veins: Vec<VeinBody>,
        comment: impl Into<String>,
    ) -> Self {
        Self {
            format: EPOCH_FORMAT.to_string(),
            version: EPOCH_VERSION,
            comment: comment.into(),
            recipe,
            era,
            ledger,
            air,
            emitted,
            veins,
        }
    }

    /// Parse and validate a planet epoch from JSON text.
    pub fn from_json(json: &str) -> Result<Self, EpochFileError> {
        let file: PlanetEpoch =
            serde_json::from_str(json).map_err(|source| EpochFileError::Parse {
                path: "<memory>".to_string(),
                source,
            })?;
        file.validate()?;
        Ok(file)
    }

    /// Compact JSON text — what [`save`](Self::save) writes. A planet is
    /// machine-generated and the ledger is large, so compact keeps it small.
    pub fn to_json(&self) -> Result<String, EpochFileError> {
        serde_json::to_string(self).map_err(EpochFileError::Serialize)
    }

    /// Read and parse through the shared gz-at-rest seam — a `.gz` path
    /// gunzips, a logical `.epoch` path resolves its `.gz` twin first.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, EpochFileError> {
        let path = path.as_ref();
        let text = compression::read_text(path).map_err(|source| EpochFileError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let file: PlanetEpoch =
            serde_json::from_str(&text).map_err(|source| EpochFileError::Parse {
                path: path.display().to_string(),
                source,
            })?;
        file.validate()?;
        Ok(file)
    }

    /// Write as compact JSON (creating parent dirs); a `.gz` extension gzips.
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
        // Version-fenced BOTH ways: a legacy v1 (or anything else) fails loud
        // here rather than being half-read.
        if self.version != EPOCH_VERSION {
            return Err(EpochFileError::Invalid(format!(
                "version {} is not a planet epoch (v{EPOCH_VERSION}); a v1 is the \
                 legacy chemistry capture — read it with EpochFile",
                self.version
            )));
        }
        if self.recipe.freq == 0 {
            return Err(EpochFileError::Invalid("recipe.freq is 0".into()));
        }
        // Every per-tile array spans the planet the recipe names — the
        // Goldberg count is worldgrid's own statement of the formula.
        let tiles = flicker_worldgrid::cell_count(self.recipe.freq);
        let l = &self.ledger;
        for (name, len) in [
            ("base", l.base.len()),
            ("l3_h", l.l3_h.len()),
            ("l3_hard", l.l3_hard.len()),
            ("l4_h", l.l4_h.len()),
            ("l4_hard", l.l4_hard.len()),
            ("rock", l.rock.len()),
            ("rock_hard", l.rock_hard.len()),
            ("sediment", l.sediment.len()),
            ("bed_hard", l.bed_hard.len()),
            ("pressure", l.pressure.len()),
            ("edge", l.edge.len()),
            ("edge_age", l.edge_age.len()),
            ("drift", l.drift.len()),
            ("suspend", l.suspend.len()),
            ("ice", l.ice.len()),
            ("sst", l.sst.len()),
            ("discharge", l.discharge.len()),
            ("moist", l.moist.len()),
            ("rain", l.rain.len()),
            ("veg", l.veg.len()),
            ("vein", l.vein.len()),
            ("vein_node_of", l.vein_node_of.len()),
        ] {
            if len != tiles {
                return Err(EpochFileError::Invalid(format!(
                    "ledger.{name} has {len} tiles, freq {} needs {tiles}",
                    self.recipe.freq
                )));
            }
        }
        // The FABRIC arrays (Phase 1), the GRADED level (Phase 2) and the
        // DISSOLVED store (Phase 3) are additive: an epoch from before any
        // of them omits it and restores null-fabric / un-met / with nothing
        // in solution, so EMPTY is legal — any other length is a corrupt
        // file and fails exactly as loud as the rest.
        for (name, len) in [
            ("strike", l.strike.len()),
            ("l3_dip", l.l3_dip.len()),
            ("l4_dip", l.l4_dip.len()),
            ("graded", l.graded.len()),
            ("dissolved", l.dissolved.len()),
        ] {
            if len != 0 && len != tiles {
                return Err(EpochFileError::Invalid(format!(
                    "ledger.{name} has {len} tiles, freq {} needs {tiles} (or none at all — \
                     the fabric, the graded level and the dissolved load are optional in an \
                     older epoch)",
                    self.recipe.freq
                )));
            }
        }
        for (i, layer) in self.air.iter().enumerate() {
            if layer.len() != tiles {
                return Err(EpochFileError::Invalid(format!(
                    "air layer {i} has {} tiles, freq {} needs {tiles}",
                    layer.len(),
                    self.recipe.freq
                )));
            }
        }
        // The vein cross-references stay inside their rosters.
        let bodies = self.veins.len();
        if let Some(bad) = l.vein_node_of.iter().find(|&&n| n as usize > bodies) {
            return Err(EpochFileError::Invalid(format!(
                "vein_node_of {bad} exceeds the {bodies} vein bodies"
            )));
        }
        if let Some(v) = self.veins.iter().find(|v| v.center as usize >= tiles) {
            return Err(EpochFileError::Invalid(format!(
                "vein body centre {} is off the {tiles}-tile planet",
                v.center
            )));
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

    /// A tiny but structurally complete planet for the v2 format tests.
    fn tiny_planet() -> PlanetEpoch {
        let freq = 1; // 12 tiles
        let tiles = flicker_worldgrid::cell_count(freq);
        let ledger = PlanetLedger {
            base: vec![0.2; tiles],
            l3_h: vec![0.0; tiles],
            l3_hard: vec![1.0; tiles],
            l4_h: vec![0.0; tiles],
            l4_hard: vec![1.0; tiles],
            strike: vec![0.0; tiles],
            l3_dip: vec![0.0; tiles],
            l4_dip: vec![0.0; tiles],
            graded: vec![0.0; tiles],
            dissolved: vec![0.0; tiles],
            rock: vec![0.1; tiles],
            rock_hard: vec![1.0; tiles],
            sediment: vec![0.0; tiles],
            bed_hard: vec![1.0; tiles],
            pressure: vec![0.0; tiles],
            edge: vec![0.0; tiles],
            edge_age: vec![0; tiles],
            drift: vec![0.0; tiles],
            suspend: vec![0.0; tiles],
            ice: vec![0.0; tiles],
            sst: vec![0.5; tiles],
            discharge: vec![0.0; tiles],
            moist: vec![0.0; tiles],
            rain: vec![0.0; tiles],
            veg: vec![0.0; tiles],
            vein: vec![0; tiles],
            vein_node_of: vec![0; tiles],
        };
        let era = PlanetEra {
            ticks: 7,
            eruptions: 1,
            steps: 0,
            heals: 0,
            water_volume: 1.5,
            ice_locked: 0.0,
            climate_base: 0.5,
            temp: 0.5,
            deep_temp: 0.35,
            greenhouse: 0.0,
            water_target: 0.7,
            veg_target: 0.35,
            veg_thirst: 1.0,
            green_share: 0.0,
            resources_ensured: false,
            well: 0.25,
            sunk: 0.75,
            delaminations: 2,
        };
        PlanetEpoch::new(
            PlanetRecipe {
                freq,
                seed: 42,
                cells: 3,
                spots: 1,
            },
            era,
            ledger,
            vec![vec![0.0; tiles]; 3],
            vec![0.5, 0.25],
            vec![VeinBody {
                center: 3,
                kind: 2,
                size: 4,
                budget: 9,
            }],
            "tiny test planet",
        )
    }

    #[test]
    fn planet_epoch_round_trips_through_json_and_disk() {
        let file = tiny_planet();
        assert_eq!(file.format, EPOCH_FORMAT);
        assert_eq!(file.version, EPOCH_VERSION);

        let back = PlanetEpoch::from_json(&file.to_json().unwrap()).unwrap();
        assert_eq!(back, file, "JSON round-trip preserves the whole planet");

        let dir = std::env::temp_dir().join("flicker_planet_epoch_test");
        let gz = dir.join("tiny.epoch.gz");
        file.save(&gz).unwrap();
        assert_eq!(PlanetEpoch::load(&gz).unwrap(), file, "gz disk round-trip");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn planet_epoch_validation_fails_loud() {
        // A ledger array off the recipe's tile count.
        let mut short = tiny_planet();
        short.ledger.rain.pop();
        assert!(short.to_json().map(|j| PlanetEpoch::from_json(&j)).unwrap().is_err());
        // A vein pointer past the body roster.
        let mut dangling = tiny_planet();
        dangling.ledger.vein_node_of[0] = 2;
        assert!(PlanetEpoch::from_json(&dangling.to_json().unwrap()).is_err());
    }

    #[test]
    fn the_two_versions_reject_each_other_loud() {
        // A v2 planet fed to the legacy loader: refused by version (and shape).
        let planet = tiny_planet().to_json().unwrap();
        assert!(EpochFile::from_json(&planet).is_err());
        // A legacy capture fed to the planet loader: refused loud.
        let mut engine = WorldEngine::from_repo().expect("engine from repo content");
        engine.set_freq(6);
        let legacy = engine.capture("legacy").to_json().unwrap();
        assert!(PlanetEpoch::from_json(&legacy).is_err());
    }
}
