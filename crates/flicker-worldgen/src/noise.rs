//! The `Vec3` face of the engine's one lattice noise.
//!
//! The implementation lives in [`flicker_primitive::noise`] — moved there so the
//! world-gen kernels and the texture synthesizer share a single lattice hash
//! rather than each carrying their own. This module is only the shim that spends
//! a `Vec3` on the scalar signature, because the epoch kernels sample with
//! direction vectors and would otherwise spell out three components at every
//! call. It holds no arithmetic of its own: change the field here and nothing
//! moves; change it in `flicker-primitive` and both consumers follow.

use glam::Vec3;

pub use flicker_primitive::noise::{billow, contrast, ridged};

/// Trilinearly-interpolated value noise at `p`, in `[0, 1)`. `salt` selects an
/// independent field (e.g. a per-element field keyed by atomic number).
pub fn value_noise(p: Vec3, salt: u64, seed: u64) -> f64 {
    flicker_primitive::noise::value3(p.x as f64, p.y as f64, p.z as f64, salt, seed)
}

/// Fractional Brownian motion: `octaves` of [`value_noise`] at doubling frequency
/// and halving amplitude, normalized back to `[0, 1)`.
pub fn fbm(p: Vec3, octaves: u32, salt: u64, seed: u64) -> f64 {
    flicker_primitive::noise::fbm3(p.x as f64, p.y as f64, p.z as f64, octaves, salt, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shim must land on the shared implementation, not drift from it.
    #[test]
    fn delegates_to_the_shared_lattice() {
        let p = Vec3::new(0.3, -1.7, 2.1);
        assert_eq!(
            value_noise(p, 7, 42).to_bits(),
            flicker_primitive::noise::value3(
                0.3_f32 as f64,
                -1.7_f32 as f64,
                2.1_f32 as f64,
                7,
                42
            )
            .to_bits()
        );
    }

    #[test]
    fn deterministic_and_in_range() {
        let p = Vec3::new(0.3, -1.7, 2.1);
        let a = value_noise(p, 7, 42);
        assert_eq!(a.to_bits(), value_noise(p, 7, 42).to_bits());
        for &v in &[a, fbm(p, 4, 7, 42)] {
            assert!((0.0..1.0).contains(&v), "noise {v} out of [0,1)");
        }
    }

    #[test]
    fn varies_across_space_salt_and_seed() {
        let p = Vec3::new(0.3, -1.7, 2.1);
        assert_ne!(
            value_noise(p, 1, 1),
            value_noise(p + Vec3::splat(1.3), 1, 1)
        );
        assert_ne!(value_noise(p, 1, 1), value_noise(p, 2, 1));
        assert_ne!(value_noise(p, 1, 1), value_noise(p, 1, 2));
    }

    #[test]
    fn fbm_is_spatially_smooth() {
        let p = Vec3::new(4.0, 1.0, -2.0);
        let d = (fbm(p + Vec3::new(0.01, 0.0, 0.0), 4, 9, 3) - fbm(p, 4, 9, 3)).abs();
        assert!(d < 0.05, "fbm jumped {d} over a 0.01 step");
    }
}
