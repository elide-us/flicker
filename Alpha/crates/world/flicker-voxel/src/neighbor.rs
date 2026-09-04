//! Cross-cluster and cross-LOD primitives.
//!
//! [`NeighborContext`] gives a consumer read access to the full
//! 26-neighborhood — the 6 face, 12 edge and 8 corner clusters — so it
//! can resolve voxel classifications and corner positions across ANY
//! cluster boundary, including the edges and corners where four or
//! eight clusters meet. [`Lod`] describes the level-of-detail at which
//! a cluster is being read. [`read_corner`] is the helper that
//! consults `NeighborContext` when a voxel lookup falls outside the
//! active cluster's bounds — the authoritative live-neighbor read the
//! boundary-meshing design is built on; it routes by the per-axis OOB
//! offset, so a single-axis (face), two-axis (edge) and three-axis
//! (corner) read are the one same rule.
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

/// Optional voxel data from this cluster's full 26-neighborhood — the
/// 6 face, 12 edge and 8 corner neighbors — plus each neighbor's
/// intended LOD.
///
/// A slot with `None` is a world boundary; a slot with `Some` carries
/// the neighbor cluster and its LOD so a consumer can resolve
/// classifications and corner positions across the boundary. ONE
/// representation: every neighbor is addressed by its per-axis offset
/// `(dx, dy, dz) ∈ {-1, 0, +1}³` — a face has one non-zero axis, an
/// edge two, a corner three. There are no named face fields; use
/// [`NeighborContext::set`] / [`NeighborContext::at`].
///
/// # Across-boundary read convention
///
/// Self's `+X` face is the neighbor's `-X` face. When sampling across
/// a boundary, self reads neighbor voxels at the neighbor's wrapped
/// coordinate on every OOB axis (`vx = -1` reads the `dx = -1`
/// neighbor at `vx = CLUSTER_DIM - 1`, `vx = CLUSTER_DIM` reads the
/// `dx = +1` neighbor at `vx = 0`) — identically for edges and
/// corners, where two or three axes wrap at once.
///
/// Use [`NeighborContext::none`] for the all-`None` case (e.g., when
/// every side is a world boundary).
#[derive(Default)]
pub struct NeighborContext<'a> {
    /// The 27 offset slots, indexed `(dx+1)·9 + (dy+1)·3 + (dz+1)`.
    /// The centre slot (self) is never set and never read.
    slots: [Option<(&'a Cluster, Lod)>; 27],
}

/// The slot index for a per-axis offset triple, each in `{-1, 0, +1}`.
#[inline]
fn slot_index(dx: i32, dy: i32, dz: i32) -> usize {
    debug_assert!(
        (-1..=1).contains(&dx) && (-1..=1).contains(&dy) && (-1..=1).contains(&dz),
        "neighbor offset out of range: ({dx}, {dy}, {dz})"
    );
    ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize
}

impl<'a> NeighborContext<'a> {
    /// A context with no neighbors on any side (every side is a world
    /// boundary). Equivalent to [`Default::default`].
    #[must_use]
    pub const fn none() -> Self {
        Self { slots: [None; 27] }
    }

    /// Install the neighbor at per-axis offset `(dx, dy, dz)`, each in
    /// `{-1, 0, +1}` with at least one non-zero (the centre is self,
    /// not a neighbor).
    pub fn set(&mut self, dx: i32, dy: i32, dz: i32, cluster: &'a Cluster, lod: Lod) {
        debug_assert!(
            dx != 0 || dy != 0 || dz != 0,
            "the centre slot is self, not a neighbor"
        );
        self.slots[slot_index(dx, dy, dz)] = Some((cluster, lod));
    }

    /// The neighbor at per-axis offset `(dx, dy, dz)`, if present. The
    /// centre `(0, 0, 0)` always reads `None`.
    #[must_use]
    pub fn at(&self, dx: i32, dy: i32, dz: i32) -> Option<(&'a Cluster, Lod)> {
        self.slots[slot_index(dx, dy, dz)]
    }

    /// Every installed neighbor, as `(offset, cluster, lod)` — for
    /// whole-neighborhood checks (e.g. the mesher's ±1 LOD adjacency
    /// assert), so a consumer never re-derives the slot walk.
    pub fn iter(&self) -> impl Iterator<Item = ([i32; 3], &'a Cluster, Lod)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.map(|(c, l)| {
                let i = i as i32;
                ([i / 9 - 1, (i / 3) % 3 - 1, i % 3 - 1], c, l)
            })
        })
    }
}

/// The per-axis neighbor offset a coordinate implies: `-1` below the
/// cluster, `+1` past it, `0` inside. The one statement of the OOB →
/// offset rule every routed read shares.
#[inline]
pub(crate) fn axis_offset(v: i32) -> i32 {
    let dim = CLUSTER_DIM as i32;
    if v < 0 {
        -1
    } else if v >= dim {
        1
    } else {
        0
    }
}

/// Read a voxel at `(vx, vy, vz)` in `cluster`'s local frame, routing
/// out-of-range coordinates through the neighborhood — faces, edges
/// and corners under the one same rule.
///
/// - All three coords in `[0, CLUSTER_DIM)`: returns `cluster.get(...)`.
/// - Any coords out of range, with the offset's neighbor `Some`:
///   returns that neighbor's voxel with every OOB coord wrapped (so
///   `vx = -1` reads the `dx = -1` slot at `vx = CLUSTER_DIM - 1`, and
///   `(-1, 20, CLUSTER_DIM)` reads the `(-1, 0, +1)` edge slot at
///   `(CLUSTER_DIM - 1, 20, 0)`).
/// - Out of range with no neighbor in that slot: returns
///   `Voxel::default()` — the world-boundary-is-empty default.
pub(crate) fn read_corner(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    vx: i32,
    vy: i32,
    vz: i32,
) -> Voxel {
    let dim = CLUSTER_DIM as i32;
    let (ox, oy, oz) = (axis_offset(vx), axis_offset(vy), axis_offset(vz));
    if ox == 0 && oy == 0 && oz == 0 {
        return cluster.get(LocalCoord::new(vx as u32, vy as u32, vz as u32).expect("in bounds"));
    }
    match neighbors.at(ox, oy, oz) {
        Some((src, _)) => src.get(
            LocalCoord::new(
                vx.rem_euclid(dim) as u32,
                vy.rem_euclid(dim) as u32,
                vz.rem_euclid(dim) as u32,
            )
            .expect("in bounds"),
        ),
        None => Voxel::default(),
    }
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
            Material::new(1, 0, 0),
        )
    }

    // ---- NeighborContext ----

    #[test]
    fn neighbor_context_none_is_all_none() {
        let nc = NeighborContext::none();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    assert!(nc.at(dx, dy, dz).is_none());
                }
            }
        }
        assert_eq!(nc.iter().count(), 0);
    }

    #[test]
    fn neighbor_context_set_lands_in_its_slot_alone() {
        let c = Cluster::empty();
        let mut nc = NeighborContext::none();
        nc.set(-1, 0, 1, &c, Lod::ZERO);
        assert!(nc.at(-1, 0, 1).is_some(), "the set slot reads back");
        let others = nc.iter().collect::<Vec<_>>();
        assert_eq!(others.len(), 1, "exactly one slot is occupied");
        assert_eq!(others[0].0, [-1, 0, 1], "…and it is the set offset");
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
        // anymore — state is dense, materials are sparse, and OOB
        // outside the field returns `Voxel::default()`).
        assert_eq!(read_corner(&c, &nc, -1, 20, 30), Voxel::default());
    }

    #[test]
    fn read_corner_single_axis_oob_routes_to_neighbor() {
        let mut neighbor = Cluster::empty();
        neighbor.set(LocalCoord::new(255, 20, 30).unwrap(), solid_voxel());
        let cluster = Cluster::empty();
        let mut nc = NeighborContext::none();
        nc.set(-1, 0, 0, &neighbor, Lod::ZERO);
        // vx = -1 wraps to vx = 255 in the -X neighbor.
        assert_eq!(read_corner(&cluster, &nc, -1, 20, 30), solid_voxel());
    }

    #[test]
    fn read_corner_edge_and_corner_oob_route_to_their_slots() {
        let dim = CLUSTER_DIM as i32;
        let mut edge = Cluster::empty();
        edge.set(
            LocalCoord::new(255, 255, 30).unwrap(),
            solid_voxel(),
        );
        let mut corner = Cluster::empty();
        corner.set(LocalCoord::new(255, 255, 0).unwrap(), solid_voxel());
        let cluster = Cluster::empty();
        let mut nc = NeighborContext::none();
        nc.set(-1, -1, 0, &edge, Lod::ZERO);
        nc.set(-1, -1, 1, &corner, Lod::ZERO);
        // Two axes OOB — the (-1, -1, 0) EDGE slot resolves, wrapped on
        // both OOB axes.
        assert_eq!(read_corner(&cluster, &nc, -1, -1, 30), solid_voxel());
        // Three axes OOB — the (-1, -1, +1) CORNER slot resolves.
        assert_eq!(read_corner(&cluster, &nc, -1, -1, dim), solid_voxel());
        // A multi-axis OOB read whose slot is absent stays the
        // world-boundary default — a face neighbor never masquerades
        // as an edge.
        assert_eq!(read_corner(&cluster, &nc, dim, dim, 30), Voxel::default());
    }
}
