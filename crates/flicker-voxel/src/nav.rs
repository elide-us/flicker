//! LOD2 walkable-surface derivation: turn one cluster's dense state
//! field into a per-column floor + a connectivity predicate a future
//! walking camera can ride.
//!
//! Pipeline stage: **cluster state field → [`ClusterNav`]**. Nav is a
//! *rendering target* like the mesh, not stored data: it is a pure,
//! deterministic function of the voxel state at a fixed sample stride
//! (LOD2, `stride = 4`, 2 ft cells), recomputed at load and never
//! baked, serialized, or shipped.
//!
//! # Built off the state field, never the mesh
//!
//! The dense state field is the authority for "is there matter at
//! `(x, y, z)`." The render mesh is a *visualization* of boundaries and
//! carries dapples/gaps the state field does not. A nav cell built from
//! the mesh could float over a gap, so nav samples the state field
//! directly — the same reasoning behind the deferred physics raycast
//! (ray-vs-state-field, not ray-vs-mesh).
//!
//! The solidity test reuses the mesher's exact occupancy predicate —
//! `read_corner(..).state().is_filled()` — taken as **binary
//! per-sample**: no interpolation, no isosurface crossing, no QEF. The
//! floor is quantized to the 2 ft grid; nav never does sub-cell surface
//! finding.
//!
//! # LOD2 ↔ LOD2 only
//!
//! Nav samples both the active cluster and its neighbors at `stride = 4`
//! through `read_corner`, which routes out-of-bounds sample coords to
//! the matching face neighbor at the **same** resolution — a straight
//! floor-index read with no cross-LOD fan. Because solidity comes from
//! the dense state field (written per-voxel at full resolution
//! regardless of a cluster's mesh-LOD), nav reads correct solidity even
//! across a boundary whose neighbor was meshed at a coarser LOD.
//!
//! # Caves/overhangs (deferred)
//!
//! v1 records only the **topmost** qualifying floor span per column.
//! The later extension changes the per-column value from a single
//! [`Option<u8>`] to a span list; the structure here does not preclude
//! that but does not implement it.

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::cluster_id::ClusterId;
use crate::neighbor::{read_corner, FaceDir, Lod, NeighborContext};

/// LOD level nav samples at — LOD2: `stride = 4`, 2 ft cells, 64
/// samples/axis. Locked by spec; nav is *only* meaningful at LOD2 and
/// deliberately takes no `Lod` parameter so a cross-LOD fan can't be
/// introduced here.
const NAV_LOD_LEVEL: u8 = 2;

/// The [`Lod`] nav samples at. Deriving the stride and grid dimension
/// from this (rather than hardcoding `4`/`64`) ties them to the same
/// locked LOD table the mesher reads, at compile time.
const NAV_LOD: Lod = match Lod::new(NAV_LOD_LEVEL) {
    Some(l) => l,
    None => panic!("NAV_LOD_LEVEL must be a valid LOD level"),
};

/// Voxel sample stride into the dense field (`2^NAV_LOD_LEVEL` = 4).
/// Sample index `i` reads voxel index `i * NAV_STRIDE`.
const NAV_STRIDE: i32 = NAV_LOD.stride() as i32;

/// Nav grid dimension per axis (`CLUSTER_DIM / NAV_STRIDE` = 64). The
/// floor grid is `NAV_DIM × NAV_DIM` columns.
pub const NAV_DIM: usize = NAV_LOD.sample_dim() as usize;

/// Empty cells required directly above a floor cell for an agent to
/// stand: ≥ 6 ft clearance = 3 × 2 ft cells. A floor at index `f`
/// qualifies only when `f+1, f+2, f+3` are all empty.
const CLEARANCE_CELLS: i32 = 3;

/// Largest floor-index delta between two 4-adjacent columns that still
/// counts as walkable-linked: ≤ 2 cells (a 2-cell rise over a 2 ft run
/// = 63.4°, the locked max slope). A delta ≥ 3 breaks the link.
const MAX_LINK_DELTA: u8 = 2;

/// Feet per LOD0 voxel — 6-inch voxels. The engine's world space is
/// voxel units (a cluster draws at `world_offset()` in voxel units and
/// spans `CLUSTER_DIM` of them), so the ring gate works in voxel units
/// and converts the spec's feet-denominated boundary once, here.
const FEET_PER_VOXEL: f32 = 0.5;

/// Outer boundary of ring 2 in feet. From the locked ring table the
/// outer boundary of ring `n` is `256 × (2^(n+1) − 1)` ft; ring 2 gives
/// `256 × (2^3 − 1) = 1792` ft. Nav is generated only for clusters
/// whose center lies within this radius of the camera (rings 0–2);
/// nothing past ring 2 is walkable. See [`in_nav_rings`].
pub const NAV_RING_OUTER_FT: f32 = 1792.0;

/// [`NAV_RING_OUTER_FT`] expressed in world/voxel units, so the gate's
/// distance test stays in the same frame as `world_offset()` and the
/// camera position without per-call unit juggling.
const NAV_RING_OUTER_VOXELS: f32 = NAV_RING_OUTER_FT / FEET_PER_VOXEL;

/// Per-cluster walkable surface at LOD2 (a `64 × 64` grid of columns).
///
/// `floor` is the only stored field; connectivity is derived on query
/// (the comparison is trivial and storing an adjacency graph would just
/// burn memory). Indices are **cluster-local**: world-space mapping
/// (`world_y = cluster_origin_y + floor * 2 ft`) belongs to the camera
/// follow-up, not here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClusterNav {
    /// `floor[x][z]` = topmost walkable LOD2 cell index (`0..=63`), or
    /// `None` when the column has no qualifying floor. v1 records the
    /// top span only; a later cave/overhang pass swaps this cell type
    /// for a span list.
    floor: [[Option<u8>; NAV_DIM]; NAV_DIM],
}

impl ClusterNav {
    /// Derive the walkable surface from a cluster's LOD2 state samples
    /// plus its face neighbors. Pure, side-effect-free, thread-safe:
    /// the result is a deterministic function of the voxel state alone
    /// (the ring gate that decides *whether* to call this lives in the
    /// caller — see [`in_nav_rings`] — so this stays state-only and
    /// deterministic).
    #[must_use]
    pub fn compute_nav(cluster: &Cluster, neighbors: &NeighborContext<'_>) -> ClusterNav {
        let mut floor = [[None; NAV_DIM]; NAV_DIM];
        for (x, column_x) in floor.iter_mut().enumerate() {
            for (z, cell) in column_x.iter_mut().enumerate() {
                *cell = column_floor(cluster, neighbors, x as i32, z as i32);
            }
        }
        ClusterNav { floor }
    }

    /// Read query #1: topmost walkable floor index at a local column.
    /// `None` = not walkable (no qualifying floor, or `x`/`z` out of the
    /// `0..64` grid).
    #[inline]
    #[must_use]
    pub fn floor_at(&self, x: u8, z: u8) -> Option<u8> {
        let (x, z) = (x as usize, z as usize);
        if x >= NAV_DIM || z >= NAV_DIM {
            return None;
        }
        self.floor[x][z]
    }

    /// Read query #2: are two 4-adjacent **local** columns walkable-
    /// linked? `true` iff both have a floor and their indices differ by
    /// ≤ 2 cells (the slope gate the camera consumes to block walking up
    /// a cliff). Callers pass N/S/E/W-adjacent columns; consuming this
    /// for movement is the camera's job.
    ///
    /// For a column on the cluster edge that links *across* a boundary,
    /// use [`Self::linked_across`].
    #[inline]
    #[must_use]
    pub fn linked(&self, a: (u8, u8), b: (u8, u8)) -> bool {
        match (self.floor_at(a.0, a.1), self.floor_at(b.0, b.1)) {
            (Some(fa), Some(fb)) => fa.abs_diff(fb) <= MAX_LINK_DELTA,
            _ => false,
        }
    }

    /// Floor of the column one step across the `dir` face — the
    /// neighbor's edge column, read **through `neighbors`** rather than
    /// from any separately-computed neighbor nav. The neighbor sample
    /// sits one index past the boundary (e.g. sample x = 64 for `PosX`);
    /// `read_corner` routes it to the matching face neighbor's state
    /// field at LOD2 (voxel 256 → the neighbor's x = 0), so the scan that
    /// produces this floor is identical to an interior column's — the
    /// seam is generated correctly in one pass, not stitched after.
    ///
    /// This is the value the cross-cluster link rule and a seam-crossing
    /// camera both need: returns `None` for a non-walkable neighbor
    /// column, an absent face neighbor, or a vertical (`NegY`/`PosY`)
    /// `dir` (nav columns have no adjacent column across those faces).
    #[must_use]
    pub fn floor_across(
        &self,
        cluster: &Cluster,
        neighbors: &NeighborContext<'_>,
        edge: (u8, u8),
        dir: FaceDir,
    ) -> Option<u8> {
        let (nsx, nsz) = match dir {
            FaceDir::NegX => (-1, i32::from(edge.1)),
            FaceDir::PosX => (NAV_DIM as i32, i32::from(edge.1)),
            FaceDir::NegZ => (i32::from(edge.0), -1),
            FaceDir::PosZ => (i32::from(edge.0), NAV_DIM as i32),
            FaceDir::NegY | FaceDir::PosY => return None,
        };
        column_floor(cluster, neighbors, nsx, nsz)
    }

    /// Cross-cluster variant of [`Self::linked`]: does the local edge
    /// column `edge` link to the neighbor column one step across the
    /// `dir` face? `true` iff `edge` has a floor and the neighbor column
    /// — read through `neighbors` via [`Self::floor_across`] — has one
    /// within the same delta ≤ 2 slope gate. `dir` must be a lateral
    /// face; vertical faces have no adjacent column and return `false`.
    #[must_use]
    pub fn linked_across(
        &self,
        cluster: &Cluster,
        neighbors: &NeighborContext<'_>,
        edge: (u8, u8),
        dir: FaceDir,
    ) -> bool {
        matches!(
            (
                self.floor_at(edge.0, edge.1),
                self.floor_across(cluster, neighbors, edge, dir),
            ),
            (Some(here), Some(there)) if here.abs_diff(there) <= MAX_LINK_DELTA
        )
    }
}

/// Topmost qualifying floor for the column at LOD2 sample `(sx, sz)`,
/// scanning `y` from the top (`NAV_DIM − 1`) downward. Returns the first
/// (highest) solid cell `f` whose `f+1, f+2, f+3` are all empty — the
/// agent stands on top of cell `f`. `None` if no such cell exists.
///
/// Factored out of [`ClusterNav::compute_nav`] so the same scan resolves
/// a neighbor's edge column when `sx`/`sz` is one step out of range
/// (`-1` or `NAV_DIM`): `read_corner` routes those samples across the
/// boundary, which is exactly the LOD2↔LOD2 seam read [`linked_across`]
/// needs. Accepts `i32` for that reason.
///
/// [`linked_across`]: ClusterNav::linked_across
fn column_floor(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    sx: i32,
    sz: i32,
) -> Option<u8> {
    for sy in (0..NAV_DIM as i32).rev() {
        if !solid(cluster, neighbors, sx, sy, sz) {
            continue;
        }
        // Clearance reads can run past the cluster top (sy + d ≥ 64),
        // where read_corner routes into the vertical neighbor's bottom
        // cells; an absent vertical neighbor reads as empty
        // (Voxel::default), i.e. open sky is walkable. v1 simplification
        // — revisitable once vertical streaming exists.
        let clear = (1..=CLEARANCE_CELLS).all(|d| !solid(cluster, neighbors, sx, sy + d, sz));
        if clear {
            return Some(sy as u8);
        }
    }
    None
}

/// Binary solidity at LOD2 sample `(sx, sy, sz)` — the mesher's exact
/// occupancy predicate, sampled at `stride = NAV_STRIDE` and routed
/// through `neighbors` for out-of-bounds coords. No interpolation, no
/// QEF: the dense state field's per-sample "is there matter" bit.
#[inline]
fn solid(cluster: &Cluster, neighbors: &NeighborContext<'_>, sx: i32, sy: i32, sz: i32) -> bool {
    read_corner(
        cluster,
        neighbors,
        sx * NAV_STRIDE,
        sy * NAV_STRIDE,
        sz * NAV_STRIDE,
    )
    .state()
    .is_filled()
}

/// World/voxel-space center of `id`'s cluster — `world_offset()` plus
/// half a cluster edge on each axis. The point the ring gate measures
/// the camera distance to.
#[must_use]
pub fn cluster_center_world(id: ClusterId) -> [f32; 3] {
    let o = id.world_offset();
    let half = CLUSTER_DIM as f32 * 0.5;
    [o[0] + half, o[1] + half, o[2] + half]
}

/// Ring 0–2 gate: `true` iff `cluster_center` lies within ring 2's
/// outer boundary (1792 ft) of `camera`. Both points are in world/voxel
/// units (e.g. from [`cluster_center_world`] and the camera position).
///
/// This is the §4.6 gate the load path consults *before* calling
/// [`ClusterNav::compute_nav`] — nothing past ring 2 gets nav. It is a
/// pure distance test, deliberately not a ring scheduler: it decides
/// per-cluster eligibility, not load/evict order. In the 9-cluster demo
/// all clusters fall inside ring 2, so all 9 generate nav; the gate
/// exists so behavior stays correct when the field expands.
#[must_use]
pub fn in_nav_rings(camera: [f32; 3], cluster_center: [f32; 3]) -> bool {
    let dx = camera[0] - cluster_center[0];
    let dy = camera[1] - cluster_center[1];
    let dz = camera[2] - cluster_center[2];
    dx * dx + dy * dy + dz * dz <= NAV_RING_OUTER_VOXELS * NAV_RING_OUTER_VOXELS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_coord::LocalCoord;
    use crate::voxel::Voxel;
    use crate::voxel_state::VoxelState;

    /// Set the single voxel that LOD2 sample `(sx, sy, sz)` reads
    /// (`voxel = sample * NAV_STRIDE`) to solid or empty. Nav probes
    /// exactly that voxel, so one write per sample is enough — no need
    /// to fill the stride³ footprint.
    fn set_sample(c: &mut Cluster, sx: u32, sy: u32, sz: u32, solid: bool) {
        let state = if solid {
            VoxelState::Solid
        } else {
            VoxelState::Empty
        };
        let coord = LocalCoord::new(sx * 4, sy * 4, sz * 4).expect("sample in range");
        c.set_state(coord, state);
    }

    /// A uniformly-solid cluster, for use as a capping vertical neighbor
    /// (its bottom cells read solid, so a floor near the cluster top
    /// fails its clearance check).
    fn solid_cluster() -> Cluster {
        Cluster::uniform(Voxel::new(
            VoxelState::Solid,
            crate::corner_vector::CornerVector::DEFAULT,
            crate::material::Material::new(1, 0, 0).unwrap(),
        ))
    }

    #[test]
    fn nav_constants_match_locked_lod2_and_ring_table() {
        // The derived stride/grid must land on the locked LOD2 row
        // (stride 4, 64 samples/axis), and the ring boundary on the
        // locked ring formula `256 × (2^(n+1) − 1)` at n = 2.
        assert_eq!(NAV_LOD_LEVEL, 2);
        assert_eq!(NAV_STRIDE, 4);
        assert_eq!(NAV_DIM, 64);
        assert_eq!(NAV_RING_OUTER_FT, 256.0 * ((1u32 << (2 + 1)) - 1) as f32);
        assert_eq!(NAV_RING_OUTER_VOXELS, 3584.0);
    }

    #[test]
    fn flat_floor_reports_index_and_empty_column_is_none() {
        // §6.1: solid at sample y=10 with ≥3 empty above → floor 10.
        let mut c = Cluster::empty();
        set_sample(&mut c, 5, 10, 5, true);
        let nav = ClusterNav::compute_nav(&c, &NeighborContext::none());
        assert_eq!(nav.floor_at(5, 5), Some(10));
        // A column with no matter anywhere → None.
        assert_eq!(nav.floor_at(0, 0), None);
        // Out-of-grid columns read None rather than panicking.
        assert_eq!(nav.floor_at(64, 0), None);
        assert_eq!(nav.floor_at(0, 200), None);
    }

    #[test]
    fn top_span_only_returns_upper_of_two_stacked_floors() {
        // §6.2: a lower floor at y=10 and an upper floor at y=20, each
        // with empty above → v1 reports only the upper (20).
        let mut c = Cluster::empty();
        set_sample(&mut c, 7, 10, 7, true); // lower floor (cave floor)
        set_sample(&mut c, 7, 20, 7, true); // upper floor (cave roof top)
        let nav = ClusterNav::compute_nav(&c, &NeighborContext::none());
        assert_eq!(nav.floor_at(7, 7), Some(20));
    }

    #[test]
    fn clearance_boundary_three_walks_two_does_not() {
        // §6.3 / §6.1: a floor capped by a solid ceiling. With exactly 3
        // empty cells of headroom the floor walks; with exactly 2 it
        // does not — and because the ceiling pillar runs to the cluster
        // top under a SOLID vertical neighbor, no surface above the
        // floor qualifies either, so the unwalkable case is a true None.
        let pos_y = solid_cluster();
        let neighbors = NeighborContext {
            pos_y: Some((&pos_y, NAV_LOD)),
            ..NeighborContext::none()
        };

        // Helper: floor at y=30, `gap` empty cells, then a solid pillar
        // up to the top sample (63). The solid pos_y neighbor caps 63's
        // clearance so the pillar never reads as a walkable surface.
        let build = |gap: u32| -> Cluster {
            let mut c = Cluster::empty();
            set_sample(&mut c, 9, 30, 9, true); // the floor
            for sy in (30 + 1 + gap)..=63 {
                set_sample(&mut c, 9, sy, 9, true); // ceiling pillar to top
            }
            c
        };

        // 3 empty (31,32,33), pillar from 34..=63 → floor 30 walks.
        let walks = ClusterNav::compute_nav(&build(3), &neighbors);
        assert_eq!(walks.floor_at(9, 9), Some(30));

        // 2 empty (31,32), pillar from 33..=63 → no qualifying floor.
        let blocked = ClusterNav::compute_nav(&build(2), &neighbors);
        assert_eq!(blocked.floor_at(9, 9), None);
    }

    #[test]
    fn slope_gate_links_at_delta_two_breaks_at_three() {
        // §6.4. Floors: (5,5)=10, (6,5)=12, (7,5)=15, (8,5)=None.
        let mut c = Cluster::empty();
        set_sample(&mut c, 5, 10, 5, true);
        set_sample(&mut c, 6, 12, 5, true);
        set_sample(&mut c, 7, 15, 5, true);
        let nav = ClusterNav::compute_nav(&c, &NeighborContext::none());
        assert_eq!(nav.floor_at(5, 5), Some(10));
        assert_eq!(nav.floor_at(6, 5), Some(12));
        assert_eq!(nav.floor_at(7, 5), Some(15));
        assert_eq!(nav.floor_at(8, 5), None);
        // delta 2 → linked.
        assert!(nav.linked((5, 5), (6, 5)));
        // delta 3 → broken.
        assert!(!nav.linked((6, 5), (7, 5)));
        // one side None → broken.
        assert!(!nav.linked((7, 5), (8, 5)));
    }

    #[test]
    fn cross_cluster_seam_links_via_neighbor_context() {
        // §6.5. Local cluster's +X edge column (x=63) floor at y=10;
        // the +X neighbor's west edge (x=0) floor at y=12. The seam read
        // resolves through NeighborContext at LOD2 (a straight stride-4
        // sample, no fan) and the same delta ≤ 2 rule links them.
        let mut here = Cluster::empty();
        set_sample(&mut here, 63, 10, 5, true);

        let mut east = Cluster::empty();
        set_sample(&mut east, 0, 12, 5, true); // delta 2 → links
        let neighbors = NeighborContext {
            pos_x: Some((&east, NAV_LOD)),
            ..NeighborContext::none()
        };
        let nav = ClusterNav::compute_nav(&here, &neighbors);
        assert_eq!(nav.floor_at(63, 5), Some(10));
        // The neighbor's edge-column floor is read through the reference,
        // in self's frame, at the sample one past the boundary.
        assert_eq!(
            nav.floor_across(&here, &neighbors, (63, 5), FaceDir::PosX),
            Some(12)
        );
        assert!(nav.linked_across(&here, &neighbors, (63, 5), FaceDir::PosX));

        // Push the neighbor floor to delta 3 → the link breaks.
        let mut east_far = Cluster::empty();
        set_sample(&mut east_far, 0, 13, 5, true);
        let neighbors_far = NeighborContext {
            pos_x: Some((&east_far, NAV_LOD)),
            ..NeighborContext::none()
        };
        let nav_far = ClusterNav::compute_nav(&here, &neighbors_far);
        assert!(!nav_far.linked_across(&here, &neighbors_far, (63, 5), FaceDir::PosX));
    }

    #[test]
    fn floor_across_is_none_without_a_neighbor_or_across_a_vertical_face() {
        // An absent face neighbor reads as empty → no neighbor floor;
        // vertical faces have no adjacent nav column at all.
        let mut c = Cluster::empty();
        set_sample(&mut c, 63, 10, 5, true);
        let nav = ClusterNav::compute_nav(&c, &NeighborContext::none());
        let none = NeighborContext::none();
        assert_eq!(nav.floor_across(&c, &none, (63, 5), FaceDir::PosX), None);
        assert_eq!(nav.floor_across(&c, &none, (63, 5), FaceDir::PosY), None);
        assert_eq!(nav.floor_across(&c, &none, (63, 5), FaceDir::NegY), None);
    }

    #[test]
    fn cluster_top_clearance_treats_absent_vertical_neighbor_as_empty() {
        // §6.6. A floor at the very top sample (63) with no pos_y
        // neighbor: clearance cells 64,65,66 fall past the cluster top
        // and, with the vertical neighbor absent, read as empty (open
        // sky) → the floor is walkable.
        let mut c = Cluster::empty();
        set_sample(&mut c, 4, 63, 4, true);
        let nav = ClusterNav::compute_nav(&c, &NeighborContext::none());
        assert_eq!(nav.floor_at(4, 4), Some(63));
    }

    #[test]
    fn compute_nav_is_deterministic() {
        // §6.7. Same inputs twice → identical floor grid.
        let mut c = Cluster::empty();
        set_sample(&mut c, 5, 10, 5, true);
        set_sample(&mut c, 20, 40, 33, true);
        set_sample(&mut c, 63, 1, 0, true);
        let nc = NeighborContext::none();
        let a = ClusterNav::compute_nav(&c, &nc);
        let b = ClusterNav::compute_nav(&c, &nc);
        assert_eq!(a, b);
    }

    #[test]
    fn ring_gate_admits_near_and_rejects_far() {
        // Center camera over the origin cluster: it (and a same-frame
        // neighbor) are well inside ring 2; a cluster thousands of feet
        // away is outside.
        let origin_center = cluster_center_world(ClusterId::new(0, 0, 0, 0));
        assert!(in_nav_rings(origin_center, origin_center));
        // All 9 demo clusters (x,z ∈ 0..3) are admitted from a camera at
        // the field's origin corner — the demo's gate is a no-op today.
        let camera = [0.0, 0.0, 0.0];
        for x in 0..3u16 {
            for z in 0..3u16 {
                let center = cluster_center_world(ClusterId::new(0, x, 0, z));
                assert!(
                    in_nav_rings(camera, center),
                    "demo cluster ({x},0,{z}) should be inside ring 2"
                );
            }
        }
        // A cluster far down the +X axis (center ≫ 1792 ft away) is gated
        // out.
        let far = cluster_center_world(ClusterId::new(0, 100, 0, 0));
        assert!(!in_nav_rings(camera, far));
    }
}
