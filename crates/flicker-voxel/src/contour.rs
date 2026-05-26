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
//! # Future extensions (NOT Phase 2)
//!
//! - QEF-based vertex placement using Hermite edge intersections, for
//!   sharp-feature preservation. Slots in at the `compute_vertex` step
//!   without changing face emission.
//! - LOD subsampling (Phase 3) and transition cells (Phase 4) will both
//!   call into this module with progressively richer cluster inputs.
//! - Worker-thread invocation (Phase 5) — the function is already pure
//!   and `Send`-compatible.

use crate::cluster::CLUSTER_DIM;
use crate::{Cluster, LocalCoord, Material, Voxel};

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

fn empty_mesh() -> CellMesh {
    CellMesh {
        vertices: Vec::new(),
        indices: Indices::U16(Vec::new()),
        metadata: MeshMetadata {
            lod: 0,
            cluster_dim: CLUSTER_DIM,
            vertex_count: 0,
            triangle_count: 0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
        },
    }
}

/// Contour `cluster` into a [`CellMesh`].
///
/// Deterministic: the same `cluster` input always produces byte-identical
/// output across runs and across platforms (modulo target-`f32` rounding,
/// which is IEEE-754 stable).
#[must_use]
pub fn contour_cluster(cluster: &Cluster) -> CellMesh {
    // --- Shortcut 1: cluster is uniform (no overrides). ---
    if cluster.is_uniform() {
        return empty_mesh();
    }

    // --- Shortcut 2: every override has the same classification as the base. ---
    // Walks only overrides — sparse, cheap.
    let base_solid = is_voxel_solid(cluster.base());
    let any_override_differs = cluster
        .overrides()
        .any(|(_, v)| is_voxel_solid(v) != base_solid);
    if !any_override_differs {
        return empty_mesh();
    }

    // --- Pre-classify all voxels into a flat array for fast lookups. ---
    let dim = CLUSTER_DIM as usize;
    let stride_y = dim;
    let stride_z = dim * dim;
    let voxel_idx = |x: u32, y: u32, z: u32| -> usize {
        x as usize + y as usize * stride_y + z as usize * stride_z
    };
    let mut is_solid = vec![base_solid; dim * dim * dim];
    for (coord, voxel) in cluster.overrides() {
        is_solid[voxel_idx(coord.x(), coord.y(), coord.z())] = is_voxel_solid(voxel);
    }
    let solid_at = |x: u32, y: u32, z: u32| -> bool { is_solid[voxel_idx(x, y, z)] };

    // --- Pass 1: scan cells, emit vertices. ---
    let cell_max = CLUSTER_DIM - 1;
    let cell_dim = cell_max as usize;
    let cell_stride_y = cell_dim;
    let cell_stride_z = cell_dim * cell_dim;
    let cell_idx = |cx: u32, cy: u32, cz: u32| -> usize {
        cx as usize + cy as usize * cell_stride_y + cz as usize * cell_stride_z
    };

    let mut vertices: Vec<Vertex> = Vec::new();
    // `cell_vertex[cell_idx(cx, cy, cz)] == u32::MAX` means no vertex; else
    // it's the index into `vertices`.
    let mut cell_vertex: Vec<u32> = vec![u32::MAX; cell_dim * cell_dim * cell_dim];

    let mut bounds_min = [f32::INFINITY; 3];
    let mut bounds_max = [f32::NEG_INFINITY; 3];

    for cz in 0..cell_max {
        for cy in 0..cell_max {
            for cx in 0..cell_max {
                // Read the 8 classifications. Index encoding: bit 0 = X bit,
                // bit 1 = Y bit, bit 2 = Z bit. So idx = i + 2j + 4k.
                let mut classes = [false; 8];
                let mut any_solid = false;
                let mut all_solid = true;
                for k in 0..=1u32 {
                    for j in 0..=1u32 {
                        for i in 0..=1u32 {
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
                let mut corners = [[0.0_f32; 3]; 8];
                let mut materials = [Material::EMPTY; 8];
                for k in 0..=1u32 {
                    for j in 0..=1u32 {
                        for i in 0..=1u32 {
                            let vx = cx + i;
                            let vy = cy + j;
                            let vz = cz + k;
                            let v = cluster.get(LocalCoord::new(vx, vy, vz).expect("in bounds"));
                            let [dx, dy, dz] = v.corner().to_components();
                            let idx = (i + (j << 1) + (k << 2)) as usize;
                            corners[idx] = [vx as f32 + dx, vy as f32 + dy, vz as f32 + dz];
                            materials[idx] = v.material();
                        }
                    }
                }

                // Centroid of the 8 owned corners.
                let mut centroid = [0.0_f32; 3];
                for c in &corners {
                    centroid[0] += c[0];
                    centroid[1] += c[1];
                    centroid[2] += c[2];
                }
                centroid[0] /= 8.0;
                centroid[1] /= 8.0;
                centroid[2] /= 8.0;

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
                    // All 8 corners mixed but zero gradient — only possible
                    // in pathological configurations not produced by the
                    // Phase 2 test set. Pick a stable fallback.
                    [0.0, 1.0, 0.0]
                };

                // Material: closest-owned-corner solid voxel.
                let mut best_d2 = f32::INFINITY;
                let mut best_material = Material::EMPTY;
                for idx in 0..8 {
                    if !classes[idx] {
                        continue;
                    }
                    let c = corners[idx];
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

    if vertices.is_empty() {
        // Defensive — the two shortcuts above should already cover this case.
        return empty_mesh();
    }

    // --- Pass 2: face emission per axis. ---
    //
    // For each voxel-grid edge between two adjacent voxels with different
    // classifications, the 4 cells sharing that edge form a quad of mesh
    // vertices. Per-axis winding is chosen so the triangle normal points
    // from solid to not-solid:
    //
    //   X-axis edge (vx,vy,vz)→(vx+1,vy,vz): 4 cells in YZ plane at X=vx.
    //     Natural winding (A=ll, B=hl, C=hh, D=lh) gives +X normal.
    //   Y-axis edge: 4 cells in XZ plane at Y=vy.
    //     Natural winding gives -Y normal.
    //   Z-axis edge: 4 cells in XY plane at Z=vz.
    //     Natural winding gives +Z normal.
    //
    // We flip the winding when the lower-coord side is not-solid.

    let mut indices_u32: Vec<u32> = Vec::new();

    // X-axis edges: vx ∈ [0, 254], vy ∈ [1, 254], vz ∈ [1, 254].
    for vz in 1..cell_max {
        for vy in 1..cell_max {
            for vx in 0..cell_max {
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
                    // Normal +X — natural winding.
                    indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                } else {
                    // Normal -X — reverse winding.
                    indices_u32.extend_from_slice(&[va, vc, vb, va, vd, vc]);
                }
            }
        }
    }

    // Y-axis edges: vy ∈ [0, 254], vx ∈ [1, 254], vz ∈ [1, 254].
    for vz in 1..cell_max {
        for vy in 0..cell_max {
            for vx in 1..cell_max {
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
                    // Want +Y normal; natural winding gives -Y → reverse.
                    indices_u32.extend_from_slice(&[va, vd, vc, va, vc, vb]);
                } else {
                    // Want -Y normal; natural winding gives -Y → keep.
                    indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                }
            }
        }
    }

    // Z-axis edges: vz ∈ [0, 254], vx ∈ [1, 254], vy ∈ [1, 254].
    for vz in 0..cell_max {
        for vy in 1..cell_max {
            for vx in 1..cell_max {
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
                    // Normal +Z — natural winding.
                    indices_u32.extend_from_slice(&[va, vb, vc, va, vc, vd]);
                } else {
                    // Normal -Z — reverse.
                    indices_u32.extend_from_slice(&[va, vc, vb, va, vd, vc]);
                }
            }
        }
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
            lod: 0,
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
        // y < 128 solid, y ≥ 128 empty.
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 0..256u32 {
            for y in 0..128u32 {
                for x in 0..256u32 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
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
        // Solid sphere, radius 64, centered at (128, 128, 128).
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
        // 64³ cube of solid voxels from 96..160.
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 96u32..160 {
            for y in 96u32..160 {
                for x in 96u32..160 {
                    c.set(coord(x, y, z), v);
                }
            }
        }
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
}
