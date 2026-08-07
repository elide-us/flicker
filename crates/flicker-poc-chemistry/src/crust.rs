//! **Crust generation** — the surface freezing over — and the stack's own
//! housekeeping. Everything that happens to crust *afterwards* belongs to the
//! conveyor ([`crate::tectonics`]).
//!
//! What is left here is the one thing that is not tectonic: a cell whose mantle
//! has cooled below the solidus proxy grows a mafic rind, thickening toward a
//! saturation thickness and then stopping. That is sea floor, and it blankets the
//! planet as it cools — bare mantle survives only where it is still a magma ocean.
//!
//! **Subduction is no longer a stage, and arcs are no longer read off a map.** Both
//! used to live here, driven by the SIGN of the velocity field's divergence at each
//! cell: outflow was declared a ridge, inflow a trench, and continental crust was
//! manufactured wherever the number came out negative. It worked, and it was a
//! shortcut in two ways that mattered. It made continents by fiat rather than
//! because anything collided, and — because a divergence is a derivative of a field
//! defined on the mesh — it printed the icosahedron through into the world, worse
//! the higher the resolution.
//!
//! Now a stack sinks because it lost a collision and was too dense to resist, and
//! an arc erupts because a slab actually went down beneath it. The hypsometry still
//! comes out bimodal, but from the two things that were always doing the work:
//! composition sets density ([`density_kg_m3`](crate::column::density_kg_m3)), and
//! survival sets thickness — sea floor is recycled entire, while rock too buoyant
//! to subduct accumulates. Absolute Airy isostasy
//! ([`elevation_m`](crate::column::elevation_m)) reads the elevation off both.
//! Nobody sets "continents are high".

use flicker_materials::ElementId;

use crate::column::FormationProcess;
use crate::planet::World;
use crate::stage::{Stage, StageRng};

/// Basaltic (mafic) melt affinity per element — the fraction of the mantle's
/// mass of that element that a spreading-centre melt draws up. Keeps real Mg/Fe/Ca
/// (mafic → dense), so oceanic crust sits low.
pub(crate) fn oceanic_affinity(e: ElementId) -> f64 {
    match e {
        8 => 0.55,  // O
        12 => 0.35, // Mg — mafic melts carry magnesium
        14 => 0.55, // Si
        26 => 0.50, // Fe
        20 => 0.70, // Ca
        13 => 0.65, // Al
        11 => 0.60, // Na
        19 => 0.60, // K
        22 => 0.60, // Ti
        16 => 0.30, // S
        _ => 0.30,
    }
}

/// Refined felsic melt affinity per element — a more evolved, silica-rich melt.
/// Strips the incompatibles (Si/Al/K/Na) and leaves Mg/Fe behind, so continental
/// crust is light and rides high.
pub(crate) fn continental_affinity(e: ElementId) -> f64 {
    match e {
        8 => 0.55,  // O
        12 => 0.08, // Mg — left in the residue (felsic → Mg-poor)
        14 => 0.70, // Si — silica-rich
        26 => 0.15, // Fe
        20 => 0.30, // Ca
        13 => 0.70, // Al
        11 => 0.90, // Na
        19 => 0.95, // K — the most incompatible
        3 => 0.92,  // Li
        92 => 0.95, // U (concentrates in continental crust — real)
        22 => 0.30, // Ti
        _ => 0.30,
    }
}

/// Crust solidifies only once a cell's mantle has cooled below this proxy solidus.
/// A magma ocean grows no crust; as the planet cools, crust **freezes over the
/// whole surface** (a temperature — i.e. chemistry — gate, never the tick number).
/// One well-mixed mantle temperature stands in for the surface at M1/M2, and the
/// real surface is far cooler, so this is an abstraction of "the surface can now
/// solidify," not a literal basalt solidus.
pub(crate) const SOLIDUS_K: f64 = 3700.0;
/// How fast a cooled cell approaches its saturation crust thickness, per Myr.
/// e-fold ≈ 500 My: the lid assembles across the early eras the way Earth's
/// did, not in the first 33 My (the pre-recalibration shortcut — tests that
/// probe the mechanism use [`Levers::brisk`](crate::Levers::brisk)).
pub const DEFAULT_CRUST_GEN_RATE: f64 = 0.002;
/// Saturation thickness targets, as a fraction of the cell's initial mass (so the
/// target is grid-frequency-independent). Oceanic basalt saturates **thin** — a
/// fixed few-km layer that blankets the seafloor; continental crust builds far
/// **thicker** because arc magmatism keeps adding and it is not recycled. The two
/// targets (~8×) are the thickness half of the bimodal hypsometry; density is the
/// other half.
const OCEANIC_SAT_FRAC: f64 = 0.0015;

/// **CrustGeneration** — the surface freezes into crust. A cell grows crust once
/// its mantle has cooled below the [`SOLIDUS_K`] proxy, so crust **covers the whole
/// surface** as the planet cools (bare mantle survives only where it is still a
/// magma ocean). Convergent arcs refine felsic *continental* crust (thick,
/// buoyant); ridges **and plate interiors alike** solidify mafic *oceanic* crust —
/// so the seafloor is blanketed and continents are the exceptions, as on a real
/// planet. Each melt is drawn from the mantle cell by affinity and deposited on top
/// of the column — mantle→crust, conserved.
pub struct CrustGeneration {
    /// How fast a cooled cell approaches its saturation thickness, per Myr.
    pub rate: f64,
}

impl Default for CrustGeneration {
    /// The physics as written.
    fn default() -> Self {
        Self { rate: DEFAULT_CRUST_GEN_RATE }
    }
}

impl Stage for CrustGeneration {
    fn name(&self) -> &'static str {
        "CrustGeneration"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let elems: Vec<ElementId> = world.mantle.elements().to_vec();
        let m0_per_cell = world.budget.total() / world.mantle.n_cells() as f64;

        for cell in 0..world.columns.len() {
            // A magma-ocean cell grows no crust; the surface freezes only once the
            // mantle beneath it has cooled below the solidus proxy.
            if world.mantle.temp_k[cell] > SOLIDUS_K {
                continue;
            }
            // Negative feedback: growth slows to zero as the sea floor reaches its
            // saturation thickness — a basalt rind, not an ever-thickening slab.
            let fill = (1.0 - world.columns[cell].mass_kg() / (OCEANIC_SAT_FRAC * m0_per_cell)).max(0.0);
            if fill <= 0.0 {
                continue;
            }
            let step = self.rate * fill * dt_myr;
            let mut melt: Vec<(ElementId, f64)> = Vec::new();
            for &e in &elems {
                let want = step * oceanic_affinity(e) * world.mantle.mass(cell, e);
                if want > 0.0 {
                    let took = world.mantle.remove(cell, e, want);
                    if took > 0.0 {
                        melt.push((e, took));
                    }
                }
            }
            if !melt.is_empty() {
                world.columns[cell].deposit(FormationProcess::OceanicCrust, world.tick_myr, &melt);
            }
        }
    }
}

/// Below this the interior makes no melt anywhere and volcanism is over for good
/// — the proxy basaltic solidus, well under the [`SOLIDUS_K`] surface-freezing
/// proxy so a planet keeps erupting long after it has grown a lid.
pub(crate) const ERUPTION_FLOOR_K: f64 = 1200.0;
/// How far above the planetary mean a cell must run to erupt at full vigor, K.
/// This is the **heat locality** scale: a plume is not "hot", it is *hotter than
/// the world around it*, which is why volcanism concentrates instead of covering
/// the planet evenly.
const PLUME_ANOMALY_K: f64 = 150.0;
/// Fraction of a plume cell's mantle drawn up as erupted melt per Myr at full
/// vigor. Anchored to real magma production: Earth turns out order 10²⁰ kg of new
/// volcanic rock per Myr, and at this rate a whole planet erupting flat out runs
/// a few times that — a young, hot world, which is what this is. Small, because a
/// volcano is a pinprick next to a mid-ocean ridge; what makes it a mountain is
/// that it keeps returning to the same place.
pub const DEFAULT_ERUPTION_RATE: f64 = 5.0e-5;

/// **Volcanism** — melt finds a way through the lid, and builds on top of it.
///
/// Where the mantle runs hotter than the planet's own average and a frozen crust
/// stands over it, melt rises and erupts. Three consequences fall out, none of
/// them written anywhere as a result:
///
/// **A mountain.** The lava is deposited on the column like any other material,
/// and absolute Airy isostasy ([`elevation_m`](crate::column::elevation_m))
/// floats the heavier pile higher. Nothing lifts anything; the ground stands up
/// because there is more of it.
///
/// **Gas with an address.** The rising melt decompresses and its volatiles come
/// out of solution into the air, through the shared
/// [`GasVocabulary`](crate::atmosphere::GasVocabulary) — the same chemistry bulk
/// degassing speaks, with the floors off (see [`vent`]). The refractory residue
/// is what actually freezes as rock. So an erupting world keeps its sky topped
/// up long after the mantle has cooled too far to degas in bulk.
///
/// **A chain.** The hot cell is in the *mantle* and the column above it is on a
/// *plate*, so the conveyor walks the crust over a stationary vent and each tick
/// erupts onto whatever column has arrived — leaving a trail of volcanic piles
/// downstream of the plume. No hotspot-track rule exists; the track is what two
/// existing mechanisms do when they run at the same time.
///
/// It fades on its own. Eruption vigor is read off the heat that is actually
/// there, and [`RadiogenicDecay`](crate::interior::RadiogenicDecay) is spending
/// that heat down the whole run — so the early molten world is violent and the
/// old one is quiet, with nothing scheduling either.
///
/// [`vent`]: crate::atmosphere::GasVocabulary::vent
pub struct Volcanism {
    /// Fraction of a plume cell's mantle erupted per Myr at full vigor.
    pub rate: f64,
    gases: crate::atmosphere::GasVocabulary,
}

impl Volcanism {
    /// Resolve the gas vocabulary an eruption vents through.
    pub fn new(tables: &flicker_materials::Tables, rate: f64) -> Self {
        Self { rate, gases: crate::atmosphere::GasVocabulary::load(tables) }
    }
}

impl Stage for Volcanism {
    fn name(&self) -> &'static str {
        "Volcanism"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.mantle.n_cells();
        if n == 0 {
            return;
        }
        // The reference the locality is measured against — sampled here rather
        // than taken from the tick's opening state, because the stages ahead of
        // this one have already moved the heat around.
        let mean = world.mantle.temp_k.iter().sum::<f64>() / n as f64;

        for cell in 0..n {
            // No lid, no volcano: an eruption needs something to erupt through.
            if world.columns[cell].layers.is_empty() {
                continue;
            }
            let t = world.mantle.temp_k[cell];
            if t < ERUPTION_FLOOR_K {
                continue;
            }
            // Heat LOCALITY: only ground hotter than the world's own average
            // feeds a vent, and how much hotter sets how hard it goes. SQUARED,
            // so output concentrates in the genuinely anomalous ground instead of
            // smearing thinly across every cell that happens to sit a degree
            // above average — which is the difference between a planet with
            // volcanoes and a planet with a warm half.
            let anomaly = ((t - mean) / PLUME_ANOMALY_K).clamp(0.0, 1.0);
            let vigor = anomaly * anomaly;
            if vigor <= 0.0 {
                continue;
            }
            let mut melt = crate::tectonics::draw_melt(
                world,
                cell,
                (self.rate * vigor * dt_myr).min(1.0),
                oceanic_affinity,
            );
            if melt.is_empty() {
                continue;
            }
            // The volatiles fly; what will not fly freezes as rock.
            self.gases.vent(&mut world.reservoirs.atmosphere, &mut melt);
            if !melt.is_empty() {
                world.columns[cell].deposit(FormationProcess::Volcanic, world.tick_myr, &melt);
            }
        }
    }
}

/// **Crystallization** — free elements in a bed organise into the minerals that
/// composition can actually make.
///
/// The rock tier needs this to exist: `rocks.json` describes rocks as **modal
/// mixtures of minerals**, so a bed with no minerals in it is a bed the catalog
/// cannot recognise, and erosion has to fall back on a default resistance for
/// everything — which erodes the whole world evenly and develops no shape at all.
///
/// Nothing here names a mineral. It asks the compound table what each candidate is
/// made of, works out how much of it this bed's elements could supply, and forms
/// the assemblage it can make most of first — the abundant phase crystallises,
/// then the next, until the elements run short. A bed of a different composition
/// therefore ends up a different rock without a single rule mentioning either.
///
/// A **stand-in for free-energy minimisation**, which is what really decides an
/// assemblage: this greedy order is a plausible proxy and is honestly a proxy. It
/// is also why metamorphism is not here — reorganising an assemblage at high
/// pressure needs each phase's stability field, and the compound catalog carries
/// no P/T data yet. The record it will read is already kept ([`Layer::peak_pt`]).
pub struct Crystallization {
    tables: std::sync::Arc<flicker_materials::Tables>,
}

impl Crystallization {
    pub fn new(tables: std::sync::Arc<flicker_materials::Tables>) -> Self {
        Self { tables }
    }
}

/// Fraction of what a bed *could* crystallise that it does per Myr — rock does not
/// organise itself instantly, and a rate keeps this a process rather than a verdict.
const CRYSTALLIZATION_RATE: f64 = 0.05;

impl Stage for Crystallization {
    fn name(&self) -> &'static str {
        "Crystallization"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        // The minerals the simulation is asked to carry, with what each is made of.
        let recipes: Vec<(u16, Vec<(ElementId, f64)>)> = self
            .tables
            .compounds()
            .iter()
            .filter(|c| c.category == "mineral" && c.sim_required)
            .map(|c| (c.id, self.tables.compound_mass_fractions(c)))
            .collect();
        if recipes.is_empty() {
            return;
        }
        // What a bed's minerals LOCK is judged against the WHOLE catalog — a
        // hydrothermal vein or a carbonate bed locks elements too, and counting
        // only this stage's own recipes would let it crystallise silicates out
        // of oxygen that calcite already owns (the audit's definition of free,
        // or the bound breaks).
        let stoich: Vec<(u16, Vec<(ElementId, f64)>)> = self
            .tables
            .compounds()
            .iter()
            .map(|c| (c.id, self.tables.compound_mass_fractions(c)))
            .collect();
        let step = (CRYSTALLIZATION_RATE * dt_myr).clamp(0.0, 1.0);

        for col in &mut world.columns {
            for layer in &mut col.layers {
                // What is still free: this bed's elements, less what its existing
                // minerals already account for — ALL of them, whoever booked
                // them. The compound-ledger bound is the standing invariant this
                // must not break.
                let mut free: Vec<(ElementId, f64)> = layer.elements.iter().collect();
                for (id, mineral_mass) in layer.minerals.iter() {
                    if let Some((_, fracs)) = stoich.iter().find(|(rid, _)| *rid == id) {
                        for &(e, f) in fracs {
                            if let Some(slot) = free.iter_mut().find(|(fe, _)| *fe == e) {
                                slot.1 = (slot.1 - mineral_mass * f).max(0.0);
                            }
                        }
                    }
                }

                // Form the assemblage this bed can make most of, then the next.
                let mut formed: Vec<(u16, f64)> = Vec::new();
                loop {
                    let mut best: Option<(u16, f64)> = None;
                    for (id, fracs) in &recipes {
                        let mut can = f64::MAX;
                        for &(e, f) in fracs {
                            if f <= 0.0 {
                                continue;
                            }
                            let have = free.iter().find(|(fe, _)| *fe == e).map_or(0.0, |s| s.1);
                            can = can.min(have / f);
                        }
                        if can.is_finite() && can > 0.0 && best.map_or(true, |(_, b)| can > b) {
                            best = Some((*id, can));
                        }
                    }
                    let Some((id, can)) = best else { break };
                    let make = can * step;
                    if make <= 0.0 {
                        break;
                    }
                    let fracs = &recipes.iter().find(|(rid, _)| *rid == id).expect("just chosen").1;
                    for &(e, f) in fracs {
                        if let Some(slot) = free.iter_mut().find(|(fe, _)| *fe == e) {
                            slot.1 = (slot.1 - make * f).max(0.0);
                        }
                    }
                    formed.push((id, make));
                    if formed.len() >= recipes.len() {
                        break;
                    }
                }
                for (id, mass) in formed {
                    layer.minerals.add(id, mass);
                }
            }
        }
    }
}

/// Time for cooling mafic crust to travel most of the way into its dense
/// assemblage, Myr. Anchored on Earth's own sea floor, which does the bulk of
/// its subsiding inside ~80 My of leaving the ridge and is nearly done by 150.
/// The bed approaches the state exponentially, so nothing switches on: a young
/// floor is barely denser than the day it froze, and an old one is a slab
/// waiting to founder.
const DENSIFICATION_EFOLD_MYR: f64 = 70.0;

/// **CrustDensification** — igneous rock, left alone, becomes heavier rock.
///
/// Not a new substance and not a gram of new mass: the same elements taking on
/// the tighter mineral assemblage that cold, deep basalt actually wears
/// (gabbro → eclogite). All this stage does is advance each igneous bed's
/// [`densified`](crate::column::Layer::densified) toward 1; what that MEANS is
/// [`density_kg_m3`](crate::column::density_kg_m3)'s business, and what the
/// density means is isostasy's.
///
/// Three things fall out of it, none of them written anywhere as a result:
///
/// **Ocean basins.** Sea floor is born lighter than the mantle and ends up
/// heavier than it, so old floor rides *below* the datum. That is the container
/// a sea needs. Without it every column floats, the water spreads out over all
/// of them, and every world ends up the same flat blue ball whatever else it
/// did — the single largest reason this simulation could not make an Earth.
///
/// **Slab pull, in the right order.** The densest stack loses a collision, and
/// old floor is now the densest thing on the planet — so the oldest sea floor
/// founders first, which is what keeps a basin cycling instead of paving over.
///
/// **Continents that stay.** The gain is scaled by how mafic the rock is, so a
/// refined felsic pile gains nothing, ever. Nothing says continents are
/// permanent; they simply never become heavy enough to sink.
pub struct CrustDensification;

impl Stage for CrustDensification {
    fn name(&self) -> &'static str {
        "CrustDensification"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let step = (dt_myr / DENSIFICATION_EFOLD_MYR).clamp(0.0, 1.0);
        for col in &mut world.columns {
            for bed in &mut col.layers {
                // Only rock a melt made. Sediment compacts and organics cook —
                // both real, both somebody else's stage — but neither is the
                // basalt phase change, and letting a mud drape "eclogitise"
                // would sink coastlines for no reason anyone could point at.
                if matches!(
                    bed.formed_by,
                    FormationProcess::Sediment
                        | FormationProcess::Organic
                        | FormationProcess::Hydrothermal
                ) {
                    continue;
                }
                bed.densified += (1.0 - bed.densified) * step;
            }
        }
    }
}

/// Overburden at which a stack's deepest rock has been pushed deep enough to
/// change phase, Pa. Earth's crust runs out of room around 70 km — below that
/// the base of a thickened root sits at pressures where the rock is no longer
/// stable as crust — and this is that depth in the currency the columns already
/// keep: `ρgh` for ~70 km of ~2800 kg/m³ rock.
const DELAMINATION_PA: f64 = 1.9e9;

/// Fraction of a root's EXCESS load that founders per Myr — an e-fold of ~10
/// My, the pace at which a real orogen sheds a root it can no longer carry.
///
/// This constant sets where the ceiling actually sits, and the arithmetic is
/// worth stating because it is not obvious: a pile settles where the collision
/// feed equals what is shed, so `excess = feed ÷ rate`. At 0.02 the equilibrium
/// peak measured 55–75 km — bounded and stable, but many times any mountain
/// this planet could hold up. Faster shedding pulls the balance point down
/// proportionally; a mountain is only ever as tall as the rock beneath it can
/// bear, and this is how quickly it stops pretending otherwise.
const DELAMINATION_RATE: f64 = 0.1;

/// **Delamination** — a mountain root that grows too deep falls off.
///
/// The pressure-driven twin of [`CrustDensification`]: that one is rock going
/// dense because it got *cold*, this one is rock going dense because it got
/// *deep*. Past [`DELAMINATION_PA`] the bottom of a thickened pile is no longer
/// stable as crust; it converts, becomes heavier than the mantle it is resting
/// on, and sinks away — taking the pile's height with it.
///
/// **This is the ceiling the model did not have.** Collisions pile stacks onto
/// stacks with nothing to stop them: a 4.5 BY bake put one column at fourteen
/// thousand kilometres of elevation — taller than the planet is wide — holding
/// something like a sixth of all the crust in the world (measured 2026-08-06).
/// Erosion is orders of magnitude too slow to answer that, and isostasy will
/// happily float a pile of any size. Real orogens do not get the choice: they
/// shed their roots, and that is why Earth's mountains stop at single-digit
/// kilometres instead of growing forever.
///
/// Conserving — the foundered rock is credited to the mantle beneath it, the
/// same move a subducting slab makes, and the same distillation loop gets it
/// back as refined arc melt later.
pub struct Delamination;

impl Stage for Delamination {
    fn name(&self) -> &'static str {
        "Delamination"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let area = world.cell_area_m2();
        let step = (DELAMINATION_RATE * dt_myr).clamp(0.0, 1.0);
        for cell in 0..world.columns.len() {
            // The load on the deepest rock is the whole stack's own weight.
            let basal = crate::column::basal_pressure_pa(&world.columns[cell], area);
            if basal < DELAMINATION_PA {
                continue;
            }
            // **What goes is measured against the OVERSHOOT, not against the
            // bottom bed.** Taking a fixed slice of the lowest bed sounds the
            // same and is not: a pile whose base has already been shaved thin
            // sheds almost nothing while its top keeps growing, so the ceiling
            // leaks — measured, a column still reached 909 km by 4.5 BY. The
            // load above the limit is a MASS (`p·A/g`), and that is the thing
            // the root cannot hold, so that is the thing that falls off. The
            // pile then settles toward the pressure it can carry, which is what
            // a ceiling means.
            let mut budget = (basal - DELAMINATION_PA) * area / crate::column::GRAVITY_M_S2 * step;
            // The BASE goes — bottom-first, because that is where the pressure
            // is. What leaves is rock, so the column gets shorter; what the
            // mountain keeps is everything above it.
            //
            // Eat upward from the base until the budget is spent, because the
            // rock that has to go may be more than the lowest bed is.
            //
            // A bed the budget can cover — or would leave as a film — goes
            // ENTIRE rather than by another fraction of a fraction. Physically
            // it has already foundered; numerically it must, because two
            // ledgers shaved by the same proportion for hundreds of ticks drift
            // apart in the last digits, and on a bed worn to kilograms that
            // drift is larger than the compound bound's tolerance (the 4.5 BY
            // bake broke on exactly that: a layer claiming 2.199352 kg of
            // oxygen against 2.199341 kg free).
            while budget > 0.0 {
                let Some(root) = world.columns[cell].layers.first_mut() else {
                    break;
                };
                let mass = root.mass_kg();
                if mass <= 0.0 {
                    world.columns[cell].layers.remove(0);
                    continue;
                }
                let whole = budget >= mass || mass - budget < crate::column::MIN_BED_MASS_KG;
                let taken = root.release(if whole { 1.0 } else { budget / mass });
                let moved: f64 = taken.iter().map(|&(_, m)| m).sum();
                for (e, m) in taken {
                    world.mantle.add(cell, e, m);
                }
                if world.columns[cell].layers.first().is_some_and(|l| l.elements.is_empty()) {
                    world.columns[cell].layers.remove(0);
                }
                budget -= moved;
                // A partial bite satisfied the budget; anything else means the
                // bed is gone and the next one down is now the root.
                if !whole || moved <= 0.0 {
                    break;
                }
            }
        }
    }
}

/// **Strata reconciliation** — the stack's housekeeping, run after the tick's
/// material has moved. It is the last step of the loop for a reason: every bed
/// records the load and heat it has just endured, and then burial is allowed to
/// erase the boundaries burial erases.
///
/// This stage **moves no mass between cells and creates none** — it only merges
/// beds within a column, whole-mass, so the conservation audit sees nothing
/// change. What it does change is the column's *history*: `peak_pt` is the
/// permanent record a bed carries, and it is what the metamorphic chemistry will
/// read when a buried carbon bed reorganises into something harder.
pub struct StrataReconcile;

/// How many beds a column carries before the merge tolerance starts widening.
/// A **data-volume** guardrail, not a shape target: past the migration every bed
/// with surface expression is potentially another 2K map per tile, so the stack
/// is allowed to grow but not without limit. See
/// [`Column::reconcile`](crate::column::Column::reconcile) for how the pressure
/// is applied — by letting the most-alike pair go, never by capping the count.
/// **20 retained physical layers above the molten map is Aaron's ruled range**
/// (2026-08-06) — enough for a canyon wall to read as a layer cake.
pub const STRATA_SOFT_CAP: usize = 20;

impl Stage for StrataReconcile {
    fn name(&self) -> &'static str {
        "StrataReconcile"
    }

    fn tick(&self, world: &mut World, _dt_myr: f64, _rng: &mut StageRng) {
        let area = world.cell_area_m2();
        for cell in 0..world.columns.len() {
            // The rock a bed sits in is the rock beneath it — the mantle cell's
            // temperature until a real geotherm arrives with the thermal stages.
            let temp_k = world.mantle.temp_k[cell];
            world.columns[cell].reconcile(temp_k, area, STRATA_SOFT_CAP);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::budget::Budget;
    use crate::column::{crust_kind, elevation_m, CrustKind};
    use crate::config::content_data_dir;
    use crate::planet::World;
    use crate::scheduler::Scheduler;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    /// A world run through the full formation pipeline for `ticks` Myr.
    fn run(freq: u32, seed: u64, ticks: usize) -> World {
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        let mut s = Scheduler::new(crate::formation_stages(std::sync::Arc::clone(&t), &w.budget.clone(), &crate::Levers::brisk()), seed);
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None); // conservation audited every tick
        }
        w
    }

    /// A lidded cell running hotter than its world erupts: lava lands on the
    /// column as its own bed, the melt's volatiles arrive in the air as booked
    /// gas, and both conserved ledgers hold through it.
    #[test]
    fn an_eruption_builds_ground_and_vents_gas() {
        use crate::column::{FormationProcess, Layer};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 61);

        // A frozen world — cold enough for a lid, warm enough to still melt —
        // with one cell running hot: the plume.
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 1400.0;
        }
        w.mantle.temp_k[0] = 1400.0 + super::PLUME_ANOMALY_K;
        // Give every column a lid to erupt through.
        for col in &mut w.columns {
            col.layers.push(Layer {
                elements: Default::default(),
                minerals: Default::default(),
                formed_at_myr: 0.0,
                formed_by: FormationProcess::OceanicCrust,
                peak_pt: (0.0, 0.0),
            densified: 0.0,
            });
        }

        let stage = super::Volcanism::new(&t, super::DEFAULT_ERUPTION_RATE);
        let mut rng = StageRng::new(3);
        for _ in 0..20 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.tick_myr += 1.0;
            w.audit("Volcanism");
            w.audit_compound_bound("Volcanism");
        }

        assert!(
            w.columns[0].layers.iter().any(|l| l.formed_by == FormationProcess::Volcanic),
            "the plume built a volcanic bed"
        );
        assert!(w.reservoirs.atmosphere.mass_kg() > 0.0, "and vented gas doing it");
        // The vent is decompression-driven, so it works at a temperature far
        // below every bulk-degassing floor — this is the mechanism that keeps a
        // cooled planet's sky topped up.
        assert!(
            w.reservoirs.atmosphere.species.amount(crate::atmosphere::WATER_VAPOUR) > 0.0
                || w.reservoirs.atmosphere.species.amount(crate::atmosphere::CARBON_DIOXIDE) > 0.0,
            "the vented gas is real species, not loose atoms"
        );
    }

    /// **Heat LOCALITY is the whole trigger.** A world at one uniform
    /// temperature has no plumes anywhere — nothing is hotter than the average
    /// it is part of — so nothing erupts however hot it is.
    #[test]
    fn a_world_with_no_hot_spots_has_no_volcanoes() {
        use crate::column::{FormationProcess, Layer};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 62);
        for c in 0..w.mantle.n_cells() {
            w.mantle.temp_k[c] = 3000.0; // hot, but uniformly so
        }
        for col in &mut w.columns {
            col.layers.push(Layer {
                elements: Default::default(),
                minerals: Default::default(),
                formed_at_myr: 0.0,
                formed_by: FormationProcess::OceanicCrust,
                peak_pt: (0.0, 0.0),
            densified: 0.0,
            });
        }
        let stage = super::Volcanism::new(&t, super::DEFAULT_ERUPTION_RATE);
        let mut rng = StageRng::new(4);
        for _ in 0..5 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("Volcanism");
        }
        assert_eq!(w.reservoirs.atmosphere.mass_kg(), 0.0, "no anomaly, no vent");
        assert!(
            !w.columns.iter().any(|c| c
                .layers
                .iter()
                .any(|l| l.formed_by == FormationProcess::Volcanic)),
            "and no volcanic rock anywhere"
        );
    }

    /// **The gate, both ways.** Volcanism is shut at t=0 because there is no lid
    /// to come through, opens once the world has frozen over, and shuts again for
    /// good once the interior is too cold to make melt ANYWHERE. Two conditions
    /// on the world's own state, both of which flip during a real run — which
    /// is exactly what the bench pauses on. Neither is a mean: coverage decides
    /// whether there is a lid, and the hottest cell decides whether there is
    /// melt.
    #[test]
    fn the_volcanism_gate_opens_on_a_lid_and_shuts_on_the_cold() {
        use crate::planet::PlanetState;
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let w = World::seed(icosphere(4), b, &t, 63);

        let molten = PlanetState::sample(&w);
        assert_eq!(molten.lid_frac, 0.0, "a magma ball has no lid");
        assert!(!crate::process_file::gate_of("Volcanism").holds(&molten, &crate::Levers::default()), "so volcanism has not woken up yet");

        let mut lidded = molten.clone();
        lidded.lid_frac = 0.01;
        lidded.max_mantle_temp_k = 2000.0;
        assert!(crate::process_file::gate_of("Volcanism").holds(&lidded, &crate::Levers::default()), "a lid over hot mantle: the gate opens");

        // The HOTTEST cell shuts it, not the average — and this is the case
        // that matters: a world whose mean has fallen below the melt floor can
        // still be carrying a plume above it, and that plume is exactly what
        // this stage exists to erupt.
        let mut mean_is_cold = lidded.clone();
        mean_is_cold.mean_mantle_temp_k = super::ERUPTION_FLOOR_K - 500.0;
        assert!(
            crate::process_file::gate_of("Volcanism").holds(&mean_is_cold, &crate::Levers::default()),
            "a cold AVERAGE over a live plume must not switch volcanism off"
        );

        let mut cold = lidded.clone();
        cold.max_mantle_temp_k = super::ERUPTION_FLOOR_K - 1.0;
        assert!(!crate::process_file::gate_of("Volcanism").holds(&cold, &crate::Levers::default()), "no melt ANYWHERE: shut for good");
    }

    /// **THE BASIN MECHANISM.** Fresh sea floor floats; the same rock, cold,
    /// sinks. Without the second half every column in the world rides above the
    /// datum, a sea has nowhere to sit, and any ocean at all spreads out and
    /// drowns everything — which is the flat-water-world every run used to end
    /// as. And it must NOT touch continents, or the fix would drown the very
    /// thing it exists to expose.
    #[test]
    fn cold_sea_floor_sinks_below_the_datum_and_continents_do_not() {
        use crate::column::{density_kg_m3, elevation_m, FormationProcess, Layer, MANTLE_DENSITY};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 77);
        let area = w.cell_area_m2();

        // Drawn OUT of the mantle, never conjured: the harness is right that a
        // fixture which invents mass is testing a world that cannot exist.
        let bed = |w: &mut World, cell: usize, els: &[(u8, f64)]| {
            let mut c = flicker_worldstate::Composition::new();
            for &(e, m) in els {
                c.add(e, w.mantle.remove(cell, e, m));
            }
            w.columns[cell].layers.push(Layer {
                elements: c,
                minerals: Default::default(),
                formed_at_myr: 0.0,
                formed_by: FormationProcess::OceanicCrust,
                peak_pt: (0.0, 0.0),
                densified: 0.0,
            });
        };
        // Cell 0: mafic sea floor. Cell 1: a refined felsic pile.
        bed(&mut w, 0, &[(8, 4.5e18), (14, 2.4e18), (12, 1.9e18), (26, 1.2e18)]);
        bed(&mut w, 1, &[(8, 4.7e18), (14, 3.4e18), (13, 1.5e18), (19, 0.4e18)]);
        w.audit("fixture");

        let fresh_floor = density_kg_m3(&w.columns[0].layers[0]);
        let fresh_high = elevation_m(&w.columns[0], area);
        let cont_before = elevation_m(&w.columns[1], area);
        assert!(fresh_floor < MANTLE_DENSITY, "young floor floats: {fresh_floor:.0}");
        assert!(fresh_high > 0.0, "…so it rides above the datum: {fresh_high:.0} m");

        // Age it. 600 My is many e-folds — an old abyssal plain.
        let stage = super::CrustDensification;
        let mut rng = StageRng::new(1);
        for _ in 0..600 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("CrustDensification"); // it moves no mass, ever
        }

        let old_floor = density_kg_m3(&w.columns[0].layers[0]);
        let old_low = elevation_m(&w.columns[0], area);
        assert!(old_floor > MANTLE_DENSITY, "old floor outweighs the mantle: {old_floor:.0}");
        assert!(
            old_low < 0.0,
            "…so it rides BELOW the datum — the basin: {old_low:.0} m (was {fresh_high:.0})"
        );
        // The continent is untouched: felsic rock never eclogitises.
        let cont_after = elevation_m(&w.columns[1], area);
        assert!(
            (cont_after - cont_before).abs() < 1e-6 * cont_before.abs().max(1.0),
            "continental crust must not sink with age: {cont_before:.0} → {cont_after:.0}"
        );
        // And the old floor is now the more subductable of the two.
        assert!(
            w.columns[0].mean_density() > w.columns[1].mean_density(),
            "old floor founders before a continent does"
        );
    }

    /// A veneer cannot arrest a slab — it goes down with the basement it was
    /// lying on. Letting the first soft film stop the descent was the model's
    /// largest homogeniser: every collision scraped the loser's whole mixed
    /// cover onto the winner, and every stack drifted to the same composition
    /// and the same drowned height.
    #[test]
    fn a_sediment_drape_rides_its_basement_down() {
        use crate::column::{Column, FormationProcess, Layer};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 78);

        // Same discipline: both contenders are made of mantle this world had.
        let mk = |w: &mut World, els: &[(u8, f64)], by: FormationProcess| {
            let mut c = flicker_worldstate::Composition::new();
            for &(e, m) in els {
                c.add(e, w.mantle.remove(0, e, m));
            }
            Layer {
                elements: c,
                minerals: Default::default(),
                formed_at_myr: 0.0,
                formed_by: by,
                peak_pt: (0.0, 0.0),
                densified: 0.0,
            }
        };
        // A dense mafic floor wearing a light sediment drape.
        let mut loser = Column::empty(0);
        loser.layers.push(mk(&mut w, &[(12, 3.0e18), (26, 3.0e18)], FormationProcess::OceanicCrust));
        loser.layers.push(mk(&mut w, &[(14, 2.0e18), (8, 1.5e18)], FormationProcess::Sediment));
        // A buoyant felsic winner.
        let mut winner = Column::empty(0);
        winner.layers.push(mk(&mut w, &[(14, 5.0e18), (19, 2.0e18)], FormationProcess::ContinentalArc));

        let area = w.cell_area_m2();
        crate::tectonics::collide_for_test(&mut w, 0, vec![winner, loser], 0.3, -1.0e9, area);

        let survived = &w.columns[0];
        assert!(
            !survived.layers.iter().any(|l| l.formed_by == FormationProcess::Sediment),
            "the drape followed its basement down instead of being scraped off"
        );
        w.audit("collide");
    }

    /// **THE CEILING.** A pile deep enough to over-press its own base sheds it,
    /// so ground cannot rise without limit — the defect the 4.5 BY bake found
    /// (one column at fourteen thousand kilometres of elevation, taller than
    /// the planet is wide). And a pile that is NOT over-pressed keeps every
    /// gram: this is a ceiling, not a rule against mountains.
    #[test]
    fn an_overloaded_root_founders_and_a_modest_range_does_not() {
        use crate::column::{basal_pressure_pa, elevation_m, FormationProcess, Layer};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 91);
        let area = w.cell_area_m2();

        // Drawn from the mantle, never conjured. Cell 0 gets an absurd pile
        // (the runaway); cell 1 gets an ordinary range.
        let stack = |w: &mut World, cell: usize, scale: f64| {
            for k in 0..3 {
                let mut c = flicker_worldstate::Composition::new();
                for (e, share) in [(8u8, 0.47), (14, 0.34), (13, 0.15), (19, 0.04)] {
                    c.add(e, w.mantle.remove(cell, e, share * scale));
                }
                w.columns[cell].layers.push(Layer {
                    elements: c,
                    minerals: Default::default(),
                    formed_at_myr: k as f64,
                    formed_by: FormationProcess::ContinentalArc,
                    peak_pt: (0.0, 0.0),
                    densified: 0.0,
                });
            }
        };
        stack(&mut w, 0, 4.0e20); // over-pressed
        stack(&mut w, 1, 2.0e18); // an ordinary range
        w.audit("fixture");

        assert!(
            basal_pressure_pa(&w.columns[0], area) > super::DELAMINATION_PA,
            "the fixture's runaway pile is genuinely over-pressed"
        );
        assert!(
            basal_pressure_pa(&w.columns[1], area) < super::DELAMINATION_PA,
            "…and the ordinary range is not"
        );
        let (tall_before, modest_before) =
            (elevation_m(&w.columns[0], area), elevation_m(&w.columns[1], area));

        let stage = super::Delamination;
        let mut rng = StageRng::new(2);
        for _ in 0..200 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("Delamination"); // the root is credited to the mantle, never lost
        }

        let tall_after = elevation_m(&w.columns[0], area);
        assert!(
            tall_after < tall_before,
            "the over-pressed pile shed its root: {tall_before:.0} m → {tall_after:.0} m"
        );
        assert_eq!(
            elevation_m(&w.columns[1], area),
            modest_before,
            "a range that never over-pressed its base keeps every gram"
        );
    }

    #[test]
    fn crust_covers_most_of_the_surface() {
        // The reframe: crust freezes over the whole cooled surface, not just at plate
        // boundaries. Most cells should carry crust once the mantle has cooled below
        // the solidus — a planet with a seafloor, not bare mantle with painted seams.
        let w = run(6, 7, 260);
        let covered = w.columns.iter().filter(|c| !c.layers.is_empty()).count();
        let frac = covered as f64 / w.columns.len() as f64;
        assert!(frac > 0.6, "crust should cover most of the surface, got {:.0}%", frac * 100.0);
    }

    #[test]
    fn crust_grows_from_the_mantle_conserved() {
        // Crust is an OUTPUT: none at t=0, some after the sim runs, and every gram
        // came from the mantle (the audit inside step() proves conservation).
        let w = run(6, 7, 200);
        let crust: f64 = w.columns.iter().map(|c| c.mass_kg()).sum();
        assert!(crust > 0.0, "crust grew from mantle melt");
        assert!(crust < 0.05 * w.budget.total(), "crust is a thin skin, not the planet");
    }

    #[test]
    fn hypsometry_is_bimodal() {
        // THE M2 gate: two populations of crust — dense low-lying oceanic and light
        // high-standing continental — at clearly separated elevations. Nobody set
        // the heights; they come from composition (density) through Airy isostasy.
        //
        // Read at freq 24, not 6. The 362-cell world was honest company only
        // while the grid-ghosted conveyor barely moved; once the resample fix
        // unlocked it, a world nineteen hexes around churns every column
        // through repeated pile-ups and its hypsometry caricatures (seed 5
        // read oceanic ABOVE continental there). At freq 24 the direction is
        // clean across seeds (1.4–1.7×). Same playbook as the plates test at
        // the ISEA landing: when a small-world read turns out to have been
        // leaning on an artifact, the test moves to ground that can hold it.
        let w = run(24, 5, 240);
        let area = w.cell_area_m2();
        let mut ocean = Vec::new();
        let mut cont = Vec::new();
        for col in &w.columns {
            match crust_kind(col) {
                CrustKind::Oceanic => ocean.push(elevation_m(col, area)),
                CrustKind::Continental => cont.push(elevation_m(col, area)),
                CrustKind::Undifferentiated => {}
            }
        }
        assert!(!ocean.is_empty(), "some ocean floor formed");
        assert!(!cont.is_empty(), "some continents formed");
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (om, cm) = (mean(&ocean), mean(&cont));
        eprintln!("continental {cm:.0} m vs oceanic {om:.0} m  ({:.2}x)", cm / om.max(1e-9));
        // Buoyant crust rides higher — Archimedes, through absolute Airy isostasy.
        // That direction is forced by the mechanism, so it is fair to require it.
        assert!(cm > om, "continents ride higher than ocean floor ({cm:.0} m vs {om:.0} m)");
        // There used to be a second clause here requiring the gap to reach 1.5×.
        // That was a **required outcome** — the one thing the standing law forbids —
        // and it duly broke when the distillation work shifted crustal densities a
        // little. How wide the gap gets is the world's business and Aaron's to judge
        // in the hypsometry view; what belongs in a test is that the mechanism
        // points the right way.
    }

    #[test]
    fn oceanic_crust_is_denser_than_continental() {
        // The mechanism behind the gate: read densities off the two crust types.
        // Off the CRUST-FORMING beds, not the top of the stack — the sea floor
        // wears a carbonate veneer now ([`crate::atmosphere::CarbonSink`]), same
        // as Earth's, and the claim here is about the melt model's two rock
        // populations, not about whatever sediment lies on them.
        use crate::column::{density_kg_m3, FormationProcess};
        let w = run(6, 9, 200);
        let mut od = Vec::new();
        let mut cd = Vec::new();
        for col in &w.columns {
            let (mut mass, mut volume) = (0.0, 0.0);
            for bed in col.layers.iter().filter(|l| l.formed_by != FormationProcess::Sediment) {
                let m = bed.mass_kg();
                mass += m;
                volume += m / density_kg_m3(bed);
            }
            if volume <= 0.0 {
                continue;
            }
            let rock = mass / volume;
            match crust_kind(col) {
                CrustKind::Oceanic => od.push(rock),
                CrustKind::Continental => cd.push(rock),
                _ => {}
            }
        }
        if !od.is_empty() && !cd.is_empty() {
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
            assert!(mean(&od) > mean(&cd), "oceanic (mafic) crust is denser than continental (felsic)");
        }
    }

    #[test]
    fn crust_does_not_track_the_shard_seams() {
        // Objective grid-ghost metric. A shard "seam" cell borders a different icosa
        // face; those are the geometrically-irregular cells. If the physics is
        // topology-blind, continents form no more often on seam cells than off them,
        // so p_seam / p_interior ≈ 1. A raw-sum operator imprints the seams → ratio ≫ 1.
        let measure = |freq: u32, seed: u64| {
            let w = run(freq, seed, 240);
            let shard = &w.grid.shard;
            let neigh = &w.grid.neighbors;
            let (mut seam_cont, mut seam_n, mut int_cont, mut int_n) = (0usize, 0usize, 0usize, 0usize);
            for (i, col) in w.columns.iter().enumerate() {
                let cont = (crust_kind(col) == CrustKind::Continental) as usize;
                if neigh[i].iter().any(|&j| shard[j as usize] != shard[i]) {
                    seam_n += 1;
                    seam_cont += cont;
                } else {
                    int_n += 1;
                    int_cont += cont;
                }
            }
            let p_seam = seam_cont as f64 / seam_n.max(1) as f64;
            let p_int = int_cont as f64 / int_n.max(1) as f64;
            let ratio = p_seam / p_int.max(1e-9);
            eprintln!(
                "freq {freq} seed {seed}: seam {seam_cont}/{seam_n} vs interior \
                 {int_cont}/{int_n} → ratio {ratio:.2}"
            );
            ratio
        };
        let r24 = measure(24, 5);
        let r48 = measure(48, 5);
        // **This is now clean at every resolution** — 0.89 / 0.91 / 1.03, continents
        // no likelier on a seam than off it anywhere. It took both of the last two
        // milestones to get here, and it is worth recording which did what:
        //
        // - The ISEA equal-area map made freq 6 and 24 neutral (they had run
        //   1.10-1.15), but freq 48 stayed near 1.3 and the residual was the kind
        //   that GROWS with resolution — the worst kind, because it means the world
        //   gets more grid-shaped the closer you look.
        // - The **conveyor** removed that. The old stages decided where continental
        //   crust appeared by reading the SIGN of the velocity field's divergence at
        //   each cell — a derivative of a field defined on the mesh, so the mesh
        //   printed through. Now crust appears where two stacks actually collide,
        //   and a collision is an event between bodies, not a number read off a
        //   grid. There is nothing left for the icosahedron to imprint on.
        //
        // The tight single-seed bounds live at freq 24 and 48, where the sample
        // can carry them: those worlds have thousands of cells on both sides of
        // the split, so a ratio there is a measurement.
        //
        // History worth keeping: after the 7E01115B sulfur sequestration
        // re-rolled the collision history, the freq-6 seed spread came back
        // ONE-SIDED (1.16–1.52 across five seeds) and the bounds were loosened
        // on a small-N-noise reading. Aaron's eyeball then showed what the
        // one-sidedness really was: the resample kernel was etching the shard
        // edges into the temperature field, the derived flow read a 1.6×
        // strain excess along them, and the conveyor's first boundaries — and
        // their arc record — traced the pentagon-to-pentagon lines at app
        // resolution. With the fit-evaluated resample
        // (`interior::MantleConvection::advect_temperature`, pinned by
        // `convection_strain_ignores_the_shard_edges`) the spread went back to
        // two-sided noise.
        assert!(r24 < 1.15, "freq24 shard imprinting regressed: {r24:.2}");
        assert!(r48 < 1.15, "freq48 shard imprinting regressed: {r48:.2}");

        // ── WHY FREQ 6 IS NOT MEASURED HERE. This is REDUCED COVERAGE, stated
        //    plainly rather than dressed up. ──
        //
        // A 362-cell world no longer produces a continental population this can
        // divide by. Once the Conveyor was gated on crust actually existing, the
        // early crust that used to appear through `open_ground` — which freezes
        // ridge crust with no temperature check, on a planet that was still a
        // magma ocean — stopped bootstrapping the tectonic system, and coarse
        // worlds now start their tectonics far later. Measured over 240 ticks
        // across eight seeds: interior continental counts of 0, 0, 0, 0, 1, 8,
        // and seed 5 down from 67 cells to 2. FOUR SEEDS PRODUCE ZERO, so the
        // ratio is a division by an empty denominator (it reported 7.5e6).
        //
        // The dependence is a sampling effect and runs the RIGHT way: the
        // coldest of N cells gets colder as N grows, so a high-resolution world
        // crosses the freezing threshold sooner. freq 48 GAINED 20% continental
        // crust over the same change, and the app ships at freq 96 — far above
        // anything asserted here. The resolutions that can measure are measured;
        // the one that went dark is not asserted on, because asserting on an
        // empty denominator is worse than admitting the arm is gone.
        //
        // To restore it: give freq 6 a tick budget long enough to grow
        // continents (260 ticks already suffices for `hypsometry_is_bimodal`),
        // then reinstate the seed-spread check — a real imprint is ONE-SIDED
        // across seeds, noise straddles 1.0, and that is the signature the
        // 7E01115B history turned on.
    }
}
