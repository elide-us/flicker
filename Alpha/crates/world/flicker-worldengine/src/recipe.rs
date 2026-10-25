//! **THE FIELD RECIPE — the V5 stack as DATA, composed the one way.**
//!
//! What the WorldBench's dials hold, as a record: every field's seed and dials,
//! the sea dial, the map size. [`compose`] turns a recipe into the composed
//! HEIGHT channel and the sea — the same arithmetic the bench draws, stated
//! once here so a headless evaluator and the window cannot disagree (the bench
//! carries a gate that its heights equal `compose(bench.recipe())`).
//!
//! On top of that, the two things a shape iteration loop needs without a GPU:
//! - [`metrics`] — MEASURED properties of the composed shape (land share,
//!   continents, coastline, where the ridges stand, the ocean floor's relief),
//!   so a dial sweep can be scored against Aaron's targets (seven continents on
//!   ~30% land, ridges on continental edges, basins and seabeds, ripples);
//! - [`render_equirect`] — the sphere DE-PROJECTED to a flat map with
//!   hypsometric tints and elevation contour rings, the civil-engineering
//!   drawing a reviewer (human or agent) reads the shape from.
//!
//! **A field is a frequency across the whole planet and an alpha mask — that is
//! it** (Aaron 2026-10-15). The stack is the planetary wave (the frequency-1
//! standing wave on the pentagons) plus a LIST of resonance slots — any count,
//! any frequencies — each with its mask, superposed. Nothing traces a contour,
//! stamps a feature or reshapes a value: an earlier coast band, wave ridge and
//! plateau exponent did exactly that and were removed as drawing, not resonance.
//!
//! None of this is simulation: the recipe is authored data, the composition a
//! pure function, the metrics a reading. The transformations-not-outcomes law
//! (935269B7) binds the FIELDS; a recipe is the dials a game master turns.

use crate::rocks::Rock;
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::bubbles::{BubbleRecipe, Bubbles};
use crate::crust::CrustField;
use crate::epochs::{raft_closing, raft_turns, Rafts};
use crate::map::{HexMap, TileId};
use crate::resonance::{
    land_share, sea_level, HarmonicWave, PlanetWave, VolcanoShape, Volcanoes, WaveMask,
    DEFAULT_AXES, DEFAULT_LOBE,
};
use crate::seams::{push_field, SeamField, DEFAULT_CELLS, DEFAULT_SPOTS};

/// A field's alpha mask: its roll and whether it applies.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskRecipe {
    pub seed: u64,
    pub on: bool,
}

/// The planetary wave's record — the frequency-1 field.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveRecipe {
    pub seed: u64,
    /// Relief as a fraction of the radius.
    pub amp: f32,
    pub lobe: f32,
    pub mask: MaskRecipe,
    pub on: bool,
}

/// How a slot's alpha mask is CUT (Aaron 2026-10-15: *"intentional masks and
/// intentional waves"*): not at all; by REGION — the mask wave's broad
/// blotches, the layer here and not there; or along the SEAM — the margins of
/// the land masses the waves made where the floor's push closes on them (the
/// subduction margins: *"the seams are the points where transitions between
/// continental shelf and ocean seabed are found"*), the ribbons where a
/// mountain layer stands as ranges and where the epochs' compression stacks
/// more. (A first cut used the mask wave's nodal lines, a second a rolled
/// plate scheme's edges: ridges that followed nothing — Aaron: "the seams
/// don't seem to follow anything in particular".)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaskKind {
    Off,
    Region,
    Seam,
}

impl MaskKind {
    /// The kinds in the bench's pill order; an index is a number.
    pub const ALL: [MaskKind; 3] = [MaskKind::Off, MaskKind::Region, MaskKind::Seam];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }
}

/// One resonance SLOT — a wave train across the whole planet: its frequency,
/// its amplitude (the game master's dial; the stack's SPREAD rolls what is
/// heard around it), and how its alpha mask is cut. Its roll and its mask's
/// roll are the STACK's, resolved by slot and frequency.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotRecipe {
    /// Cycles per great circle.
    pub frequency: f32,
    /// Relief as a fraction of the radius.
    pub amp: f32,
    /// Sinusoids in the train — 1 is one long-crested swell; 3–4 is an
    /// interference lattice of peaks, which is what a mountain layer or a field
    /// of hills wants (one train at a mountain's frequency is corduroy).
    pub axes: u32,
    pub on: bool,
    pub mask: MaskKind,
}

/// **The spectral law** for a LIST of frequencies with no amplitudes of their
/// own (the evaluator's `res.list=`): `relief · f^(−tilt)` — the finer the
/// wave, the lower it stands.
pub fn spectral_amp(relief: f32, tilt: f32, frequency: f32) -> f32 {
    relief * frequency.max(1e-3).powf(-tilt)
}

/// A resonance's own roll and mask derive from the stack's one seed and the
/// frequency, so the stack re-rolls as one and a frequency keeps its identity
/// across the list.
pub fn resonance_seed(stack_seed: u64, frequency: f32) -> u64 {
    stack_seed ^ (u64::from(frequency.to_bits())).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// A slot's roll: the stack's seed, the slot's place in the list and its
/// frequency — so two slots at ONE frequency are two different waves (Aaron:
/// *"room for multiple large waves with multiple amplitudes"*).
pub fn slot_seed(stack_seed: u64, index: usize, frequency: f32) -> u64 {
    resonance_seed(stack_seed, frequency) ^ (index as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93)
}

/// **The opening LAYERS** — frequencies and heights picked for what they MAKE
/// on an Earth-sized ball (great circle ≈ 24,900 mi; the standard tile ≈ 50
/// mi), each with the mask that cuts it: `(cycles per great circle, relief as
/// a fraction of the radius, sinusoids in the train, mask)`. Aaron 2026-10-15:
/// *"the waves should make
/// continents and seabeds, that's one layer, and then there should be
/// mountains, but those are more like a seam that might show up in several
/// ridges around the entire planet … the overall ground at the small scale
/// makes small hills … be intelligent about picking wave frequencies that
/// produce earth-like hills and dunes and mountains."* Dunes are finer than a
/// 50-mile tile; they are the cluster scale's.
pub const DEFAULT_LAYERS: [(f32, f32, u32, MaskKind); 12] = [
    // Continents: 12,000-mile swells, one train each, unmasked so they sweep
    // the whole ball; the second on another axis breaks the first's symmetry.
    (2.0, 0.020, 1, MaskKind::Off),
    (3.0, 0.012, 1, MaskKind::Off),
    // Seabeds and basins: 4,000-mile hollows, here and not there.
    (6.0, 0.006, 2, MaskKind::Region),
    // Shelves and plateaus: 2,300-mile steps, regional.
    (11.0, 0.003, 2, MaskKind::Region),
    // Mountains: 600-mile peak spacing in a three-train lattice, LIFTED only
    // along the seams.
    (40.0, 0.015, 3, MaskKind::Seam),
    // Foothills: the same seams, finer and lower.
    (80.0, 0.005, 3, MaskKind::Seam),
    // Hills: 390-mile rolls (eight tiles) from four directions, bumpy here,
    // flat there. Eight tiles is what the hex grid RESOLVES: at 160 cycles
    // (three tiles) the lattice aliased against the grid and the hill-shade
    // drew every continent in diagonal corduroy (the 2026-10-16 sweep).
    (64.0, 0.0012, 4, MaskKind::Region),
    // UNDER THE HEX — the same layers, finer (Aaron 2026-10-16: *"a fine
    // expression of THE SAME ALGORITHM"*): knob lattices of five or six
    // trains every two octaves from a 100-mile roll to a half-mile hummock,
    // bumpy here and flat there by their own region masks, or everywhere.
    // The hex grid does not carry them (`HexMap::resolving_frequency`): the
    // 2K hex evaluates them exactly at every pixel, where they are what the
    // ground does between the centres. Heights authored like every layer's
    // — a planet's spectrum is no one law from the continents down.
    (256.0, 0.0006, 5, MaskKind::Region),
    (1024.0, 0.0004, 6, MaskKind::Off),
    (4096.0, 0.0003, 5, MaskKind::Region),
    (16384.0, 0.0002, 6, MaskKind::Off),
    (49152.0, 0.0001, 5, MaskKind::Region),
];

/// **The resonance stack** — its dials and its slots. The bench's RESONANCE tab
/// is exactly this record: SPREAD, SEAM, MASK SEED, the re-roll
/// (`seed`), the FIELDS switch (`on`) and one row per slot; the evaluator's
/// `res.*` and `s<i>.*` keys are its fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackRecipe {
    /// The stack's roll: every slot's axes and phases, and how far its heard
    /// amplitude strays from its dial, derive from it.
    pub seed: u64,
    /// The masks' roll, likewise.
    pub mask_seed: u64,
    /// How far a slot's HEARD amplitude may stray from its dial, in OCTAVES:
    /// 1 lets each slot sound anywhere from half to double, 0 pins it.
    pub spread: f32,
    /// The SEAM ribbon's half-width, in rings (tiles) from a closing margin.
    pub seam: f32,
    pub on: bool,
    /// Any count, any frequencies.
    pub slots: Vec<SlotRecipe>,
}

impl StackRecipe {
    /// A stack over the opening layers.
    pub fn layers(seed: u64, mask_seed: u64, spread: f32, seam: f32) -> Self {
        Self {
            seed,
            mask_seed,
            spread,
            seam,
            on: true,
            slots: DEFAULT_LAYERS
                .iter()
                .map(|&(frequency, amp, axes, mask)| SlotRecipe {
                    frequency,
                    amp,
                    axes,
                    on: true,
                    mask,
                })
                .collect(),
        }
    }

    /// A stack over `frequencies` with no heights of their own, on the
    /// spectral law, every slot on with a REGION mask.
    pub fn over(frequencies: &[f32], seed: u64, mask_seed: u64, relief: f32, tilt: f32) -> Self {
        Self {
            seed,
            mask_seed,
            spread: Recipe::DEFAULT_SPREAD,
            seam: Recipe::DEFAULT_SEAM,
            on: true,
            slots: frequencies
                .iter()
                .map(|&f| SlotRecipe {
                    frequency: f,
                    amp: spectral_amp(relief, tilt, f),
                    axes: DEFAULT_AXES,
                    on: true,
                    mask: MaskKind::Region,
                })
                .collect(),
        }
    }

    /// The roll's stray at a slot: `2^(±spread)` on the stack's seed.
    pub fn stray(&self, index: usize, frequency: f32) -> f32 {
        let mut rng = fastrand::Rng::with_seed(
            slot_seed(self.seed, index, frequency) ^ 0x414D_504C_4954_5544,
        );
        ((rng.f32() * 2.0 - 1.0) * self.spread).exp2()
    }

    /// What a slot is HEARD at: its dial, strayed by the roll.
    pub fn heard(&self, index: usize, slot: &SlotRecipe) -> f32 {
        slot.amp * self.stray(index, slot.frequency)
    }

    /// A slot's roll.
    pub fn field_seed(&self, index: usize, frequency: f32) -> u64 {
        slot_seed(self.seed, index, frequency)
    }

    /// A slot's mask roll.
    pub fn mask_seed_of(&self, index: usize, frequency: f32) -> u64 {
        slot_seed(self.mask_seed, index, frequency)
    }

    /// How many slots sound.
    pub fn active(&self) -> usize {
        self.slots.iter().filter(|s| s.on).count()
    }
}

/// The heat field's record (the molten seams), and the emitters stamped on
/// its vents.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeatRecipe {
    pub seed: u64,
    pub cells: u32,
    pub spots: u32,
    pub on: bool,
    pub emitters: VolcanoShape,
    pub emitters_on: bool,
}

/// **The whole stack.**
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub freq: u32,
    pub wave: WaveRecipe,
    pub stack: StackRecipe,
    pub heat: HeatRecipe,
    /// The sea dial, percent.
    pub sea_percent: f32,
    /// BUBBLES ON BUBBLES — domes and bowls at every scale
    /// ([`crate::bubbles`]); a recipe written before it reads as the default
    /// roll.
    #[serde(default)]
    pub bubbles: BubbleRecipe,
}

impl Recipe {
    /// **The opening dials** — the planetary wave at 2% relief under the
    /// opening layers; the heard amplitudes stray half an octave; the seam
    /// ribbons a tenth of the mask's swing wide; the sea where ~30% stands dry
    /// (24-seed mean, 2026-10-15); the mask rolls that averaged best.
    pub const DEFAULT_WAVE_AMP: f32 = 0.02;
    pub const DEFAULT_WAVE_MASK_SEED: u64 = 7;
    pub const DEFAULT_SPREAD: f32 = 0.5;
    pub const DEFAULT_SEAM: f32 = 3.0;
    pub const DEFAULT_STACK_MASK_SEED: u64 = 42;
    pub const DEFAULT_SEA_PERCENT: f32 = 70.0;
    pub const DEFAULT_EMITTER_AMP: f32 = 0.005;
    /// **The seed the WorldBench opens on** — found by the seed sweep
    /// (`resonance_eval --seed-sweep`, the contact sheet judged at 0 and 300
    /// epochs, 2026-10-16), never a lucky world: the window opens on the world
    /// the agent judged, and RANDOMIZE rolls another.
    pub const OPENING_SEED: u64 = 49;

    /// The WorldBench's opening recipe at `seed`: every field's roll derived
    /// from the one seed (so a sweep over seeds is a sweep over worlds), the
    /// dials at the opening preset — the planetary wave, the resonance stack
    /// over [`DEFAULT_LAYERS`], the heat and its emitters, the sea.
    pub fn default_at(freq: u32, seed: u64) -> Self {
        // Wrapping throughout: the seventh sub-roll's stride overflows a u64
        // (7 × 0x2545…DD1D), which a release build wraps and a debug build
        // traps — the window crashed on it once (2026-10-16).
        let sub = |k: u64| {
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(k.wrapping_mul(0x2545_F491_4F6C_DD1D))
        };
        Self {
            freq,
            wave: WaveRecipe {
                seed: sub(1),
                amp: Self::DEFAULT_WAVE_AMP,
                lobe: DEFAULT_LOBE,
                mask: MaskRecipe {
                    seed: Self::DEFAULT_WAVE_MASK_SEED,
                    on: true,
                },
                on: true,
            },
            stack: StackRecipe::layers(
                sub(2),
                Self::DEFAULT_STACK_MASK_SEED,
                Self::DEFAULT_SPREAD,
                Self::DEFAULT_SEAM,
            ),
            heat: HeatRecipe {
                seed: sub(5),
                cells: DEFAULT_CELLS,
                spots: DEFAULT_SPOTS,
                on: true,
                emitters: VolcanoShape {
                    amp: Self::DEFAULT_EMITTER_AMP,
                    ..VolcanoShape::default()
                },
                emitters_on: true,
            },
            sea_percent: Self::DEFAULT_SEA_PERCENT,
            bubbles: BubbleRecipe::rolled(sub(8)),
        }
    }
}

/// The composed shape: the height channel per tile (radius units), the
/// standing sea (the base wave's datum), and the HARDNESS channel per tile —
/// the first non-height channel of the field stack (Aaron 2026-10-15: "get
/// the hardness ledger working"): how resistant the crust is, `0..1`, born
/// from the layers that made the relief there, each layer's own hardness
/// weighed by how much of the relief it stood up. What erosion will read.
#[derive(Clone, Debug, PartialEq)]
pub struct Composed {
    pub heights: Vec<f32>,
    pub sea: f32,
    pub hardness: Vec<f32>,
}

/// **The layers' rocks** — what each kind of crust is born as, named from
/// the engine's catalog ([`crate::rocks::Rock`], ruling 4EECFD93): the
/// planetary wave and the coarse region layers are the continental BASEMENT
/// (granite); a SEAM layer's orogenic belts are gneiss; the fine region
/// layers (the hills and seabeds at or above [`FLOOR_TEXTURE_MIN_FREQ`])
/// are the floor's basalt, a shade softer than basement — never
/// sediment-soft: the hills are crust, and the era's cut would plane them
/// first (23D867F8); the emitters and young sea floor pour basalt; crust
/// compression has thickened to its ceiling is the quartzite root; a
/// strike leaves impact breccia; the bubbles are domes of the basement;
/// and loose SEDIMENT the era lays down is shale, the soft band. The
/// resistance each carries is the catalog's, not a number typed here.
///
/// What follows is the ledger that weighs them: every layer votes its rock's
/// resistance by the height it adds, and a tile reads the weighed mean.
/// The hardness ledger under composition: each layer's contribution at a
/// tile, weighed by its magnitude, votes its own hardness.
struct HardnessLedger {
    num: Vec<f32>,
    den: Vec<f32>,
}

impl HardnessLedger {
    fn new(n: usize) -> Self {
        Self {
            num: vec![0.0; n],
            den: vec![0.0; n],
        }
    }

    fn add(&mut self, t: usize, contribution: f32, hardness: f32) {
        let w = contribution.abs();
        self.num[t] += w * hardness;
        self.den[t] += w;
    }

    /// The hardness per tile: the layers' weighed vote, or the basement's
    /// where nothing stood anything up.
    fn resolve(self) -> Vec<f32> {
        self.num
            .iter()
            .zip(&self.den)
            .map(|(n, d)| {
                if *d > 1e-12 {
                    n / d
                } else {
                    Rock::Granite.resistance()
                }
            })
            .collect()
    }
}

/// **The seam ribbons** of a world: per tile, the strength of the nearest
/// CLOSING MARGIN — how hard the floor's push closes on the land mass there,
/// `0..1` — falling smoothly to `0` at `width` rings from it, walked inward
/// over the land and one ring onto the shelf. The cut a SEAM layer stands
/// through: ranges where the ocean goes under the land, nothing on a passive
/// margin or in the open ocean.
pub fn seam_ribbon_from(map: &HexMap, rafts: &Rafts, closing: &[f32], width: f32) -> Vec<f32> {
    let n = map.len();
    let top = closing.iter().copied().fold(0.0f32, f32::max);
    let mut ring = vec![u32::MAX; n];
    let mut strength = vec![0.0f32; n];
    let mut frontier: Vec<TileId> = (0..n as TileId)
        .filter(|t| closing[*t as usize] > 0.0)
        .collect();
    for t in &frontier {
        ring[*t as usize] = 0;
        strength[*t as usize] = if top > 0.0 {
            closing[*t as usize] / top
        } else {
            1.0
        };
    }
    let mut d = 0u32;
    let reach = width.max(0.0).ceil() as u32;
    while !frontier.is_empty() && d < reach {
        let mut next = Vec::new();
        for t in &frontier {
            for nb in map.neighbours(*t) {
                let j = *nb as usize;
                let onto_floor = rafts.home[j] == u32::MAX;
                if ring[j] == u32::MAX && (!onto_floor || d == 0) {
                    ring[j] = d + 1;
                    strength[j] = strength[*t as usize];
                    next.push(*nb);
                }
            }
        }
        frontier = next;
        d += 1;
    }
    ring.iter()
        .zip(&strength)
        .map(|(r, s)| {
            if *r == u32::MAX {
                0.0
            } else {
                let x = (*r as f32 / width.max(1e-6)).clamp(0.0, 1.0);
                s * (1.0 - x * x * (3.0 - 2.0 * x))
            }
        })
        .collect()
}

/// **A world's seam ribbons**: the land masses the recipe's waves make (the
/// composition WITHOUT its seam layers, which is what the seams are cut from),
/// their rigid turns under the heat's push, the margins the floor closes on,
/// cut at the stack's width. The composition's SEAM layers and the epochs'
/// compression share the mechanics.
pub fn seam_ribbon(map: &HexMap, r: &Recipe) -> Vec<f32> {
    let base = compose_with(map, r, &|_, s| s.mask != MaskKind::Seam);
    let rafts = Rafts::from_relief(map, &base.heights, base.sea);
    let seams = SeamField::new(map, r.heat.cells, r.heat.spots, r.heat.seed);
    let push = push_field(map, &seams, 1.0, 1.0);
    let omega = raft_turns(map, &rafts, &push);
    let closing = raft_closing(map, &rafts, &push, &omega);
    seam_ribbon_from(map, &rafts, &closing, r.stack.seam)
}

/// **Add the stack's slots** that `keep` admits to `heights`: each a wave
/// train at its heard amplitude through its cut — the seams (the plate
/// scheme's converging edges) cut once for every SEAM slot kept.
fn add_slots(
    map: &HexMap,
    r: &Recipe,
    heights: &mut [f32],
    keep: &dyn Fn(usize, &SlotRecipe) -> bool,
    mut ledger: Option<&mut HardnessLedger>,
) {
    // The grid carries the layers it resolves; the finer ones are the pixel
    // tier's (the one recipe, two resolutions).
    let carried = map.resolving_frequency();
    let kept: Vec<(usize, &SlotRecipe)> = r
        .stack
        .slots
        .iter()
        .enumerate()
        .filter(|(i, s)| s.on && s.frequency <= carried && keep(*i, s))
        .collect();
    let seams = kept
        .iter()
        .any(|(_, s)| s.mask == MaskKind::Seam)
        .then(|| seam_ribbon(map, r));
    for (i, slot) in kept {
        let wave = HarmonicWave::new(
            map,
            r.stack.field_seed(i, slot.frequency),
            slot.frequency,
            slot.axes,
        );
        let amp = r.stack.heard(i, slot);
        let mask = (slot.mask == MaskKind::Region)
            .then(|| WaveMask::new(map, r.stack.mask_seed_of(i, slot.frequency)));
        let hardness = match slot.mask {
            MaskKind::Seam => Rock::Gneiss.resistance(),
            _ if slot.frequency >= FLOOR_TEXTURE_MIN_FREQ => Rock::Basalt.resistance(),
            _ => Rock::Granite.resistance(),
        };
        for (t, h) in heights.iter_mut().enumerate() {
            let v = wave.value(t as TileId);
            let c = match (&mask, &seams, slot.mask) {
                (Some(mk), _, MaskKind::Region) => v * amp * mk.value(t as TileId),
                // A SEAM LIFTS: the range stands along the plate boundary's
                // ribbon, one-signed, its peaks where the train crests.
                (_, Some(ribbon), MaskKind::Seam) => (0.5 + 0.5 * v) * amp * ribbon[t],
                _ => v * amp,
            };
            *h += c;
            if let Some(l) = ledger.as_deref_mut() {
                l.add(t, c, hardness);
            }
        }
    }
}

/// The ocean layers' frequency floor: what young sea floor is born with — the
/// seabeds and everything finer, never the continental swells.
pub const FLOOR_TEXTURE_MIN_FREQ: f32 = 6.0;

/// **The floor's texture** — the stack's fine, unseamed layers (frequency at
/// or above [`FLOOR_TEXTURE_MIN_FREQ`], cut by region or not at all) at every
/// tile: the bubbles young sea floor is born with, so an ocean the epochs open
/// is never a flat slab (Aaron 2026-10-15: "everything is bubbles").
pub fn floor_texture(map: &HexMap, r: &Recipe) -> Vec<f32> {
    let mut heights = vec![0.0f32; map.len()];
    if r.stack.on {
        // UNMASKED. A slot's region mask is the COMPOSED world's cut; the
        // floor a ridge opens is born with the slot's whole wave, so no
        // ocean the epochs open is a flat slab anywhere. Found 2026-10-19
        // (Aaron: *"these areas where the surface appears smooth are
        // perhaps mask areas … the cell motion seems to just avoid these
        // areas altogether"*): floor born inside a region mask's off-cells
        // carried no texture, and the flow across a flat slab is invisible.
        let mut whole = r.clone();
        for slot in whole.stack.slots.iter_mut() {
            if slot.mask == MaskKind::Region {
                slot.mask = MaskKind::Off;
            }
        }
        add_slots(
            map,
            &whole,
            &mut heights,
            &|_, s| s.frequency >= FLOOR_TEXTURE_MIN_FREQ && s.mask != MaskKind::Seam,
            None,
        );
    }
    heights
}

/// **Compose a recipe** over its map: `wave·amp·mask + Σ slots·heard·mask +
/// emitters + bubbles` — the slots and bubble octaves the grid RESOLVES
/// ([`HexMap::resolving_frequency`]); the finer ones are the same recipe's,
/// evaluated exactly by the pixel tier — add being the op: superposition and
/// nothing else; the sea solved over the BASE wave's own span alone.
pub fn compose(map: &HexMap, r: &Recipe) -> Composed {
    compose_with(map, r, &|_, _| true)
}

/// Compose with only the slots `keep` admits (the seam ribbon composes the
/// world without its seam layers to find the land masses).
pub fn compose_with(
    map: &HexMap,
    r: &Recipe,
    keep: &dyn Fn(usize, &SlotRecipe) -> bool,
) -> Composed {
    let n = map.len();
    let mut heights = vec![0.0f32; n];
    let mut ledger = HardnessLedger::new(n);
    let mut sea_span = (0.0f32, 0.0f32);
    if r.wave.on {
        let wave = PlanetWave::with_lobe(map, r.wave.seed, r.wave.lobe);
        let mask = WaveMask::new(map, r.wave.mask.seed);
        sea_span = wave
            .values()
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                (lo.min(v * r.wave.amp), hi.max(v * r.wave.amp))
            });
        for (i, h) in heights.iter_mut().enumerate() {
            let m = if r.wave.mask.on {
                mask.value(i as TileId)
            } else {
                1.0
            };
            let c = wave.value(i as TileId) * r.wave.amp * m;
            *h += c;
            ledger.add(i, c, Rock::Granite.resistance());
        }
    }
    if r.stack.on {
        add_slots(map, r, &mut heights, keep, Some(&mut ledger));
    }
    if r.heat.on && r.heat.emitters_on {
        let seams = SeamField::new(map, r.heat.cells, r.heat.spots, r.heat.seed);
        let crust = CrustField::derive(map, &seams);
        let volcano = Volcanoes::new(map, &seams, crust.vents(), r.heat.emitters);
        for (i, h) in heights.iter_mut().enumerate() {
            let c = volcano.value(i as TileId);
            *h += c;
            ledger.add(i, c, Rock::Basalt.resistance());
        }
    }
    if r.bubbles.on {
        let bubbles = Bubbles::resolved_by(&r.bubbles, map.tile_angle());
        for (i, h) in heights.iter_mut().enumerate() {
            let c = bubbles.eval(map.direction(i as TileId));
            *h += c;
            ledger.add(i, c, Rock::Granite.resistance());
        }
    }
    let sea = sea_level(sea_span.0, sea_span.1, r.sea_percent);
    Composed {
        heights,
        sea,
        hardness: ledger.resolve(),
    }
}

/// A land mass smaller than this share of the surface is an island, not a
/// continent (Australia ≈ 1.5% of Earth's surface).
pub const CONTINENT_MIN_SHARE: f32 = 0.015;
/// How many tiles inland "the edge of a continent" reaches for the ridge
/// reading (~150 mi at the standard tile).
pub const EDGE_RINGS: u32 = 3;

/// **What the composed shape measures as** — the readings a sweep is scored by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// Area share of the surface above the sea.
    pub land_share: f32,
    /// Land masses at or above [`CONTINENT_MIN_SHARE`] of the surface.
    pub continents: usize,
    /// Land masses below it.
    pub islands: usize,
    /// The largest land mass's share of ALL land (1.0 = one supercontinent).
    pub largest_land_share: f32,
    /// Coast tiles (land with a sea neighbour) per land tile — the coastline's
    /// crookedness: a round blob reads low, a ragged coast high.
    pub coast_per_land: f32,
    /// Share of the highest-decile land tiles that stand within [`EDGE_RINGS`]
    /// of a coast — "mountain ridges on the edge of continents".
    pub ridges_on_edges: f32,
    /// Mean land height above the sea, radius units.
    pub mean_land_height: f32,
    /// The ocean floor's relief: the standard deviation of sea-tile depths over
    /// the mean depth — basins and seabeds vs a flat floor.
    pub seabed_relief: f32,
    /// Sea-floor basins: connected sea regions deeper than the median depth, at
    /// or above [`CONTINENT_MIN_SHARE`] of the surface.
    pub basins: usize,
}

/// Measure a composed shape.
pub fn metrics(map: &HexMap, c: &Composed) -> Metrics {
    let n = map.len();
    let area = &map.grid().area;
    let total_area: f32 = area.iter().sum();
    let land: Vec<bool> = c.heights.iter().map(|h| *h > c.sea).collect();
    let land_share = land_share(map, |t| c.heights[t as usize], c.sea);

    // Connected land masses (BFS), area-weighted.
    let components = |keep: &dyn Fn(usize) -> bool| -> Vec<f32> {
        let mut seen = vec![false; n];
        let mut out = Vec::new();
        for s in 0..n {
            if seen[s] || !keep(s) {
                continue;
            }
            let mut stack = vec![s];
            seen[s] = true;
            let mut a = 0.0f32;
            while let Some(t) = stack.pop() {
                a += area[t];
                for nb in map.neighbours(t as TileId) {
                    let j = *nb as usize;
                    if !seen[j] && keep(j) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            out.push(a / total_area);
        }
        out
    };
    let masses = components(&|t| land[t]);
    let continents = masses.iter().filter(|a| **a >= CONTINENT_MIN_SHARE).count();
    let islands = masses.len() - continents;
    let largest = masses.iter().copied().fold(0.0f32, f32::max);
    let largest_land_share = if land_share > 0.0 {
        largest / land_share
    } else {
        0.0
    };

    // Coast: land with a sea neighbour; distance-to-coast over land by BFS.
    let mut coast_dist = vec![u32::MAX; n];
    let mut frontier: Vec<usize> = Vec::new();
    let mut coast_tiles = 0usize;
    let mut land_tiles = 0usize;
    for t in 0..n {
        if !land[t] {
            continue;
        }
        land_tiles += 1;
        if map
            .neighbours(t as TileId)
            .iter()
            .any(|nb| !land[*nb as usize])
        {
            coast_dist[t] = 0;
            frontier.push(t);
            coast_tiles += 1;
        }
    }
    let mut d = 0u32;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for t in &frontier {
            for nb in map.neighbours(*t as TileId) {
                let j = *nb as usize;
                if land[j] && coast_dist[j] == u32::MAX {
                    coast_dist[j] = d + 1;
                    next.push(j);
                }
            }
        }
        frontier = next;
        d += 1;
    }
    let coast_per_land = if land_tiles > 0 {
        coast_tiles as f32 / land_tiles as f32
    } else {
        0.0
    };
    // The highest decile of land, and where it stands.
    let mut land_h: Vec<f32> = (0..n)
        .filter(|t| land[*t])
        .map(|t| c.heights[t] - c.sea)
        .collect();
    land_h.sort_by(f32::total_cmp);
    let (ridges_on_edges, mean_land_height) = if land_h.is_empty() {
        (0.0, 0.0)
    } else {
        let p90 = land_h[land_h.len() * 9 / 10];
        let (mut high, mut near) = (0usize, 0usize);
        for t in 0..n {
            if land[t] && c.heights[t] - c.sea >= p90 {
                high += 1;
                if coast_dist[t] <= EDGE_RINGS {
                    near += 1;
                }
            }
        }
        (
            if high > 0 {
                near as f32 / high as f32
            } else {
                0.0
            },
            land_h.iter().sum::<f32>() / land_h.len() as f32,
        )
    };
    // The sea floor.
    let depths: Vec<f32> = (0..n)
        .filter(|t| !land[*t])
        .map(|t| c.sea - c.heights[t])
        .collect();
    let (seabed_relief, basins) = if depths.len() < 2 {
        (0.0, 0)
    } else {
        let mean = depths.iter().sum::<f32>() / depths.len() as f32;
        let var = depths.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / depths.len() as f32;
        let mut sorted = depths.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        let deep = components(&|t| !land[t] && c.sea - c.heights[t] > median);
        (
            if mean > 0.0 { var.sqrt() / mean } else { 0.0 },
            deep.iter().filter(|a| **a >= CONTINENT_MIN_SHARE).count(),
        )
    };
    Metrics {
        land_share,
        continents,
        islands,
        largest_land_share,
        coast_per_land,
        ridges_on_edges,
        mean_land_height,
        seabed_relief,
        basins,
    }
}

/// **The targets** a sweep scores against (Aaron 2026-10-13): seven continents
/// on ~30% land, ridges on the continents' edges, basins and seabeds, coasts
/// with variety. Lower is better; zero is every target met.
pub fn score(m: &Metrics) -> f32 {
    let land = (m.land_share - 0.30).abs() / 0.10;
    let conts = (m.continents as f32 - 7.0).abs() / 3.0;
    let split = (m.largest_land_share - 0.3).max(0.0) / 0.3; // no supercontinent
    let ridges = (0.6 - m.ridges_on_edges).max(0.0) / 0.6;
    let coast = (0.25 - m.coast_per_land).max(0.0) / 0.25;
    let floor = (0.35 - m.seabed_relief).max(0.0) / 0.35;
    let basins = (3.0 - m.basins as f32).max(0.0) / 3.0;
    2.0 * land + 2.0 * conts + split + ridges + 0.5 * coast + 0.5 * floor + 0.5 * basins
}

/// The nearest tile to a direction, through a 2° lat/lon bucket grid — the
/// sampler both renders share.
pub(crate) struct Nearest<'a> {
    dirs: &'a [Vec3],
    buckets: Vec<Vec<u32>>,
    bw: usize,
    bh: usize,
}

impl<'a> Nearest<'a> {
    const CELL_DEG: f32 = 2.0;

    pub(crate) fn new(map: &'a HexMap) -> Self {
        let dirs = &map.grid().dirs;
        let (bw, bh) = (
            (360.0 / Self::CELL_DEG) as usize,
            (180.0 / Self::CELL_DEG) as usize,
        );
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); bw * bh];
        let mut out = Self {
            dirs,
            buckets: Vec::new(),
            bw,
            bh,
        };
        for (i, d) in dirs.iter().enumerate() {
            let (bx, by) = out.bucket_of(*d);
            buckets[by * bw + bx].push(i as u32);
        }
        out.buckets = buckets;
        out
    }

    fn bucket_of(&self, d: Vec3) -> (usize, usize) {
        let lat = d.y.clamp(-1.0, 1.0).asin().to_degrees();
        let lon = d.z.atan2(d.x).to_degrees();
        let bx = (((lon + 180.0) / Self::CELL_DEG) as usize).min(self.bw - 1);
        let by = (((lat + 90.0) / Self::CELL_DEG) as usize).min(self.bh - 1);
        (bx, by)
    }

    pub(crate) fn tile(&self, d: Vec3) -> usize {
        let (bx, by) = self.bucket_of(d);
        let mut best = (f32::MIN, 0usize);
        for dy in -1i32..=1 {
            let yy = by as i32 + dy;
            if yy < 0 || yy >= self.bh as i32 {
                continue;
            }
            for dx in -1i32..=1 {
                let xx = (bx as i32 + dx).rem_euclid(self.bw as i32);
                for &t in &self.buckets[yy as usize * self.bw + xx as usize] {
                    let dot = self.dirs[t as usize].dot(d);
                    if dot > best.0 {
                        best = (dot, t as usize);
                    }
                }
            }
        }
        best.1
    }
}

/// **The globe as the window shows it** — an orthographic view of the
/// hemisphere facing `look`, `size` pixels square on black: every pixel its
/// tile's tint by the default [`Ramp`] against the sea, times the tile's
/// [`hill_shade`] — the SAME per-tile shade the bench's tile shell is baked
/// with, so what the agent reads off this picture is what Aaron sees on the
/// ball (an earlier cut shaded per pixel under a view light the window never
/// had; the two disagreed and the agent's eyes were not his).
pub fn render_globe(map: &HexMap, c: &Composed, size: u32, look: Vec3) -> Vec<u8> {
    let finder = Nearest::new(map);
    let z = look.normalize_or_zero();
    let mut x = Vec3::Y.cross(z);
    if x.length_squared() < 1e-6 {
        x = Vec3::X;
    }
    let x = x.normalize();
    let y = z.cross(x);
    let (lo, hi) = Ramp::span(&c.heights);
    let shade = hill_shade(map, &c.heights, c.sea);
    let ramp = Ramp::default();
    let n = size as usize;
    let mut rgb = vec![0u8; n * n * 3];
    for py in 0..n {
        for px in 0..n {
            let u = (px as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let v = 1.0 - (py as f32 + 0.5) / size as f32 * 2.0;
            let r2 = u * u + v * v;
            if r2 >= 1.0 {
                continue;
            }
            let d = x * u + y * v + z * (1.0 - r2).sqrt();
            let t = finder.tile(d);
            let tint = ramp.tint(c.heights[t], c.sea, lo, hi);
            let i = py * n + px;
            for k in 0..3 {
                rgb[i * 3 + k] = (tint[k] * shade[t] * 255.0) as u8;
            }
        }
    }
    rgb
}

/// How much the hill-shade steepens the relief's slope: a few percent of
/// radius is invisible at true scale on a ball.
pub const GLOBE_SHADE_GAIN: f32 = 12.0;
/// The hill-shade's floor — what a slope turned fully from the sun keeps.
pub const SHADE_AMBIENT: f32 = 0.22;

/// **The hill-shade** — per tile, how the relief's slope catches a sun that
/// stands 45° up from every tile's own NORTH-WEST (the cartographer's
/// hill-shade, laid over the sphere: the same light at every longitude, so the
/// ball reads the same however it is turned), `SHADE_AMBIENT..=1`. The slope
/// is the ring's gradient estimate (`Σ Δh·d/|d|²`, scaled to the gradient by
/// `2/n` — six symmetric unit directions sum to three times the identity),
/// steepened ×[`GLOBE_SHADE_GAIN`]; the sea lies flat under the light. The ONE
/// shade the evaluator's globe and the bench's tile shell share.
pub fn hill_shade(map: &HexMap, heights: &[f32], sea: f32) -> Vec<f32> {
    let alt = std::f32::consts::FRAC_1_SQRT_2; // sin 45° = cos 45°
    (0..map.len() as TileId)
        .map(|t| {
            let p = map.direction(t);
            let h = heights[t as usize];
            let mut g = Vec3::ZERO;
            let mut count = 0usize;
            if h > sea {
                for nb in map.neighbours(t) {
                    let d = map.direction(*nb) - p;
                    let l2 = d.length_squared().max(1e-12);
                    g += d * ((heights[*nb as usize] - h) / l2);
                    count += 1;
                }
            }
            let g = if count > 0 {
                let g = g * (2.0 / count as f32);
                g - p * p.dot(g)
            } else {
                Vec3::ZERO
            };
            let mut north = Vec3::Y - p * p.y;
            if north.length_squared() < 1e-6 {
                north = Vec3::X - p * p.x;
            }
            let north = north.normalize_or_zero();
            let east = p.cross(north);
            let sun = (p * alt + (north - east) * (alt * alt)).normalize_or_zero();
            let normal = (p - g * GLOBE_SHADE_GAIN).normalize_or_zero();
            SHADE_AMBIENT + (1.0 - SHADE_AMBIENT) * normal.dot(sun).max(0.0)
        })
        .collect()
}

/// **The relief ramp** — how a height READS against the sea: depth-shaded sea
/// (shallow → deep) below it, a hypsometric ramp (lowland → upland → summit)
/// above it, each scaled to the composed shape's own floor and summit. The one
/// palette the flat map and the bench's globe share — the globe tinted against
/// the planetary wave's amplitude before this, and once the stack out-weighed
/// the wave every land tile saturated to the crest colour: a snowball with
/// puddles (Aaron 2026-10-15). The stops are the caller's (the bench's are
/// authored in its scene styles); the defaults are the evaluator's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    pub deep: [f32; 3],
    pub shallow: [f32; 3],
    pub lowland: [f32; 3],
    pub upland: [f32; 3],
    pub summit: [f32; 3],
}

impl Default for Ramp {
    fn default() -> Self {
        Self {
            deep: [0.05, 0.12, 0.40],
            shallow: [0.45, 0.62, 0.85],
            lowland: [0.36, 0.56, 0.28],
            upland: [0.58, 0.47, 0.30],
            summit: [0.96, 0.96, 0.96],
        }
    }
}

impl Ramp {
    /// **The ramp's span** over a relief: a ROBUST floor and summit — the 0.5th
    /// and 99.5th percentiles — so a few stacked spikes (a collision's tallest
    /// columns) do not stretch the scale and paint every ordinary hill as
    /// lowland, while a range along the seams (a few percent of the tiles)
    /// still owns the summit colour instead of bleaching whole continents.
    /// The hills must read (Aaron 2026-10-15: "little hills, medium hills,
    /// big hills").
    pub fn span(heights: &[f32]) -> (f32, f32) {
        if heights.is_empty() {
            return (-1.0, 1.0);
        }
        let mut sorted: Vec<f32> = heights.iter().copied().filter(|h| h.is_finite()).collect();
        sorted.sort_by(f32::total_cmp);
        let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q).round() as usize];
        (at(0.005), at(0.995))
    }

    /// The colour of height `v` against the `sea`, the floor at `lo` and the
    /// summit at `hi` (radius units).
    pub fn tint(&self, v: f32, sea: f32, lo: f32, hi: f32) -> [f32; 3] {
        if v <= sea {
            let depth = ((sea - v) / (sea - lo).max(1e-6)).clamp(0.0, 1.0);
            mix(self.shallow, self.deep, depth)
        } else {
            let t = ((v - sea) / (hi - sea).max(1e-6)).clamp(0.0, 1.0);
            if t < 0.5 {
                mix(self.lowland, self.upland, t * 2.0)
            } else {
                mix(self.upland, self.summit, (t - 0.5) * 2.0)
            }
        }
    }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// **De-project the sphere**: an equirectangular RGB image, `w × h`, the sea in
/// depth-shaded blue, the land in a hypsometric ramp (lowland green → upland
/// brown → summit white), and ELEVATION CONTOUR RINGS every `contour` radius
/// units drawn dark where a pixel's band differs from its neighbour's — the
/// civil-engineering drawing of the shape, in the default [`Ramp`]. Pixels
/// sample the nearest tile through a lat/lon bucket grid.
pub fn render_equirect(map: &HexMap, c: &Composed, w: u32, h: u32, contour: f32) -> Vec<u8> {
    let finder = Nearest::new(map);
    let nearest = |d: Vec3| finder.tile(d);
    // Height per pixel first (contours need neighbours).
    let (lo, hi) = Ramp::span(&c.heights);
    let mut hp = vec![0.0f32; (w * h) as usize];
    for y in 0..h {
        let lat = (0.5 - (y as f32 + 0.5) / h as f32) * std::f32::consts::PI;
        for x in 0..w {
            let lon = ((x as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU;
            let d = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            hp[(y * w + x) as usize] = c.heights[nearest(d)];
        }
    }
    let ramp = Ramp::default();
    let band = |v: f32| ((v - c.sea) / contour.max(1e-6)).floor() as i32;
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let v = hp[i];
            let tint = ramp.tint(v, c.sea, lo, hi);
            let mut col = [tint[0] * 255.0, tint[1] * 255.0, tint[2] * 255.0];
            // Contour ring: this pixel's band differs from the one to its right
            // or below. The shoreline (band 0 ↔ −1) draws darkest.
            let b0 = band(v);
            let right = if x + 1 < w { band(hp[i + 1]) } else { b0 };
            let below = if y + 1 < h {
                band(hp[i + w as usize])
            } else {
                b0
            };
            if b0 != right || b0 != below {
                let k = if (b0 >= 0) != (right >= 0) || (b0 >= 0) != (below >= 0) {
                    0.25
                } else {
                    0.6
                };
                col = [col[0] * k, col[1] * k, col[2] * k];
            }
            rgb[i * 3] = col[0] as u8;
            rgb[i * 3 + 1] = col[1] as u8;
            rgb[i * 3 + 2] = col[2] as u8;
        }
    }
    rgb
}

/// **The hardness channel de-projected**: an equirectangular grey map, soft
/// dark → hard bright, the shoreline traced in blue so the land reads against
/// the floor — the ledger's chart, beside the relief's.
pub fn render_hardness(map: &HexMap, c: &Composed, w: u32, h: u32) -> Vec<u8> {
    let finder = Nearest::new(map);
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    let mut land = vec![false; (w * h) as usize];
    for y in 0..h {
        let lat = (0.5 - (y as f32 + 0.5) / h as f32) * std::f32::consts::PI;
        for x in 0..w {
            let lon = ((x as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU;
            let d = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let t = finder.tile(d);
            let i = (y * w + x) as usize;
            let v = (c.hardness[t].clamp(0.0, 1.0) * 255.0) as u8;
            rgb[i * 3] = v;
            rgb[i * 3 + 1] = v;
            rgb[i * 3 + 2] = v;
            land[i] = c.heights[t] > c.sea;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let right = if x + 1 < w { land[i + 1] } else { land[i] };
            let below = if y + 1 < h {
                land[i + w as usize]
            } else {
                land[i]
            };
            if land[i] != right || land[i] != below {
                rgb[i * 3] = 40;
                rgb[i * 3 + 1] = 90;
                rgb[i * 3 + 2] = 220;
            }
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;

    /// **No ocean the epochs open is a flat slab**: the floor's birth
    /// texture carries the fine layers' whole wave at every tile — a region
    /// mask is the composed world's cut, not the ridge's. The masked
    /// composition of the same layers, by contrast, leaves its off-cells
    /// flat (what Aaron read as "mask areas" the motion avoided).
    #[test]
    fn the_floor_texture_leaves_no_flat_slab() {
        let map = HexMap::new(24);
        let r = Recipe::default_at(24, 42);
        let whole = floor_texture(&map, &r);
        let mut masked = vec![0.0f32; map.len()];
        add_slots(
            &map,
            &r,
            &mut masked,
            &|_, s| s.frequency >= FLOOR_TEXTURE_MIN_FREQ && s.mask != MaskKind::Seam,
            None,
        );
        let top = whole.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(top > 0.0, "the grid resolves a fine layer");
        let flat = |v: &[f32]| v.iter().filter(|x| x.abs() < top * 0.01).count();
        let (flat_whole, flat_masked) = (flat(&whole), flat(&masked));
        // The summed waves cross zero somewhere (about two tiles in a
        // hundred at this grid); a mask's off-cells are whole regions.
        assert!(
            flat_whole * 20 < whole.len(),
            "{flat_whole} of {} tiles born flat",
            whole.len()
        );
        assert!(
            flat_masked > flat_whole * 4,
            "the mask cuts {flat_masked} tiles flat; the ridge's fabric leaves {flat_whole}"
        );
    }

    /// **A recipe composes the one way, and the readings read what is there.**
    /// Same recipe, same shape (serialized and back too); every field off is a
    /// flat sphere with no land at any sea above the base; the default recipe
    /// lands between all-land and all-sea with at least one land mass, and its
    /// flat render is the right size with sea and land pixels both present.
    #[test]
    fn a_recipe_composes_deterministically_and_measures() {
        let map = HexMap::new(MIN_FREQ);
        let r = Recipe::default_at(MIN_FREQ, 42);
        let a = compose(&map, &r);
        let json = serde_json::to_string(&r).expect("a recipe serializes");
        let back: Recipe = serde_json::from_str(&json).expect("and parses");
        assert_eq!(back, r);
        let b = compose(&map, &back);
        assert_eq!(a.heights, b.heights);
        assert_eq!(a.sea, b.sea);
        let m = metrics(&map, &a);
        assert!(m.land_share > 0.05 && m.land_share < 0.95, "{m:?}");
        assert!(m.continents + m.islands >= 1, "{m:?}");
        assert!(m.ridges_on_edges >= 0.0 && m.ridges_on_edges <= 1.0);
        assert!(score(&m).is_finite());

        // The opening stack is the LAYERS — each slot on, at its own height,
        // cut by its own mask kind; a slot alone adds exactly its frequency
        // through its mask at its HEARD height (its dial strayed by the roll
        // within the spread); the stack is a LIST: any frequencies, any count.
        assert_eq!(r.stack.slots.len(), DEFAULT_LAYERS.len());
        for (s, (f, amp, axes, mask)) in r.stack.slots.iter().zip(DEFAULT_LAYERS) {
            assert_eq!(
                (s.frequency, s.amp, s.axes, s.on, s.mask),
                (f, amp, axes, true, mask)
            );
        }
        for (i, s) in r.stack.slots.iter().enumerate() {
            let heard = r.stack.heard(i, s);
            let (lo, hi) = (
                s.amp * (-r.stack.spread).exp2(),
                s.amp * r.stack.spread.exp2(),
            );
            assert!(
                heard >= lo - 1e-9 && heard <= hi + 1e-9,
                "{}: {heard} in {lo}..{hi}",
                s.frequency
            );
        }
        let strays: Vec<f32> = r
            .stack
            .slots
            .iter()
            .enumerate()
            .map(|(i, s)| r.stack.stray(i, s.frequency))
            .collect();
        assert!(strays.windows(2).any(|w| w[0] != w[1]), "the roll scatters");
        let mut pinned = r.stack.clone();
        pinned.spread = 0.0;
        for (i, s) in pinned.slots.iter().enumerate() {
            assert!(
                (pinned.heard(i, s) - s.amp).abs() < 1e-9,
                "spread 0 is the dial"
            );
        }
        let custom = StackRecipe::over(&[2.0, 5.0, 31.0], 9, 9, 0.01, 1.0);
        assert_eq!(custom.slots.len(), 3);
        assert_eq!(custom.slots[2].frequency, 31.0);
        assert!(
            (custom.slots[2].amp - 0.01 / 31.0).abs() < 1e-9,
            "a list rides the law"
        );
        let mut one = r.clone();
        one.wave.on = false;
        one.heat.on = false;
        one.bubbles.on = false;
        for (i, s) in one.stack.slots.iter_mut().enumerate() {
            s.on = i == 2; // the basins alone, through their REGION mask
        }
        let alone = compose(&map, &one);
        let s2 = one.stack.slots[2];
        assert_eq!(s2.mask, MaskKind::Region);
        let wave = HarmonicWave::new(
            &map,
            one.stack.field_seed(2, s2.frequency),
            s2.frequency,
            s2.axes,
        );
        let mask = WaveMask::new(&map, one.stack.mask_seed_of(2, s2.frequency));
        let heard = one.stack.heard(2, &s2);
        for t in 0..map.len() {
            let want = wave.value(t as TileId) * heard * mask.value(t as TileId);
            assert!(
                (alone.heights[t] - want).abs() < 1e-7,
                "tile {t}: a slot is its frequency through its mask at its heard height"
            );
        }
        // THE SEAM CUT: the mountains stand along the land masses' closing
        // margins — the wave stays on so there IS land and sea — as a lift on
        // whatever stands there; a minority of the surface; nothing where the
        // ribbon is zero; another heat roll moves them (the push is the
        // heat's); a wider SEAM stands more; the ribbon lies on the land and
        // its shelf.
        let mut one = r.clone();
        one.heat.on = false;
        for (i, s) in one.stack.slots.iter_mut().enumerate() {
            s.on = i == 4;
        }
        let s4 = one.stack.slots[4];
        assert_eq!(s4.mask, MaskKind::Seam);
        let mut none = one.clone();
        none.stack.slots[4].on = false;
        let lift = |a: &Recipe, b: &Recipe| -> Vec<f32> {
            let (ca, cb) = (compose(&map, a), compose(&map, b));
            ca.heights
                .iter()
                .zip(&cb.heights)
                .map(|(x, y)| x - y)
                .collect()
        };
        let ridges = lift(&one, &none);
        let ribbon = seam_ribbon(&map, &one);
        let standing = ridges.iter().filter(|h| h.abs() > 1e-6).count();
        assert!(
            standing > 0 && standing < map.len() / 2,
            "ridges along seams: {standing} of {}",
            map.len()
        );
        assert!(
            ridges.iter().all(|h| *h >= -1e-6),
            "a seam lifts: nothing below what stood"
        );
        for (t, w) in ribbon.iter().enumerate() {
            if *w == 0.0 {
                assert!(ridges[t].abs() < 1e-6, "tile {t}: off the margin, no ridge");
            }
        }
        let mut hotter = one.clone();
        hotter.heat.seed ^= 0xBEEF;
        let mut hotter_none = none.clone();
        hotter_none.heat.seed ^= 0xBEEF;
        assert_ne!(
            lift(&hotter, &hotter_none),
            ridges,
            "another heat roll moves the seams"
        );
        let mut wide = one.clone();
        wide.stack.seam = 6.0;
        let mut wide_none = none.clone();
        wide_none.stack.seam = 6.0;
        let standing_wide = lift(&wide, &wide_none)
            .iter()
            .filter(|h| h.abs() > 1e-6)
            .count();
        assert!(
            standing_wide > standing,
            "a wider seam stands more: {standing_wide} > {standing}"
        );
        let base = compose(&map, &none);
        let rafts = Rafts::from_relief(&map, &base.heights, base.sea);
        assert!(
            rafts.count >= 2,
            "the wave made land masses: {}",
            rafts.count
        );
        for t in 0..map.len() as TileId {
            if ribbon[t as usize] > 0.0 && rafts.home[t as usize] == u32::MAX {
                assert!(
                    map.neighbours(t)
                        .iter()
                        .any(|nb| rafts.home[*nb as usize] != u32::MAX),
                    "tile {t}: a seam off the land is on its shelf"
                );
            }
        }
        // Two slots at ONE frequency are two different waves.
        let mut twins = one.clone();
        twins.stack.slots = vec![
            SlotRecipe {
                frequency: 2.0,
                amp: 0.02,
                axes: 1,
                on: true,
                mask: MaskKind::Off,
            },
            SlotRecipe {
                frequency: 2.0,
                amp: 0.02,
                axes: 1,
                on: true,
                mask: MaskKind::Off,
            },
        ];
        twins.stack.spread = 0.0;
        let both = compose(&map, &twins);
        twins.stack.slots.truncate(1);
        let first = compose(&map, &twins);
        assert!(
            (0..map.len()).any(|t| (both.heights[t] - 2.0 * first.heights[t]).abs() > 1e-4),
            "the second slot is not the first one twice"
        );
        // The stack's own switch silences every slot (the wave off too, so the
        // sphere is bare).
        let mut hushed = one.clone();
        hushed.wave.on = false;
        hushed.stack.on = false;
        hushed.bubbles.on = false;
        assert!(compose(&map, &hushed).heights.iter().all(|h| *h == 0.0));

        let mut off = r.clone();
        off.wave.on = false;
        off.heat.on = false;
        off.stack.on = false;
        off.bubbles.on = false;
        let flat = compose(&map, &off);
        assert!(flat.heights.iter().all(|h| *h == 0.0));
        assert_eq!(
            metrics(&map, &flat).land_share,
            0.0,
            "a flat sphere at the datum is sea"
        );

        // The ramp: the floor is deep, the waterline shallow on the sea side and
        // lowland on the land side, the summit the summit; land never reads blue.
        let ramp = Ramp::default();
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(close(ramp.tint(-0.1, 0.0, -0.1, 0.1), ramp.deep));
        assert!(close(ramp.tint(0.0, 0.0, -0.1, 0.1), ramp.shallow));
        assert!(close(ramp.tint(1e-6, 0.0, -0.1, 0.1), ramp.lowland));
        assert!(close(ramp.tint(0.1, 0.0, -0.1, 0.1), ramp.summit));
        assert!(close(ramp.tint(0.05, 0.0, -0.1, 0.1), ramp.upland));
        assert!(
            close(ramp.tint(9.0, 0.0, -0.1, 0.1), ramp.summit),
            "clamped above"
        );
        // The globe render is a disk of the ramp, shaded, on a black field.
        let globe = render_globe(&map, &a, 64, Vec3::new(0.3, 0.2, 0.93));
        assert_eq!(globe.len(), 64 * 64 * 3);
        assert_eq!(&globe[..3], &[0, 0, 0], "outside the disk is black");
        let centre = (32 * 64 + 32) * 3;
        assert!(
            globe[centre..centre + 3].iter().any(|v| *v > 0),
            "the disk is lit"
        );
        let (w, h) = (128u32, 64u32);
        let img = render_equirect(&map, &a, w, h, 0.01);
        assert_eq!(img.len(), (w * h * 3) as usize);
        let blue = img
            .chunks(3)
            .filter(|p| u16::from(p[2]) > u16::from(p[0]) + 40)
            .count();
        let green = img
            .chunks(3)
            .filter(|p| u16::from(p[1]) > u16::from(p[2]) + 20)
            .count();
        assert!(blue > 0 && green > 0, "sea {blue} land {green}");
    }
}
