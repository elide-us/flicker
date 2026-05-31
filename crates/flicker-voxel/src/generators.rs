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

/// Generate a cluster filled entirely with one material.
///
/// Equivalent to [`Cluster::uniform`] with a named purpose: this is
/// useful when the cluster is meant to act as the *neighbor* of
/// another cluster being contoured (so its base material participates
/// in the seam's classification) rather than as a renderable cluster
/// in its own right — contouring a uniform cluster on its own
/// produces an empty mesh because there's no surface.
#[must_use]
pub fn uniform_filled(material: Material) -> Cluster {
    Cluster::uniform(Voxel::new(CornerVector::DEFAULT, material))
}

/// Generate a cluster representing a noisy heightfield: for each
/// `(x, z)` column, voxels at `y < height(x, z)` are solid. The
/// per-column height is `base_height + jitter`, where `jitter` is a
/// deterministic hash-based pseudo-random offset in
/// `[-amplitude, +amplitude]`. The final height is clamped to
/// `[0, CLUSTER_DIM]`.
///
/// The hash uses a small fixed-multiplier scheme — fast, no external
/// dependencies, repeatable across runs. Not cryptographically
/// random; just enough variation to produce visible bumps on the
/// surface. For smoothly continuous terrain (Perlin/simplex) wire up
/// a noise crate at the layer above.
///
/// `base_height` is the central Y of the surface in voxels.
/// `amplitude` is the maximum +/- deviation in voxels.
/// `seed` lets the caller request different terrains from the same
/// parameters.
///
/// # Performance
///
/// Same characteristics as [`solid_slab`] — walks every solid voxel
/// and inserts it. At `base_height = 128, amplitude = 0`, identical
/// cost (~8M inserts). Octree storage will fix this in a later phase.
#[must_use]
pub fn noisy_terrain(base_height: u32, amplitude: u32, seed: u32, material: Material) -> Cluster {
    let mut c = Cluster::empty();
    if base_height == 0 && amplitude == 0 {
        return c;
    }
    let v = Voxel::new(CornerVector::DEFAULT, material);
    for z in 0..CLUSTER_DIM {
        for x in 0..CLUSTER_DIM {
            let h = column_height(x, z, base_height, amplitude, seed);
            for y in 0..h {
                c.set(LocalCoord::new(x, y, z).expect("in bounds"), v);
            }
        }
    }
    c
}

/// Compute the per-column height for [`noisy_terrain`].
///
/// Deterministic integer-hash of `(x, z, seed)`, mixed via a small
/// fold-and-multiply scheme. The low bits of the hash are folded into
/// the range `[0, 2 * amplitude]` and shifted by `-amplitude` to
/// produce an offset in `[-amplitude, +amplitude]`. The final result
/// is clamped to `[0, CLUSTER_DIM]`.
#[must_use]
pub fn column_height(x: u32, z: u32, base_height: u32, amplitude: u32, seed: u32) -> u32 {
    let mut h = x
        .wrapping_mul(0x9E3779B1)
        .wrapping_add(z.wrapping_mul(0x85EBCA77))
        .wrapping_add(seed.wrapping_mul(0xC2B2AE3D));
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846CA68B);
    h ^= h >> 16;

    let range = amplitude.saturating_mul(2).saturating_add(1);
    let offset = (h % range) as i32 - amplitude as i32;
    let computed = base_height as i32 + offset;
    computed.clamp(0, CLUSTER_DIM as i32) as u32
}

/// Generate a cluster representing a smoothly perturbed terrain: a
/// flat half-slab whose surface bumps come from **corner-vector
/// perturbation**, not classification variation. Every column is
/// solid up to the same integer `base_height`; the visible bumps come
/// entirely from joining-point displacement on the boundary layer.
///
/// Voxels at `y == base_height - 1` (the topmost solid layer) have
/// their corner-vector Y component perturbed by a coherent-noise hash
/// of `(x, z, seed)`. All other voxels carry [`CornerVector::DEFAULT`].
///
/// # Why this looks smooth
///
/// A surface quad's four corners are joins owned by four different
/// voxels (the join-point model). Perturbing one voxel's Y join by
/// `δ` shifts that join upward by `δ`. The mesh vertex computed from
/// the cell containing that join as one of its 8 corners shifts up
/// by `δ / 8`. With **coherent** noise across many voxels, adjacent
/// mesh vertices shift up together — producing visible smooth
/// undulation. With per-voxel-random noise (as in this crate's
/// `lattice_hash_2d` alone), adjacent shifts are uncorrelated and
/// produce high-frequency wobble that's hard to see at default scale.
///
/// `amplitude` is the maximum Y-perturbation in voxel units (`1.0`
/// means a join can move up by one full voxel above its default
/// position). Practical range: `0.0..=0.9`; going higher approaches
/// the `+1.5` clamp limit.
///
/// # Performance
///
/// Same as [`solid_slab`] — walks every solid voxel and inserts an
/// override. Slow at half-height (~8M inserts); tracked under the
/// octree-storage TODO.
#[must_use]
pub fn smooth_terrain(base_height: u32, amplitude: f32, seed: u32, material: Material) -> Cluster {
    let h = base_height.min(CLUSTER_DIM);
    let mut c = Cluster::empty();
    if h == 0 {
        return c;
    }
    let default_voxel = Voxel::new(CornerVector::DEFAULT, material);

    // Default-decoded Y component for the corner vector's "natural"
    // position. The corner vector encodes a byte in [0, 255] mapping
    // to [-0.5, +1.5]; byte 128 decodes to (128/255)*2 - 0.5 ≈ 0.5039.
    let default_y = (128.0_f32 / 255.0) * 2.0 - 0.5;

    let boundary_y = h - 1;
    for z in 0..CLUSTER_DIM {
        for y in 0..h {
            for x in 0..CLUSTER_DIM {
                if y == boundary_y {
                    let noise = column_noise(x, z, seed);
                    let perturbation = noise * amplitude;
                    let new_y = (default_y + perturbation).clamp(-0.5, 1.5);
                    let voxel =
                        Voxel::new(CornerVector::from_components(0.5, new_y, 0.5), material);
                    c.set(LocalCoord::new(x, y, z).expect("in bounds"), voxel);
                } else {
                    c.set(LocalCoord::new(x, y, z).expect("in bounds"), default_voxel);
                }
            }
        }
    }
    c
}

/// Coherent noise for a `(x, z)` column, returning approximately
/// `[-1.0, +1.0]`. Blends a fine-grained and a coarse-grained lattice
/// noise — adjacent voxels often fall inside the same coarse bucket,
/// producing smooth variation across neighbors.
fn column_noise(x: u32, z: u32, seed: u32) -> f32 {
    let fine_noise = lattice_noise_2d(x, z, seed, 8);
    let coarse_noise = lattice_noise_2d(x, z, seed.wrapping_add(0x9E37_79B1), 32);
    0.6 * fine_noise + 0.4 * coarse_noise
}

/// Bilinearly-interpolated noise sampled on a lattice with grid
/// spacing `step` voxels. Returns approximately `[-1.0, +1.0]`.
/// Smoothstep weights produce visually smooth transitions across
/// lattice cells.
fn lattice_noise_2d(x: u32, z: u32, seed: u32, step: u32) -> f32 {
    let step_f = step as f32;
    let xi = x / step;
    let zi = z / step;
    let fx = (x % step) as f32 / step_f;
    let fz = (z % step) as f32 / step_f;

    let n00 = lattice_hash_2d(xi, zi, seed);
    let n10 = lattice_hash_2d(xi + 1, zi, seed);
    let n01 = lattice_hash_2d(xi, zi + 1, seed);
    let n11 = lattice_hash_2d(xi + 1, zi + 1, seed);

    let wx = smoothstep(fx);
    let wz = smoothstep(fz);
    let nx0 = n00 * (1.0 - wx) + n10 * wx;
    let nx1 = n01 * (1.0 - wx) + n11 * wx;
    nx0 * (1.0 - wz) + nx1 * wz
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Hash a 2D integer lattice point to a float in approximately
/// `[-1.0, +1.0]`.
fn lattice_hash_2d(x: u32, z: u32, seed: u32) -> f32 {
    let mut h = x
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(z.wrapping_mul(0x85EB_CA77))
        .wrapping_add(seed.wrapping_mul(0xC2B2_AE3D));
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Generate a cluster containing a random scattering of spherical
/// solid blobs floating in empty space. The cluster's solid voxels
/// are the **union** of all blobs — a voxel is solid iff it lies
/// inside any blob.
///
/// Each blob is a sphere with a deterministic hash-derived center and
/// radius. Blob centers are placed in `[max_radius, CLUSTER_DIM -
/// max_radius)` on each axis so every blob is fully interior to the
/// cluster — its surface closes completely with no clipping against
/// the cluster faces. Overlapping blobs merge into a single connected
/// solid (correct union behavior — the merged surface is a non-
/// spherical blob, not two separate spheres).
///
/// `blob_count` is the number of spheres. `min_radius`/`max_radius`
/// (voxels) bound each blob's size. `seed` controls placement; the
/// same arguments always produce the same cluster.
///
/// # Why this is the key surface test
///
/// Unlike the height-field generators (where the surface only appears
/// on top of each column), floating blobs require the contour
/// algorithm to close a surface on every side of each solid region —
/// top, bottom, left, right, front, back. Correct output is a set of
/// closed 2-manifolds with outward-pointing normals everywhere. This
/// is the canonical "does surface contouring actually work" fixture.
///
/// # Performance
///
/// Walks each blob's bounding box doing an O(radius³) containment
/// test. For `blob_count` blobs of average radius `r`, that's
/// O(`blob_count` · r³) checks plus the resulting HashMap insertions.
/// Slow but fine for one-shot fixture setup. Octree storage is
/// tracked elsewhere.
#[must_use]
pub fn random_blobs(
    blob_count: u32,
    min_radius: u32,
    max_radius: u32,
    seed: u32,
    material: Material,
) -> Cluster {
    let mut c = Cluster::empty();
    if blob_count == 0 || max_radius == 0 {
        return c;
    }
    let v = Voxel::new(CornerVector::DEFAULT, material);
    let min_r = min_radius.min(max_radius);
    let max_r = max_radius;

    let half = CLUSTER_DIM / 2;
    let margin = max_r.min(half.saturating_sub(1));
    let center_lo = margin;
    let center_hi = CLUSTER_DIM - margin;

    for blob_idx in 0..blob_count {
        let cx = hash_in_range(seed, blob_idx, 0, center_lo, center_hi);
        let cy = hash_in_range(seed, blob_idx, 1, center_lo, center_hi);
        let cz = hash_in_range(seed, blob_idx, 2, center_lo, center_hi);
        let r = hash_in_range(seed, blob_idx, 3, min_r, max_r + 1);
        add_solid_sphere(&mut c, cx as i32, cy as i32, cz as i32, r as i32, v);
    }
    c
}

/// Set every voxel inside the sphere centered at `(cx, cy, cz)` with
/// integer radius `r` to `voxel`. Voxels outside the cluster are
/// skipped (the sphere is clipped against the cluster bounds).
///
/// Shared by [`random_blobs`] and [`island_world`] for floating-blob
/// placement. O(r³) inside-sphere check; deterministic.
fn add_solid_sphere(cluster: &mut Cluster, cx: i32, cy: i32, cz: i32, r: i32, voxel: Voxel) {
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

/// Generate a stepped "island world" fixture: a composed height field
/// (sea floor + island dome − lake bowl + volcano cone − crater bowl)
/// filled per column, plus floating cloud blobs in the upper region.
///
/// All joins are [`CornerVector::DEFAULT`] — terrain is purely
/// classification-driven and therefore stepped (one flat quad per
/// voxel layer, vertical side faces at every elevation change). The
/// sharp steps make terraces, the lake's interior walls, and the
/// crater concavity maximally visible. A later slope-smoothing pass
/// will lean boundary joins into ramps.
///
/// Terrain placement is hardcoded; only the cloud placement varies
/// with `seed`.
///
/// # Performance
///
/// Fills up to ~half the cluster volume in solid voxels. Slow under
/// HashMap storage; tracked under the octree-storage TODO.
#[must_use]
pub fn island_world(seed: u32, material: Material) -> Cluster {
    let mut c = Cluster::empty();
    let v = Voxel::new(CornerVector::DEFAULT, material);

    // Terrain: per-column height field.
    for z in 0..CLUSTER_DIM {
        for x in 0..CLUSTER_DIM {
            let h = island_height(x, z);
            for y in 0..h {
                c.set(LocalCoord::new(x, y, z).expect("in bounds"), v);
            }
        }
    }

    // Clouds: 6 puffy blobs in the upper region. Each cloud is a
    // cluster of 2..=4 overlapping spheres so it looks puffy rather
    // than perfectly round. The Y band [200, 240] sits well above the
    // tallest terrain feature (volcano rim ~110), so clouds float in
    // open air.
    let cloud_count: u32 = 6;
    let edge_margin: u32 = 30;
    for cloud_idx in 0..cloud_count {
        let cx = hash_in_range(seed, cloud_idx, 10, edge_margin, CLUSTER_DIM - edge_margin);
        let cy = hash_in_range(seed, cloud_idx, 11, 200, 241);
        let cz = hash_in_range(seed, cloud_idx, 12, 30, CLUSTER_DIM - edge_margin);
        let puff_count = hash_in_range(seed, cloud_idx, 13, 2, 5); // 2..=4
        let puff_seed = seed.wrapping_add(cloud_idx.wrapping_mul(0x9E37_79B1));
        for puff_idx in 0..puff_count {
            let ox = hash_in_range(puff_seed, puff_idx, 20, 0, 17) as i32 - 8;
            let oy = hash_in_range(puff_seed, puff_idx, 21, 0, 13) as i32 - 6;
            let oz = hash_in_range(puff_seed, puff_idx, 22, 0, 17) as i32 - 8;
            let r = hash_in_range(puff_seed, puff_idx, 23, 8, 17) as i32;
            add_solid_sphere(&mut c, cx as i32 + ox, cy as i32 + oy, cz as i32 + oz, r, v);
        }
    }

    c
}

/// Per-column terrain height for [`island_world`]. Pure, deterministic.
/// Composes: sea floor + island dome − lake bowl + volcano cone −
/// crater bowl, clamped to `[0, CLUSTER_DIM - 1]`.
fn island_height(x: u32, z: u32) -> u32 {
    let xf = x as f32;
    let zf = z as f32;

    // Sea floor — every column solid up to this height.
    let sea_floor = 20.0_f32;
    // Central island dome at (128, 128), radius 90, peak ~70 (quadratic
    // → domed).
    let dome = radial_falloff(xf, zf, 128.0, 128.0, 90.0, 70.0, FalloffShape::Quadratic);
    // Lake depression at (100, 150), radius 35, depth ~40 (quadratic
    // → bowled).
    let lake = radial_falloff(xf, zf, 100.0, 150.0, 35.0, 40.0, FalloffShape::Quadratic);
    // Volcano cone at (165, 110), radius 45, peak ~90 (linear → conical).
    let volcano = radial_falloff(xf, zf, 165.0, 110.0, 45.0, 90.0, FalloffShape::Linear);
    // Crater bowl at the volcano center, radius 14, depth ~50.
    let crater = radial_falloff(xf, zf, 165.0, 110.0, 14.0, 50.0, FalloffShape::Quadratic);

    let h = sea_floor + dome - lake + volcano - crater;
    h.clamp(0.0, (CLUSTER_DIM - 1) as f32).round() as u32
}

#[derive(Copy, Clone)]
enum FalloffShape {
    /// `peak * (1 - d/r)²` — domed/bowled.
    Quadratic,
    /// `peak * (1 - d/r)` — conical.
    Linear,
}

/// Radially-symmetric falloff contribution centered at `(cx, cz)`.
/// Returns `0.0` outside `radius`; `peak` at the center; the named
/// falloff shape in between. Always non-negative — callers add it
/// (mound) or subtract it (depression).
fn radial_falloff(
    x: f32,
    z: f32,
    cx: f32,
    cz: f32,
    radius: f32,
    peak: f32,
    shape: FalloffShape,
) -> f32 {
    let dx = x - cx;
    let dz = z - cz;
    let dist = (dx * dx + dz * dz).sqrt();
    if dist >= radius {
        return 0.0;
    }
    let t = 1.0 - dist / radius; // 1 at center → 0 at rim
    match shape {
        FalloffShape::Quadratic => peak * t * t,
        FalloffShape::Linear => peak * t,
    }
}

/// Generate a cluster from the world heightmap function at the world
/// origin. Equivalent to [`heightmap_terrain_at`] with a zero offset;
/// preserved as the single-cluster convenience the Step 2 example used.
#[must_use]
pub fn heightmap_terrain(seed: u64, material: Material) -> Cluster {
    heightmap_terrain_at(seed, material, [0.0, 0.0, 0.0])
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
/// different world-Y positions. The contour pass averages these
/// corners across cells, so the cell-center vertex moves smoothly
/// between adjacent cells and the rendered surface follows the
/// continuous height function as a tilted quad rather than a stepped
/// terrace. This is the same join-placement principle [`smooth_terrain`]
/// applies to incoherent per-voxel noise, applied here to a coherent
/// continuous function — and that is what turns the wedge-and-ramp
/// insight into visible ramps.
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
    use crate::contour::contour_cluster;

    fn solid_material() -> Material {
        Material::new(7, 7, 7).unwrap()
    }

    #[test]
    fn solid_slab_zero_height_is_empty() {
        let m = solid_material();
        let c = solid_slab(0, m);
        // No overrides — equivalent to `Cluster::empty()`.
        assert_eq!(c.override_count(), 0);
        let mesh = contour_cluster(&c);
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
        let mesh = contour_cluster(&c);
        assert!(
            !mesh.is_empty(),
            "half-height slab should produce a surface"
        );
        for v in mesh.vertices() {
            assert!(
                (v.position[1] - 128.0).abs() < 0.6,
                "slab vertex Y={} not near 128.0",
                v.position[1]
            );
        }
    }

    #[test]
    fn solid_slab_full_height_produces_empty_mesh() {
        // Entire cluster solid — uniform classification, no surface.
        let c = solid_slab(CLUSTER_DIM, solid_material());
        let mesh = contour_cluster(&c);
        assert!(
            mesh.is_empty(),
            "fully-solid cluster should have no surface"
        );
    }

    #[test]
    fn uniform_filled_has_no_overrides_but_base_material() {
        let m = solid_material();
        let c = uniform_filled(m);
        assert_eq!(c.override_count(), 0);
        assert_eq!(c.base().material(), m);
        // Uniform clusters have no surface on their own.
        let mesh = contour_cluster(&c);
        assert!(mesh.is_empty());
    }

    // ---- noisy_terrain ----

    #[test]
    fn noisy_terrain_zero_params_is_empty() {
        let c = noisy_terrain(0, 0, 0, solid_material());
        assert_eq!(c.override_count(), 0);
        assert!(contour_cluster(&c).is_empty());
    }

    #[test]
    fn noisy_terrain_zero_amplitude_equals_solid_slab() {
        // With amplitude = 0, every column has height = base_height.
        // The output should equal `solid_slab(base_height, ...)`.
        let m = solid_material();
        let flat = solid_slab(128, m);
        let noisy = noisy_terrain(128, 0, 0xDEAD_BEEF, m);
        assert_eq!(flat.override_count(), noisy.override_count());
    }

    #[test]
    fn noisy_terrain_is_deterministic_for_same_seed() {
        let m = solid_material();
        let a = noisy_terrain(128, 8, 42, m);
        let b = noisy_terrain(128, 8, 42, m);
        assert_eq!(a.override_count(), b.override_count());
    }

    #[test]
    fn noisy_terrain_seeds_differ() {
        // Different seeds should (with overwhelming probability) yield
        // a different total override count over a 256² column grid.
        let m = solid_material();
        let a = noisy_terrain(128, 8, 1, m);
        let b = noisy_terrain(128, 8, 2, m);
        assert_ne!(a.override_count(), b.override_count());
    }

    #[test]
    fn noisy_terrain_surface_y_in_amplitude_band() {
        // Contour the terrain and check every vertex's Y sits within
        // the band [base - amplitude - margin, base + amplitude +
        // margin]. The half-voxel margin accounts for centroid
        // placement at cell centers.
        let m = solid_material();
        let base = 128u32;
        let amp = 8u32;
        let cluster = noisy_terrain(base, amp, 12345, m);
        let mesh = contour_cluster(&cluster);
        assert!(!mesh.is_empty());
        let lo = base as f32 - amp as f32 - 1.0;
        let hi = base as f32 + amp as f32 + 1.0;
        for v in mesh.vertices() {
            assert!(
                v.position[1] >= lo && v.position[1] <= hi,
                "vertex Y={} outside band [{lo}, {hi}]",
                v.position[1]
            );
        }
    }

    #[test]
    fn column_height_within_clamped_range() {
        // Every (x, z) column's height stays inside [0, CLUSTER_DIM].
        for (x, z, base, amp, seed) in [
            (0u32, 0u32, 0u32, 0u32, 0u32),
            (0, 0, 128, 8, 0),
            (255, 255, 128, 64, 0xCAFE),
            (100, 100, 250, 32, 0xBABE),
        ] {
            let h = column_height(x, z, base, amp, seed);
            assert!(h <= CLUSTER_DIM, "height {h} > CLUSTER_DIM");
        }
    }

    // ---- smooth_terrain ----

    #[test]
    fn smooth_terrain_zero_height_is_empty() {
        let m = solid_material();
        let c = smooth_terrain(0, 0.5, 42, m);
        assert_eq!(c.override_count(), 0);
    }

    #[test]
    fn smooth_terrain_zero_amplitude_matches_solid_slab() {
        // With amplitude 0 the boundary layer's perturbation is zero
        // and every voxel uses the default Y component. The contoured
        // mesh should be byte-identical to `solid_slab`'s; the
        // override count may differ (smooth_terrain always sets the
        // boundary layer explicitly even with the default vector),
        // but vertex positions match exactly.
        let m = solid_material();
        let smooth = smooth_terrain(128, 0.0, 12345, m);
        let slab = solid_slab(128, m);

        let m_smooth = contour_cluster(&smooth);
        let m_slab = contour_cluster(&slab);

        assert_eq!(
            m_smooth.metadata().vertex_count,
            m_slab.metadata().vertex_count
        );
        assert_eq!(
            m_smooth.metadata().triangle_count,
            m_slab.metadata().triangle_count
        );
        for (vs, vsl) in m_smooth.vertices().iter().zip(m_slab.vertices().iter()) {
            for axis in 0..3 {
                assert_eq!(
                    vs.position[axis].to_bits(),
                    vsl.position[axis].to_bits(),
                    "mismatch at axis {axis}"
                );
            }
        }
    }

    #[test]
    fn smooth_terrain_amplitude_produces_y_variation() {
        let m = solid_material();
        let c = smooth_terrain(128, 0.5, 42, m);
        let mesh = contour_cluster(&c);
        assert!(!mesh.is_empty());

        // Top-surface vertices: y near 128, dominant +Y normal.
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for v in mesh.vertices() {
            if v.normal[1] > 0.8 {
                min_y = min_y.min(v.position[1]);
                max_y = max_y.max(v.position[1]);
            }
        }
        let range = max_y - min_y;
        assert!(
            range > 0.05,
            "expected top-surface Y to span > 0.05; got {range} (min={min_y}, max={max_y})"
        );
        assert!(
            range < 0.5,
            "expected top-surface Y range to stay moderate; got {range} (min={min_y}, max={max_y})"
        );
    }

    #[test]
    fn smooth_terrain_is_deterministic_per_seed() {
        let m = solid_material();
        let a = smooth_terrain(128, 0.5, 42, m);
        let b = smooth_terrain(128, 0.5, 42, m);
        let ma = contour_cluster(&a);
        let mb = contour_cluster(&b);
        assert_eq!(ma.vertices(), mb.vertices());
    }

    #[test]
    fn smooth_terrain_seeds_produce_different_meshes() {
        let m = solid_material();
        let a = smooth_terrain(128, 0.5, 1, m);
        let b = smooth_terrain(128, 0.5, 2, m);
        let ma = contour_cluster(&a);
        let mb = contour_cluster(&b);
        // Same topology (classifications match between seeds), but
        // vertex positions should differ for many vertices.
        assert_eq!(ma.metadata().vertex_count, mb.metadata().vertex_count);
        let mut differing = 0;
        for (va, vb) in ma.vertices().iter().zip(mb.vertices().iter()) {
            if (va.position[1] - vb.position[1]).abs() > 1e-5 {
                differing += 1;
            }
        }
        assert!(
            differing > 100,
            "expected many vertices to differ between seeds; got {differing}"
        );
    }

    // ---- random_blobs ----

    #[test]
    fn random_blobs_zero_count_is_empty() {
        let c = random_blobs(0, 5, 10, 42, solid_material());
        assert_eq!(c.override_count(), 0);
        assert!(contour_cluster(&c).is_empty());
    }

    #[test]
    fn random_blobs_zero_max_radius_is_empty() {
        let c = random_blobs(5, 0, 0, 42, solid_material());
        assert_eq!(c.override_count(), 0);
    }

    #[test]
    fn random_blobs_is_deterministic_per_seed() {
        let m = solid_material();
        let a = random_blobs(10, 8, 16, 42, m);
        let b = random_blobs(10, 8, 16, 42, m);
        let ma = contour_cluster(&a);
        let mb = contour_cluster(&b);
        assert_eq!(ma.vertices(), mb.vertices());
        assert_eq!(ma.indices(), mb.indices());
    }

    #[test]
    fn random_blobs_different_seeds_differ() {
        let m = solid_material();
        let a = random_blobs(10, 8, 16, 1, m);
        let b = random_blobs(10, 8, 16, 2, m);
        assert_ne!(a.override_count(), b.override_count());
    }

    #[test]
    fn random_blobs_interior_produces_closed_mesh() {
        // THE key surface-contouring test: blobs fully interior to the
        // cluster produce a closed 2-manifold — every triangle edge
        // used by exactly 2 triangles. This is the strongest
        // correctness check available for arbitrary-topology surface
        // contouring and is what proves the algorithm closes surfaces
        // around isolated solids on every side.
        let m = solid_material();
        let cluster = random_blobs(8, 16, 24, 0xCAFE_BABE, m);
        let mesh = contour_cluster(&cluster);
        assert!(!mesh.is_empty(), "8 blobs should produce a non-empty mesh");

        use std::collections::HashMap;
        let to_u32 = |indices: &crate::contour::Indices, i: usize| -> u32 {
            match indices {
                crate::contour::Indices::U16(v) => v[i] as u32,
                crate::contour::Indices::U32(v) => v[i],
            }
        };
        let indices = mesh.indices();
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        let mut i = 0;
        while i < indices.len() {
            let a = to_u32(indices, i);
            let b = to_u32(indices, i + 1);
            let cc = to_u32(indices, i + 2);
            for (x, y) in [(a, b), (b, cc), (cc, a)] {
                let k = if x < y { (x, y) } else { (y, x) };
                *counts.entry(k).or_insert(0) += 1;
            }
            i += 3;
        }
        let bad: Vec<_> = counts.iter().filter(|(_, n)| **n != 2).collect();
        assert!(
            bad.is_empty(),
            "expected closed mesh; {} edges not used by exactly 2 triangles \
             (vertices={}, triangles={})",
            bad.len(),
            mesh.metadata().vertex_count,
            mesh.metadata().triangle_count,
        );
    }

    // ---- island_world ----

    #[test]
    fn island_world_is_deterministic() {
        let m = solid_material();
        let a = island_world(42, m);
        let b = island_world(42, m);
        let ma = contour_cluster(&a);
        let mb = contour_cluster(&b);
        assert_eq!(ma.vertices(), mb.vertices());
        assert_eq!(ma.indices(), mb.indices());
    }

    #[test]
    fn island_world_produces_large_mesh() {
        let m = solid_material();
        let mesh = contour_cluster(&island_world(42, m));
        assert!(!mesh.is_empty());
        assert!(
            mesh.metadata().vertex_count > 10_000,
            "expected a substantial mesh; got {} vertices",
            mesh.metadata().vertex_count
        );
    }

    #[test]
    fn island_height_relief_ordering() {
        // Sea-floor corner (away from all features) is low.
        let corner = island_height(5, 5);
        assert!(corner < 30, "sea-floor corner {corner} should be ~20");
        // Island dome center is well above the sea floor.
        let dome = island_height(128, 128);
        assert!(
            dome > corner + 30,
            "dome {dome} not above sea floor {corner}"
        );
    }

    #[test]
    fn island_lake_is_a_depression() {
        let lake = island_height(100, 150);
        let land = island_height(128, 128);
        assert!(lake < land, "lake {lake} should be below dome {land}");
    }

    #[test]
    fn island_volcano_crater_is_a_depression() {
        let crater = island_height(165, 110); // crater floor
        let rim = island_height(165, 110 - 13); // just inside the rim
        assert!(
            crater < rim,
            "crater floor {crater} should be below rim {rim}"
        );
    }

    #[test]
    fn island_volcano_rim_is_tallest() {
        // The volcano rim should out-rank the island dome center.
        let rim = island_height(165, 110 - 13);
        let dome = island_height(128, 128);
        assert!(rim > dome, "volcano rim {rim} should exceed dome {dome}");
    }

    #[test]
    fn island_clouds_exist_above_terrain() {
        // Terrain peaks well below y=150; any solid voxel at y>=150 is
        // a cloud (the volcano rim tops out around ~110).
        let m = solid_material();
        let c = island_world(42, m);
        let cloud_voxels = c
            .overrides()
            .filter(|(coord, v)| coord.y() >= 150 && v.material() != Material::EMPTY)
            .count();
        assert!(cloud_voxels > 0, "expected cloud voxels above y=150");
    }

    // ---- heightmap_terrain ----

    fn corner_y_of(c: &Cluster, x: u32, y: u32, z: u32) -> f32 {
        c.get(LocalCoord::new(x, y, z).expect("in bounds"))
            .corner()
            .to_components()[1]
    }

    #[test]
    fn heightmap_terrain_is_deterministic() {
        let m = solid_material();
        let a = heightmap_terrain(0xDEAD_BEEF_CAFE, m);
        let b = heightmap_terrain(0xDEAD_BEEF_CAFE, m);
        assert_eq!(a.override_count(), b.override_count());
        // Spot-check a few coordinates: same voxel value in both.
        for &(x, y, z) in &[(0u32, 0u32, 0u32), (128, 100, 128), (200, 50, 30)] {
            let coord = LocalCoord::new(x, y, z).expect("in bounds");
            assert_eq!(a.get(coord), b.get(coord));
        }
    }

    #[test]
    fn heightmap_terrain_top_voxel_join_matches_fractional_part() {
        // For each of several columns the topmost solid voxel's corner-Y
        // (decoded from its byte) should equal the heightmap's fractional
        // part at that column's center, within the encoding's per-axis
        // round-trip error of 1/255.
        let seed = 0xABCD_1234_5678u64;
        let m = solid_material();
        let c = heightmap_terrain(seed, m);

        let tolerance = 1.0 / 255.0 + 1e-6;
        for &(x, z) in &[(5u32, 5u32), (50, 50), (128, 128), (200, 30), (250, 250)] {
            let h = world_height_seeded(x as f32 + 0.5, z as f32 + 0.5, seed);
            let top_y = h.floor() as u32;
            let fractional = h - top_y as f32;

            let actual = corner_y_of(&c, x, top_y, z);
            assert!(
                (actual - fractional).abs() <= tolerance,
                "column ({x},{z}): top corner_y={actual}, want ≈ {fractional} (h={h}, top_y={top_y})"
            );
        }
    }

    #[test]
    fn heightmap_terrain_lower_voxels_use_default_corner() {
        // Voxels strictly below the topmost solid layer must carry the
        // default corner. Sample several columns and inspect a voxel at
        // y = top_y - 4 (well below the boundary layer).
        let seed = 0x1357_2468_ACE0u64;
        let m = solid_material();
        let c = heightmap_terrain(seed, m);

        for &(x, z) in &[(10u32, 10u32), (100, 100), (200, 50), (240, 240)] {
            let h = world_height_seeded(x as f32 + 0.5, z as f32 + 0.5, seed);
            let top_y = h.floor() as u32;
            assert!(
                top_y >= 4,
                "expected top_y >= 4 for column ({x},{z}); got {top_y}"
            );
            let probe_y = top_y - 4;
            let voxel = c.get(LocalCoord::new(x, probe_y, z).expect("in bounds"));
            assert_eq!(
                voxel.corner(),
                CornerVector::DEFAULT,
                "voxel ({x},{probe_y},{z}) carries non-default corner"
            );
            assert_ne!(
                voxel.material(),
                Material::EMPTY,
                "voxel ({x},{probe_y},{z}) should be solid but is empty"
            );
        }
    }

    #[test]
    fn heightmap_terrain_no_solid_above_top() {
        // Voxels strictly above the topmost solid layer must be empty.
        let seed = 0x9999_AAAA_BBBBu64;
        let m = solid_material();
        let c = heightmap_terrain(seed, m);

        for &(x, z) in &[(0u32, 0u32), (60, 90), (128, 128), (255, 255)] {
            let h = world_height_seeded(x as f32 + 0.5, z as f32 + 0.5, seed);
            let top_y = h.floor() as u32;
            let above = top_y + 1;
            if above >= CLUSTER_DIM {
                continue;
            }
            let voxel = c.get(LocalCoord::new(x, above, z).expect("in bounds"));
            assert_eq!(
                voxel.material(),
                Material::EMPTY,
                "voxel ({x},{above},{z}) above the surface should be empty"
            );
        }
    }

    #[test]
    fn heightmap_terrain_produces_renderable_mesh() {
        // Sanity end-to-end: the generator -> contour pipeline should
        // produce a substantial mesh for the wave-field surface.
        let m = solid_material();
        let mesh = contour_cluster(&heightmap_terrain(0x42, m));
        assert!(!mesh.is_empty());
        assert!(
            mesh.metadata().vertex_count > 10_000,
            "expected a substantial mesh; got {} vertices",
            mesh.metadata().vertex_count
        );
    }

    // ---- heightmap_terrain_at ----

    #[test]
    fn heightmap_terrain_at_zero_offset_equals_heightmap_terrain() {
        // The Step 2 convenience wrapper must remain byte-equivalent to
        // an explicit zero-offset call.
        let m = solid_material();
        let seed = 0xDEAD_BEEF_BABE_F00D;
        let zero = heightmap_terrain_at(seed, m, [0.0, 0.0, 0.0]);
        let convenience = heightmap_terrain(seed, m);
        assert_eq!(zero.override_count(), convenience.override_count());
        // Spot-check a handful of voxels.
        for &(x, y, z) in &[(0u32, 0u32, 0u32), (128, 100, 128), (200, 50, 30)] {
            let coord = LocalCoord::new(x, y, z).expect("in bounds");
            assert_eq!(zero.get(coord), convenience.get(coord));
        }
    }

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

    #[test]
    fn random_blobs_normals_point_outward() {
        // For a single isolated blob, every surface vertex's normal
        // should point away from the blob center (outward). This
        // verifies the contour algorithm orients normals correctly on
        // a fully-surrounded solid, not just on the top of a height
        // field.
        let m = solid_material();
        let cluster = random_blobs(1, 30, 30, 7, m);
        let mesh = contour_cluster(&cluster);
        assert!(!mesh.is_empty());

        // Mesh centroid as a proxy for the blob center.
        let mut center = [0.0_f64; 3];
        for v in mesh.vertices() {
            center[0] += v.position[0] as f64;
            center[1] += v.position[1] as f64;
            center[2] += v.position[2] as f64;
        }
        let n = mesh.vertices().len() as f64;
        let center = [
            (center[0] / n) as f32,
            (center[1] / n) as f32,
            (center[2] / n) as f32,
        ];

        let mut outward = 0usize;
        let total = mesh.vertices().len();
        for v in mesh.vertices() {
            let radial = [
                v.position[0] - center[0],
                v.position[1] - center[1],
                v.position[2] - center[2],
            ];
            let dot = v.normal[0] * radial[0] + v.normal[1] * radial[1] + v.normal[2] * radial[2];
            if dot > 0.0 {
                outward += 1;
            }
        }
        let ratio = outward as f64 / total as f64;
        assert!(
            ratio > 0.9,
            "expected >90% of normals to point outward; got {:.1}% ({outward}/{total})",
            ratio * 100.0
        );
    }
}
