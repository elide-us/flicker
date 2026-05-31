//! Per-cell dual contour: surface a [`Primitive`] into a [`Cluster`].
//!
//! Pipeline stage: **primitive → contour to voxel (QEF)**. The contour
//! iterates **cells** (each cell `(cx, cy, cz)` spans the unit cube
//! `[cx, cx+1]³`; its 8 corners are voxels `(cx + i, cy + j, cz + k)` for
//! `i, j, k ∈ {0, 1}`). A cell is *active* when its 8 corner solidities are
//! not all equal; for each active cell, a single dual vertex is placed by
//! QEF-solving over the cell's sign-changing edges, then stored on the
//! cell's **min-corner voxel** as its [`CornerVector`].
//!
//! Storage convention (the load-bearing change from the original
//! per-solid-voxel contour): a voxel's `material` records this grid
//! point's solidity (`material` if solid, [`Material::EMPTY`] otherwise),
//! and its `corner` records *this cell's* dual vertex if the cell is
//! active. The two fields are independent — an empty voxel may still
//! carry a meaningful corner because its cell is active, which is exactly
//! what `mesh.rs` reads back to avoid cracks/spikes on sloped surfaces.
//!
//! Grid samples sit at integer voxel **origins** (the convention pinned
//! in [`crate::primitive`]), so the default corner `(0.5, 0.5, 0.5)`
//! decodes to the cell *center* — the right fallback for inactive cells.
//!
//! Single cluster, LOD 0, no neighbor context yet. The full-grid scan is
//! `O(CLUSTER_DIM³)` and `Cluster`'s HashMap storage dominates a dense
//! contour (octree storage TODO on `Cluster`).

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::corner_vector::CornerVector;
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::primitive::Primitive;
use crate::qef::Qef;
use crate::voxel::Voxel;

/// QEF regularization strength toward the mass point. Small enough to leave
/// well-constrained axes essentially exact, large enough to keep the solve
/// stable on flat/edge configurations.
const QEF_LAMBDA: f32 = 0.01;

/// The 8 cell-corner offsets from the cell's min corner. Index order is
/// `i + 2*j + 4*k` for `(i, j, k) ∈ {0, 1}³`.
const CORNER_OFFSETS: [[i32; 3]; 8] = [
    [0, 0, 0], // 0
    [1, 0, 0], // 1
    [0, 1, 0], // 2
    [1, 1, 0], // 3
    [0, 0, 1], // 4
    [1, 0, 1], // 5
    [0, 1, 1], // 6
    [1, 1, 1], // 7
];

/// The 12 cell edges as pairs of [`CORNER_OFFSETS`] indices: 4 along X,
/// 4 along Y, 4 along Z.
const EDGES: [(usize, usize); 12] = [
    // X edges (differ in i)
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    // Y edges (differ in j)
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    // Z edges (differ in k)
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// Contour `primitive` into a [`Cluster`] under the per-cell convention.
///
/// Every solid voxel is stored with `material`; every active cell's dual
/// vertex is stored as the [`CornerVector`] on its min-corner voxel — even
/// when that voxel is itself empty. Empty voxels with the default corner
/// are *not* stored (they round-trip through the cluster's base value).
#[must_use]
pub fn contour(primitive: &dyn Primitive, material: Material) -> Cluster {
    let mut cluster = Cluster::empty();
    let dim = CLUSTER_DIM as usize;
    let cell_dim = dim - 1;
    let idx = |x: usize, y: usize, z: usize| -> usize { (z * dim + y) * dim + x };

    // Precompute solidity for every grid point in [0, CLUSTER_DIM)³ so the
    // per-cell pass reads each is_solid at most once. ~16MB for a 256³
    // cluster; cheaper than re-querying the primitive eight times per cell.
    let mut solid = vec![false; dim * dim * dim];
    for z in 0..dim {
        for y in 0..dim {
            for x in 0..dim {
                solid[idx(x, y, z)] = primitive.is_solid(x as i32, y as i32, z as i32);
            }
        }
    }

    for z in 0..dim {
        for y in 0..dim {
            for x in 0..dim {
                let s = solid[idx(x, y, z)];
                let material_out = if s { material } else { Material::EMPTY };
                let mut corner_out = CornerVector::DEFAULT;

                // Process the cell whose min-corner is this voxel. The
                // last row of voxels along each axis owns no cell.
                if x < cell_dim && y < cell_dim && z < cell_dim {
                    let mut corner_solid = [false; 8];
                    for (i, off) in CORNER_OFFSETS.iter().enumerate() {
                        corner_solid[i] = solid[idx(
                            x + off[0] as usize,
                            y + off[1] as usize,
                            z + off[2] as usize,
                        )];
                    }

                    let any_solid = corner_solid.iter().any(|&b| b);
                    let all_solid = corner_solid.iter().all(|&b| b);
                    if any_solid && !all_solid {
                        let mut qef = Qef::new();
                        for &(a, b) in &EDGES {
                            if corner_solid[a] == corner_solid[b] {
                                continue;
                            }
                            let oa = CORNER_OFFSETS[a];
                            let ob = CORNER_OFFSETS[b];
                            let pa = [x as i32 + oa[0], y as i32 + oa[1], z as i32 + oa[2]];
                            let pb = [x as i32 + ob[0], y as i32 + ob[1], z as i32 + ob[2]];
                            let h = primitive.edge_hermite(pa, pb);
                            qef.add(h.position, h.normal);
                        }
                        if qef.count() > 0 {
                            let v = qef.solve(QEF_LAMBDA);
                            // Clamp the vertex into this cell's AABB
                            // [(x,y,z), (x+1,y+1,z+1)]. mesh.rs assumes the
                            // stored vertex belongs to this cell; an
                            // unclamped QEF for a near-degenerate normal
                            // basis can slide outside and produce spikes.
                            let fx = x as f32;
                            let fy = y as f32;
                            let fz = z as f32;
                            let vx = v[0].clamp(fx, fx + 1.0);
                            let vy = v[1].clamp(fy, fy + 1.0);
                            let vz = v[2].clamp(fz, fz + 1.0);
                            corner_out = CornerVector::from_components(
                                vx - fx,
                                vy - fy,
                                vz - fz,
                            );
                        }
                    }
                }

                // Empty voxels with the default corner round-trip through
                // the cluster's base value; storing them would waste a
                // HashMap entry. Everything else must be recorded — in
                // particular, an empty voxel whose cell is active carries
                // that cell's dual vertex and MUST be stored.
                if material_out != Material::EMPTY || corner_out != CornerVector::DEFAULT {
                    let coord =
                        LocalCoord::new(x as u32, y as u32, z as u32).expect("in bounds");
                    cluster.set(coord, Voxel::new(corner_out, material_out));
                }
            }
        }
    }

    cluster
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::{FlatField, Hermite};

    fn grey() -> Material {
        Material::new(1, 1, 0).expect("valid")
    }

    /// A small solid cube `[0, n)³` for fast contour-loop tests — avoids
    /// the multi-million-voxel fill of a full-cluster field.
    struct CubeField {
        n: i32,
    }

    impl Primitive for CubeField {
        fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
            (0..self.n).contains(&x) && (0..self.n).contains(&y) && (0..self.n).contains(&z)
        }

        fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
            // Midpoint of the two voxel origins, normal toward the empty
            // side. Enough for loop mechanics; not a precise surface.
            let ca = [a[0] as f32, a[1] as f32, a[2] as f32];
            let cb = [b[0] as f32, b[1] as f32, b[2] as f32];
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

    /// A primitive with only voxel `(1, 1, 1)` solid. Cell `(0, 0, 0)` is
    /// active (7 empty corners + 1 solid at `+++`) and its min-corner
    /// voxel `(0, 0, 0)` is empty — the case the per-solid-voxel contour
    /// would silently drop a vertex on.
    struct CornerSolid;

    impl Primitive for CornerSolid {
        fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
            x == 1 && y == 1 && z == 1
        }

        fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
            // The lone solid endpoint is (1, 1, 1). Pull the crossing
            // toward it (t = 0.9) so the QEF result is clearly off-center.
            let solid = [1_i32, 1, 1];
            let empty = if a == solid { b } else { a };
            let position = [
                solid[0] as f32 * 0.9 + empty[0] as f32 * 0.1,
                solid[1] as f32 * 0.9 + empty[1] as f32 * 0.1,
                solid[2] as f32 * 0.9 + empty[2] as f32 * 0.1,
            ];
            let mut normal = [0.0_f32; 3];
            for i in 0..3 {
                if a[i] != b[i] {
                    normal[i] = (empty[i] - solid[i]) as f32;
                }
            }
            Hermite { position, normal }
        }
    }

    #[test]
    fn small_cube_stores_only_solid_voxels() {
        let c = contour(&CubeField { n: 3 }, grey());
        // A 3³ cube: all 27 solid voxels are min-corners of either an
        // inactive (interior) cell or an active boundary cell, so all 27
        // are stored. No empty voxel outside the cube becomes the min-
        // corner of an active cell (active cells live within {0,1,2}³),
        // so nothing else is stored.
        assert_eq!(c.override_count(), 27);
        assert_ne!(
            c.get(LocalCoord::new(1, 1, 1).unwrap()).material(),
            Material::EMPTY
        );
        assert_eq!(
            c.get(LocalCoord::new(5, 5, 5).unwrap()).material(),
            Material::EMPTY
        );
    }

    #[test]
    fn flat_cell_vertex_lands_on_plane() {
        // Compose the QEF for cell (10, 127, 10) directly from
        // FlatField::at_half() — the cell straddles the y=128 plane, so
        // its 4 Y-edges are the active set. Each crosses at y=128 with
        // normal +Y, putting the dual vertex at (10.5, 128, 10.5) and the
        // stored corner at (0.5, 1.0, 0.5).
        let f = FlatField::at_half();
        let cx = 10_i32;
        let cy = 127_i32;
        let cz = 10_i32;
        let mut qef = Qef::new();
        let mut active = 0;
        for &(a, b) in &EDGES {
            let oa = CORNER_OFFSETS[a];
            let ob = CORNER_OFFSETS[b];
            let pa = [cx + oa[0], cy + oa[1], cz + oa[2]];
            let pb = [cx + ob[0], cy + ob[1], cz + ob[2]];
            if f.is_solid(pa[0], pa[1], pa[2]) != f.is_solid(pb[0], pb[1], pb[2]) {
                let h = f.edge_hermite(pa, pb);
                qef.add(h.position, h.normal);
                active += 1;
            }
        }
        assert_eq!(active, 4, "only the 4 Y-edges of this cell should cross");
        let v = qef.solve(QEF_LAMBDA);
        let cv = CornerVector::from_components(
            v[0] - cx as f32,
            v[1] - cy as f32,
            v[2] - cz as f32,
        );
        let [dx, dy, dz] = cv.to_components();
        assert!((dx - 0.5).abs() < 0.01, "dx={dx}");
        assert!((dy - 1.0).abs() < 0.01, "dy={dy}");
        assert!((dz - 0.5).abs() < 0.01, "dz={dz}");
    }

    #[test]
    fn active_cell_with_empty_min_corner_gets_a_vertex() {
        // The regression this rework exists for: cell (0,0,0) has solid
        // mass at its (1,1,1) corner only; the min-corner voxel (0,0,0)
        // is empty but the cell is active. The per-cell contour must
        // store the empty voxel anyway, carrying the cell's dual vertex
        // — otherwise mesh.rs reads a default corner and emits a crack
        // /spike along this face.
        let c = contour(&CornerSolid, grey());
        let v = c.get(LocalCoord::new(0, 0, 0).unwrap());
        assert_eq!(
            v.material(),
            Material::EMPTY,
            "min-corner voxel is empty"
        );
        assert_ne!(
            v.corner(),
            CornerVector::DEFAULT,
            "active cell's vertex must be stored on the empty min-corner"
        );
        let [dx, dy, dz] = v.corner().to_components();
        // QEF resolves three orthogonal planes at x=y=z=0.9, so the dual
        // vertex sits well past the cell center.
        assert!(dx > 0.5, "dx={dx} should be skewed toward +X solid corner");
        assert!(dy > 0.5, "dy={dy} should be skewed toward +Y solid corner");
        assert!(dz > 0.5, "dz={dz} should be skewed toward +Z solid corner");
    }
}
