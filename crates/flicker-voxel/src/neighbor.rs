//! Cross-cluster and cross-LOD primitives.
//!
//! [`NeighborContext`] gives a consumer read access to the six
//! face-adjacent clusters, so it can resolve voxel classifications and
//! corner positions across cluster boundaries. [`Lod`] describes the
//! level-of-detail at which a cluster is being read. [`read_corner`]
//! is the helper that consults `NeighborContext` when a voxel lookup
//! falls outside the active cluster's bounds — the authoritative
//! live-neighbor read the boundary-meshing design is built on.
//!
//! The old overlap/halo stitching machinery that used to live here has
//! been removed: boundary continuity now comes from reading the
//! neighbor's authoritative data directly (see the contour pipeline
//! spec), not from materializing redundant per-cluster halo slabs.

use crate::cluster::CLUSTER_DIM;
use crate::{Cluster, LocalCoord, Voxel};

/// Level of detail.
///
/// LOD `L` reads every `2^L`-th voxel along each axis when contouring.
/// Valid range is `0..=7`:
///
/// | level | stride | sample_dim | cell_dim |
/// | ----: | -----: | ---------: | -------: |
/// |   0   |    1   |    256     |   255    |
/// |   1   |    2   |    128     |   127    |
/// |   2   |    4   |     64     |    63    |
/// |   3   |    8   |     32     |    31    |
/// |   4   |   16   |     16     |    15    |
/// |   5   |   32   |      8     |     7    |
/// |   6   |   64   |      4     |     3    |
/// |   7   |  128   |      2     |     1    |
///
/// The sample sets are nested: positions sampled at LOD `L+1` are a strict
/// subset of positions sampled at LOD `L`, by construction. This is what
/// makes inter-LOD geometry consistent in later phases.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Lod(u8);

impl Lod {
    /// LOD 0 — full resolution.
    pub const ZERO: Lod = Lod(0);

    /// Maximum allowed LOD. At LOD 7 the cluster reduces to a `2³`
    /// sample grid (1 cell), the smallest non-degenerate sample set.
    pub const MAX: Lod = Lod(7);

    /// Construct an LOD from a level. Returns `None` if `level > 7`.
    #[inline]
    #[must_use]
    pub const fn new(level: u8) -> Option<Lod> {
        if level > Self::MAX.0 {
            None
        } else {
            Some(Lod(level))
        }
    }

    /// The LOD level (`0..=7`).
    #[inline]
    #[must_use]
    pub const fn level(self) -> u8 {
        self.0
    }

    /// The voxel stride at this LOD (`2^level`). Always a power of two in
    /// `[1, 128]`.
    #[inline]
    #[must_use]
    pub const fn stride(self) -> u32 {
        1u32 << self.0
    }

    /// The effective sample dimension at this LOD: `256 / stride`. Always
    /// in `[2, 256]`.
    #[inline]
    #[must_use]
    pub const fn sample_dim(self) -> u32 {
        CLUSTER_DIM >> self.0
    }

    /// The effective cell dimension at this LOD: `sample_dim - 1`. Always
    /// in `[1, 255]`.
    #[inline]
    #[must_use]
    pub const fn cell_dim(self) -> u32 {
        (CLUSTER_DIM >> self.0) - 1
    }
}

/// Which face of the local cluster a neighbor sits across. The local
/// cluster's `PosX` face touches the neighbor's `NegX` face, etc.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FaceDir {
    NegX,
    PosX,
    NegY,
    PosY,
    NegZ,
    PosZ,
}

/// Optional voxel data from this cluster's 6 face neighbors, plus each
/// neighbor's intended LOD.
///
/// A face with `None` is a world boundary; a face with `Some` carries
/// the neighbor cluster and its LOD so a consumer can resolve
/// classifications and corner positions across the boundary.
///
/// # Across-face read convention
///
/// Self's `+X` face is the neighbor's `-X` face. When sampling across
/// the boundary, self reads neighbor voxels at the neighbor's minimum-X
/// plane (`x = 0` in neighbor's local frame), and vice versa.
///
/// Use [`NeighborContext::none`] for the all-`None` case (e.g., when
/// every face is a world boundary).
#[derive(Default)]
pub struct NeighborContext<'a> {
    pub neg_x: Option<(&'a Cluster, Lod)>,
    pub pos_x: Option<(&'a Cluster, Lod)>,
    pub neg_y: Option<(&'a Cluster, Lod)>,
    pub pos_y: Option<(&'a Cluster, Lod)>,
    pub neg_z: Option<(&'a Cluster, Lod)>,
    pub pos_z: Option<(&'a Cluster, Lod)>,
}

impl NeighborContext<'_> {
    /// A context with no neighbors on any face (every face is a world
    /// boundary). Equivalent to [`Default::default`].
    #[must_use]
    pub const fn none() -> Self {
        Self {
            neg_x: None,
            pos_x: None,
            neg_y: None,
            pos_y: None,
            neg_z: None,
            pos_z: None,
        }
    }
}

/// Read a voxel at `(vx, vy, vz)` in `cluster`'s local frame, routing
/// out-of-range coordinates to the appropriate face neighbor.
///
/// - All three coords in `[0, CLUSTER_DIM)`: returns `cluster.get(...)`.
/// - Exactly one coord out of range, with the matching face neighbor
///   `Some`: returns the neighbor's voxel at the wrapped coord (so
///   `vx = -1` reads `neg_x` at `vx = CLUSTER_DIM - 1`, and
///   `vx = CLUSTER_DIM` reads `pos_x` at `vx = 0`).
/// - Exactly one coord out of range, no matching neighbor: returns
///   `cluster.base()`.
/// - Two or three coords out of range simultaneously (cluster edge or
///   corner): returns `cluster.base()`. Edge and corner neighbors are
///   intentionally not modeled.
pub(crate) fn read_corner(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    vx: i32,
    vy: i32,
    vz: i32,
) -> Voxel {
    let dim = CLUSTER_DIM as i32;
    let in_x = (0..dim).contains(&vx);
    let in_y = (0..dim).contains(&vy);
    let in_z = (0..dim).contains(&vz);
    if in_x && in_y && in_z {
        return cluster.get(LocalCoord::new(vx as u32, vy as u32, vz as u32).expect("in bounds"));
    }
    let oob_count = u32::from(!in_x) + u32::from(!in_y) + u32::from(!in_z);
    if oob_count >= 2 {
        return cluster.base();
    }
    let (src, lx, ly, lz) = if !in_x {
        let n = if vx < 0 {
            neighbors.neg_x
        } else {
            neighbors.pos_x
        };
        match n {
            Some((src, _)) => (src, vx.rem_euclid(dim), vy, vz),
            None => return cluster.base(),
        }
    } else if !in_y {
        let n = if vy < 0 {
            neighbors.neg_y
        } else {
            neighbors.pos_y
        };
        match n {
            Some((src, _)) => (src, vx, vy.rem_euclid(dim), vz),
            None => return cluster.base(),
        }
    } else {
        let n = if vz < 0 {
            neighbors.neg_z
        } else {
            neighbors.pos_z
        };
        match n {
            Some((src, _)) => (src, vx, vy, vz.rem_euclid(dim)),
            None => return cluster.base(),
        }
    };
    src.get(LocalCoord::new(lx as u32, ly as u32, lz as u32).expect("in bounds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_vector::CornerVector;
    use crate::material::Material;

    fn solid_voxel() -> Voxel {
        Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap())
    }

    // ---- Lod ----

    #[test]
    fn lod_new_accepts_0_through_7() {
        for level in 0..=7u8 {
            assert!(
                Lod::new(level).is_some(),
                "Lod::new({level}) should be Some"
            );
        }
        assert!(Lod::new(8).is_none());
        assert!(Lod::new(255).is_none());
    }

    #[test]
    fn lod_zero_constants() {
        assert_eq!(Lod::ZERO.level(), 0);
        assert_eq!(Lod::ZERO.stride(), 1);
        assert_eq!(Lod::ZERO.sample_dim(), 256);
        assert_eq!(Lod::ZERO.cell_dim(), 255);
    }

    #[test]
    fn lod_max_constants() {
        assert_eq!(Lod::MAX.level(), 7);
        assert_eq!(Lod::MAX.stride(), 128);
        assert_eq!(Lod::MAX.sample_dim(), 2);
        assert_eq!(Lod::MAX.cell_dim(), 1);
    }

    #[test]
    fn lod_dim_relationships_at_every_level() {
        for level in 0..=7u8 {
            let lod = Lod::new(level).unwrap();
            assert_eq!(lod.stride(), 1u32 << level, "stride at LOD {level}");
            assert_eq!(
                lod.sample_dim(),
                256 / lod.stride(),
                "sample_dim at LOD {level}"
            );
            assert_eq!(
                lod.cell_dim(),
                lod.sample_dim() - 1,
                "cell_dim at LOD {level}"
            );
        }
    }

    // ---- NeighborContext ----

    #[test]
    fn neighbor_context_none_is_all_none() {
        let nc = NeighborContext::none();
        assert!(nc.neg_x.is_none() && nc.pos_x.is_none());
        assert!(nc.neg_y.is_none() && nc.pos_y.is_none());
        assert!(nc.neg_z.is_none() && nc.pos_z.is_none());
    }

    #[test]
    fn neighbor_context_default_matches_none() {
        let d = NeighborContext::default();
        let n = NeighborContext::none();
        assert_eq!(d.neg_x.is_some(), n.neg_x.is_some());
        assert_eq!(d.pos_x.is_some(), n.pos_x.is_some());
        assert_eq!(d.neg_y.is_some(), n.neg_y.is_some());
        assert_eq!(d.pos_y.is_some(), n.pos_y.is_some());
        assert_eq!(d.neg_z.is_some(), n.neg_z.is_some());
        assert_eq!(d.pos_z.is_some(), n.pos_z.is_some());
    }

    // ---- read_corner ----

    #[test]
    fn read_corner_in_range_returns_cluster_voxel() {
        let mut c = Cluster::empty();
        c.set(LocalCoord::new(10, 20, 30).unwrap(), solid_voxel());
        let nc = NeighborContext::none();
        assert_eq!(read_corner(&c, &nc, 10, 20, 30), solid_voxel());
        // An out-of-cluster read with no neighbor returns the base
        // (empty Voxel for an empty cluster).
        assert_eq!(read_corner(&c, &nc, -1, 20, 30), c.base());
    }

    #[test]
    fn read_corner_single_axis_oob_routes_to_neighbor() {
        let mut neighbor = Cluster::empty();
        neighbor.set(LocalCoord::new(255, 20, 30).unwrap(), solid_voxel());
        let cluster = Cluster::empty();
        let nc = NeighborContext {
            neg_x: Some((&neighbor, Lod::ZERO)),
            ..NeighborContext::none()
        };
        // vx = -1 wraps to vx = 255 in the -X neighbor.
        assert_eq!(read_corner(&cluster, &nc, -1, 20, 30), solid_voxel());
    }

    #[test]
    fn read_corner_two_axes_oob_returns_base() {
        let cluster = Cluster::empty();
        let neighbor = Cluster::empty();
        let nc = NeighborContext {
            neg_x: Some((&neighbor, Lod::ZERO)),
            neg_y: Some((&neighbor, Lod::ZERO)),
            ..NeighborContext::none()
        };
        // Two coords out of range — even with the matching face
        // neighbors set, the helper falls back to cluster.base().
        assert_eq!(read_corner(&cluster, &nc, -1, -1, 30), cluster.base());
    }
}
