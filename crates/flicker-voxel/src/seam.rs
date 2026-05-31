//! Cross-cluster and cross-LOD primitives.
//!
//! [`NeighborContext`] gives a contour pass read access to the six
//! face-adjacent clusters, so it can resolve voxel classifications
//! across cluster seams. [`Lod`] describes the level-of-detail level
//! at which a cluster is being contoured. [`read_corner`] is the
//! helper that consults `NeighborContext` when a voxel lookup falls
//! outside the active cluster's bounds.
//!
//! [`NeighborHalo`] carries a slab of vertices contributed by a
//! neighbor's boundary plane, in the **active** cluster's coordinate
//! frame. The active cluster's contour pass consults its halos during
//! quad assembly to fill quad slots that fall just past a face — that
//! is how matched-LOD seams stitch into continuous geometry without
//! cross-cluster index sharing. Each cluster materializes its own
//! halos at contour time and intern-promotes the halo vertices it
//! actually uses; both sides of a seam end up with coincident world
//! positions for the shared boundary vertices, which is visually
//! invisible (coplanar z-fighting is benign).

use crate::cluster::CLUSTER_DIM;
use crate::material::Material;
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

/// Optional voxel data from this cluster's 6 face neighbors, plus each
/// neighbor's intended LOD.
///
/// A face with `None` is a world boundary; a face with `Some` carries
/// the neighbor cluster and its LOD so a contour pass can resolve
/// classifications and corner positions across the seam.
///
/// # Across-face read convention
///
/// Self's `+X` face is the neighbor's `-X` face. When sampling across
/// the seam, self reads neighbor voxels at the neighbor's minimum-X
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
#[allow(dead_code)]
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

/// One vertex contributed by a neighbor cluster's boundary row.
///
/// Same shape as [`crate::Vertex`] but kept separate so the halo's
/// role is explicit at type-check time: a `HaloVertex` is never
/// directly an index into the active cluster's vertex buffer. The
/// contour pass interns the halo vertices it actually uses into its
/// own vertex buffer, at which point they become indistinguishable
/// from in-cluster vertices to downstream consumers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HaloVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// Which face of the local cluster a halo represents. The local
/// cluster's `PosX` face touches the neighbor's `NegX` face, etc.
///
/// This enum doubles as the `(FaceDir, perp_a, perp_b)` key for the
/// contour pass's halo-vertex intern map.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FaceDir {
    NegX,
    PosX,
    NegY,
    PosY,
    NegZ,
    PosZ,
}

/// A slab of vertices contributed by a neighbor cluster's boundary
/// row, ready for the contour pass to query at quad-search time.
///
/// `vertex_at(perp_a, perp_b)` returns `Some` if the neighbor emitted
/// a surface vertex at the boundary-plane position, `None` otherwise.
///
/// Coordinates `(perp_a, perp_b)` are in the **active cluster's**
/// frame projected onto the seam plane. For a `PosX` halo:
/// `perp_a = vy`, `perp_b = vz`. The vertex's `position` is also in
/// the active cluster's frame — for the `PosX` halo at neighbor voxel
/// `(0, vy, vz)`, position reads `[CLUSTER_DIM + δx, vy + δy, vz +
/// δz]`, one voxel past the active cluster's `+X` face.
///
/// At LOD 0 every `(perp_a, perp_b)` slot is sampled; at higher LODs
/// the neighbor's stride is honored and intermediate slots return
/// `None`, so the contour pass naturally accepts mismatched-LOD seams
/// (with cracks at unsampled positions until T-junction fan emission
/// lands).
pub struct NeighborHalo {
    face: FaceDir,
    /// Voxel-coordinate stride between populated `vertices` slots —
    /// equals `lod.stride()` for the LOD passed to [`build_halo`]. The
    /// contour pass rounds query coordinates down to multiples of this
    /// stride before calling [`NeighborHalo::vertex_at`] so it stays a
    /// strict accessor.
    stride: u32,
    /// Flat row-major: `vertices[perp_b * CLUSTER_DIM + perp_a]`.
    /// `None` slots indicate the neighbor did not emit a vertex there
    /// (either no surface voxel or off the LOD stride).
    vertices: Vec<Option<HaloVertex>>,
}

impl NeighborHalo {
    /// The face this halo represents (in the active cluster's frame).
    #[must_use]
    pub fn face(&self) -> FaceDir {
        self.face
    }

    /// Voxel-coordinate stride between populated slots — `1` at LOD 0,
    /// `2` at LOD 1, `2^L` in general. The contour pass divides query
    /// perpendicular coordinates by this value (and multiplies back) to
    /// land on the nearest populated halo position, which is what
    /// makes mismatched-LOD seams collapse adjacent quads into fan
    /// triangles instead of producing cracks.
    #[must_use]
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Look up the vertex at boundary-plane position `(perp_a, perp_b)`.
    /// Both coords must be in `[0, CLUSTER_DIM)`; out-of-range queries
    /// return `None`. The accessor is **strict** — positions that are
    /// not exact multiples of [`Self::stride`] return `None`. Callers
    /// crossing a stride boundary are expected to round the coordinates
    /// down themselves.
    #[must_use]
    pub fn vertex_at(&self, perp_a: u32, perp_b: u32) -> Option<&HaloVertex> {
        if perp_a >= CLUSTER_DIM || perp_b >= CLUSTER_DIM {
            return None;
        }
        let idx = perp_b as usize * (CLUSTER_DIM as usize) + perp_a as usize;
        self.vertices.get(idx).and_then(|opt| opt.as_ref())
    }
}

/// Build a halo from `neighbor` for the active cluster's `face`.
///
/// Walks the neighbor's boundary plane (its `x = 0` for `PosX`, its
/// `x = CLUSTER_DIM - 1` for `NegX`, etc.). For each solid voxel on
/// that plane whose 6-neighbor classifications include at least one
/// non-solid, emits a halo vertex placed at the voxel's owned `+++`
/// corner, with the position translated into the active cluster's
/// frame by `±CLUSTER_DIM` on the across-seam axis.
///
/// Gradient and material come from the neighbor's own classification,
/// with the neighbor's own OOB reads falling back to the neighbor's
/// `base()`. The across-seam gradient component is therefore slightly
/// imperfect (the neighbor sees its across-seam side as empty rather
/// than as the active cluster's actual content), but on matched-
/// heightmap scenes the error is small and a single vertex's normal
/// contribution to a quad's winding decision is dominated by the
/// other three vertices.
///
/// `lod` is honored as a stride through the neighbor's boundary
/// plane — positions not on the stride leave their halo slot `None`.
/// Stride-aware gradient computation is a follow-up; at LOD 0 this
/// distinction does not matter.
#[must_use]
pub fn build_halo(neighbor: &Cluster, face: FaceDir, lod: Lod) -> NeighborHalo {
    let dim = CLUSTER_DIM as usize;
    let dim_i32 = CLUSTER_DIM as i32;
    let stride = lod.stride() as usize;
    let base_solid = neighbor.base().material() != Material::EMPTY;

    let solid_at = |x: i32, y: i32, z: i32| -> bool {
        if (0..dim_i32).contains(&x) && (0..dim_i32).contains(&y) && (0..dim_i32).contains(&z) {
            neighbor
                .get(LocalCoord::new(x as u32, y as u32, z as u32).expect("in bounds"))
                .material()
                != Material::EMPTY
        } else {
            base_solid
        }
    };

    // The neighbor's mesh at LOD L has its `Pos*` boundary plane at
    // sample `sample_dim - 1`, which is voxel `CLUSTER_DIM - 2^L`.
    // The halo must read at that exact voxel so its emitted halo
    // vertices coincide with the neighbor's actual mesh vertex
    // positions. `Pos*` faces remain at voxel 0 — the neighbor's
    // `Neg*` boundary is always at sample 0 = voxel 0 regardless of
    // stride.
    let neighbor_stride = lod.stride() as i32;
    let neg_boundary_voxel = dim_i32 - neighbor_stride;
    let (boundary_axis, boundary_value, shift_value): (usize, i32, f32) = match face {
        FaceDir::PosX => (0, 0, CLUSTER_DIM as f32),
        FaceDir::NegX => (0, neg_boundary_voxel, -(dim_i32 as f32)),
        FaceDir::PosY => (1, 0, CLUSTER_DIM as f32),
        FaceDir::NegY => (1, neg_boundary_voxel, -(dim_i32 as f32)),
        FaceDir::PosZ => (2, 0, CLUSTER_DIM as f32),
        FaceDir::NegZ => (2, neg_boundary_voxel, -(dim_i32 as f32)),
    };
    let (perp_a_axis, perp_b_axis): (usize, usize) = match face {
        FaceDir::NegX | FaceDir::PosX => (1, 2),
        FaceDir::NegY | FaceDir::PosY => (0, 2),
        FaceDir::NegZ | FaceDir::PosZ => (0, 1),
    };

    let mut vertices: Vec<Option<HaloVertex>> = vec![None; dim * dim];

    let mut perp_b = 0usize;
    while perp_b < dim {
        let mut perp_a = 0usize;
        while perp_a < dim {
            let mut nv = [0i32; 3];
            nv[boundary_axis] = boundary_value;
            nv[perp_a_axis] = perp_a as i32;
            nv[perp_b_axis] = perp_b as i32;

            if solid_at(nv[0], nv[1], nv[2]) {
                // Stride-aware 6-neighbor lookups so the surface
                // predicate matches what the neighbor's contour pass
                // sees at its own LOD. At LOD 0 (stride 1) these
                // are single-voxel reads as before; at LOD > 0 each
                // axis offset is `±neighbor_stride`, so the gradient
                // and the surface predicate identify the sample-grid
                // sign-changes the neighbor's mesh would emit at.
                let nxn = solid_at(nv[0] - neighbor_stride, nv[1], nv[2]);
                let nxp = solid_at(nv[0] + neighbor_stride, nv[1], nv[2]);
                let nyn = solid_at(nv[0], nv[1] - neighbor_stride, nv[2]);
                let nyp = solid_at(nv[0], nv[1] + neighbor_stride, nv[2]);
                let nzn = solid_at(nv[0], nv[1], nv[2] - neighbor_stride);
                let nzp = solid_at(nv[0], nv[1], nv[2] + neighbor_stride);
                let any_nonsolid = !nxn || !nxp || !nyn || !nyp || !nzn || !nzp;
                if any_nonsolid {
                    let vox = neighbor.get(
                        LocalCoord::new(nv[0] as u32, nv[1] as u32, nv[2] as u32)
                            .expect("in bounds"),
                    );
                    let [dx, dy, dz] = vox.corner().to_components();
                    let mut position = [nv[0] as f32 + dx, nv[1] as f32 + dy, nv[2] as f32 + dz];
                    position[boundary_axis] += shift_value;

                    let to_i = |b: bool| -> i32 {
                        if b {
                            1
                        } else {
                            0
                        }
                    };
                    let gx = to_i(nxp) - to_i(nxn);
                    let gy = to_i(nyp) - to_i(nyn);
                    let gz = to_i(nzp) - to_i(nzn);
                    let nxf = -(gx as f32);
                    let nyf = -(gy as f32);
                    let nzf = -(gz as f32);
                    let len = (nxf * nxf + nyf * nyf + nzf * nzf).sqrt();
                    let normal = if len > 0.0 {
                        [nxf / len, nyf / len, nzf / len]
                    } else {
                        [0.0, 1.0, 0.0]
                    };

                    let idx = perp_b * dim + perp_a;
                    vertices[idx] = Some(HaloVertex {
                        position,
                        normal,
                        material: vox.material().raw(),
                    });
                }
            }

            perp_a += stride;
        }
        perp_b += stride;
    }

    NeighborHalo {
        face,
        stride: stride as u32,
        vertices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_vector::CornerVector;
    use crate::generators::solid_slab;

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

    // ---- build_halo ----

    #[test]
    fn halo_round_trips_neighbor_boundary_vertices() {
        // The neighbor is a half-slab; for `FaceDir::PosX` the halo
        // walks the neighbor's `x = 0` plane. At the topmost solid
        // layer (vy = 127) every column has +Y empty → every
        // `(vy=127, vz)` cell becomes a halo vertex.
        let neighbor = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let halo = build_halo(&neighbor, FaceDir::PosX, Lod::ZERO);
        assert_eq!(halo.face(), FaceDir::PosX);

        let mut found = 0u32;
        for vz in 0..CLUSTER_DIM {
            if halo.vertex_at(127, vz).is_some() {
                found += 1;
            }
        }
        assert_eq!(
            found, CLUSTER_DIM,
            "expected halo vertex per z at vy=127; got {found}"
        );

        // Spot-check one halo vertex: position is in the active
        // cluster's frame, so the across-seam axis sits one cluster
        // beyond the active cluster's far face.
        let hv = halo.vertex_at(127, 128).expect("vy=127, vz=128 surface");
        assert!(
            (hv.position[0] - (CLUSTER_DIM as f32 + 0.5)).abs() < 0.05,
            "halo position[0] = {} expected ≈ {} (one voxel past +X face)",
            hv.position[0],
            CLUSTER_DIM as f32 + 0.5
        );
        // Material is the neighbor voxel's material.
        let m = Material::new(7, 7, 7).unwrap();
        assert_eq!(hv.material, m.raw());
    }

    #[test]
    fn halo_out_of_range_query_returns_none() {
        let neighbor = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let halo = build_halo(&neighbor, FaceDir::PosX, Lod::ZERO);
        // Coordinates ≥ CLUSTER_DIM return None rather than panicking.
        assert!(halo.vertex_at(CLUSTER_DIM, 0).is_none());
        assert!(halo.vertex_at(0, CLUSTER_DIM).is_none());
    }

    #[test]
    fn halo_stride_field_matches_lod() {
        // `stride()` reports the voxel-coord stride between populated
        // slots: 1, 2, 4, … for LOD 0, 1, 2, … respectively.
        let neighbor = solid_slab(128, Material::new(7, 7, 7).unwrap());
        for level in 0..=4u8 {
            let lod = Lod::new(level).unwrap();
            let halo = build_halo(&neighbor, FaceDir::PosX, lod);
            assert_eq!(
                halo.stride(),
                lod.stride(),
                "halo at LOD {level}: stride {} should equal lod.stride() {}",
                halo.stride(),
                lod.stride()
            );
        }
    }

    #[test]
    fn halo_at_lod_1_returns_none_at_odd_positions() {
        // At LOD 1 the halo only populates positions where both
        // perp_a and perp_b are even (stride 2). Odd positions stay
        // `None`; the contour pass rounds queries down before
        // calling vertex_at.
        let neighbor = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let halo = build_halo(&neighbor, FaceDir::PosX, Lod::new(1).unwrap());
        // vy=127 is odd → no halo vertex at any vz.
        assert!(
            halo.vertex_at(127, 0).is_none(),
            "vy=127 (odd) should be None at LOD 1"
        );
        // vy=126 is even → halo vertex present (top-of-slab column).
        assert!(
            halo.vertex_at(126, 0).is_some(),
            "vy=126 (even) should carry a halo vertex at LOD 1"
        );
    }

    #[test]
    fn neg_face_halo_position_respects_neighbor_stride() {
        // A LOD 1 neighbor's PosX boundary plane is at voxel
        // `vx = 254`, not 255 — the neighbor at LOD 1 only samples
        // even voxels, so its last in-stride X column is at 254.
        // The halo built for the local cluster's NegX face (= the
        // neighbor's PosX side) must read at that voxel and place
        // the resulting halo vertex at the matching world position.
        //
        // Off-by-one (boundary at vx=255 instead of 254) pushes the
        // halo one voxel into the seam gap and leaves a visible
        // crack between the local mesh and the neighbor's mesh.
        let neighbor = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let halo = build_halo(&neighbor, FaceDir::NegX, Lod::new(1).unwrap());
        // The neighbor's top-of-slab surface at LOD 1 sits at voxel
        // vy=126 (the last even-aligned solid row); vz=128 is an
        // arbitrary even column inside the cluster.
        let hv = halo.vertex_at(126, 128).expect("vy=126 top-of-slab");
        // Position in the active cluster's frame: vx_neighbor=254
        // plus cv_x (≈0.5) minus CLUSTER_DIM (256) = -1.5.
        assert!(
            (hv.position[0] - (-1.5)).abs() < 0.05,
            "NegX LOD 1 halo position[0] = {} expected ≈ -1.5 \
             (was previously -0.5 due to stride-ignoring boundary_value)",
            hv.position[0]
        );
    }
}
