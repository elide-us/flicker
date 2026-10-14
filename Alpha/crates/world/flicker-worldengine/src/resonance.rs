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

use glam::Vec3;

use crate::map::{HexMap, TileId};
use crate::seams::SeamField;

/// The lobe sharpness — how fast a pole's influence falls with angular
/// distance. The twelve poles stand ~63.4° apart; at 6 a pole's lobe has fallen
/// to ~e⁻³ by its neighbours, so the crests read as twelve distinct domes and
/// the saddles between opposite-signed neighbours run as clean seams.
pub const DEFAULT_LOBE: f32 = 6.0;
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

// ── THE HARMONICS (Aaron 2026-10-13: "much larger wave patterns — an 8/9th
// frequency wave from the planet scale, same basic mechanics as the first wave,
// just a different resonance; and a 1/15th and a 1/13th vibration field") ──

/// The base wave's angular WAVELENGTH, crest to crest: twice the pentagon
/// spacing of the icosahedron (`2·acos(1/√5)` ≈ 126.9°). A harmonic at ratio
/// `f` has wavelength `BASE_WAVELENGTH / f` — the 8/9 wave is a little longer
/// than… no: at ratio 8/9 its FREQUENCY is 8/9 of the base (a slightly longer
/// wave); at ratio 13 or 15 its wavelength is a thirteenth or a fifteenth of
/// the base — the "1/13th" and "1/15th" vibrations.
pub const BASE_WAVELENGTH: f32 = 2.0 * 1.107_148_7; // 2·acos(1/√5)

/// The harmonics' axis count range — how many plane waves of the one frequency
/// superpose into the standing pattern (one = a pure band pattern; several =
/// an interference lattice, the "resonance").
pub const MIN_AXES: u32 = 1;
pub const MAX_AXES: u32 = 6;
pub const DEFAULT_AXES: u32 = 3;

/// **A harmonic of the planetary wave** over one tiling: a seeded superposition
/// of `axes` plane waves, all at ONE spatial frequency — `ratio` times the base
/// wave's — normalized so the crest tile reads `+1`. Same mechanics as the base
/// wave (closed-form in (position, seed), the roll is the identity, mean ≈ 0,
/// topology-blind); the difference is the resonance.
pub struct HarmonicWave {
    seed: u64,
    ratio: f32,
    axes: u32,
    waves: Vec<(Vec3, f32)>,
    norm: f32,
    field: Vec<f32>,
}

impl HarmonicWave {
    /// Roll a harmonic at `ratio` × the base frequency with `axes` plane waves.
    pub fn new(map: &HexMap, seed: u64, ratio: f32, axes: u32) -> Self {
        let mut w = Self {
            seed,
            ratio: ratio.max(1e-3),
            axes: axes.clamp(MIN_AXES, MAX_AXES),
            waves: Vec::new(),
            norm: 1.0,
            field: Vec::new(),
        };
        w.rebuild(map);
        w
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The frequency ratio to the base wave.
    pub fn ratio(&self) -> f32 {
        self.ratio
    }

    pub fn axes(&self) -> u32 {
        self.axes
    }

    /// The angular wave number: `2π / (BASE_WAVELENGTH / ratio)`.
    pub fn wave_number(&self) -> f32 {
        std::f32::consts::TAU * self.ratio / BASE_WAVELENGTH
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
        self.raw(p.normalize_or_zero()) / self.norm
    }

    fn raw(&self, p: Vec3) -> f32 {
        let k = self.wave_number();
        self.waves
            .iter()
            .map(|(axis, phase)| (k * p.dot(*axis) + phase).sin())
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
        self.norm = 1.0;
        let raw: Vec<f32> = map.grid().dirs.iter().map(|d| self.raw(*d)).collect();
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

    /// The closed form at any direction.
    pub fn eval(&self, p: Vec3) -> f32 {
        let raw: f32 = self
            .waves
            .iter()
            .map(|(axis, amp, freq, phase)| amp * (freq * p.dot(*axis) + phase).sin())
            .sum::<f32>()
            .clamp(-1.0, 1.0);
        // Smoothstep from −edge to +edge: a soft but short transition.
        let t = ((raw + MASK_EDGE) / (2.0 * MASK_EDGE)).clamp(0.0, 1.0);
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
// looking pattern of hard molten rock" — SHAPE only for now; the material
// portion and the drops-over-ticks come later) ──

/// The emitters' authored shape, the EMITTERS tab's dials.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VolcanoShape {
    /// The figure's reach around each vent, in tile widths.
    pub radius_tiles: f32,
    /// The snowflake's fold — how many arms the ripple pattern carries.
    pub arms: u32,
    /// Ripple rings across the radius — the figure's spatial frequency.
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

/// How far past the nominal radius a figure's ripples fade to nothing.
const VOLCANO_REACH: f32 = 1.5;
/// The cone's width as a share of the radius.
const VOLCANO_CONE: f32 = 0.45;
/// The ripple's share of the cone's peak.
const VOLCANO_RIPPLE: f32 = 0.5;

/// **The volcanic emitters' shape field**: every vent of the crust (the heat
/// field's consequence) stamps a cone with an `arms`-fold ripple around it —
/// the snowflake — scaled by the molten heat under the vent and fading past
/// its reach. Nothing is placed: the vents are where the heat broke through,
/// the figure is one function of (distance, bearing) from each. Sums where
/// figures overlap, which is where the hard/soft alternation will come from
/// once material rides the ripples.
pub struct Volcanoes {
    shape: VolcanoShape,
    field: Vec<f32>,
    count: usize,
}

impl Volcanoes {
    /// Stamp every vent in `vents` over `map`, heat read from `seams`.
    pub fn new(map: &HexMap, seams: &SeamField, vents: &[TileId], shape: VolcanoShape) -> Self {
        let mut v = Self {
            shape,
            field: Vec::new(),
            count: 0,
        };
        v.rebuild(map, seams, vents);
        v
    }

    /// The shape the field was stamped with.
    pub fn shape(&self) -> VolcanoShape {
        self.shape
    }

    /// Restamp with a new shape (a dial moved). A no-op at the same shape.
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

    /// How many emitters stamped.
    pub fn count(&self) -> usize {
        self.count
    }

    /// A tile's height from the emitters, radius units; out of range is level.
    pub fn value(&self, tile: TileId) -> f32 {
        self.field.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Every tile's height.
    pub fn values(&self) -> &[f32] {
        &self.field
    }

    /// One vent's figure at `d`: the cone + the `arms`-fold ripple, in units of
    /// the full-heat peak — the closed form every tier can read.
    pub fn figure(shape: &VolcanoShape, vent: Vec3, d: Vec3, radius: f32) -> f32 {
        let r = vent.dot(d).clamp(-1.0, 1.0).acos() / radius.max(1e-6);
        if r > VOLCANO_REACH {
            return 0.0;
        }
        // The bearing in the vent's tangent frame (east, north).
        let east = Vec3::Y.cross(vent).normalize_or_zero();
        let east = if east.length_squared() < 1e-6 {
            Vec3::X
        } else {
            east
        };
        let north = vent.cross(east);
        let off = d - vent;
        let theta = off.dot(north).atan2(off.dot(east));
        let cone = (-(r / VOLCANO_CONE).powi(2)).exp();
        let ring = (std::f32::consts::TAU * shape.rings * r).cos();
        let petals = 0.5 + 0.5 * (shape.arms as f32 * theta).cos();
        let fade = (1.0 - r / VOLCANO_REACH).max(0.0);
        cone + VOLCANO_RIPPLE * ring * petals * fade
    }

    /// Stamp every vent: the figure scaled by the heat under it.
    pub fn rebuild(&mut self, map: &HexMap, seams: &SeamField, vents: &[TileId]) {
        let n = map.len();
        self.field = vec![0.0; n];
        self.count = vents.len();
        let tile_w = 2.0 / (n as f32).sqrt() * 2.0;
        let radius = self.shape.radius_tiles.max(0.5) * tile_w;
        let near = (radius * VOLCANO_REACH).cos();
        let dirs = &map.grid().dirs;
        for &v in vents {
            let vd = map.direction(v);
            let heat = seams.heat(v);
            for (i, d) in dirs.iter().enumerate() {
                if d.dot(vd) < near {
                    continue;
                }
                self.field[i] += self.shape.amp * heat * Self::figure(&self.shape, vd, *d, radius);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;

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

    /// **A harmonic is the base mechanics at another resonance.** Bounded ±1 with
    /// the crest tile at +1, mean near zero, the roll is the identity, the axis
    /// count grows the same pattern, and the RATIO sets the spatial frequency: the
    /// 1/13 vibration crosses zero along a great circle about 13 times as often as
    /// the base-frequency harmonic does.
    #[test]
    fn a_harmonic_is_the_base_mechanics_at_another_resonance() {
        let map = HexMap::new(MIN_FREQ);
        let crossings = |w: &HarmonicWave| {
            // Walk a great circle through 720 samples; count sign changes.
            let mut n = 0;
            let mut last = w.eval(Vec3::X).signum();
            for k in 1..=720 {
                let a = k as f32 / 720.0 * std::f32::consts::TAU;
                let v = w.eval(Vec3::new(a.cos(), 0.0, a.sin())).signum();
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
        assert!((hi - 1.0).abs() < 1e-5, "the crest tile reads +1");
        let mean = thirteen.values().iter().sum::<f32>() / map.len() as f32;
        assert!(mean.abs() < 0.05, "mean near zero: {mean}");
        let (c1, c13) = (crossings(&one), crossings(&thirteen));
        assert!(c1 >= 2, "the base-frequency harmonic crosses zero: {c1}");
        let r = c13 as f32 / c1 as f32;
        assert!(
            (9.0..=17.0).contains(&r),
            "the 1/13 vibration crosses ~13× as often: {c13}/{c1}"
        );
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
        let eight_ninths = HarmonicWave::new(&map, 7, 8.0 / 9.0, 3);
        assert!(crossings(&eight_ninths) < crossings(&HarmonicWave::new(&map, 7, 2.0, 3)));
        for t in map.tiles().filter(|t| map.neighbours(*t).len() == 5) {
            assert!((thirteen.eval(map.direction(t)) - thirteen.value(t)).abs() < 1e-5);
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

    /// **A volcano is a cone with a snowflake around it.** Zero vents stamp
    /// nothing; one vent peaks at its own tile, carries `arms`-fold symmetry in
    /// its ripple (the figure repeats every 2π/arms of bearing), is level past
    /// its reach, and scales with the heat under it; two vents add.
    #[test]
    fn a_volcano_is_a_cone_with_an_arms_fold_snowflake() {
        let map = HexMap::new(MIN_FREQ);
        let seams = SeamField::new(&map, 12, 8, 42);
        let none = Volcanoes::new(&map, &seams, &[], VolcanoShape::default());
        assert!(none.values().iter().all(|v| *v == 0.0));
        assert_eq!(none.count(), 0);
        // The hottest tile as the one vent.
        let (vent, _) = (0..map.len() as TileId)
            .map(|t| (t, seams.heat(t)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let shape = VolcanoShape::default();
        let one = Volcanoes::new(&map, &seams, &[vent], shape);
        assert_eq!(one.count(), 1);
        // The peak is the cone (1) plus the ripple's centre (its share), times
        // the heat under the vent.
        let peak = one.value(vent);
        let want = shape.amp * seams.heat(vent) * (1.0 + VOLCANO_RIPPLE);
        assert!(
            (peak - want).abs() < 1e-5,
            "the vent tile is the cone's peak: {peak} vs {want}"
        );
        assert!(
            one.values().iter().all(|v| *v <= peak + 1e-6),
            "nothing out-tops the vent"
        );
        let tile_w = 2.0 / (map.len() as f32).sqrt() * 2.0;
        let radius = shape.radius_tiles * tile_w;
        let vd = map.direction(vent);
        let far = map
            .tiles()
            .filter(|t| vd.dot(map.direction(*t)).clamp(-1.0, 1.0).acos() > radius * 1.6)
            .count();
        assert!(far > map.len() / 2);
        for t in map.tiles() {
            if vd.dot(map.direction(t)).clamp(-1.0, 1.0).acos() > radius * 1.6 {
                assert_eq!(one.value(t), 0.0, "level past the reach");
            }
        }
        // ARMS-fold symmetry of the closed form: the figure at bearing θ equals
        // the figure at θ + 2π/arms at the same distance.
        let east = Vec3::Y.cross(vd).normalize();
        let north = vd.cross(east);
        let r = radius * 0.8;
        let at = |theta: f32| {
            let d =
                (vd * r.cos() + (east * theta.cos() + north * theta.sin()) * r.sin()).normalize();
            Volcanoes::figure(&shape, vd, d, radius)
        };
        let step = std::f32::consts::TAU / shape.arms as f32;
        for k in 0..shape.arms {
            let th = 0.37 + k as f32 * step;
            assert!((at(th) - at(th + step)).abs() < 1e-4, "arm {k} repeats");
        }
        assert!(
            (at(0.0) - at(step * 0.5)).abs() > 0.05,
            "…and between arms it differs"
        );
        // Two vents add where they overlap.
        let nb = map.neighbours(vent)[0];
        let two = Volcanoes::new(&map, &seams, &[vent, nb], shape);
        assert!(two.value(vent) > one.value(vent), "a second vent adds");
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
