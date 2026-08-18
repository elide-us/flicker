//! [`WorldConfig`] — the per-world **input recipe**: every lever's value plus the
//! grid `freq` and base `seed`. It is the durable, serialisable truth (captured in
//! a `.epoch` file); the snapshots are regenerated from it. Levers are held in a
//! string-keyed map so the same ids flow unchanged from `epoch_defaults.json`
//! through the config to the Lua HUD — the data-driven contract the old viewer
//! used, now sourced from content instead of a hardcoded table.
//!
//! [`build_epoch1`] / [`build_transforms`] turn the flat lever values into the
//! typed [`flicker_worldgen`] epoch kernels (the salvaged `build_epochs`); the
//! seed chain + reseed jitter ([`seed_chain`], [`mutate_epoch`]) are lifted from
//! the old `world.rs` so a per-epoch reseed leaves upstream epochs byte-identical.

use std::collections::BTreeMap;

use flicker_worldgen::{Epoch1Params, Epoch2, Epoch3, Epoch4, Epoch5, Epoch6};
use serde::{Deserialize, Serialize};

use crate::levers::GeneratorParams;

/// The nine formation epochs (three groups × ~1.5 BY). Epochs 7-9 are stubbed
/// heightmap-strata layers this iteration; the seed chain and cache still carry
/// all nine so the timeline is exercised end to end.
pub const WORLD_EPOCHS: usize = 9;

/// Default subdivision frequency (≈ 23042 cells at freq 48). The engine builds the
/// sphere lazily, so a caller can drop this for a cheaper preview.
pub const DEFAULT_FREQ: u32 = 48;
/// Default world seed.
pub const DEFAULT_SEED: u64 = 0x0EC0_DE01;
/// Frequency clamp for the grid control. The ceiling is **full Earth** (`10·96²+2 ≈
/// 92k cells`); the viewer defaults to the ½-Earth size (snappy) but the size slider
/// can now be dragged up to full Earth — a heavier, live-batch-only scenario the user
/// opted into for more variation and room at scale.
pub const MIN_FREQ: u32 = 6;
pub const MAX_FREQ: u32 = 96;

/// Prefix marking an Epoch-1 element-abundance lever (`ab_O`, `ab_Fe`, …).
pub const ABUNDANCE_PREFIX: &str = "ab_";

/// A world's full input recipe. Serialisable so it round-trips through a `.epoch`
/// capture; cheap to clone for the "tweak a knob, regenerate forward" loop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldConfig {
    /// Lever id → value. Holds both the epoch knobs (`e3_mountain_uplift`) and the
    /// Epoch-1 element mix (`ab_<symbol>`).
    pub values: BTreeMap<String, f64>,
    /// Subdivision frequency of the icosphere grid.
    pub freq: u32,
    /// Base world seed (the per-epoch seed chain derives from it).
    pub seed: u64,
}

impl WorldConfig {
    /// Seed a config from the loaded lever tables: every lever at its `default`,
    /// every element at its abundance `weight`, at [`DEFAULT_FREQ`]/[`DEFAULT_SEED`].
    pub fn from_params(params: &GeneratorParams) -> Self {
        let mut values = BTreeMap::new();
        for l in params.levers() {
            values.insert(l.id.clone(), l.default);
        }
        for a in params.abundance() {
            values.insert(format!("{ABUNDANCE_PREFIX}{}", a.symbol), a.weight);
        }
        Self {
            values,
            freq: DEFAULT_FREQ,
            seed: DEFAULT_SEED,
        }
    }

    /// Current value of a lever id (`0.0` if unknown — the epoch builders only ask
    /// for ids that exist in the shipped table).
    pub fn get(&self, id: &str) -> f64 {
        self.values.get(id).copied().unwrap_or(0.0)
    }

    /// Override a lever id.
    pub fn set(&mut self, id: &str, value: f64) {
        self.values.insert(id.to_string(), value);
    }

    /// Set a lever, clamped to its `[min, max]` band from the loaded table (a
    /// no-op clamp for an unknown id). The edit path the HUD sliders use.
    pub fn set_clamped(&mut self, id: &str, value: f64, params: &GeneratorParams) {
        let v = match params.lever(id) {
            Some(l) => value.clamp(l.min, l.max),
            None => value,
        };
        self.set(id, v);
    }

    /// The clamped, usable frequency.
    pub fn clamped_freq(&self) -> u32 {
        self.freq.clamp(MIN_FREQ, MAX_FREQ)
    }
}

/// Build Epoch 1's parameters (the element scatter "birth certificate") from the
/// config's abundance levers + the element list in `params`.
pub fn build_epoch1(config: &WorldConfig, params: &GeneratorParams) -> Epoch1Params {
    let abundance = params
        .abundance()
        .iter()
        .map(|a| {
            (
                a.symbol.clone(),
                config.get(&format!("{ABUNDANCE_PREFIX}{}", a.symbol)),
            )
        })
        .collect();
    Epoch1Params {
        abundance,
        equator_bias: config.get("e1_equator_bias"),
        pole_bias: config.get("e1_pole_bias"),
        contrast: config.get("e1_contrast"),
        frequency: config.get("e1_frequency") as f32,
        ..Epoch1Params::default()
    }
}

/// Build the five tunable epoch transforms (2-6) from the config's knobs — the
/// salvaged `build_epochs`, verbatim in mapping so the physics is unchanged.
pub fn build_transforms(config: &WorldConfig) -> (Epoch2, Epoch3, Epoch4, Epoch5, Epoch6) {
    let e2 = Epoch2 {
        crust_density_max: config.get("e2_crust_density_max"),
        polar_thickening: config.get("e2_polar_thickening") as f32,
        duration: config.get("e2_duration").round().max(1.0) as u32,
        // The cooling engine sets this from the thermal clock in the Epoch-2 arm; the
        // lever-built transform starts with the `duration` fallback.
        settle_override: None,
    };
    let e3 = Epoch3 {
        plates: config.get("e3_plates").round().max(1.0) as usize,
        continental_fraction: config.get("e3_continental_fraction") as f32,
        mountain_uplift: config.get("e3_mountain_uplift") as f32,
        rift_drop: config.get("e3_rift_drop") as f32,
        hotspots: config.get("e3_hotspots").round().max(0.0) as usize,
        hotspot_uplift: config.get("e3_hotspot_uplift") as f32,
        hotspot_trail: config.get("e3_hotspot_trail") as f32,
        duration: config.get("e3_duration").round().max(1.0) as u32,
        ..Epoch3::default()
    };
    let e4 = Epoch4 {
        hydration: config.get("e4_hydration") as f32,
        equator_temp: config.get("e4_equator_temp") as f32,
        pole_temp: config.get("e4_pole_temp") as f32,
        axial_tilt: config.get("e4_axial_tilt") as f32,
        vapor_scale: config.get("e4_vapor_scale") as f32,
        duration: config.get("e4_duration").round().max(1.0) as u32,
        prebiotic_rate: config.get("e4_prebiotic_rate") as f32,
        ..Epoch4::default()
    };
    let e5 = Epoch5 {
        max_veins: config.get("e5_max_veins").round().max(0.0) as usize,
        vein_threshold: config.get("e5_vein_threshold") as f32,
        microbial_threshold: config.get("e5_microbial") as f32,
        vent_life_boost: config.get("e5_vent_life") as f32,
        duration: config.get("e5_duration").round().max(1.0) as u32,
        ..Epoch5::default()
    };
    let e6 = Epoch6 {
        rain: config.get("e6_rain") as f32,
        erosion_rate: config.get("e6_erosion_rate") as f32,
        duration: config.get("e6_duration").round().max(1.0) as u32,
        floral_threshold: config.get("e6_flora") as f32,
        organics_rate: config.get("e6_organics") as f32,
        decomposer_onset: config.get("e6_decomposer") as f32,
        carbonate_onset: config.get("e6_carbonate") as f32,
        ..Epoch6::default()
    };
    (e2, e3, e4, e5, e6)
}

/// Deterministic next seed (splitmix64) for the reseed control — salvaged verbatim.
pub fn next_seed(s: u64) -> u64 {
    let mut z = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Per-epoch seed chain from a base: `seeds[0] = base`, each next is [`next_seed`]
/// of the prior. Epoch `k` (1-indexed) runs under `seeds[k-1]`; reseeding a layer
/// advances its seed and re-derives the later ones, leaving earlier layers
/// untouched. Length [`WORLD_EPOCHS`].
pub fn seed_chain(base: u64) -> Vec<u64> {
    let mut seeds = Vec::with_capacity(WORLD_EPOCHS);
    let mut s = base;
    for _ in 0..WORLD_EPOCHS {
        seeds.push(s);
        s = next_seed(s);
    }
    seeds
}

/// Fraction of a knob's band a reseed may wander from its default (triangular,
/// biased to 0) — salvaged from the old viewer.
const LEVER_MUTATION: f64 = 0.5;
/// Fractional multiplicative jitter on each Epoch-1 abundance on reseed.
const ABUNDANCE_MUTATION: f64 = 0.5;

/// Mutate **only one epoch's** knobs (within their bands), for a per-epoch reseed:
/// upstream epochs keep their values, so they look identical, while this epoch gets
/// a fresh variation. Deterministic in `seed`. Epoch 1 also re-rolls the element
/// mix. `epoch` is 1-indexed here (matches [`LeverDef::epoch`]).
pub fn mutate_epoch(config: &mut WorldConfig, epoch: u8, seed: u64, params: &GeneratorParams) {
    let mut n = 0u64;
    for l in params.levers() {
        if l.epoch != epoch {
            continue;
        }
        // The plate count AND the drift duration are structural/timeline properties,
        // not per-reseed creative dials — jittering them would reshuffle the
        // (otherwise material-derived, reseed-stable) Epoch-3 plate layout (`e3_plates`)
        // or the amount the material has drifted (`e3_duration`). Keep both fixed so a
        // reseed varies only the deformation magnitudes, never the layout/motion.
        if l.id == "e3_plates" || l.id == "e3_duration" {
            continue;
        }
        n += 1;
        let v = (l.default + centered_rand(seed, n) * (l.max - l.min) * LEVER_MUTATION)
            .clamp(l.min, l.max);
        config.set(&l.id, v);
    }
    if epoch == 1 {
        let ab_seed = next_seed(seed);
        for (i, a) in params.abundance().iter().enumerate() {
            let factor =
                (1.0 + centered_rand(ab_seed, i as u64 + 1) * ABUNDANCE_MUTATION).max(0.05);
            config.set(
                &format!("{ABUNDANCE_PREFIX}{}", a.symbol),
                a.weight * factor,
            );
        }
    }
}

/// Deterministic triangular random in `[-1, 1]` (peaked at 0) from `(seed, n)` —
/// salvaged; keeps reseed jitter modest and plausible.
fn centered_rand(seed: u64, n: u64) -> f64 {
    let hash = |salt: u64| -> f64 {
        let mut z = seed ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    };
    hash(0xA1) + hash(0xB2) - 1.0
}
