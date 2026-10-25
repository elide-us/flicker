//! **THE RESONANCE LAYER — planet generation V5's field family.**
//!
//! V4 (the Populous bench + [`crate::Evolution`]) produces a planet's HISTORY:
//! where material welled up, spread, jammed, folded and washed away. What it
//! never had was a procedural answer to SHAPE below the hex — how a hex's
//! ledger becomes a map, how a map becomes a cluster. V5's answer (Aaron,
//! 2026-10-11) is one family of continuous fields over the sphere —
//! superpositions of waves with their own wavelengths, temporal frequencies
//! and sources — that the ledger's conserved quantities are DISTRIBUTED by, the
//! same functions evaluated at planetary wavelengths for the ledger, regional
//! wavelengths for a hex, cluster wavelengths for the lattice. The molten seam
//! field's living bands (`seams.rs`, slice 1) are the family's first member on
//! the SOURCE side; this module holds the field EXPRESSIONS the WorldBench V5
//! authors with, planet-wide first.
//!
//! Three laws bind every expression here exactly as they bind the seams:
//! **transformations, never outcomes** (935269B7 — a wave is a rule over
//! position; nothing places a feature); **closed-form in (position, seed)** —
//! the recipe IS the field, nothing accumulates and nothing is serialized;
//! **topology-blind** — a pentagon reads the same function as a hex, so no tile
//! boundary is ever a geological boundary.
//!
//! # The first expression: the FULL PLANET WAVE
//!
//! The longest wavelength the icosahedral sphere carries: a standing wave whose
//! crests and troughs are the twelve pentagons themselves — **six above the
//! mean, six below**. Each pentagon direction `v` carries a lobe
//! `exp(lobe·(p·v − 1))` — a gaussian on the sphere, 1 at the pole, falling
//! with angular distance — signed `+` or `−`. The icosahedron's vertices come
//! in six antipodal PAIRS and the roll decides each pair's sign — one pole of
//! the pair crests, the other troughs — so every crest has its trough exactly
//! opposite: the field is odd, `f(−p) = −f(p)`, and its sphere mean is zero by
//! construction. The roll chooses WHICH six crest and never a height.
//!
//! (The first cut signed the poles by a hemisphere — every pole on one side of a
//! seeded axis crested — which welded the six crests into ONE supercontinent
//! around the axis and the six troughs into one ocean opposite; Aaron saw
//! exactly that in-window, 2026-10-13: "only one continent ever". Per-pair
//! signs keep six-and-six but scatter the crests, so the same dial now rolls
//! anything from a supercontinent to six scattered domes.)

use glam::{DVec3, Vec3};

use crate::map::{HexMap, TileId};
use crate::seams::SeamField;

/// The lobe sharpness — how fast a pole's influence falls with angular
/// distance. The twelve poles stand ~63.4° apart; at 6 a pole's lobe has fallen
/// to ~e⁻³ by its neighbours, so the crests read as twelve distinct domes and
/// the saddles between opposite-signed neighbours run as clean seams.
pub const DEFAULT_LOBE: f32 = 10.0;
/// The softest lobe the dial offers — broad domes that nearly merge.
pub const MIN_LOBE: f32 = 2.0;
/// The sharpest — tight caps on the poles over a near-flat mean.
pub const MAX_LOBE: f32 = 14.0;

/// The sea dial's two regimes: up to this percent the sea rises from the LOWEST
/// tile (0 = all land) to the HIGHEST (the summit tile at the waterline); the
/// last two percent flood the planet.
pub const SEA_CREST_PERCENT: f32 = 98.0;
/// How far above the highest tile the sea stands at 100%, as a share of the
/// relief span — a visible drowning, not a film on the summit.
pub const FLOOD_MARGIN: f32 = 0.25;

/// **The full planet wave** over one [`HexMap`] tiling: the twelve pentagon
/// poles with their signs, and the per-tile value normalized so the crest tile
/// reads `+1` and its antipodal trough `−1`.
pub struct PlanetWave {
    /// The roll that chose the pairs' signs — the field's whole identity.
    seed: u64,
    lobe: f32,
    /// The twelve pentagon directions and their signs (`+1.0` crest, `−1.0` trough),
    /// antipodal pairs carrying opposite signs.
    poles: Vec<(Vec3, f32)>,
    /// The raw field's peak magnitude over the tiling — what normalizes it to ±1.
    norm: f32,
    /// Per-tile value, `−1..1`, indexed by `TileId` like every per-tile layer.
    field: Vec<f32>,
}

impl PlanetWave {
    /// Roll a wave over `map` with `seed` at the default lobe.
    pub fn new(map: &HexMap, seed: u64) -> Self {
        Self::with_lobe(map, seed, DEFAULT_LOBE)
    }

    /// Roll a wave with an explicit lobe sharpness (clamped to the dial).
    pub fn with_lobe(map: &HexMap, seed: u64, lobe: f32) -> Self {
        let mut wave = Self {
            seed,
            lobe: lobe.clamp(MIN_LOBE, MAX_LOBE),
            poles: Vec::new(),
            norm: 1.0,
            field: Vec::new(),
        };
        wave.rebuild(map);
        wave
    }

    /// The roll that chose the pairs' signs.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The lobe sharpness the field stands at.
    pub fn lobe(&self) -> f32 {
        self.lobe
    }

    /// A pole's sign at `dir` — `+1` crest, `−1` trough — for the pole nearest it.
    pub fn sign_at(&self, dir: Vec3) -> f32 {
        self.poles
            .iter()
            .max_by(|a, b| a.0.dot(dir).total_cmp(&b.0.dot(dir)))
            .map_or(0.0, |(_, s)| *s)
    }

    /// The twelve poles and their signs — the crests (`+1`) and troughs (`−1`).
    pub fn poles(&self) -> &[(Vec3, f32)] {
        &self.poles
    }

    /// How many poles crest and how many trough — `(6, 6)` on any icosahedral
    /// tiling, by the antipodal pairing.
    pub fn crests(&self) -> (usize, usize) {
        let up = self.poles.iter().filter(|(_, s)| *s > 0.0).count();
        (up, self.poles.len() - up)
    }

    /// A tile's value, `−1..1`. Out-of-range asks read as the mean (zero) rather
    /// than panicking — a viewer's question, and a hole is level.
    pub fn value(&self, tile: TileId) -> f32 {
        self.field.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Every tile's value, for a shell's closures.
    pub fn values(&self) -> &[f32] {
        &self.field
    }

    /// **The closed form** — the field at ANY direction, not only a tile centre.
    /// This is what lets a finer tier (a hex's map, a cluster) read the same
    /// wave the planet was shaped by, without a lookup table of its own.
    pub fn eval(&self, p: Vec3) -> f32 {
        self.raw(p.normalize_or_zero()) / self.norm
    }

    fn raw(&self, p: Vec3) -> f32 {
        self.poles
            .iter()
            .map(|(v, s)| s * (self.lobe * (p.dot(*v) - 1.0)).exp())
            .sum()
    }

    /// Change the lobe sharpness, keeping the roll. A no-op at the current value.
    pub fn set_lobe(&mut self, map: &HexMap, lobe: f32) {
        let lobe = lobe.clamp(MIN_LOBE, MAX_LOBE);
        if (lobe - self.lobe).abs() < 1e-6 {
            return;
        }
        self.lobe = lobe;
        self.rebuild(map);
    }

    /// Re-roll the pairs' signs (a new choice of which six pentagons crest) over
    /// the same map.
    pub fn randomize(&mut self, map: &HexMap) {
        self.seed = fastrand::u64(..);
        self.rebuild(map);
    }

    /// The map was rebuilt (a new size) or the roll changed — derive the poles
    /// from the tiling's own twelve five-sided tiles, pair each with its
    /// antipode, roll one sign per pair, and evaluate every tile. The SAME seed
    /// at another size is the same wave: the signs are the roll's, the poles are
    /// the icosahedron's. The pairs are visited in a canonical order (sorted by
    /// direction) so the roll means the same thing on every tiling.
    pub fn rebuild(&mut self, map: &HexMap) {
        let mut rng = fastrand::Rng::with_seed(self.seed);
        let mut pentagons: Vec<Vec3> = map
            .tiles()
            .filter(|t| map.neighbours(*t).len() == 5)
            .map(|t| map.direction(t))
            .collect();
        pentagons.sort_by(|a, b| {
            a.y.total_cmp(&b.y)
                .then(a.x.total_cmp(&b.x))
                .then(a.z.total_cmp(&b.z))
        });
        let mut signs = vec![0.0f32; pentagons.len()];
        for i in 0..pentagons.len() {
            if signs[i] != 0.0 {
                continue;
            }
            // The antipode: the pole nearest −v (exactly opposite on an icosahedron).
            let anti = (0..pentagons.len())
                .filter(|j| *j != i && signs[*j] == 0.0)
                .min_by(|a, b| {
                    (pentagons[*a] + pentagons[i])
                        .length()
                        .total_cmp(&(pentagons[*b] + pentagons[i]).length())
                });
            let s = if rng.bool() { 1.0 } else { -1.0 };
            signs[i] = s;
            if let Some(j) = anti {
                signs[j] = -s;
            }
        }
        self.poles = pentagons.into_iter().zip(signs).collect();
        // Normalize to the tiling's own peak so the crest tile reads +1.
        self.norm = 1.0;
        let raw: Vec<f32> = map.grid().dirs.iter().map(|d| self.raw(*d)).collect();
        let peak = raw.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
        self.norm = peak;
        self.field = raw.iter().map(|v| v / peak).collect();
    }
}

/// **The sea dial.** `percent` runs `0..=100`: at 0 the sea stands at the lowest
/// tile (all land); up to [`SEA_CREST_PERCENT`] it rises linearly to the highest
/// tile (the summit at the waterline); the last two percent lift it a further
/// [`FLOOD_MARGIN`] of the relief span above every crest — the whole planet
/// drowned. Heights and the returned level share whatever unit `lo`/`hi` carry.
pub fn sea_level(lo: f32, hi: f32, percent: f32) -> f32 {
    let p = percent.clamp(0.0, 100.0);
    let span = (hi - lo).max(0.0);
    if p <= SEA_CREST_PERCENT {
        lo + span * (p / SEA_CREST_PERCENT)
    } else {
        let flood = (p - SEA_CREST_PERCENT) / (100.0 - SEA_CREST_PERCENT);
        hi + span.max(f32::EPSILON) * FLOOD_MARGIN * flood
    }
}

/// The AREA-WEIGHTED share of the surface standing above `sea` — the land
/// fraction a sea level leaves. Area-weighted because the twelve pentagons and
/// the ISEA crease cells are smaller than the mean cell (the ledger's own
/// convention): counting tiles would over-weight them.
pub fn land_share(map: &HexMap, height: impl Fn(TileId) -> f32, sea: f32) -> f32 {
    let area = &map.grid().area;
    let (mut land, mut total) = (0.0f64, 0.0f64);
    for t in map.tiles() {
        let a = f64::from(area.get(t as usize).copied().unwrap_or(1.0));
        total += a;
        if height(t) > sea {
            land += a;
        }
    }
    if total > 0.0 {
        (land / total) as f32
    } else {
        0.0
    }
}

// ── THE RESONANCES (Aaron 2026-10-15: "Imagine a sine wave with frequency 100,
// and then another with frequency 96 and another with 83 … and 3 and 1. When
// you place those fields ACROSS THE ENTIRE PLANET you should get some
// interesting results. Each resonance mask FOR THE ENTIRE PLANET should have
// both a frequency and an alpha mask, that's it.") ──

// The opening LAYERS live with the recipe (`recipe::DEFAULT_LAYERS`).

/// The harmonics' axis count range — how many plane waves of the one frequency
/// superpose into the standing pattern (one = a pure band pattern; several =
/// an interference lattice, the "resonance").
pub const MIN_AXES: u32 = 1;
pub const MAX_AXES: u32 = 6;
pub const DEFAULT_AXES: u32 = 1;

/// **A whole-planet resonance** over one tiling: a seeded superposition of
/// `axes` sinusoids, each `sin(frequency · θ + φ)` in the ANGULAR DISTANCE θ
/// from its own axis — rings of one wavelength around each axis, `frequency`
/// cycles per great circle — normalized so the crest tile reads `+1`.
/// Closed-form in (position, seed), the roll is the identity, mean ≈ 0,
/// topology-blind; a field is its frequency and (outside, in the recipe) its
/// alpha mask, nothing else.
pub struct HarmonicWave {
    seed: u64,
    frequency: f32,
    axes: u32,
    waves: Vec<(Vec3, f32)>,
    /// The raw sum's area-weighted mean over the tiling — removed, so every
    /// resonance stands as much below the datum as above it (a low-frequency
    /// ring field is otherwise mostly one sign).
    mean: f32,
    norm: f32,
    field: Vec<f32>,
}

impl HarmonicWave {
    /// Roll a resonance at `frequency` cycles per great circle with `axes` sinusoids.
    pub fn new(map: &HexMap, seed: u64, frequency: f32, axes: u32) -> Self {
        let mut w = Self {
            seed,
            frequency: frequency.max(1e-3),
            axes: axes.clamp(MIN_AXES, MAX_AXES),
            waves: Vec::new(),
            mean: 0.0,
            norm: 1.0,
            field: Vec::new(),
        };
        w.rebuild(map);
        w
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Cycles per great circle.
    pub fn frequency(&self) -> f32 {
        self.frequency
    }

    pub fn axes(&self) -> u32 {
        self.axes
    }

    /// Change the axis count on the same roll. A no-op at the current count.
    pub fn set_axes(&mut self, map: &HexMap, axes: u32) {
        let axes = axes.clamp(MIN_AXES, MAX_AXES);
        if axes == self.axes {
            return;
        }
        self.axes = axes;
        self.rebuild(map);
    }

    /// Re-roll the axes and phases.
    pub fn randomize(&mut self, map: &HexMap) {
        self.seed = fastrand::u64(..);
        self.rebuild(map);
    }

    /// A tile's value, `−1..1`.
    pub fn value(&self, tile: TileId) -> f32 {
        self.field.get(tile as usize).copied().unwrap_or(0.0)
    }

    pub fn values(&self) -> &[f32] {
        &self.field
    }

    /// The closed form at any direction.
    pub fn eval(&self, p: Vec3) -> f32 {
        (self.raw(p.normalize_or_zero()) - self.mean) / self.norm
    }

    /// The closed form at any direction, in DOUBLE precision — the ladder
    /// under the pixel (Aaron 2026-10-16: *"the formula … needs to be able
    /// to ramp down to the LOD0 point"*): a voxel is 2.4·10⁻⁸ rad on the
    /// standard planet, under what an f32 unit vector resolves, so the
    /// trains read in f64 there; the same roll, the same wave, within f32's
    /// rounding of [`Self::eval`] everywhere.
    pub fn eval64(&self, p: DVec3) -> f64 {
        let p = p.normalize_or_zero();
        let raw: f64 = self
            .waves
            .iter()
            .map(|(axis, phase)| {
                let theta = p.dot(axis.as_dvec3()).clamp(-1.0, 1.0).acos();
                (f64::from(self.frequency) * theta + f64::from(*phase)).sin()
            })
            .sum();
        (raw - f64::from(self.mean)) / f64::from(self.norm)
    }

    fn raw(&self, p: Vec3) -> f32 {
        self.waves
            .iter()
            .map(|(axis, phase)| {
                let theta = p.dot(*axis).clamp(-1.0, 1.0).acos();
                (self.frequency * theta + phase).sin()
            })
            .sum::<f32>()
    }

    /// Derive the axes and phases from the roll (the first `axes` of one
    /// sequence, so dialing the count up grows the same pattern) and evaluate
    /// every tile.
    pub fn rebuild(&mut self, map: &HexMap) {
        let mut rng = fastrand::Rng::with_seed(self.seed ^ 0x4841_524D_4F4E_4943);
        self.waves = (0..self.axes)
            .map(|_| {
                let z = rng.f32() * 2.0 - 1.0;
                let a = rng.f32() * std::f32::consts::TAU;
                let rr = (1.0 - z * z).max(0.0).sqrt();
                (
                    Vec3::new(rr * a.cos(), z, rr * a.sin()),
                    rng.f32() * std::f32::consts::TAU,
                )
            })
            .collect();
        // Balance, then normalize: subtract the area-weighted mean so the field
        // stands equally above and below the datum, then scale the crest to +1.
        self.mean = 0.0;
        self.norm = 1.0;
        let raw: Vec<f32> = map.grid().dirs.iter().map(|d| self.raw(*d)).collect();
        let area = &map.grid().area;
        let total: f32 = area.iter().sum();
        self.mean = raw.iter().zip(area).map(|(v, a)| v * a).sum::<f32>() / total.max(1e-6);
        let raw: Vec<f32> = raw.iter().map(|v| v - self.mean).collect();
        let peak = raw.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
        self.norm = peak;
        self.field = raw.iter().map(|v| v / peak).collect();
    }
}

// ── THE ALPHA MASK (Aaron 2026-10-13: the planetary wave "has no alpha mask for
// filtering what of this expression is even expressed in the heightmap") ──

/// The mask field's waves: a few planet-scale wavelengths, so the mask reads as
/// broad blotches that admit the wave here and mute it there — the PBR layer
/// mask of the first field. `(count, freq_min, freq_span)`.
const MASK_WAVES: (usize, f32, f32) = (5, 3.0, 6.0);
/// The mask's softness: the saturating sum is smoothstepped from this low to
/// its negative, so most of the sphere is either admitted or muted and the
/// transitions are short — a mask, not a second relief.
const MASK_EDGE: f32 = 0.35;

/// **The wave's alpha mask** over one tiling: `0..1` per tile, a seeded
/// superposition of planet-scale waves pushed toward 0 and 1 — what filters how
/// much of the base wave reaches the height map. Closed-form in (position, seed)
/// like every field; the roll is the identity.
pub struct WaveMask {
    seed: u64,
    waves: Vec<(Vec3, f32, f32, f32)>,
    field: Vec<f32>,
}

impl WaveMask {
    /// Roll a mask over `map` with `seed`.
    pub fn new(map: &HexMap, seed: u64) -> Self {
        let mut m = Self {
            seed,
            waves: Vec::new(),
            field: Vec::new(),
        };
        m.rebuild(map);
        m
    }

    /// The roll.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Re-roll with a specific seed (the MASK SEED dial — a scan through masks).
    pub fn set_seed(&mut self, map: &HexMap, seed: u64) {
        if seed == self.seed && !self.field.is_empty() {
            return;
        }
        self.seed = seed;
        self.rebuild(map);
    }

    /// A tile's mask, `0..1`; out of range reads fully admitted.
    pub fn value(&self, tile: TileId) -> f32 {
        self.field.get(tile as usize).copied().unwrap_or(1.0)
    }

    /// Every tile's mask.
    pub fn values(&self) -> &[f32] {
        &self.field
    }

    /// The saturating sum at any direction, `-1..1`.
    pub fn raw(&self, p: Vec3) -> f32 {
        self.waves
            .iter()
            .map(|(axis, amp, freq, phase)| amp * (freq * p.dot(*axis) + phase).sin())
            .sum::<f32>()
            .clamp(-1.0, 1.0)
    }

    /// The closed form at any direction.
    pub fn eval(&self, p: Vec3) -> f32 {
        // Smoothstep from −edge to +edge: a soft but short transition.
        let t = ((self.raw(p) + MASK_EDGE) / (2.0 * MASK_EDGE)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Derive the waves from the roll and evaluate every tile.
    pub fn rebuild(&mut self, map: &HexMap) {
        let mut rng = fastrand::Rng::with_seed(self.seed ^ 0x6D41_534B_5F56_3500);
        let (count, fmin, fspan) = MASK_WAVES;
        self.waves = (0..count)
            .map(|_| {
                let z = rng.f32() * 2.0 - 1.0;
                let a = rng.f32() * std::f32::consts::TAU;
                let rr = (1.0 - z * z).max(0.0).sqrt();
                (
                    Vec3::new(rr * a.cos(), z, rr * a.sin()),
                    (0.5 + rng.f32()) * 2.0 / count as f32,
                    fmin + rng.f32() * fspan,
                    rng.f32() * std::f32::consts::TAU,
                )
            })
            .collect();
        self.field = map.grid().dirs.iter().map(|d| self.eval(*d)).collect();
    }
}

// ── THE VOLCANIC EMITTERS (Aaron 2026-10-13: "a volcano would emit a snowflake
// looking pattern of hard molten rock"; 2026-10-16: "they're like all exactly
// the same size stamp … always makes exactly the same drop shape, that's more
// of a stamp than a procedure, it should be producing waves that are additive
// to the ongoing surrounding terrain") ──

/// The emitters' authored shape, the EMITTERS tab's dials.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VolcanoShape {
    /// The reach a FULL-HEAT vent's waves travel, in tile widths — each vent
    /// reaches by its own heat and roll around this.
    pub radius_tiles: f32,
    /// The fold of the waves' bearing modulation — how many ways round a
    /// vent's rings bulge (the snowflake's arms, rolled in phase per vent).
    pub arms: u32,
    /// How many radial wave TRAINS a vent emits — more trains, more rings and
    /// terraces interfering around it.
    pub rings: f32,
    /// Peak height at a full-heat vent, radius units.
    pub amp: f32,
}

impl Default for VolcanoShape {
    fn default() -> Self {
        Self {
            radius_tiles: 6.0,
            arms: 6,
            rings: 2.0,
            amp: 0.02,
        }
    }
}

/// How far past a vent's own reach its waves are followed before they are
/// counted as gone (the packets decay exponentially; this is the cull).
pub const VOLCANO_REACH: f32 = 2.5;
/// How much the bearing modulations swing a ring's height (`1 ± this`).
const VOLCANO_FOLD: f32 = 0.35;
/// A vent's reach against the dial: the coldest vent the crust admits reaches
/// this share of a full-heat vent's, and every vent's roll strays ±this much
/// around its heat's reach.
const VOLCANO_COLD_REACH: f32 = 0.4;
const VOLCANO_REACH_STRAY: f32 = 0.3;

/// **One vent's emission** — its wave trains and their roll: where it is, how
/// hot it runs, how far its waves travel, and each train's wavelength, phase
/// and decay. Two vents share nothing but the dials; the heat under one and
/// its roll make its figure its own.
#[derive(Clone, Debug)]
pub struct Emitter {
    pub dir: Vec3,
    pub heat: f32,
    /// Angular reach of the waves (radians): the dial's, by heat and roll.
    pub reach: f32,
    /// `(wavelength, phase, decay)` per train, radians.
    pub trains: Vec<(f32, f32, f32)>,
    /// The bearing modulations' phases: the `arms`-fold and a two-fold warp.
    pub fold_phase: f32,
    pub warp_phase: f32,
}

impl Emitter {
    /// **Roll a vent's emission**: its reach from the dial's full-heat `radius`
    /// (radians) by its heat and a ±stray, `trains` wave trains with
    /// non-commensurate wavelengths, phases and decays, and the bearing
    /// modulations' phases — all off `rng`, so the same roll is the same
    /// vent. The standing emitters and the epochs' eruption DROPS share it.
    pub fn rolled(
        dir: Vec3,
        heat: f32,
        radius: f32,
        trains: usize,
        rng: &mut fastrand::Rng,
    ) -> Self {
        let heat = heat.clamp(0.0, 1.0);
        let stray = 1.0 + VOLCANO_REACH_STRAY * (rng.f32() * 2.0 - 1.0);
        let reach = radius * (VOLCANO_COLD_REACH + (1.0 - VOLCANO_COLD_REACH) * heat) * stray;
        let trains = (0..trains.max(1))
            .map(|j| {
                // Each train's wavelength a fraction of the reach, no two
                // commensurate, so the rings interfere into terraces.
                let wavelength = reach / (j as f32 + 1.0 + rng.f32());
                let phase = rng.f32() * std::f32::consts::TAU;
                let decay = reach * (0.5 + rng.f32());
                (wavelength, phase, decay)
            })
            .collect();
        Self {
            dir,
            heat,
            reach,
            trains,
            fold_phase: rng.f32() * std::f32::consts::TAU,
            warp_phase: rng.f32() * std::f32::consts::TAU,
        }
    }

    /// The vent's waves at `d`, in units of the full-heat peak: the trains'
    /// superposition — `cos(2πθ/λ + φ)·e^(−θ/ℓ)` each, a ring train decaying
    /// from the vent — modulated by the bearing (an `arms`-fold bulge and a
    /// two-fold warp, both rolled), scaled by the heat. A peak, a caldera, a
    /// ring of hills or a terraced shield, as the phases fall; never the same
    /// figure twice, never a stamp. Zero past the cull.
    pub fn figure(&self, arms: u32, d: Vec3) -> f32 {
        let theta = self.dir.dot(d).clamp(-1.0, 1.0).acos();
        if theta > self.reach * VOLCANO_REACH {
            return 0.0;
        }
        let east = Vec3::Y.cross(self.dir).normalize_or_zero();
        let east = if east.length_squared() < 1e-6 {
            Vec3::X
        } else {
            east
        };
        let north = self.dir.cross(east);
        let off = d - self.dir;
        let bearing = off.dot(north).atan2(off.dot(east));
        let n = self.trains.len().max(1) as f32;
        let waves: f32 = self
            .trains
            .iter()
            .map(|(wavelength, phase, decay)| {
                (std::f32::consts::TAU * theta / wavelength.max(1e-6) + phase).cos()
                    * (-theta / decay.max(1e-6)).exp()
            })
            .sum::<f32>()
            / n;
        let fold = 1.0 + VOLCANO_FOLD * (arms as f32 * bearing + self.fold_phase).cos();
        let warp = 1.0 + VOLCANO_FOLD * (2.0 * bearing + self.warp_phase).cos();
        // The last of the packet fades to exactly nothing at the cull, so the
        // field is continuous there and the tiles and the closed form agree.
        let fade = (1.0 - theta / (self.reach * VOLCANO_REACH)).max(0.0);
        self.heat * waves * fold * warp * fade
    }
}

/// **An impact's crater wave** at angular distance `theta` from the strike,
/// in units of its depth, for a crater of angular radius `radius` (Aaron
/// 2026-10-15: *"a huge wave, a crater, as it were, that covered a quarter of
/// the planet"*): a rounded BOWL inside the rim, a raised RIM at the radius,
/// and RIPPLES running outward and fading — one closed form of distance,
/// additive to whatever stands there. Zero past [`CRATER_REACH`] radii.
pub fn crater(theta: f32, radius: f32) -> f32 {
    let r = theta / radius.max(1e-6);
    if r > CRATER_REACH {
        return 0.0;
    }
    let bowl = -(-2.0 * r * r).exp();
    let rim = CRATER_RIM * (-((r - 1.0) / 0.2).powi(2)).exp();
    let ripples = if r > 1.0 {
        CRATER_RIPPLE
            * (std::f32::consts::TAU * (r - 1.0) / 0.7).cos()
            * (-(r - 1.0) / 0.8).exp()
            * (1.0 - r / CRATER_REACH).max(0.0)
    } else {
        0.0
    };
    bowl + rim + ripples
}

/// How far a crater's ripples run, in radii; the rim's height and the
/// ripples' first swing, in units of the bowl's depth.
pub const CRATER_REACH: f32 = 3.0;
pub const CRATER_RIM: f32 = 1.2;
pub const CRATER_RIPPLE: f32 = 0.4;

/// **The volcanic emitters' field**: every vent of the crust (the heat
/// field's consequence) EMITS radial wave trains — rings of relief travelling
/// out from the vent and fading — whose reach, wavelengths and phases are the
/// vent's own (its heat, and a roll off the heat field's seed and the vent),
/// superposed on whatever relief stands there. Nothing is placed and nothing
/// is stamped: the vents are where the heat broke through, and the waves are
/// one closed form of (distance, bearing) from each, readable at any
/// direction ([`Self::eval`]) so a hex's 2K map carries them exactly.
pub struct Volcanoes {
    shape: VolcanoShape,
    emitters: Vec<Emitter>,
    field: Vec<f32>,
}

impl Volcanoes {
    /// Light every vent in `vents` over `map`, heat and roll from `seams`.
    pub fn new(map: &HexMap, seams: &SeamField, vents: &[TileId], shape: VolcanoShape) -> Self {
        let mut v = Self {
            shape,
            emitters: Vec::new(),
            field: Vec::new(),
        };
        v.rebuild(map, seams, vents);
        v
    }

    /// The shape the field was emitted with.
    pub fn shape(&self) -> VolcanoShape {
        self.shape
    }

    /// Re-emit with a new shape (a dial moved). A no-op at the same shape.
    pub fn set_shape(
        &mut self,
        map: &HexMap,
        seams: &SeamField,
        vents: &[TileId],
        shape: VolcanoShape,
    ) {
        if shape == self.shape && !self.field.is_empty() {
            return;
        }
        self.shape = shape;
        self.rebuild(map, seams, vents);
    }

    /// How many emitters are lit.
    pub fn count(&self) -> usize {
        self.emitters.len()
    }

    /// The emitters themselves — reach and trains per vent.
    pub fn emitters(&self) -> &[Emitter] {
        &self.emitters
    }

    /// A tile's height from the emitters, radius units; out of range is level.
    pub fn value(&self, tile: TileId) -> f32 {
        self.field.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Every tile's height.
    pub fn values(&self) -> &[f32] {
        &self.field
    }

    /// **The closed form** — the emitters' height at any direction, radius
    /// units: the sum of every vent's waves within its cull.
    pub fn eval(&self, p: Vec3) -> f32 {
        let p = p.normalize_or_zero();
        self.emitters
            .iter()
            .map(|e| e.figure(self.shape.arms, p))
            .sum::<f32>()
            * self.shape.amp
    }

    /// Light every vent: its reach from its heat and roll, its trains rolled,
    /// then every tile within the cull summed.
    pub fn rebuild(&mut self, map: &HexMap, seams: &SeamField, vents: &[TileId]) {
        let n = map.len();
        let tile_w = 4.0 / (n as f32).sqrt();
        let radius = self.shape.radius_tiles.max(0.5) * tile_w;
        let trains = self.shape.rings.round().max(1.0) as usize;
        self.emitters = vents
            .iter()
            .map(|&v| {
                let mut rng = fastrand::Rng::with_seed(
                    seams.seed() ^ (u64::from(v) + 1).wrapping_mul(0x5651_4C43_414E_4F21),
                );
                Emitter::rolled(map.direction(v), seams.heat(v), radius, trains, &mut rng)
            })
            .collect();
        self.field = vec![0.0; n];
        let dirs = &map.grid().dirs;
        for e in &self.emitters {
            let near = (e.reach * VOLCANO_REACH).cos();
            for (i, d) in dirs.iter().enumerate() {
                if d.dot(e.dir) < near {
                    continue;
                }
                self.field[i] += self.shape.amp * e.figure(self.shape.arms, *d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;

    /// **The trains read in double precision under the pixel.** Two
    /// directions a voxel apart (2.4·10⁻⁸ rad) are one direction to f32 and
    /// two to f64: the fine train's f64 reading moves between them, its f32
    /// reading cannot; and everywhere the two readings agree to f32's
    /// rounding.
    #[test]
    fn the_trains_read_in_double_precision_under_the_pixel() {
        let map = HexMap::new(MIN_FREQ);
        let w = HarmonicWave::new(&map, 11, 49_152.0, 5);
        let coarse = HarmonicWave::new(&map, 11, 64.0, 5);
        let mut rng = fastrand::Rng::with_seed(3);
        for _ in 0..200 {
            let z = rng.f64() * 2.0 - 1.0;
            let a = rng.f64() * std::f64::consts::TAU;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let p = DVec3::new(r * a.cos(), z, r * a.sin());
            // At 64 cycles the two agree to f32's rounding; at 49 152 the f32
            // phase (frequency × angle, up to 1.5·10⁵ rad) itself rounds to a
            // hundredth of a radian — the whole case for f64 under the pixel.
            let (c64, c32) = (coarse.eval64(p), f64::from(coarse.eval(p.as_vec3())));
            assert!(
                (c64 - c32).abs() < 1e-4,
                "f64 and f32 agree: {c64} vs {c32}"
            );
            let (a64, a32) = (w.eval64(p), f64::from(w.eval(p.as_vec3())));
            assert!(
                (a64 - a32).abs() < 0.1,
                "the fine train within f32's phase: {a64} vs {a32}"
            );
        }
        let p0 = DVec3::new(0.3, 0.5, 0.8).normalize();
        let side = p0.cross(DVec3::Y).normalize();
        let voxel = 2.4e-8;
        let p1 = (p0 + side * voxel).normalize();
        assert_eq!(p0.as_vec3(), p1.as_vec3(), "one direction to f32");
        let moved: f64 = (0..64)
            .map(|k| {
                let q0 = (p0 + side * (k as f64 * voxel * 64.0)).normalize();
                let q1 = (q0 + side * voxel).normalize();
                (w.eval64(q1) - w.eval64(q0)).abs()
            })
            .fold(0.0, f64::max);
        assert!(moved > 1e-5, "f64 resolves the voxel: {moved}");
    }

    /// **Six crest, six trough — the poles ARE the pentagons.** Every pole is a
    /// five-sided tile's direction, exactly twelve of them, half signed up and
    /// half down; the crest tile reads +1 and the field never leaves ±1.
    #[test]
    fn the_wave_crests_six_pentagons_and_troughs_six() {
        let map = HexMap::new(MIN_FREQ);
        for seed in [7u64, 42, 1234, 777] {
            let w = PlanetWave::new(&map, seed);
            assert_eq!(w.poles().len(), 12, "seed {seed}: twelve poles");
            assert_eq!(w.crests(), (6, 6), "seed {seed}: six up, six down");
            assert_eq!(w.values().len(), map.len());
            assert!(w.values().iter().all(|v| (-1.0..=1.0).contains(v)));
            let hi = w.values().iter().copied().fold(f32::MIN, f32::max);
            let lo = w.values().iter().copied().fold(f32::MAX, f32::min);
            assert!((hi - 1.0).abs() < 1e-5, "the crest tile reads +1: {hi}");
            assert!((lo + 1.0).abs() < 0.05, "and the trough tile ≈ −1: {lo}");
            // The pentagon tiles themselves stand at the extremes, signed by the roll,
            // and every pole's antipode carries the opposite sign.
            for t in map.tiles().filter(|t| map.neighbours(*t).len() == 5) {
                let d = map.direction(t);
                let sign = w.sign_at(d);
                assert!(
                    w.value(t) * sign > 0.6,
                    "seed {seed}: pentagon {t} stands on its own side: {}",
                    w.value(t)
                );
                assert_eq!(w.sign_at(-d), -sign, "the antipode is the opposite");
            }
        }
        // The crests are NOT one hemisphere by construction: over many rolls, some
        // roll puts crests on both sides of any given pole's great circle — the
        // supercontinent is one outcome of the roll, not the only one.
        let mut scattered = false;
        for seed in 0..32u64 {
            let w = PlanetWave::new(&map, seed);
            let (c0, _) = w.poles()[0];
            let crests: Vec<Vec3> = w
                .poles()
                .iter()
                .filter(|(_, s)| *s > 0.0)
                .map(|(v, _)| *v)
                .collect();
            if crests.iter().any(|v| v.dot(c0) > 0.1) && crests.iter().any(|v| v.dot(c0) < -0.1) {
                scattered = true;
                break;
            }
        }
        assert!(
            scattered,
            "some roll scatters the crests across hemispheres"
        );
    }

    /// **The field is ODD, so its mean is zero by construction** — every crest has
    /// its trough exactly opposite (`f(−p) = −f(p)`, the antipodal pairing of the
    /// icosahedron's vertices), and the area-weighted mean over the tiling is a
    /// whisker from zero: six above the mean, six below, as asked.
    #[test]
    fn the_wave_is_odd_and_its_mean_is_zero() {
        let map = HexMap::new(MIN_FREQ);
        let w = PlanetWave::new(&map, 42);
        for t in (0..map.len() as TileId).step_by(97) {
            let d = map.direction(t);
            assert!(
                (w.eval(-d) + w.eval(d)).abs() < 1e-4,
                "tile {t}: f(−p) = −f(p)"
            );
            assert!(
                (w.eval(d) - w.value(t)).abs() < 1e-5,
                "eval agrees with the tile"
            );
        }
        let area = &map.grid().area;
        let (mut sum, mut tot) = (0.0f64, 0.0f64);
        for t in map.tiles() {
            let a = f64::from(area[t as usize]);
            sum += a * f64::from(w.value(t));
            tot += a;
        }
        assert!((sum / tot).abs() < 0.01, "mean {}", sum / tot);
        let above = land_share(&map, |t| w.value(t), 0.0);
        assert!(
            (above - 0.5).abs() < 0.03,
            "half the area stands above the mean: {above}"
        );
    }

    /// **The roll is the identity.** Same seed, same field; a re-roll moves the
    /// crests; the same seed at ANOTHER size keeps the same axis and the same
    /// six crests (the poles are the icosahedron's, the choice is the roll's).
    #[test]
    fn the_roll_is_the_identity_across_sizes() {
        let a = PlanetWave::new(&HexMap::new(MIN_FREQ), 1234);
        let b = PlanetWave::new(&HexMap::new(MIN_FREQ), 1234);
        assert_eq!(a.values(), b.values());
        let mut c = PlanetWave::new(&HexMap::new(MIN_FREQ), 1234);
        c.randomize(&HexMap::new(MIN_FREQ));
        assert_ne!(c.seed(), 1234);
        // Six pairs of signs is sixty-four patterns: a re-roll may land on the
        // same one; eight re-rolls do not.
        let mut rolls = 0;
        while a.values() == c.values() && rolls < 8 {
            c.randomize(&HexMap::new(MIN_FREQ));
            rolls += 1;
        }
        assert_ne!(a.values(), c.values(), "a re-roll moves the crests");
        let bigger = HexMap::new(MIN_FREQ + 12);
        let d = PlanetWave::new(&bigger, 1234);
        for (pa, pd) in a.poles().iter().zip(d.poles()) {
            assert!(
                (pa.0 - pd.0).length() < 1e-3 && pa.1 == pd.1,
                "same poles, same signs"
            );
        }
        // The lobe dial reshapes without re-rolling.
        let mut e = PlanetWave::new(&HexMap::new(MIN_FREQ), 1234);
        e.set_lobe(&HexMap::new(MIN_FREQ), MAX_LOBE);
        for (pa, pe) in a.poles().iter().zip(e.poles()) {
            assert_eq!(pa.1, pe.1, "the lobe keeps the signs");
        }
        assert_ne!(
            e.values(),
            a.values(),
            "a sharper lobe is a different shape"
        );
        assert_eq!(e.crests(), (6, 6));
    }

    /// **No topology boundary is a geological boundary.** A pentagon's value sits
    /// inside its neighbours' span at least as tightly as a hex's does — the
    /// field indexes by DIRECTION, and the pentagons are merely the poles.
    #[test]
    fn pentagons_read_the_same_field_as_the_hexes() {
        let map = HexMap::new(MIN_FREQ);
        let w = PlanetWave::new(&map, 7);
        let jump = |t: TileId| {
            let nb = map.neighbours(t);
            let mean = nb.iter().map(|n| w.value(*n)).sum::<f32>() / nb.len() as f32;
            (w.value(t) - mean).abs()
        };
        let mut hex: Vec<f32> = Vec::new();
        let mut pent = 0.0f32;
        for t in map.tiles() {
            if map.neighbours(t).len() == 5 {
                pent += jump(t);
            } else {
                hex.push(jump(t));
            }
        }
        hex.sort_by(f32::total_cmp);
        let p99 = hex[hex.len() * 99 / 100];
        assert!(
            pent / 12.0 <= p99.max(0.02),
            "pentagons jump {} vs the hexes' p99 {p99}",
            pent / 12.0
        );
    }

    /// **A resonance is a frequency across the whole planet.** Bounded ±1 with the
    /// crest tile at +1, mean near zero, the roll is the identity, the axis count
    /// grows the same pattern, and the FREQUENCY is what it says: a frequency-13
    /// field crosses zero along a great circle ~26 times, a frequency-1 one ~2.
    #[test]
    fn a_resonance_is_a_frequency_across_the_whole_planet() {
        let map = HexMap::new(MIN_FREQ);
        let crossings = |w: &HarmonicWave| {
            // Walk a great circle THROUGH the field's first axis in 720 samples;
            // count sign changes — a ring field crosses 2·f times on such a circle.
            let axis = w.waves[0].0;
            let perp = axis.cross(Vec3::Y).normalize_or_zero();
            let perp = if perp.length_squared() < 1e-6 {
                axis.cross(Vec3::X).normalize()
            } else {
                perp
            };
            let at = |a: f32| axis * a.cos() + perp * a.sin();
            let mut n = 0;
            let mut last = w.eval(at(0.0)).signum();
            for k in 1..=720 {
                let a = k as f32 / 720.0 * std::f32::consts::TAU;
                let v = w.eval(at(a)).signum();
                if v != last {
                    n += 1;
                    last = v;
                }
            }
            n
        };
        let one = HarmonicWave::new(&map, 42, 1.0, 1);
        let thirteen = HarmonicWave::new(&map, 42, 13.0, 1);
        assert!(one.values().iter().all(|v| (-1.0..=1.0).contains(v)));
        let hi = thirteen.values().iter().copied().fold(f32::MIN, f32::max);
        let lo = thirteen.values().iter().copied().fold(f32::MAX, f32::min);
        assert!(
            (hi - 1.0).abs() < 1e-5 || (lo + 1.0).abs() < 1e-5,
            "the extreme tile reads ±1: {lo}..{hi}"
        );
        assert!(hi > 0.5 && lo < -0.5, "balanced: both sides reach well out");
        for w in [&one, &thirteen] {
            let area = &map.grid().area;
            let mean = w.values().iter().zip(area).map(|(v, a)| v * a).sum::<f32>()
                / area.iter().sum::<f32>();
            assert!(mean.abs() < 1e-4, "balanced about the datum: {mean}");
        }
        let (c1, c13) = (crossings(&one), crossings(&thirteen));
        // Angular distance runs out to the antipode and back along the circle,
        // so a ring field crosses between 2·f and 2·f + 2 times.
        assert!(
            (2..=4).contains(&c1),
            "frequency 1 crosses zero 2–4× on a great circle: {c1}"
        );
        assert!(
            (26..=28).contains(&c13),
            "frequency 13 crosses 26–28×: {c13}"
        );
        assert!((thirteen.frequency() - 13.0).abs() < 1e-6);
        let again = HarmonicWave::new(&map, 42, 13.0, 1);
        assert_eq!(thirteen.values(), again.values(), "same roll, same field");
        let mut grown = HarmonicWave::new(&map, 42, 13.0, 1);
        grown.set_axes(&map, 3);
        assert_eq!(
            grown.waves[0], thirteen.waves[0],
            "the count grows the same pattern"
        );
        assert_ne!(grown.values(), thirteen.values());
        let mut rolled = HarmonicWave::new(&map, 42, 13.0, 3);
        rolled.randomize(&map);
        assert_ne!(rolled.values(), grown.values(), "a re-roll moves it");
        let hundred = HarmonicWave::new(&map, 7, 100.0, 1);
        assert!(crossings(&hundred) > crossings(&HarmonicWave::new(&map, 7, 47.0, 1)));
        assert!(hundred.values().iter().all(|v| (-1.0..=1.0).contains(v)));
        // The closed form agrees with the tile (acos near an axis is ill-conditioned
        // in f32, so the tolerance is a few thousandths at frequency 13).
        for t in map.tiles().filter(|t| map.neighbours(*t).len() == 5) {
            assert!((thirteen.eval(map.direction(t)) - thirteen.value(t)).abs() < 5e-3);
        }
    }

    /// **The alpha mask admits here and mutes there.** Bounded 0..1, broadly
    /// half-and-half, pushed toward the rails (a mask, not a second relief),
    /// the roll is the identity and a new seed moves it, and pentagons read the
    /// same function.
    #[test]
    fn the_alpha_mask_is_bounded_bimodal_and_seeded() {
        let map = HexMap::new(MIN_FREQ);
        let a = WaveMask::new(&map, 42);
        assert_eq!(a.values().len(), map.len());
        assert!(a.values().iter().all(|v| (0.0..=1.0).contains(v)));
        let mean = a.values().iter().sum::<f32>() / map.len() as f32;
        assert!((0.3..=0.7).contains(&mean), "about half admitted: {mean}");
        let rails = a
            .values()
            .iter()
            .filter(|v| **v < 0.05 || **v > 0.95)
            .count();
        assert!(
            rails * 2 > map.len(),
            "most of the sphere sits on a rail: {rails}"
        );
        let b = WaveMask::new(&map, 42);
        assert_eq!(a.values(), b.values(), "same roll, same mask");
        let mut c = WaveMask::new(&map, 42);
        c.set_seed(&map, 7);
        assert_ne!(a.values(), c.values(), "a new seed moves the mask");
        for t in map.tiles().filter(|t| map.neighbours(*t).len() == 5) {
            assert!((a.eval(map.direction(t)) - a.value(t)).abs() < 1e-6);
        }
    }

    /// **A volcano emits waves, not a stamp.** Zero vents emit nothing; one
    /// vent's field stands within its reach and is level past the cull, scales
    /// with the heat under it and with the amplitude dial, and reads the same
    /// through the closed form as through the tiles; two vents ADD; and two
    /// vents are not the same figure — sampled at the same offsets from each,
    /// their waves differ (their reach, trains and phases are their own).
    #[test]
    fn a_volcano_emits_waves_not_a_stamp() {
        let map = HexMap::new(MIN_FREQ);
        let seams = SeamField::new(&map, 12, 8, 42);
        let none = Volcanoes::new(&map, &seams, &[], VolcanoShape::default());
        assert!(none.values().iter().all(|v| *v == 0.0));
        assert_eq!(none.count(), 0);
        // The two hottest tiles, apart, as the vents.
        let mut hot: Vec<(TileId, f32)> = (0..map.len() as TileId)
            .map(|t| (t, seams.heat(t)))
            .collect();
        hot.sort_by(|a, b| b.1.total_cmp(&a.1));
        let vent = hot[0].0;
        let other = hot
            .iter()
            .find(|(t, _)| map.direction(*t).dot(map.direction(vent)) < 0.5)
            .map(|(t, _)| *t)
            .expect("a second hot vent far away");
        let shape = VolcanoShape::default();
        let one = Volcanoes::new(&map, &seams, &[vent], shape);
        assert_eq!(one.count(), 1);
        let e = &one.emitters()[0];
        assert!(e.reach > 0.0 && e.trains.len() == shape.rings.round() as usize);
        let cull = (e.reach * VOLCANO_REACH).cos();
        let mut inside = 0usize;
        for t in 0..map.len() as TileId {
            let d = map.direction(t);
            let v = one.value(t);
            if d.dot(e.dir) < cull {
                assert_eq!(v, 0.0, "level past the cull");
            } else if v.abs() > 1e-6 {
                inside += 1;
            }
            // To f32's precision through acos near the vent: a unit direction
            // re-normalized moves the angle ~1e-6 rad, a wave of λ ~ 0.08 rad
            // by a few millionths of the amplitude.
            assert!(
                (one.eval(d) - v).abs() < 2e-5,
                "the closed form is the field: tile {t} eval {} vs {v}",
                one.eval(d)
            );
        }
        assert!(inside > 10, "the waves stand within reach: {inside} tiles");
        // Heat and the dial scale the waves.
        let cold = SeamField::new(&map, 12, 8, 42);
        let _ = cold;
        let louder = Volcanoes::new(
            &map,
            &seams,
            &[vent],
            VolcanoShape {
                amp: shape.amp * 2.0,
                ..shape
            },
        );
        for t in 0..map.len() as TileId {
            assert!((louder.value(t) - 2.0 * one.value(t)).abs() < 1e-6);
        }
        // Two vents add.
        let two = Volcanoes::new(&map, &seams, &[vent, other], shape);
        let second = Volcanoes::new(&map, &seams, &[other], shape);
        for t in 0..map.len() as TileId {
            assert!((two.value(t) - (one.value(t) + second.value(t))).abs() < 1e-6);
        }
        // Not a stamp: the two figures differ at the same offsets.
        let (a, b) = (&one.emitters()[0], &second.emitters()[0]);
        assert!(
            a.reach != b.reach || a.trains != b.trains,
            "two vents, two rolls"
        );
        let ea = Vec3::Y.cross(a.dir).normalize();
        let eb = Vec3::Y.cross(b.dir).normalize();
        let mut differ = 0usize;
        for k in 1..12 {
            let off = 0.2 * k as f32 * a.reach.min(b.reach);
            let pa = (a.dir + ea * off).normalize();
            let pb = (b.dir + eb * off).normalize();
            let (fa, fb) = (
                a.figure(shape.arms, pa) / a.heat.max(1e-6),
                b.figure(shape.arms, pb) / b.heat.max(1e-6),
            );
            if (fa - fb).abs() > 0.05 {
                differ += 1;
            }
        }
        assert!(differ >= 6, "the figures differ: {differ}/11 offsets");
    }

    /// **The sea dial runs floor to crest, then floods.** 0 stands at the lowest
    /// tile, 98 at the highest, the ramp between is linear, and 100 stands a
    /// clear margin above every crest — including over a flat world.
    #[test]
    fn the_sea_dial_runs_floor_to_crest_then_floods() {
        let (lo, hi) = (-0.06f32, 0.06f32);
        assert_eq!(sea_level(lo, hi, 0.0), lo);
        assert!(
            (sea_level(lo, hi, 49.0) - 0.0).abs() < 1e-6,
            "halfway up the ramp is the mean"
        );
        assert!((sea_level(lo, hi, SEA_CREST_PERCENT) - hi).abs() < 1e-6);
        assert!(sea_level(lo, hi, 99.0) > hi);
        assert!((sea_level(lo, hi, 100.0) - (hi + 0.12 * FLOOD_MARGIN)).abs() < 1e-6);
        assert!(
            sea_level(0.0, 0.0, 100.0) > 0.0,
            "a flat world still floods"
        );
        assert_eq!(sea_level(lo, hi, -5.0), lo, "the dial has ends");
        let map = HexMap::new(MIN_FREQ);
        let w = PlanetWave::new(&map, 42);
        let h = |t: TileId| w.value(t) * 0.06;
        assert!((land_share(&map, h, sea_level(lo, hi, 0.0)) - 1.0).abs() < 1e-3);
        assert_eq!(land_share(&map, h, sea_level(lo, hi, 100.0)), 0.0);
    }
}
