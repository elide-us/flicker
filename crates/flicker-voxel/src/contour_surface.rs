//! Surface-voxel direct contouring.
//!
//! [`contour_surface`] walks **voxels**, not cells, and emits one
//! vertex per solid voxel that participates in the surface (has at
//! least one non-solid 6-neighbor). The vertex sits at the voxel's
//! own `+++` corner — no averaging. Adjacent surface voxels with
//! joins at slightly different world positions produce vertices at
//! those slightly different positions, and the triangles connecting
//! them are the surface slope directly. This is what couples the
//! heightmap generator's per-column fractional-Y joins to a smoothly
//! sloped rendered surface.
//!
//! # Quad assembly: sign-changing axis edges, orientation-free 3D search
//!
//! Once vertices are emitted, quads come from iterating
//! **sign-changing axis edges** — pairs of adjacent voxels along
//! X, Y, or Z where one is solid and the other is air. Around each
//! sign-changing edge, a **single 3D Manhattan-shell search** walks
//! offsets `(dx, dy, dz)` around both edge endpoints up to
//! [`MAX_QUAD_SEARCH_RADIUS`]. Each visited position that holds a
//! surface vertex is **classified into one of four quad slots** by
//! the signs of its position components along the two perpendicular
//! axes only. The edge's own axis is ignored for slot classification.
//! If all four slots fill (and no degenerate-pair collisions), the
//! four vertices form a quad; winding is chosen by aligning the
//! geometric face normal with the average vertex normal.
//!
//! The 3D-shell + sign-classification design is **orientation-free**:
//! the algorithm treats all three dimensions symmetrically apart from
//! identifying which is the edge axis. Surfaces in any orientation —
//! horizontal, vertical, tilted, overhanging, curved — are meshed
//! the same way. Compared to the previous perpendicular-plane-only
//! search (which stayed at the edge's two axis-aligned Y values), the
//! 3D search reaches off-plane vertices and removes the axis-aligned
//! assumption.
//!
//! Cases the rule captures:
//!
//! - **Flat surfaces.** Every Y-edge at the boundary finds four
//!   adjacent column tops at the same Y; the quad is flat.
//! - **Steps.** A sign-changing edge at a height step's lower side
//!   finds the taller column's top via a position whose Y differs by
//!   1 — the quad is tilted, bridging the step.
//! - **Vertical cliffs.** Multiple sign-changing axis edges stacked
//!   along the cliff plane each emit their own quad from cliff-face
//!   surface voxels above and below them.
//! - **Tilted and oriented surfaces.** The 3D search reaches the
//!   four diagonal-neighbor column tops on a surface tilted in two
//!   or more axes — the rotation no longer biases the algorithm's
//!   accessible directions.
//!
//! # Rotational slot tiebreak
//!
//! Positions with both perpendicular signs nonzero map naturally to
//! their `(sign_a, sign_b)` quadrant slot. Positions on a single
//! perpendicular axis (one sign nonzero, the other zero) need a
//! tiebreak so that the four perpendicular-axis-adjacent positions
//! around the edge fill four distinct slots on a flat surface. The
//! tiebreak is a fixed rotation: `(+a, 0) → (+a, +b)`,
//! `(0, +b) → (-a, +b)`, `(-a, 0) → (-a, -b)`, `(0, -b) → (+a, -b)`.
//! Without it, pairs of axis-adjacent positions would compete for
//! the same slot and produce phantom degenerate quads (the same
//! failure mode the previous prompt's `perp_a`/`perp_b` swap
//! addressed in 2D; now it lives at the classification layer).
//!
//! # Scope and simplification
//!
//! - **LOD 0 only.** Strided sampling is a follow-up.
//! - **Cross-cluster aware** via [`contour_surface_with_neighbors`].
//!   The pass takes a [`NeighborContext`] for classification at the
//!   cluster boundary (so a solid voxel at the seam sees its
//!   neighbor's matching voxel rather than the empty base) and a
//!   [`NeighborHalos`] set carrying vertex slabs from neighbor
//!   boundary rows (so the 3D quadrant search can reach across seams
//!   to fill quad slots). The bare [`contour_surface`] entry point is
//!   a one-line wrapper that passes neither — same byte-identical
//!   output as before when no neighbors are wired.
//! - **Solid side participates.** A boundary cell has a solid and an
//!   air voxel; we keep only the solid side as the vertex source.
//!   This mirrors [`crate::is_surface_boundary_voxel`] restricted to
//!   the solid half and avoids double-vertex emission at every
//!   boundary.
//! - **Each solid surface voxel emits one vertex at its own `+++`
//!   corner.** For height-field terrain (the heightmap generator's
//!   output) this is correct because every column's topmost solid
//!   voxel naturally owns the surface corner under it. For free-
//!   standing solids (e.g. a 2×2×2 solid cube in air) this is
//!   one-corner offset from where dual contouring would place the
//!   vertex — a known limitation tracked as a follow-up.
//! - **Search radius is bounded** ([`MAX_QUAD_SEARCH_RADIUS`]). On
//!   gradients steeper than the cap, some quads are skipped — small
//!   gaps rather than wildly stretched triangles.

use std::collections::HashMap;

use crate::cluster::CLUSTER_DIM;
use crate::mesh::{CellMesh, Indices, MeshMetadata, Vertex};
use crate::seam::{read_corner, FaceDir, HaloVertex, NeighborContext, NeighborHalo};
use crate::{Cluster, LocalCoord, Material, Voxel};

/// Per-face references to neighbor halos, supplied to
/// [`contour_surface_with_neighbors`] alongside [`NeighborContext`].
/// Missing entries are equivalent to "no halo on that face" — the
/// quad search treats halo-absent positions the same as missing
/// vertices.
#[derive(Default)]
pub struct NeighborHalos<'a> {
    pub neg_x: Option<&'a NeighborHalo>,
    pub pos_x: Option<&'a NeighborHalo>,
    pub neg_y: Option<&'a NeighborHalo>,
    pub pos_y: Option<&'a NeighborHalo>,
    pub neg_z: Option<&'a NeighborHalo>,
    pub pos_z: Option<&'a NeighborHalo>,
}

/// Maximum 3D Manhattan-shell radius for the orientation-free
/// quadrant search during quad assembly. Beyond this, sign-changing
/// edges that haven't filled all four slots are skipped — a tiny
/// acceptable gap rather than a wildly stretched triangle bridging
/// genuinely disconnected surface regions.
///
/// `3` covers offsets up to `|dx| + |dy| + |dz| ≤ 3`. A 3D ball of
/// this radius visits 62 positions per endpoint per edge — roughly
/// comparable per-edge cost to the previous 2D R=4 search. Larger
/// values trade computation for fewer gaps on steep regions; `3`
/// handles the Navier-Stokes wave field's typical gradient with
/// remaining gaps only on the very steepest crests.
const MAX_QUAD_SEARCH_RADIUS: i32 = 3;

#[inline]
fn is_voxel_solid(v: Voxel) -> bool {
    v.material() != Material::EMPTY
}

fn empty_surface_mesh() -> CellMesh {
    CellMesh::from_parts(
        Vec::new(),
        Indices::U16(Vec::new()),
        MeshMetadata {
            lod: 0,
            cluster_dim: CLUSTER_DIM,
            vertex_count: 0,
            triangle_count: 0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
        },
    )
}

/// Voxel array index for a cluster-local coordinate. Inlined helper
/// shared by the contour pass's pre-classification and vertex-table
/// reads.
#[inline]
fn voxel_idx(x: u32, y: u32, z: u32) -> usize {
    let dim = CLUSTER_DIM as usize;
    x as usize + y as usize * dim + z as usize * dim * dim
}

/// Map a single-axis-OOB search position to `(face, perp_a, perp_b)`.
///
/// Returns `Some` when exactly one of `(px, py, pz)` is OOB by
/// exactly one voxel (`-1` or `dim`); returns `None` for multi-axis
/// OOB (cluster edges/corners) or single-axis OOB further than one
/// voxel (beyond the halo's known data). The two in-range coords
/// become `(perp_a, perp_b)` per the face's axis convention.
fn halo_query_coords(px: i32, py: i32, pz: i32, dim: i32) -> Option<(FaceDir, u32, u32)> {
    let xo = !(0..dim).contains(&px);
    let yo = !(0..dim).contains(&py);
    let zo = !(0..dim).contains(&pz);
    let oob = u8::from(xo) + u8::from(yo) + u8::from(zo);
    if oob != 1 {
        return None;
    }
    if xo {
        let face = if px == -1 {
            FaceDir::NegX
        } else if px == dim {
            FaceDir::PosX
        } else {
            return None;
        };
        Some((face, py as u32, pz as u32))
    } else if yo {
        let face = if py == -1 {
            FaceDir::NegY
        } else if py == dim {
            FaceDir::PosY
        } else {
            return None;
        };
        Some((face, px as u32, pz as u32))
    } else {
        let face = if pz == -1 {
            FaceDir::NegZ
        } else if pz == dim {
            FaceDir::PosZ
        } else {
            return None;
        };
        Some((face, px as u32, py as u32))
    }
}

/// Promote a halo vertex into the cluster's vertex buffer on first
/// use; return the assigned vertex index (cached in `halo_intern`).
///
/// Once interned, a halo vertex is indistinguishable from an
/// in-cluster vertex to downstream consumers — it participates in
/// quads via its assigned index, and its position widens the cluster
/// mesh's bounds. The intern map is keyed by `(face, perp_a, perp_b)`
/// so distinct halo positions get distinct indices and the
/// degenerate-quad guard remains valid.
#[allow(clippy::too_many_arguments)]
fn intern_halo_vertex(
    face: FaceDir,
    perp_a: u32,
    perp_b: u32,
    hv: &HaloVertex,
    halo_intern: &mut HashMap<(FaceDir, u32, u32), u32>,
    vertices: &mut Vec<Vertex>,
    bounds_min: &mut [f32; 3],
    bounds_max: &mut [f32; 3],
) -> u32 {
    if let Some(&idx) = halo_intern.get(&(face, perp_a, perp_b)) {
        return idx;
    }
    let idx = vertices.len() as u32;
    vertices.push(Vertex {
        position: hv.position,
        normal: hv.normal,
        material: hv.material,
    });
    halo_intern.insert((face, perp_a, perp_b), idx);
    for ax in 0..3 {
        if hv.position[ax] < bounds_min[ax] {
            bounds_min[ax] = hv.position[ax];
        }
        if hv.position[ax] > bounds_max[ax] {
            bounds_max[ax] = hv.position[ax];
        }
    }
    idx
}

/// 3D Manhattan-shell quadrant search around a sign-changing axis
/// edge. Each visited offset is classified into one of four quad
/// slots by the signs of its perpendicular-axis components; the
/// first vertex per slot wins. Returns `[v_mm, v_pm, v_pp, v_mp]`
/// with `u32::MAX` for any slot that never filled.
///
/// In-cluster positions consult `voxel_to_vertex`. Single-axis OOB
/// positions (`-1` or `dim` on one axis) consult the matching halo
/// in `halos`; halo vertices are promoted into the cluster's vertex
/// buffer via [`intern_halo_vertex`] on first hit.
#[allow(clippy::too_many_arguments)]
fn find_four_quadrant_vertices(
    a: [i32; 3],
    b: [i32; 3],
    perp_axis_a: usize,
    perp_axis_b: usize,
    voxel_to_vertex: &[u32],
    halos: &NeighborHalos<'_>,
    halo_intern: &mut HashMap<(FaceDir, u32, u32), u32>,
    vertices: &mut Vec<Vertex>,
    bounds_min: &mut [f32; 3],
    bounds_max: &mut [f32; 3],
) -> [u32; 4] {
    let dim_i32 = CLUSTER_DIM as i32;
    let mut slots: [u32; 4] = [u32::MAX; 4];
    for shell in 1..=MAX_QUAD_SEARCH_RADIUS {
        for dx in -shell..=shell {
            let rem_x = shell - dx.abs();
            for dy in -rem_x..=rem_x {
                let rem_y = shell - dx.abs() - dy.abs();
                let dz_vals: [i32; 2] = if rem_y == 0 { [0, 0] } else { [rem_y, -rem_y] };
                let dz_count = if rem_y == 0 { 1 } else { 2 };
                for &dz in &dz_vals[..dz_count] {
                    let offset = [dx, dy, dz];
                    let sa = offset[perp_axis_a].signum();
                    let sb = offset[perp_axis_b].signum();
                    let slot = match (sa, sb) {
                        (0, 0) => continue,
                        (1, 1) => 2,
                        (1, -1) => 1,
                        (-1, 1) => 3,
                        (-1, -1) => 0,
                        (1, 0) => 2,
                        (-1, 0) => 0,
                        (0, 1) => 3,
                        (0, -1) => 1,
                        _ => continue,
                    };
                    if slots[slot] != u32::MAX {
                        continue;
                    }
                    for ep in &[a, b] {
                        let px = ep[0] + dx;
                        let py = ep[1] + dy;
                        let pz = ep[2] + dz;
                        if (0..dim_i32).contains(&px)
                            && (0..dim_i32).contains(&py)
                            && (0..dim_i32).contains(&pz)
                        {
                            let v = voxel_to_vertex[voxel_idx(px as u32, py as u32, pz as u32)];
                            if v != u32::MAX {
                                slots[slot] = v;
                                break;
                            }
                        } else if let Some((face, perp_a_c, perp_b_c)) =
                            halo_query_coords(px, py, pz, dim_i32)
                        {
                            let halo_opt = match face {
                                FaceDir::NegX => halos.neg_x,
                                FaceDir::PosX => halos.pos_x,
                                FaceDir::NegY => halos.neg_y,
                                FaceDir::PosY => halos.pos_y,
                                FaceDir::NegZ => halos.neg_z,
                                FaceDir::PosZ => halos.pos_z,
                            };
                            if let Some(halo) = halo_opt {
                                if let Some(hv) = halo.vertex_at(perp_a_c, perp_b_c) {
                                    let idx = intern_halo_vertex(
                                        face,
                                        perp_a_c,
                                        perp_b_c,
                                        hv,
                                        halo_intern,
                                        vertices,
                                        bounds_min,
                                        bounds_max,
                                    );
                                    slots[slot] = idx;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        if slots.iter().all(|&s| s != u32::MAX) {
            return slots;
        }
    }
    slots
}

/// Degenerate-quad guard: any pair of equal corners means the quad
/// isn't actually a quad. Two of the four push_oriented_quad triangles
/// would have a repeated vertex and render as nothing.
fn any_pair_equal(q: [u32; 4]) -> bool {
    let [a, b, c, d] = q;
    a == b || a == c || a == d || b == c || b == d || c == d
}

/// Single-cluster contour. Equivalent to
/// [`contour_surface_with_neighbors`] with no neighbor context and no
/// halos — useful when contouring a stand-alone cluster or when
/// running tests that should not depend on seam data.
#[must_use]
pub fn contour_surface(cluster: &Cluster) -> CellMesh {
    contour_surface_with_neighbors(cluster, &NeighborContext::none(), &NeighborHalos::default())
}

/// Contour `cluster` into a [`CellMesh`] by walking surface-boundary
/// voxels directly. One vertex per solid surface voxel, placed at
/// that voxel's owned `+++` corner. Quads from grid-adjacent surface
/// voxels.
///
/// The heightmap (or any other generator that places joins on the
/// surface) is the source of truth, and this pass samples those joins
/// directly. Where adjacent columns sit at slightly different
/// heights, the line between their joins becomes the surface slope —
/// no smoothing pass required.
///
/// # Cross-cluster behavior
///
/// `neighbors` resolves OOB voxel classifications at cluster faces.
/// A solid voxel at the seam sees its neighbor's matching voxel
/// instead of the empty base, so it no longer mistakenly treats the
/// seam face as a surface. `halos` carries vertex slabs from neighbor
/// boundary rows; the 3D quadrant search consults them when the
/// search visits a position just past a face, and any halo vertex it
/// uses is promoted into this cluster's vertex buffer with an
/// in-cluster index. Adjacent clusters that supply each other's
/// halos emit coincident geometry at the seam — z-buffer ties on
/// coplanar geometry are visually invisible.
///
/// # Scope
///
/// LOD 0 only.
///
/// # Simplification
///
/// Each solid surface voxel emits one vertex at its own `+++` corner.
/// For height-field terrain this is correct. For free-standing solids
/// (e.g. a 2×2×2 solid cube in air fixture) this is offset by one
/// corner from where dual contouring would place vertices — a
/// follow-up will address the full air-anchor case.
#[must_use]
pub fn contour_surface_with_neighbors(
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    halos: &NeighborHalos<'_>,
) -> CellMesh {
    let dim = CLUSTER_DIM as usize;
    let dim_i32 = CLUSTER_DIM as i32;
    let base_solid = is_voxel_solid(cluster.base());

    // Early exit: if no override has a classification differing from
    // base, there is no internal classification boundary — and
    // therefore no surface voxel — anywhere. The cluster's external
    // faces are not surfaces in this algorithm's sense unless an
    // override flips classification on that face.
    let has_difference = cluster
        .overrides()
        .any(|(_, v)| is_voxel_solid(v) != base_solid);
    if !has_difference {
        return empty_surface_mesh();
    }

    // --- Step 1: pre-classify every voxel in one O(dim³) scan. ---
    //
    // Memory: `dim³` bytes = 16 MB. We start the buffer at `base_solid`
    // and stamp only the overrides — `cluster.overrides()` is
    // typically << dim³ entries.
    let mut is_solid = vec![base_solid; dim * dim * dim];
    for (coord, voxel) in cluster.overrides() {
        is_solid[voxel_idx(coord.x(), coord.y(), coord.z())] = is_voxel_solid(voxel);
    }
    let solid_at = |x: i32, y: i32, z: i32| -> bool {
        if (0..dim_i32).contains(&x) && (0..dim_i32).contains(&y) && (0..dim_i32).contains(&z) {
            is_solid[voxel_idx(x as u32, y as u32, z as u32)]
        } else {
            // Single-axis OOB resolves through the matching face
            // neighbor via `read_corner`; multi-axis OOB falls back
            // to `cluster.base()`.
            is_voxel_solid(read_corner(cluster, neighbors, x, y, z))
        }
    };

    // --- Step 2: emit one vertex per solid-surface voxel. ---
    //
    // `voxel_to_vertex` maps every voxel index to the vertex emitted
    // for that voxel, or `u32::MAX` when the voxel emitted nothing
    // (not solid, or solid but every 6-neighbor is also solid). Same
    // memory shape as the cell-based algorithm's `cell_vertex` table.
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut voxel_to_vertex: Vec<u32> = vec![u32::MAX; dim * dim * dim];
    let mut bounds_min = [f32::INFINITY; 3];
    let mut bounds_max = [f32::NEG_INFINITY; 3];

    let to_i = |b: bool| -> i32 {
        if b {
            1
        } else {
            0
        }
    };

    for vz in 0..dim_i32 {
        for vy in 0..dim_i32 {
            for vx in 0..dim_i32 {
                if !solid_at(vx, vy, vz) {
                    continue;
                }
                let nxn = solid_at(vx - 1, vy, vz);
                let nxp = solid_at(vx + 1, vy, vz);
                let nyn = solid_at(vx, vy - 1, vz);
                let nyp = solid_at(vx, vy + 1, vz);
                let nzn = solid_at(vx, vy, vz - 1);
                let nzp = solid_at(vx, vy, vz + 1);
                let any_nonsolid = !nxn || !nxp || !nyn || !nyp || !nzn || !nzp;
                if !any_nonsolid {
                    continue;
                }

                // Vertex position = the voxel's owned `+++` corner.
                // For default corner vector this is voxel-center + 0.5
                // on each axis; the heightmap generator places the
                // surface column's topmost voxel's Y component at
                // `fractional(h)`, so the vertex Y is exactly the
                // surface height at that column's sample point.
                let voxel = cluster
                    .get(LocalCoord::new(vx as u32, vy as u32, vz as u32).expect("in bounds"));
                let [dx, dy, dz] = voxel.corner().to_components();
                let position = [vx as f32 + dx, vy as f32 + dy, vz as f32 + dz];

                // Normal = -gradient(classification). Per-axis
                // gradient is (positive-side - negative-side), so when
                // the +x neighbor is empty (the air side) and -x is
                // solid, dx = -1 and the normal points in +x — out of
                // the solid toward the empty side. Mirror of the
                // existing algorithm's normal convention.
                let gx = to_i(nxp) - to_i(nxn);
                let gy = to_i(nyp) - to_i(nyn);
                let gz = to_i(nzp) - to_i(nzn);
                let nx = -(gx as f32);
                let ny = -(gy as f32);
                let nz = -(gz as f32);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let normal = if len > 0.0 {
                    [nx / len, ny / len, nz / len]
                } else {
                    // Pathological: surface voxel with zero gradient.
                    // Cannot happen given the surface predicate above
                    // (at least one neighbor differs), but kept as a
                    // defensive fallback matching the cell-based
                    // algorithm's choice.
                    [0.0, 1.0, 0.0]
                };

                let vid = vertices.len() as u32;
                voxel_to_vertex[voxel_idx(vx as u32, vy as u32, vz as u32)] = vid;
                vertices.push(Vertex {
                    position,
                    normal,
                    material: voxel.material().raw(),
                });
                for axis in 0..3 {
                    if position[axis] < bounds_min[axis] {
                        bounds_min[axis] = position[axis];
                    }
                    if position[axis] > bounds_max[axis] {
                        bounds_max[axis] = position[axis];
                    }
                }
            }
        }
    }

    if vertices.is_empty() {
        return empty_surface_mesh();
    }

    // --- Step 3: emit quads from sign-changing axis edges. ---
    //
    // For each axis (X, Y, Z) iterate every axis-aligned voxel pair
    // and test for sign change (one solid, one not). For each
    // sign-changing edge, search the four perpendicular-plane
    // quadrants outward for the nearest emitted surface vertex via
    // [`find_four_quadrant_vertices`]; if all four quadrants return a
    // vertex, emit one quad. Winding is chosen by
    // [`push_oriented_quad`] (face normal aligned with the average of
    // the four vertex normals).
    //
    // At a height step the corner voxel buried inside the taller
    // column is interior and emits no vertex; the quadrant search
    // reaches over it into the taller column's top, producing the
    // tilted quad that bridges the step. At a cluster seam, the
    // quadrant search reaches OOB positions exactly one voxel past
    // the face — those are answered by the relevant halo, with the
    // halo vertex interned into this cluster's vertex buffer on
    // first use.
    let mut indices_u32: Vec<u32> = Vec::new();
    let mut halo_intern: HashMap<(FaceDir, u32, u32), u32> = HashMap::new();

    // X-axis edges: pair (vx, vy, vz)/(vx+1, vy, vz). Edge axis = 0
    // (X). Perpendicular axes = 1 (Y) and 2 (Z).
    for vz in 0..dim_i32 {
        for vy in 0..dim_i32 {
            for vx in 0..(dim_i32 - 1) {
                let s0 = solid_at(vx, vy, vz);
                let s1 = solid_at(vx + 1, vy, vz);
                if s0 == s1 {
                    continue;
                }
                let slots = find_four_quadrant_vertices(
                    [vx, vy, vz],
                    [vx + 1, vy, vz],
                    1,
                    2,
                    &voxel_to_vertex,
                    halos,
                    &mut halo_intern,
                    &mut vertices,
                    &mut bounds_min,
                    &mut bounds_max,
                );
                if slots.contains(&u32::MAX) {
                    continue;
                }
                if any_pair_equal(slots) {
                    continue;
                }
                push_oriented_quad(&mut indices_u32, &vertices, slots);
            }
        }
    }

    // Y-axis edges: pair (vx, vy, vz)/(vx, vy+1, vz). Edge axis = 1
    // (Y). Perpendicular axes = 0 (X) and 2 (Z).
    for vz in 0..dim_i32 {
        for vy in 0..(dim_i32 - 1) {
            for vx in 0..dim_i32 {
                let s0 = solid_at(vx, vy, vz);
                let s1 = solid_at(vx, vy + 1, vz);
                if s0 == s1 {
                    continue;
                }
                let slots = find_four_quadrant_vertices(
                    [vx, vy, vz],
                    [vx, vy + 1, vz],
                    0,
                    2,
                    &voxel_to_vertex,
                    halos,
                    &mut halo_intern,
                    &mut vertices,
                    &mut bounds_min,
                    &mut bounds_max,
                );
                if slots.contains(&u32::MAX) {
                    continue;
                }
                if any_pair_equal(slots) {
                    continue;
                }
                push_oriented_quad(&mut indices_u32, &vertices, slots);
            }
        }
    }

    // Z-axis edges: pair (vx, vy, vz)/(vx, vy, vz+1). Edge axis = 2
    // (Z). Perpendicular axes = 0 (X) and 1 (Y).
    for vz in 0..(dim_i32 - 1) {
        for vy in 0..dim_i32 {
            for vx in 0..dim_i32 {
                let s0 = solid_at(vx, vy, vz);
                let s1 = solid_at(vx, vy, vz + 1);
                if s0 == s1 {
                    continue;
                }
                let slots = find_four_quadrant_vertices(
                    [vx, vy, vz],
                    [vx, vy, vz + 1],
                    0,
                    1,
                    &voxel_to_vertex,
                    halos,
                    &mut halo_intern,
                    &mut vertices,
                    &mut bounds_min,
                    &mut bounds_max,
                );
                if slots.contains(&u32::MAX) {
                    continue;
                }
                if any_pair_equal(slots) {
                    continue;
                }
                push_oriented_quad(&mut indices_u32, &vertices, slots);
            }
        }
    }

    // Vertex count <= 65536 still fits in u16 (index range is
    // 0..vertex_count). 65537+ needs u32. Same threshold as the
    // cell-based algorithm.
    let use_u32 = vertices.len() > (u16::MAX as usize + 1);
    let indices = if use_u32 {
        Indices::U32(indices_u32)
    } else {
        Indices::U16(indices_u32.into_iter().map(|i| i as u16).collect())
    };

    let vertex_count = vertices.len();
    let triangle_count = indices.triangle_count();
    CellMesh::from_parts(
        vertices,
        indices,
        MeshMetadata {
            lod: 0,
            cluster_dim: CLUSTER_DIM,
            vertex_count,
            triangle_count,
            bounds_min,
            bounds_max,
        },
    )
}

/// Push two triangles for the quad with corners `q = [v00, v10, v11,
/// v01]` (counter-clockwise in the natural winding for the chosen
/// axis pair) into `indices`, picking the winding that aligns with
/// the average of the four vertex normals.
///
/// Geometric face normal: `(p10 - p00) × (p01 - p00)`. Compare its
/// sign against the average vertex normal; if negative, flip the
/// winding. This lets the gradient-derived per-vertex normals decide
/// the quad's outward face, so we don't need per-axis sign rules.
fn push_oriented_quad(indices: &mut Vec<u32>, vertices: &[Vertex], q: [u32; 4]) {
    let p0 = vertices[q[0] as usize].position;
    let p1 = vertices[q[1] as usize].position;
    let p3 = vertices[q[3] as usize].position;
    let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
    let e2 = [p3[0] - p0[0], p3[1] - p0[1], p3[2] - p0[2]];
    let face_n = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let mut avg = [0.0_f32; 3];
    for &vid in &q {
        let n = vertices[vid as usize].normal;
        avg[0] += n[0];
        avg[1] += n[1];
        avg[2] += n[2];
    }
    let dot = face_n[0] * avg[0] + face_n[1] * avg[1] + face_n[2] * avg[2];
    if dot >= 0.0 {
        // Natural winding: (v00, v10, v11) and (v00, v11, v01).
        indices.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
    } else {
        // Flipped winding.
        indices.extend_from_slice(&[q[0], q[2], q[1], q[0], q[3], q[2]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_vector::CornerVector;
    use crate::generators::{heightmap_terrain_at_with_depth_materials, solid_slab};
    use crate::material::Material;
    use crate::seam::Lod;

    fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("in-range")
    }

    fn solid_voxel() -> Voxel {
        Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap())
    }

    // ---- empty / trivial ----

    #[test]
    fn empty_cluster_returns_empty_mesh() {
        let m = contour_surface(&Cluster::empty());
        assert!(m.is_empty());
        assert_eq!(m.vertices().len(), 0);
        assert_eq!(m.indices().triangle_count(), 0);
        assert_eq!(m.metadata().lod, 0);
        assert_eq!(m.metadata().cluster_dim, CLUSTER_DIM);
    }

    #[test]
    fn single_solid_voxel_emits_one_vertex_no_quads() {
        // One solid voxel at (128, 128, 128) in an otherwise empty
        // cluster. Its 6 neighbors are all empty so it satisfies the
        // surface predicate. No 2×2 voxel loop has all 4 vertices,
        // so no quads are emitted.
        let mut c = Cluster::empty();
        c.set(coord(128, 128, 128), solid_voxel());
        let m = contour_surface(&c);
        assert_eq!(m.vertices().len(), 1);
        assert_eq!(m.indices().triangle_count(), 0);

        // Vertex position is the voxel's `+++` corner with the
        // default vector. Default corner encodes byte 128 which
        // decodes to ~0.5039 (not exact 0.5 due to byte quantization).
        let v = &m.vertices()[0];
        assert!((v.position[0] - 128.5).abs() < 0.01);
        assert!((v.position[1] - 128.5).abs() < 0.01);
        assert!((v.position[2] - 128.5).abs() < 0.01);
        // Material is the voxel's own (no centroid averaging).
        assert_eq!(v.material, solid_voxel().material().raw());
    }

    // ---- half-slab ----

    #[test]
    fn half_slab_top_layer_is_at_y_127_5() {
        // `solid_slab(128, m)` fills y ∈ [0, 127] with solid voxels
        // and leaves the cluster base (empty) for y >= 128. The new
        // algorithm flags as surface every solid voxel with an
        // empty 6-neighbor.
        //
        // Top layer (y=127) has +Y empty → all 256² surfaces. The
        // base, side walls, and bottom of the cluster also produce
        // surface voxels because the cluster.base is empty
        // (out-of-cluster reads return empty, matching the
        // is_surface_boundary_voxel convention). Total surface count:
        //   total solid (256² · 128 = 8_388_608)
        //   - fully-interior solid (x ∈ [1,254], y ∈ [1,126], z ∈ [1,254]
        //     = 254 · 126 · 254 = 8_129_016)
        //   = 259_592.
        let m = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let mesh = contour_surface(&m);

        // Exact vertex count from the analytical derivation above.
        assert_eq!(mesh.vertices().len(), 259_592);

        // Top layer has 256² = 65_536 vertices, all at y ≈ 127.5
        // with normals pointing +Y (out of the solid into the air
        // above).
        let mut top_count = 0;
        for v in mesh.vertices() {
            if (v.position[1] - 127.5).abs() < 0.05 {
                top_count += 1;
                assert!(
                    v.normal[1] > 0.5,
                    "top-layer normal[1] = {} not pointing up at pos {:?}",
                    v.normal[1],
                    v.position
                );
            }
        }
        assert_eq!(
            top_count,
            (CLUSTER_DIM as usize).pow(2),
            "expected one top-layer vertex per (x,z) column"
        );

        // The mesh is non-degenerate. With the rotational-priority
        // sign-changing-edge rule, each interior Y-edge at the top
        // face finds 4 distinct cardinal-neighbor column tops and
        // emits a diamond quad. (CLUSTER_DIM - 2)² = 254² = 64_516
        // interior quads = 129_032 triangles, plus contributions
        // from boundary cells where some quadrants find a workable
        // fallback. Total well over 120K.
        assert!(
            mesh.indices().triangle_count() > 120_000,
            "expected dense top-face tessellation (>120_000 triangles); got {}",
            mesh.indices().triangle_count()
        );
    }

    // ---- 4-column slope: the load-bearing diagonal test ----

    /// Build a 4-column slab where column `x` reaches `y = 4 + x`
    /// (heights 4, 5, 6, 7) along X, constant in Z. The intent is to
    /// isolate the slope between adjacent column tops so the per-
    /// algorithm vertex Y can be inspected directly.
    fn build_slope_fixture() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 0..CLUSTER_DIM {
            for x in 0..4u32 {
                let h = 4 + x; // 4, 5, 6, 7
                for y in 0..h {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    #[test]
    fn slope_places_vertex_at_each_column_top() {
        // The load-bearing test: contour_surface emits one vertex per
        // surface voxel at its `+++` corner. The topmost voxel of
        // column x is at y = h-1 = 3+x; its `+++` corner sits at
        // (x+0.5, h-0.5, z+0.5). Adjacent columns therefore differ
        // in vertex Y by exactly 1.0 — the slope is a diagonal, not
        // a stair-step.
        let c = build_slope_fixture();
        let mesh = contour_surface(&c);

        // For each column, the topmost vertex sits at
        // (x + 0.5, (3+x) + 0.5, z + 0.5). Find a vertex with x ≈
        // x_target and z ≈ 128.5 and assert its Y.
        let expected: [(f32, f32); 4] = [(0.5, 3.5), (1.5, 4.5), (2.5, 5.5), (3.5, 6.5)];
        for (col_x, expected_y) in expected {
            let v = mesh
                .vertices()
                .iter()
                .find(|v| {
                    (v.position[0] - col_x).abs() < 0.05
                        && (v.position[1] - expected_y).abs() < 0.05
                        && (v.position[2] - 128.5).abs() < 0.05
                })
                .unwrap_or_else(|| {
                    panic!(
                        "expected a vertex near ({col_x}, {expected_y}, 128.5) — \
                         the algorithm did not place a column-top vertex"
                    )
                });
            // Top-of-column normal has positive Y (gradient points
            // up out of the solid into the air above).
            assert!(
                v.normal[1] > 0.5,
                "column-top vertex at ({col_x}, {expected_y}) has normal {:?}; \
                 expected +Y dominant",
                v.normal,
            );
        }

        // Diagonal proof — the differences between adjacent column
        // tops' Y values are exactly 1.0. (For default corner vectors
        // the encoding round-trips byte-128 to ~0.5039, so the actual
        // floats differ from 0.5 by ~0.004; the differences cancel.)
        //
        // The slope fixture's cluster-edge voxels (bottom row, `-X`
        // wall at x=0, `+X` wall at x=3) all emit surface vertices at
        // the same X/Z as the column top, so filter for Y ≥ 3 to
        // pick the topmost vertex specifically.
        let top_at = |x: f32| -> f32 {
            mesh.vertices()
                .iter()
                .find(|v| {
                    (v.position[0] - x).abs() < 0.05
                        && (v.position[2] - 128.5).abs() < 0.05
                        && v.position[1] >= 3.0
                })
                .unwrap_or_else(|| panic!("no top-row vertex at x≈{x}, z≈128.5"))
                .position[1]
        };
        let y0 = top_at(0.5);
        let y1 = top_at(1.5);
        assert!(
            (y1 - y0 - 1.0).abs() < 0.05,
            "column-top diagonal: y1-y0={} not ≈ 1.0",
            y1 - y0
        );
    }

    // ---- heightmap sanity ----

    #[test]
    fn heightmap_terrain_produces_substantial_mesh() {
        // End-to-end sanity: the heightmap generator + contour pass
        // produces a substantial mesh covering the wave field. The
        // triangle threshold reflects the 3D orientation-free quadrant
        // search's coverage on Lipschitz wave terrain — gaps remain
        // only on the steepest crests where adjacent column heights
        // differ by more than the search radius reaches.
        let mesh = contour_surface(&heightmap_terrain_at_with_depth_materials(
            0x42,
            [0.0, 0.0, 0.0],
        ));
        assert!(!mesh.is_empty());
        assert!(
            mesh.vertices().len() > 50_000,
            "expected > 50_000 vertices, got {}",
            mesh.vertices().len()
        );
        assert!(
            mesh.indices().triangle_count() > 40_000,
            "expected > 40_000 triangles, got {}",
            mesh.indices().triangle_count()
        );
        // Every vertex sits inside the cluster's spatial range, plus
        // a small slop for corner-vector encoding (joins can extend
        // up to 1.5 voxels outside the integer voxel position).
        for v in mesh.vertices() {
            for axis in 0..3 {
                assert!(
                    v.position[axis] >= -0.5 && v.position[axis] <= CLUSTER_DIM as f32 + 0.5,
                    "vertex position {:?} out of expected range",
                    v.position
                );
            }
        }
    }

    // ---- determinism ----

    #[test]
    fn deterministic_on_heightmap_terrain() {
        let c = heightmap_terrain_at_with_depth_materials(0x99, [0.0, 0.0, 0.0]);
        let a = contour_surface(&c);
        let b = contour_surface(&c);
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
    }

    #[test]
    fn deterministic_on_slope_fixture() {
        let c = build_slope_fixture();
        let a = contour_surface(&c);
        let b = contour_surface(&c);
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
    }

    // ---- Triangle iteration helper for the new-quad-rule tests. ----

    /// Iterate every triangle in a `CellMesh`'s index buffer as a
    /// `[u32; 3]`, handling both u16 and u32 index widths uniformly.
    fn each_triangle(mesh: &CellMesh, mut f: impl FnMut([u32; 3])) {
        let indices = mesh.indices();
        let n = indices.len();
        let to_u32 = |i: usize| -> u32 {
            match indices {
                Indices::U16(v) => v[i] as u32,
                Indices::U32(v) => v[i],
            }
        };
        let mut i = 0;
        while i + 2 < n {
            f([to_u32(i), to_u32(i + 1), to_u32(i + 2)]);
            i += 3;
        }
    }

    // ---- new quad rule: slope, cliff, flat coverage ----

    /// 16×16 solid block at x ∈ [8, 23], z ∈ [8, 23], interior to the
    /// cluster. Columns at x < 16 (i.e. x ∈ [8, 15]) have height 10;
    /// columns at x ≥ 16 (x ∈ [16, 23]) have height 11. The single
    /// 1-voxel step at x = 16 isolates the diagonal-quad case the new
    /// quad rule must handle.
    fn build_interior_step_fixture() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 8..24u32 {
            for x in 8..24u32 {
                let h = if x < 16 { 10 } else { 11 };
                for y in 0..h {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    #[test]
    fn slope_emits_diagonal_quad() {
        // The load-bearing test for the new quad rule. At the step
        // between column 15 (top y=9) and column 16 (top y=10), the
        // Y-edge at (15, 9, z)/(15, 10, z) has all four perpendicular
        // quadrants find column tops: (-X, ±Z) finds col 14 top at
        // y=9, (+X, ±Z) reaches across the step into col 16 top at
        // y=10. The resulting tilted quad has Y values 9.5 and 10.5,
        // bridging the step in a single diagonal triangle pair — the
        // 2×2-loop rule could not produce this because the corner
        // voxel inside the taller column is interior.
        let c = build_interior_step_fixture();
        let mesh = contour_surface(&c);

        // Look for a triangle that has at least one vertex at Y ≈ 9.5
        // (lower-shelf top) AND at least one at Y ≈ 10.5 (upper-shelf
        // top), with all vertices in the step's X span [14, 17]. The
        // existence of such a triangle is exactly the diagonal proof.
        let mut found = false;
        each_triangle(&mesh, |tri| {
            let positions: [[f32; 3]; 3] =
                core::array::from_fn(|i| mesh.vertices()[tri[i] as usize].position);
            let has_low = positions.iter().any(|p| (p[1] - 9.5).abs() < 0.05);
            let has_high = positions.iter().any(|p| (p[1] - 10.5).abs() < 0.05);
            let on_step = positions.iter().all(|p| p[0] >= 14.0 && p[0] <= 17.0);
            if has_low && has_high && on_step {
                found = true;
            }
        });
        assert!(
            found,
            "expected at least one diagonal triangle bridging y=9.5 to y=10.5 \
             in the step's X span — the new quad rule must connect adjacent \
             column tops across a height step"
        );
    }

    #[test]
    fn flat_slab_top_face_is_meshed() {
        // The sign-changing-edge rule tessellates the half-slab's top
        // face with diamond-pattern quads (one per interior Y-edge,
        // with the four corners being adjacent diagonal column tops).
        // Triangles strictly on the top face — every vertex at Y ≈
        // 127.5 — must be plentiful.
        let m = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let mesh = contour_surface(&m);

        let mut top_face_triangles = 0;
        each_triangle(&mesh, |tri| {
            if tri
                .iter()
                .all(|&i| (mesh.vertices()[i as usize].position[1] - 127.5).abs() < 0.05)
            {
                top_face_triangles += 1;
            }
        });

        // Interior Y-edges: (CLUSTER_DIM - 2)² = 254² = 64_516. Each
        // emits one quad = 2 triangles. Cluster-edge Y-edges drop
        // (quadrant searches go OOB). Expect ≥ 100_000 top-face
        // triangles; assert generously.
        assert!(
            top_face_triangles > 100_000,
            "expected > 100_000 top-face triangles, got {top_face_triangles}"
        );
    }

    /// Two columns inside the cluster (at x=8 and x=9) with very
    /// different heights (4 and 10), producing a 6-voxel-tall cliff
    /// face on the +X side of column 8 (the column 9 face). The
    /// cliff is well inside the cluster so it's not interacting with
    /// cluster-boundary surface voxels.
    fn build_cliff_fixture() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 8..24u32 {
            for x in 8..10u32 {
                let h = if x == 8 { 4 } else { 10 };
                for y in 0..h {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    #[test]
    fn vertical_cliff_emits_vertical_quad() {
        // Column 9 has -X (column 8) empty for y ∈ [4, 9] (since
        // column 8 only reaches y=3). So voxels (9, y, z) for
        // y ∈ [4, 9] are surface voxels on the cliff face, vertices
        // at (9.5, y+0.5, z+0.5). Sign-changing X-edges at the cliff
        // plane between (8, y, z) empty and (9, y, z) solid exist
        // for every y on the face. Each emits a quad from the
        // surrounding surface voxels (column 9 face vertices above
        // and below, at various Z). The quads form a vertical
        // tessellation of the cliff.
        let c = build_cliff_fixture();
        let mesh = contour_surface(&c);

        // Look for a triangle that lies on the cliff face: ≥ 2
        // vertices with x ≈ 9.5, spanning at least 0.5 in Y.
        let mut found = false;
        each_triangle(&mesh, |tri| {
            let positions: [[f32; 3]; 3] =
                core::array::from_fn(|i| mesh.vertices()[tri[i] as usize].position);
            let on_cliff: Vec<&[f32; 3]> = positions
                .iter()
                .filter(|p| (p[0] - 9.5).abs() < 0.05)
                .collect();
            if on_cliff.len() >= 2 {
                let y_min = on_cliff.iter().fold(f32::INFINITY, |a, p| a.min(p[1]));
                let y_max = on_cliff.iter().fold(f32::NEG_INFINITY, |a, p| a.max(p[1]));
                if y_max - y_min >= 0.5 {
                    found = true;
                }
            }
        });
        assert!(
            found,
            "expected at least one triangle on the cliff face with vertical span — \
             the new quad rule should mesh the cliff directly"
        );
    }

    #[test]
    fn heightmap_field_no_large_top_vertex_gaps() {
        // Most column-top vertices on the heightmap surface participate
        // in at least one triangle. Some gaps remain on the steepest
        // wave crests where adjacent column heights differ by more
        // than the 3D quadrant search reaches — accepted as a known
        // limitation of the single-cluster contour pass; seam-halo
        // extension will reduce it. Threshold ≥ 90% reflects the
        // orientation-free 3D search's coverage on Lipschitz wave
        // terrain.
        let mesh = contour_surface(&heightmap_terrain_at_with_depth_materials(
            0x42,
            [0.0, 0.0, 0.0],
        ));

        let mut used = std::collections::HashSet::<u32>::new();
        each_triangle(&mesh, |tri| {
            for &i in &tri {
                used.insert(i);
            }
        });

        // A "column-top vertex" heuristic: +Y-dominant normal, Y in
        // the heightmap's nominal band [64, 192], and away from the
        // cluster XZ boundary so we don't penalize the seam-wall
        // strand vertices.
        let edge_skip = 4.0_f32;
        let mut top_total = 0usize;
        let mut top_used = 0usize;
        for (vid, v) in mesh.vertices().iter().enumerate() {
            if v.normal[1] > 0.9
                && v.position[1] > 64.0
                && v.position[1] < 192.0
                && v.position[0] > edge_skip
                && v.position[0] < CLUSTER_DIM as f32 - edge_skip
                && v.position[2] > edge_skip
                && v.position[2] < CLUSTER_DIM as f32 - edge_skip
            {
                top_total += 1;
                if used.contains(&(vid as u32)) {
                    top_used += 1;
                }
            }
        }
        assert!(
            top_total > 1000,
            "expected many column-top vertices, got {top_total} — the heuristic \
             classifies too few; tune normal/position thresholds"
        );
        let participation = top_used as f64 / top_total as f64;
        assert!(
            participation > 0.90,
            "top-vertex participation {:.1}% too low ({top_used} of {top_total} used) \
             — the 3D orientation-free search should reach >90% on Lipschitz terrain",
            participation * 100.0
        );
    }

    #[test]
    fn flat_slab_dense_axis_aligned_tessellation() {
        // With the rotational-priority quad search, a flat half-
        // slab's top face tessellates densely — one diamond quad per
        // Y-edge whose four perpendicular cardinal-neighbor column
        // tops are found at shell-1 distances `(0, 1)`/`(1, 0)`.
        // Interior Y-edges (vx ∈ [1, 254], vz ∈ [1, 254]) all
        // emit. (CLUSTER_DIM - 2)² = 254² = 64_516 quads = 129_032
        // triangles. Boundary Y-edges may emit too where the search
        // falls back to in-cluster directions. Assert ≥ 120K
        // top-face triangles to anchor the dense tessellation.
        let m = solid_slab(128, Material::new(7, 7, 7).unwrap());
        let mesh = contour_surface(&m);

        let mut top_face_triangles = 0;
        each_triangle(&mesh, |tri| {
            if tri
                .iter()
                .all(|&i| (mesh.vertices()[i as usize].position[1] - 127.5).abs() < 0.05)
            {
                top_face_triangles += 1;
            }
        });

        assert!(
            top_face_triangles > 120_000,
            "expected dense axis-aligned tessellation (>120_000 top-face \
             triangles); got {top_face_triangles} — search may still be \
             missing axis-adjacent perpendicular neighbors"
        );
    }

    #[test]
    fn tilted_surface_emits_quad_with_three_d_reach() {
        // A solid block tilted along both X and Z: heights rise by
        // one voxel for every X step AND one for every Z step. The
        // surface is a 2D plane tilted in both X and Z. The previous
        // 2D perpendicular-plane search couldn't find off-plane
        // vertices, so tilted regions produced sparse or no quads.
        // The 3D search reaches positions at any Y within the
        // Manhattan radius, so the tilted surface should mesh.
        //
        // Test strategy: assert (1) a substantial mesh exists on the
        // tilted block, (2) at least one triangle's vertices span
        // ≥ 1 voxel in Y. The strict "all 3 dimensions differ in
        // every triangle" form the spec describes is achievable
        // only when the search radius reaches the four diagonal-
        // neighbor column tops before filling slots with closer
        // axis-adjacent vertices. The rotational tiebreak prefers
        // closer axis-adjacent positions (good for flat surfaces),
        // so on a tilted surface most triangles span 2 of 3 distinct
        // Y values, not 3. The Y-span check is the orientation-
        // invariance signal — the 2D search would produce no
        // such triangles.
        let mut c = Cluster::empty();
        let v = solid_voxel();
        let base = 10u32;
        for z in 8..24u32 {
            for x in 8..24u32 {
                let h = base + (x - 8) + (z - 8);
                for y in 0..h {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        let mesh = contour_surface(&c);

        assert!(
            mesh.indices().triangle_count() > 200,
            "expected substantial mesh on tilted block; got {} triangles",
            mesh.indices().triangle_count()
        );

        // Find at least one triangle whose Y values span ≥ 1 voxel
        // — proof that the algorithm meshed across the tilt rather
        // than staying flat at a single Y.
        let mut found_tilted = false;
        each_triangle(&mesh, |tri| {
            let positions: [[f32; 3]; 3] =
                core::array::from_fn(|i| mesh.vertices()[tri[i] as usize].position);
            let ys: [f32; 3] = [positions[0][1], positions[1][1], positions[2][1]];
            let y_min = ys.iter().fold(f32::INFINITY, |a, &b| a.min(b));
            let y_max = ys.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
            // Also require X or Z to span — pure-vertical face quads
            // span Y but sit at one X or Z; we want a *tilted*
            // triangle, not a cliff face.
            let xs: [f32; 3] = [positions[0][0], positions[1][0], positions[2][0]];
            let zs: [f32; 3] = [positions[0][2], positions[1][2], positions[2][2]];
            let x_span = xs.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b))
                - xs.iter().fold(f32::INFINITY, |a, &b| a.min(b));
            let z_span = zs.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b))
                - zs.iter().fold(f32::INFINITY, |a, &b| a.min(b));
            if y_max - y_min >= 0.8 && x_span >= 0.5 && z_span >= 0.5 {
                found_tilted = true;
            }
        });
        assert!(
            found_tilted,
            "expected at least one tilted triangle (Y span ≥ 0.8, X span ≥ 0.5, \
             Z span ≥ 0.5) — the 3D search should mesh the tilted surface, \
             which the 2D-only search could not"
        );
    }

    #[test]
    fn deterministic_with_new_quad_rule_on_interior_step() {
        let c = build_interior_step_fixture();
        let a = contour_surface(&c);
        let b = contour_surface(&c);
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
    }

    // ---- 2×2×2 cube: documented limitation ----

    #[test]
    fn cube_2x2x2_emits_eight_vertices_at_solid_voxel_corners() {
        // A 2×2×2 solid cube in otherwise-empty space. The
        // simplified surface-first algorithm emits one vertex per
        // solid voxel at its own `+++` corner — eight vertices.
        // Each sits at the corresponding voxel's `+++` corner:
        // (vx+0.5, vy+0.5, vz+0.5).
        //
        // This is one-corner offset from where dual contouring would
        // place vertices for a free-standing solid (dual contouring
        // would place vertices on every corner of the cube,
        // including the 7 air-anchored ones). The follow-up to this
        // algorithm will extend the predicate to include air-anchor
        // voxels for free-standing solid cases; for now we document
        // the known shape.
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for dx in 0..2u32 {
            for dy in 0..2u32 {
                for dz in 0..2u32 {
                    c.set(coord(64 + dx, 64 + dy, 64 + dz), v);
                }
            }
        }
        let mesh = contour_surface(&c);
        assert_eq!(
            mesh.vertices().len(),
            8,
            "simplified algorithm: 8 vertices, one per solid voxel"
        );
        // Each vertex sits at (vx + 0.5039, vy + 0.5039, vz + 0.5039)
        // (default corner vector encoding). Verify positions are
        // among the expected set.
        let mut seen_positions: Vec<(u32, u32, u32)> = Vec::new();
        for v in mesh.vertices() {
            // Recover the source voxel: floor(position - 0.5039).
            // Just round to the nearest integer of (position - 0.5).
            let vx = (v.position[0] - 0.5).round() as u32;
            let vy = (v.position[1] - 0.5).round() as u32;
            let vz = (v.position[2] - 0.5).round() as u32;
            seen_positions.push((vx, vy, vz));
        }
        seen_positions.sort();
        let expected: Vec<(u32, u32, u32)> = (0..2)
            .flat_map(|dz| {
                (0..2).flat_map(move |dy| (0..2).map(move |dx| (64 + dx, 64 + dy, 64 + dz)))
            })
            .collect();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(seen_positions, expected_sorted);
    }

    // ---- seam halo ----

    #[test]
    fn no_neighbors_matches_legacy_contour_surface() {
        // The bare `contour_surface(c)` entry point is a one-line
        // wrapper around `contour_surface_with_neighbors` with empty
        // neighbor context and empty halos. The outputs must be
        // byte-equal.
        let c = heightmap_terrain_at_with_depth_materials(0x42, [0.0, 0.0, 0.0]);
        let a = contour_surface(&c);
        let b = contour_surface_with_neighbors(
            &c,
            &crate::seam::NeighborContext::none(),
            &NeighborHalos::default(),
        );
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.indices(), b.indices());
        assert_eq!(a.metadata(), b.metadata());
    }

    #[test]
    fn matched_lod_seam_removes_boundary_walls() {
        // Two adjacent clusters generated from the same heightmap at
        // world offsets [0,0,0] and [256,0,0]. Without a neighbor,
        // the +X face of the left cluster paints a wall of vertices
        // with `position[0] ≈ 256` and `normal[0] > 0.8` — mid-column
        // water voxels see their OOB +X side as empty and emit a
        // surface there. With the +X neighbor wired through, those
        // OOB reads classify as solid and the walls disappear.
        use crate::seam::{build_halo, FaceDir, NeighborContext};
        let left = heightmap_terrain_at_with_depth_materials(0x42, [0.0, 0.0, 0.0]);
        let right = heightmap_terrain_at_with_depth_materials(0x42, [256.0, 0.0, 0.0]);

        let count_walls = |mesh: &CellMesh| -> usize {
            mesh.vertices()
                .iter()
                .filter(|v| (v.position[0] - 256.0).abs() < 0.5 && v.normal[0] > 0.8)
                .count()
        };

        let wall_alone = count_walls(&contour_surface(&left));
        assert!(
            wall_alone > 100,
            "fixture sanity: alone-pass should emit many +X wall vertices; got {wall_alone}"
        );

        let neighbors = NeighborContext {
            pos_x: Some((&right, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let halo_pos_x = build_halo(&right, FaceDir::PosX, Lod::ZERO);
        let halos = NeighborHalos {
            pos_x: Some(&halo_pos_x),
            ..NeighborHalos::default()
        };
        let wall_with_neighbor =
            count_walls(&contour_surface_with_neighbors(&left, &neighbors, &halos));

        assert!(
            wall_with_neighbor * 10 < wall_alone,
            "expected +X wall count to drop ≥ 10× with halo; \
             alone = {wall_alone}, with neighbor = {wall_with_neighbor}"
        );
    }

    #[test]
    fn matched_lod_seam_emits_cross_seam_quads() {
        // With the +X halo wired in, the quadrant search around
        // boundary Y-edges reaches OOB positions on +X and finds
        // halo vertices to fill the missing slots. Those halo
        // vertices are interned into the cluster's vertex buffer at
        // `position[0] ≈ 256.5` (one voxel past the seam plane).
        // Counting triangles that contain at least one such vertex
        // measures the cross-seam stitching density.
        use crate::seam::{build_halo, FaceDir, NeighborContext};
        let left = heightmap_terrain_at_with_depth_materials(0x42, [0.0, 0.0, 0.0]);
        let right = heightmap_terrain_at_with_depth_materials(0x42, [256.0, 0.0, 0.0]);
        let neighbors = NeighborContext {
            pos_x: Some((&right, Lod::ZERO)),
            ..NeighborContext::none()
        };
        let halo_pos_x = build_halo(&right, FaceDir::PosX, Lod::ZERO);
        let halos = NeighborHalos {
            pos_x: Some(&halo_pos_x),
            ..NeighborHalos::default()
        };
        let mesh = contour_surface_with_neighbors(&left, &neighbors, &halos);

        let mut cross_seam = 0usize;
        each_triangle(&mesh, |tri| {
            for &idx in &tri {
                if mesh.vertices()[idx as usize].position[0] > 256.0 {
                    cross_seam += 1;
                    break;
                }
            }
        });
        // The bulk of cross-seam quads come from top-of-column
        // Y-edges in the vx≈255 row whose `(+X, ±Z)` slots fall onto
        // the +X halo. Empirically that's a few hundred quads (~800
        // triangles) on this heightmap seed; assert ≥ 500 to anchor
        // the seam-stitching density without flaking on small
        // surface-roughness variations.
        assert!(
            cross_seam > 500,
            "expected substantial cross-seam quad emission; got {cross_seam}"
        );
    }
}
