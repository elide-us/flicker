//! Minimal deterministic 3D value noise — the spatial-coherence primitive the
//! epoch kernels use for "smooth large-scale variation" and "correlated noise"
//! (world-gen spec, Epoch 1). No external noise crate: a splitmix64-style lattice
//! hash (the same idiom as `flicker_primitive::heightmap`) trilinearly
//! interpolated, then summed over octaves. Pure and seedable — identical
//! `(point, salt, seed)` always returns the identical value.

use glam::Vec3;

/// Hash three lattice integers + a per-field `salt` + the world `seed` to a
/// value in `[0, 1)`. splitmix64 finalizer over a mixed input.
fn hash(ix: i32, iy: i32, iz: i32, salt: u64, seed: u64) -> f64 {
    let mut z = seed
        ^ (ix as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iy as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (iz as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
        ^ salt.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Smoothstep `3t² − 2t³` — eases the lattice interpolation so the field has no
/// derivative discontinuities at cell boundaries.
fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Trilinearly-interpolated value noise at `p`, in `[0, 1)`. `salt` selects an
/// independent field (e.g. a per-element field keyed by atomic number).
pub fn value_noise(p: Vec3, salt: u64, seed: u64) -> f64 {
    let (px, py, pz) = (p.x as f64, p.y as f64, p.z as f64);
    let (ix, iy, iz) = (px.floor() as i32, py.floor() as i32, pz.floor() as i32);
    let (ux, uy, uz) = (
        smoothstep(px - px.floor()),
        smoothstep(py - py.floor()),
        smoothstep(pz - pz.floor()),
    );
    let c = |dx: i32, dy: i32, dz: i32| hash(ix + dx, iy + dy, iz + dz, salt, seed);
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), ux);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), ux);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), ux);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), ux);
    let y0 = lerp(x00, x10, uy);
    let y1 = lerp(x01, x11, uy);
    lerp(y0, y1, uz)
}

/// Fractional Brownian motion: `octaves` of [`value_noise`] at doubling
/// frequency and halving amplitude, normalized back to ~`[0, 1)`. Each octave
/// perturbs the salt so the bands don't align.
pub fn fbm(p: Vec3, octaves: u32, salt: u64, seed: u64) -> f64 {
    let (mut freq, mut amp, mut sum, mut norm) = (1.0f32, 1.0f64, 0.0f64, 0.0f64);
    for o in 0..octaves.max(1) {
        let osalt = salt ^ (o as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        sum += amp * value_noise(p * freq, osalt, seed);
        norm += amp;
        freq *= 2.0;
        amp *= 0.5;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.5
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let p = Vec3::new(0.3, -1.7, 2.1);
        let a = value_noise(p, 7, 42);
        let b = value_noise(p, 7, 42);
        assert_eq!(a.to_bits(), b.to_bits(), "value_noise not deterministic");
        for &v in &[a, fbm(p, 4, 7, 42)] {
            assert!((0.0..1.0).contains(&v), "noise {v} out of [0,1)");
        }
    }

    #[test]
    fn varies_across_space_salt_and_seed() {
        let p = Vec3::new(0.3, -1.7, 2.1);
        // Different position.
        assert_ne!(value_noise(p, 1, 1), value_noise(p + Vec3::splat(1.3), 1, 1));
        // Different salt (independent fields).
        assert_ne!(value_noise(p, 1, 1), value_noise(p, 2, 1));
        // Different seed.
        assert_ne!(value_noise(p, 1, 1), value_noise(p, 1, 2));
    }

    #[test]
    fn fbm_is_spatially_smooth() {
        // A tiny step changes the field only a little — the coherence the kernels
        // rely on for "smooth large-scale variation".
        let p = Vec3::new(4.0, 1.0, -2.0);
        let d = (fbm(p + Vec3::new(0.01, 0.0, 0.0), 4, 9, 3) - fbm(p, 4, 9, 3)).abs();
        assert!(d < 0.05, "fbm jumped {d} over a 0.01 step");
    }
}
