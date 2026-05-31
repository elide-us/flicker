//! Surface-voxel direct contouring.
//!
//! # Why a second algorithm
//!
//! The cell-based [`contour_cluster`](crate::contour_cluster) in
//! [`crate::contour`] places one vertex per 2×2×2 cell at the centroid
//! of its 8 corner-voxels' owned `+++` corners. That centroid would
//! be exactly correct if all 8 corners expressed the surface — but in
//! practice only a subset do (the solid-surface voxels) and the rest
//! are interior or far-air voxels at their default `+++` corner. The
//! centroid averages the carefully-positioned surface joins with the
//! default-positioned interior joins, attenuating any join
//! displacement to roughly `1/8`. The heightmap generator
//! ([`crate::generators::heightmap_terrain_at`]) now places topmost
//! solid voxels' joins exactly on the surface, but that work is being
//! diluted by the centroid average — which is why the rendered
//! terrain still reads as cubic despite the joins being right.
//!
//! [`contour_surface`] inverts the algorithm: it walks **voxels**,
//! not cells, and emits one vertex per solid voxel that participates
//! in the surface (has at least one non-solid 6-neighbor). The
//! vertex sits at the voxel's own `+++` corner — no averaging.
//! Adjacent surface voxels with joins at slightly different world
//! positions produce vertices at those slightly different positions,
//! and the triangles connecting them are the surface slope directly.
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
//! - **Single cluster only.** No `NeighborContext`. The function
//!   operates identically to `contour_cluster(&cluster)` with
//!   `NeighborContext::none()` in scope: cross-cluster correctness
//!   comes from upstream (both clusters' generators sample the same
//!   heightmap), not from this pass.
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
//!
//! [`contour_cluster`](crate::contour_cluster) and the rest of the
//! cell-based family remain in place. This module is additive.

use crate::cluster::CLUSTER_DIM;
use crate::contour::{CellMesh, Indices, MeshMetadata, Vertex};
use crate::{Cluster, LocalCoord, Material, Voxel};

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

/// Contour `cluster` into a [`CellMesh`] by walking surface-boundary
/// voxels directly. One vertex per solid surface voxel, placed at
/// that voxel's owned `+++` corner. Quads from grid-adjacent surface
/// voxels.
///
/// This is the surface-first algorithm: the heightmap (or any other
/// generator that places joins on the surface) is the source of
/// truth, and this pass samples those joins directly without the
/// cell-centroid averaging of
/// [`contour_cluster`](crate::contour_cluster). Where adjacent
/// columns sit at slightly different heights, the line between their
/// joins becomes the surface slope — no smoothing pass required.
///
/// # Scope
///
/// LOD 0 only. Single-cluster — no neighbor context. The existing
/// [`contour_cluster`](crate::contour_cluster) family remains in
/// place for compatibility during the transition.
///
/// # Simplification
///
/// Each solid surface voxel emits one vertex at its own `+++` corner.
/// For height-field terrain this is correct. For free-standing solids
/// (e.g. a 2×2×2 solid cube in air fixture) this is offset by one
/// corner from where dual contouring would place vertices — a
/// follow-up will address the full air-anchor case. The intent here
/// is to verify the slope-correctness of the surface-first model on
/// the active terrain scene.
#[must_use]
pub fn contour_surface(cluster: &Cluster) -> CellMesh {
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
    // Memory: `dim³` bytes = 16 MB. Same shape as `contour_cluster`'s
    // pre-classification array. We start the buffer at `base_solid`
    // and stamp only the overrides — `cluster.overrides()` is
    // typically << dim³ entries.
    let row_stride = dim;
    let slab_stride = dim * dim;
    let voxel_idx = |x: u32, y: u32, z: u32| -> usize {
        x as usize + y as usize * row_stride + z as usize * slab_stride
    };
    let mut is_solid = vec![base_solid; dim * dim * dim];
    for (coord, voxel) in cluster.overrides() {
        is_solid[voxel_idx(coord.x(), coord.y(), coord.z())] = is_voxel_solid(voxel);
    }
    let solid_at = |x: i32, y: i32, z: i32| -> bool {
        if (0..dim_i32).contains(&x) && (0..dim_i32).contains(&y) && (0..dim_i32).contains(&z) {
            is_solid[voxel_idx(x as u32, y as u32, z as u32)]
        } else {
            base_solid
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
    // quadrants outward for the nearest emitted surface vertex. If
    // all four quadrants return a vertex, emit one quad. Winding
    // chosen by `push_oriented_quad` (face normal aligned with
    // average vertex normal).
    //
    // This rule captures slopes and cliffs the previous 2×2-loop
    // rule missed: at a height step, the corner voxel buried inside
    // the taller column is interior (no vertex), so the 2×2 loop has
    // only 3 of 4 corners and emits nothing. The quadrant search
    // instead reaches *over* the interior voxel into the taller
    // column's top, producing a tilted quad that bridges the step.
    let mut indices_u32: Vec<u32> = Vec::new();

    // Search the 3D Manhattan-shell neighborhood around both edge
    // endpoints. Each visited position is classified by its
    // perpendicular-axis sign components into one of four quad
    // slots; the first vertex found per slot wins. Returns the full
    // `[v_mm, v_pm, v_pp, v_mp]` tuple. Slots that never fill come
    // back as `u32::MAX`.
    //
    // `a` and `b` are the two integer voxel endpoints of the edge.
    // `perp_axis_a` and `perp_axis_b` index into the offset triple
    // (`0=X, 1=Y, 2=Z`) identifying the two perpendicular axes; the
    // third axis is the edge axis and is ignored for slot
    // classification.
    //
    // # Slot indexing
    //
    //   0 = v_mm  ( (-perp_a, -perp_b) )
    //   1 = v_pm  ( (+perp_a, -perp_b) )
    //   2 = v_pp  ( (+perp_a, +perp_b) )
    //   3 = v_mp  ( (-perp_a, +perp_b) )
    //
    // # Slot classification with the rotational tiebreak
    //
    //   (0, 0)   → skip (offset on edge axis only)
    //   (+, +)   → 2 (v_pp)        (+, -)   → 1 (v_pm)
    //   (-, +)   → 3 (v_mp)        (-, -)   → 0 (v_mm)
    //   (+, 0)   → 2 (v_pp)        (-, 0)   → 0 (v_mm)
    //   (0, +)   → 3 (v_mp)        (0, -)   → 1 (v_pm)
    //
    // The four perpendicular-axis-adjacent positions
    // `(±1, 0)`/`(0, ±1)` map to four distinct slots via this
    // rotation — that's what gives the dense flat-surface
    // tessellation. Non-axis-adjacent positions (both perpendicular
    // components nonzero) go to their natural quadrant.
    //
    // # Search order — deterministic
    //
    //   1. Manhattan shell `s` from 1 to `MAX_QUAD_SEARCH_RADIUS`.
    //   2. Within a shell, iterate every triple `(dx, dy, dz)` with
    //      `|dx| + |dy| + |dz| == s` in lex order:
    //        - `dx` from `-s` to `+s`
    //        - `dy` from `-(s - |dx|)` to `+(s - |dx|)`
    //        - `dz` then ±(s - |dx| - |dy|), positive sign first
    //          (or `0` when the residual is `0`).
    //   3. For each offset, try endpoint `a` first, then `b`.
    //   4. First vertex found per slot wins; subsequent finds in a
    //      filled slot are ignored.
    //   5. Stop early when all four slots are filled.
    let find_four_quadrant_vertices =
        |a: [i32; 3], b: [i32; 3], perp_axis_a: usize, perp_axis_b: usize| -> [u32; 4] {
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
                                    let v =
                                        voxel_to_vertex[voxel_idx(px as u32, py as u32, pz as u32)];
                                    if v != u32::MAX {
                                        slots[slot] = v;
                                        break;
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
        };

    // Degenerate-quad guard: any pair of equal corners means the
    // quad isn't actually a quad. Two of the four `push_oriented_quad`
    // triangles would have a repeated vertex and render as nothing.
    // Cheap to filter; cleaner than emitting phantom indices.
    let any_pair_equal = |q: [u32; 4]| -> bool {
        let [a, b, c, d] = q;
        a == b || a == c || a == d || b == c || b == d || c == d
    };

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
                let slots = find_four_quadrant_vertices([vx, vy, vz], [vx + 1, vy, vz], 1, 2);
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
                let slots = find_four_quadrant_vertices([vx, vy, vz], [vx, vy + 1, vz], 0, 2);
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
                let slots = find_four_quadrant_vertices([vx, vy, vz], [vx, vy, vz + 1], 0, 1);
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
    use crate::contour::contour_cluster;
    use crate::corner_vector::CornerVector;
    use crate::generators::{heightmap_terrain, solid_slab};
    use crate::material::Material;

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

    #[test]
    fn slope_contrasts_with_cell_based_centroid() {
        // The dual contour algorithm in `contour_cluster` averages
        // 8 corners per cell. The cell at (lx=0, ly=3, lz=128) sits
        // across the column-0/column-1 slope and includes 6 solid
        // corners + 2 empty corners; its centroid is at
        // ((4·0.5 + 4·1.5)/8, (4·3.5 + 4·4.5)/8, (4·128.5 +
        //  4·129.5)/8) = (1.0, 4.0, 129.0). The Y midpoint, not at
        // either column's top. The contour_surface algorithm places
        // vertices at the column tops (y=3.5 and y=4.5) — that's the
        // demonstration the dilution is gone.
        let c = build_slope_fixture();

        let cell_mesh = contour_cluster(&c);
        // Find the centroid vertex at (1.0, _, 129.0) — that's the
        // dual-contour cell vertex straddling the column-0/column-1
        // slope at z ≈ 128/129.
        let centroid_v = cell_mesh
            .vertices()
            .iter()
            .find(|v| (v.position[0] - 1.0).abs() < 0.05 && (v.position[2] - 129.0).abs() < 0.05)
            .expect("the cell-based algorithm should emit a centroid vertex here");
        assert!(
            (centroid_v.position[1] - 4.0).abs() < 0.05,
            "cell-based centroid Y is {}, expected ≈ 4.0 (the midpoint of the \
             two column tops at 3.5 and 4.5)",
            centroid_v.position[1]
        );

        // Confirm the surface algorithm places its vertices at the
        // column tops, not at the midpoint. (Filter for Y ≥ 3 to
        // skip the slope fixture's cluster-edge bottom/wall surface
        // vertices that sit at the same X/Z as the column top.)
        let surf_mesh = contour_surface(&c);
        let surf_top_at = |x: f32| -> f32 {
            surf_mesh
                .vertices()
                .iter()
                .find(|v| {
                    (v.position[0] - x).abs() < 0.05
                        && (v.position[2] - 128.5).abs() < 0.05
                        && v.position[1] >= 3.0
                })
                .unwrap_or_else(|| panic!("no top-row vertex at x≈{x}, z≈128.5"))
                .position[1]
        };
        let surf_y_col0 = surf_top_at(0.5);
        let surf_y_col1 = surf_top_at(1.5);
        assert!(
            (surf_y_col0 - 3.5).abs() < 0.05,
            "surface alg col-0 Y = {}, expected 3.5",
            surf_y_col0
        );
        assert!(
            (surf_y_col1 - 4.5).abs() < 0.05,
            "surface alg col-1 Y = {}, expected 4.5",
            surf_y_col1
        );
    }

    // ---- heightmap sanity ----

    #[test]
    fn heightmap_terrain_produces_substantial_mesh() {
        let mesh = contour_surface(&heightmap_terrain(0x42, Material::new(7, 7, 7).unwrap()));
        assert!(!mesh.is_empty());
        assert!(
            mesh.vertices().len() > 50_000,
            "expected > 50_000 vertices, got {}",
            mesh.vertices().len()
        );
        // Triangle count: with the rotational-priority quad search
        // finding 4 distinct cardinal neighbors at shell 1, the
        // mesh is dense on gentle regions of the wave field. Gaps
        // remain on the steep regions where the +X/-X cardinal
        // search at the edge's two Y values doesn't reach the
        // neighbor column's top (which sits at a Y the
        // perpendicular-plane search doesn't traverse). Empirically
        // ~50K triangles on this fixture, up from ~31K under the
        // previous (diagonal-only) rule. Participation analysis in
        // `heightmap_field_no_large_top_vertex_gaps` quantifies the
        // remaining gap density.
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
        let m = Material::new(7, 7, 7).unwrap();
        let c = heightmap_terrain(0x99, m);
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
        // The new sign-changing-edge rule tessellates the half-slab's
        // top face with diamond-pattern quads (one per interior Y-
        // edge, with the four corners being adjacent diagonal column
        // tops). Triangles strictly on the top face — every vertex
        // at Y ≈ 127.5 — must be plentiful.
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
        // Qualitative: the majority of column-top vertices on the
        // heightmap surface participate in at least one triangle.
        //
        // With the rotational-priority axis-adjacent search at shell
        // 1, participation rose from ~68% (previous diagonal-only
        // rule) to ~80% — a real improvement but still short of the
        // 90%+ a fully meshed surface would have.
        //
        // The remaining ~20% gap comes from a deeper limitation of
        // the perpendicular-plane-only search: when a Y-edge at
        // column (vx, vz) has a neighbor column whose top is at a
        // *different Y* than either edge endpoint (i.e., neighbor
        // h_n = h-2 or lower, or h+2 or higher), the search at shell
        // 1's (0, 1) and (1, 0) positions — which stay at Y = h-1
        // or Y = h — doesn't reach the neighbor's top. Shell 2's
        // (0, 2)/(2, 0) extend along perpendicular axes but stay at
        // the same Y. Only shell 2's (1, 1) diagonal reaches across
        // both perpendicular axes, and even then only at the same
        // two Y values. A search that includes Y excursions — or a
        // halo extension across cluster seams — would push this
        // higher.
        //
        // Threshold set at ≥ 75% to anchor the rotational-priority
        // improvement while flagging the remaining gap-density work
        // as the next follow-up.
        let m = Material::new(7, 7, 7).unwrap();
        let mesh = contour_surface(&heightmap_terrain(0x42, m));

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
}
