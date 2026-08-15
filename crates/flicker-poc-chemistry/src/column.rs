//! [`Column`] and [`Layer`] — the crust stack (spec §4.4) — and the **derived
//! classifiers** that read off them (§5.4, §6.4).
//!
//! The load-bearing rule of the whole rewrite: **a layer is composition and
//! order; nothing more.** `elevation`, `crust_kind`, `thickness`, `density`,
//! `hardness`, `depth` are NOT fields — they are pure functions, recomputed on
//! demand. Storing a derived property (letting it drift from its source and become
//! the truth) is the specific failure that killed the old crates.

use flicker_materials::ElementId;
use flicker_worldstate::{Composition, CompoundLedger};


/// Atomic number of silicon (the silica-fraction read of [`crust_kind`]).
const SI: ElementId = 14;
/// Carbon — what makes a bed organic rather than silicate.
const CARBON: ElementId = 6;
/// The eclogite formers — what a garnet is built out of (see
/// [`eclogite_former_frac`]).
const MG: ElementId = 12;
const FE: ElementId = 26;
const CA: ElementId = 20;

/// The **reference** surface gravity, re-exported from
/// [`config`](crate::config::GRAVITY_M_S2). Pressure math here takes the acting
/// gravity as a parameter — a world's own is
/// [`World::gravity_m_s2`](crate::planet::World::gravity_m_s2), reference × its
/// size scale — so a half-size planet presses its stacks half as hard.
pub use crate::config::GRAVITY_M_S2;

/// Material thinner than this can never be its own bed, however exotic — a film
/// settling on a bed joins that bed. Keeps a per-tick trickle from shredding the
/// stack into thousands of unresolvable films.
///
/// It is also the floor at which a bed being consumed is taken WHOLE rather
/// than by another fraction: a layer worn down toward zero carries two ledgers
/// whose last digits have drifted apart, and past this point the drift is
/// larger than the rock (see [`Delamination`](crate::crust::Delamination)).
pub const MIN_BED_MASS_KG: f64 = 1.0e15;

/// Overburden a bed must have carried before it can merge at all. Below it the
/// beds are still loose and the contact is still a contact.
const LITHIFICATION_PA: f64 = 2.0e7;

/// How unlike two compositions are, `0` (identical proportions) .. `1` (sharing
/// nothing). Half the total absolute difference of their mass fractions — the
/// standard overlap distance, and the only notion of "a different material" the
/// stratum lifecycle uses.
pub fn dissimilarity(a: &Composition, b: &Composition) -> f64 {
    let (ta, tb) = (a.total(), b.total());
    if ta <= 0.0 || tb <= 0.0 {
        return 1.0;
    }
    let mut seen: Vec<ElementId> = a.iter().map(|(e, _)| e).collect();
    seen.extend(b.iter().map(|(e, _)| e));
    seen.sort_unstable();
    seen.dedup();
    let sum: f64 = seen
        .iter()
        .map(|&e| (a.amount(e) / ta - b.amount(e) / tb).abs())
        .sum();
    (0.5 * sum).clamp(0.0, 1.0)
}

/// How a layer came to be — a permanent historical fact, never edited after
/// formation. The process set grows one variant per stage that deposits crust.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FormationProcess {
    /// Provenance not yet assigned (the t=0 hot ball).
    Primordial,
    /// Basaltic crust from mantle partial melt at a spreading centre (M2).
    OceanicCrust,
    /// Silicic crust refined by arc magmatism above a subduction zone (M2).
    ContinentalArc,
    /// Lava a vent put here — melt that rose through a frozen lid and froze on
    /// top of it. The pile it builds is what lifts a volcano.
    Volcanic,
    /// Rock the water took from somewhere else and put down here.
    Sediment,
    /// Dead tissue that the ground kept — peat, algal mud, and what burial later
    /// cooks them into. The only bed type life makes.
    Organic,
    /// What circulating fluid dropped out of solution — the beds that end up rich
    /// enough to be worth digging.
    Hydrothermal,
}

/// One bed in the crust stack: conserved element + mineral mass, plus the
/// historical facts (`formed_*`, `peak_pt`) that later reads depend on. **NOT
/// STORED:** rock_kind, hardness, density, thickness — all derived (§6.4).
#[derive(Clone, Debug)]
pub struct Layer {
    /// Conserved element mass in this bed.
    pub elements: Composition,
    /// Conserved mineral mass, bounded by the free elements above (§4.1).
    pub minerals: CompoundLedger,
    /// When this bed formed, Myr — permanent.
    pub formed_at_myr: f64,
    /// The process that deposited it — permanent.
    pub formed_by: FormationProcess,
    /// Max (pressure, temperature) ever seen → metamorphic grade. Monotonic.
    pub peak_pt: (f64, f64),
    /// **How far this bed has cooled into its contracted state**, `0..1` —
    /// advanced by [`ThermalSubsidence`](crate::crust::ThermalSubsidence) on
    /// AGE alone, because that is what cooling depends on.
    ///
    /// INTEGRATED state, not a derived read: it is a history the bed
    /// accumulates, exactly like [`peak_pt`](Self::peak_pt) above and the
    /// mantle's own `differentiation`. Density is still a *function* — it
    /// reads this the same way it reads the element ledger.
    ///
    /// It is the whole reason this world can have OCEAN BASINS. Fresh sea
    /// floor is lighter than the mantle and rides above the datum; the same
    /// rock, cold, is heavier than the mantle and rides BELOW it. Without the
    /// second half every column floats, there is nowhere for a sea to sit, and
    /// any ocean at all spreads out and drowns the world (the flat-water-world
    /// attractor Aaron kept landing in).
    pub cooled: f64,
    /// **How far this bed has changed phase into eclogite**, `0..1` — advanced
    /// by [`Eclogitisation`](crate::crust::Eclogitisation) on the PRESSURE this
    /// particular bed carries, because that is what a phase change depends on.
    ///
    /// Split from [`cooled`](Self::cooled) after measurement: the two were one
    /// unconditional timer, and the pressure half — which should only ever
    /// reach a deep root — was firing on every bed in the world. It converted
    /// continental crust to subductable crust 3,663 times per 200 ticks with
    /// zero reversals, which is what was eating the continents from the inside.
    ///
    /// Unlike cooling, this one RETROGRADES. Eclogite carried back up out of
    /// its stability field reverts, so a root that is shed or unroofed stops
    /// being a slab-in-waiting instead of staying one forever.
    pub eclogitised: f64,
}

impl Layer {
    /// Total conserved element mass in this bed.
    pub fn mass_kg(&self) -> f64 {
        self.elements.total()
    }
}

impl Layer {
    /// Take `frac` of this bed away and hand back what was actually removed.
    ///
    /// **Minerals go with their elements.** The mineral ledger is a *claim* on this
    /// bed's elements, so taking elements out without releasing the same share of
    /// the claim leaves the bed asserting matter it no longer holds — which the
    /// compound-ledger bound catches within the tick, and rightly. Physically this
    /// is just what happens: break rock up and its minerals come apart with it,
    /// re-forming wherever the pieces end up.
    ///
    /// The one place both weathering and circulating fluid take rock from a bed, so
    /// there is one answer to what that does rather than two that can drift.
    pub fn release(&mut self, frac: f64) -> Vec<(ElementId, f64)> {
        let frac = frac.clamp(0.0, 1.0);
        if frac <= 0.0 {
            return Vec::new();
        }
        let planned: Vec<(ElementId, f64)> = self.elements.iter().map(|(e, m)| (e, m * frac)).collect();
        let mut taken = Vec::with_capacity(planned.len());
        for (e, m) in planned {
            let got = self.elements.remove(e, m);
            if got > 0.0 {
                taken.push((e, got));
            }
        }
        let releasing: Vec<(u16, f64)> = self.minerals.iter().map(|(c, m)| (c, m * frac)).collect();
        for (c, m) in releasing {
            self.minerals.remove(c, m);
        }
        taken
    }
}

/// A single hex column of the world — the stack of rock standing on one cell.
/// **NOT STORED:** elevation, crust_kind, thickness, depth, density, hardness,
/// age. The one stored non-ledger field is `accum_disp`, because it is *integrated*
/// state — how far this column has been carried since it last stepped — not a
/// function of the current ledger.
///
/// A column is a **thing that travels**. The tectonic conveyor
/// ([`crate::tectonics`]) relocates it whole, ledgers and history intact, from one
/// cell to the next; `cell_id` is where it stands right now, not where it was born.
#[derive(Clone, Debug)]
pub struct Column {
    /// Cell index (matches the topology grid) — where this stack stands **now**.
    pub cell_id: u32,
    /// The crust stack, bottom → top (index 0 = base of crust). **Empty at t=0:**
    /// crust is an OUTPUT, grown by later stages, never seeded.
    pub layers: Vec<Layer>,
    /// Displacement carried since this column last stepped to a new cell, as a
    /// tangent vector in unit-sphere units. When it reaches one cell spacing the
    /// column relocates and the step is subtracted — so a fast plate steps often
    /// and a slow one rarely, from the same rule.
    pub accum_disp: glam::Vec3,
    // Deferred to later milestones, kept OFF the struct so nothing derived is
    // stored: geotherm (M1), surface_water (M3), biomass (M5).
}

impl Column {
    /// An empty column for `cell_id` — no crust yet (the t=0 hot ball).
    pub fn empty(cell_id: u32) -> Self {
        Self {
            cell_id,
            layers: Vec::new(),
            accum_disp: glam::Vec3::ZERO,
        }
    }

    /// Mean density of the whole stack, kg/m³ — mass over volume, the read that
    /// decides who rides and who sinks when two columns contend for one cell.
    /// An empty column reads as mantle (nothing to float).
    pub fn mean_density(&self) -> f64 {
        let mass = self.mass_kg();
        if mass <= 0.0 {
            return MANTLE_DENSITY;
        }
        let volume: f64 = self.layers.iter().map(|l| l.mass_kg() / density_kg_m3(l)).sum();
        if volume <= 0.0 {
            MANTLE_DENSITY
        } else {
            mass / volume
        }
    }

    /// Take everything this column holds, leaving it empty. The debit half of a
    /// whole-stack move — the caller must land every gram somewhere.
    pub fn take_all(&mut self) -> Vec<Layer> {
        std::mem::take(&mut self.layers)
    }

    /// Stack `beds` on top of this column, oldest first. Used when one stack is
    /// thrust over another (collision) — the arriving beds keep their identity and
    /// their history, so the join is visible in the stratigraphy afterwards.
    pub fn pile_on(&mut self, beds: Vec<Layer>) {
        self.layers.extend(beds);
    }

    /// Total conserved element mass stacked in this column.
    pub fn mass_kg(&self) -> f64 {
        self.layers.iter().map(Layer::mass_kg).sum()
    }

    /// Conserved mass of one element across the whole stack.
    pub fn element_mass(&self, element: ElementId) -> f64 {
        self.layers.iter().map(|l| l.elements.amount(element)).sum()
    }

    /// Deposit newly-formed material at the TOP of the stack.
    ///
    /// **Whether this thickens the top bed or starts a new one is a consequence,
    /// never a schedule.** Like accretes to like: material that resembles what is
    /// already on top merges into it, and material that does not can't be absorbed,
    /// so it begins its own bed. That is how a bed boundary forms in the ground —
    /// the depositional environment changed — and it is self-limiting, because a
    /// slowly-drifting composition keeps resembling the running mean it is drifting.
    ///
    /// The caller has already debited the source reservoir, so this is the credit
    /// half of a conserved move.
    pub fn deposit(&mut self, process: FormationProcess, formed_at_myr: f64, add: &[(ElementId, f64)]) {
        let arriving: f64 = add.iter().map(|&(_, m)| m).sum();
        let spawn = match self.layers.last() {
            // Nothing to accrete to.
            None => true,
            // Too little to register as its own bed however different it is — a
            // film settling on a bed joins that bed.
            Some(_) if arriving < MIN_BED_MASS_KG => false,
            // **A BED IS A DEPOSITIONAL EVENT** (Aaron, ruling B, 2026-08-07),
            // not a composition contrast. Two conditions, and both are already
            // carried by the column — neither is a new constant to tune:
            //
            // 1. **A different process laid this down.** A change of process is
            //    a change of environment, and that is what a stratum boundary
            //    IS: ash on mud, mud on lava, a thrust sheet on sea floor.
            // 2. **The bed below has lithified.** Loose material does not mix
            //    into cemented rock; it settles on top of it. This is what
            //    paces the cake, because a bed must be buried enough to weld
            //    before the next one can start — so beds accumulate at the
            //    speed the world actually deposits, not per tick.
            //
            // The old test was `dissimilarity > BED_SPAWN_DISSIMILARITY`, and
            // between it and burial erasing boundaries again the stack averaged
            // **1.9 beds against a cap of 20** — the canyon wall the cap exists
            // for was never being deposited in the first place.
            Some(top) => {
                top.formed_by != process || top.peak_pt.0 >= LITHIFICATION_PA
            }
        };
        if spawn {
            self.layers.push(Layer {
                elements: Composition::new(),
                minerals: CompoundLedger::new(),
                formed_at_myr,
                formed_by: process,
                peak_pt: (0.0, 0.0),
            cooled: 0.0,
            eclogitised: 0.0,
            });
        }
        let top = self.layers.last_mut().expect("just ensured a top layer");
        for &(e, m) in add {
            top.elements.add(e, m);
        }
    }

    /// The stack's own housekeeping, run once the tick's material has moved:
    /// **record what each bed has endured, then push the overflow into bedrock.**
    ///
    /// Two consequences, no targets:
    /// - Every bed's `peak_pt` takes the max of what it has ever seen. Pressure is
    ///   the load above it, temperature is the rock it sits in. Monotone, permanent
    ///   — this is the record the metamorphic chemistry later reads.
    /// - Past `soft_cap` beds, **the BOTTOM of the stack collapses**: bed 1 is
    ///   folded into bed 0 until the count fits. Bed 0 is the bedrock — an
    ///   aggregate whose thickness is the column's root height. Mass moves
    ///   whole, so the collapse is exactly conservative.
    ///
    /// **A boundary that formed is not erased by burial** (Aaron, ruling A,
    /// 2026-08-07). This used to merge the most-alike ADJACENT pair anywhere in
    /// the stack, with a tolerance that widened past the cap. Two costs: the
    /// stack averaged 1.9 beds because boundaries were destroyed as readily as
    /// they were made, and a mid-stack merge renumbers every bed above it — so
    /// nothing downstream, least of all a layer-cake render, could hold an
    /// identity for "the third bed down". Collapsing from the bottom keeps
    /// indices stable measured from the SURFACE, which is where they are looked
    /// at, and confines the churn to the least visible place.
    ///
    /// `soft_cap` remains a **resource** guardrail, not a shape target: it says
    /// how many beds may be retained, never what they should contain.
    pub fn reconcile(
        &mut self,
        surface_k: f64,
        mantle_k: f64,
        gravity_m_s2: f64,
        cell_area_m2: f64,
        soft_cap: usize,
    ) {
        if self.layers.is_empty() {
            return;
        }
        // Load AND depth at each bed's BASE, walked top-down so each bed sees
        // what it carries and how deep it lies. Both are what `peak_pt` is for.
        //
        // **The base, not the top** — a bed's PEAK pressure is at its bottom,
        // which is where it welds and where it cooks. Reading the top instead
        // recorded zero for the topmost bed forever (nothing is above it), and
        // that silently disabled the lithification half of the bed-spawn rule:
        // `deposit` asks whether the bed it is landing on has cemented, and the
        // answer was permanently no. A thick bed lithifies under its own weight,
        // which is exactly what paces a layer cake.
        let mut above = 0.0f64;
        let mut depth = 0.0f64;
        for i in (0..self.layers.len()).rev() {
            let mass = self.layers[i].mass_kg();
            let thickness = thickness_m(&self.layers[i], cell_area_m2);
            let p = (above + mass) * gravity_m_s2 / cell_area_m2;
            let t = geotherm_k(surface_k, mantle_k, depth + thickness);
            let bed = &mut self.layers[i];
            bed.peak_pt = (bed.peak_pt.0.max(p), bed.peak_pt.1.max(t));
            above += mass;
            depth += thickness;
        }

        // **The overflow goes to bedrock.** Bed 1 folds into bed 0 until the
        // count fits, so the deepest record is the one that gives way and
        // everything above keeps its place measured from the surface.
        while self.layers.len() > soft_cap.max(1) {
            let upper = self.layers.remove(1);
            let bedrock = &mut self.layers[0];
            // Weighed BEFORE the merge — after it, bedrock's mass already
            // includes what is being blended in and the weighting is meaningless.
            let (bm, um) = (bedrock.mass_kg(), upper.mass_kg());
            let total = (bm + um).max(f64::MIN_POSITIVE);
            // Whole-mass move: every gram lands, so the audit sees no change.
            for (e, m) in upper.elements.iter() {
                bedrock.elements.add(e, m);
            }
            for (c, m) in upper.minerals.iter() {
                bedrock.minerals.add(c, m);
            }
            // Bedrock remembers the harsher history of everything it has eaten,
            // and the dense-assemblage states come with their mass — mass-weighted,
            // because a state is a property of the rock and the rock is being
            // combined. Reading either one off the survivor alone would let a
            // thin fresh bed erase the history of the pile it landed on.
            bedrock.cooled = (bedrock.cooled * bm + upper.cooled * um) / total;
            bedrock.eclogitised = (bedrock.eclogitised * bm + upper.eclogitised * um) / total;
            bedrock.peak_pt = (
                bedrock.peak_pt.0.max(upper.peak_pt.0),
                bedrock.peak_pt.1.max(upper.peak_pt.1),
            );
        }
    }

    /// Take `frac` of the top bed's element mass **iff** it was formed by `process`
    /// (subducting the youngest oceanic crust). Returns what was removed per element
    /// (for the caller to credit the mantle); pops the bed if fully drained. The
    /// debit half of a conserved move.
    pub fn subduct_top(&mut self, process: FormationProcess, frac: f64) -> Vec<(ElementId, f64)> {
        let frac = frac.clamp(0.0, 1.0);
        let planned: Vec<(ElementId, f64)> = match self.layers.last() {
            Some(top) if top.formed_by == process => {
                top.elements.iter().map(|(e, m)| (e, m * frac)).collect()
            }
            _ => return Vec::new(),
        };
        // Credit the caller with what `remove` ACTUALLY took, not the planned amount
        // — the conservation-safe contract, even though `m ≤ present` here makes them
        // equal.
        let mut taken = Vec::with_capacity(planned.len());
        if let Some(top) = self.layers.last_mut() {
            for (e, m) in planned {
                let got = top.elements.remove(e, m);
                if got > 0.0 {
                    taken.push((e, got));
                }
            }
            if top.elements.is_empty() {
                self.layers.pop();
            }
        }
        taken
    }
}

// ── Derived classifiers — FUNCTIONS, never fields (spec §5.4, §6.4, §9). ──

/// What a column *is*, read off its composition — never stored, never set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CrustKind {
    /// No crust grown yet (the t=0 hot ball).
    Undifferentiated,
    /// Thin, dense, mafic — ocean floor.
    Oceanic,
    /// Thick, light, silicic — continent.
    Continental,
}

/// Classify a column from its stack (§5.4). A column does not *have* a kind; its
/// kind is *recognised*. A column that thins, floods with basalt and loads with
/// sediment simply *becomes* something the classifier returns differently — no
/// rule says "rift basins are shallow".
///
/// The read is **buoyancy**: continental crust is the crust the mantle cannot
/// swallow. That is the same [`SUBDUCTABLE_DENSITY`] the conveyor uses to decide
/// which beds sink, so the name and the behaviour are one thing — a column called
/// continental is exactly a column that will not go down.
///
/// It used to be a silica cut at 0.29 by mass, which worked while a column was
/// made of one melt and broke the moment the conveyor started mixing them: a thick
/// pile of arc rock carrying scraped sea floor dipped under the cut and got called
/// oceanic, while a thin pure-arc sliver stayed above it — which read out as
/// continents standing *lower* than the sea floor. Density does not have that
/// failure mode, because it is the property that actually decides the outcome.
/// **Veneers do not decide what kind of crust something is.** The read is of the
/// BASEMENT — the beds a melt made — because a peat bog or a chalk drape on
/// basalt is still ocean floor, and a sea floor that grew a biosphere has not
/// become a continent by being rained on. Sediment and organic beds are
/// excluded; everything igneous or hydrothermal counts. (Subduction itself is
/// judged bed by bed in the conveyor, so nothing here decides what sinks.)
pub fn crust_kind(col: &Column) -> CrustKind {
    if col.layers.is_empty() {
        return CrustKind::Undifferentiated;
    }
    let (mut mass, mut volume) = (0.0, 0.0);
    for bed in col.layers.iter().filter(|l| {
        !matches!(l.formed_by, FormationProcess::Sediment | FormationProcess::Organic)
    }) {
        let m = bed.mass_kg();
        mass += m;
        volume += m / density_kg_m3(bed);
    }
    // A column that is nothing BUT veneer has no basement to read; fall back to
    // the whole stack rather than inventing an answer.
    let density = if volume > 0.0 { mass / volume } else { col.mean_density() };
    if density <= SUBDUCTABLE_DENSITY {
        CrustKind::Continental
    } else {
        CrustKind::Oceanic
    }
}

/// Mean stack density above which the mantle can swallow a column, kg/m³.
///
/// Mafic sea floor is denser than this and cannot resist; refined silicic rock is
/// lighter and cannot be pushed in. It sits between the two crustal densities the
/// melt model produces, so it separates the populations rather than naming either
/// of them — and it is the ONE threshold behind both [`crust_kind`] and what the
/// conveyor does to a losing stack.
pub const SUBDUCTABLE_DENSITY: f64 = 2830.0;

/// Reference mantle (peridotite) density, kg/m³ — the datum Airy isostasy floats
/// the crust on.
pub const MANTLE_DENSITY: f64 = 3300.0;

/// Bulk density of a layer, kg/m³ — **derived from its composition** (§6.4), never
/// a stored field. Read off the silica fraction: more silica → more felsic →
/// lighter, mapping the crustal range (~0.15 mafic .. ~0.35 felsic) to ~3000 ..
/// ~2650 kg/m³. This is what makes continental crust ride higher than oceanic (the
/// bimodal hypsometry). The modal-mineral density arrives with the rock tier (M6).
pub fn density_kg_m3(layer: &Layer) -> f64 {
    let mass = layer.mass_kg();
    if mass <= 0.0 {
        return MANTLE_DENSITY;
    }
    let si_frac = layer.elements.amount(SI) / mass;
    let t = ((si_frac - 0.15) / (0.35 - 0.15)).clamp(0.0, 1.0);
    let silicate = 3000.0 - t * (3000.0 - 2650.0);

    // **Cold mafic rock is heavier than hot mafic rock**, because its minerals
    // are not the same minerals: as basalt cools it takes on the garnet-bearing
    // assemblage geology calls eclogite, and the SAME elements pack denser.
    //
    // Scaled by [`eclogite_former_frac`] — the Mg/Fe/Ca a garnet needs — NOT by
    // the silica ramp above. Basalt and granite are only ~0.10 apart in silica
    // and the ramp reads a basalt as half-evolved, which handed real sea floor
    // barely half the gain and left it floating (measured: 3073 kg/m³, still
    // buoyant, no basin). The mafic minerals separate the two rock families
    // cleanly, which is the physical question being asked.
    //
    // Full strength this crosses [`MANTLE_DENSITY`], and a bed heavier than
    // what it floats on rides BELOW the datum: [`elevation_m`] goes negative
    // and the world finally has somewhere to put an ocean. A felsic pile scores
    // ~0 here and is untouched however long it sits — which is why continents
    // are permanent and sea floor is not.
    //
    // **Two states, because two different things make rock dense.** Cooling is
    // age-driven and reaches every bed that sits long enough; the eclogite
    // phase change is pressure-driven and only ever reaches the bottom of a
    // thick root. They were one field advanced by one unconditional timer, and
    // the consequence was measured: continental columns mixed themselves past
    // the buoyancy threshold and the conveyor ate them. Both gains are gated by
    // the same mafic fraction — a granite has no garnet-formers to reorganise
    // whatever happens to it — but each is advanced by its own stage on its own
    // condition.
    let mafic = eclogite_former_frac(layer);
    let silicate = silicate
        + mafic
            * (layer.cooled.clamp(0.0, 1.0) * THERMAL_GAIN
                + layer.eclogitised.clamp(0.0, 1.0) * ECLOGITE_GAIN);

    // **Carbon-rich rock is not silicate rock.** Peat, coal and oil shale are
    // among the lightest things a column can carry, and they contain no silicon
    // at all — so the silica ramp above hands them `si_frac = 0`, which is its
    // MAFIC end: the densest answer it has, and about 2.5x too heavy. Left
    // uncorrected that floats every organic bed wrong, and hands `crust_kind` a
    // peat bog it thinks is sea floor.
    //
    // Blended in over a band rather than switched, so a marl grading into a
    // coal measure has no cliff in it — and the band starts high enough that
    // carbonates (C ~ 0.12) stay on the silicate law where they belong.
    let c_frac = layer.elements.amount(CARBON) / mass;
    let organic = ((c_frac - 0.15) / (0.35 - 0.15)).clamp(0.0, 1.0);
    silicate + organic * (ORGANIC_DENSITY - silicate)
}

/// Bulk density of carbon-rich rock, kg/m³ — the peat/coal/oil-shale family.
pub const ORGANIC_DENSITY: f64 = 1400.0;

/// How much denser cold mafic rock is than the fresh melt it froze from, kg/m³
/// — the COOLING half, which is the one that builds ocean basins. Sized so that
/// old sea floor (mafic fraction 1.0 on the ramp below, silicate base ~3000)
/// reaches ~3560 and so clears [`MANTLE_DENSITY`] with room to spare. That
/// crossing is what turns an ocean floor into an ocean BASIN, and Earth's own
/// abyssal plains lie ~4 km below its continental shelves for exactly this
/// reason.
///
/// Sea floor never gets deep enough to eclogitise — a 7 km column carries about
/// 0.2 GPa against [`eclogite_pa`](crate::crust::eclogite_pa)'s 1.3 — so this
/// constant has to do the whole basin job on its own, and it keeps the value
/// the basin hypsometry was measured at.
pub const THERMAL_GAIN: f64 = 560.0;

/// The FURTHER gain when deep mafic rock actually changes phase, kg/m³ — on top
/// of [`THERMAL_GAIN`], since rock this deep is also cold.
///
/// **Currently ZERO, deliberately, pending Aaron's ruling.** The physical value
/// is ~200 (real eclogite runs ~3500–3600 against basalt's ~2900, so a root that
/// had both cooled and converted would land near 3760). But it was set to 200
/// the moment the stage landed, and 200 is *extra sink weight applied precisely
/// to deep continental roots* — measured, 18 of the 19 columns carrying
/// converted rock are continental, i.e. the thickest piles, i.e. mountains.
/// Aaron's report on the first build carrying it was *"the mountains with black
/// dot wiggling is back… you've fucked up the sink weight on mountain seams"*.
///
/// That report is not cleanly attributable — the build he ran was taken while
/// the tree was in a reverted measurement state — so this is held at zero to
/// make the density law **identical to the known-good build**. The phase change
/// still runs and `eclogitised` is still tracked and correctly gated; it simply
/// carries no weight until the ruling. Raising it is a one-line change and
/// wants its own before/after bake, because it lands on mountain roots.
pub const ECLOGITE_GAIN: f64 = 0.0;

/// **How much of this bed can actually become eclogite**, `0..1` — its
/// magnesium, iron and calcium, which are what a garnet is built out of.
///
/// The separation the whole basin mechanism rests on: melts drawn by
/// [`oceanic_affinity`](crate::crust::oceanic_affinity) run ~0.27 here and
/// eclogitise fully, while
/// [`continental_affinity`](crate::crust::continental_affinity)'s refined melts
/// run ~0.085 and score zero — so sea floor founders and continents never can,
/// out of the two melts' own chemistry rather than a rule naming either.
///
/// **This ramp is load-bearing for the OCEAN BASINS, and it is calibrated.**
/// Moving its opening from 0.10 to 0.18 was tried on 2026-08-07, to stop mixed
/// continental rock scoring: the reasoning was that a rock halfway between
/// granite and basalt is a diorite and should build little garnet. The 4.5 BY
/// bake refuted it. This world's sea floor does not sit at the ~0.27 the note
/// above assumes — a great deal of it lives between 0.10 and 0.20 — so opening
/// at 0.18 cut the densification of real sea floor, and the basin floor went
/// from −282 m back to −52 m, undoing the depth the erosion work had just won.
///
/// So it stays where it was measured. The continental leak it was aimed at —
/// mixed columns crossing [`SUBDUCTABLE_DENSITY`] from age alone — is real and
/// still open, but this is the wrong lever for it: the same number sets whether
/// the world has basins at all.
fn eclogite_former_frac(layer: &Layer) -> f64 {
    let mass = layer.mass_kg();
    if mass <= 0.0 {
        return 0.0;
    }
    let mafic = (layer.elements.amount(MG) + layer.elements.amount(FE) + layer.elements.amount(CA))
        / mass;
    ((mafic - 0.10) / (0.24 - 0.10)).clamp(0.0, 1.0)
}

/// Thickness of a layer, m: `mass / (density × cell_area)` (§4.4). The area comes
/// from the grid in play ([`cell_area_m2`](crate::config::cell_area_m2)), so this
/// is right at every frequency, not only the full-resolution planet.
pub fn thickness_m(layer: &Layer, cell_area_m2: f64) -> f64 {
    let d = density_kg_m3(layer);
    if d <= 0.0 || cell_area_m2 <= 0.0 {
        0.0
    } else {
        layer.mass_kg() / (d * cell_area_m2)
    }
}

/// Total crust thickness of a column, m — the sum over its beds.
pub fn crust_thickness_m(col: &Column, cell_area_m2: f64) -> f64 {
    col.layers.iter().map(|l| thickness_m(l, cell_area_m2)).sum()
}

/// Depth over which the ground warms from its own surface to the temperature of
/// the mantle underneath it, m — the lithosphere's thickness. Earth's runs
/// ~100 km, and the number is what sets the geothermal gradient: at a 500 K
/// surface-to-mantle contrast this gives ~5 K/km near the top, which is the
/// right order for real crust.
pub const LITHOSPHERE_SCALE_M: f64 = 100_000.0;

/// **How warm a bed is at `depth_m` below the surface** — the geotherm.
///
/// Rock gets hotter downward because the mantle is the heat source under it, so
/// the profile runs from the ground's own surface temperature toward the
/// mantle's over [`LITHOSPHERE_SCALE_M`], and never overshoots it: nothing in
/// the crust is hotter than what it is sitting on.
///
/// **Why this had to exist before metamorphism could.** Every bed used to be
/// stamped with the mantle temperature outright, whatever depth it lay at, so
/// `peak_pt`'s temperature half carried no depth information and nothing could
/// honestly read it — the codebase said so at three separate sites, all in the
/// future tense. With a real geotherm the pair finally means something: shallow
/// rock stays cool however old it gets, and only ground buried under a genuine
/// mountain root reaches the conditions that reorganise it.
pub fn geotherm_k(surface_k: f64, mantle_k: f64, depth_m: f64) -> f64 {
    let reach = (depth_m / LITHOSPHERE_SCALE_M).clamp(0.0, 1.0);
    surface_k + (mantle_k - surface_k) * reach
}

/// Overburden pressure on the TOP of bed `index`, Pa — **derived**, never stored:
/// the weight of everything stacked above it, spread over the cell.
///
/// This is what makes a stack's weight causal rather than decorative. It drives
/// compaction (which beds merge), it is the `peak_pt` a bed carries forever, and
/// it is the `P` the metamorphic chemistry will read when a buried carbon bed
/// reorganises into something harder.
pub fn overburden_pa(col: &Column, index: usize, gravity_m_s2: f64, cell_area_m2: f64) -> f64 {
    let above: f64 = col.layers.iter().skip(index + 1).map(Layer::mass_kg).sum();
    above * gravity_m_s2 / cell_area_m2.max(f64::MIN_POSITIVE)
}

/// Pressure at the BASE of the column, Pa — the whole stack's weight. What the
/// deepest bed carries.
pub fn basal_pressure_pa(col: &Column, gravity_m_s2: f64, cell_area_m2: f64) -> f64 {
    col.mass_kg() * gravity_m_s2 / cell_area_m2.max(f64::MIN_POSITIVE)
}

/// Mean elevation of a column, m above the mantle datum — **the only place
/// elevation is derived** (§6.3). **Airy isostasy on absolute column mass and
/// density**, never a percentile rank (the ridge-spine artefact, §6.2): each bed
/// floats by `thickness · (ρ_mantle − ρ_bed) / ρ_mantle`, so a thick light
/// continental stack rides high and a thin dense oceanic one sits low. An empty
/// column is the bare mantle datum ⇒ 0. (The sea-level solve that turns this into
/// land vs sea-floor is M3.)
pub fn elevation_m(col: &Column, cell_area_m2: f64) -> f64 {
    col.layers
        .iter()
        .map(|l| thickness_m(l, cell_area_m2) * (MANTLE_DENSITY - density_kg_m3(l)) / MANTLE_DENSITY)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell's worth of area, for the pressure reads — the one canon area.
    const AREA: f64 = crate::config::CELL_AREA_M2;
    /// Enough mass that a bed can stand on its own (well over [`MIN_BED_MASS_KG`]).
    const BIG: f64 = 1.0e18;

    /// Basalt-ish: O/Si/Mg/Fe.
    fn mafic(scale: f64) -> Vec<(ElementId, f64)> {
        vec![(8, 0.45 * scale), (14, 0.24 * scale), (12, 0.19 * scale), (26, 0.12 * scale)]
    }
    /// Granite-ish: the same elements in very different proportions.
    fn felsic(scale: f64) -> Vec<(ElementId, f64)> {
        vec![(8, 0.47 * scale), (14, 0.34 * scale), (13, 0.15 * scale), (19, 0.04 * scale)]
    }

    fn col_with(beds: &[Vec<(ElementId, f64)>]) -> Column {
        let mut c = Column::empty(0);
        for (i, bed) in beds.iter().enumerate() {
            // Force each into its own bed by construction, so merge tests start
            // from a known stack rather than from the spawn rule's verdict.
            c.layers.push(Layer {
                elements: Composition::new(),
                minerals: CompoundLedger::new(),
                formed_at_myr: i as f64,
                formed_by: FormationProcess::OceanicCrust,
                peak_pt: (0.0, 0.0),
            cooled: 0.0,
            eclogitised: 0.0,
            });
            for &(e, m) in bed {
                c.layers.last_mut().unwrap().elements.add(e, m);
            }
        }
        c
    }

    /// The spawn rule, both ways: like accretes to like, unlike starts a bed.
    #[test]
    fn like_material_thickens_a_bed_and_unlike_material_starts_one() {
        let mut c = Column::empty(0);
        c.deposit(FormationProcess::OceanicCrust, 0.0, &mafic(BIG));
        assert_eq!(c.layers.len(), 1, "the first material has nothing to accrete to");

        c.deposit(FormationProcess::OceanicCrust, 1.0, &mafic(BIG));
        assert_eq!(c.layers.len(), 1, "more of the same thickens the bed it lands on");

        c.deposit(FormationProcess::ContinentalArc, 2.0, &felsic(BIG));
        assert_eq!(c.layers.len(), 2, "material this different cannot be absorbed");
    }

    /// A trickle joins whatever it lands on, however exotic — otherwise a per-tick
    /// film would shred the stack into thousands of unresolvable beds.
    #[test]
    fn a_film_joins_the_bed_it_lands_on() {
        let mut c = Column::empty(0);
        c.deposit(FormationProcess::OceanicCrust, 0.0, &mafic(BIG));
        c.deposit(FormationProcess::ContinentalArc, 1.0, &felsic(MIN_BED_MASS_KG * 0.5));
        assert_eq!(c.layers.len(), 1);
    }

    /// **A boundary that formed is not erased by burial** (ruling A, 2026-08-07).
    /// This used to be the opposite: alike loaded beds merged, and between that
    /// and a composition test for spawning, the stack averaged 1.9 beds against
    /// a cap of 20 — the canyon wall the cap exists for was never deposited.
    #[test]
    fn burial_does_not_erase_a_boundary() {
        let mut c = col_with(&[mafic(BIG), mafic(BIG), mafic(BIG)]);
        for _ in 0..8 {
            c.reconcile(1200.0, 1200.0, GRAVITY_M_S2, AREA, STRATA_SOFT_CAP_FOR_TEST);
        }
        assert_eq!(c.layers.len(), 3, "alike and loaded, and still three beds");
    }

    /// Past the cap the BOTTOM gives way: bed 1 folds into bedrock until the
    /// count fits, so indices stay stable measured from the surface and the
    /// deepest record is the one that pays. Every gram lands.
    #[test]
    fn overflow_collapses_into_bedrock_and_conserves_every_gram() {
        let mut c = col_with(&[mafic(BIG), felsic(BIG), mafic(BIG), felsic(BIG)]);
        let before = c.mass_kg();
        let top_before = c.layers.last().expect("a top bed").mass_kg();

        c.reconcile(1200.0, 1200.0, GRAVITY_M_S2, AREA, 2);

        assert_eq!(c.layers.len(), 2, "collapsed to the cap");
        assert!(
            (c.mass_kg() - before).abs() < 1e-6 * before,
            "the collapse moved whole mass: {before} → {}",
            c.mass_kg()
        );
        // Bedrock ate the overflow; the surface bed is untouched, which is the
        // property the layer-cake render depends on.
        assert!(
            c.layers[0].mass_kg() > BIG * 2.5,
            "bedrock aggregated what it swallowed: {:.3e}",
            c.layers[0].mass_kg()
        );
        assert!(
            (c.layers.last().expect("a top bed").mass_kg() - top_before).abs() < 1e-9 * top_before,
            "the surface bed did not move"
        );
    }

    /// Beds that are genuinely different survive burial. A boundary is a contrast,
    /// and pressure alone does not invent a similarity that is not there.
    #[test]
    fn burial_does_not_merge_beds_that_differ() {
        let mut c = col_with(&[mafic(BIG), felsic(BIG)]);
        for _ in 0..5 {
            c.reconcile(1200.0, 1200.0, GRAVITY_M_S2, AREA, STRATA_SOFT_CAP_FOR_TEST);
        }
        assert_eq!(c.layers.len(), 2);
    }

    /// An unloaded contact is still a contact: without overburden nothing merges,
    /// however alike the beds are.
    #[test]
    fn an_unloaded_contact_survives() {
        // Two alike beds so light that the load never reaches lithification.
        let tiny = MIN_BED_MASS_KG;
        let mut c = col_with(&[mafic(tiny), mafic(tiny)]);
        for _ in 0..5 {
            c.reconcile(1200.0, 1200.0, GRAVITY_M_S2, AREA, STRATA_SOFT_CAP_FOR_TEST);
        }
        assert_eq!(c.layers.len(), 2, "no load, no compaction");
    }

    /// A bed remembers the worst it ever carried — the record the metamorphic
    /// chemistry reads later. Monotone: unloading never lowers it.
    #[test]
    fn a_bed_remembers_the_worst_it_has_seen() {
        let mut c = col_with(&[mafic(BIG), felsic(BIG)]);
        c.reconcile(1500.0, 1500.0, GRAVITY_M_S2, AREA, STRATA_SOFT_CAP_FOR_TEST);
        let (p, t) = c.layers[0].peak_pt;
        assert!(p > 0.0 && (t - 1500.0).abs() < 1e-9, "the buried bed recorded its load");
        // The top bed records the pressure at ITS OWN BASE — its own weight. A
        // bed's peak is at its bottom, and reading the top instead pinned the
        // surface bed at zero forever, which disabled the lithification half of
        // the bed-spawn rule (a bed could never cement, so nothing ever landed
        // ON it as a new stratum).
        let top = c.layers[1].peak_pt.0;
        assert!(top > 0.0, "the top bed carries its own weight: {top:.3e} Pa");
        assert!(top < p, "…and still less than the bed beneath it: {top:.3e} vs {p:.3e}");

        // Strip the load and cool it: the record must not fall.
        c.layers.pop();
        c.reconcile(300.0, 300.0, GRAVITY_M_S2, AREA, STRATA_SOFT_CAP_FOR_TEST);
        assert_eq!(c.layers[0].peak_pt, (p, t), "peak is a high-water mark, not a reading");
    }

    /// Past the soft cap the tolerance widens until a pair gives way, so the stack
    /// stays inside its data budget without any rule naming a stack shape. Beds are
    /// made progressively less alike so nothing merges at the base tolerance.
    #[test]
    fn the_soft_cap_leans_on_the_stack_until_a_pair_gives_way() {
        let cap = 4usize;
        let beds: Vec<Vec<(ElementId, f64)>> = (0..12)
            .map(|i| {
                let drift = i as f64 * 0.005;
                vec![
                    (8, (0.45 - drift) * BIG),
                    (14, (0.24 + drift) * BIG),
                    (12, 0.19 * BIG),
                    (26, 0.12 * BIG),
                ]
            })
            .collect();
        let mut c = col_with(&beds);
        let before = c.mass_kg();
        let mut settled = 0;
        for _ in 0..40 {
            let n = c.layers.len();
            c.reconcile(1200.0, 1200.0, GRAVITY_M_S2, AREA, cap);
            if c.layers.len() == n {
                settled += 1;
                if settled > 2 {
                    break;
                }
            } else {
                settled = 0;
            }
        }
        assert!(c.layers.len() <= cap, "the cap was leaned on: {} beds", c.layers.len());
        assert!(!c.layers.is_empty());
        assert!((c.mass_kg() - before).abs() < 1e-6 * before, "conserved through the squeeze");
    }

    /// The escape the bake telescope caught: a rain of THIN films — none ever
    /// carrying enough overburden to lithify — ran the stack to 180 beds against
    /// a cap of 20, because every shallow contact was exempt from the squeeze.
    /// Past the cap the guardrail pays regardless of lithification (two loose
    /// films are one loose bed); the equilibrium may ride a few beds over while
    /// the tolerance climbs, but the ruled range holds.
    #[test]
    fn the_cap_holds_against_a_rain_of_thin_films() {
        let cap = 6usize;
        // Sixty alternating THIN films (~1e-6 of a real bed): far too light for
        // any contact to reach LITHIFICATION_PA, and alternating so adjacent
        // pairs are genuinely unlike (dissimilarity ≈ 0.3).
        let thin = BIG * 1.0e-6;
        let beds: Vec<Vec<(ElementId, f64)>> = (0..60)
            .map(|i| {
                if i % 2 == 0 {
                    vec![(8, 0.45 * thin), (14, 0.24 * thin), (12, 0.19 * thin), (26, 0.12 * thin)]
                } else {
                    vec![(8, 0.30 * thin), (14, 0.10 * thin), (12, 0.30 * thin), (26, 0.30 * thin)]
                }
            })
            .collect();
        let mut c = col_with(&beds);
        let before = c.mass_kg();
        assert!(
            c.layers.iter().all(|b| b.peak_pt.0 < LITHIFICATION_PA),
            "the premise: nothing in this stack is lithified"
        );
        let mut settled = 0;
        for _ in 0..80 {
            let n = c.layers.len();
            c.reconcile(1200.0, 1200.0, GRAVITY_M_S2, AREA, cap);
            if c.layers.len() == n {
                settled += 1;
                if settled > 2 {
                    break;
                }
            } else {
                settled = 0;
            }
        }
        assert!(
            c.layers.len() <= cap + 4,
            "loose films outran the guardrail: {} beds against a cap of {cap}",
            c.layers.len()
        );
        assert!((c.mass_kg() - before).abs() < 1e-6 * before, "conserved through the squeeze");
    }

    /// Pressure is the weight above, spread over the cell — nothing else.
    #[test]
    fn overburden_is_the_weight_above() {
        let c = col_with(&[mafic(BIG), mafic(BIG), mafic(BIG)]);
        assert_eq!(overburden_pa(&c, 2, GRAVITY_M_S2, AREA), 0.0, "the top bed carries nothing");
        let one_bed = c.layers[0].mass_kg() * GRAVITY_M_S2 / AREA;
        assert!((overburden_pa(&c, 1, GRAVITY_M_S2, AREA) - one_bed).abs() < 1e-3);
        assert!((overburden_pa(&c, 0, GRAVITY_M_S2, AREA) - 2.0 * one_bed).abs() < 1e-3);
        assert!((basal_pressure_pa(&c, GRAVITY_M_S2, AREA) - 3.0 * one_bed).abs() < 1e-3);
    }

    /// The distance the whole lifecycle is written in terms of.
    #[test]
    fn dissimilarity_reads_proportions_not_amounts() {
        let (a, b) = (comp(&mafic(BIG)), comp(&mafic(BIG * 1000.0)));
        assert!(dissimilarity(&a, &b) < 1e-9, "same proportions, different amounts");
        assert!(dissimilarity(&a, &comp(&felsic(BIG))) > 0.1, "different proportions");
        assert_eq!(dissimilarity(&Composition::new(), &a), 1.0, "nothing shares nothing");
    }

    fn comp(v: &[(ElementId, f64)]) -> Composition {
        let mut c = Composition::new();
        for &(e, m) in v {
            c.add(e, m);
        }
        c
    }

    /// The production cap, mirrored so these tests do not depend on it.
    const STRATA_SOFT_CAP_FOR_TEST: usize = 12;
}
