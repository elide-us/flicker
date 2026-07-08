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
