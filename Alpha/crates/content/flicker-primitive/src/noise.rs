//! Deterministic lattice noise — the shared spatial-coherence primitive.
//!
//! One implementation, two consumers. The world-gen epoch kernels sample it in
//! **3D** for "smooth large-scale variation" and correlated regional fields; the
//! texture synthesizer samples it in **2D** for material surfaces. It lives here,
//! beside [`crate::heightmap`], because this crate is where the engine's
//! continuous scalar fields live and it carries no math-crate dependency — the
//! signatures are plain scalars, so a caller with `Vec3` passes its components
//! and a caller with pixel coordinates passes floats.
//!
//! No external noise crate: a splitmix64-style lattice hash smoothstep-
//! interpolated, then summed over octaves. Pure and seedable — identical
//! `(point, salt, seed)` always returns the identical value, so a world or a
//! texture reproduces byte-for-byte from its seed.
//!
//! # Salt vs seed
//!
//! `seed` is the world/recipe seed — one knob the author turns. `salt` selects an
//! **independent field** under that seed (a per-element field keyed by atomic
//! number, a per-channel field keyed by channel index), so two fields of the same
//! world never correlate.
//!
//! # Tiling
//!
//! A `period`, measured in lattice cells, wraps the integer lattice: sampling `x`
//! across `[0, period)` yields a field where `f(x) == f(x + period)`. That is what
//! makes a texture swatch seamless, and it is a property of the lattice — not a
//! post-process blend, so there is no mirrored seam or blurred border. A `period`
//! of `0` disables the wrap, which is the unbounded field the world-gen kernels
//! sample.
//!
//! Octaves tile only when each octave's frequency is a **whole number** of
//! lattice wraps across the tile, so the tiled fBm snaps its per-octave frequency
//! to an integer rather than approximating. With the default lacunarity of 2 the
//! snap is exact and costs nothing.
//!
//! # Domain warping
//!
//! Warping (`n(x + w(x))`) preserves tiling whenever the warp field `w` shares the
//! period, because `w(x + P) = w(x)` leaves the composed argument shifted by
//! exactly `P`. Callers may warp tiled fields freely.

/// Golden-ratio odd constant — the per-octave salt stride, so an octave's lattice
/// never aligns with its neighbour's.
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// Hash three lattice integers + a per-field `salt` + the `seed` to a value in
/// `[0, 1)`. splitmix64 finalizer over a mixed input.
fn hash3(ix: i64, iy: i64, iz: i64, salt: u64, seed: u64) -> f64 {
    let mut z = seed
        ^ (ix as u64).wrapping_mul(GOLDEN)
        ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (iz as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
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

/// Wrap a lattice index onto `[0, period)`. A `period` of zero (or less) means
/// "do not wrap" — the unbounded field the world-gen kernels sample.
fn wrap(i: i64, period: i64) -> i64 {
    if period > 0 {
        i.rem_euclid(period)
    } else {
        i
    }
}

/// Split a coordinate into its lattice cell and its eased position within it.
fn split(v: f64) -> (i64, f64) {
    let f = v.floor();
    (f as i64, smoothstep(v - f))
}

// ───────────────────────────── 3D — the world-gen face ─────────────────────────

/// Trilinearly-interpolated 3D value noise at `(x, y, z)`, in `[0, 1)`.
pub fn value3(x: f64, y: f64, z: f64, salt: u64, seed: u64) -> f64 {
    let (ix, ux) = split(x);
    let (iy, uy) = split(y);
    let (iz, uz) = split(z);
    let c = |dx: i64, dy: i64, dz: i64| hash3(ix + dx, iy + dy, iz + dz, salt, seed);
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), ux);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), ux);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), ux);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), ux);
    lerp(lerp(x00, x10, uy), lerp(x01, x11, uy), uz)
}

/// Fractional Brownian motion over [`value3`]: `octaves` at doubling frequency and
/// halving amplitude, normalized back to `[0, 1)`. Each octave perturbs the salt so
/// the bands don't align.
pub fn fbm3(x: f64, y: f64, z: f64, octaves: u32, salt: u64, seed: u64) -> f64 {
    let (mut freq, mut amp, mut sum, mut norm) = (1.0f64, 1.0f64, 0.0f64, 0.0f64);
    for o in 0..octaves.max(1) {
        let osalt = salt ^ (o as u64).wrapping_mul(GOLDEN);
        sum += amp * value3(x * freq, y * freq, z * freq, osalt, seed);
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

// ───────────────────────────── 2D — the texture face ───────────────────────────

/// Bilinearly-interpolated 2D value noise at `(x, y)`, in `[0, 1)`, with the
/// lattice wrapped on `period` cells (`0` = unbounded). Four hashes per sample
/// rather than the 3D path's eight.
pub fn value2_tiled(x: f64, y: f64, period: i64, salt: u64, seed: u64) -> f64 {
    let (ix, ux) = split(x);
    let (iy, uy) = split(y);
    let c = |dx: i64, dy: i64| hash3(wrap(ix + dx, period), wrap(iy + dy, period), 0, salt, seed);
    let x0 = lerp(c(0, 0), c(1, 0), ux);
    let x1 = lerp(c(0, 1), c(1, 1), ux);
    lerp(x0, x1, uy)
}

/// [`value2_tiled`] with no wrap — the unbounded 2D field.
pub fn value2(x: f64, y: f64, salt: u64, seed: u64) -> f64 {
    value2_tiled(x, y, 0, salt, seed)
}

/// The octave settings a 2D fBm sums over.
///
/// A struct rather than four positional arguments because `lacunarity` and `gain`
/// are both bare `f64` in the same slot range: transposing them at a call site
/// compiles, and produces a field that is merely *different* rather than wrong —
/// the kind of bug that survives a long time. Named fields make the mistake
/// unwritable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fbm {
    /// How many octaves to sum. Clamped to at least 1.
    pub octaves: u32,
    /// Per-octave frequency step. 2.0 is the classic doubling.
    pub lacunarity: f64,
    /// Per-octave amplitude step. 0.5 is the classic halving.
    pub gain: f64,
    /// Lattice period in cells — the tile width. `0` means unbounded (no tiling).
    pub period: i64,
}

impl Default for Fbm {
    /// The classic untiled fBm: 4 octaves, doubling frequency, halving amplitude.
    fn default() -> Self {
        Self {
            octaves: 4,
            lacunarity: 2.0,
            gain: 0.5,
            period: 0,
        }
    }
}

impl Fbm {
    /// The classic fBm, tiled on `period` cells.
    pub fn tiled(period: i64) -> Self {
        Self {
            period,
            ..Self::default()
        }
    }
}

/// Fractional Brownian motion over [`value2_tiled`], in `[0, 1)`.
///
/// Because a tile only closes when an octave wraps a whole number of times across
/// it, the accumulated frequency is **snapped to an integer** (minimum 1) for the
/// lattice period — a non-integer lacunarity therefore quantizes rather than
/// tearing the seam.
pub fn fbm2(x: f64, y: f64, f: Fbm, salt: u64, seed: u64) -> f64 {
    let (mut freq, mut amp, mut sum, mut norm) = (1.0f64, 1.0f64, 0.0f64, 0.0f64);
    let lac = if f.lacunarity.is_finite() {
        f.lacunarity.max(1.0)
    } else {
        2.0
    };
    let gain = if f.gain.is_finite() {
        f.gain.clamp(0.0, 1.0)
    } else {
        0.5
    };
    for o in 0..f.octaves.max(1) {
        let step = freq.round().max(1.0);
        let osalt = salt ^ (o as u64).wrapping_mul(GOLDEN);
        sum += amp * value2_tiled(x * step, y * step, f.period * step as i64, osalt, seed);
        norm += amp;
        freq *= lac;
        amp *= gain;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.5
    }
}

/// Tiled cellular (Worley) noise: the distance from `(x, y)` to the nearest
/// feature point, one point jittered inside each lattice cell, normalized to
/// `[0, 1)`. Reads as cracked plates / pebbles / cell walls — the structure fBm
/// cannot make.
///
/// Cell indices are wrapped on `period`, so the feature field tiles with
/// everything else. The raw F1 distance is divided by the cell diagonal and
/// clamped, so a lone far-from-any-feature sample saturates at 1 rather than
/// running past it.
pub fn worley2_tiled(x: f64, y: f64, period: i64, salt: u64, seed: u64) -> f64 {
    let (cx, cy) = (x.floor(), y.floor());
    let (icx, icy) = (cx as i64, cy as i64);
    // Measure CELL-LOCALLY: the sample's offset inside its own cell, against each
    // feature point's offset from that same origin. Differencing absolute
    // coordinates instead would subtract two large nearly-equal numbers far from
    // the origin, and the cancellation is exactly what puts a visible seam at a
    // tile boundary — `6 − (6 + d)` does not return the `d` that `0 − (0 + d)` does.
    let (lx, ly) = (x - cx, y - cy);
    let mut best = f64::INFINITY;
    for dy in -1..=1i64 {
        for dx in -1..=1i64 {
            let (wx, wy) = (wrap(icx + dx, period), wrap(icy + dy, period));
            // Two independent jitters per cell, from one lattice with distinct z
            // planes — the same hash, no second constant table.
            let fx = dx as f64 + hash3(wx, wy, 0, salt, seed);
            let fy = dy as f64 + hash3(wx, wy, 1, salt, seed);
            let d2 = (lx - fx) * (lx - fx) + (ly - fy) * (ly - fy);
            if d2 < best {
                best = d2;
            }
        }
    }
    (best.sqrt() / std::f64::consts::SQRT_2).clamp(0.0, 1.0)
}

// ───────────────────────────────── shaping ─────────────────────────────────────

/// Fold a `[0, 1)` field about its midpoint into sharp crests: `1 − |2v − 1|`.
/// Ridge lines where the field crosses 0.5 — mountain crests, vein filaments,
/// cracked-rock relief.
pub fn ridged(v: f64) -> f64 {
    1.0 - (2.0 * v - 1.0).abs()
}

/// The complement of [`ridged`]: `|2v − 1|`, which pushes the field away from its
/// midpoint into rounded lobes — clouds, clumped soil, billowing rock.
pub fn billow(v: f64) -> f64 {
    (2.0 * v - 1.0).abs()
}

/// Push a `[0, 1)` field away from (or toward) its midpoint. `amount` of 1 is the
/// identity, above 1 hardens the field toward two-tone, below 1 flattens it to
/// grey. The result is clamped back into `[0, 1]`.
pub fn contrast(v: f64, amount: f64) -> f64 {
    ((v - 0.5) * amount + 0.5).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property every consumer relies on: same input, same bits, always.
    #[test]
    fn deterministic_and_in_range() {
        let a = value3(0.3, -1.7, 2.1, 7, 42);
        let b = value3(0.3, -1.7, 2.1, 7, 42);
        assert_eq!(a.to_bits(), b.to_bits(), "value3 not deterministic");
        for v in [
            a,
            fbm3(0.3, -1.7, 2.1, 4, 7, 42),
            value2(0.3, -1.7, 7, 42),
            fbm2(0.3, -1.7, Fbm::tiled(8), 7, 42),
            worley2_tiled(0.3, 1.7, 8, 7, 42),
        ] {
            assert!((0.0..=1.0).contains(&v), "noise {v} out of [0,1]");
        }
    }

    #[test]
    fn varies_across_space_salt_and_seed() {
        assert_ne!(value3(0.3, -1.7, 2.1, 1, 1), value3(1.6, -0.4, 3.4, 1, 1));
        assert_ne!(value3(0.3, -1.7, 2.1, 1, 1), value3(0.3, -1.7, 2.1, 2, 1));
        assert_ne!(value3(0.3, -1.7, 2.1, 1, 1), value3(0.3, -1.7, 2.1, 1, 2));
    }

    /// Spatial coherence — the whole reason this is lattice noise and not a hash.
    #[test]
    fn fbm_is_spatially_smooth() {
        let d = (fbm3(4.01, 1.0, -2.0, 4, 9, 3) - fbm3(4.0, 1.0, -2.0, 4, 9, 3)).abs();
        assert!(d < 0.05, "fbm3 jumped {d} over a 0.01 step");
    }

    /// THE tiling contract, asserted where a raster actually meets itself: the
    /// swatch's opposite edges. A baker walks `x = column · period / width`, so the
    /// wrap point is an EXACT multiple of the period — there the lattice cell is
    /// integral and its fraction is exactly zero, and the repeat is bit-identical.
    ///
    /// (Away from an integer multiple the repeat is mathematical rather than
    /// bit-exact: `1.3 + 6.0` and `1.3` carry different float fractions, so the
    /// interpolant lands an ULP apart. That is float representation, not a seam —
    /// [`tiled_fields_repeat_within_float_error`] holds it to an epsilon, and the
    /// raster-level guarantee is what this test pins.)
    #[test]
    fn tiled_fields_repeat_bit_exactly_at_the_seam() {
        let period = 6i64;
        let p = period as f64;
        for k in 0..period {
            let (x, y) = (k as f64, k as f64);
            assert_eq!(
                value2_tiled(x, y, period, 3, 11).to_bits(),
                value2_tiled(x + p, y, period, 3, 11).to_bits(),
                "value2_tiled x-seam at cell {k}"
            );
            assert_eq!(
                value2_tiled(x, y, period, 3, 11).to_bits(),
                value2_tiled(x, y + p, period, 3, 11).to_bits(),
                "value2_tiled y-seam at cell {k}"
            );
            for octaves in 1..=5 {
                assert_eq!(
                    fbm2(
                        x,
                        y,
                        Fbm {
                            octaves,
                            ..Fbm::tiled(period)
                        },
                        3,
                        11
                    )
                    .to_bits(),
                    fbm2(
                        x + p,
                        y + p,
                        Fbm {
                            octaves,
                            ..Fbm::tiled(period)
                        },
                        3,
                        11
                    )
                    .to_bits(),
                    "fbm2 seam at {octaves} octaves, cell {k}"
                );
            }
            assert_eq!(
                worley2_tiled(x, y, period, 3, 11).to_bits(),
                worley2_tiled(x + p, y, period, 3, 11).to_bits(),
                "worley2_tiled x-seam at cell {k}"
            );
        }
    }

    /// Interior points repeat too, to within float error — so the periodicity is a
    /// property of the whole field and not just of the sample points that happen to
    /// land on a lattice line.
    #[test]
    fn tiled_fields_repeat_within_float_error() {
        let period = 6i64;
        let p = period as f64;
        for (x, y) in [(1.3, 4.7), (5.9, 0.25), (2.5, 5.75)] {
            let near = |a: f64, b: f64, what: &str| {
                assert!((a - b).abs() < 1e-12, "{what}: {a} vs {b}");
            };
            near(
                value2_tiled(x, y, period, 3, 11),
                value2_tiled(x + p, y, period, 3, 11),
                "value2_tiled",
            );
            near(
                fbm2(x, y, Fbm::tiled(period), 3, 11),
                fbm2(x + p, y + p, Fbm::tiled(period), 3, 11),
                "fbm2",
            );
            near(
                worley2_tiled(x, y, period, 3, 11),
                worley2_tiled(x, y + p, period, 3, 11),
                "worley2_tiled",
            );
        }
    }

    /// A warped tiled field still tiles — the property that lets a channel warp
    /// its own domain without tearing the swatch edge.
    ///
    /// To float error, not bit-exactly: warping displaces the sample OFF the
    /// lattice line, so the two edges' fractional positions are computed from
    /// different magnitudes and land an ULP apart. Nothing survives quantization
    /// to 8 bits — a 1e-15 field difference is 1e-13 of a level — and the texture
    /// crate pins the raster-level guarantee where it actually counts, on the
    /// baked bytes.
    #[test]
    fn a_warped_tiled_field_still_tiles() {
        let period = 4i64;
        let p = period as f64;
        let warped = |x: f64, y: f64| {
            let wx = fbm2(
                x,
                y,
                Fbm {
                    octaves: 3,
                    ..Fbm::tiled(period)
                },
                99,
                5,
            ) - 0.5;
            fbm2(
                x + wx,
                y + wx,
                Fbm {
                    octaves: 3,
                    ..Fbm::tiled(period)
                },
                1,
                5,
            )
        };
        for k in 0..period {
            let (x, y) = (k as f64, k as f64);
            let (a, b) = (warped(x, y), warped(x + p, y + p));
            assert!((a - b).abs() < 1e-12, "warped seam at cell {k}: {a} vs {b}");
        }
    }

    /// Untiled fields must NOT repeat — proves `period = 0` really disables the
    /// wrap rather than silently tiling everything.
    #[test]
    fn an_untiled_field_does_not_repeat() {
        assert_ne!(value2(1.3, 4.7, 3, 11), value2(7.3, 4.7, 3, 11));
    }

    #[test]
    fn shaping_folds_about_the_midpoint() {
        assert!((ridged(0.5) - 1.0).abs() < 1e-12);
        assert!(ridged(0.0).abs() < 1e-12);
        assert!(billow(0.5).abs() < 1e-12);
        assert!((billow(1.0) - 1.0).abs() < 1e-12);
        // Contrast pivots on 0.5 and clamps.
        assert!((contrast(0.5, 4.0) - 0.5).abs() < 1e-12);
        assert_eq!(contrast(1.0, 4.0), 1.0);
        assert_eq!(contrast(0.0, 4.0), 0.0);
    }

    /// Worley must actually be cellular — a field of constant distance would pass
    /// the range test but carry no structure.
    #[test]
    fn worley_has_near_and_far_samples() {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for i in 0..64 {
            for j in 0..64 {
                let v = worley2_tiled(i as f64 / 8.0, j as f64 / 8.0, 8, 4, 77);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        assert!(lo < 0.1, "no sample landed near a feature point (min {lo})");
        assert!(hi > 0.4, "no sample landed far from one (max {hi})");
    }
}
