//! World heightmap — the world's persistent surface representation, in
//! functional form.
//!
//! The engine's data model treats a single continuous function
//! `(world_x, world_z) -> elevation` as the **truth** about the world's
//! surface. Anything that needs to know how tall the ground is at a
//! given world point — a generator materializing a cluster, an LOD
//! aggregator, an erosion step — samples this function. Because it is
//! one continuous function, adjacent consumers (e.g., two neighboring
//! clusters) that sample the same world coordinates agree exactly, with
//! zero coordination. That is the property the upcoming multi-cluster
//! rendering work relies on for automatic seam continuity.
//!
//! This module provides only the continuous function. A later step will
//! add a cluster-resolution storage layer (the "Russian doll" pixel per
//! cluster) that *samples* this function at startup and that erosion
//! eventually mutates; that storage is not part of Step 1.
//!
//! # The wave field
//!
//! Rather than the round blobs of plain Perlin/Simplex, the field is a
//! Navier-Stokes-*inspired* wave summation: a superposition of
//! directional sinusoids at octave-spaced frequencies, **domain-warped**
//! by a pair of low-frequency waves. The warp advects the coordinate
//! grid the way a flow field would, so the ridge lines (where the
//! summed waves crest) meander into flow-aligned ridges and
//! drainage-like valleys instead of isotropic bumps. Each octave is
//! *ridged* (`1 − |sin|`) so crests are sharp ridgelines and the
//! between-ridge basins read as drainage.
//!
//! The summation is normalized so its value is in `[-1, 1]`, mapped to
//! voxel heights in `[BASE_HEIGHT − AMPLITUDE, BASE_HEIGHT + AMPLITUDE]`
//! ≈ `[64, 192]` — centered in a 256-voxel cluster with headroom above
//! and below.
//!
//! # Seeding
//!
//! The field is fully determined by a `u64` seed: identical seeds
//! reproduce the world byte-for-byte. [`world_height`] uses a global
//! default seeded once from the `FLICKER_SEED` environment variable
//! (falling back to [`DEFAULT_SEED`]); [`world_height_seeded`] takes an
//! explicit seed and touches no environment — it is pure and the basis
//! for all tests.

use std::sync::OnceLock;

use crate::cluster::CLUSTER_DIM;

/// Seed used when `FLICKER_SEED` is unset or unparseable.
pub const DEFAULT_SEED: u64 = 0xCAFE_F00D_D15E_A5E5;

/// Center of the output height band, in voxels.
const BASE_HEIGHT: f32 = 128.0;

/// Maximum deviation from [`BASE_HEIGHT`], in voxels. Base 128 ±64 puts
/// the surface in `[64, 192]` within a 256-voxel cluster.
const AMPLITUDE: f32 = 64.0;

/// Wavelength of the coarsest (octave 0) terrain wave, in voxels.
/// Picked so the dominant ridge spacing spans roughly two clusters.
const BASE_WAVELENGTH: f64 = (CLUSTER_DIM as f64) * 2.0;

/// Number of octaves summed. Each successive octave halves the
/// wavelength and is weighted down by [`PERSISTENCE`].
const OCTAVES: usize = 4;

/// Per-octave amplitude falloff. Octave `i` is weighted `PERSISTENCE^i`
/// before the weights are renormalized to sum to 1.
const PERSISTENCE: f64 = 0.55;

/// Domain-warp displacement strength, in voxels. The sample coordinate
/// is pushed by up to this much along each axis by the warp waves —
/// this is what bends straight ridgelines into flow-aligned curves.
const WARP_STRENGTH: f64 = BASE_WAVELENGTH * 0.15;

/// One directional sinusoid: `sin(kx·x + kz·z + phase)`, contributing
/// `amp` to the (normalized) summation.
#[derive(Copy, Clone, Debug)]
struct Wave {
    kx: f64,
    kz: f64,
    phase: f64,
    amp: f64,
}

impl Wave {
    #[inline]
    fn sin_at(&self, x: f64, z: f64) -> f64 {
        (self.kx * x + self.kz * z + self.phase).sin()
    }
}

/// The continuous procedural surface function — a domain-warped,
/// ridged, multi-octave directional wave sum. Built once from a seed
/// (cheap, all stack-allocated) and sampled per world point.
#[derive(Copy, Clone, Debug)]
struct WaveField {
    waves: [Wave; OCTAVES],
    warp_x: Wave,
    warp_z: Wave,
}

impl WaveField {
    fn from_seed(seed: u64) -> Self {
        let mut state = seed;
        let tau = std::f64::consts::TAU;

        let mut waves = [Wave {
            kx: 0.0,
            kz: 0.0,
            phase: 0.0,
            amp: 0.0,
        }; OCTAVES];
        let mut amp_sum = 0.0;
        for (i, w) in waves.iter_mut().enumerate() {
            let wavelength = BASE_WAVELENGTH / (1u64 << i) as f64;
            let k = tau / wavelength;
            let theta = splitmix01(&mut state) * tau;
            let phase = splitmix01(&mut state) * tau;
            let amp = PERSISTENCE.powi(i as i32);
            amp_sum += amp;
            *w = Wave {
                kx: k * theta.cos(),
                kz: k * theta.sin(),
                phase,
                amp,
            };
        }
        for w in &mut waves {
            w.amp /= amp_sum;
        }

        // Warp waves: longer wavelength than the coarsest terrain octave
        // so the advection is large-scale and smooth.
        let warp_k = tau / (BASE_WAVELENGTH * 2.0);
        let theta_x = splitmix01(&mut state) * tau;
        let phase_x = splitmix01(&mut state) * tau;
        let warp_x = Wave {
            kx: warp_k * theta_x.cos(),
            kz: warp_k * theta_x.sin(),
            phase: phase_x,
            amp: 1.0,
        };
        let theta_z = splitmix01(&mut state) * tau;
        let phase_z = splitmix01(&mut state) * tau;
        let warp_z = Wave {
            kx: warp_k * theta_z.cos(),
            kz: warp_k * theta_z.sin(),
            phase: phase_z,
            amp: 1.0,
        };

        Self {
            waves,
            warp_x,
            warp_z,
        }
    }

    /// Continuous field value in `[-1, 1]` at world `(x, z)`.
    fn value(&self, x: f64, z: f64) -> f64 {
        // Domain warp: advect the sample point by the low-frequency
        // waves. This is what turns isotropic ridges into flow-aligned
        // ones.
        let xw = x + WARP_STRENGTH * self.warp_x.sin_at(x, z);
        let zw = z + WARP_STRENGTH * self.warp_z.sin_at(x, z);

        let mut v = 0.0;
        for w in &self.waves {
            // Ridged: crest (1) where the wave crosses zero, trough (-1)
            // at its extremes. Amp-weighted; amps sum to 1 so v ∈ [-1, 1].
            let ridged = 1.0 - w.sin_at(xw, zw).abs();
            v += w.amp * (2.0 * ridged - 1.0);
        }
        v
    }

    /// Sample the field at world `(x, z)`, mapping to voxel height. The
    /// clamp is a defensive bound against float-rounding in the
    /// amplitude normalization; analytically the value is already in
    /// range.
    #[inline]
    fn sample(&self, x: f32, z: f32) -> f32 {
        let v = self.value(x as f64, z as f64) as f32;
        (BASE_HEIGHT + AMPLITUDE * v).clamp(BASE_HEIGHT - AMPLITUDE, BASE_HEIGHT + AMPLITUDE)
    }
}

/// Surface elevation in voxel units at world `(x, z)`, using the
/// process-wide default field. The default seed is taken once from
/// `FLICKER_SEED` (decimal or `0x`-prefixed hex) on first call, falling
/// back to [`DEFAULT_SEED`].
///
/// Always finite and in `[BASE_HEIGHT − AMPLITUDE, BASE_HEIGHT +
/// AMPLITUDE]` ≈ `[64, 192]`.
#[must_use]
pub fn world_height(x: f32, z: f32) -> f32 {
    default_field().sample(x, z)
}

/// Surface elevation in voxel units at world `(x, z)`, with an explicit
/// seed. Pure: touches no environment and depends only on its inputs.
/// Identical `(x, z, seed)` always returns the identical bit pattern.
///
/// Each call builds a `WaveField` (fixed-size, stack-allocated, ~a few
/// dozen float ops) and samples it. Callers doing dense evaluation with
/// the *same* seed can use [`world_height`] (which caches the default
/// field) for the common case, or are free to evaluate many points
/// without worrying — construction is cheap enough that it does not
/// dominate.
#[must_use]
pub fn world_height_seeded(x: f32, z: f32, seed: u64) -> f32 {
    WaveField::from_seed(seed).sample(x, z)
}

/// The world seed from the `FLICKER_SEED` environment variable, or
/// [`DEFAULT_SEED`] if it is unset or unparseable. This is the only
/// environment access in the module; [`world_height_seeded`] takes an
/// explicit seed and bypasses it entirely.
#[must_use]
pub fn seed_from_env() -> u64 {
    parse_seed(std::env::var("FLICKER_SEED").ok())
}

/// Process-wide default wave field, lazily built on first call from
/// [`seed_from_env`]. The seed snapshot is taken once per process —
/// changing `FLICKER_SEED` after the first call has no effect, which
/// matches the "one persistent world per run" intent.
fn default_field() -> &'static WaveField {
    static FIELD: OnceLock<WaveField> = OnceLock::new();
    FIELD.get_or_init(|| WaveField::from_seed(seed_from_env()))
}

/// Parse a seed from an optional environment string, falling back to
/// [`DEFAULT_SEED`] when absent or unparseable. Accepts decimal or
/// `0x`-prefixed hex. Split out from [`seed_from_env`] so the parsing
/// is testable without mutating process environment.
fn parse_seed(var: Option<String>) -> u64 {
    let Some(s) = var else { return DEFAULT_SEED };
    let s = s.trim();
    let parsed = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
    } else {
        s.parse::<u64>()
    };
    parsed.unwrap_or(DEFAULT_SEED)
}

/// One step of splitmix64, returned as a `f64` in `[0, 1)`.
#[inline]
fn splitmix01(state: &mut u64) -> f64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const LO: f32 = BASE_HEIGHT - AMPLITUDE; // 64
    const HI: f32 = BASE_HEIGHT + AMPLITUDE; // 192

    #[test]
    fn same_seed_is_byte_identical() {
        for &(x, z) in &[
            (0.0_f32, 0.0_f32),
            (123.4, 456.7),
            (-200.5, 1280.0),
            (1e5, -1e5),
        ] {
            let a = world_height_seeded(x, z, 42);
            let b = world_height_seeded(x, z, 42);
            assert_eq!(a.to_bits(), b.to_bits(), "non-deterministic at ({x},{z})");
        }
    }

    #[test]
    fn different_seeds_differ_somewhere() {
        let mut differing = 0;
        for x in (0..2048u32).step_by(64) {
            for z in (0..2048u32).step_by(64) {
                if world_height_seeded(x as f32, z as f32, 1).to_bits()
                    != world_height_seeded(x as f32, z as f32, 2).to_bits()
                {
                    differing += 1;
                }
            }
        }
        assert!(
            differing > 100,
            "seeds 1 and 2 too similar: {differing} differing samples"
        );
    }

    #[test]
    fn heights_are_finite_and_in_band() {
        for x in (0..2560u32).step_by(8) {
            for z in (0..2560u32).step_by(8) {
                let y = world_height_seeded(x as f32, z as f32, 0xDEAD_BEEF);
                assert!(y.is_finite(), "non-finite height at ({x},{z})");
                assert!(
                    (LO..=HI).contains(&y),
                    "height {y} at ({x},{z}) outside [{LO}, {HI}]"
                );
            }
        }
    }

    #[test]
    fn field_is_continuous_at_fine_scale() {
        // A continuous function changes little under a small step. Walk
        // fine steps along X and Z and bound the per-step delta well
        // below the amplitude — proving there are no discontinuities.
        //
        // Lipschitz bound rationale: the highest-frequency wave has
        // wavelength BASE_WAVELENGTH / 2^(OCTAVES-1) = 512/8 = 64
        // voxels, so its gradient magnitude is at most 2π/64 ≈ 0.098
        // per voxel in field-value units. Domain warping multiplies the
        // effective gradient by at most (1 + WARP_STRENGTH * warp_k) ≈
        // 1 + 76.8 * 2π/1024 ≈ 1.47. Mapped through AMPLITUDE=64 the
        // worst-case slope is roughly 64 * 0.098 * 1.47 ≈ 9.2 voxels
        // per world voxel; a 0.05-voxel step therefore moves height by
        // at most ~0.46 voxels. We assert a comfortable bound of 0.5.
        let step = 0.05_f32;
        let mut max_delta = 0.0_f32;
        let mut x = 0.0_f32;
        let z = 333.0_f32;
        while x < 1024.0 {
            let d = (world_height_seeded(x + step, z, 7) - world_height_seeded(x, z, 7)).abs();
            max_delta = max_delta.max(d);
            x += step;
        }
        assert!(
            max_delta < 0.5,
            "max per-{step}-step delta along X = {max_delta} too large"
        );

        // Same along Z.
        let mut max_delta_z = 0.0_f32;
        let mut z = 0.0_f32;
        let x = 777.0_f32;
        while z < 1024.0 {
            let d = (world_height_seeded(x, z + step, 7) - world_height_seeded(x, z, 7)).abs();
            max_delta_z = max_delta_z.max(d);
            z += step;
        }
        assert!(
            max_delta_z < 0.5,
            "max per-{step}-step delta along Z = {max_delta_z} too large"
        );
    }

    #[test]
    fn no_seam_at_cluster_boundaries() {
        // The whole point of "one continuous function": there is no jump
        // at the world coordinate where two clusters meet. Sample just
        // inside each side of the x = CLUSTER_DIM boundary.
        let boundary = CLUSTER_DIM as f32;
        for z in (0..2560u32).step_by(37) {
            let zf = z as f32;
            let left = world_height_seeded(boundary - 0.01, zf, 99);
            let right = world_height_seeded(boundary + 0.01, zf, 99);
            assert!(
                (left - right).abs() < 0.1,
                "seam at x={boundary}, z={zf}: {left} vs {right}"
            );
        }
    }

    #[test]
    fn field_has_real_variation() {
        // Sanity: the field is not a constant plane. Across a region
        // there should be meaningful spread between min and max heights.
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for x in (0..2560u32).step_by(16) {
            for z in (0..2560u32).step_by(16) {
                let y = world_height_seeded(x as f32, z as f32, 2024);
                min = min.min(y);
                max = max.max(y);
            }
        }
        assert!(
            max - min > 20.0,
            "field spread {} too flat (min {min}, max {max})",
            max - min
        );
    }

    #[test]
    fn world_height_uses_default_seed_when_env_unset() {
        // `world_height` and `world_height_seeded(_, _, DEFAULT_SEED)`
        // must agree when `FLICKER_SEED` is unset (the test harness
        // does not set it). We can't reach into the OnceLock, but we
        // can check the surface equality.
        //
        // Skip the assertion if the test environment has set
        // FLICKER_SEED — in that case `world_height` is keyed on that
        // seed, not on DEFAULT_SEED.
        if std::env::var("FLICKER_SEED").is_ok() {
            return;
        }
        for &(x, z) in &[(0.0_f32, 0.0_f32), (100.0, 200.0), (-50.0, 300.0)] {
            assert_eq!(
                world_height(x, z).to_bits(),
                world_height_seeded(x, z, DEFAULT_SEED).to_bits(),
                "default seed mismatch at ({x},{z})"
            );
        }
    }

    // ---- seed parsing (env-independent) ----

    #[test]
    fn parse_seed_decimal() {
        assert_eq!(parse_seed(Some("12345".to_string())), 12345);
    }

    #[test]
    fn parse_seed_hex() {
        assert_eq!(parse_seed(Some("0xFF".to_string())), 255);
        assert_eq!(parse_seed(Some("0Xff".to_string())), 255);
    }

    #[test]
    fn parse_seed_whitespace_trimmed() {
        assert_eq!(parse_seed(Some("  777  ".to_string())), 777);
    }

    #[test]
    fn parse_seed_absent_or_garbage_is_default() {
        assert_eq!(parse_seed(None), DEFAULT_SEED);
        assert_eq!(parse_seed(Some("not-a-number".to_string())), DEFAULT_SEED);
        assert_eq!(parse_seed(Some("".to_string())), DEFAULT_SEED);
    }
}
