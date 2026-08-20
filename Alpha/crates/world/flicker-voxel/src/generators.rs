//! Cluster generators — small builder functions that produce
//! [`Cluster`] instances with simple, well-known content.
//!
//! These are prototype-grade — useful for spinning up a cluster in a
//! few lines of test or example code. They are **slow** when the
//! resulting cluster has many overrides because the current sparse
//! storage uses a `HashMap`; a future octree-backed cluster will make
//! these O(1) in storage. For now, prefer them only when a small
//! handful of clusters need to exist.
//!
//! See the module-level doc on [`crate::cluster`] for the storage
//! trade-off discussion this references.

use clayengine::CLUSTER_DIM;
use flicker_primitive::heightmap::world_height_seeded;

use crate::cluster::Cluster;
use crate::corner_vector::CornerVector;
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::voxel::Voxel;
use crate::voxel_state::VoxelState;

/// Demo material ids for the scene generators — **catalog `MaterialId`s**
/// (`materials.json`) since the demo palette was retired (2026-08-19); the
/// mesh pass colours them from the catalog palette. The old five-shade water
/// band collapsed onto the two real materials it approximated: submerged
/// ground is Water (60), the crest/beach transition is Sand (22) — the band
/// machinery still exercises the primary/secondary/blend packing.
pub mod demo_materials {
    pub const DEEP_WATER: u8 = 60;
    pub const MID_WATER: u8 = 60;
    pub const SHALLOW: u8 = 60;
    pub const CREST: u8 = 22;
    pub const FOAM: u8 = 22;
}

/// Generate a cluster representing a flat terrain slab: every voxel at
/// `y < height` is solid with the given material, every voxel at
/// `y >= height` is empty.
///
/// `height` is in voxels and is clamped to `0..=CLUSTER_DIM`. With
/// `height == 0` the cluster is entirely empty (no overrides, no
/// surface). With `height == CLUSTER_DIM` the cluster is entirely
/// solid (uniform classification, no surface).
///
/// # Performance
///
/// Walks `height * CLUSTER_DIM * CLUSTER_DIM` voxel positions and
/// inserts an override for each. At `height = 128`, that's
/// `128 * 256 * 256 = 8_388_608` `HashMap::insert` calls — visibly
/// slow even in release builds (~hundreds of ms). Acceptable for one-
/// shot prototype setup; not acceptable in a hot path. The cluster
/// storage rewrite tracked elsewhere will replace this with a sparse
/// octree that handles this shape in O(log N).
#[must_use]
pub fn solid_slab(height: u32, material: Material) -> Cluster {
    let h = height.min(CLUSTER_DIM);
    let mut c = Cluster::empty();
    if h == 0 {
        return c;
    }
    let v = Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, material);
    for z in 0..CLUSTER_DIM {
        for y in 0..h {
            for x in 0..CLUSTER_DIM {
                c.set(LocalCoord::new(x, y, z).expect("in bounds"), v);
            }
        }
    }
    c
}

/// Generate a cluster from the world heightmap function at a given
/// world offset — the multi-cluster generator that **expresses** the
/// surface through join placement rather than only through
/// classification.
///
/// For each `(x, z)` column the heightmap is sampled at the column's
/// voxel-center world coordinates
/// `(world_offset.x + x + 0.5, world_offset.z + z + 0.5)` to obtain
/// the surface height `h` (in world voxel units). The column is then
/// materialized as:
///
/// - Voxels at `y < floor(h)`: solid with [`CornerVector::DEFAULT`].
/// - Voxel at `y == floor(h)` (the topmost solid layer): solid with
///   `CornerVector::Y == fractional(h)`. This pulls the voxel's
///   owned `+++` corner from its default position at world Y
///   `floor(h) + 0.5` to world Y `floor(h) + fractional(h) == h`, so
///   the corner sits exactly on the surface.
/// - Voxels at `y > floor(h)`: empty.
///
/// # Why this produces smooth slopes
///
/// A surface quad in the contour pass uses up to 8 voxel corners as
/// its inputs. Adjacent columns have continuous heights (the heightmap
/// is Lipschitz-bounded), so their topmost-voxel joins sit at slightly
/// different world-Y positions. The contour pass reads these
/// corners directly, so the rendered surface follows the continuous
/// height function as a tilted quad rather than a stepped terrace.
///
/// # Coordinate convention
///
/// Cluster-local voxel `(x, y, z)` is at world position
/// `(world_offset.x + x, world_offset.y + y, world_offset.z + z)`.
/// The cluster's local Y=0 corresponds to world Y=`world_offset.y`.
///
/// **Y-stacking is not implemented yet.** This function assumes
/// `world_offset.y == 0` — the surface is expressed entirely within
/// the cluster's local Y span. When Y-stacking is added in a later
/// step, this function will gain the logic to handle surfaces that
/// fall above or below the cluster's local Y range; until then,
/// callers should pass `world_offset.y = 0`. The Y component is
/// otherwise ignored.
///
/// # Cross-cluster seam continuity
///
/// Two adjacent clusters call this function with offsets differing by
/// `CLUSTER_DIM` along one axis. Their boundary columns sample the
/// heightmap at world coordinates that are exactly one voxel apart
/// (e.g., cluster A at offset 0, column 255: world x=255.5; cluster
/// B at offset 256, column 0: world x=256.5), and the heightmap is
/// continuous, so the topmost-voxel joins from one cluster meet the
/// next continuously **with no coordination by the contour pass**.
/// That is the architectural payoff: cross-cluster correctness comes
/// from sampling the same continuous function, not from a seam-time
/// gate.
///
/// # Cliffs
///
/// The heightmap is smooth but not arbitrarily flat — adjacent columns
/// can differ by more than one voxel where the field's gradient is
/// steep. Those height jumps become standard vertical-face emissions
/// by the contour pass; no special cliff-fill logic is needed here.
///
/// # Performance
///
/// Iterates every solid voxel and inserts an override (worst case
/// ~8M inserts at base height 128). Same `HashMap`-bound cost as the
/// other generators; tracked under the octree-storage TODO.
#[must_use]
pub fn heightmap_terrain_at(seed: u64, material: Material, world_offset: [f32; 3]) -> Cluster {
    let mut c = Cluster::empty();
    let solid_default = Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, material);
    let ox = world_offset[0];
    let oz = world_offset[2];

    for z in 0..CLUSTER_DIM {
        for x in 0..CLUSTER_DIM {
            let h = world_height_seeded(ox + x as f32 + 0.5, oz + z as f32 + 0.5, seed);
            // The heightmap is finite by construction, but be defensive.
            if !h.is_finite() {
                continue;
            }
            // Surface entirely below the cluster floor: nothing solid.
            if h <= 0.0 {
                continue;
            }

            let top_y_i = h.floor() as i64;
            // Clamp the topmost layer's index to the cluster ceiling.
            // When the surface is at or above the ceiling, the whole
            // column is solid with default joins (no in-cluster
            // boundary layer to express).
            let capped = top_y_i >= CLUSTER_DIM as i64;
            let top_y = if capped {
                CLUSTER_DIM - 1
            } else {
                top_y_i as u32
            };

            // Solid fill below the surface — default joins.
            for y in 0..top_y {
                c.set(LocalCoord::new(x, y, z).expect("in bounds"), solid_default);
            }

            // Topmost layer: corner Y positioned to express the
            // fractional part of h. When capped, leave the default join.
            let top_voxel = if capped {
                solid_default
            } else {
                let fractional = h - top_y as f32;
                Voxel::new(
                    VoxelState::Solid,
                    CornerVector::from_components(0.5, fractional, 0.5),
                    material,
                )
            };
            c.set(LocalCoord::new(x, top_y, z).expect("in bounds"), top_voxel);
        }
    }

    c
}

/// Generate a cluster from the world heightmap function at a given
/// world offset, with each solid voxel's material keyed to its
/// normalized Y position within the heightmap band — producing a
/// smooth dark-to-light depth gradient across the surface.
///
/// Identical geometry to [`heightmap_terrain_at`]: same heightmap
/// sampling, same fractional-Y top-corner placement. The only
/// difference is material assignment.
///
/// # Banding
///
/// The heightmap band is `[BASE_HEIGHT - AMPLITUDE, BASE_HEIGHT +
/// AMPLITUDE] = [64, 192]`. For each solid voxel at world Y `y`, we
/// compute `t = (y - 64) / 128` clamped to `[0, 1]` and look up the
/// `(primary, secondary, blend)` for that `t` via [`water_material_at`]:
///
/// | `t` range  | Transition                |
/// | ---------- | ------------------------- |
/// | `[0, 0.25)`  | `DEEP_WATER` → `MID_WATER` |
/// | `[0.25, 0.55)` | `MID_WATER` → `SHALLOW`    |
/// | `[0.55, 0.80)` | `SHALLOW` → `CREST`        |
/// | `[0.80, 0.95)` | `CREST` → `FOAM`           |
/// | `[0.95, 1.0]`  | `FOAM` solid               |
///
/// The blend factor encodes the *fractional* position within the
/// current segment, so adjacent voxels at slightly different heights
/// produce smoothly interpolated colors after the shader's `mix`.
#[must_use]
pub fn heightmap_terrain_at_with_depth_materials(seed: u64, world_offset: [f32; 3]) -> Cluster {
    let mut c = Cluster::empty();
    let ox = world_offset[0];
    let oz = world_offset[2];

    // The heightmap band edges in world Y (see `heightmap.rs`). Local
    // Y equals world Y when `world_offset.y == 0` (the Y-stacking
    // assumption documented on `heightmap_terrain_at`).
    const BAND_LO: f32 = 64.0;
    const BAND_HI: f32 = 192.0;
    const BAND_SPAN: f32 = BAND_HI - BAND_LO;

    for z in 0..CLUSTER_DIM {
        for x in 0..CLUSTER_DIM {
            let h = world_height_seeded(ox + x as f32 + 0.5, oz + z as f32 + 0.5, seed);
            if !h.is_finite() {
                continue;
            }
            if h <= 0.0 {
                continue;
            }

            let top_y_i = h.floor() as i64;
            let capped = top_y_i >= CLUSTER_DIM as i64;
            let top_y = if capped {
                CLUSTER_DIM - 1
            } else {
                top_y_i as u32
            };

            // Solid fill below the surface — default joins, material
            // banded by each voxel's own world Y.
            for y in 0..top_y {
                let t = ((y as f32 - BAND_LO) / BAND_SPAN).clamp(0.0, 1.0);
                let (p, s, b) = water_material_at(t);
                let m = Material::new(p, s, b);
                c.set(
                    LocalCoord::new(x, y, z).expect("in bounds"),
                    Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, m),
                );
            }

            // Topmost layer: corner Y positioned to express the
            // fractional part of h (matches `heightmap_terrain_at`).
            let t_top = ((top_y as f32 - BAND_LO) / BAND_SPAN).clamp(0.0, 1.0);
            let (p, s, b) = water_material_at(t_top);
            let m = Material::new(p, s, b);
            let top_voxel = if capped {
                Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, m)
            } else {
                let fractional = h - top_y as f32;
                Voxel::new(
                    VoxelState::Solid,
                    CornerVector::from_components(0.5, fractional, 0.5),
                    m,
                )
            };
            c.set(LocalCoord::new(x, top_y, z).expect("in bounds"), top_voxel);
        }
    }

    c
}

/// Map a normalized vertical position `t ∈ [0, 1]` within the water
/// heightmap band to `(primary, secondary, blend_byte)`. `t = 0`
/// is the deepest trough; `t = 1` is the highest crest. The blend
/// byte is the local segment fraction scaled into `[0, 255]`.
fn water_material_at(t: f32) -> (u8, u8, u8) {
    use demo_materials::*;
    // Segment edges over `t`. Each entry is `(edge, primary, secondary)`
    // — within `[prev_edge, edge)`, the voxel interpolates from
    // `primary` to `secondary`.
    let segments: [(f32, u8, u8); 4] = [
        (0.25, DEEP_WATER, MID_WATER),
        (0.55, MID_WATER, SHALLOW),
        (0.80, SHALLOW, CREST),
        (0.95, CREST, FOAM),
    ];
    let mut prev_edge = 0.0_f32;
    for (edge, lo_idx, hi_idx) in segments {
        if t < edge {
            let local_t = ((t - prev_edge) / (edge - prev_edge)).clamp(0.0, 1.0);
            return (lo_idx, hi_idx, (local_t * 255.0) as u8);
        }
        prev_edge = edge;
    }
    // Above all segment edges: solid FOAM.
    (demo_materials::FOAM, demo_materials::FOAM, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_material() -> Material {
        Material::new(7, 7, 7)
    }

    #[test]
    fn solid_slab_zero_height_is_empty() {
        let m = solid_material();
        let c = solid_slab(0, m);
        // No overrides — equivalent to `Cluster::empty()`.
        assert_eq!(c.override_count(), 0);
    }

    #[test]
    fn solid_slab_thin_layer_count() {
        // height = 1 → one Y plane of overrides = 256 * 256.
        let c = solid_slab(1, solid_material());
        assert_eq!(
            c.override_count(),
            (CLUSTER_DIM as usize) * (CLUSTER_DIM as usize)
        );
    }

    #[test]
    fn solid_slab_half_height_count() {
        // height = 128 → 128 * 256 * 256 = 8_388_608 overrides.
        let c = solid_slab(128, solid_material());
        let expected = 128usize * (CLUSTER_DIM as usize) * (CLUSTER_DIM as usize);
        assert_eq!(c.override_count(), expected);
    }

    // ---- heightmap_terrain_at ----

    #[test]
    fn heightmap_terrain_at_world_offset_shifts_sample_coordinates() {
        // A cluster at offset (CLUSTER_DIM, 0, 0) at its local column
        // (lx, lz) must materialize the topmost join from the heightmap
        // sampled at world (CLUSTER_DIM + lx + 0.5, lz + 0.5, seed).
        let m = solid_material();
        let seed = 0x1357_ACE0;
        let offset_x = CLUSTER_DIM as f32;
        let c = heightmap_terrain_at(seed, m, [offset_x, 0.0, 0.0]);

        let tol = 1.0 / 255.0 + 1e-6;
        for &(lx, lz) in &[(0u32, 5u32), (10, 200), (255, 128)] {
            let world_x = offset_x + lx as f32 + 0.5;
            let world_z = lz as f32 + 0.5;
            let h = world_height_seeded(world_x, world_z, seed);
            let top_y = h.floor() as u32;
            let fractional = h - top_y as f32;

            let actual = c
                .get(LocalCoord::new(lx, top_y, lz).expect("in bounds"))
                .corner()
                .to_components()[1];
            assert!(
                (actual - fractional).abs() <= tol,
                "offset-cluster column ({lx},{lz}) corner_y={actual}, want ≈ {fractional} (h={h})"
            );
        }
    }

    #[test]
    fn adjacent_clusters_agree_at_seam() {
        // The architectural payoff: two clusters at adjacent world
        // offsets must produce continuous join positions at the seam
        // — without any cross-cluster coordination.
        //
        // Cluster A's column 255 samples at world x=255.5.
        // Cluster B (at offset CLUSTER_DIM=256) column 0 samples at
        // world x=256.5. Each cluster's topmost-voxel corner-Y is the
        // fractional part of the height sampled at its own world x;
        // the world-space Y of those corners is then
        // (floor(h) + corner_y) = h. So the world-space Y of both
        // adjacent clusters' joins is exactly the heightmap value at
        // their respective sample points — proving the world-offset
        // arithmetic is correct.
        let m = solid_material();
        let seed = 0xCAFE_F00D_D15E_A5E5;
        let a = heightmap_terrain_at(seed, m, [0.0, 0.0, 0.0]);
        let b = heightmap_terrain_at(seed, m, [CLUSTER_DIM as f32, 0.0, 0.0]);

        let tol = 1.0 / 255.0 + 1e-6;
        for lz in (0u32..CLUSTER_DIM).step_by(13) {
            let world_z = lz as f32 + 0.5;

            // Cluster A's +X boundary column.
            let h_a = world_height_seeded(255.5, world_z, seed);
            let top_a = h_a.floor() as u32;
            let frac_a = h_a - top_a as f32;
            let cy_a = a
                .get(LocalCoord::new(255, top_a, lz).expect("in bounds"))
                .corner()
                .to_components()[1];
            assert!(
                (cy_a - frac_a).abs() <= tol,
                "cluster A col 255 z={lz} corner_y={cy_a} != frac {frac_a}"
            );

            // Cluster B's -X boundary column. Local lx = 0, world x = 256 + 0.5.
            let h_b = world_height_seeded(256.5, world_z, seed);
            let top_b = h_b.floor() as u32;
            let frac_b = h_b - top_b as f32;
            let cy_b = b
                .get(LocalCoord::new(0, top_b, lz).expect("in bounds"))
                .corner()
                .to_components()[1];
            assert!(
                (cy_b - frac_b).abs() <= tol,
                "cluster B col 0  z={lz} corner_y={cy_b} != frac {frac_b}"
            );

            // The heightmap is Lipschitz-continuous so heights at world
            // x=255.5 and 256.5 (one voxel apart) cannot differ by much.
            // The Step 1 continuity test bounded per-0.05-step delta at
            // 0.5 voxels; per-1-voxel delta is therefore < 10 voxels in
            // the worst case. A 10-voxel jump over one voxel is a steep
            // cliff but legal under the wave parameters — assert a
            // loose 20-voxel bound that confirms we're in the same band.
            assert!(
                (h_a - h_b).abs() < 20.0,
                "seam discontinuity at z={lz}: h_A={h_a}, h_B={h_b}"
            );
        }
    }
}
