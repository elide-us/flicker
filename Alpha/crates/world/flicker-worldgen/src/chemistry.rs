//! **The compound former** (material-pipeline stage 2). Turns a cell's *elements*
//! into *compounds* — the real chemistry the sim had been faking — filling the
//! second ledger ([`flicker_worldstate::CompoundLedger`], [`HexState::compounds`]).
//!
//! It is **epoch-aligned**: each natural compound forms at the epoch whose
//! conditions suit it — oxides/silicates in the molten bulk (early), salts and
//! delivered water at the hydrosphere, sulfide ores at the hydrothermal veins,
//! carbonates/organics with warm shallow life — driven by that epoch's output
//! conditions on the cell (temperature, water, hydrothermal signature, biomass).
//! The rules are category-general (derived from each compound's formula, not
//! hand-coded per compound), so **all** natural compounds are covered at once.
//!
//! **Conservation.** The element ledger [`HexState::composition`] stays the
//! conserved truth; formation never depletes it. A compound of mass `m` locks
//! `fractionₑ·m` of each constituent element, and the former caps every compound at
//! the *free* element mass (`composition − already-locked`), so the element mass
//! bound up in compounds can never exceed what the cell holds. The one thing that
//! adds mass is **water delivery**: most H₂O arrives from the outer system, so at
//! the hydrosphere it is added to *both* the element ledger (its H/O) and the
//! compound ledger (H₂O) — additive, and tracked.

use std::collections::HashMap;

use flicker_materials::{CompoundDef, ElementId, Tables};
use serde::{Deserialize, Serialize};

use crate::state::HexState;

// Atomic numbers the classifier keys on.
const H: ElementId = 1;
const C: ElementId = 6;
const N: ElementId = 7;
const O: ElementId = 8;
const NA: ElementId = 11;
const S: ElementId = 16;
const CL: ElementId = 17;
const K: ElementId = 19;
const CA: ElementId = 20;

/// Density (g/cm³) at/above which an element counts as a "metal" for sulfide/ore
/// classification — matches the differentiation cutoff.
const METAL_DENSITY: f64 = 4.5;

/// Levers for the compound former (all tunable; part of the generator's knob set).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChemistryParams {
    /// H₂O mass delivered to each cell from the outer system at the hydrosphere
    /// epoch — the additive water source (most water is delivered, not in-situ).
    pub water_delivery: f64,
    /// Fraction of the stoichiometric maximum a compound forms in its pass
    /// (`0..1`) — leaves free elements for the other compounds and later epochs.
    pub formation_rate: f64,
}

impl Default for ChemistryParams {
    fn default() -> Self {
        Self {
            water_delivery: 1200.0,
            formation_rate: 0.5,
        }
    }
}

/// How a compound forms — its chemistry family, the epoch it forms at, and a
/// within-epoch priority (lower forms first, claiming shared elements like O).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    Oxide,     // silica / metal oxides — the molten bulk (E2)
    Water,     // H₂O — delivered at the hydrosphere (E4)
    Salt,      // Na/K chlorides — evaporation (E4)
    Sulfide,   // metal sulfides — hydrothermal ores (E5)
    Native,    // single-element concentrations — veins (E5)
    Carbonate, // Ca–C–O — warm shallow seas (E6)
    Nitrate,   // N-bearing — biotic (E6)
    Organic,   // biological C/H/O (E6)
    Misc,      // anything else — where its elements sit (E5)
}

impl Class {
    fn epoch(self) -> u8 {
        match self {
            Class::Oxide => 2,
            Class::Water | Class::Salt => 4,
            Class::Sulfide | Class::Native | Class::Misc => 5,
            Class::Carbonate | Class::Nitrate | Class::Organic => 6,
        }
    }
    /// Within-epoch order (lower first).
    fn priority(self) -> u8 {
        match self {
            Class::Oxide => 0,
            Class::Water => 0,
            Class::Salt => 1,
            Class::Sulfide => 0,
            Class::Native => 1,
            Class::Misc => 2,
            Class::Carbonate => 0,
            Class::Nitrate => 1,
            Class::Organic => 2,
        }
    }
}

/// A precompiled formation entry for one natural compound.
struct Formable {
    id: u16,
    class: Class,
    /// Per-element mass fractions (Σ = 1), keyed by atomic number.
    fractions: Vec<(ElementId, f64)>,
}

/// The compiled plan: every natural compound, sorted so a `form_for_epoch` pass is
/// a simple filter. Built once from the vocabulary.
pub struct FormerPlan {
    entries: Vec<Formable>,
    water_id: Option<u16>,
    water_h: f64,
    water_o: f64,
}

impl FormerPlan {
    /// Compile the plan from the loaded vocabulary.
    pub fn compile(tables: &Tables) -> Self {
        let mut entries = Vec::new();
        let mut water_id = None;
        let (mut water_h, mut water_o) = (0.0, 0.0);
        for c in tables.compounds() {
            if !c.natural {
                continue; // crafted alloys don't form in the world
            }
            let fractions = tables.compound_mass_fractions(c);
            if fractions.is_empty() {
                continue;
            }
            let class = classify(c, &fractions, tables);
            if class == Class::Water {
                water_id = Some(c.id);
                water_h = frac(&fractions, H);
                water_o = frac(&fractions, O);
            }
            entries.push(Formable {
                id: c.id,
                class,
                fractions,
            });
        }
        // Stable order: by epoch then priority, so a pass forms in the right order.
        entries.sort_by_key(|e| (e.class.epoch(), e.class.priority(), e.id));
        Self {
            entries,
            water_id,
            water_h,
            water_o,
        }
    }

    /// Run the compounds that form at `epoch` over every cell, in place.
    pub fn form_for_epoch(&self, cells: &mut [HexState], epoch: u8, params: &ChemistryParams) {
        // Additive water delivery lands at the hydrosphere epoch.
        if epoch == 4 && params.water_delivery > 0.0 {
            if let Some(wid) = self.water_id {
                for cell in cells.iter_mut() {
                    let m = params.water_delivery;
                    // The delivered water's elements are new mass on the planet.
                    cell.composition.add(H, self.water_h * m);
                    cell.composition.add(O, self.water_o * m);
                    cell.compounds.add(wid, m);
                }
            }
        }

        let rate = params.formation_rate.clamp(0.0, 1.0);
        for cell in cells.iter_mut() {
            // Free element mass = ledger − what is already locked in compounds.
            let mut free = self.free_elements(cell);
            for e in self
                .entries
                .iter()
                .filter(|e| e.class.epoch() == epoch && e.class != Class::Water)
            {
                let cond = condition(e.class, cell);
                if cond <= 0.0 {
                    continue;
                }
                // Stoichiometric ceiling: the least-available constituent limits it.
                let max_form =
                    formable_mass(|el| free.get(&el).copied().unwrap_or(0.0), &e.fractions);
                if max_form <= 0.0 {
                    continue;
                }
                let form = max_form * rate * cond;
                if form <= 0.0 {
                    continue;
                }
                cell.compounds.add(e.id, form);
                for &(el, fr) in &e.fractions {
                    *free.entry(el).or_insert(0.0) -= fr * form;
                }
            }
        }
    }

    /// Free element mass in a cell = element ledger − mass locked in its compounds.
    fn free_elements(&self, cell: &HexState) -> HashMap<ElementId, f64> {
        let mut free: HashMap<ElementId, f64> = cell.composition.iter().collect();
        for (cid, amount) in cell.compounds.iter() {
            if let Some(e) = self.entries.iter().find(|e| e.id == cid) {
                for &(el, fr) in &e.fractions {
                    *free.entry(el).or_insert(0.0) -= fr * amount;
                }
            }
        }
        free
    }
}

/// Mass fraction of element `n` in a fractions list (`0` if absent).
fn frac(fractions: &[(ElementId, f64)], n: ElementId) -> f64 {
    fractions
        .iter()
        .find(|&&(e, _)| e == n)
        .map(|&(_, f)| f)
        .unwrap_or(0.0)
}

/// The maximum mass of a compound formable from the available element masses (`avail(el)`),
/// given its per-element mass fractions (Σ = 1): the least-available constituent sets the
/// ceiling (`0` if any needed element is absent). The **shared stoichiometric primitive** used
/// by both the epoch compound former ([`FormerPlan::form_for_epoch`]) and the tick-sim
/// outgassing (`crate::outgas`) — one source for "how much of this compound can these elements
/// make", so the two never drift.
pub fn formable_mass(avail: impl Fn(ElementId) -> f64, fractions: &[(ElementId, f64)]) -> f64 {
    let mut max_form = f64::INFINITY;
    for &(el, fr) in fractions {
        if fr > 0.0 {
            max_form = max_form.min(avail(el) / fr);
        }
    }
    if max_form.is_finite() {
        max_form.max(0.0)
    } else {
        0.0
    }
}

/// The delivered-water element split from the vocabulary's canonical **Water** compound:
/// `(compound id, H mass-fraction, O mass-fraction)`, read from the compound's real
/// stoichiometry + the periodic-table atomic masses. The **single source** both the compound
/// former ([`FormerPlan`]) and the tick-sim hydrosphere (`grow_hydrosphere`) split a delivered
/// water mass by, so no atomic mass is ever hardcoded (canon values stay unanimous). `None`
/// if the table carries no Water compound.
pub fn water_element_split(tables: &Tables) -> Option<(u16, f64, f64)> {
    let water = tables.compound("Water")?;
    let fractions = tables.compound_mass_fractions(water);
    Some((water.id, frac(&fractions, H), frac(&fractions, O)))
}

/// Classify a compound into its formation family from its elements.
fn classify(c: &CompoundDef, fractions: &[(ElementId, f64)], tables: &Tables) -> Class {
    let has = |n: ElementId| fractions.iter().any(|&(e, _)| e == n);
    let has_metal = c.elements.iter().any(|e| {
        tables
            .element(&e.symbol)
            .map(|el| el.density_g_cm3 as f64 >= METAL_DENSITY)
            .unwrap_or(false)
    });

    if c.category == "biological" {
        Class::Organic
    } else if fractions.len() == 2 && has(H) && has(O) && !has(C) {
        Class::Water
    } else if has(S) && has_metal {
        Class::Sulfide
    } else if has(CL) && (has(NA) || has(K)) {
        Class::Salt
    } else if has(C) && has(CA) && has(O) {
        Class::Carbonate
    } else if has(N) {
        Class::Nitrate
    } else if has(O) {
        Class::Oxide
    } else if fractions.len() == 1 {
        Class::Native
    } else {
        Class::Misc
    }
}

/// The `0..1` condition factor gating how readily a class forms in a cell, from the
/// epoch's output state. Rough v1 rules — all tunable.
fn condition(class: Class, cell: &HexState) -> f64 {
    let temp = cell.temperature as f64;
    let biomass = cell.biomass as f64;
    let warm = (temp / 30.0).clamp(0.0, 1.0);
    let submerged = cell.water_depth > 0.0;
    match class {
        // Silicates/oxides are the molten bulk — they form everywhere.
        Class::Oxide => 1.0,
        // Salt precipitates as warm water evaporates on the shelves.
        Class::Salt => {
            if submerged || cell.precipitation > 0.0 {
                (0.1 + 0.9 * warm).clamp(0.0, 1.0)
            } else {
                0.05
            }
        }
        // Ore sulfides ride the hydrothermal plumbing (a trace forms off-vent).
        Class::Sulfide => (0.05 + cell.hydrothermal as f64).clamp(0.0, 1.0),
        // Native concentrations track the veins.
        Class::Native => (0.1 + cell.vein_strength as f64).clamp(0.0, 1.0),
        // Carbonates need warm, shallow, living seas.
        Class::Carbonate => {
            let shallow = cell.water_depth > 0.0 && cell.water_depth < 0.6;
            if shallow {
                ((temp - 5.0) / 25.0).clamp(0.0, 1.0) * (0.3 + biomass).min(1.0)
            } else {
                0.0
            }
        }
        Class::Nitrate => (0.2 + biomass).clamp(0.0, 1.0),
        Class::Organic => (0.05 + biomass).clamp(0.0, 1.0),
        Class::Water => 0.0, // delivered, not condition-formed
        Class::Misc => 0.5,
    }
}

/// Total element mass locked inside a cell's compounds — the accounting check that
/// it never exceeds the element ledger.
pub fn locked_element_mass(cell: &HexState, tables: &Tables) -> HashMap<ElementId, f64> {
    let mut locked: HashMap<ElementId, f64> = HashMap::new();
    for (cid, amount) in cell.compounds.iter() {
        if let Some(cd) = tables.compound_by_id(cid) {
            for (el, fr) in tables.compound_mass_fractions(cd) {
                *locked.entry(el).or_insert(0.0) += fr * amount;
            }
        }
    }
    locked
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::JsonTableSource;
    use flicker_worldstate::Composition;

    fn tables() -> Tables {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// One warm, submerged, mineralized, living cell — conditions for many classes.
    fn rich_cell() -> HexState {
        let mut s = HexState::new(Composition::from_iter([
            (O, 5000.0),
            (14, 2000.0),
            (26, 800.0),
            (CA, 400.0),
            (C, 200.0),
            (S, 150.0),
            (NA, 300.0),
            (CL, 300.0),
            (H, 100.0),
        ]));
        s.temperature = 24.0;
        s.water_depth = 0.3;
        s.hydrothermal = 0.7;
        s.vein_strength = 0.5;
        s.biomass = 0.6;
        s
    }

    #[test]
    fn forms_compounds_without_exceeding_the_element_ledger() {
        let t = tables();
        let plan = FormerPlan::compile(&t);
        let params = ChemistryParams::default();
        let mut cells = vec![rich_cell()];
        for epoch in 2..=6 {
            plan.form_for_epoch(&mut cells, epoch, &params);
        }
        let cell = &cells[0];
        assert!(!cell.compounds.is_empty(), "no compounds formed");
        // The locked element mass never exceeds the ledger, for every element.
        let locked = locked_element_mass(cell, &t);
        for (el, m) in &locked {
            let have = cell.composition.amount(*el);
            assert!(
                *m <= have + 1e-6,
                "element {el}: locked {m} exceeds ledger {have}"
            );
        }
    }

    #[test]
    fn water_delivery_is_additive_and_conserved_into_the_compound() {
        let t = tables();
        let plan = FormerPlan::compile(&t);
        let params = ChemistryParams::default();
        let mut cells = vec![HexState::new(Composition::from_iter([(O, 100.0)]))];
        let before_o = cells[0].composition.amount(O);
        let before_h = cells[0].composition.amount(H);
        plan.form_for_epoch(&mut cells, 4, &params);
        let wid = t.compound("Water").unwrap().id;
        let water = cells[0].compounds.amount(wid);
        assert!(water > 0.0, "no water delivered");
        // The delivered water added its H and O to the element ledger (additive).
        assert!(cells[0].composition.amount(O) > before_o);
        assert!(cells[0].composition.amount(H) > before_h);
        // And the compound's element content matches the ledger growth.
        let locked = locked_element_mass(&cells[0], &t);
        for (el, m) in &locked {
            assert!(*m <= cells[0].composition.amount(*el) + 1e-6);
        }
    }
}
