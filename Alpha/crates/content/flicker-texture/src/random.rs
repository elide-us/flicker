//! The **randomizer** — roll a whole instrument in one pass.
//!
//! Not a "vary the current patch" control: it re-rolls the rack, the blends, the
//! colour ramp, the output stage and the glow together, because interesting
//! surfaces come from combinations nobody would have typed. It is the fastest way
//! to find a starting point, and the fastest way to learn what the instrument can
//! do — you turn one knob on a sound you did not expect.
//!
//! # Still deterministic
//!
//! A roll takes a seed and returns a recipe. Same seed, same surface, forever —
//! so a roll you liked is reproducible, and a rolled recipe is an ordinary recipe
//! that saves and rebuilds like any other. Nothing here reads a clock.
//!
//! # Biased on purpose
//!
//! Uniform-random parameters produce grey mush with striking regularity: every
//! channel at a middling amount averages into noise, and a ramp of random colours
//! is a smear. So the roll is *shaped* — a bed channel that always establishes the
//! field, detail voices that mostly modulate rather than replace, scales spread
//! across octaves so coarse structure and fine grain coexist, and a ramp built as
//! one hue walked through value rather than four unrelated colours. The goal is
//! "surprising but plausibly a material", not "uniformly random".

use crate::channel::{BlendMode, Channel, NoiseKind, CHANNEL_COUNT};
use crate::output::{ColorRamp, OutputStage, RampStop};
use crate::recipe::TextureRecipe;

/// A splitmix64 stream — the same finalizer family the lattice hash uses, but a
/// SEQUENCE rather than a spatial field. Kept local: `noise` hashes coordinates,
/// this hands out one value after another, and widening that module's API for a
/// different job would blur what it is for.
struct Roll(u64);

impl Roll {
    /// Finalize the SEED before it becomes stream state.
    ///
    /// splitmix64 advances by ADDING the golden constant, so a seed that is itself
    /// a multiple of it starts the stream at a point already on that lattice —
    /// different seeds then march in lockstep and their rolls correlate. Observed
    /// for real: eight rolls seeded `i · GOLDEN` came out with identical metalness
    /// and near-identical glow. Hashing the seed first decouples it.
    fn new(seed: u64) -> Self {
        let mut z = seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Roll(z ^ (z >> 31))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A float in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A float in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.unit() * (hi - lo)
    }

    /// An integer in `[lo, hi]`.
    fn int(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next() % (hi - lo + 1) as u64) as u32
    }

    /// True with probability `p`.
    fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[(self.next() % from.len() as u64) as usize]
    }
}

/// The blends a DETAIL voice may use — `Base` is excluded because a detail voice
/// that replaces the bus erases everything under it, which is how a six-channel
/// rack ends up sounding like one channel.
const DETAIL_BLENDS: [BlendMode; 7] = [
    BlendMode::Add,
    BlendMode::Mul,
    BlendMode::Screen,
    BlendMode::Overlay,
    BlendMode::Min,
    BlendMode::Max,
    BlendMode::Diff,
];

/// Roll a complete recipe from `seed`.
///
/// `id`/`name` are the caller's to set — a roll produces a SURFACE, not an
/// identity, and naming it is the moment a human decides it is worth keeping.
pub fn random(seed: u64) -> TextureRecipe {
    let mut r = Roll::new(seed);
    let mut channels = [Channel::default(); CHANNEL_COUNT];

    // Voice 1 is always the BED: enabled, `Base`, full amount. Without a bed the
    // bus starts at zero and every later fold multiplies against black.
    let voices = r.int(2, CHANNEL_COUNT as u32) as usize;
    for (i, ch) in channels.iter_mut().enumerate().take(voices) {
        let bed = i == 0;
        // Scale climbs with the voice index so coarse STRUCTURE and fine GRAIN
        // coexist instead of every channel competing at one frequency.
        let band = i as f32 / CHANNEL_COUNT as f32;
        let lo = 2.0 + band * 26.0;
        *ch = Channel {
            enabled: true,
            source: r.pick(&NoiseKind::ALL),
            scale: r.range(lo, lo * 2.2).round().clamp(1.0, 64.0) as u32,
            octaves: r.int(1, if bed { 6 } else { 4 }),
            lacunarity: r.range(1.7, 2.6) as f64,
            gain: r.range(0.35, 0.65) as f64,
            // Warp is what turns ruled patterns into rock, so it is generous —
            // but only sometimes, or everything comes out equally churned.
            warp: if r.chance(0.55) {
                r.range(0.1, 1.1) as f64
            } else {
                0.0
            },
            salt: r.next(),
            contrast: r.range(0.7, 2.6) as f64,
            invert: r.chance(0.25),
            blend: if bed {
                BlendMode::Base
            } else {
                r.pick(&DETAIL_BLENDS)
            },
            // Detail voices sit back: a rack of six full-amount folds saturates.
            amount: if bed { 1.0 } else { r.range(0.2, 0.8) as f64 },
        };
    }

    let recipe = TextureRecipe {
        id: "rolled".into(),
        name: "Rolled".into(),
        material: None,
        seed,
        channels,
        out: random_output(&mut r),
    };
    // `emissive_band` stays a FRACTION here; `bake` seats it into the field's real
    // range (it used to be seated here, roll-only, which is why hand-set emit
    // controls baked black — MCP). A rolled recipe now behaves exactly like a
    // hand-built one: the band means the same thing however the recipe was made.
    recipe
}

/// The output stage: one hue walked through value, plus a surface finish.
fn random_output(r: &mut Roll) -> OutputStage {
    // ONE hue with a little drift, walked dark→light. Four unrelated colours make
    // a smear; a single hue through value reads as a material.
    let hue = r.range(0.0, 1.0);
    let drift = r.range(-0.08, 0.08);
    let sat = r.range(0.05, 0.55);
    let stops = (0..4)
        .map(|i| {
            let t = i as f32 / 3.0;
            // Value climbs but never reaches the ends — pure black and pure white
            // both read as broken lighting rather than as a surface.
            let v = 0.06 + t * r.range(0.55, 0.85);
            RampStop {
                at: t,
                color: hsv(hue + drift * t, sat, v),
            }
        })
        .collect();

    // Metal is a COMMITMENT, not a dial: physically a surface is a conductor or it
    // is not, so the roll picks a side rather than landing in the meaningless
    // middle. The modulation is what lets veins of one sit in the other.
    let metallic = r.chance(0.3);
    let glowing = r.chance(0.25);
    OutputStage {
        ramp: ColorRamp { stops },
        relief: r.range(0.2, 1.0),
        roughness: if metallic {
            r.range(0.1, 0.55)
        } else {
            r.range(0.45, 0.95)
        },
        roughness_mod: r.range(-0.4, 0.4),
        metalness: if metallic { r.range(0.7, 1.0) } else { 0.0 },
        metalness_mod: if metallic {
            r.range(-0.5, 0.2)
        } else {
            r.range(0.0, 0.5)
        },
        ao: r.range(0.2, 0.9),
        // A glow is rare and, when it happens, SATURATED and banded — lava in the
        // cracks, runes in the crests. A dim wide glow just looks like fog.
        emissive: hsv(r.range(0.0, 1.0), r.range(0.6, 1.0), 1.0),
        emissive_strength: if glowing { r.range(0.5, 1.0) } else { 0.0 },
        emissive_band: r.range(0.45, 0.85),
    }
}

/// HSV → linear RGB. `h` wraps, so a hue walked past 1.0 keeps going round the
/// wheel rather than clamping to magenta.
fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{bake, MapKind};

    /// A roll is a RECIPE, so it obeys the crate's determinism law: same seed,
    /// same surface, forever. Otherwise a roll you liked could not be saved.
    #[test]
    fn a_roll_is_reproducible_from_its_seed() {
        assert_eq!(random(12345), random(12345));
        assert_ne!(random(12345), random(12346));
    }

    /// Every roll must be a VALID recipe — the baker trusts its ranges, and a
    /// rolled value outside them would render as a corrupt surface rather than an
    /// interesting one.
    #[test]
    fn every_roll_is_in_range_and_bakes() {
        for seed in 0..64u64 {
            let r = random(seed.wrapping_mul(0x9E37_79B9));
            assert!(r.active_channels() >= 2, "seed {seed}: too few voices");
            for ch in &r.channels {
                assert!(
                    (1..=64).contains(&ch.scale),
                    "seed {seed}: scale {}",
                    ch.scale
                );
                assert!(
                    (1..=8).contains(&ch.octaves),
                    "seed {seed}: octaves {}",
                    ch.octaves
                );
                assert!(
                    (0.0..=1.0).contains(&ch.amount),
                    "seed {seed}: amount {}",
                    ch.amount
                );
                assert!(
                    ch.warp >= 0.0 && ch.warp.is_finite(),
                    "seed {seed}: warp {}",
                    ch.warp
                );
                assert!(ch.contrast > 0.0, "seed {seed}: contrast {}", ch.contrast);
            }
            let o = &r.out;
            for (name, v) in [
                ("relief", o.relief),
                ("roughness", o.roughness),
                ("metalness", o.metalness),
                ("ao", o.ao),
                ("emissive_strength", o.emissive_strength),
            ] {
                assert!((0.0..=1.0).contains(&v), "seed {seed}: {name} = {v}");
            }
            assert!(o.ramp.stops.len() >= 2, "seed {seed}: a ramp needs stops");
            for st in &o.ramp.stops {
                assert!(
                    st.color.iter().all(|c| (0.0..=1.0).contains(c)),
                    "seed {seed}: colour"
                );
            }
        }
    }

    /// The FIRST voice is always the bed. Without one the bus starts at zero and
    /// every later fold multiplies against black — a whole roll wasted on a dark
    /// square.
    #[test]
    fn the_first_voice_always_establishes_the_field() {
        for seed in 0..32u64 {
            let r = random(seed);
            assert!(r.channels[0].enabled, "seed {seed}: no bed voice");
            assert_eq!(
                r.channels[0].blend,
                BlendMode::Base,
                "seed {seed}: bed does not set the bus"
            );
            assert_eq!(r.channels[0].amount, 1.0, "seed {seed}: bed is not at full");
        }
        // And no LATER voice may replace the bus, which would erase the rack.
        for seed in 0..32u64 {
            for ch in random(seed).channels.iter().skip(1).filter(|c| c.enabled) {
                assert_ne!(ch.blend, BlendMode::Base, "a detail voice replaces the bus");
            }
        }
    }

    /// The point of a roll is VARIETY. A generator that returns the same-looking
    /// surface every time is worse than useless — it would look like it worked.
    #[test]
    fn rolls_actually_differ_from_each_other() {
        let sample = |seed: u64| {
            let set = bake(&random(seed), 32);
            let base = set.get(MapKind::BaseColor).unwrap();
            let mean: u32 = base.pixels.chunks(4).map(|p| p[0] as u32).sum::<u32>()
                / (base.pixels.len() / 4) as u32;
            mean
        };
        let means: Vec<u32> = (0..12).map(|s| sample(s * 7919)).collect();
        let lo = *means.iter().min().unwrap();
        let hi = *means.iter().max().unwrap();
        assert!(hi - lo > 20, "every roll looks the same: means {means:?}");
    }

    /// And each roll must be a surface, not a flat wash — the failure mode of a
    /// uniform-random generator is grey mush that passes every range check.
    #[test]
    fn a_roll_has_structure_rather_than_being_flat() {
        let mut flat = 0;
        for seed in 0..16u64 {
            let set = bake(&random(seed.wrapping_mul(31)), 48);
            let base = set.get(MapKind::BaseColor).unwrap();
            let lo = base.pixels.chunks(4).map(|p| p[0]).min().unwrap();
            let hi = base.pixels.chunks(4).map(|p| p[0]).max().unwrap();
            if (hi as i32) - (lo as i32) < 16 {
                flat += 1;
            }
        }
        assert!(flat <= 2, "{flat}/16 rolls came out flat");
    }

    /// STRUCTURED seeds must still decorrelate. The bench feeds this an LCG step of
    /// the previous seed, and a caller may well feed it `i · CONSTANT` — if the
    /// stream is sensitive to that shape, whole runs of rolls come out alike while
    /// every statistical test over scattered seeds still passes.
    #[test]
    fn adversarially_structured_seeds_still_vary() {
        // Multiples of the very constant the stream advances by — the worst case.
        const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
        let rolls: Vec<TextureRecipe> =
            (0..16u64).map(|i| random(i.wrapping_mul(GOLDEN))).collect();

        let metals: Vec<bool> = rolls.iter().map(|r| r.out.metalness > 0.0).collect();
        assert!(
            metals.iter().any(|m| *m) && metals.iter().any(|m| !*m),
            "structured seeds all landed on the same metalness: {metals:?}"
        );
        let voices: std::collections::BTreeSet<usize> =
            rolls.iter().map(|r| r.active_channels()).collect();
        assert!(
            voices.len() >= 3,
            "structured seeds barely vary the rack: {voices:?}"
        );

        // And plain sequential seeds, the other obvious caller shape.
        let seq: std::collections::BTreeSet<usize> =
            (0..16u64).map(|i| random(i).active_channels()).collect();
        assert!(
            seq.len() >= 3,
            "sequential seeds barely vary the rack: {seq:?}"
        );
    }

    /// Metal is a commitment: a rolled surface is a conductor or it is not, never
    /// stranded in the physically meaningless middle.
    #[test]
    fn metalness_picks_a_side() {
        for seed in 0..48u64 {
            let m = random(seed.wrapping_mul(7)).out.metalness;
            assert!(
                m == 0.0 || m >= 0.7,
                "seed {seed}: metalness {m} is neither"
            );
        }
    }

    /// Glow is rare and, when present, actually emits — a roll that "has emission"
    /// but bakes black would be a lie in the readout.
    #[test]
    fn glow_is_occasional_and_real() {
        let glowing: Vec<u64> = (0..64u64)
            .filter(|s| random(s.wrapping_mul(13)).out.emissive_strength > 0.0)
            .collect();
        assert!(!glowing.is_empty(), "no roll ever glows");
        assert!(
            glowing.len() < 48,
            "almost every roll glows — it should be a surprise"
        );
        for s in glowing.iter().take(4) {
            let r = random(s.wrapping_mul(13));
            let set = bake(&r, 32);
            let e = set.get(MapKind::Emit).unwrap();
            assert!(
                e.pixels.chunks(4).any(|p| p[0] > 4 || p[1] > 4 || p[2] > 4),
                "seed {s} claims a glow but bakes black"
            );
        }
    }
}
