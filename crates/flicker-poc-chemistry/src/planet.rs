//! [`World`] — the full mutable simulation state — and [`PlanetState`], the cheap
//! global aggregate stages read (spec §7.1). Plus the **conservation harness**
//! (§4.3): the invariant that every gram is here, arrived, or gone, asserted after
//! every stage. Written before any stage, per the spec's first rule.

use std::collections::{BTreeMap, BTreeSet};

use flicker_materials::{ElementId, Tables};
use flicker_worldgrid::Sphere;

use crate::budget::Budget;
use crate::column::Column;
use crate::mantle::MantleField;
use crate::reservoir::Reservoirs;

/// Atomic numbers of the two radiogenic heat sources present in the Prism table.
const K: ElementId = 19;
const U: ElementId = 92;

/// The whole planet: the immutable accretion [`Budget`], the boundary/global
/// [`Reservoirs`], the per-cell interior [`MantleField`] (M1), one [`Column`] per
/// cell, and the topology grid.
pub struct World {
    /// The immutable starting inventory — the right-hand side of the conservation
    /// invariant.
    pub budget: Budget,
    /// Boundary + global reservoirs (core/atmosphere/ocean/delivered/escaped). The
    /// mantle is **not** here at M1 — it is the per-cell [`mantle`](World::mantle)
    /// field.
    pub reservoirs: Reservoirs,
    /// The per-cell mantle interior (temperature, composition, differentiation) —
    /// the substrate the interior stages act on (M1).
    pub mantle: MantleField,
    /// One column per cell, index == cell_id. Empty crust at t=0.
    pub columns: Vec<Column>,
    /// The icosphere topology (92,162 cells at freq 96). Substrate for the per-cell
    /// and neighbour reads the interior stages need.
    pub grid: Sphere,
    /// Model time elapsed, Myr.
    pub tick_myr: f64,
    /// Per-compound element mass-fractions (immutable reference data from the
    /// material catalog) so the World can validate its own mineral ledger against
    /// the element ledger (§4.1) without threading `Tables` through the scheduler.
    /// A lookup table, not conserved state.
    compound_stoich: BTreeMap<u16, Vec<(ElementId, f64)>>,
}

impl World {
    /// Seed an **undifferentiated hot ball** on `grid`: the entire budget spread
    /// homogeneously across the per-cell mantle at a magma-ocean temperature, every
    /// other reservoir empty, every column crust-free (spec §3.1). The core has not
    /// yet sunk out; there is no crust, no air, no sea. `seed` sets the initial
    /// thermal perturbation (the per-run initial condition, §3.5).
    pub fn seed(grid: Sphere, budget: Budget, tables: &Tables, seed: u64) -> Self {
        let mantle = MantleField::seed(&budget, &grid, seed);
        let columns = (0..grid.len() as u32).map(Column::empty).collect();
        // Immutable stoichiometry lookup so the World audits its own mineral ledger
        // against the element ledger (§4.1) with no Tables handle at tick time.
        let compound_stoich = tables
            .compounds()
            .iter()
            .map(|def| (def.id, tables.compound_mass_fractions(def)))
            .collect();
        Self {
            budget,
            reservoirs: Reservoirs::default(),
            mantle,
            columns,
            grid,
            tick_myr: 0.0,
            compound_stoich,
        }
    }

    /// Number of cells / columns.
    pub fn cell_count(&self) -> usize {
        self.columns.len()
    }

    /// Mass of `element` currently on the *present* side of the invariant: every
    /// reservoir + the per-cell mantle + every column stack + `escaped` (§4.3).
    pub fn present_mass(&self, element: ElementId) -> f64 {
        let columns: f64 = self.columns.iter().map(|c| c.element_mass(element)).sum();
        self.reservoirs.conserved_mass(element) + self.mantle.element_mass(element) + columns
    }

    /// Mass of `element` that SHOULD be present: accreted + delivered (§4.3).
    pub fn expected_mass(&self, element: ElementId) -> f64 {
        self.budget.accreted(element) + self.reservoirs.delivered.amount(element)
    }

    /// **The conservation harness (§4.3).** For every tracked element,
    /// `present == expected` to 1e-9 relative — or panic naming the stage and the
    /// offending element. Never disabled: run after every stage in debug/tests and
    /// periodically in release (see [`Scheduler`](crate::scheduler::Scheduler)).
    pub fn audit(&self, after_stage: &str) {
        // Accumulate the present-side mass per element in ONE pass — cheap even at
        // 92k columns (the old per-element × per-column scan was ~30M B-tree lookups
        // a tick, which made the sim crawl once crust layers existed). Each layer's
        // sparse composition is visited once. Also catches a *created* element the
        // budget never held (a leak in the creation direction), since it appears in
        // `present` and is checked against a zero expected.
        let r = &self.reservoirs;
        let mut present: BTreeMap<ElementId, f64> = BTreeMap::new();
        for comp in [&r.core, &r.atmosphere, &r.ocean.contents, &r.escaped] {
            for (e, m) in comp.iter() {
                *present.entry(e).or_insert(0.0) += m;
            }
        }
        for &e in self.mantle.elements() {
            *present.entry(e).or_insert(0.0) += self.mantle.element_mass(e);
        }
        for col in &self.columns {
            for layer in &col.layers {
                for (e, m) in layer.elements.iter() {
                    *present.entry(e).or_insert(0.0) += m;
                }
            }
        }

        // Check every element that appears anywhere — present, budget, or delivered.
        let mut elements: BTreeSet<ElementId> = present.keys().copied().collect();
        elements.extend(self.budget.iter().map(|(e, _)| e));
        elements.extend(r.delivered.iter().map(|(e, _)| e));
        for element in elements {
            let p = present.get(&element).copied().unwrap_or(0.0);
            let expected = self.expected_mass(element);
            let tol = 1e-9 * expected.abs().max(1.0);
            if (p - expected).abs() > tol {
                panic!(
                    "conservation broken after stage '{after_stage}': element {element} present \
                     {p:.6e} kg but budget+delivered = {expected:.6e} kg (Δ {:.3e} kg). \
                     Every gram must be here, arrived, or gone.",
                    p - expected,
                );
            }
        }
    }

    /// **The compound-ledger bound (§4.1).** For every column layer and every
    /// element, the element mass locked inside its minerals must not exceed the
    /// free element mass — compounds are an accounting *of* the element budget, not
    /// new matter. Run every tick alongside [`audit`](World::audit) (the second
    /// conserved-ledger gate). Vacuous at M0 (no minerals formed); the mineral
    /// former (M6) is what it guards. Panics naming the stage on violation.
    pub fn audit_compound_bound(&self, after_stage: &str) {
        for col in &self.columns {
            for layer in &col.layers {
                let mut implied: BTreeMap<ElementId, f64> = BTreeMap::new();
                for (compound_id, mineral_mass) in layer.minerals.iter() {
                    if let Some(fracs) = self.compound_stoich.get(&compound_id) {
                        for &(element, frac) in fracs {
                            *implied.entry(element).or_insert(0.0) += mineral_mass * frac;
                        }
                    }
                }
                for (element, locked) in implied {
                    let free = layer.elements.amount(element);
                    if locked > free + 1e-6 * free.max(1.0) {
                        panic!(
                            "compound bound broken after stage '{after_stage}': cell {} layer \
                             locks {locked:.6e} kg of element {element} in minerals but holds only \
                             {free:.6e} kg free",
                            col.cell_id,
                        );
                    }
                }
            }
        }
    }
}

/// A cheap global aggregate of the world, recomputed at the **top** of each tick
/// from the previous tick's ledger (spec §7.1). Stages read it and never write it;
/// it lags one tick by construction, which makes read/write ordering unambiguous.
/// Fields grow as stages populate the chemistry they summarise; at M0 it is the
/// reservoir-mass partition (temperature / CO₂ / biosphere land with M3–M5).
#[derive(Clone, Debug, Default)]
pub struct PlanetState {
    pub tick_myr: f64,
    pub core_mass_kg: f64,
    pub mantle_mass_kg: f64,
    pub crust_mass_kg: f64,
    pub atmosphere_mass_kg: f64,
    pub ocean_mass_kg: f64,
    /// Mass gone to space, kg — the present-side term that closes the ledger.
    pub escaped_mass_kg: f64,
    /// Mass delivered from the outer system, kg — the extra right-hand-side term.
    pub delivered_mass_kg: f64,
    /// The accreted planet mass, kg (the conservation ledger's baseline).
    pub planet_mass_kg: f64,
    /// Mean mantle temperature, K (M1).
    pub mean_mantle_temp_k: f64,
    /// Mean core-formation progress across cells, 0..1 (M1) — the iron catastrophe.
    pub differentiation_frac: f64,
    /// Total radiogenic heat production, terawatts (M1) — falls as the isotopes
    /// decay, which is why the early planet ran hotter.
    pub radiogenic_power_tw: f64,
    /// Crust mass as a fraction of the planet — a thin skin, grown not seeded (M2).
    pub crust_frac: f64,
    /// Fraction of crust-bearing columns that are continental (M2).
    pub continental_frac: f64,
    /// Mean elevation of crust-bearing columns above the mantle datum, m (M2).
    pub mean_elevation_m: f64,
    /// Mean surface temperature, K (0 until radiative balance lands, M3).
    pub mean_surface_temp_k: f64,
    /// Atmospheric CO₂ partial-pressure proxy (0 until outgassing, M3).
    pub p_co2: f64,
}

impl PlanetState {
    /// Sample the aggregate from the world — the top-of-tick snapshot.
    pub fn sample(world: &World) -> Self {
        use crate::column::{crust_kind, elevation_m, CrustKind};
        let r = &world.reservoirs;
        let m = &world.mantle;
        let crust: f64 = world.columns.iter().map(Column::mass_kg).sum();
        let n = m.n_cells().max(1) as f64;
        let power_w =
            crate::interior::radiogenic_power_w(m.element_mass(U), m.element_mass(K), world.tick_myr);

        // Crust aggregates (M2): how much crust, how much of it is continental, and
        // how high it rides.
        let (mut n_crust, mut n_cont, mut elev_sum) = (0usize, 0usize, 0.0f64);
        for col in &world.columns {
            match crust_kind(col) {
                CrustKind::Undifferentiated => {}
                kind => {
                    n_crust += 1;
                    elev_sum += elevation_m(col);
                    if kind == CrustKind::Continental {
                        n_cont += 1;
                    }
                }
            }
        }

        Self {
            tick_myr: world.tick_myr,
            core_mass_kg: r.core.total(),
            mantle_mass_kg: m.total_mass(),
            crust_mass_kg: crust,
            atmosphere_mass_kg: r.atmosphere.total(),
            ocean_mass_kg: r.ocean.mass_kg(),
            escaped_mass_kg: r.escaped.total(),
            delivered_mass_kg: r.delivered.total(),
            planet_mass_kg: world.budget.total(),
            mean_mantle_temp_k: m.temp_k.iter().sum::<f64>() / n,
            differentiation_frac: m.differentiation.iter().sum::<f64>() / n,
            radiogenic_power_tw: power_w / 1e12,
            crust_frac: crust / world.budget.total().max(1.0),
            continental_frac: if n_crust > 0 { n_cont as f64 / n_crust as f64 } else { 0.0 },
            mean_elevation_m: if n_crust > 0 { elev_sum / n_crust as f64 } else { 0.0 },
            mean_surface_temp_k: 0.0,
            p_co2: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::column::{crust_kind, elevation_m, CrustKind, FormationProcess, Layer};
    use crate::config::content_data_dir;
    use flicker_materials::JsonTableSource;
    use flicker_worldgrid::icosphere;
    use flicker_worldstate::{Composition, CompoundLedger};

    fn tables() -> Tables {
        Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("material tables")
    }

    fn tiny_world() -> World {
        // A small planet (freq 4 = 162 cells) — conservation is size-independent,
        // so the harness is exercised without paying for 92k cells per test.
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget from repo");
        World::seed(icosphere(4), b, &t, 42)
    }

    #[test]
    fn seed_is_undifferentiated_and_balanced() {
        let w = tiny_world();
        // The whole budget is mantle; core/atmosphere/ocean are empty (§3.1).
        assert!(w.reservoirs.core.is_empty(), "core has not differentiated yet");
        assert!(w.reservoirs.atmosphere.is_empty(), "no atmosphere outgassed yet");
        assert_eq!(w.reservoirs.ocean.mass_kg(), 0.0, "no ocean yet");
        assert!(
            w.columns.iter().all(|c| c.layers.is_empty()),
            "crust is an OUTPUT, never seeded"
        );
        // present == expected for every element.
        w.audit("seed");
        // Derived reads (functions, not fields): an empty column is Undifferentiated
        // at sea-floor datum.
        assert_eq!(crust_kind(&w.columns[0]), CrustKind::Undifferentiated);
        assert_eq!(elevation_m(&w.columns[0]), 0.0);
    }

    #[test]
    fn conserving_transfer_holds() {
        let mut w = tiny_world();
        // A differentiation-like move: sink iron from a mantle cell into core. Mass
        // only moves — the ledger stays balanced.
        let moved = w.mantle.remove(0, 26, 1.0e20);
        w.reservoirs.core.add(26, moved);
        assert!(moved > 0.0);
        w.audit("differentiation-like");
    }

    #[test]
    #[should_panic(expected = "conservation broken")]
    fn raw_leak_is_caught() {
        let mut w = tiny_world();
        w.audit("seed"); // balanced
        // Vanish iron into nowhere — no escaped, no delivered. The harness must
        // catch it.
        w.mantle.remove(0, 26, 1.0e18);
        w.audit("leak");
    }

    #[test]
    fn compound_bound_is_vacuous_at_seed() {
        // No minerals formed yet, so the second-ledger gate trivially holds — but
        // it runs (it is not dead code).
        tiny_world().audit_compound_bound("seed");
    }

    #[test]
    #[should_panic(expected = "compound bound broken")]
    fn compound_bound_catches_over_budget_minerals() {
        let t = tables();
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 42);
        let water = t.compound("Water").expect("water in the catalog").id;
        // A layer whose minerals lock ~888 kg of oxygen (1000 kg of H₂O) over only
        // 10 kg of free oxygen — the second conserved ledger's bound must fire.
        let mut minerals = CompoundLedger::new();
        minerals.add(water, 1000.0);
        let mut elements = Composition::new();
        elements.add(8, 10.0); // oxygen
        w.columns[0].layers.push(Layer {
            elements,
            minerals,
            formed_at_myr: 0.0,
            formed_by: FormationProcess::Primordial,
            peak_pt: (0.0, 0.0),
        });
        w.audit_compound_bound("over-budget");
    }

    #[test]
    #[should_panic(expected = "conservation broken")]
    fn creation_of_an_unbudgeted_element_is_caught() {
        let mut w = tiny_world();
        // Element 5 (boron) is not in the Prism table / accretion budget. Spawning
        // it from nothing is a leak in the creation direction — the audit must
        // notice, which means tracking must scan the reservoirs, not just the
        // budget.
        w.reservoirs.core.add(5, 1.0e15);
        w.audit("spontaneous-creation");
    }

    #[test]
    fn delivery_adds_to_both_sides() {
        let mut w = tiny_world();
        // Volatile delivery ADDS mass (§4.2): credit the atmosphere and record it
        // as delivered. Both sides of the invariant rise together, so it balances.
        w.reservoirs.atmosphere.add(1, 5.0e19); // hydrogen arriving
        w.reservoirs.delivered.add(1, 5.0e19);
        w.audit("delivery");
        assert!(w.expected_mass(1) > w.budget.accreted(1), "delivery raised the accounted total");
    }
}
