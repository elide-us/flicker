//! **The planetary cooling clock** — the master variable the continuous-sim
//! redesign keys every process off (`docs/flicker-world-continuous-sim.md` §1).
//!
//! The planet is born molten and radiates its heat budget away over deep time. This
//! module owns that single **global thermal state** `T` (normalized `0..1`:
//! `T_MOLTEN` = a whole-planet magma ocean, `T_SPACE` = radiated cold) and the
//! **Newtonian cooling** law that decays it — `T -= k·(T − T_space)` per step, whose
//! closed form ([`temperature_at`]) gives the logarithmic "fast early, geological
//! crawl late" feel for free.
//!
//! `k` is **composition-modulated**: the planet's **radiogenic** elements (in the
//! Prism table, the real heat producers present are **potassium** and **uranium**)
//! slow the decay, so a recipe rich in them stays hot longer and reaches every onset
//! *later*. That is the source of planet-to-planet variety — the recipe sets the
//! pace, and thus when the tectonics onset (and, in later slices, water delivery /
//! outgassing / chemistry) fires.
//!
//! Two things scale off `T` in this slice:
//! - **Convection vigor** ([`convection_vigor`]) — fierce molten stir early, gentler
//!   as it cools. Mean-preserving over the molten era, so the *total* stir matches the
//!   old uniform pass (the Earth-like guardrail) while the *distribution* front-loads.
//! - **Differentiation completeness** ([`differentiation_settle`]) — a planet that
//!   stays molten longer sorts its density more fully; a fast quench freezes with
//!   heavies stranded in the crust (an emergent, undifferentiated world).
//!
//! And the **tectonics onset** ([`tectonics_onset_delay`]) is a *condition* on the
//! state — convection stops merely stirring melt and starts gripping a rigid lid once
//! `T` falls below the crust solidus **and** the crust has firmed into a coherent lid
//! ([`coherent_lid`]). A world that never firms a mobile crust (stays molten within
//! the window, or never grows a lid) is a valid stagnant/mushy outcome, not a failure.
//!
//! `T` is *global* — one planetary heat budget, enough for the accounting here; a
//! per-hex surface temperature ([`HexState::temperature`], written in °C by Epoch 4)
//! is a separate, later-stage field and must not be confused with this normalized `T`.

use flicker_materials::ElementId;
use flicker_worldstate::Composition;

use crate::epoch3::MY_PER_TECTONIC_STEP;
use crate::state::HexState;

// ── the thermal scale (real KELVIN) ──────────────────────────────────────────────
// Temperature is a real state in Kelvin, adjusted each tick by radiative heat loss (the
// dominant term today; radiogenics modulate the rate). It decays TOWARD space — there is no
// floor it "aims for"; the phase transitions it crosses (rock solidus, water boil/freeze) are
// emergent, not targets. A derived [`normalized`] `0..1` is kept only for rendering the heat
// curve and scaling the "how molten" processes. (Fuller model, later: per-cell/per-layer heat
// balance — radiogenic gain, inter-layer transfer, latent heat — so a layer can warm as well
// as cool and change state; today one global heat budget suffices.)

/// The magma-ocean temperature at t=0 (K) — a whole-planet melt.
pub const T_MOLTEN: f32 = 1900.0;
/// Deep space (K) — radiative heat loss decays toward it. Not a target/floor; just where an
/// isolated body's radiation goes (≈ the cosmic microwave background).
pub const T_SPACE: f32 = 2.7;

/// Heat-loss **time constant** in millions of years — a rough geological estimate (~1 BY) that
/// sets the real-time cooling rate. THIS is the honest rate dial: the planet bleeds its heat
/// over deep time, not the ~20 My the old per-tick guess produced. Tunable.
pub const COOLING_TAU_MY: f32 = 1000.0;
/// Baseline Newtonian cooling coefficient **per tick**, for a radiogenic-free planet — derived
/// from the real time constant and the tick length ([`MY_PER_TECTONIC_STEP`]×5 = one tick), so
/// the decay plays out over billions of years. Radiogenic content reduces it (see [`cooling_k`]).
pub const BASE_COOLING_K: f32 = (MY_PER_TECTONIC_STEP * 5.0) / COOLING_TAU_MY;

/// The planet's temperature as a `0..1` fraction of its magma-ocean start above space — the
/// derived read the heat-curve render and the "how molten" process rates use. `1` = magma
/// ocean, `0` = radiated to space. (The Kelvin `T` is the truth; this is just a rescale.)
pub fn normalized(temp: f32) -> f32 {
    ((temp - T_SPACE) / (T_MOLTEN - T_SPACE)).clamp(0.0, 1.0)
}

// ── radiogenic modulation (what sets each planet's pace) ────────────────────────

/// Potassium (Z=19) — the dominant radiogenic heat producer *present in the recipe*
/// (Earth-like ≈ 2.6 % crustal mass; the default's main cooling brake).
const POTASSIUM: ElementId = 19;
/// Uranium (Z=92) — a far stronger heat producer per unit mass, but only a trace in
/// the default (unlisted in `abundance.json` → the Epoch-1 floor). Matters when a
/// recipe deliberately enriches it.
const URANIUM: ElementId = 92;
/// Per-mass radiogenic **heat weight** of potassium (the unit reference).
pub const K_HEAT: f32 = 1.0;
/// Per-mass radiogenic heat weight of uranium, relative to potassium — U runs far
/// hotter per unit mass, so even trace enrichment slows cooling noticeably.
pub const U_HEAT: f32 = 4.0;

/// Ceiling on how much radiogenic content can slow cooling — a radiogenic-saturated
/// planet cools at `BASE_COOLING_K · (1 − MAX_RADIOGENIC_SLOWDOWN)`.
pub const MAX_RADIOGENIC_SLOWDOWN: f32 = 0.6;
/// Radiogenic heat index at which the slowdown reaches half its ceiling (a saturating
/// response, so cranking radiogenics has diminishing returns rather than a runaway).
pub const RADIOGENIC_HALF: f32 = 0.03;

// ── the onset thresholds (the Earth-like guardrails) ────────────────────────────

/// Silicate crust **solidus** (K): below this the lid is rigid rock rather than melt — the
/// real ~1400 K value, so crust freezes when the temperature physically crosses it.
pub const T_SOLIDUS: f32 = 1400.0;
/// Density differentiation stalls once the melt has cooled past this (it needs a liquid to
/// sort) — coincides with the solidus (K).
pub const T_DIFF_FREEZE: f32 = 1400.0;

/// **Water-delivery window** (K) — at or below this the surface is cool enough to *retain* an
/// outer-system water veneer rather than boil it straight off, so delivery begins accumulating.
/// A condition on the state, not a schedule — how much water ends up delivered is emergent (how
/// long the recipe lingers here against a finite budget), never a target.
pub const T_WATER_DELIVERY: f32 = 500.0;
/// **Water condensation point** (K) — below this water vapour condenses to liquid (feeds the
/// ocean); above it, it is vapour (held in the atmosphere). The real ~373 K boiling point.
/// Whether an ocean results is emergent from the temperature path through it — not a target.
pub const T_CONDENSE: f32 = 373.0;
/// Differentiation completeness accrued per step per unit of `(T − T_DIFF_FREEZE)`;
/// tuned so the default molten era reaches full settle (≈ today) while a fast quench
/// falls short (heavies stranded in the crust).
pub const DIFF_RATE: f32 = 0.2;

/// A cell counts toward the coherent lid once its `crust_fraction` reaches this — a
/// firmed silicate raft rather than open melt.
pub const CRUST_FIRM: f32 = 0.5;
/// Share of cells that must be firmed for the lid to be *coherent* enough for plate
/// tectonics to engage. Below it (a crust-poor, metal-dominated world), no plates
/// ever form regardless of temperature — a valid stagnant outcome.
pub const LID_SHARE: f32 = 0.5;

// ── the model ───────────────────────────────────────────────────────────────────

/// Radiogenic heat weight of an element (`0` for non-producers) — the **single source**
/// for both the global cooling pace ([`radiogenic_index`]) and the per-cell molten heat
/// map ([`cell_radiogenic_heat`]).
pub fn element_heat_weight(el: ElementId) -> f32 {
    match el {
        POTASSIUM => K_HEAT,
        URANIUM => U_HEAT,
        _ => 0.0,
    }
}

/// The planet's **radiogenic heat index**: the heat-weighted mass fraction of its
/// radiogenic elements (potassium + uranium) across the whole composition. Conserved
/// (composition never depletes), so it is the same at every epoch — a stable pace dial.
pub fn radiogenic_index(cells: &[HexState]) -> f32 {
    let (mut heat, mut total) = (0.0f64, 0.0f64);
    for c in cells {
        total += c.composition.total();
        for (el, amount) in c.composition.iter() {
            heat += amount * element_heat_weight(el) as f64;
        }
    }
    if total > 0.0 {
        (heat / total) as f32
    } else {
        0.0
    }
}

/// **Per-cell radiogenic heat production** — the heat-weighted mass fraction of K + U in
/// ONE cell's composition (same weighting as [`radiogenic_index`], but local). This is
/// the per-cell heat SOURCE the molten convection adds to its buoyancy heat map, so a
/// radioactive-rich cell runs hot on its own account: heat and material distribution
/// become two reads of the one molten layer, not separate systems.
pub fn cell_radiogenic_heat(comp: &Composition) -> f32 {
    let total = comp.total();
    if total <= 0.0 {
        return 0.0;
    }
    let mut heat = 0.0f64;
    for (el, amount) in comp.iter() {
        heat += amount * element_heat_weight(el) as f64;
    }
    (heat / total) as f32
}

/// The per-step Newtonian cooling coefficient for a composition: [`BASE_COOLING_K`]
/// reduced by a saturating function of its [`radiogenic_index`]. More radiogenics →
/// smaller `k` → slower cooling → later onsets.
pub fn cooling_k(cells: &[HexState]) -> f32 {
    let idx = radiogenic_index(cells);
    let slowdown = MAX_RADIOGENIC_SLOWDOWN * idx / (idx + RADIOGENIC_HALF);
    (BASE_COOLING_K * (1.0 - slowdown)).max(0.0)
}

/// Planetary temperature after `step` Newtonian-cooling ticks — the closed form of
/// `T -= k·(T − T_space)`: `T = T_space + (T_MOLTEN − T_space)·(1 − k)^step`. The
/// geometric decay is what gives the scrubber its logarithmic, front-loaded feel.
pub fn temperature_at(k: f32, step: u32) -> f32 {
    T_SPACE + (T_MOLTEN - T_SPACE) * (1.0 - k).powi(step as i32)
}

/// **One Newtonian cooling tick**: `T ← T − k·(T − T_space)`. The tick-sim's radiative
/// cooling process applies this each tick, so `T` is a *state variable the sim produces*
/// rather than the `temperature_at` closed form over a scrub cursor. Iterating this from
/// `T_MOLTEN` reproduces `temperature_at(k, step)` exactly.
pub fn cooling_step(temp: f32, k: f32) -> f32 {
    temp - k * (temp - T_SPACE)
}

/// Per-step **convection vigor** over a molten era of `full_steps` steps: the thermal
/// state at each step, **normalized so its mean is 1** across the whole era. So a
/// scrubbed prefix is applied as-is, the stir is *fierce early and gentle late* (vigor
/// ∝ `T`), yet the total over the default budget matches the old uniform pass — the
/// guardrail that keeps the downstream (E6) landscape from regressing. Empty for a
/// zero-length era.
pub fn convection_vigor(k: f32, full_steps: u32) -> Vec<f32> {
    if full_steps == 0 {
        return Vec::new();
    }
    let raw: Vec<f32> = (0..full_steps).map(|s| temperature_at(k, s)).collect();
    let mean = raw.iter().sum::<f32>() / full_steps as f32;
    if mean <= 0.0 {
        return vec![1.0; full_steps as usize];
    }
    raw.iter().map(|t| t / mean).collect()
}

/// Differentiation **completeness** `0..1` accrued over a molten era of `full_steps`:
/// density sorting proceeds at a rate ∝ how far the melt is above [`T_DIFF_FREEZE`],
/// integrated over the era and clamped. A long/hot (radiogenic-rich) era fully sorts
/// (settle → 1, today's fully-differentiated crust); a fast quench falls short,
/// stranding heavies in the crust. This replaces Epoch 2's old `duration`-fraction.
pub fn differentiation_settle(k: f32, full_steps: u32) -> f64 {
    let mut acc = 0.0f32;
    for s in 0..full_steps {
        // In normalized `0..1` space, so the accrual is scale-independent of the Kelvin values.
        acc += (normalized(temperature_at(k, s)) - normalized(T_DIFF_FREEZE)).max(0.0);
    }
    (DIFF_RATE * acc).clamp(0.0, 1.0) as f64
}

/// Whether the crust has firmed into a **coherent lid** — at least [`LID_SHARE`] of
/// cells at or above [`CRUST_FIRM`]. A precondition for plate tectonics: a world whose
/// crust never coalesces (metal-dominated, or too little differentiation) stays
/// stagnant, whatever its temperature.
pub fn coherent_lid(cells: &[HexState]) -> bool {
    if cells.is_empty() {
        return false;
    }
    let firm = cells
        .iter()
        .filter(|c| c.crust_fraction as f32 >= CRUST_FIRM)
        .count();
    firm as f32 / cells.len() as f32 >= LID_SHARE
}

/// **Tectonics onset**, as a delay in tectonic-era steps: how many steps into the
/// tectonic era pass before the convection flow can grip a rigid lid — i.e. before `T`
/// (continuing to cool from `molten_steps` onward) first falls below [`T_SOLIDUS`],
/// given the lid is already coherent. `Some(0)` = the crust firmed and cooled through
/// the solidus during the molten era, so plates move from the first tectonic step (the
/// Earth-like default). `None` = the onset never arrives within the window (the planet
/// stays molten, or never grows a coherent lid) — a valid stagnant/mushy world, no
/// plate motion at all.
pub fn tectonics_onset_delay(
    k: f32,
    molten_steps: u32,
    tectonic_steps: u32,
    lid_firm: bool,
) -> Option<u32> {
    if !lid_firm {
        return None;
    }
    (0..=tectonic_steps).find(|&i| temperature_at(k, molten_steps + i) < T_SOLIDUS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldstate::Composition;

    /// A planet of `n` identical cells with the given composition — enough for the
    /// global (per-planet) accounting these functions do.
    fn planet(n: usize, comp: Composition) -> Vec<HexState> {
        (0..n).map(|_| HexState::new(comp.clone())).collect()
    }

    #[test]
    fn radiogenic_elements_slow_cooling() {
        // Two planets of equal mass: one silica-only, one with a slug of potassium.
        let plain = planet(4, Composition::from_iter([(14u8, 1000.0)]));
        let hot = planet(
            4,
            Composition::from_iter([(14u8, 900.0), (POTASSIUM, 100.0)]),
        );
        assert!(radiogenic_index(&hot) > radiogenic_index(&plain));
        assert!(
            cooling_k(&hot) < cooling_k(&plain),
            "a radiogenic-rich recipe must cool more slowly ({} vs {})",
            cooling_k(&hot),
            cooling_k(&plain)
        );
        // Uranium is weighted hotter than potassium per unit mass.
        let u = planet(4, Composition::from_iter([(14u8, 900.0), (URANIUM, 100.0)]));
        assert!(radiogenic_index(&u) > radiogenic_index(&hot));
    }

    #[test]
    fn cell_radiogenic_heat_rewards_potassium_and_uranium() {
        let plain = Composition::from_iter([(14u8, 1000.0)]); // silica, inert
        let potas = Composition::from_iter([(14u8, 900.0), (POTASSIUM, 100.0)]);
        let uran = Composition::from_iter([(14u8, 900.0), (URANIUM, 100.0)]);
        assert_eq!(
            cell_radiogenic_heat(&plain),
            0.0,
            "no K/U → no radiogenic heat"
        );
        assert!(cell_radiogenic_heat(&potas) > 0.0);
        // Uranium is weighted 4× potassium per unit mass.
        assert!(cell_radiogenic_heat(&uran) > cell_radiogenic_heat(&potas));
    }

    #[test]
    fn slower_cooling_differentiates_more_fully() {
        // Same era length; the slower cooler (smaller k) stays molten longer → sorts more.
        let full = 20;
        let fast = differentiation_settle(0.12, full);
        let slow = differentiation_settle(0.05, full);
        assert!(
            slow >= fast,
            "slower cooling should differentiate at least as much ({slow} vs {fast})"
        );
        assert!((0.0..=1.0).contains(&slow) && (0.0..=1.0).contains(&fast));
    }

    #[test]
    fn convection_vigor_is_front_loaded_but_mean_preserving() {
        let full = 20;
        let v = convection_vigor(0.06, full);
        assert_eq!(v.len(), full as usize);
        assert!(
            v[0] > v[full as usize - 1],
            "vigor must be fiercer early than late"
        );
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        assert!(
            (mean - 1.0).abs() < 1e-5,
            "vigor must average 1 (same total stir), got {mean}"
        );
    }

    #[test]
    fn coherent_lid_needs_a_firm_majority() {
        let firm = planet(10, Composition::new());
        // No crust anywhere → not coherent.
        assert!(!coherent_lid(&firm));
        // A firm majority → coherent.
        let mut cells = planet(10, Composition::new());
        for (i, c) in cells.iter_mut().enumerate() {
            c.crust_fraction = if i < 6 { 0.9 } else { 0.1 };
        }
        assert!(coherent_lid(&cells));
    }
}
