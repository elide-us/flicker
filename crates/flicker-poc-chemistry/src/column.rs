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

use crate::config::CELL_AREA_M2;

/// Atomic number of silicon (the silica-fraction read of [`crust_kind`]).
const SI: ElementId = 14;

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
}

impl Layer {
    /// Total conserved element mass in this bed.
    pub fn mass_kg(&self) -> f64 {
        self.elements.total()
    }
}

/// A single hex column of the world — one of the 92,162 cells. **NOT STORED:**
/// elevation, crust_kind, thickness, depth, density, hardness, age. Only
/// `relief_m` is stored, because it is *integrated* state (§4.6), not a function
/// of the current ledger.
#[derive(Clone, Debug)]
pub struct Column {
    /// Cell index `0..92_162` (matches the topology grid).
    pub cell_id: u32,
    /// The crust stack, bottom → top (index 0 = base of crust). **Empty at t=0:**
    /// crust is an OUTPUT, grown by later stages, never seeded.
    pub layers: Vec<Layer>,
    /// Integrated internal relief (§4.6): `relief += uplift − erosional_decay`.
    /// The only sub-hex quantity the formation phase carries.
    pub relief_m: f64,
    // Deferred to later milestones, kept OFF the struct so nothing derived is
    // stored: geotherm (M1), surface_water (M3), biomass (M5).
}

impl Column {
    /// An empty column for `cell_id` — no crust yet (the t=0 hot ball).
    pub fn empty(cell_id: u32) -> Self {
        Self {
            cell_id,
            layers: Vec::new(),
            relief_m: 0.0,
        }
    }

    /// Total conserved element mass stacked in this column.
    pub fn mass_kg(&self) -> f64 {
        self.layers.iter().map(Layer::mass_kg).sum()
    }

    /// Conserved mass of one element across the whole stack.
    pub fn element_mass(&self, element: ElementId) -> f64 {
        self.layers.iter().map(|l| l.elements.amount(element)).sum()
    }

    /// Deposit newly-formed crust at the TOP of the stack. Merges into the top bed
    /// if it shares `process` (so a growing basalt crust is ONE bed, not thousands
    /// of per-tick films — and a legible stack for the cutaway viewer); otherwise
    /// starts a new bed. The caller has already debited the source reservoir, so
    /// this is the credit half of a conserved move.
    pub fn deposit(&mut self, process: FormationProcess, formed_at_myr: f64, add: &[(ElementId, f64)]) {
        let merge = self.layers.last().map(|l| l.formed_by == process).unwrap_or(false);
        if !merge {
            self.layers.push(Layer {
                elements: Composition::new(),
                minerals: CompoundLedger::new(),
                formed_at_myr,
                formed_by: process,
                peak_pt: (0.0, 0.0),
            });
        }
        let top = self.layers.last_mut().expect("just ensured a top layer");
        for &(e, m) in add {
            top.elements.add(e, m);
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
/// rule says "rift basins are shallow". M0 columns are empty →
/// `Undifferentiated`; the silica / thickness thresholds firm up when real crust
/// and densities exist (M2/M6).
pub fn crust_kind(col: &Column) -> CrustKind {
    if col.layers.is_empty() {
        return CrustKind::Undifferentiated;
    }
    let mass = col.mass_kg();
    let silica_frac = if mass > 0.0 { col.element_mass(SI) / mass } else { 0.0 };
    // Silicic (felsic, light) reads continental; mafic reads oceanic. The two crust
    // populations the melt model produces sit either side of ~0.29 silica by mass
    // (mafic ocean basalt ≈ 0.24, refined continental ≈ 0.35), so the cut goes
    // between them. Refined by real modal-mineral density at the rock tier (M6).
    if silica_frac >= 0.29 {
        CrustKind::Continental
    } else {
        CrustKind::Oceanic
    }
}

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
    3000.0 - t * (3000.0 - 2650.0)
}

/// Thickness of a layer, m: `mass / (density × cell_area)` (§4.4). Areal, so it
/// inherits the equal-area caveat (§2.1) until worldgrid Slice 3b lands.
pub fn thickness_m(layer: &Layer) -> f64 {
    let d = density_kg_m3(layer);
    if d <= 0.0 {
        0.0
    } else {
        layer.mass_kg() / (d * CELL_AREA_M2)
    }
}

/// Total crust thickness of a column, m — the sum over its beds.
pub fn crust_thickness_m(col: &Column) -> f64 {
    col.layers.iter().map(thickness_m).sum()
}

/// Mean elevation of a column, m above the mantle datum — **the only place
/// elevation is derived** (§6.3). **Airy isostasy on absolute column mass and
/// density**, never a percentile rank (the ridge-spine artefact, §6.2): each bed
/// floats by `thickness · (ρ_mantle − ρ_bed) / ρ_mantle`, so a thick light
/// continental stack rides high and a thin dense oceanic one sits low. An empty
/// column is the bare mantle datum ⇒ 0. (The sea-level solve that turns this into
/// land vs sea-floor is M3.)
pub fn elevation_m(col: &Column) -> f64 {
    col.layers
        .iter()
        .map(|l| thickness_m(l) * (MANTLE_DENSITY - density_kg_m3(l)) / MANTLE_DENSITY)
        .sum()
}
