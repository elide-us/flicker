//! Per-cell dual contouring: turn a contoured [`Cluster`] into a flat-
//! shaded mesh.
//!
//! Pipeline stage: **[contour to voxel](crate::contour) → cluster → mesh**.
//! For every axis-aligned grid edge whose two endpoints differ in solidity,
//! the four cells sharing that edge contribute one quad. Each cell's vertex
//! is the position carried by its **min-corner voxel** — the owned-vertex
//! formula `p(x,y,z) = [x, y, z] + voxel.corner().to_components()`. Vertices
//! are duplicated per quad (four fresh ones each, all carrying the oriented
//! face normal) so the result is flat-shaded with crisp facets.
//!
//! Solidity is `voxel.material() != Material::EMPTY`. Coordinates outside
//! `[0, CLUSTER_DIM)` are looked up through the [`NeighborContext`]: a
//! single-axis-OOB voxel routes to the matching face neighbor (so the
//! quad emits at the seam); two or three coords OOB falls back to the
//! cluster's base. With [`NeighborContext::none()`] every face is a world
//! boundary and the cluster-border faces are intentionally left open.
//!
//! # Seam ownership (low-side)
//!
//! Each seam quad is emitted by exactly one side. The +-axis boundary
//! edge `g.a = dim - 1 → dim` (which crosses the seam plane) is owned
//! by the **low** side: when that neighbor is `Some`, the loop resolves
//! the far endpoint's solidity via [`read_corner`] instead of skipping.
//! The 4 surrounding cells live entirely in the +-seam shell at
//! `lx = dim - 1` (contour stored their vertices there).
//!
//! Tangent quads at the seam fall out from the high side's normal
//! iteration: a perpendicular edge at the high side's `gx = 0` gathers
//! cells whose min-corners reach `lx = -1`, which `read_corner` resolves
//! across to the low side's stored vertex. The self-local cell coord
//! (including `-1`) plus the stored corner offset is the vertex
//! position in self's frame — the same arithmetic as any interior cell,
//! and byte-equal in world space to the low side's emission because
//! both read the exact same stored offset.
//!
//! No graphics dependency: the output [`ClusterVertex`] is field-for-
//! field convertible to `flicker-render`'s `MeshVertex`, but mapping
//! happens in the consumer (see `examples/voxel-cluster`).

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::neighbor::{read_corner, Lod, NeighborContext};

/// One flat-shaded mesh vertex in cluster-local voxel units. Field-for-
/// field convertible to `flicker-render`'s `MeshVertex` — the example does
/// the mapping so this crate stays graphics-free.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClusterVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// CPU mesh: per-quad-duplicated vertices plus a `u32` triangle-index list.
#[derive(Clone, Debug, Default)]
pub struct ClusterMesh {
    pub vertices: Vec<ClusterVertex>,
    pub indices: Vec<u32>,
}

impl ClusterMesh {
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    #[inline]
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Regenerate a renderable mesh from a contoured cluster via per-cell
/// dual contouring. `neighbors` supplies face neighbors for seam
/// closure; pass [`NeighborContext::none`] for a free-standing cluster.
/// `lod` is the cluster's level-of-detail: iteration steps in **sample
/// indices** of `[0, sample_dim)` and each cell occupies `stride^3`
/// voxels. Stored cell corners are in cell-units `[0, 1]`; decoded
/// positions are in cluster-local voxel coords (`origin + corner·stride`).
/// At LOD 0 (`stride = 1`) this reduces to the pre-§2 behavior exactly.
#[must_use]
pub fn mesh(cluster: &Cluster, neighbors: &NeighborContext<'_>, lod: Lod) -> ClusterMesh {
    let mut out = ClusterMesh::default();
    let stride = lod.stride() as i32;
    let sample_dim = lod.sample_dim() as i32;

    // ±1 LOD adjacency. The cross-LOD seam approach assumes the
    // transition between any two adjacent clusters is 2:1 or smaller —
    // higher steps would need recursive transition rows we don't build.
    // World-gen at the layer above is responsible for honouring this;
    // panic loudly here so a violation is impossible to miss.
    for axis in 0..3 {
        for positive in [false, true] {
            if let Some((_, nl)) = neighbor_at(neighbors, axis, positive) {
                let diff = nl.level() as i32 - lod.level() as i32;
                assert!(
                    diff.abs() <= 1,
                    "neighbor LOD {} differs from self LOD {} by more than 1",
                    nl.level(),
                    lod.level()
                );
            }
        }
    }

    // Per-seam emission rule: each shared seam is emitted by exactly
    // one cluster — the finer of the two, with low-side ownership as
    // tiebreaker for uniform LOD (which keeps §1 byte-identical).
    //
    // The two rules below derive from that single principle:
    //   • At the +-axis seam (self's +X/+Y/+Z face), self emits iff
    //     self_lod ≤ +-axis-neighbor.lod  (finer-or-tie).
    //   • At the −-axis seam (self's −X/−Y/−Z face), self emits iff
    //     self_lod <  −-axis-neighbor.lod (strictly finer; ties go to
    //     the −-axis neighbor as the low side).
    let plus_neighbor_skip = |axis: usize| -> bool {
        // Returns true when self is coarser than the +-axis neighbor —
        // the finer neighbor will emit the seam from its −-axis face.
        neighbor_at(neighbors, axis, true)
            .map(|(_, nl)| nl.level() < lod.level())
            .unwrap_or(false)
    };

    for gz in 0..sample_dim {
        for gy in 0..sample_dim {
            for gx in 0..sample_dim {
                let g_sample = [gx, gy, gz];
                let g_voxel = sample_to_voxel(g_sample, stride);
                let s_g = is_solid_in_range(cluster, g_voxel);

                for axis_a in 0..3 {
                    let mut n_sample = g_sample;
                    n_sample[axis_a] += 1;
                    let n_voxel = sample_to_voxel(n_sample, stride);

                    // At the +-axis seam crossing (n_sample[axis_a] ==
                    // sample_dim), skip emission if self is coarser
                    // than the +-axis neighbor — that neighbor (being
                    // finer) owns the seam from its −-axis face.
                    if n_sample[axis_a] == sample_dim
                        && plus_neighbor_skip(axis_a)
                    {
                        continue;
                    }

                    let s_n = if n_sample[axis_a] < sample_dim {
                        is_solid_in_range(cluster, n_voxel)
                    } else if has_face_neighbor(neighbors, axis_a, true) {
                        is_solid_with_neighbors(cluster, neighbors, n_voxel)
                    } else {
                        continue;
                    };
                    if s_g == s_n {
                        continue;
                    }

                    let cells = [
                        cell_coord(g_sample, axis_a, 0, 0),
                        cell_coord(g_sample, axis_a, -1, 0),
                        cell_coord(g_sample, axis_a, -1, -1),
                        cell_coord(g_sample, axis_a, 0, -1),
                    ];
                    let Some(positions) =
                        resolve_cell_vertices(cluster, neighbors, &cells, stride, sample_dim)
                    else {
                        continue;
                    };

                    let mut expected_normal = [0.0_f32; 3];
                    expected_normal[axis_a] = if s_g { 1.0 } else { -1.0 };

                    let solid_endpoint = if s_g { g_voxel } else { n_voxel };
                    let material = solid_material(cluster, neighbors, solid_endpoint);

                    push_quad(&mut out, positions, expected_normal, material);
                }
            }
        }
    }

    // Cross-LOD takeover: when self is strictly finer than a −-axis
    // neighbor, self emits the +-axis-crossing seam quads at that face
    // (the work the coarser low-side neighbor would have done at
    // uniform LOD). The boundary layer's iteration runs at the
    // coarser stride; the 4 cells are all in the coarser neighbor
    // (read via `read_corner`, decoded with §3a's per-neighbor stride).
    for axis_a in 0..3 {
        let Some((_, neighbor_lod)) = neighbor_at(neighbors, axis_a, false) else {
            continue;
        };
        if neighbor_lod.level() <= lod.level() {
            continue;
        }
        emit_neg_seam_takeover(&mut out, cluster, neighbors, lod, neighbor_lod, axis_a);
    }

    out
}

/// Emit the +-axis-crossing seam quads on self's −-axis face when self
/// is strictly finer than the −-axis neighbor. Iterates the in-axis
/// directions at the coarser stride; the 4 cells around each seam
/// edge all live in the coarser neighbor and are read via
/// [`read_corner`] (§3a decode handles the per-neighbor stride).
fn emit_neg_seam_takeover(
    out: &mut ClusterMesh,
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    self_lod: Lod,
    neighbor_lod: Lod,
    axis_a: usize,
) {
    let s_self = self_lod.stride() as i32;
    let s_n = neighbor_lod.stride() as i32;
    let n_n = neighbor_lod.sample_dim() as i32;
    let coarse_step = s_n / s_self; // step in self's sample-index units

    let b_axis = (axis_a + 1) % 3;
    let c_axis = (axis_a + 2) % 3;

    // Iterate the in-axis directions at coarse cadence. Skip the
    // corners (j_b == 0 or j_c == 0) — those would have edge-neighbor
    // cells (multi-axis OOB), which face-neighbor-only meshing
    // doesn't handle.
    for j_b in 1..n_n {
        for j_c in 1..n_n {
            // Edge endpoints (self-local voxel coords).
            // near = self-side endpoint at voxel 0 on the OOB axis,
            // far  = neighbor-side endpoint at voxel −s_n on the OOB axis.
            let mut near_v = [0i32; 3];
            near_v[axis_a] = 0;
            near_v[b_axis] = j_b * s_n;
            near_v[c_axis] = j_c * s_n;
            let mut far_v = near_v;
            far_v[axis_a] = -s_n;

            // Solidity check: near is in self at a coarse-aligned slot.
            let s_near = is_solid_in_range(cluster, near_v);
            // Far is in the −-axis neighbor at its seam-shell row.
            let s_far = is_solid_with_neighbors(cluster, neighbors, far_v);
            if s_near == s_far {
                continue;
            }

            // 4 cells around the +-axis-crossing edge, in perimeter
            // order matching `cell_coord` with axis_a = face axis.
            // All 4 are at self-sample coord −coarse_step on the face
            // axis (which is voxel −s_n in self's frame, OOB → routes
            // to the neighbor via read_corner).
            let g_sample = {
                let mut s = [0i32; 3];
                s[axis_a] = 0;
                s[b_axis] = j_b * coarse_step;
                s[c_axis] = j_c * coarse_step;
                s
            };
            let cells_coarse: [[i32; 3]; 4] = {
                // Equivalent to cell_coord(g, axis_a, db, dc) with
                // (db, dc) ∈ {(0,0), (-1,0), (-1,-1), (0,-1)} stepping
                // in coarse-cadence units (= `coarse_step` in self's
                // sample-index space).
                let offsets = [(0, 0), (-1, 0), (-1, -1), (0, -1)];
                let mut out_cells = [[0i32; 3]; 4];
                for (i, &(db, dc)) in offsets.iter().enumerate() {
                    let mut c = g_sample;
                    c[axis_a] -= coarse_step; // step back to the −-side cell row
                    c[b_axis] += db * coarse_step;
                    c[c_axis] += dc * coarse_step;
                    out_cells[i] = c;
                }
                out_cells
            };

            let Some(positions) = resolve_cell_vertices(
                cluster,
                neighbors,
                &cells_coarse,
                s_self,
                self_lod.sample_dim() as i32,
            ) else {
                continue;
            };

            let mut expected_normal = [0.0_f32; 3];
            expected_normal[axis_a] = if s_near { -1.0 } else { 1.0 };

            let solid_endpoint = if s_near { near_v } else { far_v };
            let material = solid_material(cluster, neighbors, solid_endpoint);

            push_quad(out, positions, expected_normal, material);
        }
    }
}

#[inline]
fn sample_to_voxel(s: [i32; 3], stride: i32) -> [i32; 3] {
    [s[0] * stride, s[1] * stride, s[2] * stride]
}

/// Face neighbor on the OOB side of `coord` (if any), as the stored
/// `(cluster, lod)` pair. Caller is expected to have already ensured
/// exactly one OOB axis; multi-axis OOB returns `None`.
fn neighbor_at<'a>(
    neighbors: &NeighborContext<'a>,
    axis: usize,
    positive: bool,
) -> Option<(&'a Cluster, Lod)> {
    match (axis, positive) {
        (0, false) => neighbors.neg_x,
        (0, true) => neighbors.pos_x,
        (1, false) => neighbors.neg_y,
        (1, true) => neighbors.pos_y,
        (2, false) => neighbors.neg_z,
        (2, true) => neighbors.pos_z,
        _ => None,
    }
}

/// LOD of the face neighbor on the OOB side of `coord`. Returns `None`
/// when no axis is OOB or the matching face neighbor is absent. Used
/// by [`cell_vertex`] to scale the decoded corner by the neighbor's
/// stride (the corner is stored in the neighbor's cell-units, which
/// differ from self's at a cross-LOD face).
fn face_neighbor_lod(neighbors: &NeighborContext<'_>, coord: [i32; 3]) -> Option<Lod> {
    let dim = CLUSTER_DIM as i32;
    for axis in 0..3 {
        let c = coord[axis];
        if c < 0 {
            return neighbor_at(neighbors, axis, false).map(|(_, l)| l);
        }
        if c >= dim {
            return neighbor_at(neighbors, axis, true).map(|(_, l)| l);
        }
    }
    None
}

/// Resolve the 4-cell gather (cells in sample-index space) to 4 vertex
/// positions in self's cluster-local voxel coords. A cell with sample
/// index in range is read from `cluster`; one with a single coord at
/// `-1` or `sample_dim` is routed to the matching face neighbor via
/// [`read_corner`] and its stored corner is decoded into self's frame.
/// Two-or-three-axis OOB returns `None` (the quad is skipped).
fn resolve_cell_vertices(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    cells: &[[i32; 3]; 4],
    stride: i32,
    sample_dim: i32,
) -> Option<[[f32; 3]; 4]> {
    let mut out = [[0.0_f32; 3]; 4];
    for (i, &cell) in cells.iter().enumerate() {
        out[i] = cell_vertex(cluster, neighbors, cell, stride, sample_dim)?;
    }
    Some(out)
}

/// Append one oriented quad (4 vertices, 6 indices). The quad's face
/// normal is the geometric normal of triangle `(v0,v1,v2)`; if it points
/// opposite to `expected_normal`, the vertex order is reversed so the
/// emitted face faces the solid→empty direction.
fn push_quad(
    out: &mut ClusterMesh,
    positions: [[f32; 3]; 4],
    expected_normal: [f32; 3],
    material: u32,
) {
    let v0 = positions[0];
    let v1 = positions[1];
    let v2 = positions[2];
    let v3 = positions[3];

    let raw_normal = cross(sub(v1, v0), sub(v2, v0));
    let mut face_normal = normalize(raw_normal);

    let ordered = if dot(face_normal, expected_normal) < 0.0 {
        face_normal = [-face_normal[0], -face_normal[1], -face_normal[2]];
        // Keep v0 in place, reverse the rest: both triangles flip sign.
        [v0, v3, v2, v1]
    } else {
        [v0, v1, v2, v3]
    };

    let base = out.vertices.len() as u32;
    for p in ordered {
        out.vertices.push(ClusterVertex {
            position: p,
            normal: face_normal,
            material,
        });
    }
    out.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Min-corner coord of one of the four cells around an edge along
/// `axis_a` at grid point `g`. `db`, `dc` are signed offsets in the two
/// non-`a` axes, which are taken cyclically: `b = (a+1) % 3`,
/// `c = (a+2) % 3`. The cyclic choice makes `e_b × e_c = e_a`, which is
/// what gives the quad a `+e_a` geometric normal in perimeter order.
fn cell_coord(g: [i32; 3], axis_a: usize, db: i32, dc: i32) -> [i32; 3] {
    let b = (axis_a + 1) % 3;
    let c = (axis_a + 2) % 3;
    let mut out = g;
    out[b] += db;
    out[c] += dc;
    out
}

/// Vertex position for a cell (sample index) in self's cluster-local
/// voxel coords. In-range cells read from `cluster`; single-axis-OOB
/// cells read the neighbor's stored corner via [`read_corner`] and
/// decode it through the same `origin + corner·stride` formula —
/// negative or sample-dim-boundary sample indices included. Multi-axis
/// OOB returns `None`.
fn cell_vertex(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    cell_sample: [i32; 3],
    stride: i32,
    sample_dim: i32,
) -> Option<[f32; 3]> {
    let oob_count = u32::from(!cell_sample_in_range(cell_sample[0], sample_dim))
        + u32::from(!cell_sample_in_range(cell_sample[1], sample_dim))
        + u32::from(!cell_sample_in_range(cell_sample[2], sample_dim));
    if oob_count >= 2 {
        return None;
    }

    // Cluster-local voxel coord of the cell's min corner at self's
    // stride. Out-of-cluster samples (`-1` or `sample_dim`) produce
    // `-stride` or `CLUSTER_DIM`, which `read_corner` routes to the
    // matching face neighbor.
    let mut cell_voxel = sample_to_voxel(cell_sample, stride);

    let (voxel, decode_stride) = if oob_count == 0 {
        let v = cluster.get(
            LocalCoord::new(cell_voxel[0] as u32, cell_voxel[1] as u32, cell_voxel[2] as u32)
                .expect("in range"),
        );
        (v, stride)
    } else if !single_axis_face_neighbor_present(neighbors, cell_voxel) {
        return None;
    } else {
        let neighbor_lod = face_neighbor_lod(neighbors, cell_voxel)
            .expect("single-axis OOB + neighbor present implies a face neighbor");
        let s_n = neighbor_lod.stride() as i32;
        // Cross-LOD snap: when the neighbor is coarser, the data lives
        // only at coarse-aligned voxel slots — and the stored corner is
        // in cell-units relative to the *coarse* cell's min-corner.
        // Floor cell_voxel to the coarser stride on every axis so:
        //   • the read lands on the coarse cell that contains our query,
        //   • the decode places the vertex at the coarse cell's actual
        //     QEF position (cell_min + corner·s_coarse) rather than at
        //     a non-aligned fine slot that holds nothing.
        // At uniform LOD `s_n == stride`, the snap is a no-op.
        let s_coarse = s_n.max(stride);
        if s_coarse > stride {
            for axis in 0..3 {
                cell_voxel[axis] = cell_voxel[axis].div_euclid(s_coarse) * s_coarse;
            }
        }
        let v = read_corner(cluster, neighbors, cell_voxel[0], cell_voxel[1], cell_voxel[2]);
        (v, s_coarse)
    };

    let [dx, dy, dz] = voxel.corner().to_components();
    let fs = decode_stride as f32;
    Some([
        cell_voxel[0] as f32 + dx * fs,
        cell_voxel[1] as f32 + dy * fs,
        cell_voxel[2] as f32 + dz * fs,
    ])
}

#[inline]
fn cell_sample_in_range(s: i32, sample_dim: i32) -> bool {
    s >= 0 && s < sample_dim
}

#[inline]
fn has_face_neighbor(neighbors: &NeighborContext<'_>, axis: usize, positive: bool) -> bool {
    neighbor_at(neighbors, axis, positive).is_some()
}

/// `true` iff `coord` has exactly one OOB axis AND the matching face
/// neighbor is present. (Caller is expected to have already filtered
/// out multi-axis OOB.)
fn single_axis_face_neighbor_present(
    neighbors: &NeighborContext<'_>,
    coord: [i32; 3],
) -> bool {
    let dim = CLUSTER_DIM as i32;
    for axis in 0..3 {
        let c = coord[axis];
        if c < 0 {
            return has_face_neighbor(neighbors, axis, false);
        }
        if c >= dim {
            return has_face_neighbor(neighbors, axis, true);
        }
    }
    // No axis OOB — caller used this incorrectly, but treat as "yes,
    // a read is possible" (it's all self-storage).
    true
}

/// Solidity test for an arbitrary voxel, routing OOB through
/// [`NeighborContext`]. With no neighbor on the relevant face, returns
/// `false` (matches the historical world-boundary-is-empty default).
fn is_solid_with_neighbors(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    g: [i32; 3],
) -> bool {
    read_corner(cluster, neighbors, g[0], g[1], g[2]).material() != Material::EMPTY
}

#[inline]
fn is_solid_in_range(cluster: &Cluster, g: [i32; 3]) -> bool {
    let coord = LocalCoord::new(g[0] as u32, g[1] as u32, g[2] as u32)
        .expect("caller verified range");
    cluster.get(coord).material() != Material::EMPTY
}

/// Material of the solid endpoint, with OOB routed through neighbors.
fn solid_material(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    endpoint: [i32; 3],
) -> u32 {
    read_corner(cluster, neighbors, endpoint[0], endpoint[1], endpoint[2])
        .material()
        .raw()
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if l2 > 1e-20 {
        let inv = 1.0 / l2.sqrt();
        [v[0] * inv, v[1] * inv, v[2] * inv]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster_id::ClusterId;
    use crate::contour::contour;
    use crate::corner_vector::CornerVector;
    use crate::primitive::{FlatField, Hermite, Primitive};
    use crate::voxel::Voxel;

    fn grey() -> Material {
        Material::new(1, 1, 0).expect("valid")
    }

    fn origin_id() -> ClusterId {
        ClusterId::new(0, 0, 0, 0)
    }

    /// A small solid cube `[0, n)³` — same shape used in the contour
    /// tests, copied here to avoid an `pub(crate)` carve-out just for one
    /// test. Cheap enough to mesh without the full 16M-voxel fill.
    struct CubeField {
        n: i32,
    }

    impl Primitive for CubeField {
        fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
            (0..self.n).contains(&x) && (0..self.n).contains(&y) && (0..self.n).contains(&z)
        }

        fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
            let ca = [a[0] as f32 + 0.5, a[1] as f32 + 0.5, a[2] as f32 + 0.5];
            let cb = [b[0] as f32 + 0.5, b[1] as f32 + 0.5, b[2] as f32 + 0.5];
            Hermite {
                position: [
                    (ca[0] + cb[0]) * 0.5,
                    (ca[1] + cb[1]) * 0.5,
                    (ca[2] + cb[2]) * 0.5,
                ],
                normal: [cb[0] - ca[0], cb[1] - ca[1], cb[2] - ca[2]],
            }
        }
    }

    /// Geometric (un-normalized) normal of triangle `(p0, p1, p2)`.
    fn tri_normal(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> [f32; 3] {
        cross(sub(p1, p0), sub(p2, p0))
    }

    #[test]
    fn push_quad_orients_against_expected_normal() {
        // Square in the y=0 plane, vertex order chosen so (v1-v0)×(v2-v0)
        // already points +Y.
        let positions = [
            [0.0_f32, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        ];

        let mut m = ClusterMesh::default();
        push_quad(&mut m, positions, [0.0, 1.0, 0.0], 42);

        assert_eq!(m.vertices.len(), 4);
        assert_eq!(m.indices.len(), 6);
        for v in &m.vertices {
            assert!((v.normal[0]).abs() < 1e-5);
            assert!((v.normal[1] - 1.0).abs() < 1e-5);
            assert!((v.normal[2]).abs() < 1e-5);
            assert_eq!(v.material, 42);
        }
        for tri in m.indices.chunks(3) {
            let p0 = m.vertices[tri[0] as usize].position;
            let p1 = m.vertices[tri[1] as usize].position;
            let p2 = m.vertices[tri[2] as usize].position;
            let n = tri_normal(p0, p1, p2);
            assert!(n[1] > 0.0, "triangle {:?} not facing +Y: {:?}", tri, n);
        }
    }

    #[test]
    fn push_quad_reverses_when_expected_normal_flipped() {
        let positions = [
            [0.0_f32, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        ];

        let mut m = ClusterMesh::default();
        push_quad(&mut m, positions, [0.0, -1.0, 0.0], 7);

        for v in &m.vertices {
            assert!((v.normal[1] + 1.0).abs() < 1e-5);
        }
        for tri in m.indices.chunks(3) {
            let p0 = m.vertices[tri[0] as usize].position;
            let p1 = m.vertices[tri[1] as usize].position;
            let p2 = m.vertices[tri[2] as usize].position;
            let n = tri_normal(p0, p1, p2);
            assert!(n[1] < 0.0, "triangle {:?} not facing -Y: {:?}", tri, n);
        }
    }

    #[test]
    fn small_cube_produces_sane_mesh() {
        let c = contour(&CubeField { n: 3 }, grey(), origin_id());
        let m = mesh(&c, &NeighborContext::none(), Lod::ZERO);

        assert!(!m.is_empty(), "cube should produce some quads");
        assert_eq!(m.vertices.len() % 4, 0);
        assert_eq!(m.indices.len() % 6, 0);
        for i in &m.indices {
            assert!((*i as usize) < m.vertices.len());
        }
        for v in &m.vertices {
            let len = (v.normal[0] * v.normal[0]
                + v.normal[1] * v.normal[1]
                + v.normal[2] * v.normal[2])
                .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-3,
                "normal not unit-length: {:?}",
                v.normal
            );
        }
    }

    #[test]
    fn flat_patch_emits_quad_on_the_plane_facing_up() {
        // 2×2 surface-voxel block at y=127 with the QEF-style corner
        // (0.5, 1.0, 0.5) — the contour's result for a half-height flat
        // field, replicated cheaply. The +Y edge at the patch's interior
        // grid point (2, 127, 2) has all four cells inside the block, so
        // every vertex of that quad lies on y=128.
        let mut cluster = Cluster::empty();
        let mat = grey();
        let corner = CornerVector::from_components(0.5, 1.0, 0.5);
        for x in 1..=2u32 {
            for z in 1..=2u32 {
                let c = LocalCoord::new(x, 127, z).expect("in range");
                cluster.set(c, Voxel::new(corner, mat));
            }
        }

        let m = mesh(&cluster, &NeighborContext::none(), Lod::ZERO);
        assert!(!m.is_empty());

        let mut found = false;
        for quad in m.vertices.chunks_exact(4) {
            let on_plane = quad.iter().all(|v| (v.position[1] - 128.0).abs() < 0.01);
            let faces_up = quad.iter().all(|v| {
                v.normal[0].abs() < 0.01
                    && (v.normal[1] - 1.0).abs() < 0.01
                    && v.normal[2].abs() < 0.01
            });
            if on_plane && faces_up {
                found = true;
                break;
            }
        }
        assert!(found, "no +Y face quad at y=128 in patch");
    }

    /// Number of triangles in a mesh whose all 3 vertices land in the
    /// world-X plane at `wx ± tol`. Used to find seam-plane geometry.
    fn tris_on_x_plane(m: &ClusterMesh, wx: f32, tol: f32) -> usize {
        m.indices
            .chunks_exact(3)
            .filter(|t| {
                t.iter().all(|i| {
                    (m.vertices[*i as usize].position[0] - wx).abs() < tol
                })
            })
            .count()
    }

    #[test]
    fn cluster_pair_x_seam_emits_crossing_only_on_low_side() {
        // Two adjacent FlatField clusters along X. A's +X-shell row owns
        // the seam-crossing quads (+X faces of the surface at the seam
        // plane is not what this checks — but A's +X axis-edge emission
        // at gx=255 should produce zero `world x ≈ 256` quads here
        // because the flat-field's surface is horizontal, not vertical).
        //
        // What we DO check: the high side (B) does NOT emit any
        // perpendicular crossing-quad at the seam (no quads whose 4
        // vertices all sit at world x ≈ 256). With low-side ownership
        // and no high-side iteration extension, B emits only the
        // tangent (Y) quads that *span* the seam, never quads parked on
        // it. (For a flat field with no vertical surface, the count is
        // trivially zero on both sides; this is a sanity guard against a
        // future regression that re-introduces -side iteration.)
        let a = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 0, 0, 0));
        let b = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 1, 0, 0));

        let neighbors_a = NeighborContext {
            pos_x: Some((&b, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let neighbors_b = NeighborContext {
            neg_x: Some((&a, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let m_a = mesh(&a, &neighbors_a, Lod::ZERO);
        let m_b = mesh(&b, &neighbors_b, Lod::ZERO);

        // A's mesh is in its local frame (`x ∈ [0, 256]`); world x=256
        // is A-local 256. B's mesh is in B's local frame; world x=256
        // is B-local 0.
        assert_eq!(tris_on_x_plane(&m_a, 256.0, 0.01), 0);
        assert_eq!(tris_on_x_plane(&m_b, 0.0, 0.01), 0);
    }

    #[test]
    fn cluster_pair_x_seam_byte_equal_world_positions() {
        // Two adjacent FlatField clusters: the seam-shell vertex stored
        // in A at lx=255 must equal (in world coords) what B sees when
        // it reads across via read_corner.
        let a = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 0, 0, 0));
        let b = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 1, 0, 0));

        // A's seam-shell cell at (255, 127, 100): vertex in A-local
        // frame is (255, 127, 100) + corner.
        let v_a = a.get(LocalCoord::new(255, 127, 100).unwrap());
        let [dx, dy, dz] = v_a.corner().to_components();
        let world_a = [
            CLUSTER_DIM as f32 * 0.0 + 255.0 + dx,
            127.0 + dy,
            100.0 + dz,
        ];

        // B reads cell (-1, 127, 100) in its local frame via
        // read_corner — routed to A at (255, 127, 100). The vertex in
        // B's local frame is (-1 + dx, 127 + dy, 100 + dz), which
        // becomes world x = B.world_offset.x + (-1 + dx) = 256 + (-1 +
        // dx) = 255 + dx. Same world position as A's emission.
        let neighbors_b = NeighborContext {
            neg_x: Some((&a, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let voxel_via_b = read_corner(&b, &neighbors_b, -1, 127, 100);
        assert_eq!(voxel_via_b.corner(), v_a.corner());

        let b_local = [-1.0 + dx, 127.0 + dy, 100.0 + dz];
        let b_world = [
            CLUSTER_DIM as f32 + b_local[0],
            b_local[1],
            b_local[2],
        ];
        // Both world positions byte-equal.
        assert_eq!(world_a, b_world);
    }

    #[test]
    fn row_of_three_clusters_along_x_seam_closed() {
        // A 1×3 row along X over FlatField::at_half(). Combine the
        // three meshes (each translated by its world offset) and check
        // that every shared edge in the combined mesh appears in
        // exactly two triangles — the watertightness invariant the
        // alignment rule gives us. World-exterior edges (the row's
        // boundary in -X, +X, ±Y, ±Z) are unshared and appear once;
        // we exclude them by skipping edges that lie on the outer
        // bounding box.
        let ids: [ClusterId; 3] = [
            ClusterId::new(0, 0, 0, 0),
            ClusterId::new(0, 1, 0, 0),
            ClusterId::new(0, 2, 0, 0),
        ];
        let clusters: Vec<Cluster> = ids
            .iter()
            .map(|id| contour(&FlatField::at_half(), grey(), *id))
            .collect();

        // Build a NeighborContext per cluster (only X-axis neighbors).
        let contexts: Vec<NeighborContext<'_>> = (0..3)
            .map(|i| NeighborContext {
                neg_x: if i > 0 {
                    Some((&clusters[i - 1], Lod::ZERO))
                } else {
                    None
                },
                pos_x: if i < 2 {
                    Some((&clusters[i + 1], Lod::ZERO))
                } else {
                    None
                },
                ..NeighborContext::none()
            })
            .collect();

        let meshes: Vec<ClusterMesh> = (0..3)
            .map(|i| mesh(&clusters[i], &contexts[i], Lod::ZERO))
            .collect();

        // Combine: translate each mesh by its world offset, dedupe
        // vertices by quantized world position, build the index list,
        // then count edge uses.
        let mut verts_q: Vec<[i64; 3]> = Vec::new();
        let mut vert_lookup: std::collections::HashMap<[i64; 3], u32> =
            std::collections::HashMap::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        let q = |p: [f32; 3]| -> [i64; 3] {
            // Quantize to 1/1024 voxel — well inside the byte-quantized
            // storage precision and far above f32 round-off for the
            // small position magnitudes we use here.
            [
                (p[0] * 1024.0).round() as i64,
                (p[1] * 1024.0).round() as i64,
                (p[2] * 1024.0).round() as i64,
            ]
        };
        for (i, m) in meshes.iter().enumerate() {
            let off = ids[i].world_offset();
            for tri in m.indices.chunks_exact(3) {
                let mut idx = [0u32; 3];
                for (k, &vi) in tri.iter().enumerate() {
                    let p = m.vertices[vi as usize].position;
                    let world = [p[0] + off[0], p[1] + off[1], p[2] + off[2]];
                    let key = q(world);
                    idx[k] = *vert_lookup.entry(key).or_insert_with(|| {
                        let new_id = verts_q.len() as u32;
                        verts_q.push(key);
                        new_id
                    });
                }
                triangles.push(idx);
            }
        }

        // Edge-use histogram.
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        for t in &triangles {
            for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_use.entry(key).or_insert(0) += 1;
            }
        }

        // FlatField vertices sit at cell-centers (x+0.5, 128, z+0.5),
        // but the corner's byte encoding pulls 0.5 to ≈0.5039 (byte
        // 128 decodes to 128/255·2 − 0.5). Quantized at 1024: 516.
        // The surface perimeter is at world x ∈ {0.504, 767.504} and
        // z ∈ {0.504, 255.504}. Cluster-internal seams (world x ∈
        // {256, 512}) are not on these planes — any 1-use edge there
        // is an open seam.
        let half_q: i64 = 516;
        let x_max_q: i64 = (CLUSTER_DIM as i64 * 3 - 1) * 1024 + half_q; // 785924
        let z_max_q: i64 = (CLUSTER_DIM as i64 - 1) * 1024 + half_q; // 261636
        let on_world_boundary = |a: [i64; 3], b: [i64; 3]| -> bool {
            (a[0] == half_q && b[0] == half_q)
                || (a[0] == x_max_q && b[0] == x_max_q)
                || (a[2] == half_q && b[2] == half_q)
                || (a[2] == z_max_q && b[2] == z_max_q)
        };

        let mut over_shared: Vec<((u32, u32), u32)> = Vec::new();
        let mut interior_unshared: Vec<((u32, u32), [i64; 3], [i64; 3])> = Vec::new();
        let mut boundary_count = 0_usize;
        for (&(a, b), &uses) in &edge_use {
            let pa = verts_q[a as usize];
            let pb = verts_q[b as usize];
            if uses > 2 {
                over_shared.push(((a, b), uses));
                continue;
            }
            if uses == 1 {
                if on_world_boundary(pa, pb) {
                    boundary_count += 1;
                } else {
                    interior_unshared.push(((a, b), pa, pb));
                }
            }
        }
        assert!(
            over_shared.is_empty(),
            "edges used by >2 triangles (low-side double-emit?): {:?}",
            over_shared.iter().take(10).collect::<Vec<_>>()
        );
        assert!(
            interior_unshared.is_empty(),
            "interior 1-use edges (open seam?): {:?}",
            interior_unshared.iter().take(10).collect::<Vec<_>>()
        );
        // Expected perimeter: 2*255 z-runs + 2*767 x-runs = 2044.
        let expected_perimeter = 2 * (CLUSTER_DIM as usize - 1)
            + 2 * (3 * CLUSTER_DIM as usize - 1);
        assert_eq!(
            boundary_count, expected_perimeter,
            "world-perimeter edge count mismatch"
        );
    }

    #[test]
    fn lod2_single_cluster_flat_field_watertight() {
        // LOD 2: stride=4, sample_dim=64. The flat field's surface at
        // y=128 still meshes into a sheet, but at coarse resolution
        // (one quad per 4×4 voxel patch in XZ). The sheet is one
        // surface with a single perimeter; no neighbors → all 4 sides
        // are world-boundary edges.
        let id_lod2 = ClusterId::new(2, 0, 0, 0);
        let c = contour(&FlatField::at_half(), grey(), id_lod2);
        let m = mesh(&c, &NeighborContext::none(), Lod::new(2).unwrap());
        assert!(!m.is_empty(), "LOD-2 flat field should produce quads");

        // Every vertex should sit at world y=128 — the LOD-2 cell that
        // straddles the plane has min-corner voxel y=124 and a stored
        // corner.y of 1.0 (cell-units), decoded as 124 + 1.0·4 = 128.
        for v in &m.vertices {
            assert!(
                (v.position[1] - 128.0).abs() < 0.05,
                "vertex y={} off the surface plane",
                v.position[1]
            );
        }

        // Watertight: build the edge-use histogram (same shape as
        // §1's row-of-three but for a single LOD-2 cluster with no
        // neighbors). Every interior edge is 2-use; the perimeter
        // surrounds the sheet. Assert no edges with count > 2 and
        // every 1-use edge sits on the cluster's world-boundary
        // perimeter.
        let q = |p: [f32; 3]| -> [i64; 3] {
            [
                (p[0] * 1024.0).round() as i64,
                (p[1] * 1024.0).round() as i64,
                (p[2] * 1024.0).round() as i64,
            ]
        };
        let mut verts_q: Vec<[i64; 3]> = Vec::new();
        let mut lookup: std::collections::HashMap<[i64; 3], u32> =
            std::collections::HashMap::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for tri in m.indices.chunks_exact(3) {
            let mut idx = [0u32; 3];
            for (k, &vi) in tri.iter().enumerate() {
                let key = q(m.vertices[vi as usize].position);
                idx[k] = *lookup.entry(key).or_insert_with(|| {
                    let id = verts_q.len() as u32;
                    verts_q.push(key);
                    id
                });
            }
            triangles.push(idx);
        }
        let mut edge_use: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        for t in &triangles {
            for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_use.entry(key).or_insert(0) += 1;
            }
        }

        // The LOD-2 cells' QEF vertices land at (i·4 + 2, 128, k·4 + 2)
        // for sample indices i, k in [0, 63]. Byte-encoding doesn't
        // shift the half-cell offset because stride=4 makes the cell-
        // units "0.5" decode to (4·0.5+ε) → 2.0 + a few thousandths.
        // Quantize the expected perimeter range generously: any 1-use
        // edge both of whose endpoints share an extreme X or extreme Z
        // (the smallest/largest unique quantized value across the
        // mesh) is a boundary edge.
        let mut all_xs: Vec<i64> = verts_q.iter().map(|p| p[0]).collect();
        let mut all_zs: Vec<i64> = verts_q.iter().map(|p| p[2]).collect();
        all_xs.sort_unstable();
        all_xs.dedup();
        all_zs.sort_unstable();
        all_zs.dedup();
        let x_min = *all_xs.first().unwrap();
        let x_max = *all_xs.last().unwrap();
        let z_min = *all_zs.first().unwrap();
        let z_max = *all_zs.last().unwrap();
        let on_perim = |a: [i64; 3], b: [i64; 3]| -> bool {
            (a[0] == x_min && b[0] == x_min)
                || (a[0] == x_max && b[0] == x_max)
                || (a[2] == z_min && b[2] == z_min)
                || (a[2] == z_max && b[2] == z_max)
        };
        let mut over_shared: Vec<((u32, u32), u32)> = Vec::new();
        let mut interior_unshared: Vec<((u32, u32), [i64; 3], [i64; 3])> = Vec::new();
        for (&(a, b), &uses) in &edge_use {
            let pa = verts_q[a as usize];
            let pb = verts_q[b as usize];
            if uses > 2 {
                over_shared.push(((a, b), uses));
            } else if uses == 1 && !on_perim(pa, pb) {
                interior_unshared.push(((a, b), pa, pb));
            }
        }
        assert!(
            over_shared.is_empty(),
            "LOD-2: edges used by >2 triangles: {:?}",
            over_shared.iter().take(10).collect::<Vec<_>>()
        );
        assert!(
            interior_unshared.is_empty(),
            "LOD-2: interior 1-use edges (open mesh): {:?}",
            interior_unshared.iter().take(10).collect::<Vec<_>>()
        );
    }

    #[test]
    #[should_panic(expected = "differs from self LOD")]
    fn mesh_panics_on_two_level_lod_jump() {
        // The ±1 adjacency assertion guards the cross-LOD seam approach:
        // the transition is bounded to 2:1, never 4:1 or larger. A self
        // at LOD 0 with an LOD-2 face neighbor must panic.
        let a = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 0, 0, 0));
        let b = contour(&FlatField::at_half(), grey(), ClusterId::new(2, 1, 0, 0));
        let neighbors = NeighborContext {
            pos_x: Some((&b, Lod::new(2).unwrap())),
            ..NeighborContext::none()
        };
        let _ = mesh(&a, &neighbors, Lod::ZERO);
    }

    #[test]
    fn cross_lod_read_decodes_in_neighbors_stride() {
        // A (fine, LOD 0) reads across its +X face to B (coarse, LOD 1).
        // B's stored seam-shell corner at b-voxel (255, ...) is encoded
        // in B's cell-units (relative to s_B=2). When A's mesh reads
        // it across, the decode must scale the corner by B's stride,
        // not A's — otherwise the resulting position is half what it
        // should be. The check: a known B-corner of (0.5, 1.0, 0.5)
        // decodes to a vertex one full B-cell up in y (a 2-voxel step,
        // not 1), and offset by 1 voxel in X/Z.
        let b_lod = Lod::new(1).unwrap();
        let a = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 0, 0, 0));
        let b = contour(&FlatField::at_half(), grey(), ClusterId::new(1, 1, 0, 0));

        // B's seam-shell cell at sample (sample_dim_B-1, 63, 0) =
        // voxel (254, 126, 0); cell spans y=[126, 128] at LOD 1.
        // Stored corner.y in cell-units ≈ 1.0 (on the +Y face = world
        // y=128). Read it across from A at voxel (256, 126, 0) →
        // wraps to B at (0, 126, 0)? No: A's read across goes to its
        // +X neighbor — vx in A's frame at 256 maps to B's vx=0,
        // but B's storage at the +X seam shell is at b-voxel (254, …),
        // not (0, …). So we read B's +X face from A's perspective:
        // A's cross-cluster read at the +-X seam is bx=0, not 254. The
        // cell at sample 0 in B is B's interior `[0, 2]` cell. For a
        // flat field at y=128 that cell is fully below — no QEF stored
        // there. Instead, the meaningful cross-LOD slot is along Y/Z
        // perpendicular reads. Easier to test via `cell_vertex` direct.
        //
        // Construct: A's cell at sample-index (256, 63, 0)? That'd be
        // OOB on +X. cell_voxel = (256, 63, 0). Resolves via
        // read_corner to B at (0, 63, 0). But sample y=63 isn't a B
        // sample (B's samples are even y). Skip — use y=62 instead so
        // it's a B sample (62/s_B = 31).
        let neighbors = NeighborContext {
            pos_x: Some((&b, b_lod)),
            ..NeighborContext::none()
        };
        // Sample (256, 126, 0) — past A's +X face, lands on B at
        // b-voxel (0, 126, 0). This is B's surface-straddling cell at
        // sample (0, 63, 0) (y spans [126, 128]); B's contour stored a
        // corner ≈ (0.5, 1.0, 0.5) here, on +Y face = world y=128.
        let pos = cell_vertex(&a, &neighbors, [256, 126, 0], 1, 256);
        let pos = pos.expect("read across to B should resolve");
        // Decoded with B's stride (2), the cell_voxel = (256, 126, 0)
        // in A's local frame, corner ≈ (0.5, 1.0, 0.5) → vertex
        // (256 + 0.5*2, 126 + 1.0*2, 0 + 0.5*2) = (257, 128, 1).
        // (Bytes encode 0.5 → ~0.504 etc., so allow a small tolerance.)
        assert!(
            (pos[0] - 257.0).abs() < 0.05,
            "x={} should land at ~257 (cross-LOD decode using s=2)",
            pos[0]
        );
        assert!(
            (pos[1] - 128.0).abs() < 0.05,
            "y={} should land on the surface plane",
            pos[1]
        );
        assert!(
            (pos[2] - 1.0).abs() < 0.05,
            "z={} should land at ~1 (0 + 0.5*2)",
            pos[2]
        );
    }

    #[test]
    fn uniform_lod_decode_unchanged_by_neighbor_stride_path() {
        // Regression: at uniform LOD the cross-cluster decode path
        // must still produce the same vertex positions §1's two-
        // cluster byte-equal test guarantees. Re-running that scenario
        // through `cell_vertex` should give A's seam-shell cell read
        // (via cluster.get) and B's view (via read_corner) byte-equal
        // world positions, since neighbor_lod == self_lod → same
        // stride.
        let a = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 0, 0, 0));
        let b = contour(&FlatField::at_half(), grey(), ClusterId::new(0, 1, 0, 0));
        let nb_b = NeighborContext {
            neg_x: Some((&a, Lod::ZERO)),
            ..NeighborContext::none()
        };

        // From A's side: cell at sample (255, 127, 100) — in-storage.
        let v_a =
            cell_vertex(&a, &NeighborContext::none(), [255, 127, 100], 1, 256).expect("in range");
        // From B's side: cell at sample (-1, 127, 100) — read across.
        let v_b = cell_vertex(&b, &nb_b, [-1, 127, 100], 1, 256).expect("read across");

        // World positions: A's vertex is at v_a, B's vertex is at
        // v_b + CLUSTER_DIM on X.
        let v_b_world = [v_b[0] + CLUSTER_DIM as f32, v_b[1], v_b[2]];
        // Byte-equal up to the byte encoding's resolution.
        for axis in 0..3 {
            assert!(
                (v_a[axis] - v_b_world[axis]).abs() < 1e-3,
                "uniform-LOD decode drifted on axis {}: a={} b_world={}",
                axis,
                v_a[axis],
                v_b_world[axis],
            );
        }
    }
}
