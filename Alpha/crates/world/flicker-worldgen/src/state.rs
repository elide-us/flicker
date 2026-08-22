//! `HexState` — the per-hex working state that flows through the epoch chain.
//!
//! Each epoch reads the previous state and adds or transforms fields,
//! accumulating from a raw molten composition (Epoch 1) toward a gameplay-planet
//! foundation: a differentiated **crust** (Epoch 2) and plate-driven
//! **elevation** — continents and mountains (Epoch 3). The ground output the
//! erosion (water-cycle) sim consumes is the `elevation` heightmap together with
//! the `crust` / `composition` it erodes.

use flicker_materials::ElementId;
use flicker_worldstate::{Composition, CompoundLedger};
use serde::{Deserialize, Serialize};

use crate::layer::LayerLedger;

/// Dominant biome of a hex's surface (Epoch 6), from temperature + moisture +
/// elevation. A Whittaker-style classification; the runtime reads it to dress
/// the surface.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum Biome {
    /// Submerged — below sea level.
    #[default]
    Ocean,
    /// Frozen waste — very cold.
    Ice,
    /// Cold, treeless.
    Tundra,
    /// Cold, moist — boreal conifer forest.
    Taiga,
    /// Temperate, dry — open grass.
    Grassland,
    /// Temperate, moist — deciduous forest.
    Forest,
    /// Hot, wet.
    Rainforest,
    /// Hot, seasonally dry — grass + scattered trees.
    Savanna,
    /// Hot, arid.
    Desert,
    /// Above the tree line — bare rock / permanent snow.
    Alpine,
}

/// A hex's boundary relationship to its neighbours' plates (Epoch 3).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Boundary {
    /// Plate interior — all neighbours share this hex's plate.
    #[default]
    Interior,
    /// Plates pushing together — mountains / volcanism.
    Convergent,
    /// Plates pulling apart — rifts / new ocean floor.
    Divergent,
    /// Plates sliding past — faults.
    Transform,
}

/// Where a hex sits on the **life thread** — woven across Epochs 4–6, advanced and
/// never regressed. Precursor chemistry (Epoch 4) → microbial mats at the vents
/// (Epoch 5) → fungus and flora on land (Epoch 6). Ordered so gates can ask
/// `stage >= Floral`.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum LifeStage {
    /// No appreciable chemistry.
    #[default]
    Barren,
    /// Prebiotic precursor compounds present, but no life yet (Epoch 4).
    Prebiotic,
    /// Prokaryotic microbial life — mats at hydrothermal vents / rich cradles (Epoch 5).
    Microbial,
    /// Ground fungus / decomposers (Epoch 6).
    Fungal,
    /// Flora / forest (Epoch 6).
    Floral,
}

/// Per-hex state accumulated across the epoch chain. Cheap to clone (the
/// pass-through epochs and the per-layer stack snapshots both clone it).
///
/// Serializable so an epoch snapshot can be baked to a `.epoch` file and
/// reloaded. `#[serde(default)]` on the container means **any field missing from an
/// older `.epoch` is filled from [`HexState::default`]** — so the state can grow new
/// fields (the redesign expects the data model to expand) without breaking old bakes.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HexState {
    /// Bulk composition — the conserved element mass (Epoch 1).
    pub composition: Composition,
    /// **The vertical column** — this cell's growable stack of
    /// [`Layer`](crate::layer::Layer)s (core → mantle → crust → …), the physical truth the
    /// flat fields below are migrating onto. Seeded as one primordial mantle at t=0;
    /// processes grow and `transfer` between layers, conserved. This is the durable state
    /// the `.epoch` output serializes.
    pub column: LayerLedger,
    /// Light fraction differentiated to the surface — the crust (Epoch 2). Empty
    /// until Epoch 2 runs.
    pub crust: Composition,
    /// Crust mass as a fraction of the bulk (Epoch 2), `0..1`.
    pub crust_fraction: f64,
    /// Mantle **convection heat** `0..1` (Epoch 2): the self-organized thermal field of
    /// the molten era — hot **upwelling** cells (light material rising) vs cold
    /// **downwelling** cells (dense material sinking). It emerges from the composition's
    /// buoyancy relaxed into coherent cells, and is the driver Epoch 3's plate seams form
    /// over: downwelling → convergent/subduction seams, upwelling → divergent ridges.
    /// `0` before Epoch 2.
    pub heat: f32,
    /// Volcanic activity `0..1` (Epoch 2: thin crust; raised at Epoch 3
    /// convergent boundaries).
    pub volcanic: f32,
    /// Plate id (Epoch 3).
    pub plate: u16,
    /// Whether this hex's plate is continental (vs oceanic) (Epoch 3).
    pub continental: bool,
    /// Crust age in millions of years (Epoch 3): ~0 at divergent spreading ridges
    /// (newly created sea floor), rising with distance toward the subducting
    /// margins; old (cratonic) on continental plates. `0` before Epoch 3.
    pub plate_age: f32,
    /// Boundary relationship to neighbouring plates (Epoch 3).
    pub boundary: Boundary,
    /// Mean surface elevation — the proto-heightmap (Epoch 3): `-1` deep ocean ..
    /// `+1` peak. The ground output the erosion sim refines.
    pub elevation: f32,
    /// Orogenic fold intensity `0..1` — the convergence strength where plates
    /// collide (Epoch 3). Drives the folded, lifted mountain relief in the field.
    pub orogeny: f32,
    /// Global sea level in the normalized elevation space, once the hydrosphere
    /// forms (Epoch 4). `0` before it runs.
    pub sea_level: f32,
    /// Water depth at this hex (`sea_level - elevation`, clamped ≥ 0) (Epoch 4).
    pub water_depth: f32,
    /// Surface temperature from latitude + elevation lapse (Epoch 4).
    pub temperature: f32,
    /// Local atmospheric makeup (Epoch 4): the planet's well-mixed outgassed
    /// volatiles (H₂O vapor, CO₂, N₂, SO₂, HCl — tracked as their elements) plus a
    /// local water-vapor loading over warm water. Empty before Epoch 4. The gas /
    /// hydrosphere reservoir the runtime water cycle and greenhouse read.
    pub atmosphere: Composition,
    /// Baseline precipitation / effective moisture `0..1` (Epoch 4):
    /// ocean-proximity modulated by warmth (floored so cold-wet coasts stay
    /// moist) — the single moisture truth Epoch 6 reads for biomes and flora.
    /// After the Epoch-4 **water cycle** runs it is the *emergent* rain field
    /// (rain shadows behind mountains, wet windward coasts), not the static
    /// estimate. `0` before Epoch 4.
    pub precipitation: f32,
    /// Liquid water standing on the surface (water-cycle state, Epoch 4) — oceans
    /// and lakes. One of the three conserved water phases
    /// (`surface_water + humidity + ice`); the cycle moves mass between them and
    /// across neighbours without creating or destroying any. `0` before Epoch 4.
    pub surface_water: f32,
    /// Atmospheric water vapour over the cell (water-cycle state, Epoch 4) — the
    /// **gaseous** phase the wind advects between cells before it rains out. `0`
    /// before Epoch 4.
    pub humidity: f32,
    /// Frozen water — ice caps / glaciers (water-cycle state, Epoch 4): surface
    /// water freezes below 0°C and melts above it. `0` before Epoch 4.
    pub ice: f32,
    /// Prebiotic chemistry `0..1` — accumulated life-**precursor compounds** (first
    /// written Epoch 4, then carried forward). Life/chemistry is a cross-cutting
    /// thread, not a single epoch: precursors brew where liquid water, organic
    /// elements (C/N/P/S) and energy (warmth + volcanism, incl. the hotspot island
    /// chains) coincide, over the epoch's cycles — warm shallow seas and wet
    /// volcanic shores are the cradles. The seed the later life thread grows from.
    /// `0` before Epoch 4.
    pub prebiotic: f32,
    /// Life-thread stage (Epochs 4–6): Barren → Prebiotic → Microbial → Fungal →
    /// Floral, advanced as each epoch's conditions allow.
    pub life_stage: LifeStage,
    /// Standing living matter `0..1` — Epoch 5 microbial mats, grown by Epoch 6
    /// flora. `0` where lifeless.
    pub biomass: f32,
    /// Dead organic matter accumulated on the surface (Epoch 6) — litter shed by
    /// flora over time. The peat / precursor that time-gated preservation later
    /// turns into coal & oil. `0` before Epoch 6.
    pub organics: f32,
    /// Preserved mineral **deposits** — a ledger distinct from the bulk rock
    /// (`composition`/`crust`), the seed of the harvestable underground layer
    /// (Epoch 6). Carbon from organics that escaped late decomposers (coal on land,
    /// oil under sea); calcium + carbon carbonate from shelled plankton (chalk).
    /// Empty before Epoch 6.
    pub deposits: Composition,
    /// Hydrothermal mineralization intensity `0..1` (Epoch 5): high along the
    /// active fault/boundary plumbing (with fluid present) where metals
    /// precipitate. Drives where ore veins can form. `0` before it runs.
    pub hydrothermal: f32,
    /// The metal this hex's ore vein carries, if a vein runs through it
    /// (Epoch 5). `None` off a vein.
    pub vein_element: Option<ElementId>,
    /// Vein concentration at this hex `0..1` (Epoch 5): peaks near the vein
    /// source and tapers toward the tips. `0` off a vein.
    pub vein_strength: f32,
    /// Drainage flow accumulated at this hex (Epoch 6): the rainfall gathered
    /// from everything that drains through it. High along trunk rivers — the
    /// water sim's starting flow field. `0` before Epoch 6.
    pub flow: f32,
    /// Loose sediment deposited on this hex by macro-erosion (Epoch 6), in
    /// normalized elevation units — the soft cover the water sim moves first.
    pub sediment: f32,
    /// Drainage basin id (Epoch 6): the index of the terminal sink this hex drains
    /// to by steepest descent — hexes sharing a value form one watershed. `0`
    /// before Epoch 6.
    pub watershed: u32,
    /// Dominant biome (Epoch 6).
    pub biome: Biome,
    /// **The compound ledger** (material pipeline stage 2) — how much of each real
    /// compound (`compounds.json` ids: H₂O, SiO₂, CaCO₃, ores, …) the epoch chain
    /// has formed here, from this cell's elements (by stoichiometry) plus additive
    /// delivery (most water arrives from the outer system). The element mass locked
    /// in these compounds never exceeds `composition`; gameplay harvests compounds.
    /// Empty before any chemistry runs.
    pub compounds: CompoundLedger,
}

impl HexState {
    /// Initial state from Epoch 1's bulk composition: undifferentiated, no plate,
    /// at sea level. Every other field takes its [`Default`] (all zero / `Barren` /
    /// `Ocean`), so growing the data model with new fields needs no change here.
    pub fn new(composition: Composition) -> Self {
        let column = LayerLedger::from_primordial(composition.clone(), 1.0);
        Self {
            composition,
            column,
            ..Default::default()
        }
    }

    /// The composition visible at the surface: the crust once differentiated,
    /// otherwise the bulk composition.
    pub fn surface(&self) -> &Composition {
        if self.crust.is_empty() {
            &self.composition
        } else {
            &self.crust
        }
    }
}
