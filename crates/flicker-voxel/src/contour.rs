//! Single-cluster dual contouring at LOD 0.
//!
//! Phase 2 mesh generation. Pure data — input is a [`Cluster`], output is a
//! [`CellMesh`] of CPU-side vertex and index buffers. No graphics, no
//! threading, no inter-cluster neighbor reads.
//!
//! # Algorithm
//!
//! The voxel grid defines a binary scalar field by surface classification:
//! a voxel is **solid** if its material is not [`Material::EMPTY`], and
//! **not-solid** otherwise. The surface lies between solid and not-solid
//! voxels.
//!
//! A **cell** is the unit cube spanned by 8 adjacent voxel positions —
//! cell `(cx, cy, cz)` has corners at voxel positions `(cx+i, cy+j, cz+k)`
//! for `i, j, k ∈ {0, 1}`. With 256 voxels per axis we have 255 cells per
//! axis (255³ cells per cluster).
//!
//! For each cell whose 8 corner-voxels are not all the same classification:
//!
//! 1. Emit **one** vertex.
//! 2. Position = the centroid (mean) of the 8 voxels' owned corner
//!    positions. Each voxel at `(vx, vy, vz)` owns a corner at world-position
//!    `(vx + δx, vy + δy, vz + δz)` where `(δx, δy, δz)` is the decoded
//!    corner vector. The centroid is what couples corner-vector encoding
//!    to mesh geometry.
//! 3. Normal = `-gradient(classification)`, computed via a 4-sample central
//!    difference at the cell center, then normalized. Points away from
//!    solid voxels (out of the surface).
//! 4. Material = the material of the solid corner-voxel whose owned corner
//!    is closest to the centroid; ties broken by iteration order
//!    `(k, j, i)`.
//!
//! For each voxel-grid edge between two adjacent voxels with different
//! classifications, emit a quad (two triangles) connecting the 4 cells that
//! share that edge. Winding is chosen per-axis so the triangle normals
//! point from solid to not-solid.
//!
//! # Performance posture
//!
//! Phase 2 prioritizes correctness and clarity over speed. The uniform-
//! cluster shortcut returns immediately without scanning. After that, the
//! algorithm pre-classifies every voxel into a flat `Vec<bool>` (16 MB) so
//! the inner all-equal check is array indexing, and uses a flat
//! `Vec<u32>` (66 MB) as the cell→vertex index map with `u32::MAX` as
//! "no vertex" sentinel. Cells and edges are walked in deterministic
//! `(z, y, x)` order so output is byte-identical across runs.
//!
//! # Phase 4: seam handling via the extended cell grid
//!
//! [`contour_cluster_lod_with_neighbors`] accepts a [`NeighborContext`]
//! and, for each face whose neighbor is `Some`, extends the cell-grid
//! iteration range by one cell across that face. The extra row of
//! cells straddles the seam — its in-face corners come from `self`'s
//! interior voxels, its across-face corners come from the neighbor's
//! voxels (read through [`read_corner`]). These boundary cells go
//! through the same vertex/normal/material/quad logic as interior
//! cells; only the corner source changes.
//!
//! There is no separate seam mesh, no secondary baking pass, and no
//! seam metadata. Seam alignment is a runtime property of how each
//! cluster bakes its own boundary cells.
//!
//! ## Byte-equal world positions
//!
//! Two adjacent clusters A and B sharing a face each extend their cell
//! grids onto the seam plane. For the shared boundary row, each
//! cluster's `i = 0` corners read voxels owned by the lower-coord
//! cluster and the `i = 1` corners read voxels owned by the higher-
//! coord cluster — symmetrically across both sides.
//!
//! Each boundary cell computes corner positions in a **shared world
//! frame** (the lower-coord cluster's local frame). `self`'s offset
//! into that world frame is `0` on a `+axis` face (self IS the lower
//! cluster) or `CLUSTER_DIM` on a `-axis` face (self is the higher
//! cluster). Corners are summed in the canonical `(k, j, i)` iteration
//! order and divided by 8 to produce a world-frame centroid. The
//! result is converted to self's local frame by a single subtraction
//! of self's world offset.
//!
//! Because both clusters read the same voxel data, apply the same f32
//! arithmetic in the same order, and operate in the same world frame,
//! their boundary vertices land at byte-identical world positions.
//! When the meshes are combined and B's vertices are translated by
//! `+CLUSTER_DIM` on the seam axis, the boundary triangles from each
//! side overlap perfectly. Each cluster owns its own copy of the
//! boundary triangles in its own index space, so the per-cluster
//! edge histogram remains all-2s.
//!
//! Interior cells (where self's world offset is `0` and no neighbor
//! reads occur) compute identically to Phase 3 — adding `0.0` and
//! subtracting `0.0` is exact in IEEE-754, so `cluster.get` reads
//! plus a `read_corner` wrapper that delegates to `cluster.get` on
//! in-range coordinates produce byte-identical output to Phase 3 when
//! the [`NeighborContext`] is all-`None`.
//!
//! ## Mixed-LOD seams
//!
//! When two adjacent clusters have different LODs:
//!
//! - **The fine side does nothing different.** It extends its boundary
//!   row in the main loop at self's native stride, reads neighbor
//!   corners through [`read_corner`], and emits cells exactly as in the
//!   equal-LOD case.
//! - **The coarse side refines its boundary row to the neighbor's
//!   (finer) stride.** The main loop terminates at `cell_dim` on the
//!   affected face (no extension), and a separate per-face fine-stride
//!   pass emits the boundary row.
//!
//! The fine-stride boundary slab sits flush against the seam plane,
//! one fine-cell thick on the across axis (X, Y, or Z for the affected
//! face) and at fine stride on the two in-face axes. This positioning
//! ensures both clusters across the seam read the same 8 voxel
//! coordinates per boundary cell, so the byte-equality argument carries
//! over from the equal-LOD case unchanged.
//!
//! Each fine boundary cell's vertex index is stored in a per-face
//! `HashMap<(i32, i32), u32>` keyed by the two in-face fine cell
//! coordinates. Face emission within the boundary slab uses these maps.
//! For each fine cross-seam voxel-edge that sign-changes, the algorithm
//! emits either:
//!
//! - **Fan triangles**: when the matching coarse interior cell at
//!   `(cell_dim - 1, cy_c, cz_c)` (or `(0, …)` on a `-axis` face) has
//!   a vertex in `cell_vertex`. Two triangles are emitted, both sharing
//!   the coarse interior vertex; the four bracketing fine boundary
//!   cells supply the remaining triangle vertices. The fan replaces the
//!   standard pass-2 X-axis quad split at this edge.
//! - **Standard quad**: when the matching coarse interior cell has no
//!   vertex (typical of uniform coarse interiors). Four fine boundary
//!   cells form the standard quad split.
//!
//! T-junction fan triangles on the fine side's interior-to-boundary
//! transition are accepted for this phase. They do not produce cracks
//! because the fine side's boundary cells read the same voxel data as
//! the coarse side's fine boundary slab.
//!
//! ## Edge and corner neighbors
//!
//! Face neighbors only. When a boundary cell's corner has two or three
//! voxel coordinates out of self's range simultaneously (an edge or
//! corner of the cluster), [`read_corner`] falls back to
//! `cluster.base()` rather than chaining through neighbors-of-
//! neighbors.
//!
//! # Future extensions (post-Phase 4)
//!
//! - QEF-based vertex placement using Hermite edge intersections, for
//!   sharp-feature preservation. Slots in at the centroid step without
//!   changing face emission.
//! - Worker-thread invocation (Phase 5) — the function is already pure
//!   and `Send`-compatible.

use std::collections::HashMap;

use crate::cluster::CLUSTER_DIM;
use crate::{Cluster, LocalCoord, Material, Voxel};

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
/// When [`contour_cluster_lod_with_neighbors`] sees a face neighbor at a
/// **coarser** LOD than `self_lod`, it emits boundary geometry on that
/// face at the neighbor's coarse stride so the two clusters' surfaces
/// connect at the seam. A face with `None` is a world boundary; a face
/// with an equal-or-finer neighbor receives no special handling — the
/// fine side is responsible for seam closure.
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

impl<'a> NeighborContext<'a> {
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

/// One mesh vertex.
///
/// Position is in cluster-local coordinates (each axis in `[0, 256]`).
/// Normal is unit-length and points away from solid voxels. Material is
/// the packed 12/12/8 representation returned by [`Material::raw`].
///
/// The layout is `#[repr(C)]` and plain data — 28 bytes — so a downstream
/// graphics crate can view it as `bytemuck::Pod` without this crate having
/// to depend on `bytemuck`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// Mesh index buffer; width is chosen automatically based on vertex count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Indices {
    /// Indices fit in 16 bits (vertex count `≤ 65536`).
    U16(Vec<u16>),
    /// Vertex count exceeds 16 bits; 32-bit indices needed.
    U32(Vec<u32>),
}

impl Indices {
    /// Total index count (always a multiple of 3).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Indices::U16(v) => v.len(),
            Indices::U32(v) => v.len(),
        }
    }

    /// `true` if the buffer holds no indices.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of triangles.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.len() / 3
    }

    /// `true` iff this buffer uses the compact 16-bit representation.
    #[must_use]
    pub fn is_u16(&self) -> bool {
        matches!(self, Indices::U16(_))
    }
}

/// Descriptive metadata about a contoured mesh.
///
/// Plain data — public fields, no encapsulation. `vertex_count` and
/// `triangle_count` are snapshots taken at construction time and mirror the
/// buffer lengths exactly.
///
/// For an empty mesh, both `bounds_min` and `bounds_max` are
/// `[0.0, 0.0, 0.0]` (the degenerate box).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MeshMetadata {
    /// LOD level the mesh was contoured at. Always `0` in Phase 2.
    pub lod: u32,
    /// Source cluster dimension in voxels (256 in Phase 2).
    pub cluster_dim: u32,
    /// Vertex count. Mirrors `CellMesh::vertices().len()`.
    pub vertex_count: usize,
    /// Triangle count. Mirrors `CellMesh::indices().triangle_count()`.
    pub triangle_count: usize,
    /// Axis-aligned bounding box minimum in cluster-local coordinates.
    /// For an empty mesh, this is `[0.0, 0.0, 0.0]`.
    pub bounds_min: [f32; 3],
    /// Axis-aligned bounding box maximum in cluster-local coordinates.
    /// For an empty mesh, this is `[0.0, 0.0, 0.0]`.
    pub bounds_max: [f32; 3],
}

/// Output of [`contour_cluster`].
#[derive(Debug)]
pub struct CellMesh {
    vertices: Vec<Vertex>,
    indices: Indices,
    metadata: MeshMetadata,
}

impl CellMesh {
    /// All vertices in submission order.
    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// The index buffer.
    #[must_use]
    pub fn indices(&self) -> &Indices {
        &self.indices
    }

    /// Descriptive metadata — LOD, dimensions, counts, bounds.
    #[must_use]
    pub fn metadata(&self) -> &MeshMetadata {
        &self.metadata
    }

    /// `true` iff the mesh has no geometry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

// ===== Internals =====

#[inline]
fn is_voxel_solid(v: Voxel) -> bool {
    v.material() != Material::EMPTY
}

fn empty_mesh(lod: Lod) -> CellMesh {
    CellMesh {
        vertices: Vec::new(),
        indices: Indices::U16(Vec::new()),
        metadata: MeshMetadata {
            lod: lod.level() as u32,
            cluster_dim: CLUSTER_DIM,
            vertex_count: 0,
            triangle_count: 0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
        },
    }
}

/// Contour `cluster` at LOD 0 (full resolution). Convenience wrapper around
/// [`contour_cluster_lod`].
#[must_use]
pub fn contour_cluster(cluster: &Cluster) -> CellMesh {
    contour_cluster_lod(cluster, Lod::ZERO)
}

/// Contour `cluster` at the requested LOD with no neighbor context.
/// Equivalent to [`contour_cluster_lod_with_neighbors`] called with an
/// all-`None` [`NeighborContext`].
#[must_use]
pub fn contour_cluster_lod(cluster: &Cluster, lod: Lod) -> CellMesh {
    contour_cluster_lod_with_neighbors(cluster, lod, &NeighborContext::none())
}

/// Contour `cluster` into a [`CellMesh`] at the requested level of detail,
/// using `neighbors` to align boundary cells across seams.
///
/// At LOD `L` the algorithm reads every `2^L`-th voxel along each axis.
/// LOD 0 is full resolution and is bit-identical to [`contour_cluster`].
/// Vertex positions are always expressed in cluster-local coordinates
/// (`0..256` per axis) regardless of LOD, so meshes at different LODs
/// share a world frame.
///
/// # Seam handling
///
/// For each face whose neighbor is `Some`, the cell-grid iteration range
/// extends by one cell row across that face. The extra row of boundary
/// cells reads voxel data through [`read_corner`] so the across-face
/// corners come from the neighbor cluster. These boundary cells go through
/// the same vertex/normal/material/quad logic as interior cells.
///
/// Adjacent clusters at the same LOD each emit their own copy of the
/// boundary triangles. Their boundary vertices land at byte-identical
/// world positions because both compute the centroid in a shared world
/// frame (the lower-coord cluster's local frame) with the same f32
/// arithmetic in the same iteration order. Each cluster stores the result
/// in its own local frame.
///
/// With `neighbors = &NeighborContext::none()` the iteration ranges are
/// unchanged from Phase 3, no `read_corner` redirection occurs, and the
/// extra world-offset arithmetic reduces to `+ 0.0` (exact), so the output
/// is byte-identical to [`contour_cluster_lod`].
///
/// Equal-LOD seams close cleanly. Mixed-LOD seams are handled only when
/// the neighbor is the same or finer LOD; coarser-neighbor handling and
/// T-junction fan triangles are deferred.
///
/// Deterministic: the same `(cluster, lod, neighbors)` input always
/// produces byte-identical output.
#[must_use]
pub fn contour_cluster_lod_with_neighbors(
    cluster: &Cluster,
    lod: Lod,
    neighbors: &NeighborContext<'_>,
) -> CellMesh {
    let stride = lod.stride();
    let stride_mask = stride - 1; // valid because stride is a power of two
    let stride_i32 = stride as i32;
    let dim_i32 = CLUSTER_DIM as i32;

    // --- Shortcut: no visible override has a classification differing from
    // self's base AND no neighbor could contribute a seam surface. ---
    let base_solid = is_voxel_solid(cluster.base());
    let has_visible_difference = cluster.overrides().any(|(coord, voxel)| {
        let visible = (coord.x() & stride_mask) == 0
            && (coord.y() & stride_mask) == 0
            && (coord.z() & stride_mask) == 0;
        visible && is_voxel_solid(voxel) != base_solid
    });
    let neighbor_list = [
        neighbors.neg_x,
        neighbors.pos_x,
        neighbors.neg_y,
        neighbors.pos_y,
        neighbors.neg_z,
        neighbors.pos_z,
    ];
    let neighbor_could_contribute = neighbor_list.iter().any(|n| match n {
        Some((nb, _)) => {
            is_voxel_solid(nb.base()) != base_solid
                || nb.overrides().any(|(_, v)| is_voxel_solid(v) != base_solid)
        }
        None => false,
    });
    if !has_visible_difference && !neighbor_could_contribute {
        return empty_mesh(lod);
    }

    // --- Pre-classify self's LOD-visible voxels into a flat array. ---
    //
    // Memory: `sample_dim^3` bytes — 16 MB at LOD 0 down to 8 bytes at
    // LOD 7. Out-of-range classification reads (extended-row cell corners
    // landing on neighbor voxels) route through `read_corner`; they are
    // not stored in this array.
    let sample_dim = lod.sample_dim() as usize;
    let sample_dim_i32 = sample_dim as i32;
    let row_stride = sample_dim;
    let slab_stride = sample_dim * sample_dim;
    let voxel_idx = |x: u32, y: u32, z: u32| -> usize {
        x as usize + y as usize * row_stride + z as usize * slab_stride
    };
    let mut is_solid = vec![base_solid; sample_dim * sample_dim * sample_dim];
    for (coord, voxel) in cluster.overrides() {
        // Only stride-aligned overrides participate in the LOD view; the
        // rest are invisible at this LOD (no interpolation across skipped
        // voxels).
        if (coord.x() & stride_mask) == 0
            && (coord.y() & stride_mask) == 0
            && (coord.z() & stride_mask) == 0
        {
            let vx = coord.x() / stride;
            let vy = coord.y() / stride;
            let vz = coord.z() / stride;
            is_solid[voxel_idx(vx, vy, vz)] = is_voxel_solid(voxel);
        }
    }
    let solid_at = |sx: i32, sy: i32, sz: i32| -> bool {
        if (0..sample_dim_i32).contains(&sx)
            && (0..sample_dim_i32).contains(&sy)
            && (0..sample_dim_i32).contains(&sz)
        {
            is_solid[voxel_idx(sx as u32, sy as u32, sz as u32)]
        } else {
            is_voxel_solid(read_corner(
                cluster,
                neighbors,
                sx * stride_i32,
                sy * stride_i32,
                sz * stride_i32,
            ))
        }
    };

    // --- Extended cell-grid bounds. ---
    //
    // Cell coords are signed: a `cx = -1` cell straddles the `-X` seam,
    // a `cx = cell_dim` cell straddles the `+X` seam. The extension is
    // applied independently on each face based on `NeighborContext`. With
    // every neighbor `None` the bounds reduce to `0..cell_dim`, matching
    // the Phase 3 iteration exactly.
    //
    // When the neighbor on a face is **finer** than self, the main loop
    // does NOT extend on that face — that face's boundary row is emitted
    // by a separate fine-stride pass after the main loop (see "Mixed-LOD
    // boundary passes" below). When the neighbor is equal-or-coarser, the
    // main loop extends as in the equal-LOD case.
    let extend_main = |n: Option<(&Cluster, Lod)>| -> bool {
        match n {
            Some((_, n_lod)) => n_lod.stride() >= stride,
            None => false,
        }
    };
    let cell_dim = lod.cell_dim() as i32;
    let cx_lo: i32 = if extend_main(neighbors.neg_x) { -1 } else { 0 };
    let cx_hi: i32 = cell_dim + i32::from(extend_main(neighbors.pos_x));
    let cy_lo: i32 = if extend_main(neighbors.neg_y) { -1 } else { 0 };
    let cy_hi: i32 = cell_dim + i32::from(extend_main(neighbors.pos_y));
    let cz_lo: i32 = if extend_main(neighbors.neg_z) { -1 } else { 0 };
    let cz_hi: i32 = cell_dim + i32::from(extend_main(neighbors.pos_z));
    let cell_extent_x = (cx_hi - cx_lo) as usize;
    let cell_extent_y = (cy_hi - cy_lo) as usize;
    let cell_extent_z = (cz_hi - cz_lo) as usize;
    let cell_idx = |cx: i32, cy: i32, cz: i32| -> usize {
        (cx - cx_lo) as usize
            + (cy - cy_lo) as usize * cell_extent_x
            + (cz - cz_lo) as usize * cell_extent_x * cell_extent_y
    };

    let mut vertices: Vec<Vertex> = Vec::new();
    // `cell_vertex[cell_idx(cx, cy, cz)] == u32::MAX` means no vertex; else
    // it's the index into `vertices`.
    let mut cell_vertex: Vec<u32> = vec![u32::MAX; cell_extent_x * cell_extent_y * cell_extent_z];

    let mut bounds_min = [f32::INFINITY; 3];
    let mut bounds_max = [f32::NEG_INFINITY; 3];

    // --- Pass 1: scan cells, emit vertices. ---
    //
    // For boundary cells on a `-axis` face (cell coord negative on that
    // axis), self is the higher-coord cluster in the shared world frame
    // and its world offset on that axis is `CLUSTER_DIM`. For `+axis` and
    // interior cells the offset is `0`. Corner positions are computed in
    // the world frame; the centroid is converted to self's local frame
    // by subtracting the world offset.
    //
    // When the offset is 0 the arithmetic reduces to `vx as f32 + δ` and
    // `centroid - 0.0` — both exact in IEEE-754 — so interior cells with
    // no neighbor reads produce byte-identical results to Phase 3.
    for cz in cz_lo..cz_hi {
        let woz_int: i32 = if cz < 0 { dim_i32 } else { 0 };
        let woz_f32: f32 = woz_int as f32;
        for cy in cy_lo..cy_hi {
            let woy_int: i32 = if cy < 0 { dim_i32 } else { 0 };
            let woy_f32: f32 = woy_int as f32;
            for cx in cx_lo..cx_hi {
                let wox_int: i32 = if cx < 0 { dim_i32 } else { 0 };
                let wox_f32: f32 = wox_int as f32;

                // Read the 8 classifications. Index encoding: bit 0 = X bit,
                // bit 1 = Y bit, bit 2 = Z bit. So idx = i + 2j + 4k.
                let mut classes = [false; 8];
                let mut any_solid = false;
                let mut all_solid = true;
                for k in 0..=1i32 {
                    for j in 0..=1i32 {
                        for i in 0..=1i32 {
                            let s = solid_at(cx + i, cy + j, cz + k);
                            let idx = (i + (j << 1) + (k << 2)) as usize;
                            classes[idx] = s;
                            any_solid |= s;
                            all_solid &= s;
                        }
                    }
                }
                if any_solid == all_solid {
                    continue;
                }

                // Mixed cell — fetch the 8 voxels (for corner vectors and
                // materials) and compute the vertex.
                //
                // Corner `(cx+i, cy+j, cz+k)` reads voxel at local
                // coordinate `((cx+i)*stride, ...)`. Out-of-range reads
                // route to face neighbors through `read_corner` (and fall
                // back to `cluster.base()` on edges/corners and absent
                // neighbors). Owned corner positions in the shared world
                // frame are `(vx_local + world_offset) as f32 + δ`.
                let mut corners = [[0.0_f32; 3]; 8];
                let mut materials = [Material::EMPTY; 8];
                for k in 0..=1i32 {
                    for j in 0..=1i32 {
                        for i in 0..=1i32 {
                            let vx_local = (cx + i) * stride_i32;
                            let vy_local = (cy + j) * stride_i32;
                            let vz_local = (cz + k) * stride_i32;
                            let v = read_corner(cluster, neighbors, vx_local, vy_local, vz_local);
                            let [dx, dy, dz] = v.corner().to_components();
                            let idx = (i + (j << 1) + (k << 2)) as usize;
                            corners[idx] = [
                                (vx_local + wox_int) as f32 + dx,
                                (vy_local + woy_int) as f32 + dy,
                                (vz_local + woz_int) as f32 + dz,
                            ];
                            materials[idx] = v.material();
                        }
                    }
                }

                // Centroid of the 8 owned corners in the shared world frame.
                let mut centroid_world = [0.0_f32; 3];
                for c in &corners {
                    centroid_world[0] += c[0];
                    centroid_world[1] += c[1];
                    centroid_world[2] += c[2];
                }
                centroid_world[0] /= 8.0;
                centroid_world[1] /= 8.0;
                centroid_world[2] /= 8.0;

                // Convert to self's local frame. With zero world offset
                // this is `- 0.0`, which is exact.
                let centroid = [
                    centroid_world[0] - wox_f32,
                    centroid_world[1] - woy_f32,
                    centroid_world[2] - woz_f32,
                ];

                // Normal = -gradient(classification). Central differences
                // over the 2³ corner grid: for each pair of corners
                // differing only in one axis, take (high-axis class -
                // low-axis class), sum the 4 pairs per axis, negate
                // (gradient points into solid; we want pointing out).
                let to_i = |b: bool| -> i32 {
                    if b {
                        1
                    } else {
                        0
                    }
                };
                let dx_sum: i32 = (to_i(classes[0b001]) - to_i(classes[0b000]))
                    + (to_i(classes[0b011]) - to_i(classes[0b010]))
                    + (to_i(classes[0b101]) - to_i(classes[0b100]))
                    + (to_i(classes[0b111]) - to_i(classes[0b110]));
                let dy_sum: i32 = (to_i(classes[0b010]) - to_i(classes[0b000]))
                    + (to_i(classes[0b011]) - to_i(classes[0b001]))
                    + (to_i(classes[0b110]) - to_i(classes[0b100]))
                    + (to_i(classes[0b111]) - to_i(classes[0b101]));
                let dz_sum: i32 = (to_i(classes[0b100]) - to_i(classes[0b000]))
                    + (to_i(classes[0b101]) - to_i(classes[0b001]))
                    + (to_i(classes[0b110]) - to_i(classes[0b010]))
                    + (to_i(classes[0b111]) - to_i(classes[0b011]));
                let nx = -(dx_sum as f32);
                let ny = -(dy_sum as f32);
                let nz = -(dz_sum as f32);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let normal = if len > 0.0 {
                    [nx / len, ny / len, nz / len]
                } else {
                    // All 8 corners mixed but zero gradient — pathological
                    // configuration. Pick a stable fallback.
                    [0.0, 1.0, 0.0]
                };

                // Material: closest-owned-corner solid voxel. Compare
                // corner-in-local-frame to centroid-in-local-frame so the
                // comparison matches Phase 3 byte-for-byte on interior
                // cells.
                let mut best_d2 = f32::INFINITY;
                let mut best_material = Material::EMPTY;
                for idx in 0..8 {
                    if !classes[idx] {
                        continue;
                    }
                    let c = [
                        corners[idx][0] - wox_f32,
                        corners[idx][1] - woy_f32,
                        corners[idx][2] - woz_f32,
                    ];
                    let dx = c[0] - centroid[0];
                    let dy = c[1] - centroid[1];
                    let dz = c[2] - centroid[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_material = materials[idx];
                    }
                }

                let vid = vertices.len() as u32;
                cell_vertex[cell_idx(cx, cy, cz)] = vid;
                vertices.push(Vertex {
                    position: centroid,
                    normal,
                    material: best_material.raw(),
                });

                for axis in 0..3 {
                    if centroid[axis] < bounds_min[axis] {
                        bounds_min[axis] = centroid[axis];
                    }
                    if centroid[axis] > bounds_max[axis] {
                        bounds_max[axis] = centroid[axis];
                    }
                }
            }
        }
    }

    // --- Pass 2: face emission per axis. ---
    //
    // For each voxel-grid edge between two adjacent samples with different
    // classifications, the 4 cells sharing that edge form a quad of mesh
    // vertices. Per-axis winding is chosen so the triangle normal points
    // from solid to not-solid; flipped when the lower-coord side is
    // not-solid.
    //
    //   X-axis edge (vx,vy,vz)→(vx+1,vy,vz): 4 cells in YZ plane at X=vx.
    //     Natural winding (A=ll, B=hl, C=hh, D=lh) gives +X normal.
    //   Y-axis edge: 4 cells in XZ plane at Y=vy.
    //     Natural winding gives -Y normal.
    //   Z-axis edge: 4 cells in XY plane at Z=vz.
    //     Natural winding gives +Z normal.
    //
    // Loop bounds use the extended cell range so seam-crossing edges
    // have all 4 surrounding cells available from both interior and
    // boundary rows.
    let mut indices_u32: Vec<u32> = Vec::new();

    // X-axis edges.
    for vz in (cz_lo + 1)..cz_hi {
        for vy in (cy_lo + 1)..cy_hi {
            for vx in cx_lo..cx_hi {
                let a_solid = solid_at(vx, vy, vz);
                let b_solid = solid_at(vx + 1, vy, vz);
                if a_solid == b_solid {
                    continue;
                }
                let va = cell_vertex[cell_idx(vx, vy - 1, vz - 1)];
                let vb = cell_vertex[cell_idx(vx, vy, vz - 1)];
                let vc = cell_vertex[cell_idx(vx, vy, vz)];
                let vd = cell_vertex[cell_idx(vx, vy - 1, vz)];
                if va == u32::MAX || vb == u32::MAX || vc == u32::MAX || vd == u32::MAX {
                    continue;
                }
                if a_solid {
                    indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                } else {
                    indices_u32.extend_from_slice(&[va, vc, vb, va, vd, vc]);
                }
            }
        }
    }

    // Y-axis edges.
    for vz in (cz_lo + 1)..cz_hi {
        for vy in cy_lo..cy_hi {
            for vx in (cx_lo + 1)..cx_hi {
                let a_solid = solid_at(vx, vy, vz);
                let b_solid = solid_at(vx, vy + 1, vz);
                if a_solid == b_solid {
                    continue;
                }
                let va = cell_vertex[cell_idx(vx - 1, vy, vz - 1)];
                let vb = cell_vertex[cell_idx(vx, vy, vz - 1)];
                let vc = cell_vertex[cell_idx(vx, vy, vz)];
                let vd = cell_vertex[cell_idx(vx - 1, vy, vz)];
                if va == u32::MAX || vb == u32::MAX || vc == u32::MAX || vd == u32::MAX {
                    continue;
                }
                if a_solid {
                    indices_u32.extend_from_slice(&[va, vd, vc, va, vc, vb]);
                } else {
                    indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                }
            }
        }
    }

    // Z-axis edges.
    for vz in cz_lo..cz_hi {
        for vy in (cy_lo + 1)..cy_hi {
            for vx in (cx_lo + 1)..cx_hi {
                let a_solid = solid_at(vx, vy, vz);
                let b_solid = solid_at(vx, vy, vz + 1);
                if a_solid == b_solid {
                    continue;
                }
                let va = cell_vertex[cell_idx(vx - 1, vy - 1, vz)];
                let vb = cell_vertex[cell_idx(vx, vy - 1, vz)];
                let vc = cell_vertex[cell_idx(vx, vy, vz)];
                let vd = cell_vertex[cell_idx(vx - 1, vy, vz)];
                if va == u32::MAX || vb == u32::MAX || vc == u32::MAX || vd == u32::MAX {
                    continue;
                }
                if a_solid {
                    indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                } else {
                    indices_u32.extend_from_slice(&[va, vc, vb, va, vd, vc]);
                }
            }
        }
    }

    // --- Mixed-LOD: per-face fine-stride boundary passes. ---
    //
    // For each face whose neighbor is finer than self, the main loop has
    // already declined to extend on that face. Here we emit a thin slab
    // of fine boundary cells at the neighbor's stride, append their
    // vertices to `vertices`, and connect them to each other (in-plane
    // emission) and to the coarse interior (fan emission).
    //
    // The boundary slab on a `+axis` face spans across-axis voxel
    // positions [CLUSTER_DIM - fine_stride, CLUSTER_DIM] (one fine-cell
    // thick at the seam). On a `-axis` face it spans
    // [-fine_stride, 0] in self-local coords (shared world frame: equal
    // to [CLUSTER_DIM - fine_stride, CLUSTER_DIM] from the lower
    // cluster's view). The fine in-face dimensions are at neighbor stride.
    //
    // Both clusters across the seam read the same 8 corners per fine
    // boundary cell (from the lower cluster for `i = 0` corners and the
    // higher cluster for `i = 1` corners, both at the same fine voxel
    // coordinates), so vertex positions are byte-identical across the
    // seam after translating by `+CLUSTER_DIM` on the seam axis.

    type FaceEntry<'a> = (Option<(&'a Cluster, Lod)>, usize, bool);
    let face_info: [FaceEntry<'_>; 6] = [
        (neighbors.neg_x, 0, false),
        (neighbors.pos_x, 0, true),
        (neighbors.neg_y, 1, false),
        (neighbors.pos_y, 1, true),
        (neighbors.neg_z, 2, false),
        (neighbors.pos_z, 2, true),
    ];

    let mut fine_face_maps: [Option<HashMap<(i32, i32), u32>>; 6] =
        [None, None, None, None, None, None];

    // --- Per-face fine-stride boundary pass. ---
    for (face_idx, &(n, axis, positive)) in face_info.iter().enumerate() {
        let Some((_, n_lod)) = n else { continue };
        if n_lod.stride() >= stride {
            continue;
        }
        let fine_stride = n_lod.stride() as i32;
        let fine_cell_dim = n_lod.cell_dim() as i32;
        let (a1, a2): (usize, usize) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        // World offset on across axis: 0 for `+axis` (self is lower
        // cluster), CLUSTER_DIM for `-axis` (self is higher cluster).
        let mut wo = [0i32; 3];
        if !positive {
            wo[axis] = dim_i32;
        }
        // Across-axis voxel positions for the 2 corners of the boundary
        // cell. The boundary slab is `fine_stride` wide on the across
        // axis, sitting flush against the seam plane. This matches the
        // fine neighbor's boundary cell at `cx = -1` in its main loop,
        // so both sides read the same world-frame corners.
        let across_pos = if positive {
            [dim_i32 - fine_stride, dim_i32]
        } else {
            [-fine_stride, 0]
        };
        let mut map: HashMap<(i32, i32), u32> = HashMap::new();
        for fc2 in 0..fine_cell_dim {
            for fc1 in 0..fine_cell_dim {
                let mut classes = [false; 8];
                let mut any_solid = false;
                let mut all_solid = true;
                let mut corners = [[0.0_f32; 3]; 8];
                let mut materials = [Material::EMPTY; 8];
                for k in 0..=1i32 {
                    for j in 0..=1i32 {
                        for i in 0..=1i32 {
                            let mut v = [0i32; 3];
                            v[axis] = across_pos[i as usize];
                            v[a1] = (fc1 + j) * fine_stride;
                            v[a2] = (fc2 + k) * fine_stride;
                            let vox = read_corner(cluster, neighbors, v[0], v[1], v[2]);
                            let s = is_voxel_solid(vox);
                            let idx = (i + (j << 1) + (k << 2)) as usize;
                            classes[idx] = s;
                            any_solid |= s;
                            all_solid &= s;
                            let [dx, dy, dz] = vox.corner().to_components();
                            let deltas = [dx, dy, dz];
                            corners[idx] = [
                                (v[0] + wo[0]) as f32 + deltas[0],
                                (v[1] + wo[1]) as f32 + deltas[1],
                                (v[2] + wo[2]) as f32 + deltas[2],
                            ];
                            materials[idx] = vox.material();
                        }
                    }
                }
                if any_solid == all_solid {
                    continue;
                }

                let mut centroid_world = [0.0_f32; 3];
                for c in &corners {
                    centroid_world[0] += c[0];
                    centroid_world[1] += c[1];
                    centroid_world[2] += c[2];
                }
                centroid_world[0] /= 8.0;
                centroid_world[1] /= 8.0;
                centroid_world[2] /= 8.0;
                let centroid = [
                    centroid_world[0] - wo[0] as f32,
                    centroid_world[1] - wo[1] as f32,
                    centroid_world[2] - wo[2] as f32,
                ];

                let to_i = |b: bool| -> i32 {
                    if b {
                        1
                    } else {
                        0
                    }
                };
                let dx_sum: i32 = (to_i(classes[0b001]) - to_i(classes[0b000]))
                    + (to_i(classes[0b011]) - to_i(classes[0b010]))
                    + (to_i(classes[0b101]) - to_i(classes[0b100]))
                    + (to_i(classes[0b111]) - to_i(classes[0b110]));
                let dy_sum: i32 = (to_i(classes[0b010]) - to_i(classes[0b000]))
                    + (to_i(classes[0b011]) - to_i(classes[0b001]))
                    + (to_i(classes[0b110]) - to_i(classes[0b100]))
                    + (to_i(classes[0b111]) - to_i(classes[0b101]));
                let dz_sum: i32 = (to_i(classes[0b100]) - to_i(classes[0b000]))
                    + (to_i(classes[0b101]) - to_i(classes[0b001]))
                    + (to_i(classes[0b110]) - to_i(classes[0b010]))
                    + (to_i(classes[0b111]) - to_i(classes[0b011]));
                let nx = -(dx_sum as f32);
                let ny = -(dy_sum as f32);
                let nz = -(dz_sum as f32);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let normal = if len > 0.0 {
                    [nx / len, ny / len, nz / len]
                } else {
                    [0.0, 1.0, 0.0]
                };

                let mut best_d2 = f32::INFINITY;
                let mut best_material = Material::EMPTY;
                for idx in 0..8 {
                    if !classes[idx] {
                        continue;
                    }
                    let c = [
                        corners[idx][0] - wo[0] as f32,
                        corners[idx][1] - wo[1] as f32,
                        corners[idx][2] - wo[2] as f32,
                    ];
                    let dx = c[0] - centroid[0];
                    let dy = c[1] - centroid[1];
                    let dz = c[2] - centroid[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_material = materials[idx];
                    }
                }

                let vid = vertices.len() as u32;
                map.insert((fc1, fc2), vid);
                vertices.push(Vertex {
                    position: centroid,
                    normal,
                    material: best_material.raw(),
                });
                for ax in 0..3 {
                    if centroid[ax] < bounds_min[ax] {
                        bounds_min[ax] = centroid[ax];
                    }
                    if centroid[ax] > bounds_max[ax] {
                        bounds_max[ax] = centroid[ax];
                    }
                }
            }
        }
        fine_face_maps[face_idx] = Some(map);
    }

    // --- Per-face fine cross-seam edge emission (in-plane + fan). ---
    //
    // For each face with a finer neighbor, iterate fine cross-seam edges
    // along the across-axis at fine in-face positions. If the edge
    // sign-changes and a coarse interior vertex exists at the matching
    // coarse cell, emit a fan (2 triangles using the coarse vertex);
    // otherwise emit a standard 4-fine-cell quad (2 triangles).
    for (face_idx, &(n, axis, positive)) in face_info.iter().enumerate() {
        let Some(map) = fine_face_maps[face_idx].as_ref() else {
            continue;
        };
        let Some((_, n_lod)) = n else { continue };
        let fine_stride = n_lod.stride() as i32;
        let fine_cell_dim = n_lod.cell_dim() as i32;
        let (a1, a2): (usize, usize) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        // Cross-seam voxel positions on the across axis:
        // - `+axis` face: self side at CLUSTER_DIM - fine_stride, neighbor at CLUSTER_DIM.
        // - `-axis` face: self side at 0, neighbor at -fine_stride.
        let self_side: i32 = if positive { dim_i32 - fine_stride } else { 0 };
        let neighbor_side: i32 = if positive { dim_i32 } else { -fine_stride };
        // The lower-coord endpoint of the across-axis edge: on `+axis` face
        // the lower endpoint is self (since `dim_i32 - fine_stride < dim_i32`);
        // on `-axis` face the lower endpoint is the neighbor (since
        // `-fine_stride < 0`).
        // "Natural winding" for X- and Z-axis edges puts the surface normal
        // along `+axis` when the lower-coord endpoint is solid. For Y-axis
        // edges the natural winding is `-axis`, so we reverse.
        let natural_normal_positive = axis != 1;
        // Coarse interior cell index on the across axis:
        // - `+axis` face: cell_dim - 1 (last interior column)
        // - `-axis` face: 0 (first interior column)
        let coarse_across: i32 = if positive { cell_dim - 1 } else { 0 };
        for fc2 in 1..fine_cell_dim {
            for fc1 in 1..fine_cell_dim {
                let mut v_self = [0i32; 3];
                v_self[axis] = self_side;
                v_self[a1] = fc1 * fine_stride;
                v_self[a2] = fc2 * fine_stride;
                let mut v_nbr = [0i32; 3];
                v_nbr[axis] = neighbor_side;
                v_nbr[a1] = fc1 * fine_stride;
                v_nbr[a2] = fc2 * fine_stride;
                let self_solid = is_voxel_solid(read_corner(
                    cluster, neighbors, v_self[0], v_self[1], v_self[2],
                ));
                let nbr_solid = is_voxel_solid(read_corner(
                    cluster, neighbors, v_nbr[0], v_nbr[1], v_nbr[2],
                ));
                if self_solid == nbr_solid {
                    continue;
                }
                // Whether the lower-coord endpoint is solid (= same convention
                // as `a_solid` in the main pass 2 X-axis emission).
                let lower_solid = if positive { self_solid } else { nbr_solid };
                let want_positive_normal = lower_solid;
                let use_natural_winding = want_positive_normal == natural_normal_positive;

                let va = map.get(&(fc1 - 1, fc2 - 1)).copied().unwrap_or(u32::MAX);
                let vb = map.get(&(fc1, fc2 - 1)).copied().unwrap_or(u32::MAX);
                let vc = map.get(&(fc1, fc2)).copied().unwrap_or(u32::MAX);
                let vd = map.get(&(fc1 - 1, fc2)).copied().unwrap_or(u32::MAX);
                if va == u32::MAX || vb == u32::MAX || vc == u32::MAX || vd == u32::MAX {
                    continue;
                }

                // Coarse interior cell coordinate. On a `+axis` face the
                // edge's in-face voxel position `fc * fine_stride` lies in
                // the coarse cell `fc * fine_stride / self_stride` (integer
                // division). On a `-axis` face the convention matches.
                let coarse_a1: i32 = (fc1 * fine_stride) / stride_i32;
                let coarse_a2: i32 = (fc2 * fine_stride) / stride_i32;
                let mut coarse_cell = [0i32; 3];
                coarse_cell[axis] = coarse_across;
                coarse_cell[a1] = coarse_a1;
                coarse_cell[a2] = coarse_a2;
                // Coarse cell must be in the main loop's iterated range.
                let coarse_in_range = coarse_cell[0] >= cx_lo
                    && coarse_cell[0] < cx_hi
                    && coarse_cell[1] >= cy_lo
                    && coarse_cell[1] < cy_hi
                    && coarse_cell[2] >= cz_lo
                    && coarse_cell[2] < cz_hi;
                let coarse_v = if coarse_in_range {
                    cell_vertex[cell_idx(coarse_cell[0], coarse_cell[1], coarse_cell[2])]
                } else {
                    u32::MAX
                };

                if coarse_v != u32::MAX {
                    // Fan: replace `va` in the standard quad split with the
                    // coarse vertex. T1 = (K, vb, vc), T2 = (K, vc, vd).
                    if use_natural_winding {
                        indices_u32.extend_from_slice(&[coarse_v, vb, vc, coarse_v, vc, vd]);
                    } else {
                        indices_u32.extend_from_slice(&[coarse_v, vc, vb, coarse_v, vd, vc]);
                    }
                } else {
                    // No coarse interior vertex: emit the standard
                    // 4-fine-cell quad.
                    if use_natural_winding {
                        indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                    } else {
                        indices_u32.extend_from_slice(&[va, vc, vb, va, vd, vc]);
                    }
                }
            }
        }
    }

    if vertices.is_empty() {
        return empty_mesh(lod);
    }

    // Vertex count == 65536 (max u16 index = 65535) still fits in u16.
    // 65537+ requires u32.
    let use_u32 = vertices.len() > (u16::MAX as usize + 1);
    let indices = if use_u32 {
        Indices::U32(indices_u32)
    } else {
        Indices::U16(indices_u32.into_iter().map(|i| i as u16).collect())
    };

    let vertex_count = vertices.len();
    let triangle_count = indices.triangle_count();
    CellMesh {
        metadata: MeshMetadata {
            lod: lod.level() as u32,
            cluster_dim: CLUSTER_DIM,
            vertex_count,
            triangle_count,
            bounds_min,
            bounds_max,
        },
        vertices,
        indices,
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
fn read_corner(
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
    use std::collections::HashMap;

    // ---- helpers ----

    fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("in-range")
    }

    fn solid_voxel() -> Voxel {
        Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap())
    }

    fn solid_voxel_with(m: Material) -> Voxel {
        Voxel::new(CornerVector::DEFAULT, m)
    }

    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn normalize(v: [f32; 3]) -> [f32; 3] {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len > 0.0 {
            [v[0] / len, v[1] / len, v[2] / len]
        } else {
            [0.0, 0.0, 0.0]
        }
    }

    /// Collect undirected (min, max) edges across all triangles and return
    /// a histogram of edge → triangle count.
    fn edge_use_counts(indices: &Indices) -> HashMap<(u32, u32), u32> {
        let to_u32 = |i: usize| -> u32 {
            match indices {
                Indices::U16(v) => v[i] as u32,
                Indices::U32(v) => v[i],
            }
        };
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        let n = indices.len();
        let mut i = 0;
        while i < n {
            let a = to_u32(i);
            let b = to_u32(i + 1);
            let c = to_u32(i + 2);
            for (x, y) in [(a, b), (b, c), (c, a)] {
                let k = if x < y { (x, y) } else { (y, x) };
                *counts.entry(k).or_insert(0) += 1;
            }
            i += 3;
        }
        counts
    }

    // ---- shared cluster fixtures ----

    /// Phase 2/3 fixture: `y < 128` solid, `y ≥ 128` empty.
    fn build_plane_cluster() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 0..256u32 {
            for y in 0..128u32 {
                for x in 0..256u32 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    /// Phase 2/3 fixture: solid sphere of radius 64 centered at (128, 128, 128).
    /// Uses strict `<` for the radius check — must match the Phase 2 inline
    /// fixture's exact membership rule.
    fn build_sphere_cluster() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        let cx = 128i32;
        let r2 = 64i32 * 64i32;
        for z in 0..256i32 {
            let dz = z - cx;
            for y in 0..256i32 {
                let dy = y - cx;
                for x in 0..256i32 {
                    let dx = x - cx;
                    if dx * dx + dy * dy + dz * dz < r2 {
                        c.set(coord(x as u32, y as u32, z as u32), v);
                    }
                }
            }
        }
        c
    }

    /// Phase 2/3 fixture: 64³ solid cube of voxels at positions `96..160`
    /// on each axis.
    fn build_cube_cluster() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 96u32..160 {
            for y in 96u32..160 {
                for x in 96u32..160 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    // ---- shortcut tests ----

    #[test]
    fn empty_cluster_returns_empty_mesh() {
        let c = Cluster::empty();
        let mesh = contour_cluster(&c);
        assert!(mesh.is_empty());
        assert_eq!(mesh.metadata().vertex_count, 0);
        assert_eq!(mesh.metadata().triangle_count, 0);
        assert_eq!(mesh.metadata().lod, 0);
        assert_eq!(mesh.metadata().cluster_dim, 256);
    }

    #[test]
    fn fully_solid_uniform_returns_empty_mesh() {
        let base = Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap());
        let c = Cluster::uniform(base);
        let mesh = contour_cluster(&c);
        assert!(mesh.is_empty());
    }

    #[test]
    fn uniform_classification_with_material_variation_returns_empty() {
        // Base solid, override one cell to a different solid material. Still
        // uniform classification → no surface.
        let base = Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap());
        let other = Voxel::new(CornerVector::DEFAULT, Material::new(2, 0, 0).unwrap());
        let mut c = Cluster::uniform(base);
        c.set(coord(100, 100, 100), other);
        let mesh = contour_cluster(&c);
        assert!(mesh.is_empty());
    }

    #[test]
    fn uniform_shortcut_is_fast() {
        // Sanity check: the shortcut path should not scan the 16M cells.
        // Generous bound to survive debug builds and CI.
        let c = Cluster::empty();
        let start = std::time::Instant::now();
        let mesh = contour_cluster(&c);
        let elapsed = start.elapsed();
        assert!(mesh.is_empty());
        assert!(
            elapsed.as_millis() < 500,
            "uniform shortcut took {elapsed:?}, expected < 500ms"
        );
    }

    // ---- single solid voxel ----

    #[test]
    fn single_solid_voxel_produces_closed_cube_mesh() {
        let mut c = Cluster::empty();
        c.set(coord(128, 128, 128), solid_voxel());
        let mesh = contour_cluster(&c);

        // 8 surrounding cells × 1 vertex each = 8 vertices.
        // 6 sign-changing axis edges × 1 quad (2 triangles) each = 12 triangles.
        assert_eq!(
            mesh.metadata().vertex_count,
            8,
            "expected 8 vertices for single voxel"
        );
        assert_eq!(
            mesh.metadata().triangle_count,
            12,
            "expected 12 triangles (6 faces × 2)"
        );

        // Every edge of a closed mesh appears in exactly 2 triangles.
        let counts = edge_use_counts(mesh.indices());
        for (edge, n) in &counts {
            assert_eq!(*n, 2, "edge {edge:?} used {n} times; expected 2");
        }

        // All vertex positions should sit near (128, 128, 128).
        for v in mesh.vertices() {
            for axis in 0..3 {
                assert!(
                    (v.position[axis] - 128.0).abs() < 1.5,
                    "vertex {:?} too far from solid voxel center",
                    v.position
                );
            }
        }
    }

    // ---- axis-aligned plane ----

    #[test]
    fn horizontal_plane_produces_flat_upward_facing_mesh() {
        let c = build_plane_cluster();
        let mesh = contour_cluster(&c);

        // Surface at y ≈ 127.5 across the full XZ extent: 255×255 cells.
        let n = mesh.metadata().vertex_count;
        assert!(
            (63_000..=66_000).contains(&n),
            "plane vertex count out of expected band: {n}"
        );

        // All vertex Y positions near 128.0. The cell centroid for cy=127 with
        // default corner vectors lands at y = cy + 1.0 (mean of voxel centers at
        // y=cy and y=cy+1, each offset by ~0.5 from the voxel-min-corner) ≈ 128.0.
        for v in mesh.vertices() {
            assert!(
                (v.position[1] - 128.0).abs() < 0.1,
                "plane vertex Y {} not near 128.0",
                v.position[1]
            );
        }

        // Normals predominantly +Y.
        let mut mean_n = [0.0_f32; 3];
        for v in mesh.vertices() {
            mean_n[0] += v.normal[0];
            mean_n[1] += v.normal[1];
            mean_n[2] += v.normal[2];
        }
        let inv = 1.0 / n as f32;
        let mean = [mean_n[0] * inv, mean_n[1] * inv, mean_n[2] * inv];
        assert!(mean[1] > 0.99, "mean normal Y = {} not near 1", mean[1]);
        assert!(mean[0].abs() < 0.05);
        assert!(mean[2].abs() < 0.05);

        // Triangle count: 254² quads × 2 ≈ 129k.
        let tri = mesh.metadata().triangle_count;
        assert!(
            (120_000..=140_000).contains(&tri),
            "plane triangle count out of band: {tri}"
        );
    }

    // ---- sphere ----

    #[test]
    fn sphere_sdf_produces_approximately_spherical_mesh() {
        let c = build_sphere_cluster();
        let mesh = contour_cluster(&c);

        // Analytic surface area is 4π·64² ≈ 51,500; the discrete boundary band
        // (cells with at least one corner inside and at least one outside) is
        // wider than the continuous surface, typically ~1.5× the analytic
        // count. Generous band to absorb that.
        let n = mesh.metadata().vertex_count;
        assert!(
            (40_000..=100_000).contains(&n),
            "sphere vertex count out of band: {n}"
        );

        // Every vertex within a small distance from radius 64.
        let center = [128.0_f32, 128.0, 128.0];
        let mut min_r = f32::INFINITY;
        let mut max_r = 0.0_f32;
        let mut outward_dot_sum = 0.0_f32;
        for v in mesh.vertices() {
            let d = [
                v.position[0] - center[0],
                v.position[1] - center[1],
                v.position[2] - center[2],
            ];
            let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            min_r = min_r.min(r);
            max_r = max_r.max(r);
            // Normals should point outward (radial).
            let radial = normalize(d);
            outward_dot_sum += dot(v.normal, radial);
        }
        assert!(
            min_r > 60.0 && max_r < 68.0,
            "sphere vertex radii out of band: [{min_r}, {max_r}]"
        );
        let mean_outward = outward_dot_sum / n as f32;
        assert!(
            mean_outward > 0.85,
            "mean outward normal alignment {mean_outward} too low"
        );
    }

    // ---- cube ----

    #[test]
    fn cube_produces_six_axis_aligned_faces() {
        let c = build_cube_cluster();
        let mesh = contour_cluster(&c);
        assert!(mesh.metadata().vertex_count > 0);

        // Closed mesh: every edge appears in exactly 2 triangles.
        let counts = edge_use_counts(mesh.indices());
        let bad: Vec<_> = counts.iter().filter(|(_, n)| **n != 2).collect();
        assert!(
            bad.is_empty(),
            "cube mesh not closed; {} non-pair edges",
            bad.len()
        );

        // Group vertices by dominant normal axis (signed).
        // Expect 6 clusters: ±X, ±Y, ±Z.
        let mut groups: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
        for (i, v) in mesh.vertices().iter().enumerate() {
            let abs = [v.normal[0].abs(), v.normal[1].abs(), v.normal[2].abs()];
            let dom_axis = if abs[0] >= abs[1] && abs[0] >= abs[2] {
                0
            } else if abs[1] >= abs[2] {
                1
            } else {
                2
            };
            // Filter vertices whose normal is very diagonal (edges/corners of the cube).
            if abs[dom_axis] < 0.85 {
                continue;
            }
            let mut sig = [0i32; 3];
            sig[dom_axis] = if v.normal[dom_axis] > 0.0 { 1 } else { -1 };
            groups.entry((sig[0], sig[1], sig[2])).or_default().push(i);
        }

        // All 6 signed axes represented.
        for sig in [
            (1i32, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            assert!(
                groups.get(&sig).map_or(0, |v| v.len()) > 1000,
                "face {sig:?} underpopulated"
            );
        }

        // The +Y face vertices should cluster around y ≈ 160.0 (cell at cy=159
        // with default corner vectors has centroid Y ≈ 160). Accumulate in f64
        // — summing thousands of f32s around 160 loses precision in the
        // mantissa.
        let plus_y = &groups[&(0, 1, 0)];
        let mean_y: f64 = plus_y
            .iter()
            .map(|&i| mesh.vertices()[i].position[1] as f64)
            .sum::<f64>()
            / plus_y.len() as f64;
        assert!(
            (mean_y - 160.0).abs() < 0.1,
            "+Y face mean Y = {mean_y}, expected ~160.0"
        );

        // The -X face vertices should cluster around x ≈ 96.0.
        let minus_x = &groups[&(-1, 0, 0)];
        let mean_x: f64 = minus_x
            .iter()
            .map(|&i| mesh.vertices()[i].position[0] as f64)
            .sum::<f64>()
            / minus_x.len() as f64;
        assert!(
            (mean_x - 96.0).abs() < 0.1,
            "-X face mean X = {mean_x}, expected ~96.0"
        );
    }

    // ---- corner-vector influence ----

    #[test]
    fn corner_vector_changes_vertex_position() {
        // Two clusters with a single solid voxel; in the second, the voxel's
        // owned corner is shifted +X by 1 unit (from default ~0.5 to byte 255 = 1.5).
        let solid_default = solid_voxel();
        let solid_shifted = Voxel::new(
            CornerVector::from_components(1.5, 0.5, 0.5),
            Material::new(1, 0, 0).unwrap(),
        );

        let mut c1 = Cluster::empty();
        c1.set(coord(100, 100, 100), solid_default);
        let mesh1 = contour_cluster(&c1);

        let mut c2 = Cluster::empty();
        c2.set(coord(100, 100, 100), solid_shifted);
        let mesh2 = contour_cluster(&c2);

        assert_eq!(mesh1.metadata().vertex_count, mesh2.metadata().vertex_count);
        assert!(mesh1.metadata().vertex_count > 0);

        // The shift in the owned corner contributes to the centroid of any
        // cell containing this voxel as a corner. Since 1 of 8 corners shifts
        // by Δ in +X, each affected centroid shifts by Δ/8.
        //
        // Decoded shift: byte 255 - byte 128 = 1.5 - (128/255*2 - 0.5).
        let default_decoded = (128.0_f32 / 255.0) * 2.0 - 0.5;
        let delta = 1.5 - default_decoded;
        let expected_centroid_shift = delta / 8.0;

        // Compare bounds: max_x in mesh2 should be larger by ~ shift.
        let max1 = mesh1.metadata().bounds_max;
        let max2 = mesh2.metadata().bounds_max;
        let diff = max2[0] - max1[0];
        assert!(
            (diff - expected_centroid_shift).abs() < 0.01,
            "expected max_x to shift by {expected_centroid_shift}, got {diff}"
        );

        // Y and Z bounds should be unchanged.
        assert!((max2[1] - max1[1]).abs() < 0.001);
        assert!((max2[2] - max1[2]).abs() < 0.001);
    }

    // ---- validity invariants ----

    #[test]
    fn all_indices_in_bounds() {
        // Cube test — exercises lots of indices.
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 100u32..110 {
            for y in 100u32..110 {
                for x in 100u32..110 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        let mesh = contour_cluster(&c);
        let n = mesh.metadata().vertex_count as u32;
        match mesh.indices() {
            Indices::U16(v) => {
                for &i in v {
                    assert!((i as u32) < n);
                }
            }
            Indices::U32(v) => {
                for &i in v {
                    assert!(i < n);
                }
            }
        }
    }

    #[test]
    fn no_degenerate_triangles_on_cube() {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 100u32..110 {
            for y in 100u32..110 {
                for x in 100u32..110 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        let mesh = contour_cluster(&c);
        let to_u32 = |i: usize| -> u32 {
            match mesh.indices() {
                Indices::U16(v) => v[i] as u32,
                Indices::U32(v) => v[i],
            }
        };
        let n = mesh.indices().len();
        let mut i = 0;
        while i < n {
            let a = mesh.vertices()[to_u32(i) as usize].position;
            let b = mesh.vertices()[to_u32(i + 1) as usize].position;
            let c = mesh.vertices()[to_u32(i + 2) as usize].position;
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            // Cross product magnitude squared.
            let cx = ab[1] * ac[2] - ab[2] * ac[1];
            let cy = ab[2] * ac[0] - ab[0] * ac[2];
            let cz = ab[0] * ac[1] - ab[1] * ac[0];
            let area2 = cx * cx + cy * cy + cz * cz;
            assert!(area2 > 1e-12, "degenerate triangle at index {i}");
            i += 3;
        }
    }

    // ---- determinism ----

    #[test]
    fn contouring_is_deterministic() {
        // Build a non-trivial cluster (mini-sphere) and contour twice. The
        // outputs must be byte-equal modulo IEEE-754 stability.
        let mut c = Cluster::empty();
        let v = solid_voxel_with(Material::new(3, 5, 100).unwrap());
        let cx = 50i32;
        let r2 = 12i32 * 12i32;
        for z in 38..62i32 {
            let dz = z - cx;
            for y in 38..62i32 {
                let dy = y - cx;
                for x in 38..62i32 {
                    let dx = x - cx;
                    if dx * dx + dy * dy + dz * dz < r2 {
                        c.set(coord(x as u32, y as u32, z as u32), v);
                    }
                }
            }
        }

        let m1 = contour_cluster(&c);
        let m2 = contour_cluster(&c);

        assert_eq!(m1.metadata(), m2.metadata());
        assert_eq!(m1.vertices(), m2.vertices());
        assert_eq!(m1.indices(), m2.indices());
    }

    // ---- material flows through ----

    #[test]
    fn vertex_material_comes_from_solid_corner() {
        let mut c = Cluster::empty();
        let mat = Material::new(42, 17, 200).unwrap();
        c.set(coord(80, 80, 80), solid_voxel_with(mat));
        let mesh = contour_cluster(&c);
        // Every vertex has the solid voxel as its only solid corner, so its
        // material must equal `mat`.
        for v in mesh.vertices() {
            assert_eq!(v.material, mat.raw());
        }
    }

    // ---- bounds ----

    #[test]
    fn bounds_track_vertex_extent() {
        let mut c = Cluster::empty();
        c.set(coord(50, 50, 50), solid_voxel());
        let mesh = contour_cluster(&c);
        let min = mesh.metadata().bounds_min;
        let max = mesh.metadata().bounds_max;
        for v in mesh.vertices() {
            for a in 0..3 {
                assert!(v.position[a] >= min[a] - 1e-6);
                assert!(v.position[a] <= max[a] + 1e-6);
            }
        }
    }

    // ====================================================================
    // ============================ Phase 3 ===============================
    // ====================================================================

    // ---- Lod type validation ----

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

    // ---- LOD 0 equivalence ----

    /// `contour_cluster(c)` and `contour_cluster_lod(c, Lod::ZERO)` are the
    /// same call by construction (the former is a one-line wrapper around
    /// the latter), but we exercise it to lock in the regression guarantee.
    #[test]
    fn lod_0_equivalence_single_voxel() {
        let mut c = Cluster::empty();
        c.set(coord(128, 128, 128), solid_voxel());
        let a = contour_cluster(&c);
        let b = contour_cluster_lod(&c, Lod::ZERO);
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
        assert_eq!(a.metadata(), b.metadata());
    }

    #[test]
    fn lod_0_equivalence_small_cube() {
        // A 10³ solid cube — same algorithm but cheap to contour at LOD 0.
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 100u32..110 {
            for y in 100u32..110 {
                for x in 100u32..110 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        let a = contour_cluster(&c);
        let b = contour_cluster_lod(&c, Lod::ZERO);
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
        assert_eq!(a.metadata(), b.metadata());
    }

    // ---- Uniform clusters at every LOD ----

    #[test]
    fn uniform_empty_cluster_is_empty_at_every_lod() {
        let c = Cluster::empty();
        for level in 0..=7u8 {
            let lod = Lod::new(level).unwrap();
            let mesh = contour_cluster_lod(&c, lod);
            assert!(
                mesh.is_empty(),
                "LOD {level} of empty cluster should be empty"
            );
            assert_eq!(mesh.metadata().lod, level as u32);
        }
    }

    #[test]
    fn uniform_solid_cluster_is_empty_at_every_lod() {
        let base = Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap());
        let c = Cluster::uniform(base);
        for level in 0..=7u8 {
            let lod = Lod::new(level).unwrap();
            let mesh = contour_cluster_lod(&c, lod);
            assert!(
                mesh.is_empty(),
                "LOD {level} of solid uniform cluster should be empty"
            );
        }
    }

    // ---- Vertex count scaling on the plane ----

    #[test]
    fn plane_vertex_count_scales_with_lod() {
        let c = build_plane_cluster();
        let counts: Vec<usize> = (0..=3u8)
            .map(|l| {
                contour_cluster_lod(&c, Lod::new(l).unwrap())
                    .metadata()
                    .vertex_count
            })
            .collect();

        let n0 = counts[0] as f64;
        for (i, &n) in counts.iter().enumerate().skip(1) {
            // Surface area shrinks as (sample_dim / 256)² since the plane
            // surface is 2-D.
            let scale = ((256u32 >> i) as f64 / 256.0).powi(2);
            let expected = n0 * scale;
            let n_f = n as f64;
            let rel = (n_f - expected).abs() / expected;
            assert!(
                rel < 0.30,
                "LOD {i}: vertex count {n} not within 30% of expected {expected:.0} \
                 (n0 = {n0}, scale = {scale})"
            );
        }
    }

    #[test]
    fn plane_at_lod_7_does_not_panic() {
        // LOD 7 reduces the plane to a 2³ sample grid. All 8 samples likely
        // land on the solid side (y < 128). Should produce 0 vertices, not
        // panic.
        let c = build_plane_cluster();
        let mesh = contour_cluster_lod(&c, Lod::MAX);
        // We don't assert on the vertex count exactly — different rounding
        // would change it. Just that the call completed.
        assert_eq!(mesh.metadata().lod, 7);
    }

    // ---- Vertex count scaling on a sphere ----

    #[test]
    fn sphere_vertex_count_scales_with_lod() {
        let c = build_sphere_cluster();
        let n0 = contour_cluster_lod(&c, Lod::ZERO).metadata().vertex_count as f64;
        let n1 = contour_cluster_lod(&c, Lod::new(1).unwrap())
            .metadata()
            .vertex_count as f64;
        let n2 = contour_cluster_lod(&c, Lod::new(2).unwrap())
            .metadata()
            .vertex_count as f64;

        let exp_1 = n0 * 0.25;
        let exp_2 = n0 * 0.0625;

        assert!(
            (n1 - exp_1).abs() / exp_1 < 0.30,
            "LOD 1 sphere vertex count {n1} not within 30% of expected {exp_1:.0}"
        );
        assert!(
            (n2 - exp_2).abs() / exp_2 < 0.40,
            "LOD 2 sphere vertex count {n2} not within 40% of expected {exp_2:.0}"
        );
    }

    #[test]
    fn sphere_lod_2_radii_in_band() {
        let c = build_sphere_cluster();
        let mesh = contour_cluster_lod(&c, Lod::new(2).unwrap());
        assert!(mesh.metadata().vertex_count > 0);
        let center = [128.0_f32; 3];
        let mut min_r = f32::INFINITY;
        let mut max_r = 0.0_f32;
        for v in mesh.vertices() {
            let d = [
                v.position[0] - center[0],
                v.position[1] - center[1],
                v.position[2] - center[2],
            ];
            let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            min_r = min_r.min(r);
            max_r = max_r.max(r);
        }
        // Vertex positions are in cluster-local coords at every LOD, so the
        // sphere radius band stays in [~60, ~68] independent of LOD —
        // higher LODs just produce fewer vertices in the same band.
        assert!(
            min_r > 58.0 && max_r < 70.0,
            "LOD 2 sphere radii out of band: [{min_r}, {max_r}]"
        );
    }

    // ---- Nested-subset spatial property ----

    #[test]
    fn lod_1_vertices_live_near_lod_0_surface() {
        let c = build_sphere_cluster();
        let m0 = contour_cluster_lod(&c, Lod::ZERO);
        let m1 = contour_cluster_lod(&c, Lod::new(1).unwrap());

        // Sample LOD 1 vertices (every 20th, capped at 1000) and check that
        // each has a LOD 0 vertex within 3 units. 90%+ must qualify.
        let lod1_sample: Vec<&Vertex> = m1.vertices().iter().step_by(20).take(1000).collect();
        let lod0 = m0.vertices();
        let mut within_3 = 0;
        for v1 in &lod1_sample {
            let p1 = v1.position;
            let mut min_d2 = f32::INFINITY;
            for v0 in lod0 {
                let p0 = v0.position;
                let d2 =
                    (p0[0] - p1[0]).powi(2) + (p0[1] - p1[1]).powi(2) + (p0[2] - p1[2]).powi(2);
                if d2 < min_d2 {
                    min_d2 = d2;
                }
            }
            if min_d2 <= 9.0 {
                within_3 += 1;
            }
        }
        let ratio = within_3 as f64 / lod1_sample.len() as f64;
        assert!(
            ratio >= 0.9,
            "only {within_3}/{} LOD 1 vertices have a LOD 0 vertex within 3 units",
            lod1_sample.len()
        );
    }

    // ---- Determinism at multiple LODs ----

    #[test]
    fn determinism_at_lods_0_1_2_4() {
        // Small mini-sphere fixture to keep this test quick across 4 LODs.
        let mut c = Cluster::empty();
        let v = solid_voxel_with(Material::new(3, 5, 100).unwrap());
        let cx = 50i32;
        let r2 = 12i32 * 12i32;
        for z in 38..62i32 {
            let dz = z - cx;
            for y in 38..62i32 {
                let dy = y - cx;
                for x in 38..62i32 {
                    let dx = x - cx;
                    if dx * dx + dy * dy + dz * dz < r2 {
                        c.set(coord(x as u32, y as u32, z as u32), v);
                    }
                }
            }
        }
        for level in [0u8, 1, 2, 4] {
            let lod = Lod::new(level).unwrap();
            let a = contour_cluster_lod(&c, lod);
            let b = contour_cluster_lod(&c, lod);
            assert_eq!(
                a.vertices(),
                b.vertices(),
                "LOD {level} vertices not deterministic"
            );
            assert_eq!(
                a.indices(),
                b.indices(),
                "LOD {level} indices not deterministic"
            );
            assert_eq!(
                a.metadata(),
                b.metadata(),
                "LOD {level} metadata not deterministic"
            );
        }
    }

    // ---- Metadata.lod field ----

    #[test]
    fn metadata_lod_field_equals_requested_level() {
        // Use a small fixture that produces a non-empty mesh at every LOD,
        // so we exercise the full path (not just the early-out shortcuts).
        let mut c = Cluster::empty();
        // A 2x2x2 block of solid voxels at positions divisible by 128 so
        // they're visible at every LOD up to 7.
        c.set(coord(0, 0, 0), solid_voxel());
        c.set(coord(128, 0, 0), solid_voxel());
        c.set(coord(0, 128, 0), solid_voxel());
        c.set(coord(0, 0, 128), solid_voxel());
        for level in 0..=7u8 {
            let lod = Lod::new(level).unwrap();
            let mesh = contour_cluster_lod(&c, lod);
            assert_eq!(
                mesh.metadata().lod,
                level as u32,
                "LOD {level}: metadata.lod field mismatch"
            );
        }
    }

    // ---- Cube at LOD 2 ----

    #[test]
    fn cube_at_lod_2_six_face_directions() {
        let c = build_cube_cluster();
        let mesh = contour_cluster_lod(&c, Lod::new(2).unwrap());
        assert_eq!(mesh.metadata().lod, 2);
        assert!(mesh.metadata().vertex_count > 0);

        // Closed mesh.
        let counts = edge_use_counts(mesh.indices());
        let bad: Vec<_> = counts.iter().filter(|(_, n)| **n != 2).collect();
        assert!(
            bad.is_empty(),
            "LOD 2 cube mesh not closed; {} non-pair edges",
            bad.len()
        );

        // Group vertices by dominant axis direction; all 6 axis-signed
        // groups should be populated.
        let mut groups: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
        for (i, v) in mesh.vertices().iter().enumerate() {
            let abs = [v.normal[0].abs(), v.normal[1].abs(), v.normal[2].abs()];
            let dom = if abs[0] >= abs[1] && abs[0] >= abs[2] {
                0
            } else if abs[1] >= abs[2] {
                1
            } else {
                2
            };
            if abs[dom] < 0.85 {
                continue;
            }
            let mut sig = [0i32; 3];
            sig[dom] = if v.normal[dom] > 0.0 { 1 } else { -1 };
            groups.entry((sig[0], sig[1], sig[2])).or_default().push(i);
        }
        for sig in [
            (1i32, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            assert!(
                groups.get(&sig).map_or(0, |v| v.len()) > 30,
                "LOD 2 face {sig:?} underpopulated"
            );
        }

        // +Y face mean Y. At LOD 2 (stride 4), the +Y face cell straddles
        // cluster y=156 (in cube, solid) and y=160 (out, empty). Centroid
        // Y = (156 + 0.5039 + 160 + 0.5039) / 2 ≈ 158.5039 — NOT 160 as at
        // LOD 0, because the LOD 2 cell spans 4 cluster units in Y. Both
        // expressions are in cluster-local coords, just over different
        // cell spans.
        let plus_y = &groups[&(0, 1, 0)];
        let mean_y: f64 = plus_y
            .iter()
            .map(|&i| mesh.vertices()[i].position[1] as f64)
            .sum::<f64>()
            / plus_y.len() as f64;
        assert!(
            (mean_y - 158.5).abs() < 0.5,
            "LOD 2 +Y face mean Y = {mean_y}, expected ~158.5"
        );
    }

    // ---- LOD 7 single voxel ----

    #[test]
    fn lod_7_single_voxel_one_vertex_zero_triangles() {
        // Voxel at (0, 0, 0) is solid; everything else empty (base). At LOD
        // 7 (stride 128) virtual (0,0,0) reads cluster (0,0,0) = solid;
        // virtual (1,0,0) reads cluster (128,0,0) = empty. The single cell
        // at virtual (0,0,0) is mixed.
        let mut c = Cluster::empty();
        c.set(coord(0, 0, 0), solid_voxel());
        let mesh = contour_cluster_lod(&c, Lod::MAX);
        assert_eq!(mesh.metadata().vertex_count, 1);
        // cell_dim = 1 so the face-emission loops are empty (1..1 has no
        // iterations); no quads can be formed.
        assert_eq!(mesh.metadata().triangle_count, 0);
        assert_eq!(mesh.metadata().lod, 7);
    }

    #[test]
    fn lod_7_unaligned_override_invisible() {
        // A solid voxel at a non-stride-aligned position is invisible at
        // LOD 7 — no surface, empty mesh.
        let mut c = Cluster::empty();
        c.set(coord(1, 1, 1), solid_voxel());
        let mesh = contour_cluster_lod(&c, Lod::MAX);
        assert!(
            mesh.is_empty(),
            "unaligned override should be invisible at LOD 7"
        );
    }

    // ====================================================================
    // ============================ Phase 4 ===============================
    // ====================================================================

    // ---- Test 1: no neighbors == Phase 3 byte-identical ----

    #[test]
    fn no_neighbors_equivalent_to_phase_3_sphere() {
        let c = build_sphere_cluster();
        let lod = Lod::ZERO;
        let m_p3 = contour_cluster_lod(&c, lod);
        let m_p4 = contour_cluster_lod_with_neighbors(&c, lod, &NeighborContext::none());
        assert_eq!(m_p3.vertices(), m_p4.vertices());
        assert_eq!(m_p3.indices(), m_p4.indices());
        assert_eq!(m_p3.metadata(), m_p4.metadata());
    }

    #[test]
    fn no_neighbors_equivalent_to_phase_3_cube() {
        let c = build_cube_cluster();
        for level in 0..=3u8 {
            let lod = Lod::new(level).unwrap();
            let m_p3 = contour_cluster_lod(&c, lod);
            let m_p4 = contour_cluster_lod_with_neighbors(&c, lod, &NeighborContext::none());
            assert_eq!(m_p3.vertices(), m_p4.vertices(), "LOD {level}");
            assert_eq!(m_p3.indices(), m_p4.indices(), "LOD {level}");
            assert_eq!(m_p3.metadata(), m_p4.metadata(), "LOD {level}");
        }
    }

    // ---- Test 2: equal-LOD neighbors don't change anything ----

    #[test]
    fn equal_lod_neighbor_does_not_change_output() {
        // Two clusters with identical sphere content at the same LOD.
        // With each as the other's +X neighbor at equal LOD, the contour
        // output must be unchanged vs. no-neighbor.
        let a = build_sphere_cluster();
        let b = build_sphere_cluster();
        let lod = Lod::ZERO;

        let a_alone = contour_cluster_lod(&a, lod);
        let a_with_eq_b = contour_cluster_lod_with_neighbors(
            &a,
            lod,
            &NeighborContext {
                pos_x: Some((&b, lod)),
                ..NeighborContext::none()
            },
        );

        assert_eq!(a_alone.vertices(), a_with_eq_b.vertices());
        assert_eq!(a_alone.indices(), a_with_eq_b.indices());
    }

    // ---- Test 8: world-boundary face (None neighbor) doesn't subdivide ----

    #[test]
    fn world_boundary_none_neighbor_no_subdivision() {
        // Same as equal-LOD test but with None — should be identical to
        // Phase 3 with no special handling. Distinguishing the "None means
        // world boundary" semantic from "neighbor at equal LOD" semantic
        // is mainly a future concern; both currently emit identical output.
        let c = build_cube_cluster();
        let m_none = contour_cluster_lod_with_neighbors(&c, Lod::ZERO, &NeighborContext::none());
        let m_p3 = contour_cluster_lod(&c, Lod::ZERO);
        assert_eq!(m_none.vertices(), m_p3.vertices());
        assert_eq!(m_none.indices(), m_p3.indices());
    }

    // ---- Helpers for seam tests ----

    /// Build a cluster where the +X half (x ≥ 128) is solid. Used for
    /// seam tests where the surface lies on x ≈ 128.
    fn build_left_solid_cluster() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 0..256u32 {
            for y in 0..256u32 {
                for x in 0..128u32 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    /// Translate a vertex position by `offset` and return the modified
    /// vertex. Used to combine adjacent-cluster meshes in world space.
    #[allow(dead_code)] // used by #[ignore]-d follow-up tests
    fn translated(v: Vertex, offset: [f32; 3]) -> Vertex {
        Vertex {
            position: [
                v.position[0] + offset[0],
                v.position[1] + offset[1],
                v.position[2] + offset[2],
            ],
            ..v
        }
    }

    // ---- Six-face boundary-row triangle count tests ----
    //
    // Self is fully solid, neighbor on the target face is fully empty. The
    // extended boundary row of cells at the target face produces a 2D
    // surface across the seam plane. At equal LOD 0, every cell in that
    // row is mixed (4 solid corners from self, 4 empty from the neighbor),
    // and every cross-seam axis edge sign-changes. The face emission loop
    // produces `(cell_max - 1)² = 254²` quads (`= 254² · 2 = 129032`
    // triangles) connecting the 4 cells around each cross-seam edge.

    fn solid_uniform_cluster() -> Cluster {
        Cluster::uniform(solid_voxel())
    }

    fn seam_tri_count(a: &Cluster, lod_a: Lod, b: &Cluster, lod_b: Lod, face: &str) -> usize {
        let neighbors = match face {
            "pos_x" => NeighborContext {
                pos_x: Some((b, lod_b)),
                ..NeighborContext::none()
            },
            "neg_x" => NeighborContext {
                neg_x: Some((b, lod_b)),
                ..NeighborContext::none()
            },
            "pos_y" => NeighborContext {
                pos_y: Some((b, lod_b)),
                ..NeighborContext::none()
            },
            "neg_y" => NeighborContext {
                neg_y: Some((b, lod_b)),
                ..NeighborContext::none()
            },
            "pos_z" => NeighborContext {
                pos_z: Some((b, lod_b)),
                ..NeighborContext::none()
            },
            "neg_z" => NeighborContext {
                neg_z: Some((b, lod_b)),
                ..NeighborContext::none()
            },
            _ => panic!("unknown face"),
        };
        contour_cluster_lod_with_neighbors(a, lod_a, &neighbors)
            .metadata()
            .triangle_count
    }

    const EXPECTED_BOUNDARY_TRIS_LOD0: usize = 254 * 254 * 2;

    #[test]
    fn boundary_row_pos_x_triangle_count() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        assert_eq!(
            seam_tri_count(&a, Lod::ZERO, &b, Lod::ZERO, "pos_x"),
            EXPECTED_BOUNDARY_TRIS_LOD0,
        );
    }

    #[test]
    fn boundary_row_neg_x_triangle_count() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        assert_eq!(
            seam_tri_count(&a, Lod::ZERO, &b, Lod::ZERO, "neg_x"),
            EXPECTED_BOUNDARY_TRIS_LOD0,
        );
    }

    #[test]
    fn boundary_row_pos_y_triangle_count() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        assert_eq!(
            seam_tri_count(&a, Lod::ZERO, &b, Lod::ZERO, "pos_y"),
            EXPECTED_BOUNDARY_TRIS_LOD0,
        );
    }

    #[test]
    fn boundary_row_neg_y_triangle_count() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        assert_eq!(
            seam_tri_count(&a, Lod::ZERO, &b, Lod::ZERO, "neg_y"),
            EXPECTED_BOUNDARY_TRIS_LOD0,
        );
    }

    #[test]
    fn boundary_row_pos_z_triangle_count() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        assert_eq!(
            seam_tri_count(&a, Lod::ZERO, &b, Lod::ZERO, "pos_z"),
            EXPECTED_BOUNDARY_TRIS_LOD0,
        );
    }

    #[test]
    fn boundary_row_neg_z_triangle_count() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        assert_eq!(
            seam_tri_count(&a, Lod::ZERO, &b, Lod::ZERO, "neg_z"),
            EXPECTED_BOUNDARY_TRIS_LOD0,
        );
    }

    // ---- Integrated-approach Phase 4 invariants ----

    /// `contour_cluster_lod_with_neighbors(c, lod, &NeighborContext::none())`
    /// returns byte-identical output to `contour_cluster_lod(c, lod)`. The
    /// integrated extended-grid loop must collapse to Phase 3 when no
    /// neighbor is present on any face.
    #[test]
    fn extended_grid_no_neighbors_byte_identical_to_phase_3() {
        let c = build_cube_cluster();
        for level in 0..=3u8 {
            let lod = Lod::new(level).unwrap();
            let m_p3 = contour_cluster_lod(&c, lod);
            let m_p4 = contour_cluster_lod_with_neighbors(&c, lod, &NeighborContext::none());
            assert_eq!(m_p3.vertices(), m_p4.vertices(), "LOD {level} vertices");
            assert_eq!(m_p3.indices(), m_p4.indices(), "LOD {level} indices");
            assert_eq!(m_p3.metadata(), m_p4.metadata(), "LOD {level} metadata");
        }
    }

    /// Two adjacent clusters at the same LOD, each with a closed cube fully
    /// inside their own region (away from the seam). Neither cluster's
    /// boundary row produces a surface (uniform empty across the seam), so
    /// the combined mesh is just the two closed cubes and every edge
    /// appears in exactly 2 triangles (per-cluster index space).
    #[test]
    fn extended_grid_closes_equal_lod_seam_no_cracks() {
        let mut a = Cluster::empty();
        let mut b = Cluster::empty();
        let v = solid_voxel();
        for z in 100u32..110 {
            for y in 100u32..110 {
                for x in 100u32..110 {
                    a.set(coord(x, y, z), v);
                    b.set(coord(x, y, z), v);
                }
            }
        }
        let lod = Lod::ZERO;
        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            lod,
            &NeighborContext {
                pos_x: Some((&b, lod)),
                ..NeighborContext::none()
            },
        );
        let m_b = contour_cluster_lod_with_neighbors(
            &b,
            lod,
            &NeighborContext {
                neg_x: Some((&a, lod)),
                ..NeighborContext::none()
            },
        );

        let mut combined: Vec<u32> = match m_a.indices() {
            Indices::U16(v) => v.iter().map(|&i| i as u32).collect(),
            Indices::U32(v) => v.clone(),
        };
        let a_count = m_a.vertices().len() as u32;
        match m_b.indices() {
            Indices::U16(v) => combined.extend(v.iter().map(|&i| i as u32 + a_count)),
            Indices::U32(v) => combined.extend(v.iter().map(|&i| i + a_count)),
        }
        let combined_indices = Indices::U32(combined);
        let counts = edge_use_counts(&combined_indices);
        let bad: Vec<_> = counts.iter().filter(|(_, n)| **n != 2).collect();
        assert!(
            bad.is_empty(),
            "combined mesh has {} non-pair edges",
            bad.len()
        );
    }

    /// Solid-uniform A with empty B at equal LOD: A's boundary vertices on
    /// `+X` (stored in A's local frame, X ≈ 256) must byte-equal B's
    /// boundary vertices on `-X` (stored in B's local frame, X ≈ 0) after
    /// translating B by `+CLUSTER_DIM` on X. Both sides compute the
    /// centroid in the same shared world frame from the same corner data
    /// in the same iteration order, so the world position is byte-exact.
    #[test]
    fn boundary_vertices_byte_equal_at_equal_lod() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        let lod = Lod::ZERO;
        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            lod,
            &NeighborContext {
                pos_x: Some((&b, lod)),
                ..NeighborContext::none()
            },
        );
        let m_b = contour_cluster_lod_with_neighbors(
            &b,
            lod,
            &NeighborContext {
                neg_x: Some((&a, lod)),
                ..NeighborContext::none()
            },
        );

        let mut a_seam: Vec<[f32; 3]> = m_a
            .vertices()
            .iter()
            .filter(|v| v.position[0] > 255.0)
            .map(|v| v.position)
            .collect();
        let mut b_seam: Vec<[f32; 3]> = m_b
            .vertices()
            .iter()
            .filter(|v| v.position[0] < 1.0)
            .map(|v| {
                [
                    v.position[0] + CLUSTER_DIM as f32,
                    v.position[1],
                    v.position[2],
                ]
            })
            .collect();

        assert!(
            !a_seam.is_empty(),
            "no boundary vertices on A's +X face — fixture broken"
        );
        assert_eq!(
            a_seam.len(),
            b_seam.len(),
            "boundary vertex counts differ: A={}, B={}",
            a_seam.len(),
            b_seam.len()
        );

        let cmp = |p: &[f32; 3], q: &[f32; 3]| {
            p[1].total_cmp(&q[1])
                .then(p[2].total_cmp(&q[2]))
                .then(p[0].total_cmp(&q[0]))
        };
        a_seam.sort_by(cmp);
        b_seam.sort_by(cmp);
        for (av, bv) in a_seam.iter().zip(b_seam.iter()) {
            for axis in 0..3 {
                assert_eq!(
                    av[axis].to_bits(),
                    bv[axis].to_bits(),
                    "axis {axis}: A boundary vertex {av:?} not byte-equal to B (translated) {bv:?}"
                );
            }
        }
    }

    // ---- Test 3: boundary vertex alignment on the seam plane ----

    #[test]
    fn seam_vertices_align_at_coarse_stride() {
        // A is fully solid, B is empty. A's `+X` boundary row produces
        // vertices on the seam plane at A-local X ≈ 256 (centroid of 4
        // corners at X=255 and 4 at X=256, with default-encoded δ ≈
        // 0.5039). The neighbor's coarser LOD doesn't change A's emission
        // — A always extends its grid at self's stride.
        let mut a = Cluster::empty();
        let v = solid_voxel();
        for z in 0..256u32 {
            for y in 0..256u32 {
                for x in 0..256u32 {
                    a.set(coord(x, y, z), v);
                }
            }
        }
        let b = Cluster::empty();

        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            Lod::ZERO,
            &NeighborContext {
                pos_x: Some((&b, Lod::new(2).unwrap())),
                ..NeighborContext::none()
            },
        );

        // Every seam vertex should have X ≈ 256 (centroid of 4 corners at
        // x=255 and 4 at x=256, with default-encoded δ ≈ 0.5039).
        let mut seam_count = 0usize;
        for vtx in m_a.vertices() {
            if (vtx.position[0] - 256.0).abs() < 1.0 {
                seam_count += 1;
                assert!(
                    (vtx.position[0] - 256.0).abs() < 0.51,
                    "seam vertex X = {} should be near 256",
                    vtx.position[0]
                );
            }
        }
        assert!(seam_count > 0, "no seam vertices found");
    }

    // ---- Test 6: determinism of seam vertex positions (relaxed) ----
    //
    // The strict "byte-equal vertex positions across A and B" guarantee
    // requires both clusters to compute identical world positions through
    // identical arithmetic. Phase 4's current implementation translates
    // through the cluster's local frame, which differs by cluster offset.
    // A direct byte-equality check requires composition of f32 addition
    // (centroid local + offset) which can introduce rounding. We assert
    // a tight float tolerance instead and leave strict byte-equality as
    // a follow-up that pulls the world-frame computation into a shared
    // path.

    #[test]
    fn seam_vertices_deterministic_within_run() {
        // Single-cluster determinism with neighbors: two runs of the same
        // (cluster, lod, neighbors) input produce byte-identical output.
        let mut a = Cluster::empty();
        let v = solid_voxel();
        for x in 0..256u32 {
            a.set(coord(x, 100, 100), v);
        }
        let b = Cluster::empty();
        let nbr = NeighborContext {
            pos_x: Some((&b, Lod::new(2).unwrap())),
            ..NeighborContext::none()
        };
        let m1 = contour_cluster_lod_with_neighbors(&a, Lod::ZERO, &nbr);
        let m2 = contour_cluster_lod_with_neighbors(&a, Lod::ZERO, &nbr);
        assert_eq!(m1.vertices(), m2.vertices());
        assert_eq!(m1.indices(), m2.indices());
        assert_eq!(m1.metadata(), m2.metadata());
    }

    /// Reproducibility byte-equality: the integrated extended-grid pass
    /// produces byte-identical boundary vertex positions across runs of
    /// the same contour invocation.
    #[test]
    fn seam_vertices_byte_equal_within_run() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        let neighbors = NeighborContext {
            pos_x: Some((&b, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let m1 = contour_cluster_lod_with_neighbors(&a, Lod::ZERO, &neighbors);
        let m2 = contour_cluster_lod_with_neighbors(&a, Lod::ZERO, &neighbors);
        assert_eq!(m1.vertices(), m2.vertices());
    }

    /// Cross-cluster byte-equality: A's `+X` boundary vertices and B's
    /// `-X` boundary vertices (translated by `+CLUSTER_DIM` on X) match
    /// byte-for-byte. Both sides compute the centroid in the shared world
    /// frame from the same corner data in the same iteration order, so
    /// the resulting world positions are byte-identical.
    #[test]
    fn seam_vertices_byte_equal_under_symmetry() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        let lod = Lod::ZERO;
        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            lod,
            &NeighborContext {
                pos_x: Some((&b, lod)),
                ..NeighborContext::none()
            },
        );
        let m_b = contour_cluster_lod_with_neighbors(
            &b,
            lod,
            &NeighborContext {
                neg_x: Some((&a, lod)),
                ..NeighborContext::none()
            },
        );

        let mut a_seam: Vec<[f32; 3]> = m_a
            .vertices()
            .iter()
            .filter(|v| v.position[0] > 255.0)
            .map(|v| v.position)
            .collect();
        let mut b_seam: Vec<[f32; 3]> = m_b
            .vertices()
            .iter()
            .filter(|v| v.position[0] < 1.0)
            .map(|v| {
                [
                    v.position[0] + CLUSTER_DIM as f32,
                    v.position[1],
                    v.position[2],
                ]
            })
            .collect();

        let cmp = |p: &[f32; 3], q: &[f32; 3]| {
            p[1].total_cmp(&q[1])
                .then(p[2].total_cmp(&q[2]))
                .then(p[0].total_cmp(&q[0]))
        };
        a_seam.sort_by(cmp);
        b_seam.sort_by(cmp);
        assert_eq!(a_seam, b_seam);
    }

    // ---- Mixed-LOD seams ----

    /// A LOD 0, B LOD 2, A.+X = B at LOD 2, B.-X = A at LOD 0.
    /// A is fine (no mixed-LOD on A — main loop extends as in equal-LOD).
    /// B is coarse (B's -X face triggers the fine-stride boundary pass).
    /// Each cluster's mesh is closed (every edge in exactly 2 triangles).
    #[test]
    fn coarse_to_fine_seam_no_cracks_in_combined_mesh() {
        // Each cluster has a small solid cube fully interior to itself
        // (not reaching the seam). This makes each cluster's mesh a
        // closed cube on its own. The fine-stride boundary pass on B's
        // -X face runs but produces no surface (uniform-empty corners
        // since A's voxels at vx = 255 are empty for this fixture).
        let mut a = Cluster::empty();
        let mut b = Cluster::empty();
        let v = solid_voxel();
        for z in 100u32..110 {
            for y in 100u32..110 {
                for x in 100u32..110 {
                    a.set(coord(x, y, z), v);
                    b.set(coord(x, y, z), v);
                }
            }
        }
        let nbr_a = NeighborContext {
            pos_x: Some((&b, Lod::new(2).unwrap())),
            ..NeighborContext::none()
        };
        let nbr_b = NeighborContext {
            neg_x: Some((&a, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let m_a = contour_cluster_lod_with_neighbors(&a, Lod::ZERO, &nbr_a);
        let m_b = contour_cluster_lod_with_neighbors(&b, Lod::new(2).unwrap(), &nbr_b);

        let counts_a = edge_use_counts(m_a.indices());
        let bad_a: Vec<_> = counts_a.iter().filter(|(_, n)| **n != 2).collect();
        assert!(
            bad_a.is_empty(),
            "A's mesh not closed; {} non-pair edges",
            bad_a.len()
        );

        let counts_b = edge_use_counts(m_b.indices());
        let bad_b: Vec<_> = counts_b.iter().filter(|(_, n)| **n != 2).collect();
        assert!(
            bad_b.is_empty(),
            "B's mesh not closed; {} non-pair edges",
            bad_b.len()
        );
    }

    /// Chain three clusters at different LODs: A LOD 0, B LOD 1, C LOD 3.
    /// Each pairwise seam must close — i.e., each cluster's mesh is a
    /// closed 2-manifold.
    #[test]
    fn three_way_lod_chain() {
        // Each cluster has a small interior cube; seams have uniform
        // empty content. Each mesh should be a closed cube.
        let mut a = Cluster::empty();
        let mut b = Cluster::empty();
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 100u32..110 {
            for y in 100u32..110 {
                for x in 100u32..110 {
                    a.set(coord(x, y, z), v);
                    b.set(coord(x, y, z), v);
                    c.set(coord(x, y, z), v);
                }
            }
        }
        let lod_a = Lod::ZERO;
        let lod_b = Lod::new(1).unwrap();
        let lod_c = Lod::new(3).unwrap();
        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            lod_a,
            &NeighborContext {
                pos_x: Some((&b, lod_b)),
                ..NeighborContext::none()
            },
        );
        let m_b = contour_cluster_lod_with_neighbors(
            &b,
            lod_b,
            &NeighborContext {
                neg_x: Some((&a, lod_a)),
                pos_x: Some((&c, lod_c)),
                ..NeighborContext::none()
            },
        );
        let m_c = contour_cluster_lod_with_neighbors(
            &c,
            lod_c,
            &NeighborContext {
                neg_x: Some((&b, lod_b)),
                ..NeighborContext::none()
            },
        );

        for (name, mesh) in [("A", &m_a), ("B", &m_b), ("C", &m_c)] {
            let counts = edge_use_counts(mesh.indices());
            let bad: Vec<_> = counts.iter().filter(|(_, n)| **n != 2).collect();
            assert!(
                bad.is_empty(),
                "{name}'s mesh not closed; {} non-pair edges",
                bad.len()
            );
        }
    }

    /// Mixed-LOD byte-equality: A fully solid at LOD 2 (coarse), B fully
    /// empty at LOD 0 (fine), A.+X = B at LOD 0. A's `+X` boundary
    /// vertices (in A's local frame) and B's `-X` boundary vertices
    /// translated by `+CLUSTER_DIM` on X must be byte-identical. A
    /// refines its boundary row to B's fine stride; both clusters read
    /// the same 8 corners per fine boundary cell in the same iteration
    /// order, producing the same world-frame centroid.
    #[test]
    fn mixed_lod_byte_equality() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        let lod_a = Lod::new(2).unwrap();
        let lod_b = Lod::ZERO;
        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            lod_a,
            &NeighborContext {
                pos_x: Some((&b, lod_b)),
                ..NeighborContext::none()
            },
        );
        let m_b = contour_cluster_lod_with_neighbors(
            &b,
            lod_b,
            &NeighborContext {
                neg_x: Some((&a, lod_a)),
                ..NeighborContext::none()
            },
        );

        let mut a_seam: Vec<[f32; 3]> = m_a
            .vertices()
            .iter()
            .filter(|v| v.position[0] > 254.0)
            .map(|v| v.position)
            .collect();
        let mut b_seam: Vec<[f32; 3]> = m_b
            .vertices()
            .iter()
            .filter(|v| v.position[0] < 2.0)
            .map(|v| {
                [
                    v.position[0] + CLUSTER_DIM as f32,
                    v.position[1],
                    v.position[2],
                ]
            })
            .collect();

        assert!(
            !a_seam.is_empty(),
            "no boundary vertices on A's +X face — fixture broken"
        );
        assert_eq!(
            a_seam.len(),
            b_seam.len(),
            "boundary vertex counts differ: A={}, B={}",
            a_seam.len(),
            b_seam.len()
        );

        let cmp = |p: &[f32; 3], q: &[f32; 3]| {
            p[1].total_cmp(&q[1])
                .then(p[2].total_cmp(&q[2]))
                .then(p[0].total_cmp(&q[0]))
        };
        a_seam.sort_by(cmp);
        b_seam.sort_by(cmp);
        for (av, bv) in a_seam.iter().zip(b_seam.iter()) {
            for axis in 0..3 {
                assert_eq!(
                    av[axis].to_bits(),
                    bv[axis].to_bits(),
                    "axis {axis}: A boundary vertex {av:?} not byte-equal to B (translated) {bv:?}"
                );
            }
        }
    }

    /// Fan triangle count for a known fixture: A fully solid at LOD 2
    /// (coarse), B fully empty at LOD 0 (fine). A's +X face emits a
    /// `(fine_cell_dim - 1)² · 2 = 254² · 2 = 129032` triangle quad
    /// slab from the fine-stride boundary pass. The fan pass produces
    /// zero additional triangles because A's interior is uniform solid
    /// and has no coarse interior vertices on the boundary column.
    #[test]
    fn fan_triangle_count_uniform_fixture() {
        let a = solid_uniform_cluster();
        let b = Cluster::empty();
        let m_a = contour_cluster_lod_with_neighbors(
            &a,
            Lod::new(2).unwrap(),
            &NeighborContext {
                pos_x: Some((&b, Lod::ZERO)),
                ..NeighborContext::none()
            },
        );
        let expected_tris = 254 * 254 * 2;
        assert_eq!(
            m_a.metadata().triangle_count,
            expected_tris,
            "expected {expected_tris} triangles on A's +X boundary, got {}",
            m_a.metadata().triangle_count
        );
    }

    /// The fine side's output is independent of the neighbor's LOD. A at
    /// LOD 0 contoured with B at LOD 2 as +X neighbor must produce
    /// identical output to A at LOD 0 contoured with B at LOD 0. (B has
    /// the same voxel data either way; `cluster.get` returns the same
    /// voxel regardless of how `Lod` is labelled on the neighbor entry.)
    #[test]
    fn fine_side_independent_of_neighbor_lod() {
        let a = build_sphere_cluster();
        let b = build_sphere_cluster();
        let m_lod2 = contour_cluster_lod_with_neighbors(
            &a,
            Lod::ZERO,
            &NeighborContext {
                pos_x: Some((&b, Lod::new(2).unwrap())),
                ..NeighborContext::none()
            },
        );
        let m_lod0 = contour_cluster_lod_with_neighbors(
            &a,
            Lod::ZERO,
            &NeighborContext {
                pos_x: Some((&b, Lod::ZERO)),
                ..NeighborContext::none()
            },
        );
        assert_eq!(m_lod2.vertices(), m_lod0.vertices());
        assert_eq!(m_lod2.indices(), m_lod0.indices());
        assert_eq!(m_lod2.metadata(), m_lod0.metadata());
    }
}
