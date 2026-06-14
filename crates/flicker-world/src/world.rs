//! World data + generation parameters.
//!
//! Pulls the icosahedral grid from `flicker-worldgrid` and runs the existing
//! `flicker-worldgen` epoch chain over it (via the topology-agnostic `EpochCtx`),
//! keeping **every** epoch layer so the viewer can show any phase. The chain is
//! driven by [`WorldParams`] — a tunable override of each epoch's knobs — so the
//! "play god, turn a phase's knobs, regenerate" loop is just another [`generate`]
//! call. Generation is pure given `(params, freq, seed)`.

use std::collections::HashMap;

use flicker::render::Vec3;
use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{
    watersheds, Epoch1, Epoch1Params, Epoch2, Epoch3, Epoch4, Epoch5, Epoch6, EpochCtx,
    EpochTransform, HexState, Plate, Watershed, EPOCHS,
};
use flicker_worldgrid::{icosphere_with_outlines, Sphere};

/// Default subdivision frequency (≈ 23042 cells at freq 48 — the test default;
/// still comfortable on the A18 box, the epoch chain is cheap at this cell count).
pub const DEFAULT_FREQ: u32 = 48;
/// Default world seed.
pub const DEFAULT_SEED: u64 = 0x0EC0_DE01;
/// Frequency clamp for the grid control.
pub const MIN_FREQ: u32 = 6;
pub const MAX_FREQ: u32 = 48;

/// Display names for the six epoch layers (index 0 = Epoch 1).
pub const EPOCH_LABELS: [&str; 6] = [
    "Composition",
    "Differentiation",
    "Tectonics",
    "Hydrosphere",
    "Mineralization",
    "Erosion",
];

/// The tunable epoch knobs: `(epoch index 0..6, param id, default, min, max)`. The
/// id is the contract shared by [`generate`] (reads it), the HUD model (publishes
/// the current value), and `ui_elements.json` (label; its slider range must match
/// the `(min, max)` here). Defaults mirror each epoch struct's `Default`; the
/// `(min, max)` band is the reasonable range [`mutate_params`] re-rolls within on
/// reseed.
pub const PARAM_DEFS: &[(usize, &str, f64, f64, f64)] = &[
    // Epoch 1 — composition.
    (0, "e1_equator_bias", 1.5, 0.0, 3.0),
    (0, "e1_pole_bias", 1.5, 0.0, 3.0),
    (0, "e1_contrast", 4.0, 1.0, 6.0),
    (0, "e1_frequency", 2.2, 0.5, 6.0),
    // Epoch 2 — differentiation.
    (1, "e2_crust_density_max", 3.5, 2.5, 5.0),
    (1, "e2_polar_thickening", 0.3, 0.0, 1.0),
    // Epoch 3 — tectonics (incl. hotspot volcanism).
    (2, "e3_plates", 8.0, 2.0, 24.0),
    (2, "e3_continental_fraction", 0.4, 0.0, 1.0),
    (2, "e3_mountain_uplift", 0.6, 0.0, 1.5),
    (2, "e3_rift_drop", 0.25, 0.0, 1.0),
    (2, "e3_hotspots", 6.0, 0.0, 20.0),
    (2, "e3_hotspot_uplift", 0.6, 0.0, 1.5),
    (2, "e3_hotspot_trail", 0.4, 0.05, 1.5),
    (2, "e3_cycles", 1.0, 1.0, 8.0),
    // Epoch 4 — hydrosphere (incl. axial tilt).
    (3, "e4_hydration", 1.0, 0.0, 3.0),
    (3, "e4_equator_temp", 28.0, 0.0, 50.0),
    (3, "e4_pole_temp", -25.0, -60.0, 10.0),
    (3, "e4_axial_tilt", 23.5, 0.0, 90.0),
    (3, "e4_vapor_scale", 1.0, 0.0, 3.0),
    (3, "e4_cycles", 1.0, 1.0, 8.0),
    (3, "e4_prebiotic_rate", 0.3, 0.0, 1.0),
    // Epoch 5 — mineralization + microbial life.
    (4, "e5_max_veins", 64.0, 0.0, 256.0),
    (4, "e5_vein_threshold", 0.35, 0.0, 1.0),
    (4, "e5_microbial", 0.12, 0.02, 0.5),
    (4, "e5_vent_life", 1.0, 0.0, 3.0),
    // Epoch 6 — erosion + flora.
    (5, "e6_rain", 1.0, 0.0, 3.0),
    (5, "e6_erosion_rate", 0.018, 0.0, 0.1),
    (5, "e6_iterations", 8.0, 1.0, 30.0),
    (5, "e6_flora", 0.45, 0.0, 1.0),
    (5, "e6_organics", 0.05, 0.0, 0.2),
    (5, "e6_decomposer", 0.6, 0.0, 1.0),
    (5, "e6_carbonate", 0.3, 0.0, 1.0),
];

/// Epoch 1's starting element mix: `(symbol, default relative abundance)`. These
/// seed `WorldParams` under `ab_<symbol>` and drive `Epoch1Params.abundance`; the
/// HUD shows one slider per element on the Composition epoch. Defaults mirror
/// `Epoch1Params::default`'s crustal mix; weights are relative (renormalised per
/// cell), so the slider scale is just "how much this element competes".
pub const ABUNDANCE_DEFS: &[(&str, f64)] = &[
    ("O", 46.0),
    ("Si", 28.0),
    ("Al", 8.0),
    ("Fe", 5.0),
    ("Ca", 3.6),
    ("Na", 2.8),
    ("K", 2.6),
    ("Ti", 0.6),
    ("C", 0.2),
    ("H", 0.14),
    ("P", 0.1),
    ("S", 0.05),
    ("Cl", 0.05),
    ("N", 0.02),
];

const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");

/// Load the material vocabulary tables from the repo `data/materials` dir.
pub fn load_tables() -> anyhow::Result<Tables> {
    Ok(Tables::from_source(&JsonTableSource::new(MATERIALS_DIR))?)
}

/// Tunable per-epoch parameters, keyed by the ids in [`PARAM_DEFS`]. Seeded with
/// the defaults; the HUD mutates entries and regenerates.
#[derive(Clone)]
pub struct WorldParams {
    values: HashMap<String, f64>,
}

impl Default for WorldParams {
    fn default() -> Self {
        let mut values: HashMap<String, f64> = PARAM_DEFS
            .iter()
            .map(|(_, id, def, _, _)| ((*id).to_string(), *def))
            .collect();
        for (sym, def) in ABUNDANCE_DEFS {
            values.insert(format!("ab_{sym}"), *def);
        }
        Self { values }
    }
}

impl WorldParams {
    /// Current value of a param id (defaults are pre-seeded, so this never misses
    /// a known id).
    pub fn get(&self, id: &str) -> f64 {
        self.values.get(id).copied().unwrap_or(0.0)
    }

    /// Override a param id.
    pub fn set(&mut self, id: &str, value: f64) {
        self.values.insert(id.to_string(), value);
    }
}

/// Deposit element ids (coal/oil carbon, chalk calcium) read by the Deposits view.
const CARBON: u8 = 6;
const CALCIUM: u8 = 20;

/// Field ranges over one epoch layer, for normalising colour ramps.
pub struct Ranges {
    pub elev_min: f32,
    pub elev_max: f32,
    pub temp_min: f32,
    pub temp_max: f32,
    pub max_depth: f32,
    pub crust_min: f32,
    pub crust_max: f32,
    /// Max per-cell deposit mass (coal/oil carbon + chalk calcium) in the layer.
    pub dep_max: f32,
    /// Max drainage flow and deposited sediment in the layer, for the river /
    /// sediment heatmaps.
    pub flow_max: f32,
    pub sediment_max: f32,
}

/// A generated planet: the grid, per-cell geometry, and **all** epoch layers.
pub struct WorldData {
    pub sphere: Sphere,
    pub outlines: Vec<Vec<Vec3>>,
    pub layers: Vec<Vec<HexState>>,
    /// Epoch 3's cross-hex tectonic plates (id, kind, drift, membership) — the
    /// `plates` structure the per-hex `plate` field maps into.
    pub plates: Vec<Plate>,
    /// Epoch 6's drainage basins (the cross-hex `watersheds` structure) — what the
    /// per-hex `watershed` sink id groups into.
    pub watersheds: Vec<Watershed>,
    /// Per-epoch seeds (`seeds[k]` drove epoch k). Reseeding a layer advances its
    /// seed and re-derives the later ones, leaving earlier layers identical.
    pub seeds: Vec<u64>,
    pub freq: u32,
}

impl WorldData {
    /// Number of epoch layers (6).
    pub fn epochs(&self) -> usize {
        self.layers.len()
    }
}

/// Generate a planet at `(params, freq, seed)` — derives the per-epoch seed chain
/// from `seed`, then runs it. Convenience over [`generate_with_seeds`].
pub fn generate(tables: &Tables, params: &WorldParams, freq: u32, seed: u64) -> WorldData {
    generate_with_seeds(tables, params, freq, &seed_chain(seed))
}

/// Generate a planet from **per-epoch seeds** (`seeds[k]` drives epoch k's
/// stochastic choices). Builds the icosahedral grid and runs the six transforms,
/// each under its own seed, so reseeding one layer (advancing `seeds[k]` and
/// re-deriving the later ones) leaves the earlier layers byte-identical. Keeps
/// every layer. (Epochs 2 & 4 are seedless — varied only by their params.)
pub fn generate_with_seeds(
    tables: &Tables,
    params: &WorldParams,
    freq: u32,
    seeds: &[u64],
) -> WorldData {
    let freq = freq.clamp(MIN_FREQ, MAX_FREQ);
    let (sphere, outlines) = icosphere_with_outlines(freq);

    let abundance = ABUNDANCE_DEFS
        .iter()
        .map(|(sym, _)| ((*sym).to_string(), params.get(&format!("ab_{sym}"))))
        .collect();
    let e1p = Epoch1Params {
        abundance,
        equator_bias: params.get("e1_equator_bias"),
        pole_bias: params.get("e1_pole_bias"),
        contrast: params.get("e1_contrast"),
        frequency: params.get("e1_frequency") as f32,
        ..Epoch1Params::default()
    };
    let e1 = Epoch1::new(tables, e1p, seeds[0]);

    let seed_layer: Vec<HexState> = sphere
        .dirs
        .iter()
        .map(|&d| HexState::new(e1.seed_hex(d)))
        .collect();

    let e2 = Epoch2 {
        crust_density_max: params.get("e2_crust_density_max"),
        polar_thickening: params.get("e2_polar_thickening") as f32,
    };
    let e3 = Epoch3 {
        plates: params.get("e3_plates").round().max(1.0) as usize,
        continental_fraction: params.get("e3_continental_fraction") as f32,
        mountain_uplift: params.get("e3_mountain_uplift") as f32,
        rift_drop: params.get("e3_rift_drop") as f32,
        hotspots: params.get("e3_hotspots").round().max(0.0) as usize,
        hotspot_uplift: params.get("e3_hotspot_uplift") as f32,
        hotspot_trail: params.get("e3_hotspot_trail") as f32,
        cycles: params.get("e3_cycles").round().max(1.0) as u32,
        ..Epoch3::default()
    };
    let e4 = Epoch4 {
        hydration: params.get("e4_hydration") as f32,
        equator_temp: params.get("e4_equator_temp") as f32,
        pole_temp: params.get("e4_pole_temp") as f32,
        axial_tilt: params.get("e4_axial_tilt") as f32,
        vapor_scale: params.get("e4_vapor_scale") as f32,
        cycles: params.get("e4_cycles").round().max(1.0) as u32,
        prebiotic_rate: params.get("e4_prebiotic_rate") as f32,
        ..Epoch4::default()
    };
    let e5 = Epoch5 {
        max_veins: params.get("e5_max_veins").round().max(0.0) as usize,
        vein_threshold: params.get("e5_vein_threshold") as f32,
        microbial_threshold: params.get("e5_microbial") as f32,
        vent_life_boost: params.get("e5_vent_life") as f32,
        ..Epoch5::default()
    };
    let e6 = Epoch6 {
        rain: params.get("e6_rain") as f32,
        erosion_rate: params.get("e6_erosion_rate") as f32,
        iterations: params.get("e6_iterations").round().max(0.0) as u32,
        floral_threshold: params.get("e6_flora") as f32,
        organics_rate: params.get("e6_organics") as f32,
        decomposer_onset: params.get("e6_decomposer") as f32,
        carbonate_onset: params.get("e6_carbonate") as f32,
        ..Epoch6::default()
    };

    // Each epoch runs under its own per-layer seed, so reseeding one layer leaves
    // the upstream layers byte-identical (only this layer and those built on it
    // re-vary). `seeds[k]` drives epoch k.
    let dirs = &sphere.dirs;
    let nbr = &sphere.neighbors;
    let ctx = |s: u64| EpochCtx { tables, dirs, neighbors: nbr, seed: s };
    let l2 = e2.apply(&ctx(seeds[1]), &seed_layer);
    let l3 = e3.apply(&ctx(seeds[2]), &l2);
    let l4 = e4.apply(&ctx(seeds[3]), &l3);
    let l5 = e5.apply(&ctx(seeds[4]), &l4);
    let l6 = e6.apply(&ctx(seeds[5]), &l5);

    // Record Epoch 3's cross-hex plate structure (same deterministic partition,
    // same seed + Epoch-2 crust the chain ran) and the ground layer's drainage basins.
    let plates = e3.partition(&ctx(seeds[2]), &l2).plates;
    let basins = watersheds(&l6);
    let layers = vec![seed_layer, l2, l3, l4, l5, l6];

    WorldData {
        sphere,
        outlines,
        layers,
        plates,
        watersheds: basins,
        seeds: seeds.to_vec(),
        freq,
    }
}

/// Field ranges over one epoch layer.
pub fn ranges_for(states: &[HexState]) -> Ranges {
    let mut r = Ranges {
        elev_min: f32::MAX,
        elev_max: f32::MIN,
        temp_min: f32::MAX,
        temp_max: f32::MIN,
        max_depth: 0.0,
        crust_min: f32::MAX,
        crust_max: f32::MIN,
        dep_max: 0.0,
        flow_max: 0.0,
        sediment_max: 0.0,
    };
    for s in states {
        r.elev_min = r.elev_min.min(s.elevation);
        r.elev_max = r.elev_max.max(s.elevation);
        r.temp_min = r.temp_min.min(s.temperature);
        r.temp_max = r.temp_max.max(s.temperature);
        r.max_depth = r.max_depth.max(s.water_depth);
        let cf = s.crust_fraction as f32;
        r.crust_min = r.crust_min.min(cf);
        r.crust_max = r.crust_max.max(cf);
        r.dep_max = r.dep_max.max((s.deposits.amount(CARBON) + s.deposits.amount(CALCIUM)) as f32);
        r.flow_max = r.flow_max.max(s.flow);
        r.sediment_max = r.sediment_max.max(s.sediment);
    }
    r
}

/// Deterministic next seed (splitmix64) for the reseed control.
pub fn next_seed(s: u64) -> u64 {
    let mut z = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Per-epoch seed chain from a base: `seeds[0] = base`, each next is `next_seed` of
/// the prior. Each epoch runs under `seeds[k]`; reseeding a layer advances its seed
/// and re-derives the later ones, leaving the earlier layers untouched.
pub fn seed_chain(base: u64) -> Vec<u64> {
    let mut seeds = Vec::with_capacity(EPOCHS);
    let mut s = base;
    for _ in 0..EPOCHS {
        seeds.push(s);
        s = next_seed(s);
    }
    seeds
}

/// Fraction of a knob's full band a reseed may wander from its default. The jitter
/// is triangular (biased to 0), so a typical reseed moves each knob much less —
/// keeping the planet plausible while still exploring the band.
const PARAM_MUTATION: f64 = 0.5;
/// Fractional multiplicative jitter applied to each Epoch-1 element abundance on
/// reseed (the mix shifts but the dominant rock-formers stay dominant).
const ABUNDANCE_MUTATION: f64 = 0.5;

/// Mutate **only one epoch's** knobs (within their bands), for a per-layer reseed:
/// upstream epochs keep their params (and seeds), so they look identical, while
/// this layer gets a fresh variation. Deterministic in `seed`. Epoch 0 also owns
/// the element mix.
pub fn mutate_epoch_params(params: &mut WorldParams, epoch: usize, seed: u64) {
    let mut n = 0u64;
    for (ep, id, def, min, max) in PARAM_DEFS {
        if *ep != epoch {
            continue;
        }
        n += 1;
        let v = (def + centered_rand(seed, n) * (max - min) * PARAM_MUTATION).clamp(*min, *max);
        params.set(id, v);
    }
    if epoch == 0 {
        let ab_seed = next_seed(seed);
        for (i, (sym, def)) in ABUNDANCE_DEFS.iter().enumerate() {
            let factor = (1.0 + centered_rand(ab_seed, i as u64 + 1) * ABUNDANCE_MUTATION).max(0.05);
            params.set(&format!("ab_{sym}"), def * factor);
        }
    }
}

/// Deterministic triangular random in `[-1, 1]` (peaked at 0) from `(seed, n)` —
/// the average of two splitmix64 uniforms, so reseed jitter favours modest,
/// plausible departures from the default over extremes.
fn centered_rand(seed: u64, n: u64) -> f64 {
    let hash = |salt: u64| -> f64 {
        let mut z = seed ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64 // [0, 1)
    };
    hash(0xA1) + hash(0xB2) - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fraction of the planet submerged at Epoch 4 (the hydrosphere layer).
    fn submerged_fraction(tables: &Tables, set: impl Fn(&mut WorldParams)) -> f32 {
        let mut p = WorldParams::default();
        set(&mut p);
        let w = generate(tables, &p, 24, 7);
        let l4 = &w.layers[3];
        l4.iter().filter(|s| s.water_depth > 0.0).count() as f32 / l4.len() as f32
    }

    #[test]
    fn ocean_volume_follows_the_hydrogen_and_oxygen_budget() {
        let tables = load_tables().expect("tables load");
        // The default Earth-like mix floods a sensible fraction (calibration guard).
        let earthlike = submerged_fraction(&tables, |_| {});
        assert!(
            (0.4..=0.85).contains(&earthlike),
            "default ocean coverage {earthlike} drifted out of the sane band — re-check WATER_FILL_GAIN"
        );
        // Water is H₂O: starving the hydrogen *or* the oxygen endowment drains the
        // ocean (dial back to Epoch 1, lower H or O → less sea), and adding hydrogen
        // floods it. This is the composition → hydrosphere continuity.
        let dry_h = submerged_fraction(&tables, |p| p.set("ab_H", 0.04));
        let wet_h = submerged_fraction(&tables, |p| p.set("ab_H", 0.5));
        let dry_o = submerged_fraction(&tables, |p| p.set("ab_O", 18.0));
        assert!(dry_h < earthlike, "less hydrogen should drain the ocean ({dry_h} vs {earthlike})");
        assert!(wet_h > earthlike, "more hydrogen should flood the world ({wet_h} vs {earthlike})");
        assert!(dry_o < earthlike, "less oxygen should drain the ocean ({dry_o} vs {earthlike})");
    }

    #[test]
    fn tuning_a_knob_changes_the_world() {
        let tables = load_tables().expect("tables load");
        let base = generate(&tables, &WorldParams::default(), 6, 42);

        let mut tuned = WorldParams::default();
        tuned.set("e3_plates", 3.0); // far from the default 8
        let other = generate(&tables, &tuned, 6, 42);

        assert_eq!(base.layers.len(), 6, "all epoch layers kept");
        // The tectonics layer (index 2) must differ once we change plate count.
        let changed = base.layers[2]
            .iter()
            .zip(&other.layers[2])
            .any(|(a, b)| a.plate != b.plate || a.elevation != b.elevation);
        assert!(changed, "changing e3_plates should change the tectonics layer");
    }

    #[test]
    fn boosting_an_element_shifts_the_composition() {
        let tables = load_tables().expect("tables load");
        let base = generate(&tables, &WorldParams::default(), 6, 7);

        let mut tuned = WorldParams::default();
        tuned.set("ab_Fe", 60.0); // make iron dominate the mix
        let other = generate(&tables, &tuned, 6, 7);

        // Epoch 1 (index 0) is the raw composition; far more cells should now be
        // iron-dominant than before.
        let fe_dominant = |layer: &[flicker_worldgen::HexState]| {
            layer
                .iter()
                .filter(|s| s.composition.dominant() == Some(26))
                .count()
        };
        assert!(
            fe_dominant(&other.layers[0]) > fe_dominant(&base.layers[0]),
            "boosting ab_Fe should make more cells iron-dominant"
        );
    }

    #[test]
    fn composition_view_recolours_when_the_mix_changes() {
        let tables = load_tables().expect("tables load");
        let base = generate(&tables, &WorldParams::default(), 6, 7);
        let mut tuned = WorldParams::default();
        tuned.set("ab_Fe", 60.0);
        let iron = generate(&tables, &tuned, 6, 7);

        // The continuous composition view must change colour across much of the
        // foundation layer (index 0) when the element mix is tuned — the whole
        // point of the diffuse material map.
        use crate::color::{cell_material, ViewMode};
        let (rb, ri) = (ranges_for(&base.layers[0]), ranges_for(&iron.layers[0]));
        let n = base.sphere.len();
        let changed = (0..n)
            .filter(|&i| {
                cell_material(ViewMode::Composition, &base.layers[0][i], &rb)
                    != cell_material(ViewMode::Composition, &iron.layers[0][i], &ri)
            })
            .count();
        assert!(
            changed > n / 4,
            "boosting iron should recolour much of the foundation map ({changed}/{n})"
        );
    }

    #[test]
    fn differentiation_thins_the_crust_with_more_iron() {
        let tables = load_tables().expect("tables load");
        let base = generate(&tables, &WorldParams::default(), 6, 7);
        let mut tuned = WorldParams::default();
        tuned.set("ab_Fe", 60.0);
        let iron = generate(&tables, &tuned, 6, 7);

        // Epoch 2 (layer 1) sinks the heavy metals; a heavier mix → less light
        // crust, so the mean crust fraction drops.
        let mean_crust =
            |l: &[HexState]| l.iter().map(|s| s.crust_fraction).sum::<f64>() / l.len() as f64;
        assert!(
            mean_crust(&iron.layers[1]) < mean_crust(&base.layers[1]),
            "more iron should sink → thinner light crust"
        );
    }

    #[test]
    fn relief_view_spans_depths_and_heights() {
        let tables = load_tables().expect("tables load");
        let w = generate(&tables, &WorldParams::default(), 8, 7);
        let layer = &w.layers[2]; // tectonics — real continents/basins/mountains
        let r = ranges_for(layer);
        let distinct: std::collections::HashSet<u32> = layer
            .iter()
            .map(|s| crate::color::cell_material(crate::color::ViewMode::Elevation, s, &r))
            .collect();
        assert!(
            distinct.len() > 8,
            "the hypsometric relief should span many depth/height bands ({} distinct)",
            distinct.len()
        );
    }

    #[test]
    fn hotspots_lift_the_tectonic_surface() {
        let tables = load_tables().expect("tables load");

        let mut off = WorldParams::default();
        off.set("e3_hotspots", 0.0);
        let flat = generate(&tables, &off, 12, 7);

        let mut on = WorldParams::default();
        on.set("e3_hotspots", 12.0);
        on.set("e3_hotspot_uplift", 1.0);
        let bumped = generate(&tables, &on, 12, 7);

        // Hotspot volcanism adds positive relief, so the tectonics layer (index 2)
        // has cells lifted above where they sat with no hotspots.
        let max_elev = |layer: &[flicker_worldgen::HexState]| {
            layer.iter().map(|s| s.elevation).fold(f32::MIN, f32::max)
        };
        assert!(
            max_elev(&bumped.layers[2]) > max_elev(&flat.layers[2]),
            "hotspots should lift the surface above the no-hotspot peak"
        );
    }

    #[test]
    fn axial_tilt_flattens_the_climate() {
        let tables = load_tables().expect("tables load");

        let mut upright = WorldParams::default();
        upright.set("e4_axial_tilt", 0.0);
        let cold_poles = generate(&tables, &upright, 12, 7);

        let mut tilted = WorldParams::default();
        tilted.set("e4_axial_tilt", 90.0);
        let warm_poles = generate(&tables, &tilted, 12, 7);

        // The hydrosphere layer (index 3) holds temperature; flattening the band
        // raises the coldest (polar) temperature.
        let min_temp = |layer: &[flicker_worldgen::HexState]| {
            layer.iter().map(|s| s.temperature).fold(f32::MAX, f32::min)
        };
        assert!(
            min_temp(&warm_poles.layers[3]) > min_temp(&cold_poles.layers[3]),
            "high axial tilt should warm the poles"
        );
    }

    #[test]
    fn reseeding_an_epoch_mutates_only_its_own_knobs() {
        // An Epoch-3 (index 2) reseed moves that epoch's knobs — deterministically,
        // within band — and leaves every other epoch's knobs (and the element mix)
        // exactly as they were.
        let baseline = WorldParams::default();
        let mut a = WorldParams::default();
        let mut b = WorldParams::default();
        mutate_epoch_params(&mut a, 2, 999);
        mutate_epoch_params(&mut b, 2, 999);

        for (ep, id, def, min, max) in PARAM_DEFS {
            if *ep == 2 {
                assert_eq!(a.get(id), b.get(id), "{id} not deterministic");
                let v = a.get(id);
                assert!(v >= *min - 1e-9 && v <= *max + 1e-9, "{id} = {v} out of band");
                let _ = def;
            } else {
                assert_eq!(a.get(id), baseline.get(id), "{id} (not epoch 2) was mutated");
            }
        }
        assert!(
            PARAM_DEFS
                .iter()
                .any(|(ep, id, def, _, _)| *ep == 2 && (a.get(id) - def).abs() > 1e-9),
            "the epoch-2 reseed moved no knob"
        );
        // The element mix belongs to Epoch 1 — an Epoch-3 reseed leaves it alone.
        for (sym, _) in ABUNDANCE_DEFS {
            let id = format!("ab_{sym}");
            assert_eq!(a.get(&id), baseline.get(&id), "{id} changed on an epoch-2 reseed");
        }
    }

    #[test]
    fn reseeding_a_layer_preserves_the_upstream_layers() {
        let tables = load_tables().expect("tables load");
        let params = WorldParams::default();
        let seeds = seed_chain(DEFAULT_SEED);
        let world = generate_with_seeds(&tables, &params, 8, &seeds);

        // Reseed at epoch index 2 (Tectonics): advance seeds[2], re-derive 3..,
        // mutate only epoch-2 params — exactly what clicking "new seed" on that
        // layer does.
        let e = 2;
        let mut seeds2 = seeds.clone();
        seeds2[e] = next_seed(seeds2[e]);
        for k in (e + 1)..seeds2.len() {
            seeds2[k] = next_seed(seeds2[k - 1]);
        }
        let mut params2 = params.clone();
        mutate_epoch_params(&mut params2, e, seeds2[e]);
        let reseeded = generate_with_seeds(&tables, &params2, 8, &seeds2);

        // Upstream layers are byte-identical; the reseeded layer changes.
        assert_eq!(world.layers[0], reseeded.layers[0], "Epoch 1 changed on a layer-3 reseed");
        assert_eq!(world.layers[1], reseeded.layers[1], "Epoch 2 changed on a layer-3 reseed");
        assert_ne!(world.layers[2], reseeded.layers[2], "the reseeded layer didn't change");

        // Isostasy: the gross land/ocean pattern follows the (unchanged) crust
        // buoyancy, so reseeding Epoch 3 moves the plate boundaries / mountain belts
        // but the continents stay put. In both worlds the buoyant crust rides high.
        let mean_elev_of_top_third_buoyancy = |layer: &[flicker_worldgen::HexState]| {
            let mut idx: Vec<usize> = (0..layer.len()).collect();
            idx.sort_by(|&a, &b| {
                layer[a].crust_fraction.partial_cmp(&layer[b].crust_fraction).unwrap()
            });
            let third = layer.len() / 3;
            let mean = |sl: &[usize]| sl.iter().map(|&i| layer[i].elevation).sum::<f32>() / sl.len() as f32;
            (mean(&idx[..third]), mean(&idx[idx.len() - third..]))
        };
        for layer in [&world.layers[2], &reseeded.layers[2]] {
            let (low_buoy, high_buoy) = mean_elev_of_top_third_buoyancy(layer);
            // The crust-driven ramp dominates the tectonic deformation: dense crust
            // averages an ocean basin (below the epoch's sea level, 0), buoyant crust
            // averages continental land (above it). This is what makes the Elevation
            // view read as the inherited Crust view rather than random mountains.
            assert!(low_buoy < 0.0, "dense-crust basins should average ocean ({low_buoy})");
            assert!(high_buoy > 0.0, "buoyant-crust provinces should average land ({high_buoy})");
        }
        // But the tectonic detail did move (boundaries re-rolled with the new seed).
        let boundaries = |layer: &[flicker_worldgen::HexState]| {
            layer.iter().map(|s| s.boundary).collect::<Vec<_>>()
        };
        assert_ne!(
            boundaries(&world.layers[2]),
            boundaries(&reseeded.layers[2]),
            "reseeding Epoch 3 should move the plate boundaries"
        );
    }

    #[test]
    fn higher_contrast_widens_the_crust_buoyancy_spread() {
        // The Epoch 1-2 retune: sharper province contrast gives Epoch 2's crust
        // fraction (the buoyancy signal Epoch 3 isostasy will sculpt) a wider
        // spatial spread — distinct continental-buoyant vs oceanic-dense regions.
        let tables = load_tables().expect("tables load");
        let mut lo = WorldParams::default();
        lo.set("e1_contrast", 1.0);
        let mut hi = WorldParams::default();
        hi.set("e1_contrast", 6.0);
        let spread = |p: &WorldParams| {
            let w = generate(&tables, p, 8, 7);
            let r = ranges_for(&w.layers[1]); // Epoch 2 crust fraction
            r.crust_max - r.crust_min
        };
        assert!(
            spread(&hi) > spread(&lo),
            "higher contrast should widen the crust-buoyancy spread for isostasy"
        );
    }
}
