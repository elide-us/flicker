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
    /// How far life has got — the one piece of biosphere state, and the reason
    /// a planet has coal. Advance-only (see [`LifeStage`](crate::biosphere::LifeStage));
    /// everything life *does* is booked in the same conserved ledgers as the rest.
    pub life: crate::biosphere::LifeStage,
    /// Per-compound element mass-fractions (immutable reference data from the
    /// material catalog) so the World can validate its own mineral ledger against
    /// the element ledger (§4.1) without threading `Tables` through the scheduler.
    /// A lookup table, not conserved state.
    compound_stoich: BTreeMap<u16, Vec<(ElementId, f64)>>,
}

impl World {
    /// Area of one of this world's cells, m² — read from the grid it was built on
    /// ([`cell_area_m2`](crate::config::cell_area_m2)), so a coarse test planet is
    /// a coarse planet rather than a silently 16×-too-thick one. Equal-area makes
    /// the single number exact for every cell.
    pub fn cell_area_m2(&self) -> f64 {
        crate::config::cell_area_m2(self.grid.len())
    }

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
            life: crate::biosphere::LifeStage::Barren,
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
        for comp in [&r.core, &r.atmosphere.contents, &r.ocean.contents, &r.escaped] {
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
    /// new matter. The **air's species ledger** answers to the same bound: every
    /// booked gas must be backed by the elements actually in the atmosphere. Run
    /// every tick alongside [`audit`](World::audit) (the second conserved-ledger
    /// gate). Panics naming the stage on violation.
    pub fn audit_compound_bound(&self, after_stage: &str) {
        let air = &self.reservoirs.atmosphere;
        let mut implied: BTreeMap<ElementId, f64> = BTreeMap::new();
        for (compound_id, gas_mass) in air.species.iter() {
            if let Some(fracs) = self.compound_stoich.get(&compound_id) {
                for &(element, frac) in fracs {
                    *implied.entry(element).or_insert(0.0) += gas_mass * frac;
                }
            }
        }
        for (element, locked) in implied {
            let free = air.contents.amount(element);
            if locked > free + 1e-6 * free.max(1.0) {
                panic!(
                    "compound bound broken after stage '{after_stage}': the air books \
                     {locked:.6e} kg of element {element} in gas species but holds only \
                     {free:.6e} kg free"
                );
            }
        }
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
/// **Freeze a lid over the whole world** — a test fixture for "this planet has
/// cooled", CONSERVED: the crust comes out of the mantle beneath it, exactly as
/// [`CrustGeneration`](crate::crust::CrustGeneration) would have made it.
///
/// Cooling a test world by writing `mantle.temp_k` alone does NOT do this, and
/// the difference is load-bearing: bare ground reads the interior's own heat
/// ([`cell_surface_temp_k`](crate::surface::cell_surface_temp_k)), so a world
/// with no crust has a molten surface however cold you set its mantle. A world
/// that has "cooled" in the sense any downstream read cares about is one that
/// has grown a skin.
#[cfg(test)]
pub(crate) fn freeze_lid(world: &mut World) {
    let at = world.tick_myr;
    for cell in 0..world.columns.len() {
        let mut melt = Vec::new();
        for e in [14u8, 8, 12, 26] {
            let got = world.mantle.remove(cell, e, 5.0e17);
            if got > 0.0 {
                melt.push((e, got));
            }
        }
        if !melt.is_empty() {
            world.columns[cell].deposit(crate::column::FormationProcess::OceanicCrust, at, &melt);
        }
    }
}

/// Atmospheric CO₂ partial pressure, Pa — the weight of the booked CO₂ spread
/// over the surface (`m·g/A`). The ONE pressure read, shared by
/// [`PlanetState::sample`] and the habitability observer's pH proxy so the two
/// can never disagree about the sky.
pub fn p_co2_pa(world: &World) -> f64 {
    world.reservoirs.atmosphere.species.amount(crate::atmosphere::CARBON_DIOXIDE)
        * crate::column::GRAVITY_M_S2
        / (world.cell_area_m2() * world.columns.len().max(1) as f64)
}

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
    /// The COLDEST mantle cell, K. A mean cannot answer "has anywhere cooled
    /// enough yet" — the first crust freezes over the coldest cell on the
    /// planet while the average is still magma — so any gate about a threshold
    /// something can *locally* cross reads this instead.
    pub min_mantle_temp_k: f64,
    /// The HOTTEST mantle cell, K — the other end of the same argument. A stage
    /// whose tick acts on cells above a threshold is still doing real work
    /// while a single plume stands above it, however cold the average has got;
    /// gating such a stage on the mean silently stops it early. Volcanism is
    /// the case that makes this obvious, since a plume is BY DEFINITION hotter
    /// than the world around it.
    pub max_mantle_temp_k: f64,
    /// Water delivered so far, kg. The infall's own progress against its
    /// budget, separable from [`delivered_mass_kg`](Self::delivered_mass_kg)
    /// (which also carries the late veneer's metals) because water arrives as
    /// H and O and the veneer does not.
    pub delivered_water_kg: f64,
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
    /// Fraction of the world's columns carrying ANY crust — how much of the
    /// planet has a solid surface at all. Distinct from
    /// [`crust_frac`](Self::crust_frac), which is crust MASS against the whole
    /// planet and is a thin skin's worth even when the lid is total. This is
    /// the read that answers "is there ground here yet", so it is what the
    /// surface temperature blends on and what life needs before it can begin.
    pub lid_frac: f64,
    /// Atmospheric CO₂ partial pressure, Pa — the booked CO₂'s weight over the
    /// surface (`m·g/A`). Zero until the mantle exhales it.
    pub p_co2: f64,
    /// Water vapour aloft, kg — the booked steam in the sky's species ledger.
    /// The read the water-cycle gate needs: rain is possible work only when
    /// there is vapour to fall AND ground to land on.
    pub water_vapour_kg: f64,
    /// Mass bound into COMPOUNDS anywhere the species ledgers book them, kg —
    /// gases flown as real molecules plus minerals organised in beds. The
    /// elements→compounds progress read: raw element mass becoming named
    /// chemistry as the world matures.
    pub compounds_kg: f64,
    /// The most abundant gas in the air, by catalog id — what kind of atmosphere
    /// this is (steam, carbon hotbox, temperate nitrogen…). `None` while airless.
    pub dominant_gas: Option<u16>,
    /// What the air holds in, K above the bare stellar sum — a read of the
    /// **species** ledger, so a thick transparent air warms nothing.
    pub greenhouse_k: f64,
    /// Sea level, m on the same datum as [`elevation_m`](crate::column::elevation_m)
    /// — **solved, never set**: see [`sea_level_m`]. Sits at the lowest column
    /// while the ocean is empty, and rises as water arrives.
    pub sea_level_m: f64,
    /// Fraction of columns standing below sea level. A read of the hypsometry
    /// against the water actually present — never a target.
    pub submerged_frac: f64,
    /// Mean number of beds in a crust-bearing column — how much history the stacks
    /// are carrying. Rises as strata accumulate, falls when burial merges them.
    pub mean_strata: f64,
    /// The deepest stack's bed count — the one the soft cap acts on first.
    pub max_strata: usize,
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
        let area = world.cell_area_m2();
        let (mut n_crust, mut n_cont, mut elev_sum) = (0usize, 0usize, 0.0f64);
        let (mut strata_sum, mut max_strata) = (0usize, 0usize);
        for col in &world.columns {
            strata_sum += col.layers.len();
            max_strata = max_strata.max(col.layers.len());
            match crust_kind(col) {
                CrustKind::Undifferentiated => {}
                kind => {
                    n_crust += 1;
                    elev_sum += elevation_m(col, area);
                    if kind == CrustKind::Continental {
                        n_cont += 1;
                    }
                }
            }
        }
        let sea = sea_level_m(world);
        let submerged = world
            .columns
            .iter()
            .filter(|c| elevation_m(c, area) < sea)
            .count();

        Self {
            tick_myr: world.tick_myr,
            core_mass_kg: r.core.total(),
            mantle_mass_kg: m.total_mass(),
            crust_mass_kg: crust,
            atmosphere_mass_kg: r.atmosphere.mass_kg(),
            ocean_mass_kg: r.ocean.mass_kg(),
            escaped_mass_kg: r.escaped.total(),
            delivered_mass_kg: r.delivered.total(),
            planet_mass_kg: world.budget.total(),
            mean_mantle_temp_k: m.temp_k.iter().sum::<f64>() / n,
            min_mantle_temp_k: m.temp_k.iter().copied().fold(f64::INFINITY, f64::min),
            max_mantle_temp_k: m.temp_k.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            delivered_water_kg: r.delivered.amount(1) + r.delivered.amount(8),
            differentiation_frac: m.differentiation.iter().sum::<f64>() / n,
            radiogenic_power_tw: power_w / 1e12,
            crust_frac: crust / world.budget.total().max(1.0),
            continental_frac: if n_crust > 0 { n_cont as f64 / n_crust as f64 } else { 0.0 },
            mean_elevation_m: if n_crust > 0 { elev_sum / n_crust as f64 } else { 0.0 },
            lid_frac: n_crust as f64 / world.columns.len().max(1) as f64,
            p_co2: p_co2_pa(world),
            water_vapour_kg: r.atmosphere.species.amount(crate::atmosphere::WATER_VAPOUR),
            compounds_kg: r.atmosphere.species.total()
                + world
                    .columns
                    .iter()
                    .flat_map(|c| c.layers.iter())
                    .map(|b| b.minerals.total())
                    .sum::<f64>(),
            dominant_gas: r
                .atmosphere
                .species
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| id),
            greenhouse_k: crate::surface::greenhouse_k(world),
            sea_level_m: sea,
            submerged_frac: submerged as f64 / world.columns.len().max(1) as f64,
            mean_strata: strata_sum as f64 / world.columns.len().max(1) as f64,
            max_strata,
        }
    }
}

/// **Solve** for sea level, m on the elevation datum — the level at which the
/// water the planet actually holds exactly fills the basins the crust actually
/// made.
///
/// Never a set number and never a fraction of the surface: it is a root-find on
/// the hypsometry the columns produced. Raise a trial level, sum the volume it
/// would flood, and find where that equals the ocean reservoir's volume. So a
/// world that outgasses more water floods more of itself, a world that loses its
/// ocean to space drains, and thickened continental crust rides above the result
/// on its own — none of which any rule states.
///
/// An **empty ocean floods nothing**, so the level rests at the lowest column: a
/// dry planet is a legal answer, and this read comes alive the moment water does.
pub fn sea_level_m(world: &World) -> f64 {
    use crate::column::elevation_m;

    let area = world.cell_area_m2();
    let mut elevations: Vec<f64> = world.columns.iter().map(|c| elevation_m(c, area)).collect();
    if elevations.is_empty() {
        return 0.0;
    }
    elevations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let floor = elevations[0];
    // Water volume from the conserved ocean ledger, at liquid-water density.
    let volume = world.reservoirs.ocean.mass_kg() / WATER_DENSITY;
    if volume <= 0.0 {
        return floor;
    }
    // Flooded volume rises monotonically with the level, so bisect it.
    let flooded = |level: f64| -> f64 {
        elevations
            .iter()
            .take_while(|&&e| e < level)
            .map(|&e| (level - e) * area)
            .sum()
    };
    let (mut lo, mut hi) = (floor, elevations[elevations.len() - 1]);
    // If even drowning the highest column is not enough, the whole world is under.
    if flooded(hi) < volume {
        return hi + (volume - flooded(hi)) / (elevations.len() as f64 * area);
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if flooded(mid) < volume {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Liquid water density, kg/m³ — turns the conserved ocean mass into the volume
/// the sea-level solve fills basins with.
const WATER_DENSITY: f64 = 1000.0;

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


    /// Give a column a bed of `mass` kg of `element`, so its elevation is nonzero.
    /// Drawn OUT of that cell's mantle, because the harness is right: a fixture
    /// that conjures mass is a fixture that is testing a world that cannot exist.
    fn stack(w: &mut World, cell: usize, element: ElementId, mass: f64) {
        let taken = w.mantle.remove(cell, element, mass);
        let mut elements = Composition::new();
        elements.add(element, taken);
        w.columns[cell].layers.push(Layer {
            elements,
            minerals: CompoundLedger::new(),
            formed_at_myr: 0.0,
            formed_by: FormationProcess::OceanicCrust,
            peak_pt: (0.0, 0.0),
            densified: 0.0,
        });
    }

    /// A dry planet is a legal answer. With no water there is nothing to pond, so
    /// the level rests on the lowest ground and nothing is submerged.
    #[test]
    fn an_empty_ocean_floods_nothing() {
        let mut w = tiny_world();
        let area = w.cell_area_m2();
        for cell in 0..8 {
            stack(&mut w, cell, 14, 1.0e18 * (cell + 1) as f64);
        }
        assert_eq!(w.reservoirs.ocean.mass_kg(), 0.0);
        let sea = sea_level_m(&w);
        let lowest = w
            .columns
            .iter()
            .map(|c| elevation_m(c, area))
            .fold(f64::MAX, f64::min);
        assert!((sea - lowest).abs() < 1e-9, "sea {sea} rests on the lowest ground {lowest}");
        assert!(w.columns.iter().all(|c| elevation_m(c, area) >= sea));
    }

    /// **The sea-level solve.** Whatever water the planet holds is exactly the
    /// water the level ponds — so the level is a consequence of the ocean ledger
    /// and the hypsometry the crust made, never a number anybody set.
    ///
    /// Run backwards to avoid a magic number: pick a level inside the terrain, ask
    /// how much water would stand at it, deliver exactly that, and require the
    /// solve to find its way back.
    #[test]
    fn sea_level_ponds_exactly_the_water_present() {
        let mut w = tiny_world();
        let area = w.cell_area_m2();
        // A varied hypsometry so there is something to flood into.
        for cell in 0..w.columns.len() {
            stack(&mut w, cell, 14, 1.0e17 * (1 + cell % 7) as f64);
        }
        w.audit("test fixture");

        let elevations: Vec<f64> = w.columns.iter().map(|c| elevation_m(c, area)).collect();
        let (lo, hi) = elevations.iter().fold((f64::MAX, f64::MIN), |(a, b), &e| (a.min(e), b.max(e)));
        let want_level = 0.5 * (lo + hi);
        let want_volume: f64 = elevations.iter().map(|&e| (want_level - e).max(0.0) * area).sum();
        assert!(want_volume > 0.0, "the fixture has to have basins to flood");

        // Water arrives (a delivery, so the conservation ledger stays balanced).
        let water = want_volume * 1000.0;
        w.reservoirs.ocean.contents.add(8, water);
        w.reservoirs.delivered.add(8, water);
        w.audit("test delivery");

        let sea = sea_level_m(&w);
        assert!(
            (sea - want_level).abs() < 1e-6 * (hi - lo),
            "the solve found {sea} m for water that stands at {want_level} m"
        );
        let ponded: f64 = elevations.iter().map(|&e| (sea - e).max(0.0) * area).sum();
        assert!(
            (ponded - want_volume).abs() < 1e-6 * want_volume,
            "ponded {ponded:.6e} m³ vs the {want_volume:.6e} m³ actually present"
        );
        // And the aggregate read agrees: some ground under, some standing clear.
        let st = PlanetState::sample(&w);
        assert!(st.submerged_frac > 0.0 && st.submerged_frac < 1.0);
        assert!((st.sea_level_m - sea).abs() < 1e-9);
    }

    /// More water floods more of the world. The direction is forced by the solve,
    /// not by a rule about how much ocean a planet ought to have.
    #[test]
    fn more_water_stands_higher() {
        let mut w = tiny_world();
        for cell in 0..w.columns.len() {
            stack(&mut w, cell, 14, 1.0e17 * (1 + cell % 7) as f64);
        }
        w.reservoirs.ocean.contents.add(8, 1.0e19);
        let low = sea_level_m(&w);
        w.reservoirs.ocean.contents.add(8, 9.0e19);
        let high = sea_level_m(&w);
        assert!(high > low, "sea level rose with the water: {low} → {high}");
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
        assert_eq!(elevation_m(&w.columns[0], w.cell_area_m2()), 0.0);
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
            densified: 0.0,
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
        w.reservoirs.atmosphere.contents.add(1, 5.0e19); // hydrogen arriving
        w.reservoirs.delivered.add(1, 5.0e19);
        w.audit("delivery");
        assert!(w.expected_mass(1) > w.budget.accreted(1), "delivery raised the accounted total");
    }
}
