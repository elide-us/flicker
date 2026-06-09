//! `HexState` — the per-hex working state that flows through the epoch chain.
//!
//! Each epoch reads the previous state and adds or transforms fields,
//! accumulating from a raw molten composition (Epoch 1) toward a gameplay-planet
//! foundation: a differentiated **crust** (Epoch 2) and plate-driven
//! **elevation** — continents and mountains (Epoch 3). The ground output the
//! erosion (water-cycle) sim consumes is the `elevation` heightmap together with
//! the `crust` / `composition` it erodes.

use flicker_worldstate::Composition;

/// A hex's boundary relationship to its neighbours' plates (Epoch 3).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
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

/// Per-hex state accumulated across the epoch chain. Cheap to clone (the
/// pass-through epochs and the per-layer stack snapshots both clone it).
#[derive(Clone, Debug, PartialEq)]
pub struct HexState {
    /// Bulk composition — the conserved element mass (Epoch 1).
    pub composition: Composition,
    /// Light fraction differentiated to the surface — the crust (Epoch 2). Empty
    /// until Epoch 2 runs.
    pub crust: Composition,
    /// Crust mass as a fraction of the bulk (Epoch 2), `0..1`.
    pub crust_fraction: f64,
    /// Volcanic activity `0..1` (Epoch 2: thin crust; raised at Epoch 3
    /// convergent boundaries).
    pub volcanic: f32,
    /// Plate id (Epoch 3).
    pub plate: u16,
    /// Whether this hex's plate is continental (vs oceanic) (Epoch 3).
    pub continental: bool,
    /// Boundary relationship to neighbouring plates (Epoch 3).
    pub boundary: Boundary,
    /// Mean surface elevation — the proto-heightmap (Epoch 3): `-1` deep ocean ..
    /// `+1` peak. The ground output the erosion sim refines.
    pub elevation: f32,
    /// Orogenic fold intensity `0..1` — the convergence strength where plates
    /// collide (Epoch 3). Drives the folded, lifted mountain relief in the field.
    pub orogeny: f32,
}

impl HexState {
    /// Initial state from Epoch 1's bulk composition: undifferentiated, no plate,
    /// at sea level.
    pub fn new(composition: Composition) -> Self {
        Self {
            composition,
            crust: Composition::new(),
            crust_fraction: 0.0,
            volcanic: 0.0,
            plate: 0,
            continental: false,
            boundary: Boundary::Interior,
            elevation: 0.0,
            orogeny: 0.0,
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
