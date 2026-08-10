//! **The life-supporting condition observer** — a pure classifier over the live
//! [`World`], ported from the legacy engine's `habitability.rs` per the T7b
//! ruling (the LOCKED 5-axis detection model, unification rulings R8/R9).
//!
//! It reads what the causal pipeline has already produced and reports *where
//! each condition axis sits* relative to its habitable green band. It adds **no
//! causal rules** and never steers the sim: every world is always *somewhere*,
//! an epoch is a named region of this axis space, and **life-supporting is the
//! one coincidence where every axis is simultaneously in band** — detected,
//! never scripted.
//!
//! The port turns on what the sky tier made real: the legacy observer carried
//! `Surface temp` and `Ocean pH` as dead axes ("no signal — the driving process
//! is not simulated yet"). The greenhouse read drives the first now, and the
//! carbon system drives the second. An axis with no signal is greyed and does
//! not count toward the verdict — and the verdict is the **gate indication**
//! for the truth migration: it tells the God Mode maintainer the planet is
//! ready; the maintainer still arms the migration. An observer is never causal.

use flicker_materials::ElementId;

use crate::mantle::MAGMA_OCEAN_K;
use crate::planet::{p_co2_pa, World};
use crate::Levers;

/// One condition axis: where the planet sits (`signal`, `0..1`) against its
/// habitable band `[lo, hi]`. `signal == None` means the driving process has
/// nothing to say yet — displayed greyed, excluded from the verdict.
///
/// `name` and the end labels are **stringtable tokens** — the scene resolves
/// them (model-channel strings gate); the observer never emits display copy.
#[derive(Clone)]
pub struct Axis {
    /// Gauge label token.
    pub name: &'static str,
    /// Normalised position `0..1`, or `None` while the axis has no signal.
    pub signal: Option<f64>,
    /// Habitable green-band bounds (`0..1`).
    pub lo: f64,
    pub hi: f64,
    /// End-caption tokens: what the low / high ends of this axis mean.
    pub low_label: &'static str,
    pub high_label: &'static str,
}

impl Axis {
    /// Whether this axis has a live signal sitting inside its habitable band.
    pub fn in_band(&self) -> bool {
        matches!(self.signal, Some(v) if v >= self.lo && v <= self.hi)
    }
}

/// The five-axis reading plus the aggregate life-supporting verdict.
#[derive(Clone)]
pub struct Habitability {
    /// The condition axes, in display order.
    pub axes: Vec<Axis>,
    /// True only when **every** axis has a live signal **and** every signal is
    /// in its band — the simultaneous coincidence.
    pub life_supporting: bool,
    /// How many axes are currently in their band.
    pub axes_in_band: usize,
    /// How many axes have a live signal at all.
    pub axes_live: usize,
}

/// The volatile elements that make up the air (H, C, N, S, Cl) — the same set
/// the outgassing series draws on. The atmosphere axis reads how much of this
/// inventory has degassed.
const VOLATILES: [ElementId; 5] = [1, 6, 7, 16, 17];

// ── Illustrative green-band calibrations (ported; the exact bounds remain an
// open spec point). Detection thresholds, never targets the sim aims for. ──

/// The interior cooling clock: too hot = magma ocean, too cold = dead; the
/// green band is the mobile-lid window around the solidus.
const INTERIOR_BAND: (f64, f64) = (0.12, 0.32);
/// Mean radiative surface temperature mapped over 150..450 K: the band is the
/// liquid-water window (~255..345 K on that ramp).
const SURFACE_BAND: (f64, f64) = (0.35, 0.65);
/// Fraction of the volatile inventory degassed: a thin envelope is fine, a
/// runaway (nothing condensed) is not.
const ATMO_BAND: (f64, f64) = (0.05, 0.55);
/// Fraction of the water endowment standing as liquid sea.
const HYDRO_BAND: (f64, f64) = (0.30, 0.95);
/// The carbon system's acidity read (log-pCO₂): a hotbox sea is acid, a
/// carbon-starved one alkaline; Earth's trace sits mid-band.
const PH_BAND: (f64, f64) = (0.35, 0.65);

/// The five bands in display order — the single source the HUD's gauge nodes
/// bake their green zones from ([lo, hi] pairs, same order as [`observe`]).
pub const BANDS: [(f64, f64); 5] =
    [INTERIOR_BAND, SURFACE_BAND, ATMO_BAND, HYDRO_BAND, PH_BAND];

/// Cold end of the interior normalisation, K — a dead, frozen mantle.
const INTERIOR_COLD_K: f64 = 250.0;
/// The surface ramp: 150..450 K maps to 0..1.
const SURFACE_COLD_K: f64 = 150.0;
const SURFACE_SPAN_K: f64 = 300.0;

/// Observe a [`World`]: read each condition axis off the state the pipeline has
/// produced. Pure — it mutates nothing and encodes no causal rule. `levers`
/// supplies the two boundary references the axes are read against (stellar
/// heat for the surface ramp, the water budget as the hydrosphere endowment) —
/// **at reference scale**, like every lever: the observer sizes them to this
/// world itself ([`Levers::sized`]), so the hydrosphere axis compares the sized
/// ocean against the sized endowment rather than under-reading every world
/// smaller than the reference by its size³.
pub fn observe(world: &World, levers: &Levers) -> Habitability {
    let levers = &levers.sized(world.size_scale());
    let n = world.mantle.n_cells().max(1) as f64;

    // Interior — the cooling clock, normalised magma-ocean → cold.
    let mean_mantle = world.mantle.temp_k.iter().sum::<f64>() / n;
    let interior = Axis {
        name: "$chem_ax_interior",
        signal: Some(
            ((mean_mantle - INTERIOR_COLD_K) / (MAGMA_OCEAN_K - INTERIOR_COLD_K)).clamp(0.0, 1.0),
        ),
        lo: INTERIOR_BAND.0,
        hi: INTERIOR_BAND.1,
        low_label: "$chem_axlo_dead",
        high_label: "$chem_axhi_magma",
    };

    // Surface — the radiative + greenhouse read the sky tier made live.
    let surface_k = crate::surface::mean_surface_temp_k(world, levers.stellar_heat);
    let surface = Axis {
        name: "$chem_ax_surface",
        signal: Some(((surface_k - SURFACE_COLD_K) / SURFACE_SPAN_K).clamp(0.0, 1.0)),
        lo: SURFACE_BAND.0,
        hi: SURFACE_BAND.1,
        low_label: "$chem_axlo_frozen",
        high_label: "$chem_axhi_hothouse",
    };

    // Atmosphere — the fraction of the planet's DEGASSABLE volatile inventory
    // now aloft: air over air + ocean + mantle + crust. The core is deliberately
    // NOT in the denominator: metal-locked volatiles can never reach the sky —
    // and the core holds ~97% of all sulfur, which is itself ~97% of the whole
    // H/C/N/S/Cl budget, once differentiation has run (defect 7E01115B). With
    // the core counted, the axis ceiling fell below the band's own floor the
    // moment the core claimed its sulfur, and the verdict could never fire.
    let mut air_vol = 0.0;
    let mut total_vol = 0.0;
    for &v in &VOLATILES {
        let in_air = world.reservoirs.atmosphere.contents.amount(v);
        air_vol += in_air;
        total_vol += in_air
            + world.reservoirs.ocean.contents.amount(v)
            + world.mantle.element_mass(v)
            + world
                .columns
                .iter()
                .map(|c| c.layers.iter().map(|l| l.elements.amount(v)).sum::<f64>())
                .sum::<f64>();
    }
    let atmosphere = Axis {
        name: "$chem_ax_atmosphere",
        signal: (total_vol > 0.0).then(|| (air_vol / total_vol).clamp(0.0, 1.0)),
        lo: ATMO_BAND.0,
        hi: ATMO_BAND.1,
        low_label: "$chem_axlo_thin",
        high_label: "$chem_axhi_runaway",
    };

    // Hydrosphere — how much of the water endowment stands as liquid sea.
    let hydrosphere = Axis {
        name: "$chem_ax_hydrosphere",
        signal: (levers.water_budget_kg > 0.0)
            .then(|| (world.reservoirs.ocean.mass_kg() / levers.water_budget_kg).clamp(0.0, 1.0)),
        lo: HYDRO_BAND.0,
        hi: HYDRO_BAND.1,
        low_label: "$chem_axlo_desert",
        high_label: "$chem_axhi_drowned",
    };

    // Ocean pH — the carbon system's acidity read: seawater pH tracks pCO₂
    // over geologic states, so the log of the sky's carbon IS the proxy.
    // 0.01 Pa .. 10 MPa maps alkaline → acid; Earth's 40 Pa sits mid-band.
    let ocean_ph = Axis {
        name: "$chem_ax_ocean_ph",
        signal: (world.reservoirs.ocean.mass_kg() > 0.0)
            .then(|| (1.0 - (p_co2_pa(world).max(1e-2).log10() + 2.0) / 9.0).clamp(0.0, 1.0)),
        lo: PH_BAND.0,
        hi: PH_BAND.1,
        low_label: "$chem_axlo_acidic",
        high_label: "$chem_axhi_alkaline",
    };

    let axes = vec![interior, surface, atmosphere, hydrosphere, ocean_ph];
    let axes_live = axes.iter().filter(|a| a.signal.is_some()).count();
    let axes_in_band = axes.iter().filter(|a| a.in_band()).count();
    let life_supporting = axes.iter().all(|a| a.in_band());
    Habitability { axes, life_supporting, axes_in_band, axes_live }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atmosphere::{CARBON_DIOXIDE, NITROGEN};
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    fn world(seed: u64) -> (World, Tables) {
        let t = Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables");
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        (World::seed(icosphere(4), b, &t, seed), t)
    }

    /// INVARIANT (ported): the observer is a pure classifier — five axes, no
    /// mutation, and a magma-ocean seed honestly reads *not* life-supporting
    /// (interior pegged hot, no sea).
    #[test]
    fn observer_reads_five_axes_and_is_pure() {
        let (w, _t) = world(51);
        let levers = Levers::brisk();
        let h = observe(&w, &levers);
        assert_eq!(h.axes.len(), 5, "the five condition axes");
        assert!(!h.life_supporting, "a magma ocean is not life-supporting");
        assert!(h.axes[0].signal.unwrap_or(0.0) > INTERIOR_BAND.1, "interior pegged hot");
        let again = observe(&w, &levers);
        assert_eq!(h.axes[0].signal, again.axes[0].signal, "the read is pure");
    }

    /// The coincidence is reachable: hand a world the conditions a temperate
    /// wet planet would have earned — cooled lid, a sea holding most of the
    /// endowment, a modest degassed envelope, a trace-CO₂ sky — and every axis
    /// reads in band at once. The verdict is detection, and this is the state
    /// it detects.
    #[test]
    fn a_temperate_wet_world_reads_life_supporting() {
        let (mut w, t) = world(52);
        let levers = Levers::brisk();

        // A cooled, mobile-lid interior.
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 1000.0;
        }
        // A mobile lid means an actual lid — without one the surface reads the
        // 1000 K mantle and the temperate world is a lava world.
        crate::planet::freeze_lid(&mut w);

        // A sea holding half the endowment — THIS world's endowment, which on a
        // freq-4 planetoid is size³ of the reference lever. An unsized sea here
        // is an Earth ocean dumped on a 270-km world, and it drowns the
        // atmosphere axis's denominator.
        let sea = levers.sized(w.size_scale()).water_budget_kg * 0.5;
        w.reservoirs.ocean.contents.add(1, sea / 9.0);
        w.reservoirs.ocean.contents.add(8, sea * 8.0 / 9.0);

        // A modest degassed envelope: move ~10% of the mantle's volatile
        // inventory into the air (conserved — a hand-run of what outgassing
        // does), flown as N₂ plus a trace of CO₂.
        for &v in &VOLATILES {
            let tenth = w.mantle.element_mass(v) * 0.1;
            let per_cell = tenth / w.mantle.n_cells() as f64;
            for c in 0..w.mantle.n_cells() {
                w.mantle.remove(c, v, per_cell);
            }
            w.reservoirs.atmosphere.contents.add(v, tenth);
        }
        w.reservoirs.atmosphere.species.add(NITROGEN, 5.0e17);
        // A trace-CO₂ sky: the booked species mass sets pCO₂ ≈ Earth's ~40 Pa.
        let area = w.cell_area_m2() * w.columns.len() as f64;
        let trace = 40.0 * area / w.gravity_m_s2();
        w.reservoirs.atmosphere.species.add(CARBON_DIOXIDE, trace);

        let h = observe(&w, &levers);
        let _ = t;
        for ax in &h.axes {
            assert!(
                ax.in_band(),
                "{} in band (signal {:?} vs {}..{})",
                ax.name,
                ax.signal,
                ax.lo,
                ax.hi
            );
        }
        assert!(h.life_supporting, "every axis in band at once IS the verdict");
        assert_eq!(h.axes_live, 5, "every axis has a signal");
    }
}
