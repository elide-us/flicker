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
//! # Multi-cluster and LOD
//!
//! Contour takes the cluster's [`ClusterId`], which encodes both the
//! cluster's world position and its [`Lod`] level. The primitive is
//! queried at world coordinates (`world = id.world_offset() + i·stride`).
//! At LOD `L` the sample stride is `2^L`: the cluster has `sample_dim =
//! 256/stride` sample positions per axis, and the cell-min-corner loop
//! covers `[0, sample_dim)` — one row wider than the cell range
//! `[0, sample_dim − 1)`, so the **+-seam shell** at sample index
//! `sample_dim − 1` is processed too. Those cells' `+1` corners reach
//! one sample past the cluster boundary; the primitive is global, so
//! it answers the OOB sample queries directly, and the resulting QEF
//! vertex is stored on the sample-aligned min-corner voxel
//! `((sample_dim−1)·stride, …)`.
//!
//! Stored corners are in **cell-units** `[0, 1]`: `corner = (v − origin)
//! /stride`. Mesh decodes back to cluster-local voxel coords by
//! `origin + corner·stride`. At LOD 0 (`stride = 1`) the encode is the
//! identity, so LOD-0 storage is byte-identical to the pre-§2 path.
//!
//! Cross-LOD seam handling lives in [`crate::mesh`] — contour is
//! per-cluster and oblivious to neighbour LODs.

use crate::cluster::Cluster;
use crate::cluster_id::ClusterId;
use crate::corner_vector::CornerVector;
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::neighbor::Lod;
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

/// Contour `primitive` into a [`Cluster`] for the cluster at `id`.
///
/// The primitive is queried at world coordinates so that two adjacent
/// clusters sample the same continuous function across their shared
/// face. Every active cell's dual vertex is stored as a
/// [`CornerVector`] on its min-corner voxel — including the +-seam-shell
/// row at `lx = CLUSTER_DIM - 1` whose `+1` corners reach one voxel
/// into the high-side neighbor (the global primitive answers those
/// truthfully). Empty voxels with the default corner are *not* stored.
#[must_use]
pub fn contour(primitive: &dyn Primitive, material: Material, id: ClusterId) -> Cluster {
    let mut cluster = Cluster::empty();
    let lod = Lod::new(id.lod()).expect("ClusterId LOD must be in [0, 7] for intra-cluster contour");
    let stride = lod.stride() as i32;
    let sample_dim = lod.sample_dim() as usize;
    let span = sample_dim + 1;
    let idx = |x: usize, y: usize, z: usize| -> usize { (z * span + y) * span + x };

    let off = id.world_offset();
    let off_x = off[0] as i32;
    let off_y = off[1] as i32;
    let off_z = off[2] as i32;

    let mut solid = vec![false; span * span * span];
    for k in 0..span {
        let wz = off_z + k as i32 * stride;
        for j in 0..span {
            let wy = off_y + j as i32 * stride;
            for i in 0..span {
                let wx = off_x + i as i32 * stride;
                solid[idx(i, j, k)] = primitive.is_solid(wx, wy, wz);
            }
        }
    }

    for k in 0..sample_dim {
        for j in 0..sample_dim {
            for i in 0..sample_dim {
                let s = solid[idx(i, j, k)];
                let material_out = if s { material } else { Material::EMPTY };
                let mut corner_out = CornerVector::DEFAULT;

                let mut corner_solid = [false; 8];
                for (n, ofs) in CORNER_OFFSETS.iter().enumerate() {
                    corner_solid[n] = solid[idx(
                        i + ofs[0] as usize,
                        j + ofs[1] as usize,
                        k + ofs[2] as usize,
                    )];
                }

                let any_solid = corner_solid.iter().any(|&b| b);
                let all_solid = corner_solid.iter().all(|&b| b);

                let cell_vx = i as i32 * stride;
                let cell_vy = j as i32 * stride;
                let cell_vz = k as i32 * stride;

                if any_solid && !all_solid {
                    corner_out = cell_qef_corner(
                        primitive,
                        &corner_solid,
                        [cell_vx, cell_vy, cell_vz],
                        [off_x, off_y, off_z],
                        stride,
                    );
                }

                if material_out != Material::EMPTY || corner_out != CornerVector::DEFAULT {
                    let coord = LocalCoord::new(
                        cell_vx as u32,
                        cell_vy as u32,
                        cell_vz as u32,
                    )
                    .expect("sample-aligned voxel in [0, CLUSTER_DIM)");
                    cluster.set(coord, Voxel::new(corner_out, material_out));
                }
            }
        }
    }

    cluster
}

/// Compute the QEF dual vertex for a single cell and return its
/// cell-units corner encoding. The cell spans `[cell_v, cell_v +
/// stride]` in cluster-local voxel coords; the QEF runs in cluster-
/// local voxel units (so LOD-0 byte path is unchanged), then re-encodes
/// the result in `[0, 1]` cell-units. Returns the default corner when
/// the cell has no sign-crossing edges (degenerate input).
fn cell_qef_corner(
    primitive: &dyn Primitive,
    corner_solid: &[bool; 8],
    cell_v: [i32; 3],
    world_off: [i32; 3],
    stride: i32,
) -> CornerVector {
    let wx_cell = world_off[0] + cell_v[0];
    let wy_cell = world_off[1] + cell_v[1];
    let wz_cell = world_off[2] + cell_v[2];

    let mut qef = Qef::new();
    for &(a, b) in &EDGES {
        if corner_solid[a] == corner_solid[b] {
            continue;
        }
        let oa = CORNER_OFFSETS[a];
        let ob = CORNER_OFFSETS[b];
        let pa = [
            wx_cell + oa[0] * stride,
            wy_cell + oa[1] * stride,
            wz_cell + oa[2] * stride,
        ];
        let pb = [
            wx_cell + ob[0] * stride,
            wy_cell + ob[1] * stride,
            wz_cell + ob[2] * stride,
        ];
        let h = primitive.edge_hermite(pa, pb);
        let pos_local = [
            h.position[0] - world_off[0] as f32,
            h.position[1] - world_off[1] as f32,
            h.position[2] - world_off[2] as f32,
        ];
        qef.add(pos_local, h.normal);
    }
    if qef.count() == 0 {
        return CornerVector::DEFAULT;
    }
    let v = qef.solve(QEF_LAMBDA);
    let fx = cell_v[0] as f32;
    let fy = cell_v[1] as f32;
    let fz = cell_v[2] as f32;
    let fs = stride as f32;
    let vx = v[0].clamp(fx, fx + fs);
    let vy = v[1].clamp(fy, fy + fs);
    let vz = v[2].clamp(fz, fz + fs);
    CornerVector::from_components(
        (vx - fx) / fs,
        (vy - fy) / fs,
        (vz - fz) / fs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitive::{FlatField, Hermite};

    fn grey() -> Material {
        Material::new(1, 1, 0).expect("valid")
    }

    fn origin_id() -> ClusterId {
        ClusterId::new(0, 0, 0, 0)
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
        let c = contour(&CubeField { n: 3 }, grey(), origin_id());
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
        let c = contour(&CornerSolid, grey(), origin_id());
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

    #[test]
    fn seam_shell_cell_stored_on_max_voxel_for_flat_field() {
        // FlatField at y=128: the +X-seam shell row at lx=255 has 256³
        // cells along YZ; the one at (255, 127, 0) straddles the
        // surface so its dual vertex must be stored on the min-corner
        // voxel (255, 127, 0). Before the seam-shell extension, the
        // loop's `< cell_dim` gate would have skipped lx=255 entirely.
        let c = contour(&FlatField::at_half(), grey(), origin_id());
        let seam = c.get(LocalCoord::new(255, 127, 0).unwrap());
        assert_ne!(
            seam.corner(),
            CornerVector::DEFAULT,
            "+-seam shell must store the surface-straddling cell's vertex"
        );
        let [_dx, dy, _dz] = seam.corner().to_components();
        // The flat field's Y-edges cross at y=128 → cell-relative
        // Y-offset 1.0 for the cell whose min-corner is at lx=255,
        // ly=127. The X and Z components are unconstrained by the
        // 4 Y-edges (the QEF leaves them at the mass point ≈ 0.5).
        assert!((dy - 1.0).abs() < 0.01, "dy={dy} should land on plane");
    }

    #[test]
    fn world_offset_shifts_primitive_sampling() {
        // A cluster at id (0, 5, 0, 0) — world offset (1280, 0, 0) —
        // contoured against a CubeField n=3 (solid at world [0,3)³)
        // sees an entirely empty primitive in its territory and stores
        // nothing.
        let id_far = ClusterId::new(0, 5, 0, 0);
        let c = contour(&CubeField { n: 3 }, grey(), id_far);
        assert_eq!(
            c.override_count(),
            0,
            "cluster at world offset 1280 sees no cube"
        );
    }

    #[test]
    fn lod2_contour_stores_at_sample_aligned_slots_only() {
        // LOD 2: stride=4, sample_dim=64. Every stored override's
        // (x, y, z) must be a multiple of 4 — that's the §2 storage
        // invariant. FlatField produces a band of active cells around
        // the surface, plenty of slots to check.
        let id_lod2 = ClusterId::new(2, 0, 0, 0);
        let c = contour(&FlatField::at_half(), grey(), id_lod2);
        assert!(c.override_count() > 0, "LOD-2 flat field should contour");
        for (coord, _voxel) in c.overrides() {
            assert_eq!(coord.x() % 4, 0, "x={} not stride-aligned", coord.x());
            assert_eq!(coord.y() % 4, 0, "y={} not stride-aligned", coord.y());
            assert_eq!(coord.z() % 4, 0, "z={} not stride-aligned", coord.z());
        }
    }

    #[test]
    fn lod2_flat_cell_corner_lands_on_surface_plane() {
        // At LOD 2, sample_dim=64 → samples at world y ∈ {0, 4, 8, …, 256}.
        // The surface y=128 sits exactly on sample index 32. The active
        // cell straddling y=128 has min-corner at sample 31 → voxel
        // (i*4, 124, k*4) with the Y-axis crossing one cell up at
        // sample 32 (voxel y=128). Stored corner.y in cell-units
        // should be ~1.0 (the QEF lands the dual vertex right on the
        // plane); decoded vertex y = 124 + 1.0·4 = 128.
        let id_lod2 = ClusterId::new(2, 0, 0, 0);
        let c = contour(&FlatField::at_half(), grey(), id_lod2);
        // Cell at sample (0, 31, 0) → min-corner voxel (0, 124, 0).
        let cell = c.get(LocalCoord::new(0, 124, 0).unwrap());
        let [_dx, dy, _dz] = cell.corner().to_components();
        // cell-units: 1.0 places the vertex on the +Y face, which is
        // at world y = 124 + 4 = 128 — the plane.
        assert!(
            (dy - 1.0).abs() < 0.01,
            "LOD-2 cell-units dy={dy} should be 1.0 (vertex at y=128)"
        );
    }

    #[test]
    fn lod0_byte_identical_to_pre_lod_path() {
        // Regression: the LOD-0 contour result must be unchanged by the
        // §2 rework. Compare the override count and a few representative
        // cells against known values from the §1 baseline.
        let c = contour(&FlatField::at_half(), grey(), origin_id());
        // §1 stores 256² seam-shell cells along the surface plus the
        // dense solid fill below the plane (256² · 128 cells). The
        // specific count is whatever §1 produced; what we guard is
        // that exact byte-equality at a tracer cell.
        let cell = c.get(LocalCoord::new(10, 127, 10).unwrap());
        let bytes = cell.corner().bytes();
        // The pre-§2 path encoded this cell as (0.5, 1.0, 0.5) →
        // bytes [128, 191, 128]. (encode_axis(0.5)=128, encode_axis(1.0)=191.)
        assert_eq!(
            bytes, [128, 191, 128],
            "LOD-0 cell corner bytes drifted: {bytes:?}"
        );
    }
}
