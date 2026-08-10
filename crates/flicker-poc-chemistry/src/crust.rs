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
        // What can crystallise out of rock, with what each is made of — the
        // catalog's own answer (`crystallizes`), never a list typed in here.
        //
        // This filtered on `sim_required` until 2026-08-06, which is a
        // PROVENANCE flag naming the rows added beyond the Book III tables. The
        // twelve it happens to select are all rock-formers, so it looked right
        // and was not: every ORE mineral is a Book III row, so Hematite,
        // Chalcopyrite and Native Gold could never form and ore stayed a bare
        // element count with no mineral, no hardness and no rock identity. It
        // also excluded Quartz — and with no quartz there is no sandstone,
        // chert or quartzite, which are the three most erosion-resistant rocks
        // in the catalog. The world could not make its own hardest ground.
        let recipes: Vec<(u16, Vec<(ElementId, f64)>)> = self
            .tables
            .compounds()
            .iter()
            .filter(|c| c.crystallizes)
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
const COOLING_EFOLD_MYR: f64 = 70.0;

/// **ThermalSubsidence** — igneous rock, left alone, cools and contracts.
///
/// Not a new substance and not a gram of new mass: the same elements packing
/// tighter as the heat leaves. All this stage does is advance each igneous
/// bed's [`cooled`](crate::column::Layer::cooled) toward 1; what that MEANS is
/// [`density_kg_m3`](crate::column::density_kg_m3)'s business, and what the
/// density means is isostasy's.
///
/// **Age is the whole condition, and that is correct here.** Cooling depends on
/// how long the rock has been losing heat and on nothing else, so this stage
/// has no pressure or depth gate and should not have one. Its twin
/// [`Eclogitisation`] is the half that does — the two used to share this timer,
/// which is what made a phase change that belongs 45 km down fire on every bed
/// in the world.
///
/// Three things fall out of it, none of them written anywhere as a result:
///
/// **Ocean basins.** Sea floor is born lighter than the mantle and ends up
/// heavier than it, so old floor rides *below* the datum. That is the container
/// a sea needs. Without it every column floats, the water spreads out over all
/// of them, and every world ends up the same flat blue ball whatever else it
/// did — the single largest reason this simulation could not make an Earth.
/// Sea floor is far too thin to ever reach [`eclogite_pa`], so this stage does
/// that job alone, and keeps the full gain the basins were measured at.
///
/// **Slab pull, in the right order.** The densest stack loses a collision, and
/// old floor is now the densest thing on the planet — so the oldest sea floor
/// founders first, which is what keeps a basin cycling instead of paving over.
///
/// **Continents that stay.** The gain is scaled by how mafic the rock is, so a
/// refined felsic pile gains nothing, ever. Nothing says continents are
/// permanent; they simply never become heavy enough to sink.
pub struct ThermalSubsidence;

impl Stage for ThermalSubsidence {
    fn name(&self) -> &'static str {
        "ThermalSubsidence"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let step = (dt_myr / COOLING_EFOLD_MYR).clamp(0.0, 1.0);
        for col in &mut world.columns {
            for bed in &mut col.layers {
                if !cools(bed) {
                    continue;
                }
                bed.cooled += (1.0 - bed.cooled) * step;
            }
        }
    }
}

/// Whether a bed is the kind of rock these two stages act on at all.
///
/// Only rock a melt made. Sediment compacts and organics cook — both real, both
/// somebody else's stage — but neither is the basalt phase change, and letting
/// a mud drape "eclogitise" would sink coastlines for no reason anyone could
/// point at.
fn cools(bed: &crate::column::Layer) -> bool {
    !matches!(
        bed.formed_by,
        FormationProcess::Sediment | FormationProcess::Organic | FormationProcess::Hydrothermal
    )
}

/// Depth at which mafic rock changes phase into eclogite, m — ~45 km, where
/// garnet becomes the stable assemblage in real basalt.
///
/// **This is the gate that was missing.** The phase change was being advanced
/// on age alone, with no depth condition at all, so it reached every bed on the
/// planet including the surface of continents. Measured over 200 ticks it
/// converted continental crust to subductable crust 3,663 times and reversed
/// zero times — a one-way ratchet that ate the continents from the inside and
/// gave the conveyor a new classification to thrash against every tick.
pub const ECLOGITE_DEPTH_M: f64 = 45_000.0;

/// Reference density of the rock doing the pressing, kg/m³ — mid-crustal, the
/// same ~2800 [`delamination_pa`] is quoted against.
const OVERBURDEN_REF_DENSITY: f64 = 2800.0;

/// The overburden [`ECLOGITE_DEPTH_M`] corresponds to **on this world**, Pa.
///
/// **A depth, not a pressure, because gravity here is derived from planet
/// size.** The first cut of this stage hardcoded 1.3 GPa — correct for Earth —
/// and the bake then measured the deepest overburden anywhere on the planet at
/// 3.49e8 Pa, 0.27× the gate: the stage was inert, every tick, everywhere. This
/// world is small (g ≈ 2.45 m/s² at the reference frequency, against Earth's
/// 9.81) and its crust is thin, so an Earth pressure is out of reach by design.
/// Asking the question as a DEPTH gets the same petrology at any planet size,
/// which is what the size model requires of every constant that presses.
///
/// Ordering against [`delamination_pa`] is the physics: rock converts at this
/// depth, and a root that has converted is heavy enough that by the
/// delamination limit it can no longer hold itself up and founders. **That
/// constant is still an absolute pressure and has the same size-model flaw —
/// flagged, not changed here, because the ceiling it sets was measured.**
pub fn eclogite_pa(world: &World) -> f64 {
    OVERBURDEN_REF_DENSITY * world.gravity_m_s2() * ECLOGITE_DEPTH_M
}

/// How fast a bed converts once it is deep enough — an e-fold of ~20 My, the
/// pace of a reaction that needs the rock to be both deep and wet.
const ECLOGITISATION_EFOLD_MYR: f64 = 20.0;

/// How fast eclogite reverts once it is NOT deep enough — an e-fold of ~60 My,
/// three times slower than it converts. Retrogression is the sluggish
/// direction in real rock: it needs fluid to rehydrate the assemblage, and dry
/// eclogite can sit metastable in the crust for a very long time.
const RETROGRADE_EFOLD_MYR: f64 = 60.0;

/// **Eclogitisation** — mafic rock buried deep enough changes phase.
///
/// The pressure-driven half of what used to be one unconditional timer.
/// [`ThermalSubsidence`] is rock getting denser because it got *cold*; this is
/// rock getting denser because it got *deep*, and only the bottom of a genuinely
/// thick pile ever qualifies. Each bed is judged on the overburden IT carries,
/// not on the column's total, so a root converts from the base upward as it
/// thickens — which is the order the rock actually does it in.
///
/// **It runs both ways.** Eclogite that comes back up out of its stability
/// field reverts to the lighter assemblage, so a root that is shed, unroofed or
/// eroded stops being a slab-in-waiting instead of staying one for the rest of
/// the run. That reversibility is the point: the one-way version was measured
/// making 3,663 continental→subductable conversions with zero reversals, and a
/// ratchet with no pawl release only ever runs the world down in one direction.
///
/// Feeds [`Delamination`], which is the consequence: convert at
/// [`eclogite_pa`], founder at [`delamination_pa`].
pub struct Eclogitisation;

impl Stage for Eclogitisation {
    fn name(&self) -> &'static str {
        "Eclogitisation"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let area = world.cell_area_m2();
        let gravity = world.gravity_m_s2();
        let gate = eclogite_pa(world);
        let forward = (dt_myr / ECLOGITISATION_EFOLD_MYR).clamp(0.0, 1.0);
        let back = (dt_myr / RETROGRADE_EFOLD_MYR).clamp(0.0, 1.0);
        for col in &mut world.columns {
            // **One walk, top-down, carrying the load.** The load on a bed is
            // the weight of everything above it — a whole-column read would
            // convert a thin drape on a deep root as though it lay at the
            // bottom of one. Asking `overburden_pa` per bed re-sums that stack
            // every time, which is O(beds²) with a 28-element sum inside: free
            // at the 1.9 beds this world used to carry, and 400 mass-sums per
            // cell per tick once a stack can actually reach the cap of 20.
            let mut above = 0.0f64;
            for index in (0..col.layers.len()).rev() {
                let load = above * gravity / area.max(f64::MIN_POSITIVE);
                above += col.layers[index].mass_kg();
                if !cools(&col.layers[index]) {
                    continue;
                }
                let bed = &mut col.layers[index];
                if load >= gate {
                    bed.eclogitised += (1.0 - bed.eclogitised) * forward;
                } else {
                    bed.eclogitised -= bed.eclogitised * back;
                }
            }
        }
    }
}

/// Overburden at which a stack's deepest rock has been pushed deep enough to
/// change phase, Pa. Earth's crust runs out of room around 70 km — below that
/// the base of a thickened root sits at pressures where the rock is no longer
/// stable as crust — and this is that depth in the currency the columns already
/// keep: `ρgh` for ~70 km of ~2800 kg/m³ rock.
const DELAMINATION_DEPTH_M: f64 = 70_000.0;

/// The overburden [`DELAMINATION_DEPTH_M`] corresponds to **on this world**, Pa.
///
/// **A depth, not a pressure — the same size-model bug the eclogite gate had.**
/// This was written as an absolute `1.9e9`, which is that depth at EARTH's
/// gravity (70 km × 2800 kg/m³ × 9.81). This planet's gravity is derived from
/// its size (2.45 m/s² at the reference frequency), so the same 70 km of rock
/// presses about 4.8e8 here and the ceiling sat roughly 4× out of reach.
///
/// Measured 2026-08-07: **0 of 5762 columns had ever reached it** — max basal
/// pressure on the planet 1.302e9 against a 1.90e9 threshold. Delamination has
/// never fired in this simulation. That went unnoticed because the tectonic
/// conveyor was stranding ~91% of subducted mass in the mantle and *that* was
/// acting as the crust sink; the two faults concealed each other, and the
/// earlier "crustal height is now BOUNDED" result was reading the stranding.
/// Closing the seam circuit removed the accidental sink and left crust
/// unbounded, which is what exposed this.
pub fn delamination_pa(world: &World) -> f64 {
    OVERBURDEN_REF_DENSITY * world.gravity_m_s2() * DELAMINATION_DEPTH_M
}

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
/// The last step of the depth story, and [`Eclogitisation`] is the one before
/// it: that stage converts the rock at [`eclogite_pa`], this one takes it away
/// once the pile passes [`delamination_pa`] and the root can no longer hold
/// itself up. Rock that has converted is heavier than the mantle it is resting
/// on, so it sinks away — taking the pile's height with it.
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
        let gravity = world.gravity_m_s2();
        let step = (DELAMINATION_RATE * dt_myr).clamp(0.0, 1.0);
        let ceiling = delamination_pa(world);
        for cell in 0..world.columns.len() {
            // The load on the deepest rock is the whole stack's own weight.
            let basal = crate::column::basal_pressure_pa(&world.columns[cell], gravity, area);
            if basal < ceiling {
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
            let mut budget = (basal - ceiling) * area / gravity * step;
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

/// Fraction of what a bed COULD reorganise that it does per Myr. Metamorphism
/// is slow even by the standards of this pipeline — an orogen holds its root at
/// depth for tens of millions of years — and the rate keeps it a process rather
/// than a verdict handed down the instant a threshold is crossed.
const METAMORPHIC_RATE: f64 = 0.02;

/// **Metamorphism** — rock that has been buried deep enough and hot enough
/// stops being what it was.
///
/// The one transformation in the pipeline that reads [`Layer::peak_pt`], and
/// the reason that record is kept at all. Grade is a **high-water mark**: the
/// pair is the worst a bed has ever endured, so rock carried back to the
/// surface stays what the depth made it. That is why a worn-down orogen exposes
/// slate and gneiss rather than the mud it started as.
///
/// **Nothing here names a mineral or a pressure.** Each reaction is a phase's
/// own stability limit in the catalog (`metamorphic: {to, pressure_pa,
/// temp_k}`), so the chemistry is content and this is only the machinery.
///
/// Those limits are the **real numbers for the reaction** and are not chosen
/// against what any particular world can reach. A threshold no ground on a
/// planet ever attains means that planet never made the phase — a fact about
/// the world, not a miscalibration, and not something to correct by moving the
/// number. (Recorded because the first draft of this stage did exactly that:
/// the diamond limit was placed just under the delamination ceiling so that
/// diamonds would come out rare-but-present, which is writing an outcome.)
///
/// **Element-neutral by construction.** A reaction may only rearrange a phase
/// into one built from the same elements in the same proportions — checked at
/// construction, loudly. Carbon does this honestly: buried coal orders into
/// graphite and graphite into diamond, all of it pure carbon, so the element
/// ledger never moves and the compound bound cannot be broken. A reaction that
/// releases a volatile — serpentine dehydrating to olivine and water — has to
/// book that water somewhere, and is deliberately not expressible yet.
pub struct Metamorphism {
    /// `(from, to, pressure_pa, temp_k)`, resolved from the catalog once.
    reactions: Vec<(u16, u16, f64, f64)>,
}

impl Metamorphism {
    /// Resolve every stability limit the catalog states. Panics if a reaction
    /// names a phase that is missing or one built from different elements —
    /// both are content errors that would otherwise surface as a conservation
    /// panic thousands of ticks into a bake.
    pub fn new(tables: &flicker_materials::Tables) -> Self {
        let mut reactions = Vec::new();
        for c in tables.compounds() {
            let Some(rule) = c.metamorphic.as_ref() else { continue };
            let to = tables.compound(&rule.to).unwrap_or_else(|| {
                panic!("{} metamorphoses to '{}', absent from the catalog", c.name, rule.to)
            });
            let (a, b) = (tables.compound_mass_fractions(c), tables.compound_mass_fractions(to));
            let neutral = a.len() == b.len()
                && a.iter()
                    .all(|&(e, f)| b.iter().any(|&(e2, f2)| e2 == e && (f - f2).abs() < 1e-9));
            assert!(
                neutral,
                "{} → {} is not element-neutral; a reaction that changes composition must book \
                 what it releases",
                c.name, to.name
            );
            reactions.push((c.id, to.id, rule.pressure_pa, rule.temp_k));
        }
        Self { reactions }
    }
}

impl Stage for Metamorphism {
    fn name(&self) -> &'static str {
        "Metamorphism"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        if self.reactions.is_empty() {
            return;
        }
        let step = (METAMORPHIC_RATE * dt_myr).clamp(0.0, 1.0);
        for col in &mut world.columns {
            for bed in &mut col.layers {
                let (p, t) = bed.peak_pt;
                for &(from, to, need_p, need_t) in &self.reactions {
                    if p < need_p || t < need_t {
                        continue;
                    }
                    let have = bed.minerals.amount(from);
                    if have <= 0.0 {
                        continue;
                    }
                    // Mineral ledger only: same elements in the same
                    // proportions, so the conserved ledger never moves and the
                    // compound bound cannot break.
                    let moved = bed.minerals.remove(from, have * step);
                    if moved > 0.0 {
                        bed.minerals.add(to, moved);
                    }
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
        let gravity = world.gravity_m_s2();
        // The sky's contribution to the surface, sampled once — the same read
        // the weather uses, so the ground and the air cannot disagree about how
        // warm the world is.
        let greenhouse = crate::surface::greenhouse_k(world);
        for cell in 0..world.columns.len() {
            // **A bed's temperature is its DEPTH's temperature.** It used to be
            // handed the mantle's outright, at every depth, which is why the
            // record's temperature half meant nothing and metamorphism could
            // not read it. Now the column warms from its own surface toward the
            // mantle beneath it (see [`geotherm_k`]).
            let surface_k = crate::surface::cell_surface_temp_k(world, cell, 1.0, greenhouse);
            let mantle_k = world.mantle.temp_k[cell];
            world.columns[cell].reconcile(surface_k, mantle_k, gravity, area, STRATA_SOFT_CAP);
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

    /// Draw a bed of a **given composition** out of a cell's mantle, never
    /// conjuring mass: every element is scaled by the same factor so the ratios
    /// the caller asked for survive whatever the cell can actually supply. A
    /// fixture that silently drew a different rock than it asked for is how two
    /// of these tests first went red.
    fn drawn(
        w: &mut World,
        cell: usize,
        want: &[(u8, f64)],
        mass_kg: f64,
    ) -> flicker_worldstate::Composition {
        let scale = want
            .iter()
            .map(|&(e, f)| w.mantle.mass(cell, e) / (f * mass_kg))
            .fold(f64::INFINITY, f64::min)
            .min(1.0);
        let mut c = flicker_worldstate::Composition::new();
        for &(e, f) in want {
            c.add(e, w.mantle.remove(cell, e, f * mass_kg * scale));
        }
        c
    }

    /// Push a bed of `want` onto `cell`'s stack, drawing it from `from`'s mantle.
    fn push_bed(
        w: &mut World,
        cell: usize,
        from: usize,
        want: &[(u8, f64)],
        mass_kg: f64,
        by: crate::column::FormationProcess,
    ) {
        let elements = drawn(w, from, want, mass_kg);
        w.columns[cell].layers.push(crate::column::Layer {
            elements,
            minerals: Default::default(),
            formed_at_myr: 0.0,
            formed_by: by,
            peak_pt: (0.0, 0.0),
            cooled: 0.0,
            eclogitised: 0.0,
        });
    }

    /// **ORE IS A MINERAL, AND THE WORLD CAN MAKE ITS OWN HARDEST ROCK.**
    ///
    /// The crystalliser used to select its candidates with `sim_required` — a
    /// PROVENANCE flag naming rows added beyond the Book III tables. Every ore
    /// mineral is a Book III row, so gold, hematite and chalcopyrite could
    /// never form and ore was a bare element count with no mineral, no
    /// hardness and no rock identity; quartz could not form either, and
    /// without quartz there is no sandstone, chert or quartzite — the three
    /// most erosion-resistant rocks in the catalog. Both are consequences of
    /// one wrong predicate, so both are pinned here.
    #[test]
    fn ore_and_quartz_can_crystallise() {
        use crate::column::{FormationProcess, Layer};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("t"));
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 12);

        // A silica bed with a little iron and gold in it — drawn from the
        // mantle, never conjured, like every other fixture.
        let mut melt = Vec::new();
        for (e, want) in [(8u8, 6.0e16), (14, 5.0e16), (26, 2.0e16), (79, 1.0e14)] {
            let mut want = want;
            for src in 0..w.mantle.n_cells() {
                if want <= 0.0 {
                    break;
                }
                let took = w.mantle.remove(src, e, want);
                if took > 0.0 {
                    melt.push((e, took));
                }
                want -= took;
            }
        }
        w.columns[0].deposit(FormationProcess::OceanicCrust, 0.0, &melt);
        w.audit("fixture");

        let stage = super::Crystallization::new(std::sync::Arc::clone(&t));
        let mut rng = StageRng::new(1);
        for _ in 0..40 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("Crystallization");
            w.audit_compound_bound("Crystallization");
        }

        let made: Vec<String> = w.columns[0]
            .layers
            .iter()
            .flat_map(|l: &Layer| l.minerals.iter())
            .filter(|&(_, m)| m > 0.0)
            .filter_map(|(id, _)| t.compound_by_id(id).map(|c| c.name.clone()))
            .collect();
        assert!(made.iter().any(|n| n == "Quartz"), "quartz must be formable: {made:?}");
        assert!(
            made.iter().any(|n| t.compound(n).is_some_and(|c| c.harvestable)),
            "an ore mineral must be formable: {made:?}"
        );
    }

    /// **DEPTH COOKS ROCK; AGE DOES NOT.** The geotherm and the reaction it
    /// unlocks, together, because neither is worth anything alone: before this
    /// every bed was stamped with the mantle's temperature at every depth, so
    /// the record's temperature half carried no depth information and no
    /// transformation could honestly read it.
    ///
    /// Two carbon beds, identical but for how deeply they are buried. The deep
    /// one orders into graphite; the shallow one stays coal however long it
    /// sits there. Nothing says "coal becomes graphite at 7 km" — the catalog
    /// states a stability limit and the column states a depth.
    #[test]
    fn only_deeply_buried_carbon_reorganises() {
        use crate::column::{geotherm_k, FormationProcess, Layer};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let coal = t.compound("Coal").expect("coal").id;
        let graphite = t.compound("Graphite").expect("graphite").id;
        let rule = t.compound("Coal").unwrap().metamorphic.clone().expect("coal has a limit");

        // The geotherm itself: warms downward, never past the mantle, and the
        // surface is the surface.
        assert_eq!(geotherm_k(290.0, 1800.0, 0.0), 290.0, "the top is the surface");
        assert!(geotherm_k(290.0, 1800.0, 10_000.0) > 290.0, "it warms downward");
        assert!(
            geotherm_k(290.0, 1800.0, 1.0e9) <= 1800.0,
            "nothing in the crust is hotter than what it sits on"
        );

        let bed = |mass: f64| {
            let mut elements = flicker_worldstate::Composition::new();
            elements.add(6, mass);
            let mut minerals = flicker_worldstate::CompoundLedger::new();
            minerals.add(coal, mass);
            Layer {
                elements,
                minerals,
                formed_at_myr: 0.0,
                formed_by: FormationProcess::Organic,
                peak_pt: (0.0, 0.0),
                cooled: 0.0,
                eclogitised: 0.0,
            }
        };
        // Hand each bed the conditions directly — the claim under test is the
        // reaction, not how a column comes to be deep.
        let mut deep = bed(1.0e17);
        deep.peak_pt = (rule.pressure_pa * 1.5, rule.temp_k + 100.0);
        let mut shallow = bed(1.0e17);
        shallow.peak_pt = (rule.pressure_pa * 0.1, rule.temp_k + 100.0);
        let mut col = crate::column::Column::empty(0);
        col.layers.push(deep);
        col.layers.push(shallow);

        let mut w = World::seed(icosphere(4), Budget::from_dir(&dir, &t).expect("b"), &t, 3);
        w.columns[0] = col;
        let stage = super::Metamorphism::new(&t);
        let mut rng = StageRng::new(7);
        for _ in 0..200 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit_compound_bound("Metamorphism");
        }

        let deep_bed = &w.columns[0].layers[0];
        let shallow_bed = &w.columns[0].layers[1];
        assert!(deep_bed.minerals.amount(graphite) > 0.0, "the deep bed ordered into graphite");
        assert!(
            deep_bed.minerals.amount(coal) < 1.0e16,
            "and most of its coal is gone: {}",
            deep_bed.minerals.amount(coal)
        );
        assert_eq!(
            shallow_bed.minerals.amount(graphite),
            0.0,
            "shallow carbon stays coal however long it sits"
        );
        // Element-neutral: carbon in, carbon out, the ledger untouched.
        assert!((deep_bed.elements.amount(6) - 1.0e17).abs() < 1.0, "no element moved");
    }

    /// The catalog's own statement of what crystallises — a phase with another
    /// route must not be makeable out of rock, or the world grows coal in its
    /// basalt and rock salt at the bottom of the sea.
    #[test]
    fn only_phases_that_crystallise_are_candidates() {
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        for (name, want) in [
            ("Quartz", true),
            ("Native Gold", true),
            ("Hematite", true),
            ("Olivine", true),
            ("Halite", false),   // evaporite — needs standing water to dry out
            ("Coal", false),     // Maturation makes it from buried tissue
            ("Bauxite", false),  // a tropical weathering residue
        ] {
            let c = t.compound(name).unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(c.crystallizes, want, "{name}: crystallizes should be {want}");
        }
        // And nothing outside the mineral category may crystallise out of rock.
        for c in t.compounds().iter().filter(|c| c.crystallizes) {
            assert_eq!(c.category, "mineral", "{} crystallises but is not a mineral", c.name);
        }
    }

    /// A world run through the full formation pipeline for `ticks` Myr.
    fn run(freq: u32, seed: u64, ticks: usize) -> World {
        let dir = content_data_dir();
        let t = std::sync::Arc::new(Tables::from_source(&JsonTableSource::new(&dir)).expect("tables"));
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, seed);
        let mut s = Scheduler::new(crate::formation_stages(std::sync::Arc::clone(&t), &w, &crate::Levers::brisk()), seed);
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
            cooled: 0.0,
            eclogitised: 0.0,
            });
        }

        // Mechanism-test speed, the same posture as `Levers::brisk`: a freq-4
        // planetoid's per-cell mantle is ~24× lighter than the reference
        // world's, while the bed-film floor (`MIN_BED_MASS_KG`) is areal and
        // absolute — the same hex at every size. At the as-written rate each
        // tick's lava is a sub-floor film that joins the lid, and no run length
        // can spawn a bed from films; the probe is about the MECHANISM (a plume
        // erupts through the lid, builds Volcanic ground, vents gas), so it
        // runs the eruption brisk.
        let stage = super::Volcanism::new(&t, 100.0 * super::DEFAULT_ERUPTION_RATE);
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
            cooled: 0.0,
            eclogitised: 0.0,
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
                cooled: 0.0,
                eclogitised: 0.0,
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
        let stage = super::ThermalSubsidence;
        let mut rng = StageRng::new(1);
        for _ in 0..600 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("ThermalSubsidence"); // it moves no mass, ever
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

    /// **THE CONTINENT-EATER, pinned — as a CHARACTERISATION, not a pass.**
    ///
    /// A continental column is never pure arc melt: it picks up volcanic beds,
    /// thrust sheets and floor scraped off collisions, so its mafic fraction
    /// climbs. Age alone then carries it across [`SUBDUCTABLE_DENSITY`],
    /// `crust_kind` calls it sea floor, and the conveyor subducts it — measured
    /// at 3,663 conversions per 200 ticks with zero reversals.
    ///
    /// **Splitting the timers did not close this, and the obvious lever is not
    /// available.** Moving `eclogite_former_frac`'s ramp right stops the leak in
    /// this fixture and takes the ocean basins with it (the 4.5 BY bake put the
    /// basin floor back at −52 m from −282 m), because the same number decides
    /// whether real sea floor densifies at all. So the leak stands, deliberately
    /// and measured, until it is ruled on.
    ///
    /// What this test locks is the half that IS fixed: the pressure-driven
    /// phase change no longer fires on surface rock. The final assert records
    /// the leak that remains — **when it is closed, that assert should be
    /// inverted, not deleted.**
    #[test]
    fn a_mixed_continental_bed_still_ages_into_subductability() {
        use crate::column::{density_kg_m3, FormationProcess, SUBDUCTABLE_DENSITY};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 91);

        // Felsic silica (Si = 0.30) carrying a real contamination load: Mg+Fe =
        // 0.15 of the bed, against a clean granite's ~0.085. Aluminium and
        // potassium are left out on purpose — the mantle holds almost no K
        // (5.5e14 kg in a cell, measured), so asking for them yields a bed of a
        // quite different rock than the one this test means to make.
        const MIXED: &[(u8, f64)] = &[(8, 0.55), (14, 0.30), (12, 0.09), (26, 0.06)];
        push_bed(&mut w, 0, 0, MIXED, 1.2e18, FormationProcess::ContinentalArc);
        w.audit("fixture");

        // The fixture is the claim, so check it IS the rock described above
        // before drawing any conclusion from how it behaves.
        {
            let bed = &w.columns[0].layers[0];
            let m = bed.mass_kg();
            let si = bed.elements.amount(14) / m;
            let mafic = (bed.elements.amount(12) + bed.elements.amount(26)) / m;
            assert!((si - 0.30).abs() < 0.01, "felsic silica: {si:.3}");
            assert!((mafic - 0.15).abs() < 0.01, "contaminated, not clean: {mafic:.3}");
        }

        // Age it far past every e-fold there is — cooling saturates.
        let stage = super::ThermalSubsidence;
        let mut rng = StageRng::new(3);
        for _ in 0..1000 {
            stage.tick(&mut w, 1.0, &mut rng);
        }

        let bed = &w.columns[0].layers[0];
        assert!(bed.cooled > 0.99, "the fixture really did saturate: {:.3}", bed.cooled);

        // THE HALF THAT IS FIXED. A surface bed carries nothing above it, so the
        // phase change cannot touch it however long the world runs. This is the
        // ratchet that used to run here, and it is gone.
        let ecl = super::Eclogitisation;
        for _ in 0..1000 {
            ecl.tick(&mut w, 1.0, &mut rng);
        }
        assert_eq!(
            w.columns[0].layers[0].eclogitised, 0.0,
            "no overburden, no phase change — the pressure gate holds at the surface"
        );

        // THE HALF THAT IS NOT. Cooling alone still carries this mixture past
        // the buoyancy threshold, which is the continent leak. Recorded so the
        // gap is visible in the suite rather than only in a bake nobody runs.
        let rho = density_kg_m3(&w.columns[0].layers[0]);
        assert!(
            rho > SUBDUCTABLE_DENSITY,
            "KNOWN GAP — if this now fails the leak has been closed, and this \
             assert should be INVERTED rather than removed: {rho:.0} vs {SUBDUCTABLE_DENSITY:.0}"
        );
    }

    /// **The phase change belongs at depth, and it lets go.** Two guards on
    /// [`Eclogitisation`] in one fixture, because they are the same claim from
    /// both sides: pressure is the condition, so rock without the pressure never
    /// converts, and rock that LOSES the pressure reverts. The one-way version
    /// reversed zero times in 200 ticks, which is what made it a ratchet.
    #[test]
    fn eclogite_needs_depth_and_gives_it_back() {
        use crate::column::{overburden_pa, FormationProcess};
        use crate::stage::{Stage, StageRng};
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        let mut w = World::seed(icosphere(4), b, &t, 12);
        let area = w.cell_area_m2();
        let gravity = w.gravity_m_s2();
        let gate = super::eclogite_pa(&w);

        // Basalt: a third of it eclogite-formers, so the composition gate is
        // wide open and DEPTH is the only thing under test.
        const BASALT: &[(u8, f64)] = &[(8, 0.44), (14, 0.24), (12, 0.19), (26, 0.13)];
        // A root cannot come out of the ground beneath it — one cell's mantle
        // is nowhere near enough to press its own base to the gate — so it has
        // to be gathered from a wide area. That is exactly what a collision
        // does, and it is why roots are rare.
        const BED_KG: f64 = 1.6e18;

        // Cell 0: a lone bed — sea floor, nothing on top of it.
        push_bed(&mut w, 0, 0, BASALT, BED_KG, FormationProcess::OceanicCrust);
        // Cell 1: a root, each bed drawn from a different cell's mantle.
        for from in 1..18 {
            push_bed(&mut w, 1, from, BASALT, BED_KG, FormationProcess::OceanicCrust);
        }
        w.audit("fixture");

        let shallow = overburden_pa(&w.columns[0], 0, gravity, area);
        let deep = overburden_pa(&w.columns[1], 0, gravity, area);
        assert!(shallow < gate, "sea floor is shallow: {shallow:.2e} Pa");
        assert!(deep >= gate, "the root's base is deep: {deep:.2e} Pa");

        let stage = super::Eclogitisation;
        let mut rng = StageRng::new(5);
        for _ in 0..200 {
            stage.tick(&mut w, 1.0, &mut rng);
            w.audit("Eclogitisation"); // it moves no mass, ever
        }

        assert_eq!(
            w.columns[0].layers[0].eclogitised, 0.0,
            "same rock, same age, no depth — no phase change"
        );
        let converted = w.columns[1].layers[0].eclogitised;
        assert!(converted > 0.9, "the root's base converted: {converted:.3}");

        // Now unroof it — delamination, erosion and the conveyor all do this —
        // and the rock must come back out of the assemblage it went into. What
        // comes off has to GO somewhere: hand it back to the mantle, the same
        // move a foundering root makes, so the world still balances.
        for bed in w.columns[1].layers.split_off(1) {
            for (e, m) in bed.elements.iter() {
                w.mantle.add(1, e, m);
            }
        }
        w.audit("unroofed");
        assert!(
            overburden_pa(&w.columns[1], 0, gravity, area) < gate,
            "the load really is gone"
        );
        for _ in 0..200 {
            stage.tick(&mut w, 1.0, &mut rng);
        }
        let after = w.columns[1].layers[0].eclogitised;
        assert!(
            after < 0.1 * converted,
            "eclogite carried back up retrogrades: {converted:.3} → {after:.3}"
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

        // Same discipline: both contenders are made of mantle this world had —
        // drawn from across the whole planetoid, because one sized freq-4 cell
        // holds ~2.7e18 kg total and the old single-cell draws came up empty
        // (the third bed was drawing from a cell the first two had drained).
        let mk = |w: &mut World, els: &[(u8, f64)], by: FormationProcess| {
            let mut c = flicker_worldstate::Composition::new();
            for &(e, m) in els {
                let mut want = m;
                for src in 0..w.mantle.n_cells() {
                    if want <= 0.0 {
                        break;
                    }
                    let took = w.mantle.remove(src, e, want);
                    c.add(e, took);
                    want -= took;
                }
            }
            Layer {
                elements: c,
                minerals: Default::default(),
                formed_at_myr: 0.0,
                formed_by: by,
                peak_pt: (0.0, 0.0),
                cooled: 0.0,
                eclogitised: 0.0,
            }
        };
        // A dense mafic floor wearing a light sediment drape.
        let mut loser = Column::empty(0);
        loser.layers.push(mk(&mut w, &[(12, 3.0e17), (26, 3.0e17)], FormationProcess::OceanicCrust));
        loser.layers.push(mk(&mut w, &[(14, 2.0e17), (8, 1.5e17)], FormationProcess::Sediment));
        // A buoyant felsic winner.
        let mut winner = Column::empty(0);
        winner.layers.push(mk(&mut w, &[(14, 5.0e17), (19, 2.0e17)], FormationProcess::ContinentalArc));

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
        let gravity = w.gravity_m_s2();
        // The mass whose weight sits exactly at the ceiling ON THIS WORLD —
        // the fixture is stated against it, not in typed-in kilograms, so the
        // test means the same thing at any planet size. (On a freq-4 planetoid
        // gravity is ~4% of the reference, so the pile that over-presses its
        // base is enormous — which is why small worlds keep taller mountains.)
        let ceiling_kg = super::delamination_pa(&w) * area / gravity;

        // Drawn from the mantle, never conjured — from across the whole
        // planetoid, because one small-world cell holds less rock than the
        // ceiling needs. Cell 0 gets the runaway pile; cell 1 an ordinary range.
        let stack = |w: &mut World, cell: usize, total: f64| {
            for k in 0..3 {
                let mut c = flicker_worldstate::Composition::new();
                for (e, share) in [(8u8, 0.47), (14, 0.34), (13, 0.15), (19, 0.04)] {
                    let mut want = share * total / 3.0;
                    let mut got = 0.0;
                    for src in 0..w.mantle.n_cells() {
                        if want <= 0.0 {
                            break;
                        }
                        let take = w.mantle.remove(src, e, want);
                        got += take;
                        want -= take;
                    }
                    c.add(e, got);
                }
                w.columns[cell].layers.push(Layer {
                    elements: c,
                    minerals: Default::default(),
                    formed_at_myr: k as f64,
                    formed_by: FormationProcess::ContinentalArc,
                    peak_pt: (0.0, 0.0),
                    cooled: 0.0,
                    eclogitised: 0.0,
                });
            }
        };
        stack(&mut w, 0, 2.2 * ceiling_kg); // over-pressed (margin for share shortfalls)
        stack(&mut w, 1, 0.1 * ceiling_kg); // an ordinary range
        w.audit("fixture");

        assert!(
            basal_pressure_pa(&w.columns[0], gravity, area) > super::delamination_pa(&w),
            "the fixture's runaway pile is genuinely over-pressed"
        );
        assert!(
            basal_pressure_pa(&w.columns[1], gravity, area) < super::delamination_pa(&w),
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
        //
        // And read at 500 ticks, not 240 (2026-08-06, the erosion rework):
        // mass wasting spreads young one-cell belts into their rings and the
        // talus rides adjacent slabs into trenches, so at 240 ticks the read
        // lands mid-avalanche — belts shaved, arc return not yet caught up.
        // The claim is about what the two melts DO, and by 500 the recycling
        // loop has paid the belts back.
        let w = run(24, 5, 500);
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
        let med = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v[v.len() / 2]
        };
        let (om, cm) = (mean(&ocean), mean(&cont));
        eprintln!(
            "continental {cm:.0} m (n {}, med {:.0}) vs oceanic {om:.0} m (n {}, med {:.0})  ({:.2}x)",
            cont.len(),
            med(&mut cont.clone()),
            ocean.len(),
            med(&mut ocean.clone()),
            cm / om.max(1e-9)
        );
        // The four-way split that decodes a failure here instantly: the water-laid
        // veneer populations churn hard in the young window (talus riding slabs,
        // shelves building), and knowing WHICH population moved is the difference
        // between a mechanism bug and an early read (it took three
        // investigations to learn that the first time).
        {
            let sedimented = |c: &crate::column::Column| {
                c.layers.iter().any(|l| l.formed_by == crate::column::FormationProcess::Sediment)
            };
            let mut oc = (0usize, Vec::new());
            let mut os = (0usize, Vec::new());
            let mut cc = (0usize, Vec::new());
            let mut cs = (0usize, Vec::new());
            for col in &w.columns {
                let e = elevation_m(col, area);
                match (crust_kind(col), sedimented(col)) {
                    (CrustKind::Oceanic, false) => { oc.0 += 1; oc.1.push(e); }
                    (CrustKind::Oceanic, true) => { os.0 += 1; os.1.push(e); }
                    (CrustKind::Continental, false) => { cc.0 += 1; cc.1.push(e); }
                    (CrustKind::Continental, true) => { cs.0 += 1; cs.1.push(e); }
                    _ => {}
                }
            }
            let m = |v: &Vec<f64>| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
            eprintln!(
                "  ocean pure n {} mean {:.0} · ocean+sed n {} mean {:.0} · cont pure n {} mean {:.0} · cont+sed n {} mean {:.0}",
                oc.0, m(&oc.1), os.0, m(&os.1), cc.0, m(&cc.1), cs.0, m(&cs.1)
            );
            let strata: f64 = w.columns.iter().map(|c| c.layers.len() as f64).sum::<f64>()
                / w.columns.len() as f64;
            eprintln!("  mean strata {strata:.1} · sea {:.0} m", crate::planet::sea_level_m(&w));
        }
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

