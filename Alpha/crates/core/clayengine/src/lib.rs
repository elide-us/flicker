//! ClayEngine — the foundation layer of the voxel world.
//!
//! This is the lowest crate in the engine: the single, authoritative home
//! for the fundamental **magic numbers** that define the voxel world — the
//! spatial dimensions, the level-of-detail range, and the physical scale
//! that every layer above (storage, primitives, contouring, meshing,
//! navigation) is calibrated to. Centralising them here gives those layers
//! one source of truth and stops any of them from depending on another
//! merely to learn the size or scale of the world's atoms. ClayEngine
//! depends on nothing; everything that needs these constants depends on it.
//!
//! # What belongs here
//!
//! *Engine configuration* — numbers that define the world itself, where
//! changing one re-scales or re-shapes everything. A constant earns a place
//! here if more than one layer is calibrated to it, or if it sets a
//! world-defining invariant (cluster size, LOD depth, voxel scale).
//!
//! # What does NOT belong here
//!
//! Per-module *tuning* and *encoding* details: QEF regularisation, nav ring
//! distances, bake format versions, water levels, bit-field widths, type
//! sentinels. Those live with the module they serve. The test is "would a
//! second layer need to agree on this exact value?" — if not, it stays local.

/// Side length of a cluster, in voxels. A cluster is `CLUSTER_DIM³` voxels;
/// at 6 inches per voxel ([`FEET_PER_VOXEL`]) that is 128 feet on a side.
///
/// Must remain a power of two: the LOD ladder samples at strides of `2^L`,
/// so the world's level-of-detail depth ([`MAX_LOD`]) is derived from it.
pub const CLUSTER_DIM: u32 = 256;

/// Total voxel count in a cluster (`CLUSTER_DIM³`).
pub const VOXEL_COUNT: usize = (CLUSTER_DIM as usize).pow(3);

/// Coarsest level of detail: the LOD at which a whole cluster reduces to a
/// single sample vector.
///
/// Derived from [`CLUSTER_DIM`], not an independent magic number. At LOD `L`
/// the sample stride is `2^L`, so a cluster holds `CLUSTER_DIM >> L` samples
/// per axis — exactly 1 at `L = log2(CLUSTER_DIM)`, and 0 (degenerate) one
/// step beyond. For a 256³ cluster that floor is LOD 8. Computing it as
/// `log2(CLUSTER_DIM)` keeps the two constants from ever drifting apart.
pub const MAX_LOD: u8 = CLUSTER_DIM.trailing_zeros() as u8;

/// Physical edge length of one voxel, in feet. A voxel is a 6-inch cube, so
/// `0.5` ft. This is the conversion every world-space measurement — nav ring
/// distances, physics, render scale — is calibrated against.
pub const FEET_PER_VOXEL: f32 = 0.5;

/// **The planet size model** (Aaron's canon, moved here from the Populous
/// bench 2026-08-28 when the world bake became a second consumer — the
/// engine-config charter above: a number more than one layer is calibrated
/// to lives here once).
///
/// The standard hex tile width, miles — THE canon cell size: it stays
/// constant as a planet scales, so frequency IS planet size. It is also the
/// VoxelFarm-lineage chain made physical: a hex spans 2048 clusters of
/// 128 ft (`CLUSTER_DIM · FEET_PER_VOXEL`), and 2048 · 128 ft = 49.65 mi
/// (a consistency test in flicker-worldtile pins the two statements
/// together).
pub const TILE_MI: f64 = 49.65;
/// The STANDARD world's diameter, miles (Earth's mean) — what
/// [`STANDARD_FREQ`] tiles out to at [`TILE_MI`] per cell.
pub const EARTH_DIAMETER_MI: f64 = 7_917.5;
/// The standard icosphere frequency — the Earth-sized planet. 92,162 tiles.
pub const STANDARD_FREQ: u32 = 96;

/// The diameter a planet of `freq` tiles out to, in miles: the tile stays
/// the standard width, so the planet scales linearly with frequency —
/// `EARTH_DIAMETER_MI · freq / STANDARD_FREQ`.
pub fn diameter_mi(freq: u32) -> f64 {
    EARTH_DIAMETER_MI * f64::from(freq) / f64::from(STANDARD_FREQ)
}

/// Metres per mile — the one conversion constant the size model needs to
/// answer in SI, stated once.
pub const METERS_PER_MILE: f64 = 1_609.344;

/// One cluster's physical span in metres (`CLUSTER_DIM · FEET_PER_VOXEL`
/// = 128 ft) — the atlas pixel scale of the world cluster map.
pub fn cluster_span_m() -> f64 {
    f64::from(CLUSTER_DIM) * f64::from(FEET_PER_VOXEL) * 0.3048
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fundamentals_match_spec() {
        assert_eq!(CLUSTER_DIM, 256);
        assert_eq!(VOXEL_COUNT, 256 * 256 * 256);
        // The single-vector LOD floor is log2(256) = 8.
        assert_eq!(MAX_LOD, 8);
        assert_eq!(CLUSTER_DIM >> MAX_LOD, 1);
        assert_eq!(FEET_PER_VOXEL, 0.5);
    }
}
