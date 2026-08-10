//! **The distillation column** — circulating fluid taking metal out of a lot of
//! rock and putting it down in a little.
//!
//! # Why the tectonic cycle alone is not enough
//!
//! Subduction and arc magmatism already fractionate: what sinks is refined on the
//! way back up, so incompatible elements climb in the crust each pass. Measured
//! over 800 Myr that is real and it is the right direction — but it is a **2×**
//! effect, and the metals that matter are worse off than that. Most of a planet's
//! gold, copper and silver followed iron into the core during differentiation, so
//! there is almost none left in the mantle for the melt to refine.
//!
//! Ore is not made by chemistry alone. It is made by **volume ratio**: hot water
//! circulating through a large body of rock dissolves a trace of metal from all of
//! it and drops that metal where conditions change, in a body a thousand times
//! smaller. That is the step that turns a fraction of a gram per tonne into
//! something worth digging, and it is the step this module is.
//!
//! # The rules
//!
//! - **Heat drives circulation.** Hot crust convects the water in it; cold crust
//!   does not. Nothing marks a "hydrothermal zone" — the temperature field says
//!   where fluid moves, and the tectonics say where the temperature is.
//! - **The fluid carries what it can carry.** Which elements travel is read from
//!   the compound catalog — the elements of the minerals flagged harvestable —
//!   never a list typed into this file. A catalog that adds an ore adds it here.
//! - **It flows down the thermal gradient and drops its load as it cools.** So
//!   metal migrates away from the hot rock it was leached from and accumulates
//!   where the fluid dies. Repeated over deep time, many cells give up a trace and
//!   a few gain a great deal.
//! - **Nothing is placed.** There is no vein object, no seeded node, no ore
//!   spawner. A "vein" is a stratum that ended up with a lot of something in it —
//!   which is exactly what Ruling R1b says a vein is.
//!
//! # Prospecting is not simulation
//!
//! [`prospect`] reads a finished world and reports whether it has ore worth
//! digging. It is an **acceptance classifier and nothing else**: no stage calls it,
//! nothing in the sim knows it exists, and it never steers anything. A world that
//! fails it is discarded, never patched (R1b, emphatic). It is here beside the
//! mechanism only because it is about the same subject.

use std::collections::BTreeMap;

use flicker_materials::{ElementId, Tables};
#[cfg(test)]
use flicker_worldstate::Composition;

use crate::column::FormationProcess;
use crate::planet::World;
use crate::stage::{Stage, StageRng};

/// Crust hotter than this circulates the water in it. Below, the rock is tight and
/// cold and the fluid is going nowhere.
const CIRCULATION_K: f64 = 1200.0;

/// Fraction of a hot bed's mobile content the fluid takes per Myr. Small on
/// purpose — the concentration comes from doing it to a great deal of rock for a
/// very long time, not from stripping any one place. e-fold ≈ 200 My: veins are
/// assembled across supercontinent cycles, not in a planet's first chapters.
pub const DEFAULT_LEACH_RATE: f64 = 0.005;

/// Fraction of the fluid's load that drops out at each step along the way.
///
/// **Low is the whole point.** What falls short of the fracture is a vein's fringe,
/// thinning away from the fault; what reaches it is the deposit. Drop too much en
/// route and the metal is smeared over the whole planet — measured at 0.15 the
/// fluid was depositing something in nearly every cell of 1442, which is a volume
/// ratio of one and no ore at all.
const PRECIPITATE_RATE: f64 = 0.02;

/// **Hydrothermal** — leach, carry, drop.
pub struct Hydrothermal {
    /// The elements the fluid can carry — **metal**, and only metal.
    ///
    /// Read from the tables at construction and nowhere typed in: the elements ore
    /// is *extracted for*, kept to those the periodic table calls transition
    /// metals. A catalog that adds a metal ore adds it to the fluid; one that adds
    /// a salt does not.
    ///
    /// The narrowness is the point. A first cut took every element appearing in any
    /// harvestable mineral, which swept in silicon, aluminium, oxygen and calcium —
    /// the framework the rock is built out of. A fluid that carries everything
    /// separates nothing, and the measured enrichment stayed at 3× no matter how
    /// hard the funnel was driven. A brine carries metal and leaves the silicate
    /// behind, and that difference IS the ore-forming step.
    mobile: Vec<ElementId>,
    /// How much of a hot bed the fluid works on per Myr.
    pub leach_rate: f64,
}

/// **The metals a hydrothermal fluid can move** — the elements ore is
/// *extracted for*, kept to those the periodic table calls transition metals.
/// Read from the catalog and nowhere typed in: a catalog that adds a metal ore
/// adds it to the fluid; one that adds a salt does not.
///
/// **The one definition.** The ore view in God Mode needs exactly this set to
/// decide what to look for, and had its own byte-for-byte copy of the filter —
/// two statements of one rule, free to drift the moment either was edited.
///
/// ⚠️ Known gap (2026-08-06): the periodic table carries only three categories
/// (transition_metal / nonmetal_metalloid / alkali_metal), so **tin and lead —
/// real ore metals — are lumped in with silicon and aluminium and excluded**.
/// Widening the filter is not the fix: a first cut took every element in any
/// harvestable mineral, swept in the framework the rock is built out of, and
/// measured enrichment stayed at 3× however hard the funnel was driven — a
/// fluid that carries everything separates nothing. Expressing "ore metal"
/// properly needs a finer category on the element table, which is a content
/// ruling, not a code change.
pub fn ore_metals(tables: &Tables) -> Vec<ElementId> {
    let mut mobile: Vec<ElementId> = tables
        .compounds()
        .iter()
        .filter(|c| c.harvestable)
        .filter_map(|c| c.extracted_element.as_deref())
        .filter_map(|sym| tables.element(sym))
        .filter(|e| e.category == "transition_metal")
        .map(|e| e.number)
        .collect();
    mobile.sort_unstable();
    mobile.dedup();
    mobile
}

impl Hydrothermal {
    pub fn new(tables: &Tables, leach_rate: f64) -> Self {
        Self { mobile: ore_metals(tables), leach_rate }
    }
}

impl Stage for Hydrothermal {
    fn name(&self) -> &'static str {
        "Hydrothermal"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.columns.len();
        if n == 0 || self.mobile.is_empty() {
            return;
        }
        let temp = world.mantle.temp_k.clone();
        let vel = &world.mantle.velocity;

        // **How broken the rock is.** The strain across a cell's contacts — the same
        // quantity the conveyor uses to decide where lithosphere yields, because it
        // is the same fact: rock that is being torn is rock with cracks in it, and
        // cracks are where fluid can actually go. Read off the velocity field, not
        // off the plate observer, which is read-only annotation and must never feed
        // back into a cause.
        let fracture: Vec<f32> = (0..n)
            .map(|i| {
                world.grid.neighbors[i]
                    .iter()
                    .map(|&j| {
                        let j = j as usize;
                        let gap = (world.grid.dirs[i] - world.grid.dirs[j]).length().max(1e-9);
                        (vel[i] - vel[j]).length() / gap
                    })
                    .fold(0.0f32, f32::max)
            })
            .collect();

        // Where a cell's fluid goes: toward the most broken neighbour, because that
        // is the way out. A cell more broken than everything around it is a
        // **discharge site** — the fluid has arrived, and what it is carrying comes
        // out of solution there.
        //
        // This is the volume ratio that makes ore. Fluid is leached from every hot
        // cell, which is most of a young planet, and it all drains into the few
        // cells where the crust is being pulled apart. A great deal of rock gives up
        // a trace; a little rock receives all of it.
        let outlet: Vec<Option<usize>> = (0..n)
            .map(|i| {
                world.grid.neighbors[i]
                    .iter()
                    .map(|&j| j as usize)
                    .filter(|&j| fracture[j] > fracture[i])
                    .max_by(|&a, &b| {
                        fracture[a].partial_cmp(&fracture[b]).unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .collect();

        // Leach: hot rock gives up a trace of what the fluid can carry.
        let take = (self.leach_rate * dt_myr).clamp(0.0, 1.0);
        let mut load: Vec<BTreeMap<ElementId, f64>> = vec![BTreeMap::new(); n];
        for cell in 0..n {
            if temp[cell] < CIRCULATION_K {
                continue;
            }
            let Some(top) = world.columns[cell].layers.last_mut() else {
                continue;
            };
            // The fluid works on `take` of the bed and keeps only what it can carry.
            // Released through the shared rule so the minerals come apart with their
            // elements; the species that cannot travel go straight back, which is
            // the same as saying the fluid passed them by.
            let released = top.release(take);
            for (e, m) in released {
                if self.mobile.binary_search(&e).is_ok() {
                    *load[cell].entry(e).or_insert(0.0) += m;
                } else {
                    world.columns[cell].layers.last_mut().expect("bed").elements.add(e, m);
                }
            }
        }

        // Carry it along the fracture network. Least-broken first, so a load handed
        // onward keeps moving in the same pass — that ordering is the topology of
        // the flow, since fluid only ever moves toward more broken rock.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            fracture[a].partial_cmp(&fracture[b]).unwrap_or(std::cmp::Ordering::Equal)
        });
        for &cell in &order {
            let carried = std::mem::take(&mut load[cell]);
            if carried.is_empty() {
                continue;
            }
            match outlet[cell] {
                // A discharge site. Nowhere more broken to go, so the fluid stops
                // and everything it holds comes out of solution here.
                None => precipitate(world, cell, carried.into_iter().collect()),
                Some(to) => {
                    // A little drops out along the way — the fringe of a vein — and
                    // the rest carries on toward the fracture.
                    let mut settling = Vec::new();
                    for (&e, &m) in &carried {
                        let out = m * PRECIPITATE_RATE;
                        if out > 0.0 {
                            settling.push((e, out));
                        }
                        *load[to].entry(e).or_insert(0.0) += m - out;
                    }
                    precipitate(world, cell, settling);
                }
            }
        }

        // Nothing stays in solution between ticks: whatever is still held comes out
        // where it stands.
        for cell in 0..n {
            let left = std::mem::take(&mut load[cell]);
            if !left.is_empty() {
                precipitate(world, cell, left.into_iter().collect());
            }
        }
    }
}

/// Drop a fluid's load into the rock at this cell. It arrives as material on top,
/// so it goes through the stratum lifecycle like anything else: a load unlike the
/// bed beneath it starts its own bed, which is what a vein *is* — a stratum that
/// ended up with a lot of one thing in it.
fn precipitate(world: &mut World, cell: usize, load: Vec<(ElementId, f64)>) {
    let load: Vec<(ElementId, f64)> = load.into_iter().filter(|&(_, m)| m > 0.0).collect();
    if load.is_empty() {
        return;
    }
    let at = world.tick_myr;
    world.columns[cell].deposit(FormationProcess::Hydrothermal, at, &load);
}

/// How enriched the **richest bed** in a column is in one element, as a multiple of
/// the whole planet's share of it, and which bed that is. `1` is unremarkable rock;
/// ore is orders of magnitude above.
///
/// Read per **bed**, not per column, because that is what a vein is and what a
/// digger actually hits. A rich seam under a thick pile of ordinary rock averages
/// away to nothing at column scale — the same mistake that once had continents
/// reading as lower than the sea floor, made against a different quantity.
pub fn enrichment(world: &World, cell: usize, element: ElementId) -> (f64, Option<usize>) {
    // Against everything the planet HAS, accreted plus delivered — otherwise a
    // metal that mostly arrived late reads as astronomically enriched against a
    // denominator that predates its arrival.
    let have = world.budget.accreted(element) + world.reservoirs.delivered.amount(element);
    let total = world.budget.total() + world.reservoirs.delivered.total();
    let bulk = have / total.max(1.0);
    if bulk <= 0.0 {
        return (0.0, None);
    }
    let mut best = (0.0f64, None);
    for (i, bed) in world.columns[cell].layers.iter().enumerate() {
        let mass = bed.mass_kg();
        if mass <= 0.0 {
            continue;
        }
        let e = (bed.elements.amount(element) / mass) / bulk;
        if e > best.0 {
            best = (e, Some(i));
        }
    }
    best
}

/// What one element looks like across a finished world.
#[derive(Clone, Debug)]
pub struct Prospect {
    pub element: ElementId,
    /// The best enrichment anywhere, as a multiple of the planet's bulk share.
    pub best_enrichment: f64,
    /// The cell it is in.
    pub best_cell: usize,
    /// How deep under the surface of that column the richest bed sits, m — the
    /// question of whether anyone could actually reach it.
    pub depth_m: f64,
    /// How many cells carry a workable concentration of it at all.
    pub workable_cells: usize,
}

/// **The acceptance classifier — read a finished world, never steer one.**
///
/// Reports, for every element the catalog says is worth extracting, whether this
/// world grew any of it into something worth digging and whether anyone could
/// reach it. That is the whole of it: no stage calls this, nothing in the sim knows
/// it exists, and it never feeds back into anything. A world that comes out short
/// is **discarded, not patched** — the ruling is emphatic that accessibility is a
/// property a world either has or does not, never a thing the simulation arranges
/// (R1b: *"A world not generating an iron vein isn't a failure, it's just
/// invalid."*).
///
/// `workable` is the enrichment a cell must reach to count, and `reach_m` how deep
/// a digger can get. Both parameterise the **judgement**, never the generation.
pub fn prospect(world: &World, tables: &Tables, workable: f64, reach_m: f64) -> Vec<Prospect> {
    let area = world.cell_area_m2();
    let mut wanted: Vec<ElementId> = tables
        .compounds()
        .iter()
        .filter(|c| c.harvestable)
        .filter_map(|c| c.extracted_element.as_deref())
        .filter_map(|sym| tables.element(sym).map(|e| e.number))
        .collect();
    wanted.sort_unstable();
    wanted.dedup();

    // How deep a bed sits: everything stacked on top of it.
    let depth_of = |cell: usize, bed: Option<usize>| -> f64 {
        bed.map_or(f64::MAX, |i| {
            world.columns[cell]
                .layers
                .iter()
                .skip(i + 1)
                .map(|l| crate::column::thickness_m(l, area))
                .sum()
        })
    };

    wanted
        .into_iter()
        .map(|element| {
            // **The richest node a digger could actually GET TO.** `reach_m`
            // used to be accepted and discarded, so the best seam on the planet
            // was reported however deep it lay — and a world holding a fabulous
            // orebody twenty kilometres down read as rich, then failed the
            // depth check, while a modest reachable seam beside it went unseen.
            // A classifier that asks "is there an accessible node" has to score
            // reachable ground; the deepest one is not the answer to that.
            let (mut best, mut best_cell, mut best_depth, mut workable_cells) =
                (0.0f64, 0usize, f64::MAX, 0usize);
            for cell in 0..world.columns.len() {
                let (e, bed) = enrichment(world, cell, element);
                let depth = depth_of(cell, bed);
                if depth > reach_m {
                    continue;
                }
                if e >= workable {
                    workable_cells += 1;
                }
                if e > best {
                    best = e;
                    best_cell = cell;
                    best_depth = depth;
                }
            }
            Prospect {
                element,
                best_enrichment: best,
                best_cell,
                depth_m: if best > 0.0 { best_depth } else { f64::MAX },
                workable_cells,
            }
        })
        .collect()
}

/// Whether a world is **valid** to play: every element worth extracting reached a
/// workable concentration somewhere a digger could get to it.
///
/// The verdict only. What to do about a `false` is not a question for this code —
/// the answer is always to throw the world away and grow another.
pub fn is_playable(prospects: &[Prospect], workable: f64, reach_m: f64) -> bool {
    prospects
        .iter()
        .all(|p| p.best_enrichment >= workable && p.depth_m <= reach_m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::scheduler::Scheduler;
    use flicker_materials::JsonTableSource;
    use flicker_worldgrid::icosphere;
    use std::sync::Arc;

    fn run(freq: u32, seed: u64, ticks: usize) -> (World, Arc<Tables>) {
        let t = Arc::new(Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables"));
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        let mut s = Scheduler::new(crate::formation_stages(Arc::clone(&t), &w, &crate::Levers::brisk()), seed);
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None);
        }
        (w, t)
    }

    /// **The fluid carries metal and leaves the rock behind.** Read from the tables,
    /// never typed here — and the narrowness is the mechanism: a first cut took
    /// every element of every harvestable mineral, which swept in the silicate
    /// framework, and a fluid that carries everything separates nothing.
    #[test]
    fn the_fluid_carries_metal_not_the_framework() {
        let t = Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables");
        let h = Hydrothermal::new(&t, crate::hydrothermal::DEFAULT_LEACH_RATE);
        let has = |sym: &str| t.element(sym).is_some_and(|e| h.mobile.binary_search(&e.number).is_ok());
        assert!(has("Cu") && has("Au") && has("Fe"), "metal travels");
        assert!(!has("Si") && !has("Al") && !has("O"), "the framework stays put");
    }

    /// Leach, carry and drop move mass and never make it — and nothing is left
    /// dissolved between ticks, because a fluid is not a place to keep matter.
    #[test]
    fn circulation_conserves_every_gram() {
        let (mut w, t) = run(6, 5, 100);
        let stage = Hydrothermal::new(&t, crate::hydrothermal::DEFAULT_LEACH_RATE);
        let mut rng = crate::stage::StageRng::new(4);
        for _ in 0..25 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("Hydrothermal");
            w.audit_compound_bound("Hydrothermal");
        }
    }

    /// Enrichment is read per **bed**, because that is what a vein is and what a
    /// digger hits. A rich seam under a thick pile of ordinary rock must not average
    /// away to nothing.
    #[test]
    fn a_vein_is_a_bed_not_a_column_average() {
        let (mut w, _t) = run(6, 7, 0);
        let cell = 0usize;
        let gold = 79u8;
        // A big ordinary bed, then a thin very rich one on top of it.
        let mut plain = Composition::new();
        plain.add(14, 1.0e19);
        let mut rich = Composition::new();
        rich.add(gold, 1.0e15);
        for elements in [plain, rich] {
            w.columns[cell].layers.push(crate::column::Layer {
                elements,
                minerals: flicker_worldstate::CompoundLedger::new(),
                formed_at_myr: 0.0,
                formed_by: FormationProcess::Hydrothermal,
                peak_pt: (0.0, 0.0),
            cooled: 0.0,
            eclogitised: 0.0,
            });
        }
        let (best, bed) = enrichment(&w, cell, gold);
        assert_eq!(bed, Some(w.columns[cell].layers.len() - 1), "the richest bed is the seam");
        // Averaged over the column the seam is one part in ten thousand; read as a
        // bed it is the whole of it.
        let col = &w.columns[cell];
        let averaged = col.element_mass(gold) / col.mass_kg();
        let as_bed = col.layers.last().expect("seam").elements.amount(gold)
            / col.layers.last().expect("seam").mass_kg();
        assert!(as_bed > averaged * 100.0, "the column average buries the seam");
        assert!(best > 0.0);
    }

    /// **Prospecting reads a world; it never touches one.** The classifier exists
    /// outside the simulation entirely — no stage calls it, and a world that comes
    /// up short is discarded rather than corrected (R1b).
    #[test]
    fn prospecting_only_reads() {
        let (w, t) = run(6, 11, 150);
        let before: Vec<usize> = w.columns.iter().map(|c| c.layers.len()).collect();
        let before_mass: f64 = w.columns.iter().map(|c| c.mass_kg()).sum();
        let report = prospect(&w, &t, 5.0, 3000.0);
        assert!(!report.is_empty(), "the catalog names things worth extracting");
        let after: Vec<usize> = w.columns.iter().map(|c| c.layers.len()).collect();
        let after_mass: f64 = w.columns.iter().map(|c| c.mass_kg()).sum();
        assert_eq!(before, after, "prospecting changed the world");
        assert_eq!(before_mass, after_mass, "prospecting changed the world");
        // And the verdict is a verdict — it says nothing about what to do next.
        let _: bool = is_playable(&report, 5.0, 3000.0);
    }

    /// The tectonic cycle really does sort: what partitions into a melt climbs in
    /// the crust, pass after pass, while what stays behind in the residue does not.
    /// This is the mechanism the vein ruling calls the distillation column.
    #[test]
    fn the_cycle_sorts_incompatible_from_compatible() {
        let (w, _t) = run(6, 13, 250);
        let bulk = |z: u8| w.budget.accreted(z) / w.budget.total();
        let crust: f64 = w.columns.iter().map(|c| c.mass_kg()).sum();
        if crust <= 0.0 {
            return;
        }
        let in_crust = |z: u8| w.columns.iter().map(|c| c.element_mass(z)).sum::<f64>() / crust;
        // Potassium is the most incompatible thing in the table; magnesium stays in
        // the residue. Their ratio in the crust must beat their ratio in the planet.
        let sorted = (in_crust(19) / bulk(19)) / (in_crust(12) / bulk(12)).max(1e-12);
        assert!(sorted > 1.5, "the melt did not sort incompatible from compatible: {sorted:.2}x");
    }
}
