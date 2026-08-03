//! One channel of the rack, and how channels fold together.
//!
//! A channel is one voice of the synthesizer: it evaluates a single scalar field
//! over the tile, shapes it, and folds its result into the mix bus by a blend
//! mode at an amount. Channels never reference each other — the fold is
//! **ordinal**, channel 1 into channel 2 into channel 3 — which is what keeps the
//! instrument a strip of sliders instead of a graph of wires.

use flicker_primitive::noise::{self, Fbm};
use serde::{Deserialize, Serialize};

/// How many voices the rack has. Fixed, not a `Vec`: the instrument is a physical
/// console, so the count is part of the layout, the serialized form, and the
/// controller's L1/R1 walk. Growing it is a deliberate change to all three.
pub const CHANNEL_COUNT: usize = 6;

/// Salts for the two domain-warp offset fields. Distinct from any channel salt so
/// a voice's warp never correlates with its own field.
const WARP_SALT_X: u64 = 0x5741_5250_5F58_0001;
const WARP_SALT_Y: u64 = 0x5741_5250_5F59_0002;

/// What a channel generates before shaping.
///
/// Every kind is **tileable** and takes its period from the channel's scale, so no
/// combination of sources can produce a seam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    /// Smooth interpolated lattice noise — soft blotches. The bed most surfaces
    /// start from.
    #[default]
    Value,
    /// Summed octaves — the natural-looking cloud/rock field.
    Fbm,
    /// fBm folded about its midpoint: sharp crests and drainage between them.
    /// Mountain relief, cracked stone, vein filaments.
    Ridged,
    /// fBm pushed away from its midpoint: rounded lobes. Clumped soil, cloud.
    Billow,
    /// Cellular (Worley) distance — plates, pebbles, cell walls. The structure
    /// summed octaves cannot make.
    Worley,
    /// Parallel bands along an axis, warped by the field. Sedimentary strata,
    /// wood grain, brushed metal.
    Stripe,
}

impl NoiseKind {
    /// Every kind, in console order — the list the channel's source selector
    /// steps through, and the list the drift gate checks a recipe against.
    pub const ALL: [NoiseKind; 6] = [
        NoiseKind::Value,
        NoiseKind::Fbm,
        NoiseKind::Ridged,
        NoiseKind::Billow,
        NoiseKind::Worley,
        NoiseKind::Stripe,
    ];

    /// Stable id — the serialized name, the style-lookup key, and the label token
    /// suffix. One string per kind, so a rename moves all three together.
    pub fn id(self) -> &'static str {
        match self {
            NoiseKind::Value => "value",
            NoiseKind::Fbm => "fbm",
            NoiseKind::Ridged => "ridged",
            NoiseKind::Billow => "billow",
            NoiseKind::Worley => "worley",
            NoiseKind::Stripe => "stripe",
        }
    }
}

/// How a channel's field folds into the mix bus.
///
/// `Base` is the bed — it REPLACES rather than combines, so the first enabled
/// channel establishes the field and the rest modify it. A rack whose first
/// channel is not `Base` still works: the bus starts at zero and the first fold
/// operates on that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    /// Replace the bus.
    #[default]
    Base,
    /// Sum — brightens and adds detail.
    Add,
    /// Multiply — darkens, and masks the bus by this channel.
    Mul,
    /// Inverse-multiply — brightens without clipping as hard as `Add`.
    Screen,
    /// Multiply the darks, screen the lights — raises contrast around the mid.
    Overlay,
    /// Keep the darker — carves this channel's lows into the bus.
    Min,
    /// Keep the lighter — deposits this channel's highs onto the bus.
    Max,
    /// Absolute difference — edges and interference bands.
    Diff,
}

impl BlendMode {
    /// Every mode, in console order.
    pub const ALL: [BlendMode; 8] = [
        BlendMode::Base,
        BlendMode::Add,
        BlendMode::Mul,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Min,
        BlendMode::Max,
        BlendMode::Diff,
    ];

    /// Stable id — serialized name and label token suffix.
    pub fn id(self) -> &'static str {
        match self {
            BlendMode::Base => "base",
            BlendMode::Add => "add",
            BlendMode::Mul => "mul",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::Min => "min",
            BlendMode::Max => "max",
            BlendMode::Diff => "diff",
        }
    }

    /// Fold `top` into `bus`, then cross-fade by `amount` so a channel's
    /// contribution is continuous from silent (0) to full (1).
    ///
    /// `Base` at amount 0 is the bus unchanged, which is what makes the amount
    /// slider mean the same thing on every mode: "how much of this voice".
    pub fn apply(self, bus: f64, top: f64, amount: f64) -> f64 {
        let blended = match self {
            BlendMode::Base => top,
            BlendMode::Add => bus + top,
            BlendMode::Mul => bus * top,
            BlendMode::Screen => 1.0 - (1.0 - bus) * (1.0 - top),
            BlendMode::Overlay => {
                if bus < 0.5 {
                    2.0 * bus * top
                } else {
                    1.0 - 2.0 * (1.0 - bus) * (1.0 - top)
                }
            }
            BlendMode::Min => bus.min(top),
            BlendMode::Max => bus.max(top),
            BlendMode::Diff => (bus - top).abs(),
        };
        let a = amount.clamp(0.0, 1.0);
        // The endpoints are EXACT, not merely approached: a fully-open amount is
        // the blend itself and a closed one is the bus itself. Letting the
        // interpolation compute them instead leaves a float crumb (0.8 + (0.3 −
        // 0.8) is not 0.3), which accumulates across six folds and makes a rack's
        // output depend on how many silent channels sit in front of it.
        let mixed = if a >= 1.0 {
            blended
        } else if a <= 0.0 {
            bus
        } else {
            bus + (blended - bus) * a
        };
        mixed.clamp(0.0, 1.0)
    }
}

/// One voice of the rack.
///
/// Ranges are the console's, not the maths': `scale` is **cells across the tile**
/// (and therefore also the lattice period, which is why it is an integer — a
/// fractional number of cells cannot close a seam), `warp` is in cells of
/// displacement, `contrast` pivots on the midpoint.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    /// Silent channels are skipped entirely — no evaluation cost, and the strip
    /// shows `--off--`.
    pub enabled: bool,
    /// What this voice generates.
    pub source: NoiseKind,
    /// Cells across the tile. Doubles as the lattice period, so the swatch tiles.
    /// Clamped to at least 1 — zero cells is not a field.
    pub scale: u32,
    /// Octaves summed (ignored by `Value` and `Worley`, which are single-lattice).
    pub octaves: u32,
    /// Per-octave frequency step.
    pub lacunarity: f64,
    /// Per-octave amplitude step.
    pub gain: f64,
    /// Domain-warp strength, in cells. Zero is unwarped. The warp field shares
    /// this channel's period, so warping never opens a seam.
    pub warp: f64,
    /// Selects an independent field under the recipe seed — turning this
    /// re-rolls only this voice.
    pub salt: u64,
    /// Midpoint contrast: 1 is neutral, above hardens, below flattens.
    pub contrast: f64,
    /// Flip the field about its midpoint after shaping.
    pub invert: bool,
    /// How this voice folds into the bus.
    pub blend: BlendMode,
    /// How much of the fold to take, 0..=1.
    pub amount: f64,
}

impl Default for Channel {
    /// A silent, neutral voice — what an unused slot holds, and the base every
    /// edit starts from.
    fn default() -> Self {
        Self {
            enabled: false,
            source: NoiseKind::Fbm,
            scale: 4,
            octaves: 4,
            lacunarity: 2.0,
            gain: 0.5,
            warp: 0.0,
            salt: 0,
            contrast: 1.0,
            invert: false,
            blend: BlendMode::Base,
            amount: 1.0,
        }
    }
}

impl Channel {
    /// The lattice period this voice tiles on: its scale, floored at one cell.
    pub fn period(&self) -> i64 {
        self.scale.max(1) as i64
    }

    /// The fBm settings this voice's octaves describe.
    fn fbm(&self) -> Fbm {
        Fbm {
            octaves: self.octaves.clamp(1, 12),
            lacunarity: self.lacunarity,
            gain: self.gain,
            period: self.period(),
        }
    }

    /// Evaluate this voice at tile position `(u, v)` in `[0, 1)`, returning
    /// `[0, 1]`. `seed` is the recipe seed; the channel's own `salt` separates it
    /// from its neighbours.
    ///
    /// `u`/`v` are tile-relative rather than pixel indices so the field is
    /// resolution-independent: the same recipe at 256² and at 2048² is the same
    /// image, sampled more finely.
    pub fn eval(&self, u: f64, v: f64, seed: u64) -> f64 {
        let period = self.period();
        let cells = period as f64;
        let (mut x, mut y) = (u * cells, v * cells);

        // Domain warp: displace the sample by a low-octave field on the SAME
        // period, which is what keeps a warped channel seamless (a periodic warp
        // of a periodic field is periodic).
        if self.warp.abs() > f64::EPSILON {
            let w = Fbm { octaves: 2, lacunarity: 2.0, gain: 0.5, period };
            let wx = noise::fbm2(x, y, w, self.salt ^ WARP_SALT_X, seed) - 0.5;
            let wy = noise::fbm2(x, y, w, self.salt ^ WARP_SALT_Y, seed) - 0.5;
            x += wx * self.warp;
            y += wy * self.warp;
        }

        let raw = match self.source {
            NoiseKind::Value => noise::value2_tiled(x, y, period, self.salt, seed),
            NoiseKind::Fbm => noise::fbm2(x, y, self.fbm(), self.salt, seed),
            NoiseKind::Ridged => noise::ridged(noise::fbm2(x, y, self.fbm(), self.salt, seed)),
            NoiseKind::Billow => noise::billow(noise::fbm2(x, y, self.fbm(), self.salt, seed)),
            NoiseKind::Worley => noise::worley2_tiled(x, y, period, self.salt, seed),
            NoiseKind::Stripe => {
                // Bands across the tile: `y` is in cells, so the sine completes
                // exactly `scale` whole cycles and the bands meet at the seam.
                // Straight on their own — the domain warp above is what bends them
                // into undulating strata, which is why `warp` is the slider that
                // turns ruled lines into rock.
                (y * std::f64::consts::TAU).sin() * 0.5 + 0.5
            }
        };

        let shaped = noise::contrast(raw, self.contrast);
        if self.invert {
            1.0 - shaped
        } else {
            shaped
        }
    }
}

/// Fold every enabled channel of `rack` into one field at `(u, v)`.
///
/// The bus starts at zero, so a rack with no enabled channel is a black field —
/// honestly empty rather than an arbitrary grey.
pub fn mix(rack: &[Channel; CHANNEL_COUNT], u: f64, v: f64, seed: u64) -> f64 {
    let mut bus = 0.0;
    for ch in rack.iter().filter(|c| c.enabled) {
        bus = ch.blend.apply(bus, ch.eval(u, v, seed), ch.amount);
    }
    bus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_amount_zero_is_the_bus_untouched() {
        for mode in BlendMode::ALL {
            assert_eq!(mode.apply(0.37, 0.91, 0.0), 0.37, "{:?} not neutral at 0", mode);
        }
    }

    #[test]
    fn blend_modes_are_the_classic_identities() {
        // A fully-open amount is the blend EXACTLY — no interpolation crumb.
        assert_eq!(BlendMode::Base.apply(0.2, 0.8, 1.0), 0.8);
        assert_eq!(BlendMode::Min.apply(0.8, 0.3, 1.0), 0.3);
        assert_eq!(BlendMode::Max.apply(0.8, 0.3, 1.0), 0.8);
        assert!((BlendMode::Mul.apply(0.5, 0.5, 1.0) - 0.25).abs() < 1e-12);
        assert!((BlendMode::Screen.apply(0.5, 0.5, 1.0) - 0.75).abs() < 1e-12);
        assert!((BlendMode::Diff.apply(0.8, 0.3, 1.0) - 0.5).abs() < 1e-12);
        // Add and Screen both brighten, but Screen never clips.
        assert_eq!(BlendMode::Add.apply(0.8, 0.8, 1.0), 1.0);
        assert!(BlendMode::Screen.apply(0.8, 0.8, 1.0) < 1.0);
    }

    /// Every mode must stay in range whatever it is fed — a channel that pushed
    /// the bus out of `[0,1]` would corrupt every downstream fold.
    #[test]
    fn blends_stay_in_range() {
        for mode in BlendMode::ALL {
            for &bus in &[0.0, 0.5, 1.0] {
                for &top in &[0.0, 0.5, 1.0] {
                    for &amt in &[0.0, 0.5, 1.0] {
                        let v = mode.apply(bus, top, amt);
                        assert!((0.0..=1.0).contains(&v), "{mode:?}({bus},{top},{amt}) = {v}");
                    }
                }
            }
        }
    }

    /// Every source must produce a finite, in-range field — the gate that catches
    /// a new kind wired up wrong.
    #[test]
    fn every_source_is_finite_and_in_range() {
        for source in NoiseKind::ALL {
            let ch = Channel { enabled: true, source, warp: 0.7, ..Channel::default() };
            for i in 0..16 {
                for j in 0..16 {
                    let v = ch.eval(i as f64 / 16.0, j as f64 / 16.0, 9);
                    assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{source:?} → {v}");
                }
            }
        }
    }

    /// A disabled channel contributes nothing — the strip's `--off--` is real.
    #[test]
    fn a_disabled_channel_is_silent() {
        let mut rack = [Channel::default(); CHANNEL_COUNT];
        rack[0] = Channel { enabled: true, ..Channel::default() };
        let with_only_first = mix(&rack, 0.3, 0.6, 5);
        rack[1] = Channel { enabled: false, blend: BlendMode::Add, ..Channel::default() };
        assert_eq!(with_only_first, mix(&rack, 0.3, 0.6, 5));
    }

    /// Ids are what the serialized form, the styles and the labels all key on, so
    /// they must be unique.
    #[test]
    fn ids_are_unique() {
        let kinds: Vec<_> = NoiseKind::ALL.iter().map(|k| k.id()).collect();
        let modes: Vec<_> = BlendMode::ALL.iter().map(|m| m.id()).collect();
        for list in [&kinds, &modes] {
            let mut sorted = list.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), list.len(), "duplicate id in {list:?}");
        }
    }
}
