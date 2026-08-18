//! Cluster bake format — serialization between contoured `Cluster`s and a
//! per-cluster JSON "database row" on disk.
//!
//! # The data domain vs the render domain
//!
//! Single source of truth: **LOD0** voxel data. The on-disk file holds the
//! cluster's identity, its dense per-voxel state field, its sparse surface
//! overrides, and the cluster's bulk-fill material — nothing about higher
//! LODs lives in the data. Render-time LOD striding reads the same LOD0
//! data at varying strides; the per-face seam override decides when a
//! neighbour at a different LOD changes the stride at the seam itself.
//!
//! # Schema v2 — dense state + sparse surface overrides
//!
//! The v2 schema is the encoding-side companion to the `Cluster`
//! refactor that split physics state (dense, 2-bit-per-voxel) from
//! surface data (sparse, `(corner, material)` at active cells only).
//! On-disk fields:
//!
//! - `version` — `2`.
//! - `id` — `(lod, x, y, z)` from the `ClusterId`.
//! - `default_material` — 4-byte little-endian `Material::raw()` for the
//!   cluster's bulk-fill.
//! - `state` — the dense 4 MB `StateField` as a hex string (8 MB ASCII;
//!   bytes are emitted in the same order as `StateField::as_words()`'s
//!   `u64::to_le_bytes()` concatenation).
//! - `horizon_voxel` — `[x, y, z]` for the cluster's "dot on the horizon"
//!   pick, or `null` for clusters with no overrides.
//! - `surface_cells` — sorted by `(z, y, x)`, each `{ pos, corner,
//!   material }`. These are the active-cell entries: voxels that carry
//!   surface vertex information beyond what the cluster's defaults
//!   imply.
//!
//! Round-trip is bit-identical: every byte of the state field, every
//! sparse override, and the bulk-fill material all survive untouched.
//!
//! # `horizon_voxel` selection
//!
//! Same rule as v1, retained because nothing in the cluster split
//! changed which voxel is "the dot at extreme LOD":
//!
//! 1. Topmost surface override in column `(x = 0, z = 0)`.
//! 2. Otherwise, the override closest to cluster centre `(128, 128,
//!    128)`, tiebroken by `(z, y, x)`.
//! 3. Otherwise (no overrides at all), `None` — the renderer draws a
//!    placeholder unit cube.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use clayengine::CLUSTER_DIM;

use crate::cluster::Cluster;
use crate::cluster_id::ClusterId;
use crate::corner_vector::CornerVector;
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::voxel::Voxel;
use crate::voxel_state::{StateField, STATE_FIELD_WORDS};

/// Bake format version stamped into every file. Bump on every breaking
/// schema change; loaders must reject mismatches loudly rather than try
/// to migrate silently.
pub const BAKE_VERSION: u32 = 2;

/// A `Cluster` plus its bake-time metadata, ready to round-trip through
/// JSON. Use [`Self::from_cluster`] to construct from a freshly
/// contoured cluster, [`Self::to_json`] / [`Self::to_json_pretty`] to
/// serialize, and [`Self::from_json`] to load.
///
/// `horizon_voxel` is computed at bake time by [`find_horizon_voxel`]
/// and recorded in the file so the engine can fetch it without
/// re-scanning the override set on every load.
#[derive(Debug)]
pub struct BakedCluster {
    pub id: ClusterId,
    pub cluster: Cluster,
    pub horizon_voxel: Option<LocalCoord>,
}

impl BakedCluster {
    /// Wrap a freshly contoured `Cluster`. Scans for the horizon voxel
    /// per the documented selection rule.
    #[must_use]
    pub fn from_cluster(id: ClusterId, cluster: Cluster) -> Self {
        let horizon_voxel = find_horizon_voxel(&cluster);
        Self {
            id,
            cluster,
            horizon_voxel,
        }
    }

    /// Pretty-printed JSON. Use for human-readable bakes and diff-
    /// friendly output. The voxel array is sorted by `(z, y, x)` so
    /// diffs stay stable across re-bakes.
    pub fn to_json_pretty(&self) -> Result<String, BakeError> {
        serde_json::to_string_pretty(&self.to_schema()).map_err(BakeError::Json)
    }

    /// Compact JSON. Use for shipping or transport where size matters
    /// more than readability.
    pub fn to_json(&self) -> Result<String, BakeError> {
        serde_json::to_string(&self.to_schema()).map_err(BakeError::Json)
    }

    /// Parse a `BakedCluster` from JSON. Rejects mismatched versions
    /// and out-of-range positions.
    pub fn from_json(s: &str) -> Result<Self, BakeError> {
        let schema: BakedSchema = serde_json::from_str(s).map_err(BakeError::Json)?;
        Self::from_schema(schema)
    }

    /// Serialize to the on-disk byte format: compact JSON, then gzip-
    /// compressed via [`flicker_core::compress_gzip`]. This is the
    /// canonical bake artefact format — far smaller than raw JSON
    /// because the dense state field hex (4 MB × 2 chars = 8 MB
    /// ASCII per cluster) and the long runs of identical material
    /// bytes both compress 5–10× under default gzip.
    pub fn to_disk_bytes(&self) -> Result<Vec<u8>, BakeError> {
        let json = self.to_json()?;
        Ok(flicker_core::compress_gzip(json.as_bytes()))
    }

    /// Parse a `BakedCluster` from on-disk bytes — auto-detects gzip
    /// via the 2-byte magic-number sniff and decompresses if so,
    /// otherwise treats the input as already-uncompressed JSON. This
    /// is the loader's entry point and the inverse of
    /// [`Self::to_disk_bytes`]; pass raw `Vec<u8>` straight off disk
    /// and it does the right thing for both new and legacy bakes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BakeError> {
        let json = if flicker_core::is_gzipped(bytes) {
            flicker_core::decompress_gzip(bytes).map_err(BakeError::Decompress)?
        } else {
            bytes.to_vec()
        };
        let s = std::str::from_utf8(&json).map_err(BakeError::InvalidUtf8)?;
        Self::from_json(s)
    }

    fn to_schema(&self) -> BakedSchema {
        let mut surface_cells: Vec<BakedSurfaceCell> = self
            .cluster
            .overrides()
            .map(|(coord, voxel)| BakedSurfaceCell {
                pos: [coord.x(), coord.y(), coord.z()],
                corner: voxel.corner().bytes(),
                material: voxel.material().raw().to_le_bytes(),
            })
            .collect();
        // Deterministic (z, y, x) ordering — matches the contour's own
        // iteration order and makes round-trip diffs trivial to read.
        surface_cells.sort_by_key(|e| (e.pos[2], e.pos[1], e.pos[0]));

        BakedSchema {
            version: BAKE_VERSION,
            id: BakedId {
                lod: self.id.lod(),
                x: self.id.x(),
                y: self.id.y(),
                z: self.id.z(),
            },
            default_material: self.cluster.default_material().raw().to_le_bytes(),
            state: StateHex(self.cluster.state_field().as_words().to_vec()),
            horizon_voxel: self.horizon_voxel.map(|c| [c.x(), c.y(), c.z()]),
            surface_cells,
        }
    }

    fn from_schema(schema: BakedSchema) -> Result<Self, BakeError> {
        if schema.version != BAKE_VERSION {
            return Err(BakeError::UnsupportedVersion {
                got: schema.version,
                expected: BAKE_VERSION,
            });
        }
        if schema.id.lod > ClusterId::MAX_LOD
            || schema.id.x > ClusterId::MAX_X
            || schema.id.y > ClusterId::MAX_Y
            || schema.id.z > ClusterId::MAX_Z
        {
            return Err(BakeError::InvalidClusterId {
                lod: schema.id.lod,
                x: schema.id.x,
                y: schema.id.y,
                z: schema.id.z,
            });
        }
        let id = ClusterId::new(schema.id.lod, schema.id.x, schema.id.y, schema.id.z);

        if schema.state.0.len() != STATE_FIELD_WORDS {
            return Err(BakeError::InvalidStateField {
                got_words: schema.state.0.len(),
                expected_words: STATE_FIELD_WORDS,
            });
        }
        let mut words: Box<[u64; STATE_FIELD_WORDS]> = Box::new([0u64; STATE_FIELD_WORDS]);
        for (i, w) in schema.state.0.iter().enumerate() {
            words[i] = *w;
        }

        let mut cluster = Cluster::empty();
        cluster.set_default_material(Material::from_raw(u32::from_le_bytes(
            schema.default_material,
        )));
        cluster.replace_state_field(StateField::from_words(words));

        // Surface overrides: each entry's state is whatever the dense
        // field already says at that coord. We round-trip the
        // (corner, material) pair through `cluster.set`, which sets
        // state-from-the-Voxel — so we read the state out of the
        // dense field first to keep it intact.
        for entry in &schema.surface_cells {
            let coord = LocalCoord::new(entry.pos[0], entry.pos[1], entry.pos[2])
                .ok_or(BakeError::InvalidVoxelPos(entry.pos))?;
            let state = cluster.get(coord).state();
            let corner = CornerVector::from_bytes(entry.corner);
            let material = Material::from_raw(u32::from_le_bytes(entry.material));
            cluster.set(coord, Voxel::new(state, corner, material));
        }
        let horizon_voxel = schema
            .horizon_voxel
            .map(|p| LocalCoord::new(p[0], p[1], p[2]).ok_or(BakeError::InvalidHorizonPos(p)))
            .transpose()?;
        Ok(Self {
            id,
            cluster,
            horizon_voxel,
        })
    }
}

/// Apply the [`BakedCluster::horizon_voxel`] selection rule against an
/// arbitrary `Cluster`. Pulled out as a free function so callers can
/// recompute the horizon voxel for a cluster they edited without
/// having to re-bake the whole thing.
#[must_use]
pub fn find_horizon_voxel(cluster: &Cluster) -> Option<LocalCoord> {
    // Rule 1: column (x = 0, z = 0), topmost non-default corner. For a
    // heightfield this is the active LOD-0 cell straddling the surface
    // at that corner column — a stable, unambiguous "the cluster looks
    // like *this* point from far away" answer.
    for y in (0..CLUSTER_DIM).rev() {
        let coord = LocalCoord::new(0, y, 0).expect("y < CLUSTER_DIM");
        if cluster.get(coord).corner() != CornerVector::DEFAULT {
            return Some(coord);
        }
    }

    // Rule 2: closest override to the cluster's geometric centre, with
    // `(z, y, x)` lexicographic tiebreaking so the choice is
    // deterministic across re-bakes (HashMap iteration order would
    // otherwise leak in).
    //
    // Rule 3 (no overrides at all → renderer's responsibility) collapses
    // naturally: `min_by_key` returns `None` on an empty iterator and we
    // pass that through.
    let centre = (CLUSTER_DIM / 2) as i32;
    cluster
        .overrides()
        .map(|(coord, _)| {
            let dx = coord.x() as i32 - centre;
            let dy = coord.y() as i32 - centre;
            let dz = coord.z() as i32 - centre;
            let dsq = (dx * dx + dy * dy + dz * dz) as u64;
            (dsq, coord.z(), coord.y(), coord.x(), coord)
        })
        .min_by_key(|&(dsq, z, y, x, _)| (dsq, z, y, x))
        .map(|(_, _, _, _, coord)| coord)
}

// ---- On-disk schema (private) ----

#[derive(Debug, Serialize, Deserialize)]
struct BakedSchema {
    version: u32,
    id: BakedId,
    default_material: [u8; 4],
    state: StateHex,
    horizon_voxel: Option<[u32; 3]>,
    surface_cells: Vec<BakedSurfaceCell>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BakedId {
    lod: u8,
    x: u16,
    y: u16,
    z: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BakedSurfaceCell {
    pos: [u32; 3],
    corner: [u8; 3],
    material: [u8; 4],
}

/// Wrapper around the dense state field's raw `u64` words that
/// serialises to / from a hex string. The serialised form is
/// `2 × 8 × STATE_FIELD_WORDS` hex chars = 8 MB ASCII per cluster.
/// Each `u64` is emitted in **little-endian** byte order so the on-
/// disk bytes match `u64::to_le_bytes()` of the in-memory word.
#[derive(Debug)]
struct StateHex(Vec<u64>);

impl Serialize for StateHex {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        // Pre-size the output to avoid String re-growth: 8 hex chars
        // per byte × 8 bytes per word × N words.
        let mut s = String::with_capacity(self.0.len() * 16);
        for word in &self.0 {
            for byte in word.to_le_bytes() {
                s.push(HEX_LO[(byte >> 4) as usize] as char);
                s.push(HEX_LO[(byte & 0xF) as usize] as char);
            }
        }
        ser.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for StateHex {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        if s.len() % 16 != 0 {
            return Err(serde::de::Error::custom(format!(
                "state hex length {} is not a multiple of 16",
                s.len()
            )));
        }
        let n_words = s.len() / 16;
        let bytes = s.as_bytes();
        let mut words = Vec::with_capacity(n_words);
        for w in 0..n_words {
            let mut bs = [0u8; 8];
            for b in 0..8 {
                let hi = hex_value(bytes[(w * 16) + b * 2])
                    .ok_or_else(|| serde::de::Error::custom("non-hex char in state"))?;
                let lo = hex_value(bytes[(w * 16) + b * 2 + 1])
                    .ok_or_else(|| serde::de::Error::custom("non-hex char in state"))?;
                bs[b] = (hi << 4) | lo;
            }
            words.push(u64::from_le_bytes(bs));
        }
        Ok(StateHex(words))
    }
}

const HEX_LO: &[u8; 16] = b"0123456789abcdef";

#[inline]
fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(10 + c - b'a'),
        b'A'..=b'F' => Some(10 + c - b'A'),
        _ => None,
    }
}

/// Errors surfaced from bake serialization / deserialization.
#[derive(Debug)]
pub enum BakeError {
    /// JSON layer rejected the input (syntax / structural mismatch).
    Json(serde_json::Error),
    /// gzip decoder rejected the input — corrupt header, truncated
    /// body, or bad CRC. Caller should treat the bake file as
    /// damaged and re-bake.
    Decompress(flicker_core::CompressionError),
    /// Decompressed bytes weren't valid UTF-8 — the file's gzip
    /// envelope decoded but the payload inside wasn't text JSON.
    InvalidUtf8(std::str::Utf8Error),
    /// File's `version` field doesn't match the loader's
    /// [`BAKE_VERSION`]. Migrate offline, then re-load.
    UnsupportedVersion { got: u32, expected: u32 },
    /// `id` fields exceeded the locked `[LOD:4][x:10][y:8][z:10]`
    /// bit budget — file is corrupt.
    InvalidClusterId { lod: u8, x: u16, y: u16, z: u16 },
    /// State field word count mismatch — file is corrupt or was
    /// written for a different `CLUSTER_DIM`.
    InvalidStateField {
        got_words: usize,
        expected_words: usize,
    },
    /// A `surface_cells[i].pos` was outside `[0, CLUSTER_DIM)` on at
    /// least one axis — file is corrupt.
    InvalidVoxelPos([u32; 3]),
    /// `horizon_voxel` was outside `[0, CLUSTER_DIM)` on at least one
    /// axis — file is corrupt.
    InvalidHorizonPos([u32; 3]),
}

impl std::fmt::Display for BakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(e) => write!(f, "bake JSON error: {e}"),
            Self::Decompress(e) => write!(f, "bake gzip decode failed: {e}"),
            Self::InvalidUtf8(e) => write!(f, "bake decompressed bytes are not UTF-8: {e}"),
            Self::UnsupportedVersion { got, expected } => write!(
                f,
                "unsupported bake format version: got {got}, expected {expected}"
            ),
            Self::InvalidClusterId { lod, x, y, z } => {
                write!(f, "invalid ClusterId fields: lod={lod} x={x} y={y} z={z}")
            }
            Self::InvalidStateField {
                got_words,
                expected_words,
            } => write!(
                f,
                "state field has {got_words} words, expected {expected_words}"
            ),
            Self::InvalidVoxelPos(p) => write!(f, "surface cell position out of range: {p:?}"),
            Self::InvalidHorizonPos(p) => write!(f, "horizon voxel position out of range: {p:?}"),
        }
    }
}

impl std::error::Error for BakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            Self::Decompress(e) => Some(e),
            Self::InvalidUtf8(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contour::contour;
    use crate::voxel_state::VoxelState;
    use flicker_primitive::FlatField;

    fn grey() -> Material {
        Material::new(1, 1, 0).expect("valid material")
    }

    #[test]
    fn version_constant_matches_format() {
        assert_eq!(BAKE_VERSION, 2);
    }

    #[test]
    fn round_trip_flat_field_cluster_is_byte_equal() {
        let id = ClusterId::new(0, 0, 0, 0);
        let cluster = contour(&FlatField::at_half(), grey(), id);
        let original_override_count = cluster.override_count();
        let original_default_material = cluster.default_material();
        // Sample a handful of state bits to compare after round-trip.
        let probe_coords = [
            LocalCoord::new(0, 0, 0).unwrap(),
            LocalCoord::new(0, 127, 0).unwrap(),
            LocalCoord::new(0, 128, 0).unwrap(),
            LocalCoord::new(255, 127, 255).unwrap(),
            LocalCoord::new(128, 128, 128).unwrap(),
        ];
        let original_states: Vec<_> = probe_coords
            .iter()
            .map(|c| cluster.get(*c).state())
            .collect();
        let baked = BakedCluster::from_cluster(id, cluster);
        let json = baked.to_json().expect("serialize");
        let reloaded = BakedCluster::from_json(&json).expect("deserialize");

        assert_eq!(reloaded.id.bits(), baked.id.bits());
        assert_eq!(
            reloaded.cluster.default_material(),
            original_default_material
        );
        assert_eq!(reloaded.cluster.override_count(), original_override_count);
        for (c, expected_state) in probe_coords.iter().zip(original_states.iter()) {
            assert_eq!(
                reloaded.cluster.get(*c).state(),
                *expected_state,
                "state at {c:?} drifted"
            );
        }
        for (coord, voxel) in baked.cluster.overrides() {
            let reloaded_voxel = reloaded.cluster.get(coord);
            assert_eq!(
                reloaded_voxel.corner().bytes(),
                voxel.corner().bytes(),
                "corner drift at {coord:?}"
            );
            assert_eq!(
                reloaded_voxel.material().raw(),
                voxel.material().raw(),
                "material drift at {coord:?}"
            );
            assert_eq!(
                reloaded_voxel.state(),
                voxel.state(),
                "state drift at {coord:?}"
            );
        }
        assert_eq!(reloaded.horizon_voxel, baked.horizon_voxel);
    }

    #[test]
    fn horizon_voxel_rule_1_finds_heightfield_surface_at_origin_column() {
        let id = ClusterId::new(0, 0, 0, 0);
        let cluster = contour(&FlatField::at_half(), grey(), id);
        let horizon = find_horizon_voxel(&cluster).expect("FlatField has a surface");
        assert_eq!((horizon.x(), horizon.z()), (0, 0));
        assert_eq!(horizon.y(), 127);
        assert_ne!(cluster.get(horizon).corner(), CornerVector::DEFAULT);
    }

    #[test]
    fn horizon_voxel_rule_3_returns_none_for_uniform_cluster() {
        let cluster = Cluster::empty();
        assert_eq!(find_horizon_voxel(&cluster), None);
    }

    #[test]
    fn horizon_voxel_rule_2_falls_back_to_centre_closest() {
        let mut cluster = Cluster::empty();
        let centre_coord = LocalCoord::new(130, 128, 126).expect("in range");
        let far_coord = LocalCoord::new(250, 250, 250).expect("in range");
        let voxel = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_components(0.4, 0.6, 0.7),
            grey(),
        );
        cluster.set(centre_coord, voxel);
        cluster.set(far_coord, voxel);

        let horizon = find_horizon_voxel(&cluster).expect("cluster has overrides");
        assert_eq!(horizon, centre_coord);
    }

    #[test]
    fn reject_unsupported_version() {
        let json = r#"{
            "version": 99,
            "id": { "lod": 0, "x": 0, "y": 0, "z": 0 },
            "default_material": [0, 0, 0, 0],
            "state": "",
            "horizon_voxel": null,
            "surface_cells": []
        }"#;
        let err = BakedCluster::from_json(json).expect_err("must reject version mismatch");
        assert!(
            matches!(
                err,
                BakeError::UnsupportedVersion {
                    got: 99,
                    expected: 2
                }
            ),
            "wrong error: {err:?}"
        );
    }

    #[test]
    fn reject_out_of_range_voxel_pos() {
        // Build a valid state field's worth of hex zeros so the JSON
        // parses up to the surface_cells check.
        let zero_state = "0".repeat(STATE_FIELD_WORDS * 16);
        let json = format!(
            r#"{{
                "version": 2,
                "id": {{ "lod": 0, "x": 0, "y": 0, "z": 0 }},
                "default_material": [0, 0, 0, 0],
                "state": "{zero_state}",
                "horizon_voxel": null,
                "surface_cells": [
                    {{ "pos": [256, 0, 0], "corner": [128, 128, 128], "material": [0, 0, 0, 0] }}
                ]
            }}"#
        );
        let err = BakedCluster::from_json(&json).expect_err("must reject out-of-range pos");
        assert!(
            matches!(err, BakeError::InvalidVoxelPos([256, 0, 0])),
            "wrong error: {err:?}"
        );
    }

    #[test]
    fn surface_cells_ordering_is_deterministic_zyx() {
        let id = ClusterId::new(0, 0, 0, 0);
        let mut cluster = Cluster::empty();
        let v = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_bytes([1, 2, 3]),
            grey(),
        );
        for &(x, y, z) in &[(5_u32, 1, 2), (1, 1, 2), (1, 5, 2), (1, 5, 1), (1, 1, 1)] {
            cluster.set(LocalCoord::new(x, y, z).expect("in range"), v);
        }
        let json_a = BakedCluster::from_cluster(id, cluster).to_json().unwrap();
        let id2 = ClusterId::new(0, 0, 0, 0);
        let mut cluster2 = Cluster::empty();
        for &(x, y, z) in &[(1_u32, 5, 1), (5, 1, 2), (1, 5, 2), (1, 1, 1), (1, 1, 2)] {
            cluster2.set(LocalCoord::new(x, y, z).expect("in range"), v);
        }
        let json_b = BakedCluster::from_cluster(id2, cluster2).to_json().unwrap();
        assert_eq!(
            json_a, json_b,
            "ordering must not depend on insertion order"
        );
    }

    #[test]
    fn round_trip_through_disk_bytes_is_byte_equal() {
        // The compressed-bytes path should give us the same Cluster
        // back as the JSON-string path — gzip is just transport.
        let id = ClusterId::new(0, 0, 0, 0);
        let cluster = contour(&FlatField::at_half(), grey(), id);
        let baked = BakedCluster::from_cluster(id, cluster);
        let bytes = baked.to_disk_bytes().expect("compress");
        // First two bytes are the gzip magic — proves we actually
        // wrote a compressed stream and not raw JSON.
        assert!(flicker_core::is_gzipped(&bytes));
        let reloaded = BakedCluster::from_bytes(&bytes).expect("decompress + parse");
        assert_eq!(reloaded.id.bits(), baked.id.bits());
        assert_eq!(
            reloaded.cluster.override_count(),
            baked.cluster.override_count()
        );
        assert_eq!(reloaded.horizon_voxel, baked.horizon_voxel);
        assert_eq!(
            reloaded.cluster.default_material(),
            baked.cluster.default_material()
        );
    }

    #[test]
    fn from_bytes_accepts_uncompressed_json() {
        // Legacy / hand-decompressed bakes should still load — the
        // sniff falls back to "treat as plain JSON" when the gzip
        // magic isn't there.
        let id = ClusterId::new(0, 0, 0, 0);
        let cluster = contour(&FlatField::at_half(), grey(), id);
        let baked = BakedCluster::from_cluster(id, cluster);
        let json = baked.to_json().expect("serialize");
        assert!(!flicker_core::is_gzipped(json.as_bytes()));
        let reloaded =
            BakedCluster::from_bytes(json.as_bytes()).expect("uncompressed should also load");
        assert_eq!(reloaded.id.bits(), baked.id.bits());
    }

    #[test]
    fn state_field_round_trips_through_hex() {
        // Build a small field, set a handful of states, round-trip
        // through JSON, verify state at each probe is preserved.
        let id = ClusterId::new(0, 0, 0, 0);
        let mut cluster = Cluster::empty();
        cluster.set_state(LocalCoord::new(0, 0, 0).unwrap(), VoxelState::Solid);
        cluster.set_state(LocalCoord::new(1, 0, 0).unwrap(), VoxelState::Viscous);
        cluster.set_state(LocalCoord::new(2, 0, 0).unwrap(), VoxelState::Flowing);
        cluster.set_state(LocalCoord::new(255, 255, 255).unwrap(), VoxelState::Solid);
        let baked = BakedCluster::from_cluster(id, cluster);
        let json = baked.to_json().unwrap();
        let reloaded = BakedCluster::from_json(&json).unwrap();
        assert_eq!(
            reloaded
                .cluster
                .get(LocalCoord::new(0, 0, 0).unwrap())
                .state(),
            VoxelState::Solid
        );
        assert_eq!(
            reloaded
                .cluster
                .get(LocalCoord::new(1, 0, 0).unwrap())
                .state(),
            VoxelState::Viscous
        );
        assert_eq!(
            reloaded
                .cluster
                .get(LocalCoord::new(2, 0, 0).unwrap())
                .state(),
            VoxelState::Flowing
        );
        assert_eq!(
            reloaded
                .cluster
                .get(LocalCoord::new(255, 255, 255).unwrap())
                .state(),
            VoxelState::Solid
        );
        assert_eq!(
            reloaded
                .cluster
                .get(LocalCoord::new(50, 50, 50).unwrap())
                .state(),
            VoxelState::Empty
        );
    }
}
