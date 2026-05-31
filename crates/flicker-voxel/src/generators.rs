//! Cluster generators — small builder functions that produce
//! [`Cluster`] instances with simple, well-known content.
//!
//! These are prototype-grade — useful for spinning up a renderable
//! cluster in a few lines of test or example code. They are **slow**
//! when the resulting cluster has many overrides because the current
//! sparse storage uses a `HashMap`; a future octree-backed cluster
//! will make these O(1) in storage. For now, prefer them only when
//! a small handful of clusters need to exist.
//!
//! See the module-level doc on [`crate::cluster`] for the storage
//! trade-off discussion this references.

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::corner_vector::CornerVector;
use crate::heightmap::world_height_seeded;
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::voxel::Voxel;

/// Demo material indices for the scene generators. **STUB** — there
/// is no real material/contents system yet; these are stable indices
/// the shader's `material_index_color` switches on. Replace when the
/// real material system lands.
///
/// Indices `1..=5` cover the water depth band (deep → mid → shallow →
/// crest → foam). Indices `6..=8` cover the cloud body band (dark →
/// mid → light). Index `9` is cirrus.
pub mod demo_materials {
    pub const DEEP_WATER: u16 = 1;
    pub const MID_WATER: u16 = 2;
    pub const SHALLOW: u16 = 3;
    pub const CREST: u16 = 4;
    pub const FOAM: u16 = 5;
    pub const CLOUD_DARK: u16 = 6;
    pub const CLOUD_MID: u16 = 7;
    pub const CLOUD_LIGHT: u16 = 8;
    pub const CIRRUS: u16 = 9;
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
    let v = Voxel::new(CornerVector::DEFAULT, material);
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
    let solid_default = Voxel::new(CornerVector::DEFAULT, material);
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
                let m = Material::new(p, s, b).expect("demo indices in range");
                c.set(
                    LocalCoord::new(x, y, z).expect("in bounds"),
                    Voxel::new(CornerVector::DEFAULT, m),
                );
            }

            // Topmost layer: corner Y positioned to express the
            // fractional part of h (matches `heightmap_terrain_at`).
            let t_top = ((top_y as f32 - BAND_LO) / BAND_SPAN).clamp(0.0, 1.0);
            let (p, s, b) = water_material_at(t_top);
            let m = Material::new(p, s, b).expect("demo indices in range");
            let top_voxel = if capped {
                Voxel::new(CornerVector::DEFAULT, m)
            } else {
                let fractional = h - top_y as f32;
                Voxel::new(CornerVector::from_components(0.5, fractional, 0.5), m)
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
fn water_material_at(t: f32) -> (u16, u16, u8) {
    use demo_materials::*;
    // Segment edges over `t`. Each entry is `(edge, primary, secondary)`
    // — within `[prev_edge, edge)`, the voxel interpolates from
    // `primary` to `secondary`.
    let segments: [(f32, u16, u16); 4] = [
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

/// Map a normalized vertical position `t ∈ [0, 1]` within a cloud's
/// vertical band to `(primary, secondary, blend_byte)`. `t = 0` is
/// the cloud's flat anvil base (dark underbelly); `t = 1` is the
/// sunlit crown.
fn cloud_material_at(t: f32) -> (u16, u16, u8) {
    use demo_materials::*;
    let segments: [(f32, u16, u16); 2] = [
        (0.30, CLOUD_DARK, CLOUD_MID),
        (0.70, CLOUD_MID, CLOUD_LIGHT),
    ];
    let mut prev_edge = 0.0_f32;
    for (edge, lo_idx, hi_idx) in segments {
        if t < edge {
            let local_t = ((t - prev_edge) / (edge - prev_edge)).clamp(0.0, 1.0);
            return (lo_idx, hi_idx, (local_t * 255.0) as u8);
        }
        prev_edge = edge;
    }
    (demo_materials::CLOUD_LIGHT, demo_materials::CLOUD_LIGHT, 0)
}

/// Stamp a single sphere into `cluster` with per-voxel materials
/// resolved by a caller-supplied closure. Voxels outside the cluster
/// are skipped. Default corner vector throughout — the per-voxel
/// material gives the cloud its gradient, no joint perturbation.
///
/// `material_for(world_y)` is called once per stamped voxel; for
/// cloud generators it picks the right band via [`cloud_material_at`].
fn add_solid_sphere_with<F>(
    cluster: &mut Cluster,
    cx: i32,
    cy: i32,
    cz: i32,
    r: i32,
    mut material_for: F,
) where
    F: FnMut(i32) -> Material,
{
    if r <= 0 {
        return;
    }
    let r2 = r * r;
    let lo = |c: i32| (c - r).max(0);
    let hi = |c: i32| (c + r).min(CLUSTER_DIM as i32 - 1);

    for z in lo(cz)..=hi(cz) {
        let dz = z - cz;
        let dz2 = dz * dz;
        if dz2 > r2 {
            continue;
        }
        for y in lo(cy)..=hi(cy) {
            let dy = y - cy;
            let dy2 = dy * dy;
            if dy2 + dz2 > r2 {
                continue;
            }
            let remaining = r2 - dy2 - dz2;
            let m = material_for(y);
            let voxel = Voxel::new(CornerVector::DEFAULT, m);
            for x in lo(cx)..=hi(cx) {
                let dx = x - cx;
                if dx * dx <= remaining {
                    cluster.set(
                        LocalCoord::new(x as u32, y as u32, z as u32).expect("in bounds"),
                        voxel,
                    );
                }
            }
        }
    }
}

/// Stamp a cumulonimbus-shaped cloud into `cluster`. The cloud sits
/// in a vertical region `[base_y, top_y]` centered horizontally at
/// `(cx, cz)`, with overall horizontal scale `max_radius`.
///
/// Structure (in order of stamping):
///   1. Trunk: 4–8 large overlapping spheres along the central
///      vertical axis, radius ≈ `0.6 * max_radius`.
///   2. Body: 8–16 medium spheres scattered within a vertically-
///      biased ellipsoid, radius ≈ `0.3 * max_radius`.
///   3. Cauliflower top: 6–10 smaller spheres above ~70% height,
///      radius ≈ `0.2 * max_radius`, spread out to ~`1.4 * max_radius`.
///   4. Anvil base clip: every solid voxel below `base_y` within
///      the cloud's bounding column is reset to empty, giving the
///      sharp flat underside.
///
/// Material is assigned per voxel by normalized vertical position
/// within `[base_y, top_y]` via [`cloud_material_at`]. `seed` controls
/// sphere placement deterministically.
pub fn cumulonimbus_at(
    cluster: &mut Cluster,
    seed: u32,
    cx: i32,
    cz: i32,
    base_y: i32,
    top_y: i32,
    max_radius: i32,
) {
    if top_y <= base_y || max_radius <= 0 {
        return;
    }
    let band_span = (top_y - base_y) as f32;
    let material_for = |world_y: i32| -> Material {
        let t = ((world_y - base_y) as f32 / band_span).clamp(0.0, 1.0);
        let (p, s, b) = cloud_material_at(t);
        Material::new(p, s, b).expect("demo indices in range")
    };

    let trunk_r = (max_radius as f32 * 0.6).round() as i32;
    let body_r = (max_radius as f32 * 0.3).round() as i32;
    let top_r = (max_radius as f32 * 0.2).round() as i32;

    // ---- Trunk: 4..=8 spheres climbing the central axis. ----
    let trunk_count = hash_in_range(seed, 0, 0, 4, 9); // [4, 9) = 4..=8
    for i in 0..trunk_count {
        // Distribute trunk centers across the lower 70% of the band.
        let frac = if trunk_count > 1 {
            i as f32 / (trunk_count - 1) as f32
        } else {
            0.5
        };
        let y = base_y + (frac * band_span * 0.7) as i32;
        // Small lateral jitter so the trunk isn't a perfect vertical column.
        let ox = hash_in_range(seed, i, 1, 0, 7) as i32 - 3;
        let oz = hash_in_range(seed, i, 2, 0, 7) as i32 - 3;
        let r = trunk_r + hash_in_range(seed, i, 3, 0, 4) as i32 - 1;
        add_solid_sphere_with(cluster, cx + ox, y, cz + oz, r, material_for);
    }

    // ---- Body: 8..=15 mid-radius spheres scattered through the trunk band. ----
    let body_count = hash_in_range(seed, 0, 10, 8, 16);
    for i in 0..body_count {
        let ox = hash_in_range(seed, i, 11, 0, (max_radius as u32) + 1) as i32 - (max_radius / 2);
        let oz = hash_in_range(seed, i, 12, 0, (max_radius as u32) + 1) as i32 - (max_radius / 2);
        // Body fills [10%, 75%] of the band.
        let y_frac = 0.10 + hash_in_range(seed, i, 13, 0, 1001) as f32 / 1000.0 * 0.65;
        let y = base_y + (y_frac * band_span) as i32;
        let r = body_r + hash_in_range(seed, i, 14, 0, 4) as i32 - 1;
        add_solid_sphere_with(cluster, cx + ox, y, cz + oz, r, material_for);
    }

    // ---- Cauliflower top: 6..=9 small spheres above 70% height, spread wider. ----
    let top_count = hash_in_range(seed, 0, 20, 6, 10);
    let spread = (max_radius as f32 * 1.4) as i32;
    for i in 0..top_count {
        let ox = hash_in_range(seed, i, 21, 0, (2 * spread as u32) + 1) as i32 - spread;
        let oz = hash_in_range(seed, i, 22, 0, (2 * spread as u32) + 1) as i32 - spread;
        // Top fills [0.7, 1.0] of the band.
        let y_frac = 0.70 + hash_in_range(seed, i, 23, 0, 1001) as f32 / 1000.0 * 0.30;
        let y = base_y + (y_frac * band_span) as i32;
        let r = top_r + hash_in_range(seed, i, 24, 0, 3) as i32 - 1;
        add_solid_sphere_with(cluster, cx + ox, y, cz + oz, r, material_for);
    }

    // ---- Anvil base clip: clear every *cloud* voxel below `base_y`
    // in the cloud's horizontal footprint, producing the sharp flat
    // underside. We restrict the clear to cloud-band materials so we
    // do not punch a hole through the water surface or other terrain
    // that happens to share the column underneath the cloud.
    // The Y range only needs to cover where the trunk's lowest sphere
    // could reach: `base_y - trunk_r` upward. The footprint is the
    // cauliflower spread plus the trunk radius — comfortably larger
    // than any sphere reaches.
    let footprint = spread + trunk_r;
    let x_lo = (cx - footprint).max(0) as u32;
    let x_hi = (cx + footprint).min(CLUSTER_DIM as i32 - 1) as u32;
    let z_lo = (cz - footprint).max(0) as u32;
    let z_hi = (cz + footprint).min(CLUSTER_DIM as i32 - 1) as u32;
    let y_clip_top = base_y.min(CLUSTER_DIM as i32 - 1).max(0) as u32;
    // Trunk and body spheres can extend roughly `trunk_r + 1` below
    // base_y. Clip a bit further to be safe.
    let y_clip_bottom = (base_y - trunk_r - 2).max(0) as u32;
    let empty = Voxel::EMPTY;
    for z in z_lo..=z_hi {
        for y in y_clip_bottom..y_clip_top {
            for x in x_lo..=x_hi {
                let coord = LocalCoord::new(x, y, z).expect("in bounds");
                let primary = cluster.get(coord).material().primary();
                if matches!(
                    primary,
                    demo_materials::CLOUD_DARK
                        | demo_materials::CLOUD_MID
                        | demo_materials::CLOUD_LIGHT
                ) {
                    cluster.set(coord, empty);
                }
            }
        }
    }
}

/// Stamp a thin elongated cirrus wisp into `cluster`. A series of
/// small spheres along a roughly horizontal axis from `start` to
/// `end`, with a gentle sinusoidal lateral curl. Single material
/// (`CIRRUS`) throughout — no banding, no blending — so the wisp
/// reads as a uniform pale streak.
pub fn cirrus_wisp_at(
    cluster: &mut Cluster,
    seed: u32,
    start: [i32; 3],
    end: [i32; 3],
    thickness: i32,
) {
    if thickness <= 0 {
        return;
    }
    let cirrus = Material::new(demo_materials::CIRRUS, demo_materials::CIRRUS, 0)
        .expect("demo indices in range");
    let cirrus_for = |_y: i32| -> Material { cirrus };

    // 10 sphere stamps along the parametric line.
    let stamp_count: u32 = 10;
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let dz = end[2] - start[2];

    // Choose a curl-perpendicular axis in the XZ plane: rotate the
    // (dx, dz) projection 90° so the wisp curls sideways, not up/down.
    let perp_x = -dz as f32;
    let perp_z = dx as f32;
    let perp_len = (perp_x * perp_x + perp_z * perp_z).sqrt().max(1.0);
    let perp_x = perp_x / perp_len;
    let perp_z = perp_z / perp_len;

    for i in 0..stamp_count {
        let t = i as f32 / (stamp_count - 1).max(1) as f32;
        // Curl amplitude: a few voxels, modulated along the wisp.
        let curl = (t * std::f32::consts::PI * 2.0).sin() * (thickness as f32 * 1.5);
        let x = start[0] + (dx as f32 * t) as i32 + (perp_x * curl) as i32;
        let y = start[1] + (dy as f32 * t) as i32;
        let z = start[2] + (dz as f32 * t) as i32 + (perp_z * curl) as i32;
        // Per-stamp radius jitter for organic thickness variation.
        let jitter = hash_in_range(seed, i, 0, 0, 3) as i32 - 1;
        let r = (thickness + jitter).max(1);
        add_solid_sphere_with(cluster, x, y, z, r, cirrus_for);
    }
}

/// Deterministic uniform integer hash in `[lo, hi)`, keyed by a
/// `(seed, blob_idx, param_idx)` triple. Returns `lo` if `hi <= lo`.
fn hash_in_range(seed: u32, blob_idx: u32, param_idx: u32, lo: u32, hi: u32) -> u32 {
    if hi <= lo {
        return lo;
    }
    let range = hi - lo;
    let mut h = seed
        .wrapping_mul(0xC2B2_AE3D)
        .wrapping_add(blob_idx.wrapping_mul(0x9E37_79B1))
        .wrapping_add(param_idx.wrapping_mul(0x85EB_CA77));
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    lo + (h % range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contour_surface::contour_surface;

    fn solid_material() -> Material {
        Material::new(7, 7, 7).unwrap()
    }

    #[test]
    fn solid_slab_zero_height_is_empty() {
        let m = solid_material();
        let c = solid_slab(0, m);
        // No overrides — equivalent to `Cluster::empty()`.
        assert_eq!(c.override_count(), 0);
        let mesh = contour_surface(&c);
        assert!(mesh.is_empty());
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

    #[test]
    fn solid_slab_half_height_produces_surface_near_128() {
        let c = solid_slab(128, solid_material());
        let mesh = contour_surface(&c);
        assert!(
            !mesh.is_empty(),
            "half-height slab should produce a surface"
        );
        // Find a +Y-facing vertex on the top face and assert its Y is
        // at the top-voxel's owned corner (≈ 127.5).
        let mut found_top = false;
        for v in mesh.vertices() {
            if v.normal[1] > 0.9 {
                assert!(
                    (v.position[1] - 127.5).abs() < 0.05,
                    "top-face vertex Y={} not near 127.5",
                    v.position[1]
                );
                found_top = true;
            }
        }
        assert!(found_top, "expected at least one +Y-facing top vertex");
    }

    #[test]
    fn uniform_solid_cluster_produces_empty_mesh() {
        // A `Cluster::uniform` with a solid base has no internal
        // classification difference; contour_surface's early-exit
        // returns an empty mesh. (A solid_slab at full height fills
        // overrides on an empty base, which still has classification
        // boundaries at the cluster walls — handled elsewhere.)
        let base = Voxel::new(CornerVector::DEFAULT, solid_material());
        let c = Cluster::uniform(base);
        let mesh = contour_surface(&c);
        assert!(
            mesh.is_empty(),
            "uniform-solid cluster should have no internal surface"
        );
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
