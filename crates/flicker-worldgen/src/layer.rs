//! The per-cell **vertical column** — the world's physical truth as a growable, dynamic
//! stack of [`Layer`]s from the deep interior up into the atmosphere.
//!
//! A cell is not a flat bag of epoch scalars; it is a [`LayerLedger`] — an ordered column
//! of strata (`[0]` deepest, `[len-1]` highest). Each [`Layer`] owns its material AND its
//! dynamics: `composition`, `heat`, and `motion` are three fields of the *same* object, so
//! "heat and motion are two aspects of one layer" is literally true.
//!
//! The column is **dynamic**: processes [`insert`](LayerLedger::insert),
//! [`remove`](LayerLedger::remove), and resize layers as the simulation iterates — a
//! stratum is not only appended on top (an early carbon atmosphere can grow a fresh crust
//! layer *beneath* it). New [`LayerKind`]s arrive with the processes that first produce
//! them.
//!
//! Material moves only through [`transfer`](LayerLedger::transfer) (vertical, between
//! layers) or the sim's neighbour exchange (horizontal, same layer across cells) — the
//! same conserve-by-construction discipline as [`Composition`]: total column mass is an
//! invariant. **Geometry is derived**: a layer's `thickness` is its drawn height, and the
//! cell's surface elevation is the sum of its layers' thicknesses — the shell renderer
//! draws one shell per layer at that height, so the data model and the picture are one read.

use flicker_materials::ElementId;
use flicker_worldstate::{Composition, CompoundLedger};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// What a [`Layer`] IS. Open-ended (`#[non_exhaustive]`): a new variant lands when the
/// process that first produces that stratum is built. Ordered coarsely deep→high so a
/// fresh column stacks sanely by default.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum LayerKind {
    /// The differentiated metallic centre (sinks out of the mantle as the planet cools).
    Core,
    /// The convecting silicate interior — the whole planet is one Mantle layer at t=0.
    Mantle,
    /// Solid rock skin frozen from the mantle at the surface.
    Crust,
    /// A condensed **liquid-water** body — the hydrosphere. Grows from delivered outer-system
    /// water once the surface has cooled into the liquid-water window; drains back to vapour
    /// if it warms out of it. Its size is whatever the condition process yields, never a target.
    Ocean,
    /// A **gas** envelope — delivered/outgassed volatiles held as vapour above the surface.
    /// "Atmosphere" here means *a layer of gas* of whatever composition the process produces,
    /// **not** necessarily breathable air.
    Atmosphere,
    // Sediment and other strata arrive with the processes that form them.
}

impl LayerKind {
    /// Stacking rank, deepest (`0`) → highest — where a layer of this kind sits in the
    /// column. The single source of vertical order, so every process that grows a layer
    /// ([`LayerLedger::ensure`]) inserts it in the physically correct place (an atmosphere
    /// above an ocean above the crust, whatever order the conditions actually fire in).
    pub fn rank(self) -> u8 {
        match self {
            LayerKind::Core => 0,
            LayerKind::Mantle => 1,
            LayerKind::Crust => 2,
            LayerKind::Ocean => 3,
            LayerKind::Atmosphere => 4,
        }
    }
}

/// One physical stratum of a cell's column — the atomic unit of material truth. It owns
/// its material and its dynamical state together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// What this stratum is.
    pub kind: LayerKind,
    /// Conserved element mass in THIS layer (the ledger — never invented or destroyed).
    pub composition: Composition,
    /// Conserved compounds/minerals formed within this layer, bounded by its elements.
    pub compounds: CompoundLedger,
    /// This layer's temperature (radiogenic production + advected heat accrue here).
    pub heat: f32,
    /// This layer's bulk flow — Coriolis-deflected in the deep layers, zonally sheared in
    /// the shallow ones: the per-layer motion a single cell-scalar could never hold.
    #[serde(with = "vec3_serde")]
    pub motion: Vec3,
    /// The layer's height in the column — its drawn vertical extent and its contribution to
    /// the cell's surface elevation. The sim keeps it consistent with the mass it holds.
    pub thickness: f32,
}

impl Layer {
    /// A new layer of `kind` holding `composition` at temperature `heat` with `thickness`
    /// height, initially still (no motion) with an empty compound ledger.
    pub fn new(kind: LayerKind, composition: Composition, heat: f32, thickness: f32) -> Self {
        Self {
            kind,
            composition,
            compounds: CompoundLedger::new(),
            heat,
            motion: Vec3::ZERO,
            thickness,
        }
    }

    /// Total conserved element mass in this layer.
    pub fn mass(&self) -> f64 {
        self.composition.total()
    }
}

/// A cell's full vertical column, deepest → highest. Growable and dynamic — the physical
/// truth every derived read (elevation, surface material, crust presence) reads off.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct LayerLedger {
    layers: Vec<Layer>,
}

impl LayerLedger {
    /// The **primordial column**: one undifferentiated [`Mantle`](LayerKind::Mantle) layer
    /// holding the whole accreted `bulk` at magma-ocean `heat`, occupying the full column
    /// (`thickness` 1.0). Core, crust, ocean, and air are all OUTPUTS grown from here —
    /// never seeded.
    pub fn from_primordial(bulk: Composition, heat: f32) -> Self {
        Self {
            layers: vec![Layer::new(LayerKind::Mantle, bulk, heat, 1.0)],
        }
    }

    /// An empty column (no strata) — a cell before it is seeded.
    pub fn empty() -> Self {
        Self { layers: Vec::new() }
    }

    /// The strata, deepest → highest.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }
    /// Mutable strata (for a process that rewrites layer fields in place).
    pub fn layers_mut(&mut self) -> &mut [Layer] {
        &mut self.layers
    }
    /// Number of strata.
    pub fn len(&self) -> usize {
        self.layers.len()
    }
    /// Whether the column has no strata.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// The index of the first layer of `kind`, if any ("where is the crust?").
    pub fn find(&self, kind: LayerKind) -> Option<usize> {
        self.layers.iter().position(|l| l.kind == kind)
    }

    /// The mantle layer (the convecting silicate interior), if the column has one.
    pub fn mantle(&self) -> Option<&Layer> {
        self.find(LayerKind::Mantle).map(|i| &self.layers[i])
    }

    /// The mantle layer, mutable — the layer molten convection stirs.
    pub fn mantle_mut(&mut self) -> Option<&mut Layer> {
        let i = self.find(LayerKind::Mantle)?;
        Some(&mut self.layers[i])
    }

    /// The layers immediately **below** and **above** index `i` — the vertical neighbours a
    /// cross-layer process reads (e.g. the crust a carbon atmosphere forms *beneath*).
    pub fn neighbours(&self, i: usize) -> (Option<&Layer>, Option<&Layer>) {
        let below = (i > 0).then(|| &self.layers[i - 1]);
        let above = self.layers.get(i + 1);
        (below, above)
    }

    /// Insert a stratum at position `at` (`0` = new deepest). Grows the column *anywhere* —
    /// not only on top.
    pub fn insert(&mut self, at: usize, layer: Layer) {
        self.layers.insert(at, layer);
    }
    /// Push a stratum onto the top of the column (the common "a new layer forms at the
    /// surface" case).
    pub fn push_top(&mut self, layer: Layer) {
        self.layers.push(layer);
    }
    /// Remove and return the stratum at `at` (e.g. a fully-subducted slab or a dissolved
    /// atmosphere).
    pub fn remove(&mut self, at: usize) -> Layer {
        self.layers.remove(at)
    }

    /// Find the layer of `kind`, or grow an empty one at its correct stacked position (by
    /// [`LayerKind::rank`]) at temperature `heat`, returning its index. The one entry point
    /// for "a process needs this layer to exist" — so core/crust/ocean/atmosphere each land
    /// in the right place regardless of the order their conditions fire.
    pub fn ensure(&mut self, kind: LayerKind, heat: f32) -> usize {
        if let Some(i) = self.find(kind) {
            return i;
        }
        let rank = kind.rank();
        let at = self
            .layers
            .iter()
            .position(|l| l.kind.rank() > rank)
            .unwrap_or(self.layers.len());
        self.layers
            .insert(at, Layer::new(kind, Composition::new(), heat, 0.0));
        at
    }

    /// **Conserved vertical transfer**: move the element mass in `part` from layer `from`
    /// to layer `to`, taking only what `from` actually holds (clamped), so total column mass
    /// is invariant. The credit-and-debit halves of every mantle→crust / crust→mantle move.
    pub fn transfer(&mut self, from: usize, to: usize, part: &Composition) {
        if from == to {
            return;
        }
        let wants: Vec<(ElementId, f64)> = part.iter().collect();
        for (el, want) in wants {
            let took = self.layers[from].composition.remove(el, want);
            if took > 0.0 {
                self.layers[to].composition.add(el, took);
            }
        }
    }

    /// Total conserved element mass across every stratum — the quantity the conservation
    /// invariant holds fixed as layers form, transfer, and dissolve.
    pub fn total_mass(&self) -> f64 {
        self.layers.iter().map(Layer::mass).sum()
    }

    /// The hex's **whole conserved composition** — every stratum's elements merged into
    /// one bag, regardless of which layer currently holds them. The single read for "how
    /// much of each element this hex contains", so a quantity that has migrated *within*
    /// the column (uranium sunk to the core, potassium risen into the crust) is still
    /// counted where it physically sits. Derived on demand from the layers — never a
    /// stored mirror, so it cannot drift from the strata that back it.
    pub fn total_composition(&self) -> Composition {
        let mut all = Composition::new();
        for l in &self.layers {
            all.add_composition(&l.composition);
        }
        all
    }

    /// The cell's surface **elevation**, DERIVED as the summed height of its strata: a
    /// column that grows a thick crust literally stands taller. (Any datum / normalization
    /// is the sim's; this is the raw stacked height the shell renderer draws to.)
    pub fn column_height(&self) -> f32 {
        self.layers.iter().map(|l| l.thickness).sum()
    }

    /// Set each layer's `thickness` to its share of the column's total mass — the simple
    /// volume proxy the shell renderer draws (true dense-vs-light volume is a later
    /// refinement). A process calls this after moving mass so the drawn heights stay
    /// consistent with what each layer holds. No-op for an empty / massless column.
    pub fn set_thickness_by_mass_share(&mut self) {
        let total = self.total_mass();
        if total <= 0.0 {
            return;
        }
        for l in &mut self.layers {
            l.thickness = (l.mass() / total) as f32;
        }
    }
}

/// Serialize [`Layer::motion`] as `[x, y, z]`, so the column round-trips into the `.epoch`
/// output without depending on glam's optional serde feature.
mod vec3_serde {
    use glam::Vec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &Vec3, s: S) -> Result<S::Ok, S::Error> {
        [v.x, v.y, v.z].serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec3, D::Error> {
        let [x, y, z] = <[f32; 3]>::deserialize(d)?;
        Ok(Vec3::new(x, y, z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: ElementId = 8;
    const FE: ElementId = 26;
    const SI: ElementId = 14;

    fn bulk() -> Composition {
        Composition::from_iter([(FE, 3200.0), (O, 3000.0), (SI, 1500.0)])
    }

    #[test]
    fn primordial_column_is_one_mantle_layer_holding_the_budget() {
        let c = LayerLedger::from_primordial(bulk(), 1.0);
        assert_eq!(c.len(), 1);
        assert_eq!(c.layers()[0].kind, LayerKind::Mantle);
        assert_eq!(c.total_mass(), bulk().total());
        assert_eq!(c.find(LayerKind::Mantle), Some(0));
        assert_eq!(c.find(LayerKind::Crust), None);
    }

    #[test]
    fn insert_and_remove_grow_and_shrink_the_column() {
        let mut c = LayerLedger::from_primordial(bulk(), 1.0);
        // A crust layer forms ON TOP of the mantle.
        c.push_top(Layer::new(
            LayerKind::Crust,
            Composition::from_iter([(SI, 100.0)]),
            0.2,
            0.05,
        ));
        assert_eq!(c.len(), 2);
        assert_eq!(c.find(LayerKind::Crust), Some(1)); // above the mantle
                                                       // A stratum can be inserted BENEATH the top, not only appended (the carbon-atmosphere
                                                       // -grows-crust-under-it case).
        c.insert(
            1,
            Layer::new(
                LayerKind::Crust,
                Composition::from_iter([(O, 10.0)]),
                0.3,
                0.01,
            ),
        );
        assert_eq!(c.len(), 3);
        // …and removed again (it subducted / dissolved).
        let gone = c.remove(1);
        assert_eq!(gone.kind, LayerKind::Crust);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn transfer_between_layers_conserves_total_mass() {
        let mut c = LayerLedger::from_primordial(bulk(), 1.0);
        c.push_top(Layer::new(LayerKind::Crust, Composition::new(), 0.2, 0.0));
        let before = c.total_mass();
        // Move some silica from the mantle (0) up into the crust (1).
        c.transfer(0, 1, &Composition::from_iter([(SI, 400.0)]));
        assert!(
            (c.total_mass() - before).abs() < 1e-9,
            "column mass must be conserved across a transfer"
        );
        assert_eq!(
            c.layers()[1].composition.amount(SI),
            400.0,
            "the crust received the silica"
        );
        assert_eq!(
            c.layers()[0].composition.amount(SI),
            1500.0 - 400.0,
            "the mantle gave it up"
        );
    }

    #[test]
    fn transfer_takes_only_what_the_source_holds() {
        let mut c = LayerLedger::from_primordial(bulk(), 1.0);
        c.push_top(Layer::new(LayerKind::Crust, Composition::new(), 0.2, 0.0));
        let before = c.total_mass();
        // Ask for far more iron than the mantle has — only what's present moves; conserved.
        c.transfer(0, 1, &Composition::from_iter([(FE, 1.0e9)]));
        assert!((c.total_mass() - before).abs() < 1e-9);
        assert_eq!(
            c.layers()[0].composition.amount(FE),
            0.0,
            "the mantle gave all its iron"
        );
        assert_eq!(
            c.layers()[1].composition.amount(FE),
            3200.0,
            "the crust got exactly what existed"
        );
    }

    #[test]
    fn neighbours_read_below_and_above() {
        let mut c = LayerLedger::from_primordial(bulk(), 1.0); // [0] mantle
        c.push_top(Layer::new(LayerKind::Crust, Composition::new(), 0.2, 0.1)); // [1]
        c.push_top(Layer::new(LayerKind::Crust, Composition::new(), 0.1, 0.2)); // [2]
        let (below, above) = c.neighbours(1);
        assert_eq!(below.map(|l| l.kind), Some(LayerKind::Mantle));
        assert_eq!(above.map(|l| l.kind), Some(LayerKind::Crust));
        assert!(
            c.neighbours(0).0.is_none(),
            "nothing below the deepest layer"
        );
        assert!(c.neighbours(2).1.is_none(), "nothing above the top layer");
    }

    #[test]
    fn column_height_is_the_summed_thickness() {
        let mut c = LayerLedger::from_primordial(bulk(), 1.0); // thickness 1.0
        c.push_top(Layer::new(LayerKind::Crust, Composition::new(), 0.2, 0.05));
        c.push_top(Layer::new(LayerKind::Crust, Composition::new(), 0.1, 0.03));
        assert!((c.column_height() - (1.0 + 0.05 + 0.03)).abs() < 1e-6);
    }
}
