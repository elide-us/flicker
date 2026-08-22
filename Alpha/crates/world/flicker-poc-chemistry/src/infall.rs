//! **Matter arriving from outside** — the only sense in which the rest of the
//! system still touches this planet (Ruling R1d).
//!
//! The `Delivered` ledger is the right-hand side of the conservation invariant:
//! anything that arrives is credited to it *and* to whatever reservoir received it,
//! so the harness balances while the planet genuinely gains matter it did not
//! accrete. Nothing here conjures anything; it moves the boundary.
//!
//! Two cargoes arrive, and the difference between them is a matter of timing that
//! turns out to decide what the world is worth digging.

use flicker_materials::{ElementId, Tables};

use crate::budget::Budget;
use crate::planet::World;
use crate::stage::{Stage, StageRng};

/// Atomic numbers of the two elements water is made of.
const H: ElementId = 1;
const O: ElementId = 8;

/// Fraction of the remaining water budget delivered per Myr. e-fold ≈ 1 BY —
/// the infall is a tail of late accretion spanning the young planet's first
/// eras, never a hose ("not all of the water is meant to arrive at once" —
/// Aaron, 2026-08-06; the old 250 My e-fold front-loaded the sea).
pub const DEFAULT_WATER_DELIVERY_RATE: f64 = 0.001;

/// **WaterDelivery** — the volatile infall, one of the three inputs at the system
/// boundary.
///
/// Arrives as H₂O by mass, not as a bag of atoms — and **what it lands ON decides
/// what it arrives AS**: the molten share of the surface flashes its share of the
/// infall to steam on contact (a comet cannot leave a puddle on magma), and only
/// the solid share receives standing water. How much of the world the sea floods
/// is decided nowhere near here: [`sea_level_m`](crate::planet::sea_level_m) reads
/// the ocean against the ground the crust happened to make.
pub struct WaterDelivery {
    /// Total water to deliver over the run, kg.
    pub budget_kg: f64,
    /// Fraction of the remaining budget delivered per Myr.
    pub rate: f64,
    /// The coverage cutoff, `0..1`: delivery runs only while the solved
    /// submerged fraction is below this. `1.0` = no cutoff. The dial that makes
    /// island-chain water worlds (high) and drier continents (low) — within the
    /// range the planet's own outgassed steam leaves to govern.
    pub target_coverage: f64,
    /// Water's element mass-fractions (catalog stoichiometry) — the split the
    /// arriving H₂O is booked at, which is what lets the steam share be booked
    /// as the compound it flies as without slackening the compound bound.
    fracs: Vec<(ElementId, f64)>,
}

impl WaterDelivery {
    /// Resolve water's stoichiometry from the catalog; the knobs start at their
    /// Earth-ish defaults (~1.4e21 kg — the scale of Earth's ocean — arriving on
    /// the [`DEFAULT_WATER_DELIVERY_RATE`] tail, no coverage cutoff). Panics if
    /// the catalog moved — same contract as
    /// [`WaterCycle::new`](crate::atmosphere::WaterCycle::new).
    pub fn new(tables: &Tables) -> Self {
        let def = tables
            .compound("Water")
            .expect("water delivery needs 'Water' in compounds.json");
        assert_eq!(
            def.id,
            crate::atmosphere::WATER_VAPOUR,
            "'Water' moved in the catalog: {}",
            def.id
        );
        Self {
            budget_kg: crate::surface::DEFAULT_WATER_KG,
            rate: DEFAULT_WATER_DELIVERY_RATE,
            target_coverage: 1.0,
            fracs: tables.compound_mass_fractions(def),
        }
    }
}

impl Stage for WaterDelivery {
    fn name(&self) -> &'static str {
        "WaterDelivery"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let already = world.reservoirs.delivered.amount(H) + world.reservoirs.delivered.amount(O);
        let left = (self.budget_kg - already).max(0.0);
        if left <= 0.0 {
            return;
        }
        let amount = (left * self.rate * dt_myr).min(left);
        // What the water lands ON decides what it arrives AS. The molten share
        // of the surface flashes its share to steam on contact; only the solid
        // share receives standing water. Same molten measure as the WaterCycle
        // boil, so arrival and boil can never disagree about where a sea may
        // stand — without this split, one tick of infall books a phantom sea
        // onto a magma ocean and every ocean-gated stage downstream believes it.
        let n = world.mantle.n_cells().max(1) as f64;
        let solidus = crate::crust::SOLIDUS_K;
        let molten = world
            .mantle
            .temp_k
            .iter()
            .filter(|&&t| t >= solidus)
            .count() as f64
            / n;
        let steam = amount * molten;
        // Booked at CATALOG stoichiometry — it is water, not a bag of atoms,
        // and the species ledger is bounded against water's real fractions.
        for &(element, frac) in &self.fracs {
            world.reservoirs.delivered.add(element, amount * frac);
            world
                .reservoirs
                .ocean
                .contents
                .add(element, (amount - steam) * frac);
            world
                .reservoirs
                .atmosphere
                .contents
                .add(element, steam * frac);
        }
        if steam > 0.0 {
            // Booked as the compound it flies as — the same booking as
            // `WaterCycle::lift` — so the veil, the greenhouse and the
            // saturation law all see the steam.
            world
                .reservoirs
                .atmosphere
                .species
                .add(crate::atmosphere::WATER_VAPOUR, steam);
        }
    }
}

/// **LateVeneer** — accretion did not stop when the core formed, and the difference
/// matters enormously.
///
/// Iron sinking to the core takes the metals that follow it with it: gold,
/// platinum, nickel, cobalt. Measured on a run, exactly those come out at ~3× bulk
/// while the metals that stay behind reach 20–90× — the fluid can only concentrate
/// what is still there to concentrate, and for the siderophiles there is almost
/// nothing. That is not a bug; it is the correct consequence of differentiation,
/// and it is why a planet whose accretion stopped at core formation has no gold
/// worth digging anywhere in its crust.
///
/// What arrives **after** the core has already separated never gets that chance to
/// sink. It stays in the mantle, joins the melts, and becomes what the hydrothermal
/// system has to work with. On Earth this is the late veneer, and it is where
/// essentially all accessible gold and platinum came from.
///
/// So this stage waits — gated on the planet's own chemistry, never on a tick
/// number — until the core has substantially formed, then delivers a thin, metal-
/// rich shell into the mantle. It states no outcome: whether any of it ever
/// concentrates into something workable is the distillation column's business, and
/// whether the resulting world is worth playing is [`prospect`](crate::prospect)'s.
pub struct LateVeneer {
    /// Total mass to deliver, kg — a **veneer**, tiny against the planet.
    pub budget_kg: f64,
    /// Fraction of the remaining budget delivered per Myr once it starts.
    pub rate: f64,
    /// The elements it carries, with their share of the cargo. Chondritic-ish
    /// proportions read from the catalog at construction, not typed in.
    cargo: Vec<(ElementId, f64)>,
    /// Core formation this far along before any of it arrives — the gate is the
    /// planet's chemistry, never the clock (§7.2).
    pub after_differentiation: f64,
}

/// Core formation this far along before the veneer arrives — the gate number
/// `processes.json` carries, pinned equal by the drift test.
pub(crate) const VENEER_AFTER_DIFFERENTIATION: f64 = 0.6;

impl LateVeneer {
    /// **Late accretion is more of the same material.** The cargo is therefore the
    /// metals in exactly the proportions the planet accreted them — the accretion
    /// budget itself, restricted to the metals a fluid can move.
    ///
    /// That proportionality matters more than it looks. Giving each metal an equal
    /// share of the cargo delivers as much gold as iron, and a run then reads gold
    /// at three hundred thousand times bulk: a world made of it. Chondritic
    /// proportions put gold where gold actually is — vanishingly rare, and worth
    /// something precisely because the fluid has to work over a whole planet to
    /// gather any.
    pub fn new(tables: &Tables, budget: &Budget, budget_kg: f64) -> Self {
        let mut cargo: Vec<(ElementId, f64)> = tables
            .compounds()
            .iter()
            .filter(|c| c.harvestable)
            .filter_map(|c| c.extracted_element.as_deref())
            .filter_map(|sym| tables.element(sym))
            .filter(|e| e.category == "transition_metal")
            .map(|e| (e.number, budget.accreted(e.number)))
            .collect();
        cargo.sort_by_key(|&(e, _)| e);
        cargo.dedup_by_key(|&mut (e, _)| e);
        cargo.retain(|&(_, share)| share > 0.0);
        let total: f64 = cargo
            .iter()
            .map(|&(_, w)| w)
            .sum::<f64>()
            .max(f64::MIN_POSITIVE);
        for slot in cargo.iter_mut() {
            slot.1 /= total;
        }
        Self {
            budget_kg,
            rate: 0.01,
            cargo,
            after_differentiation: VENEER_AFTER_DIFFERENTIATION,
        }
    }
}

impl Stage for LateVeneer {
    fn name(&self) -> &'static str {
        "LateVeneer"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        if self.cargo.is_empty() {
            return;
        }
        let already: f64 = self
            .cargo
            .iter()
            .map(|&(e, _)| world.reservoirs.delivered.amount(e))
            .sum();
        let left = (self.budget_kg - already).max(0.0);
        if left <= 0.0 {
            return;
        }
        let amount = (left * self.rate * dt_myr).min(left);
        // Spread over the whole surface, because that is how it arrives — as
        // infall, not as one impact anybody placed.
        let n = world.mantle.n_cells();
        if n == 0 {
            return;
        }
        for &(element, share) in &self.cargo {
            let mass = amount * share;
            if mass <= 0.0 {
                continue;
            }
            world.reservoirs.delivered.add(element, mass);
            let per_cell = mass / n as f64;
            for cell in 0..n {
                world.mantle.add(cell, element, per_cell);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use flicker_materials::JsonTableSource;
    use flicker_worldgrid::icosphere;

    fn dry_world() -> (World, Tables) {
        let t = Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables");
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let w = World::seed(icosphere(4), b, &t, 3);
        (w, t)
    }

    /// Water is **delivered**, never conjured: the receiving reservoir and the
    /// right-hand side of the conservation ledger are credited together, so the
    /// harness stays balanced while the planet gains water it did not start with.
    /// A fresh world is a magma ocean, so the infall arrives as **steam** — a
    /// comet cannot leave a puddle on molten ground, and without this the first
    /// tick books a phantom sea that opens every ocean-gated stage on lava.
    #[test]
    fn water_arrives_from_outside_and_the_ledger_still_balances() {
        let (mut w, t) = dry_world();
        assert_eq!(w.reservoirs.ocean.mass_kg(), 0.0, "the planet starts dry");
        assert!(
            w.mantle
                .temp_k
                .iter()
                .all(|&t| t >= crate::crust::SOLIDUS_K),
            "a fresh seed is a magma ocean — the premise of the steam routing"
        );
        let stage = WaterDelivery::new(&t);
        let mut rng = crate::stage::StageRng::new(1);
        for _ in 0..50 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("WaterDelivery");
            w.audit_compound_bound("WaterDelivery");
        }
        assert_eq!(w.reservoirs.ocean.mass_kg(), 0.0, "no sea stands on magma");
        let vapour = w
            .reservoirs
            .atmosphere
            .species
            .amount(crate::atmosphere::WATER_VAPOUR);
        assert!(vapour > 0.0, "the infall flashed to steam in the sky");
        // Roughly two hydrogens to an oxygen by mass — it is water at catalog
        // stoichiometry, not a bag of atoms.
        let (h, o) = (
            w.reservoirs.atmosphere.contents.amount(1),
            w.reservoirs.atmosphere.contents.amount(8),
        );
        assert!(
            (o / h - 8.0).abs() < 0.1,
            "delivered H:O is {}:1 by mass",
            o / h
        );
        assert!(
            (vapour - (h + o)).abs() < 1.0,
            "the steam is booked as the compound it flies as"
        );
    }

    /// The other half of the arrival rule: once the ground is solid, the same
    /// infall pools as a standing sea — the ocean fills only over ground that
    /// can hold it.
    #[test]
    fn a_sea_stands_only_on_solid_ground() {
        let (mut w, t) = dry_world();
        for temp in w.mantle.temp_k.iter_mut() {
            *temp = crate::crust::SOLIDUS_K - 100.0;
        }
        let stage = WaterDelivery::new(&t);
        let mut rng = crate::stage::StageRng::new(1);
        for _ in 0..50 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("WaterDelivery");
            w.audit_compound_bound("WaterDelivery");
        }
        assert!(
            w.reservoirs.ocean.mass_kg() > 0.0,
            "a sea arrived on the solid world"
        );
        assert_eq!(
            w.reservoirs
                .atmosphere
                .species
                .amount(crate::atmosphere::WATER_VAPOUR),
            0.0,
            "nothing flashed to steam — no molten ground to flash on"
        );
    }

    /// **Why a planet needs a late veneer to be worth digging.** The metals that
    /// followed iron into the core are gone from the mantle; what arrives after the
    /// core has separated never had the chance to sink, and is all a fluid has left
    /// to concentrate. Delivered conserved, like everything that crosses the
    /// boundary.
    #[test]
    fn the_veneer_arrives_after_the_core_and_the_ledger_balances() {
        let (mut w, t) = dry_world();
        let stage = LateVeneer::new(&t, &w.budget.clone(), 1.0e19);
        let mut rng = crate::stage::StageRng::new(1);
        let gold = t.element("Au").expect("gold is in the table").number;
        let before = w.mantle.element_mass(gold);
        for _ in 0..40 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("LateVeneer");
        }
        assert!(
            w.mantle.element_mass(gold) > before,
            "the veneer put metal into the mantle"
        );
    }

    /// It waits on the planet's chemistry, never on the clock: a veneer means
    /// nothing until there is a core for it to have arrived after.
    #[test]
    fn the_veneer_waits_for_a_core_to_exist() {
        let (w, _t) = dry_world();
        let state = crate::planet::PlanetState::sample(&w);
        assert_eq!(
            state.differentiation_frac, 0.0,
            "nothing has differentiated at t=0"
        );
        assert!(
            !crate::process_file::gate_of("LateVeneer").holds(&state, &crate::Levers::default()),
            "so the veneer has not arrived"
        );
    }
}
