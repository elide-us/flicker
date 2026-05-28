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
}
