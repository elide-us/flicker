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

/// Surface gravity, m/s² — turns the mass stacked above a bed into the pressure
/// on it.
pub const GRAVITY_M_S2: f64 = 9.81;

/// How unlike the bed beneath arriving material must be before it starts its own
/// bed instead of thickening that one. A mass-fraction distance (see
/// [`dissimilarity`]), so ~0.1 means "a tenth of the composition changed".
const BED_SPAWN_DISSIMILARITY: f64 = 0.10;

/// Material thinner than this can never be its own bed, however exotic — a film
/// settling on a bed joins that bed. Keeps a per-tick trickle from shredding the
/// stack into thousands of unresolvable films.
///
/// It is also the floor at which a bed being consumed is taken WHOLE rather
/// than by another fraction: a layer worn down toward zero carries two ledgers
/// whose last digits have drifted apart, and past this point the drift is
/// larger than the rock (see [`Delamination`](crate::crust::Delamination)).
pub const MIN_BED_MASS_KG: f64 = 1.0e15;

/// How alike two buried beds must be before burial erases the boundary between
/// them. Tighter than the spawn threshold on purpose: a boundary that formed is
/// not undone by the same contrast that made it.
const MERGE_DISSIMILARITY: f64 = 0.04;

/// Overburden a bed must have carried before it can merge at all. Below it the
/// beds are still loose and the contact is still a contact.
const LITHIFICATION_PA: f64 = 2.0e7;

/// How much each bed over the soft cap widens the merge tolerance. The cap is a
/// data-volume guardrail — every bed is potentially another 2K map per tile — and
/// this is how it is paid: by letting the most-alike pair go, never by capping the
/// count outright. At this pressure the tolerance passes total dissimilarity
/// (1.0) about twelve beds past the cap, so even a stack of mutually alien beds
/// has a hard ceiling within reach of the ruled range (20).
const CAP_PRESSURE_PER_BED: f64 = 2.0;

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
    /// **How far this bed's minerals have reorganised into the cold, dense
    /// assemblage**, `0..1` — the gabbro→eclogite transition, advanced by
    /// [`CrustDensification`](crate::crust::CrustDensification).
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
    pub densified: f64,
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
            Some(top) => {
                let mut incoming = Composition::new();
                for &(e, m) in add {
                    incoming.add(e, m);
                }
                dissimilarity(&top.elements, &incoming) > BED_SPAWN_DISSIMILARITY
            }
        };
        if spawn {
            self.layers.push(Layer {
                elements: Composition::new(),
                minerals: CompoundLedger::new(),
                formed_at_myr,
                formed_by: process,
                peak_pt: (0.0, 0.0),
            densified: 0.0,
            });
        }
        let top = self.layers.last_mut().expect("just ensured a top layer");
        for &(e, m) in add {
            top.elements.add(e, m);
        }
    }

    /// The stack's own housekeeping, run once the tick's material has moved:
    /// **record what each bed has endured, then let burial erase the boundaries
    /// that burial erases.**
    ///
    /// Two consequences, no targets:
    /// - Every bed's `peak_pt` takes the max of what it has ever seen. Pressure is
    ///   the load above it, temperature is the rock it sits in. Monotone, permanent
    ///   — this is the record the metamorphic chemistry later reads.
    /// - Adjacent beds that are alike **and** loaded merge into one. A bed boundary
    ///   is a contrast; squeeze two similar beds together hard enough and there is
    ///   no contrast left to see. Mass moves whole, so the merge is exactly
    ///   conservative.
    ///
    /// `soft_cap` is a **resource** guardrail, not a shape target: as the stack
    /// grows past it the tolerance widens, so the pair that merges is always the
    /// most-alike pair available. Nothing here ever names a desired stack.
    pub fn reconcile(&mut self, temp_k: f64, cell_area_m2: f64, soft_cap: usize) {
        if self.layers.is_empty() {
            return;
        }
        // Load above each bed, walked top-down so each bed sees what it carries.
        let mut above = 0.0f64;
        for i in (0..self.layers.len()).rev() {
            let p = above * GRAVITY_M_S2 / cell_area_m2;
            let bed = &mut self.layers[i];
            bed.peak_pt = (bed.peak_pt.0.max(p), bed.peak_pt.1.max(temp_k));
            above += bed.mass_kg();
        }

        // Over the soft cap the tolerance opens up, so the most-alike loaded pair
        // gives way first. Under it, only genuinely-alike pairs merge.
        let over = self.layers.len().saturating_sub(soft_cap) as f64;
        let tolerance = MERGE_DISSIMILARITY * (1.0 + over * CAP_PRESSURE_PER_BED);
        let mut i = self.layers.len().saturating_sub(1);
        while i > 0 {
            // The load ON THE CONTACT is what welds it: the contact sits at the base
            // of bed `i`, which is the top of bed `i - 1`, so the pressure there is
            // what bed `i - 1` carries — bed `i` plus everything above it. A fresh
            // film on a deep pile therefore does NOT weld (its contact is shallow),
            // while a deep pair does. Reading bed `i`'s own load instead would say
            // the topmost contact is never loaded, which is never true of anything
            // but the top surface itself.
            let loaded = self.layers[i - 1].peak_pt.0 >= LITHIFICATION_PA;
            // Past the ruled cap the guardrail pays REGARDLESS of lithification:
            // two loose films are one loose bed — there is no welded contrast to
            // preserve between them — and a cap that a rain of thin films can
            // outrun is no cap at all (measured: 180 beds against a cap of 20,
            // every shallow contact exempt). Under the cap the weld gate stands,
            // so genuinely distinct young strata keep their contacts.
            if (loaded || over > 0.0)
                && dissimilarity(&self.layers[i - 1].elements, &self.layers[i].elements) <= tolerance
            {
                let upper = self.layers.remove(i);
                let lower = &mut self.layers[i - 1];
                // Whole-mass move: every gram lands, so the audit sees no change.
                for (e, m) in upper.elements.iter() {
                    lower.elements.add(e, m);
                }
                for (c, m) in upper.minerals.iter() {
                    lower.minerals.add(c, m);
                }
                // The merged bed remembers the harsher history of the two.
                lower.peak_pt = (
                    lower.peak_pt.0.max(upper.peak_pt.0),
                    lower.peak_pt.1.max(upper.peak_pt.1),
                );
            }
            i -= 1;
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
    let silicate =
        silicate + layer.densified.clamp(0.0, 1.0) * ECLOGITE_GAIN * eclogite_former_frac(layer);

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

/// How much denser fully-eclogitised mafic rock is than the fresh basalt it was,
/// kg/m³. Real eclogite runs ~3500 against basalt's ~2900, and this is that
/// difference: enough that cold sea floor clears [`MANTLE_DENSITY`] with room
/// to spare. That crossing is what turns an ocean floor into an ocean BASIN,
/// and Earth's own abyssal plains lie ~4 km below its continental shelves for
/// exactly this reason.
pub const ECLOGITE_GAIN: f64 = 560.0;

/// **How much of this bed can actually become eclogite**, `0..1` — its
/// magnesium, iron and calcium, which are what a garnet is built out of.
///
/// The separation the whole basin mechanism rests on: melts drawn by
/// [`oceanic_affinity`](crate::crust::oceanic_affinity) run ~0.27 here and
/// eclogitise fully, while
/// [`continental_affinity`](crate::crust::continental_affinity)'s refined melts
/// run ~0.085 and score zero — so sea floor founders and continents never can,
/// out of the two melts' own chemistry rather than a rule naming either.
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

/// Overburden pressure on the TOP of bed `index`, Pa — **derived**, never stored:
/// the weight of everything stacked above it, spread over the cell.
///
/// This is what makes a stack's weight causal rather than decorative. It drives
/// compaction (which beds merge), it is the `peak_pt` a bed carries forever, and
/// it is the `P` the metamorphic chemistry will read when a buried carbon bed
/// reorganises into something harder.
pub fn overburden_pa(col: &Column, index: usize, cell_area_m2: f64) -> f64 {
    let above: f64 = col.layers.iter().skip(index + 1).map(Layer::mass_kg).sum();
    above * GRAVITY_M_S2 / cell_area_m2.max(f64::MIN_POSITIVE)
}

/// Pressure at the BASE of the column, Pa — the whole stack's weight. What the
/// deepest bed carries.
pub fn basal_pressure_pa(col: &Column, cell_area_m2: f64) -> f64 {
    col.mass_kg() * GRAVITY_M_S2 / cell_area_m2.max(f64::MIN_POSITIVE)
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

    /// A cell's worth of area, for the pressure reads.
    const AREA: f64 = 5.534e9;
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
            densified: 0.0,
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

    /// Burial erases a boundary between beds that are alike — and every gram of
    /// both is still there afterwards.
    #[test]
    fn burial_merges_alike_beds_and_conserves_every_gram() {
        let mut c = col_with(&[mafic(BIG), mafic(BIG), mafic(BIG)]);
        let before = c.mass_kg();
        // Load them first (peak_pt is what the merge reads), then let it act.
        c.reconcile(1200.0, AREA, STRATA_SOFT_CAP_FOR_TEST);
        c.reconcile(1200.0, AREA, STRATA_SOFT_CAP_FOR_TEST);
        assert_eq!(c.layers.len(), 1, "alike, loaded beds become one bed");
        assert!(
            (c.mass_kg() - before).abs() < 1e-6 * before,
            "the merge moved whole mass: {before} → {}",
            c.mass_kg()
        );
    }

    /// Beds that are genuinely different survive burial. A boundary is a contrast,
    /// and pressure alone does not invent a similarity that is not there.
    #[test]
    fn burial_does_not_merge_beds_that_differ() {
        let mut c = col_with(&[mafic(BIG), felsic(BIG)]);
        for _ in 0..5 {
            c.reconcile(1200.0, AREA, STRATA_SOFT_CAP_FOR_TEST);
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
            c.reconcile(1200.0, AREA, STRATA_SOFT_CAP_FOR_TEST);
        }
        assert_eq!(c.layers.len(), 2, "no load, no compaction");
    }

    /// A bed remembers the worst it ever carried — the record the metamorphic
    /// chemistry reads later. Monotone: unloading never lowers it.
    #[test]
    fn a_bed_remembers_the_worst_it_has_seen() {
        let mut c = col_with(&[mafic(BIG), felsic(BIG)]);
        c.reconcile(1500.0, AREA, STRATA_SOFT_CAP_FOR_TEST);
        let (p, t) = c.layers[0].peak_pt;
        assert!(p > 0.0 && (t - 1500.0).abs() < 1e-9, "the buried bed recorded its load");
        assert_eq!(c.layers[1].peak_pt.0, 0.0, "the top bed carries nothing");

        // Strip the load and cool it: the record must not fall.
        c.layers.pop();
        c.reconcile(300.0, AREA, STRATA_SOFT_CAP_FOR_TEST);
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
            c.reconcile(1200.0, AREA, cap);
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
        assert!(c.layers.len() >= 1);
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
            c.reconcile(1200.0, AREA, cap);
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
        assert_eq!(overburden_pa(&c, 2, AREA), 0.0, "the top bed carries nothing");
        let one_bed = c.layers[0].mass_kg() * GRAVITY_M_S2 / AREA;
        assert!((overburden_pa(&c, 1, AREA) - one_bed).abs() < 1e-3);
        assert!((overburden_pa(&c, 0, AREA) - 2.0 * one_bed).abs() < 1e-3);
        assert!((basal_pressure_pa(&c, AREA) - 3.0 * one_bed).abs() < 1e-3);
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
