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

use clayengine::CLUSTER_DIM;

use crate::lod::Lod;
use crate::{Cluster, LocalCoord, Voxel};

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
///   `Voxel::default()`.
/// - Two or three coords out of range simultaneously (cluster edge or
///   corner): returns `Voxel::default()`. Edge and corner neighbors are
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
        return Voxel::default();
    }
    let (src, lx, ly, lz) = if !in_x {
        let n = if vx < 0 {
            neighbors.neg_x
        } else {
            neighbors.pos_x
        };
        match n {
            Some((src, _)) => (src, vx.rem_euclid(dim), vy, vz),
            None => return Voxel::default(),
        }
    } else if !in_y {
        let n = if vy < 0 {
            neighbors.neg_y
        } else {
            neighbors.pos_y
        };
        match n {
            Some((src, _)) => (src, vx, vy.rem_euclid(dim), vz),
            None => return Voxel::default(),
        }
    } else {
        let n = if vz < 0 {
            neighbors.neg_z
        } else {
            neighbors.pos_z
        };
        match n {
            Some((src, _)) => (src, vx, vy, vz.rem_euclid(dim)),
            None => return Voxel::default(),
        }
    };
    src.get(LocalCoord::new(lx as u32, ly as u32, lz as u32).expect("in bounds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_vector::CornerVector;
    use crate::material::Material;
    use crate::voxel_state::VoxelState;

    fn solid_voxel() -> Voxel {
        Voxel::new(
            VoxelState::Solid,
            CornerVector::DEFAULT,
            Material::new(1, 0, 0).unwrap(),
        )
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
        // An out-of-cluster read with no neighbor returns the
        // empty-voxel sentinel (the cluster has no "base" voxel
        // anymore — state is dense, materials are sparse, and
        // multi-axis OOB outside the field returns `Voxel::default()`).
        assert_eq!(read_corner(&c, &nc, -1, 20, 30), Voxel::default());
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
        // neighbors set, the helper falls back to Voxel::default().
        assert_eq!(read_corner(&cluster, &nc, -1, -1, 30), Voxel::default());
    }
}
