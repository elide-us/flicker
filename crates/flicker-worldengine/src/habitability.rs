//! **The life-supporting condition observer** — a pure classifier over the live [`World`].
//!
//! It reads what the causal tick simulation has already produced and reports *where each
//! condition axis sits* relative to its habitable "green band". It adds **no causal rules**
//! and never steers the sim (the observer/detection model — memory
//! `habitability-detection-model`, unification rulings R3/R8): every world is always
//! *somewhere*, an epoch is a named region of this axis space, and **life-supporting is the
//! one coincidence where every axis is simultaneously in band** — detected, never scripted.
//!
//! Axes whose driving process is **not simulated yet** report `signal = None` ("no signal");
//! they are shown greyed and do **not** count toward the verdict. As each new procedure lands
//! (surface-temperature energy balance, ocean chemistry, …) its axis gains a live signal and
//! begins to move — the companion view for growing the sim.

use std::collections::BTreeMap;

use flicker_materials::ElementId;
use flicker_worldgen::cooling::normalized;
use flicker_worldgen::LayerKind;

use crate::sim::World;

/// One condition axis: where the planet sits (`signal`, `0..1`) against its habitable band
/// `[lo, hi]`. `signal == None` means the causal stage that would drive this axis is not in
/// the sim yet — displayed as "no signal", excluded from the verdict.
pub struct Axis {
    /// Display name (the gauge label).
    pub name: &'static str,
    /// Normalised position `0..1`, or `None` when the driving process isn't simulated yet.
    pub signal: Option<f32>,
    /// Habitable green-band lower bound (`0..1`).
    pub lo: f32,
    /// Habitable green-band upper bound (`0..1`).
    pub hi: f32,
    /// What the low / high ends of this axis mean (gauge end captions).
    pub low_label: &'static str,
    pub high_label: &'static str,
}

impl Axis {
    /// Whether this axis has a live signal sitting inside its habitable band.
    pub fn in_band(&self) -> bool {
        matches!(self.signal, Some(v) if v >= self.lo && v <= self.hi)
    }
}

/// The five-axis reading plus the aggregate life-supporting verdict for a [`World`].
pub struct Habitability {
    /// The condition axes, in display order.
    pub axes: Vec<Axis>,
    /// True only when **every** axis has a live signal **and** every signal is in its band —
    /// the simultaneous coincidence. With any axis still unsimulated this stays `false`,
    /// which is the honest state: the world cannot be judged life-supporting until every
    /// condition can be observed.
    pub life_supporting: bool,
    /// How many axes are currently in their band (of the live ones).
    pub axes_in_band: usize,
    /// How many axes have a live signal at all.
    pub axes_live: usize,
    /// The atmosphere's current **kind** — the dominant outgassed/delivered gas species (steam,
    /// CO₂ hotbox, N₂ temperate, SO₂ volcanic, …). `"—"` when there is no atmosphere yet.
    pub atmosphere_kind: &'static str,
}

/// The volatile elements that make up the air (H, C, N, S, Cl) — the same set the outgassing
/// series draws on. The atmosphere axis reads how much of this inventory has degassed.
const VOLATILES: [ElementId; 5] = [1, 6, 7, 16, 17];

/// `(volatile mass in the atmosphere, total volatile mass in the planet)` — a conserved
/// reference, so the atmosphere axis reads the *fraction degassed* (0 = locked in the interior,
/// 1 = fully outgassed), independent of the total inventory.
fn volatile_split(world: &World) -> (f64, f64) {
    let (mut air, mut total) = (0.0f64, 0.0f64);
    for c in &world.cells {
        for layer in c.column.layers() {
            let m: f64 = VOLATILES
                .iter()
                .map(|&el| layer.composition.amount(el))
                .sum();
            total += m;
            if layer.kind == LayerKind::Atmosphere {
                air += m;
            }
        }
    }
    (air, total)
}

/// The atmosphere's dominant gas **compound** across the planet → a human "kind" label. The
/// ids are the stable `compounds.json` atmospheric species (canon, never reused).
fn atmosphere_kind(world: &World) -> &'static str {
    let mut totals: BTreeMap<u16, f64> = BTreeMap::new();
    for c in &world.cells {
        if let Some(i) = c.column.find(LayerKind::Atmosphere) {
            for (id, amt) in c.column.layers()[i].compounds.iter() {
                *totals.entry(id).or_default() += amt;
            }
        }
    }
    let dominant = totals
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(id, _)| id);
    match dominant {
        Some(1) => "steam (H₂O)",
        Some(2) => "carbon hotbox (CO₂)",
        Some(91) => "temperate (N₂)",
        Some(92) => "volcanic (SO₂)",
        Some(93) => "acidic (HCl)",
        Some(94) => "reducing (CH₄)",
        Some(95) => "reducing (NH₃)",
        _ => "—",
    }
}

// ── illustrative green-band calibrations ─────────────────────────────────────────
// The exact bounds are calibration (memory: "the exact green-band bounds per axis" is an
// open spec point). These are plausible placeholders so the gauges read sensibly while the
// causal signals mature — they are detection thresholds, never targets the sim aims for.

/// Interior: the normalized cooling clock. Too hot = magma ocean; too cold = a dead, frozen
/// interior; the green band is the mobile-lid window in between (crust firm, interior still
/// convecting — around the solidus).
const INTERIOR_LO: f32 = 0.12;
const INTERIOR_HI: f32 = 0.32;
/// Hydrosphere: the fraction of the planet's water endowment now standing as liquid ocean.
/// Too little = desert; the green band is "oceans present" (with the finite budget it cannot
/// physically drown, so the upper bound is generous).
const HYDRO_LO: f32 = 0.30;
const HYDRO_HI: f32 = 0.95;
/// Atmosphere: the fraction of the water endowment held as vapour/gas. A thin envelope is
/// fine; a thick one (nothing condensed, a steam runaway) is not.
const ATMO_LO: f32 = 0.05;
const ATMO_HI: f32 = 0.55;

/// Total mass held in layers of `kind` across every cell's column.
fn layer_mass(world: &World, kind: LayerKind) -> f64 {
    world
        .cells
        .iter()
        .filter_map(|c| c.column.find(kind).map(|i| c.column.layers()[i].mass()))
        .sum()
}

/// Observe a [`World`]: read each condition axis off the state the tick sim has produced.
/// Pure — it mutates nothing and encodes no causal rule.
pub fn observe(world: &World) -> Habitability {
    // Interior / tectonic — the live cooling clock (magma ocean → mobile lid → dead).
    let interior = Axis {
        name: "Interior",
        signal: Some(normalized(world.temp)), // the 0..1 read of the Kelvin temperature
        lo: INTERIOR_LO,
        hi: INTERIOR_HI,
        low_label: "dead/frozen",
        high_label: "magma ocean",
    };

    // Hydrosphere: how much of the planet's water endowment (delivered + still-to-deliver)
    // now stands as liquid ocean. Zero endowment (no Water compound) ⇒ no signal.
    let endowment = world.delivered + world.water_budget();
    let hydro_signal = (endowment > 0.0)
        .then(|| (layer_mass(world, LayerKind::Ocean) / endowment).clamp(0.0, 1.0) as f32);

    // Atmosphere: the fraction of the planet's volatile inventory now degassed into the air
    // (a conserved reference). No volatiles at all ⇒ no signal.
    let (air_vol, total_vol) = volatile_split(world);
    let atmo_signal = (total_vol > 0.0).then(|| (air_vol / total_vol).clamp(0.0, 1.0) as f32);

    let hydrosphere = Axis {
        name: "Hydrosphere",
        signal: hydro_signal,
        lo: HYDRO_LO,
        hi: HYDRO_HI,
        low_label: "desert",
        high_label: "drowned",
    };
    let atmosphere = Axis {
        name: "Atmosphere",
        signal: atmo_signal,
        lo: ATMO_LO,
        hi: ATMO_HI,
        low_label: "too thin",
        high_label: "runaway",
    };

    // Not yet simulated — their causal stages (a surface energy balance, ocean chemistry) do
    // not exist in the tick sim, so they honestly have no signal until those procedures land.
    let surface = Axis {
        name: "Surface temp",
        signal: None,
        lo: 0.35,
        hi: 0.65,
        low_label: "frozen",
        high_label: "hothouse",
    };
    let ocean_ph = Axis {
        name: "Ocean pH",
        signal: None,
        lo: 0.35,
        hi: 0.65,
        low_label: "acidic",
        high_label: "alkaline",
    };

    let axes = vec![interior, surface, atmosphere, hydrosphere, ocean_ph];
    let axes_live = axes.iter().filter(|a| a.signal.is_some()).count();
    let axes_in_band = axes.iter().filter(|a| a.in_band()).count();
    // Life-supporting = the full conjunction: every axis live AND in band.
    let life_supporting = axes.iter().all(|a| a.in_band());

    Habitability {
        axes,
        life_supporting,
        axes_in_band,
        axes_live,
        atmosphere_kind: atmosphere_kind(world),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Simulation;

    fn sim() -> Simulation {
        use crate::config::WorldConfig;
        use crate::levers::GeneratorParams;
        use flicker_materials::{JsonTableSource, Tables};
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        let tables = Tables::from_source(&JsonTableSource::new(dir)).expect("tables");
        let params = GeneratorParams::from_dir(dir).expect("params");
        let mut config = WorldConfig::from_params(&params);
        config.freq = 6;
        Simulation::new(tables, params, config)
    }

    /// INVARIANT: the observer is a pure classifier — it reads five axes and mutates nothing
    /// (re-observing the same state yields the identical reading). No outcome is asserted.
    #[test]
    fn observer_reads_five_axes_and_is_pure() {
        let mut s = sim();
        let h = observe(s.state_at(0));
        assert_eq!(h.axes.len(), 5, "the five condition axes");
        let again = observe(s.state_at(0));
        assert_eq!(
            h.axes[0].signal, again.axes[0].signal,
            "the read is pure (no mutation)"
        );
    }
}
