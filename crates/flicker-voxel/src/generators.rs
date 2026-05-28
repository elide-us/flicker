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
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::voxel::Voxel;

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
