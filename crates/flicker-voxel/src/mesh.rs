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
//! Solidity is `voxel.state().is_filled()` (anything that isn't `Empty`).
//! Coordinates outside
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
//!
//! # Conformance role — CPU-meshing fallback (Memory & Resource Architecture spec)
//!
//! This CPU dual-contour mesher is the **`MESH-1` / §7 `FALLBACK-1`
//! CPU-meshing fallback**, explicitly labeled as such. It produces
//! CPU-side geometry that the renderer uploads (`Renderer::upload_mesh`)
//! before drawing — a CPU-mesh → upload → render path. That path is *not*
//! the steady-state target: per `MESH-1` the per-cell contour/QEF work
//! should target a GPU-compute pass emitting `DeviceResident` output that
//! feeds rendering with no CPU round trip (and no upload). The fallback
//! is kept expressible per `FALLBACK-1` and is the only path built today;
//! the label is here so a conformance pass reads it as the deferred
//! fallback, not an accidental steady-state CPU→upload path.
//!
//! # Read surface
//!
//! All voxel/field reads in this module go through a single
//! [`FieldReader`]. It is the one place that decides self-vs-neighbor
//! routing; emission code never holds a bare `&Cluster`, so a
//! coarse-local (self-only) sample is not expressible. This enforces
//! `MESH-3` (coarse-side seam geometry reads the finer neighbor via the
//! routed read, never a coarse-local sample) *structurally* — by what is
//! in scope — rather than by reviewer vigilance.
//!
//! # Known limitation — cluster edge/corner seam gap (deferred)
//!
//! A seam quad whose 4-cell gather reaches a cluster **edge** (2 axes
//! OOB) or **corner** (3 axes OOB) hits [`FieldReader::cell_vertex`]'s
//! multi-axis-OOB `None` and is dropped, because [`NeighborContext`]
//! models only the 6 face neighbors — not the 12 edge or 8 corner
//! neighbors. Observed symptom: two adjacent cells across such an edge
//! fail to agree on the connecting quad, leaving a visible gap (this is
//! the residual `delta <= 16` slack tolerated in
//! `flat_field_3x3_centre_lod1_seam_counts`). Closing it requires
//! extending the neighbor data model and is deferred to a future spec;
//! it is deliberately **not** addressed here. Navigation is unaffected:
//! nav derives from the dense state field (see [`crate::nav`]), not from
//! this mesh, so it never inherits these holes.

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::local_coord::LocalCoord;
use crate::neighbor::{read_corner, Lod, NeighborContext};

/// The single neighbor-aware read surface for the mesher.
///
/// Every voxel/field read in this module goes through a `FieldReader`.
/// It wraps the cluster being meshed plus its [`NeighborContext`], and
/// every method routes out-of-range coordinates through [`read_corner`]
/// — whose in-bounds branch is exactly `Cluster::get`, so the routed
/// read is also the fast path for interior coords, at no extra cost.
///
/// # Why this is the *only* read surface (MESH-3)
///
/// On the coarse side of a cross-LOD boundary, seam geometry must read
/// the finer neighbor across the seam via the routed read — never a
/// coarse-local, self-only sample (the locked `MESH-3` invariant). By
/// exposing only routed reads, and by never handing a bare `&Cluster` to
/// emission code, "read from self only" is not something a call site can
/// express: the invariant is enforced by what is in scope, not by
/// reviewer vigilance. There is deliberately no `cluster()` accessor and
/// no coarse-local sampling method.
// FUTURE: `crate::nav` does its own routed reads through `read_corner`;
// migrating it onto a shared `FieldReader` is possible but out of scope
// for this pass (kept contained to the mesher).
#[derive(Copy, Clone)]
struct FieldReader<'a> {
    cluster: &'a Cluster,
    neighbors: &'a NeighborContext<'a>,
}

impl<'a> FieldReader<'a> {
    #[inline]
    fn new(cluster: &'a Cluster, neighbors: &'a NeighborContext<'a>) -> Self {
        Self { cluster, neighbors }
    }

    /// Solidity at an arbitrary voxel, OOB routed through the
    /// `NeighborContext`. No neighbor on the relevant face (or multi-axis
    /// OOB) reads as `false` — the world-boundary-is-empty default. NB:
    /// `false` here means "empty", *not* "do not emit"; the world-edge
    /// skip decision lives at the call site (see `mesh`).
    #[inline]
    fn solid(&self, g: [i32; 3]) -> bool {
        read_corner(self.cluster, self.neighbors, g[0], g[1], g[2])
            .state()
            .is_filled()
    }

    /// Solidity at a voxel the caller asserts is in `[0, CLUSTER_DIM)` on
    /// every axis. Debug-asserts the precondition (the "interior loop
    /// stays interior" guardrail the old `is_solid_in_range` enforced
    /// with a panic), then reads through the *same* routed path as
    /// [`Self::solid`]. For an in-range coord `read_corner` takes its
    /// `Cluster::get` branch, so the result is identical — no
    /// coarse-local capability is introduced.
    #[inline]
    fn solid_in_range(&self, g: [i32; 3]) -> bool {
        let dim = CLUSTER_DIM as i32;
        debug_assert!(
            (0..dim).contains(&g[0]) && (0..dim).contains(&g[1]) && (0..dim).contains(&g[2]),
            "solid_in_range called with out-of-range coord {g:?}; the interior loop must stay interior"
        );
        self.solid(g)
    }

    /// Raw material of the (solid) endpoint voxel, OOB routed through
    /// neighbors.
    #[inline]
    fn material(&self, endpoint: [i32; 3]) -> u32 {
        read_corner(
            self.cluster,
            self.neighbors,
            endpoint[0],
            endpoint[1],
            endpoint[2],
        )
        .material()
        .raw()
    }

    /// `true` iff a face neighbor is present on the given face. A
    /// presence check, not a field read — it reads no voxel data, so it
    /// does not reopen the coarse-local path. Lets the boundary pass take
    /// only a `&FieldReader`.
    #[inline]
    fn has_face_neighbor(&self, axis: usize, positive: bool) -> bool {
        has_face_neighbor(self.neighbors, axis, positive)
    }

    /// Vertex position for a cell whose min-corner voxel is `cell_voxel`
    /// in self's cluster-local frame.
    ///
    /// Contour stores the same `CornerVector` at every voxel inside an
    /// active LOD-cell's footprint — cell-units relative to that cell's
    /// min-corner with self's stride. Decode snaps `cell_voxel` down to
    /// the nearest stride-aligned position before applying
    /// `voxel + corner·stride`, so all voxels in one footprint resolve to
    /// the same byte-equal world position.
    ///
    /// `self_stride` is self's own LOD stride. A single-axis-OOB cell is
    /// routed through [`read_corner`] to the neighbor and decoded with
    /// the **neighbor's** stride (the corner was encoded in the
    /// neighbor's frame). Two-or-three-axis OOB returns `None` — the
    /// cluster edge/corner seam gap documented at the module level; it is
    /// left as-is here (not a watertightness fix).
    fn cell_vertex(&self, cell_voxel: [i32; 3], self_stride: i32) -> Option<[f32; 3]> {
        let dim = CLUSTER_DIM as i32;
        let in_x = (0..dim).contains(&cell_voxel[0]);
        let in_y = (0..dim).contains(&cell_voxel[1]);
        let in_z = (0..dim).contains(&cell_voxel[2]);
        let oob_count = u32::from(!in_x) + u32::from(!in_y) + u32::from(!in_z);
        if oob_count >= 2 {
            return None;
        }

        let (voxel, decode_stride) = if oob_count == 0 {
            let v = self.cluster.get(
                LocalCoord::new(
                    cell_voxel[0] as u32,
                    cell_voxel[1] as u32,
                    cell_voxel[2] as u32,
                )
                .expect("in range"),
            );
            (v, self_stride)
        } else if !single_axis_face_neighbor_present(self.neighbors, cell_voxel) {
            return None;
        } else {
            let neighbor_lod = face_neighbor_lod(self.neighbors, cell_voxel)
                .expect("single-axis OOB + neighbor present implies a face neighbor");
            let v = read_corner(
                self.cluster,
                self.neighbors,
                cell_voxel[0],
                cell_voxel[1],
                cell_voxel[2],
            );
            (v, neighbor_lod.stride() as i32)
        };

        // Snap `cell_voxel` to the decode stride so every voxel in a cell
        // footprint produces the same world position.
        let snapped = [
            cell_voxel[0].div_euclid(decode_stride) * decode_stride,
            cell_voxel[1].div_euclid(decode_stride) * decode_stride,
            cell_voxel[2].div_euclid(decode_stride) * decode_stride,
        ];

        let [dx, dy, dz] = voxel.corner().to_components();
        let fs = decode_stride as f32;
        Some([
            snapped[0] as f32 + dx * fs,
            snapped[1] as f32 + dy * fs,
            snapped[2] as f32 + dz * fs,
        ])
    }
}

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

    /// Per-edge use count in this mesh's triangle list. Each edge is
    /// keyed by `(min_vertex_index, max_vertex_index)`; the value is
    /// the number of triangles referencing that pair. After
    /// [`Self::weld`], a watertight surface has every edge at:
    ///
    /// - `2` — interior edge, shared by two triangles.
    /// - `1` — boundary edge, on the world-exterior perimeter of the
    ///   surface (legitimate if the surface terminates at e.g. the
    ///   cluster's world-Y boundary).
    /// - `> 2` — over-share, a bug (the same edge is in multiple
    ///   stacked triangles).
    ///
    /// A `1`-count edge in the *interior* of a surface is a topological
    /// gap. Use this in diagnostic code to identify where the contour
    /// + mesh pipeline is failing to close.
    pub fn edge_use_histogram(&self) -> std::collections::HashMap<(u32, u32), u32> {
        let mut out = std::collections::HashMap::new();
        for tri in self.indices.chunks_exact(3) {
            for &(a, b) in &[(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *out.entry(key).or_insert(0_u32) += 1;
            }
        }
        out
    }

    /// Collapse vertices with byte-equal positions to a single index,
    /// averaging+renormalizing their normals.
    ///
    /// Dual contouring with per-quad vertex emission produces many
    /// "duplicate" vertices: each shared cell appears in up to four
    /// quads, and each emission pushes a fresh vertex slot. Even when
    /// the positions are mathematically identical (same byte pattern
    /// in cluster-local coords), the GPU's vertex shader can drift them
    /// apart by sub-ULP through the MVP transform, leaving a pixel-
    /// wide gap that rasterizes as background between adjacent
    /// triangles. Sharing the vertex *index* eliminates that drift —
    /// both triangles read the cached vertex shader output.
    ///
    /// The cost is that per-quad face normals are averaged into per-
    /// vertex smooth normals. For curved surfaces (heightmap) that's
    /// strictly better; for sharp features (cube edges) it softens the
    /// corner slightly. Adding smooth-group handling is left for a
    /// later pass.
    pub fn weld(&mut self) {
        if self.vertices.is_empty() {
            return;
        }
        use std::collections::HashMap;

        let mut by_pos: HashMap<[u32; 3], u32> = HashMap::with_capacity(self.vertices.len());
        let mut new_vertices: Vec<ClusterVertex> = Vec::with_capacity(self.vertices.len());
        // Parallel buffers for normal accumulation. We could store the
        // running sum in the vertex itself, but keeping them separate
        // makes the renormalize step a single linear scan.
        let mut normal_sums: Vec<[f32; 3]> = Vec::with_capacity(self.vertices.len());
        let mut normal_counts: Vec<u32> = Vec::with_capacity(self.vertices.len());
        let mut old_to_new: Vec<u32> = Vec::with_capacity(self.vertices.len());

        for v in &self.vertices {
            // Bit-pattern key. Two vertices weld iff their position
            // floats are byte-identical; mathematically equal-but-bit-
            // different positions (e.g. `-0.0` vs `0.0`) do not weld,
            // which is intentional — we want lossless welding of the
            // actually-shared cell vertices and no accidental merge of
            // anything else.
            let key = [
                v.position[0].to_bits(),
                v.position[1].to_bits(),
                v.position[2].to_bits(),
            ];
            let new_idx = match by_pos.get(&key) {
                Some(&idx) => {
                    let u = idx as usize;
                    normal_sums[u][0] += v.normal[0];
                    normal_sums[u][1] += v.normal[1];
                    normal_sums[u][2] += v.normal[2];
                    normal_counts[u] += 1;
                    idx
                }
                None => {
                    let idx = new_vertices.len() as u32;
                    new_vertices.push(*v);
                    normal_sums.push(v.normal);
                    normal_counts.push(1);
                    by_pos.insert(key, idx);
                    idx
                }
            };
            old_to_new.push(new_idx);
        }

        for i in 0..new_vertices.len() {
            let n = normal_counts[i] as f32;
            let avg = [
                normal_sums[i][0] / n,
                normal_sums[i][1] / n,
                normal_sums[i][2] / n,
            ];
            new_vertices[i].normal = normalize(avg);
        }

        for idx in self.indices.iter_mut() {
            *idx = old_to_new[*idx as usize];
        }

        self.vertices = new_vertices;
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
    // The single read surface: every voxel/field read below goes through
    // this, so emission code never holds a bare `&Cluster` and can't take
    // a coarse-local sample (see `FieldReader` — MESH-3). `neighbors` is
    // still read directly for *neighbor-LOD config* (the adjacency assert
    // and `per_face_stride`), which are not field reads.
    let reader = FieldReader::new(cluster, neighbors);
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

    // Per-face boundary stride: at each face that has a neighbor,
    // `2^min(self_lod, neighbor_lod)` — the finer of the two strides.
    // When `per_face_stride[f] == stride`, the override is a no-op for
    // that face; when it's *less than* `stride`, self is the coarser
    // side and its boundary layer on that face must iterate at the
    // smaller stride so both sides hit the seam at the same cadence.
    // Face index: `2*axis + (positive as usize)` → 0:−X, 1:+X, 2:−Y,
    // 3:+Y, 4:−Z, 5:+Z.
    let per_face_stride: [i32; 6] = {
        let mut s = [stride; 6];
        for axis in 0..3 {
            for positive in [false, true] {
                if let Some((_, nl)) = neighbor_at(neighbors, axis, positive) {
                    let face_idx = 2 * axis + (positive as usize);
                    s[face_idx] = 1i32 << lod.level().min(nl.level());
                }
            }
        }
        s
    };

    for gz in 0..sample_dim {
        for gy in 0..sample_dim {
            for gx in 0..sample_dim {
                let g_sample = [gx, gy, gz];
                let g_voxel = sample_to_voxel(g_sample, stride);
                let s_g = reader.solid_in_range(g_voxel);

                for axis_a in 0..3 {
                    // Defer to the boundary pass when this edge lives in
                    // an override-active face row. -side faces defer
                    // their in-axis (tangent) edges; +side faces defer
                    // all 3 axes (the seam-crossing one too).
                    if main_loop_defers(&per_face_stride, stride, g_sample, axis_a) {
                        continue;
                    }
                    let mut n_sample = g_sample;
                    n_sample[axis_a] += 1;
                    let n_voxel = sample_to_voxel(n_sample, stride);

                    let s_n = if n_sample[axis_a] < sample_dim {
                        reader.solid_in_range(n_voxel)
                    } else if has_face_neighbor(neighbors, axis_a, true) {
                        reader.solid(n_voxel)
                    } else {
                        // +neighbor is OOB and there is no face neighbor:
                        // a true world edge — leave it open (do NOT read,
                        // which would return empty and spuriously close it).
                        continue;
                    };
                    if s_g == s_n {
                        continue;
                    }

                    let cells_voxel = [
                        sample_to_voxel(cell_coord(g_sample, axis_a, 0, 0), stride),
                        sample_to_voxel(cell_coord(g_sample, axis_a, -1, 0), stride),
                        sample_to_voxel(cell_coord(g_sample, axis_a, -1, -1), stride),
                        sample_to_voxel(cell_coord(g_sample, axis_a, 0, -1), stride),
                    ];
                    let Some(positions) = resolve_cell_vertices(&reader, &cells_voxel, stride)
                    else {
                        continue;
                    };

                    let mut expected_normal = [0.0_f32; 3];
                    expected_normal[axis_a] = if s_g { 1.0 } else { -1.0 };

                    let solid_endpoint = if s_g { g_voxel } else { n_voxel };
                    let material = reader.material(solid_endpoint);

                    push_quad(&mut out, positions, expected_normal, material);
                }
            }
        }
    }

    // Boundary pass: only −side overrides need one. +side overrides
    // would emit only degenerates (see `main_loop_defers` doc), so the
    // main loop's coarse emissions at +side boundary rows are the
    // emissions there.
    for axis in 0..3 {
        let face_idx = 2 * axis; // −side
        let s_o = per_face_stride[face_idx];
        if s_o >= stride {
            continue;
        }
        emit_boundary_face(
            &mut out,
            &reader,
            axis,
            false,
            s_o,
            stride,
            &per_face_stride,
        );
    }

    // Weld byte-equal positions so adjacent triangles share vertex
    // indices and the rasterizer can't drift them apart.
    out.weld();
    out
}

/// True iff the main-loop emission at `g_sample` along `axis_a` should
/// be deferred to the boundary pass.
///
/// Only −side overrides defer: on a −side row (g[face_axis] == 0) the
/// in-axis tangent edges (`axis_a != face_axis`) defer to the boundary
/// pass, which iterates at the finer override stride and gathers cells
/// across the seam into the finer neighbour. The interior axis_a ==
/// face_axis edge at g=0 goes from voxel 0 to voxel stride (inward) —
/// it's not a seam edge, so the main loop emits it as usual.
///
/// +side overrides defer **nothing**: the in-axis tangent and the
/// perpendicular seam-crossing emissions at +side boundary rows pull
/// no cells across the seam (both `g.x+0` and `g.x-1` land inside self),
/// so at fine cadence they all snap to the same self LOD-cell and
/// collapse to degenerate emissions. Letting the main loop emit at
/// self.stride is what provides the connecting edges between adjacent
/// LOD-cell vertices that the finer neighbour's cross-cluster reads
/// pair with; the +side boundary pass would add nothing useful.
fn main_loop_defers(
    per_face_stride: &[i32; 6],
    stride: i32,
    g_sample: [i32; 3],
    axis_a: usize,
) -> bool {
    for face_axis in 0..3 {
        let neg = 2 * face_axis;
        if per_face_stride[neg] < stride && g_sample[face_axis] == 0 && axis_a != face_axis {
            return true;
        }
    }
    false
}

/// Emit the boundary row for one face at the override stride.
///
/// Iteration walks the two in-face axes at `s_o` (the override stride
/// — the finer of self's and the face neighbor's strides). −side rows
/// emit the two in-axis (tangent) edges; +side rows emit all 3 axis
/// directions (tangents + the perpendicular seam-crossing edge).
///
/// `self_stride` is the cluster's own LOD stride and is passed straight
/// through to [`resolve_cell_vertices`] for in-self corner decode: the
/// finer iteration cadence walks voxel positions inside coarse cell
/// footprints, but those voxels' per-voxel corner data was contoured
/// so it decodes with `self_stride` to the containing cell's QEF.
///
/// Ownership at overlap lines: at grid points where this face's row
/// meets another override-active face's row (the cluster's edges and
/// corners), the face with the **lower face_axis index** owns the
/// emission. This avoids the same edge being emitted by two boundary
/// passes (which would over-share). `per_face_stride` is consulted so
/// the rule only suppresses against faces that are actually overriding.
fn emit_boundary_face(
    out: &mut ClusterMesh,
    reader: &FieldReader,
    face_axis: usize,
    positive: bool,
    s_o: i32,
    self_stride: i32,
    per_face_stride: &[i32; 6],
) {
    let n_o = (CLUSTER_DIM as i32) / s_o;
    let face_idx_in_self = if positive { n_o - 1 } else { 0 };
    let b = (face_axis + 1) % 3;
    let c = (face_axis + 2) % 3;

    for j_c in 0..n_o {
        for j_b in 0..n_o {
            let mut g_sample = [0i32; 3];
            g_sample[face_axis] = face_idx_in_self;
            g_sample[b] = j_b;
            g_sample[c] = j_c;
            let g_voxel = sample_to_voxel(g_sample, s_o);

            // Lower-face_axis-wins ownership at overlap lines: if this
            // position also sits on a boundary row for some face whose
            // axis is < `face_axis` (and whose override is active),
            // that face owns the emission — skip here.
            if lower_face_owns(per_face_stride, self_stride, face_axis, g_voxel) {
                continue;
            }

            let s_g = reader.solid(g_voxel);

            for axis_a in 0..3 {
                // -side rows only emit tangent (in-axis) edges; +side
                // rows also emit the perpendicular seam-crossing edge.
                if !positive && axis_a == face_axis {
                    continue;
                }
                let mut n_sample = g_sample;
                n_sample[axis_a] += 1;
                let n_voxel = sample_to_voxel(n_sample, s_o);

                let s_n = if n_sample[axis_a] < n_o {
                    reader.solid(n_voxel)
                } else if reader.has_face_neighbor(axis_a, true) {
                    reader.solid(n_voxel)
                } else {
                    // True world edge (no neighbor): leave it open.
                    continue;
                };
                if s_g == s_n {
                    continue;
                }

                let cells_voxel = [
                    sample_to_voxel(cell_coord(g_sample, axis_a, 0, 0), s_o),
                    sample_to_voxel(cell_coord(g_sample, axis_a, -1, 0), s_o),
                    sample_to_voxel(cell_coord(g_sample, axis_a, -1, -1), s_o),
                    sample_to_voxel(cell_coord(g_sample, axis_a, 0, -1), s_o),
                ];
                let Some(positions) = resolve_cell_vertices(reader, &cells_voxel, self_stride)
                else {
                    continue;
                };

                let mut expected_normal = [0.0_f32; 3];
                expected_normal[axis_a] = if s_g { 1.0 } else { -1.0 };

                let solid_endpoint = if s_g { g_voxel } else { n_voxel };
                let material = reader.material(solid_endpoint);
                push_quad(out, positions, expected_normal, material);
            }
        }
    }
}

/// At the overlap lines where two override-active face rows meet, the
/// face with the lower axis index owns the emission. Returns `true`
/// when `g_voxel` lies on the boundary row of some face whose axis is
/// strictly less than `own_face_axis` and that face is currently
/// overriding — i.e. this position is somebody else's responsibility.
fn lower_face_owns(
    per_face_stride: &[i32; 6],
    self_stride: i32,
    own_face_axis: usize,
    g_voxel: [i32; 3],
) -> bool {
    let dim = CLUSTER_DIM as i32;
    for other_axis in 0..own_face_axis {
        let s_neg = per_face_stride[2 * other_axis];
        let s_pos = per_face_stride[2 * other_axis + 1];
        if s_neg < self_stride && g_voxel[other_axis] == 0 {
            return true;
        }
        if s_pos < self_stride && g_voxel[other_axis] == dim - s_pos {
            return true;
        }
    }
    false
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

/// Resolve the 4-cell gather (cells in cluster-local voxel coords) to
/// 4 vertex positions, all reads going through `reader`. A cell in
/// `[0, CLUSTER_DIM)` on every axis reads self's storage; one with a
/// single coord OOB is routed to the matching face neighbor and its
/// stored corner is decoded into self's frame. Two-or-three-axis OOB
/// returns `None` (the quad is skipped). `self_stride` is the in-self
/// decode stride (the cluster's own LOD stride), independent of how
/// the caller's iteration walks voxel space.
fn resolve_cell_vertices(
    reader: &FieldReader,
    cells_voxel: &[[i32; 3]; 4],
    self_stride: i32,
) -> Option<[[f32; 3]; 4]> {
    let mut out = [[0.0_f32; 3]; 4];
    for (i, &cv) in cells_voxel.iter().enumerate() {
        out[i] = reader.cell_vertex(cv, self_stride)?;
    }
    Some(out)
}

/// Append one oriented quad (4 vertices, 6 indices). The quad's face
/// normal is the geometric normal of triangle `(v0,v1,v2)`; if it points
/// opposite to `expected_normal`, the vertex order is reversed so the
/// emitted face faces the solid→empty direction.
///
/// Degenerate-collapse: if two adjacent perimeter vertices are byte-
/// equal, the quad would split into a zero-area triangle and a real
/// one. Drop the duplicate vertex and emit a single fan triangle.
fn push_quad(
    out: &mut ClusterMesh,
    positions: [[f32; 3]; 4],
    expected_normal: [f32; 3],
    material: u32,
) {
    // Adjacent-duplicate scan. Perimeter is 0→1→2→3→0; if any (i, i+1)
    // are equal, that vertex collapses with its successor.
    let dup_after: Option<usize> = (0..4).find(|&i| positions[i] == positions[(i + 1) % 4]);

    if let Some(i) = dup_after {
        // Drop the second of the duplicate pair, keep the remaining
        // three in perimeter order so the winding is preserved.
        let drop = (i + 1) % 4;
        let mut tri = [[0.0_f32; 3]; 3];
        let mut k = 0;
        for j in 0..4 {
            if j == drop {
                continue;
            }
            tri[k] = positions[j];
            k += 1;
        }
        // Multi-duplicate (e.g. two pairs of equal positions) means the
        // input was already a line or a point — nothing renderable.
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[2] == tri[0] {
            return;
        }
        push_triangle(out, tri, expected_normal, material);
        return;
    }

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

/// Append one oriented triangle (3 vertices, 3 indices). Same winding-
/// flip rule as [`push_quad`]: if the geometric normal of (v0,v1,v2)
/// disagrees with `expected_normal`, swap two vertices.
fn push_triangle(
    out: &mut ClusterMesh,
    positions: [[f32; 3]; 3],
    expected_normal: [f32; 3],
    material: u32,
) {
    let v0 = positions[0];
    let v1 = positions[1];
    let v2 = positions[2];

    let raw_normal = cross(sub(v1, v0), sub(v2, v0));
    let mut face_normal = normalize(raw_normal);

    let ordered = if dot(face_normal, expected_normal) < 0.0 {
        face_normal = [-face_normal[0], -face_normal[1], -face_normal[2]];
        [v0, v2, v1]
    } else {
        [v0, v1, v2]
    };

    let base = out.vertices.len() as u32;
    for p in ordered {
        out.vertices.push(ClusterVertex {
            position: p,
            normal: face_normal,
            material,
        });
    }
    out.indices.extend_from_slice(&[base, base + 1, base + 2]);
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

#[inline]
fn has_face_neighbor(neighbors: &NeighborContext<'_>, axis: usize, positive: bool) -> bool {
    neighbor_at(neighbors, axis, positive).is_some()
}

/// `true` iff `coord` has exactly one OOB axis AND the matching face
/// neighbor is present. (Caller is expected to have already filtered
/// out multi-axis OOB.)
fn single_axis_face_neighbor_present(neighbors: &NeighborContext<'_>, coord: [i32; 3]) -> bool {
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
    use crate::material::Material;
    use crate::primitive::{FlatField, Hermite, Primitive};
    use crate::voxel::Voxel;
    use crate::voxel_state::VoxelState;

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
        // After the weld pass, vertices are no longer in per-quad
        // groups — indices reference welded slots. Indices still come
        // in triangle triples (6 per quad), so `% 3 == 0` is the
        // surviving structural invariant.
        assert_eq!(m.indices.len() % 3, 0);
        for i in &m.indices {
            assert!((*i as usize) < m.vertices.len());
        }
        for v in &m.vertices {
            let len =
                (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
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
                cluster.set(c, Voxel::new(VoxelState::Solid, corner, mat));
            }
        }

        let m = mesh(&cluster, &NeighborContext::none(), Lod::ZERO);
        assert!(!m.is_empty());

        // Post-weld, vertex normals are averages of every adjacent
        // face's normal (including the side walls), so no single
        // vertex carries a pure +Y normal anymore. The check shifts to
        // triangle *geometric* normals — at least one triangle should
        // have all three vertices on the y=128 plane and a face normal
        // pointing +Y, which is the top of the patch.
        let mut found = false;
        for tri in m.indices.chunks_exact(3) {
            let p0 = m.vertices[tri[0] as usize].position;
            let p1 = m.vertices[tri[1] as usize].position;
            let p2 = m.vertices[tri[2] as usize].position;
            let on_plane = (p0[1] - 128.0).abs() < 0.01
                && (p1[1] - 128.0).abs() < 0.01
                && (p2[1] - 128.0).abs() < 0.01;
            if !on_plane {
                continue;
            }
            let n = normalize(cross(sub(p1, p0), sub(p2, p0)));
            if n[1].abs() > 0.99 {
                found = true;
                break;
            }
        }
        assert!(found, "no triangle on y=128 plane with +Y face normal");
    }

    /// Number of triangles in a mesh whose all 3 vertices land in the
    /// world-X plane at `wx ± tol`. Used to find seam-plane geometry.
    fn tris_on_x_plane(m: &ClusterMesh, wx: f32, tol: f32) -> usize {
        m.indices
            .chunks_exact(3)
            .filter(|t| {
                t.iter()
                    .all(|i| (m.vertices[*i as usize].position[0] - wx).abs() < tol)
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
        let b_world = [CLUSTER_DIM as f32 + b_local[0], b_local[1], b_local[2]];
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
        let expected_perimeter =
            2 * (CLUSTER_DIM as usize - 1) + 2 * (3 * CLUSTER_DIM as usize - 1);
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
        let mut lookup: std::collections::HashMap<[i64; 3], u32> = std::collections::HashMap::new();
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

    /// Sum unshared (count==1) and over-shared (count>2) edges across
    /// every per-cluster mesh's [`ClusterMesh::edge_use_histogram`].
    /// Per-mesh totals; cross-cluster seam edges are unshared by this
    /// metric even when the corresponding edge in the neighbour mesh
    /// matches up byte-for-byte. See [`combined_edge_use_totals`] for
    /// the across-cluster welded count.
    fn edge_use_totals(meshes: &[ClusterMesh]) -> (usize, usize) {
        let mut unshared = 0_usize;
        let mut over = 0_usize;
        for m in meshes {
            let hist = m.edge_use_histogram();
            for (_, uses) in hist {
                match uses {
                    0 | 2 => {}
                    1 => unshared += 1,
                    _ => over += 1,
                }
            }
        }
        (unshared, over)
    }

    /// Cross-cluster welded edge histogram: each cluster's mesh is
    /// translated by its world offset, vertices are deduped by quantized
    /// world position across all clusters, and edges are counted on the
    /// combined triangle list. This is the real watertightness check —
    /// unshared here means a genuine gap or world-boundary edge, not a
    /// per-mesh artifact of where the seam emission lives.
    fn combined_edge_use_totals(ids: &[ClusterId], meshes: &[ClusterMesh]) -> (usize, usize) {
        use std::collections::HashMap;
        let q = |p: [f32; 3]| -> [i64; 3] {
            [
                (p[0] * 1024.0).round() as i64,
                (p[1] * 1024.0).round() as i64,
                (p[2] * 1024.0).round() as i64,
            ]
        };
        let mut verts_q: Vec<[i64; 3]> = Vec::new();
        let mut lookup: HashMap<[i64; 3], u32> = HashMap::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for (i, m) in meshes.iter().enumerate() {
            let off = ids[i].world_offset();
            for tri in m.indices.chunks_exact(3) {
                let mut idx = [0u32; 3];
                for (k, &vi) in tri.iter().enumerate() {
                    let p = m.vertices[vi as usize].position;
                    let world = [p[0] + off[0], p[1] + off[1], p[2] + off[2]];
                    let key = q(world);
                    idx[k] = *lookup.entry(key).or_insert_with(|| {
                        let new_id = verts_q.len() as u32;
                        verts_q.push(key);
                        new_id
                    });
                }
                triangles.push(idx);
            }
        }
        let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
        for t in &triangles {
            for &(a, b) in &[(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_use.entry(key).or_insert(0) += 1;
            }
        }
        let mut unshared = 0_usize;
        let mut over = 0_usize;
        for (_, &uses) in &edge_use {
            match uses {
                0 | 2 => {}
                1 => unshared += 1,
                _ => over += 1,
            }
        }
        (unshared, over)
    }

    /// Diagnostic: 3×3 XZ field of FlatField clusters with the centre
    /// cluster optionally at LOD 1 (all four lateral LOD-0 neighbours
    /// running the per-face boundary stride override). The watertight
    /// check is on the combined mesh (welded across cluster boundaries):
    /// over-shared must be 0, and unshared must equal the world
    /// perimeter — the same closure invariant the §1 row-of-three test
    /// established for uniform LOD.
    #[test]
    fn flat_field_3x3_centre_lod1_seam_counts() {
        fn build(center_lod: u8) -> (Vec<ClusterId>, Vec<ClusterMesh>) {
            let lod_for = |x: u16, z: u16| if x == 1 && z == 1 { center_lod } else { 0 };
            let ids: Vec<ClusterId> = (0..3u16)
                .flat_map(|x| (0..3u16).map(move |z| ClusterId::new(lod_for(x, z), x, 0, z)))
                .collect();
            let clusters: Vec<Cluster> = ids
                .iter()
                .map(|id| contour(&FlatField::at_half(), grey(), *id))
                .collect();
            let meshes: Vec<ClusterMesh> = (0..ids.len())
                .map(|i| {
                    let id = ids[i];
                    let x = id.x();
                    let z = id.z();
                    let nb = |xx: u16, zz: u16| -> Option<(&Cluster, Lod)> {
                        if xx >= 3 || zz >= 3 {
                            return None;
                        }
                        let idx = (xx as usize) * 3 + (zz as usize);
                        Some((
                            &clusters[idx],
                            Lod::new(lod_for(xx, zz)).expect("valid lod"),
                        ))
                    };
                    let neg_x = if x > 0 { nb(x - 1, z) } else { None };
                    let pos_x = if x < 2 { nb(x + 1, z) } else { None };
                    let neg_z = if z > 0 { nb(x, z - 1) } else { None };
                    let pos_z = if z < 2 { nb(x, z + 1) } else { None };
                    let neighbors = NeighborContext {
                        neg_x,
                        pos_x,
                        neg_z,
                        pos_z,
                        ..NeighborContext::none()
                    };
                    mesh(
                        &clusters[i],
                        &neighbors,
                        Lod::new(id.lod()).expect("valid lod"),
                    )
                })
                .collect();
            (ids, meshes)
        }

        let (ids0, meshes0) = build(0);
        let (ids1, meshes1) = build(1);

        let (pu0, po0) = edge_use_totals(&meshes0);
        let (pu1, po1) = edge_use_totals(&meshes1);
        let (cu0, co0) = combined_edge_use_totals(&ids0, &meshes0);
        let (cu1, co1) = combined_edge_use_totals(&ids1, &meshes1);
        eprintln!(
            "flat 3x3 uniform LOD 0:   per-mesh unshared={pu0}/over={po0}, combined unshared={cu0}/over={co0}"
        );
        eprintln!(
            "flat 3x3 centre at LOD 1: per-mesh unshared={pu1}/over={po1}, combined unshared={cu1}/over={co1}"
        );

        // Per-mesh and combined over-shared must be 0 in every case —
        // an edge in >2 triangles is always a bug.
        assert_eq!(po0, 0, "uniform LOD 0 per-mesh over-shared");
        assert_eq!(po1, 0, "centre LOD 1 per-mesh over-shared");
        assert_eq!(co0, 0, "uniform LOD 0 combined over-shared");
        assert_eq!(co1, 0, "centre LOD 1 combined over-shared");

        // Combined unshared = world-boundary perimeter. With the
        // override working end-to-end the centre-LOD-1 case is within
        // a small handful of edges of the uniform-LOD-0 baseline; the
        // residual delta is the cluster-corner artifact from
        // `cell_vertex` returning `None` on multi-axis OOB, which
        // affects four 3-cluster-meet corners around the centre.
        let delta = (cu1 as i64 - cu0 as i64).abs();
        assert!(
            delta <= 16,
            "centre LOD 1 combined unshared {cu1} differs from baseline {cu0} by {delta} (>16 = real seam gap)"
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
        let pos = FieldReader::new(&a, &neighbors).cell_vertex([256, 126, 0], 1);
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
        let v_a = FieldReader::new(&a, &NeighborContext::none())
            .cell_vertex([255, 127, 100], 1)
            .expect("in range");
        // From B's side: cell at sample (-1, 127, 100) — read across.
        let v_b = FieldReader::new(&b, &nb_b)
            .cell_vertex([-1, 127, 100], 1)
            .expect("read across");

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
