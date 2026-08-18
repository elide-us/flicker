//! [`HexWorld`] — a body's surface stored as a grid of conserved composition: the
//! **planet-scale macro-voxel** (spec §8), and the world-storage foundation the evolution
//! iteration steps over.
//!
//! Composition theory at two scales: a voxel cluster is a container for a portion of a
//! *cluster's* material distribution (micro, ~128 ft); a hex is the same idea at *planet*
//! scale — a container for a portion of a *planet's* material distribution. `HexWorld` is the
//! array of those hexes for one body: one conserved [`Composition`] per cell, ordered parallel
//! to the icosphere topology (`flicker_worldgrid`) that the renderer and the steppers supply.
//!
//! Storage only — **topology lives elsewhere** (the epochs are topology-agnostic; they take
//! `dirs`/`neighbors` as `EpochCtx`). The single load-bearing property here is **conservation**:
//! every evolution step *moves* material between cells, never creating or destroying it, so a
//! `HexWorld`'s [`total`](HexWorld::total) is invariant under stepping.

use flicker_materials::ElementId;
use flicker_worldstate::Composition;
use serde::{Deserialize, Serialize};

/// A body's surface as `cells.len()` hexes, each a conserved element [`Composition`] — the
/// material truth the evolution moves around. `freq` is the icosphere resolution the cell order
/// matches (`cells.len() == 10·freq² + 2` for a full sphere). Serializable so an evolved
/// planet's state can be captured for the game.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HexWorld {
    /// Icosphere resolution this grid was built at (the §8 hex budget — see [`crate::hex`]).
    pub freq: u32,
    cells: Vec<Composition>,
}

impl HexWorld {
    /// An empty world: `n` cells of no mass at resolution `freq`.
    pub fn new(freq: u32, n: usize) -> Self {
        Self {
            freq,
            cells: vec![Composition::new(); n],
        }
    }

    /// A world from an explicit per-cell distribution (e.g. a materialised composition spread).
    pub fn from_cells(freq: u32, cells: Vec<Composition>) -> Self {
        Self { freq, cells }
    }

    /// Number of hex cells.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// The per-cell compositions, in icosphere-cell order.
    pub fn cells(&self) -> &[Composition] {
        &self.cells
    }

    /// Mutable per-cell access — for a stepper that rewrites the whole grid at once.
    pub fn cells_mut(&mut self) -> &mut [Composition] {
        &mut self.cells
    }

    /// One cell's composition, if `i` is in range.
    pub fn cell(&self, i: usize) -> Option<&Composition> {
        self.cells.get(i)
    }

    /// Total mass across every cell — the conserved quantity every step must preserve.
    pub fn total(&self) -> f64 {
        self.cells.iter().map(Composition::total).sum()
    }

    /// Move up to `amount` of `element` from cell `from` into cell `to`, returning the mass
    /// actually moved. The conservation-safe transport primitive the advection / density-sort
    /// steps build on: it can never move more than `from` holds, and total mass is unchanged.
    /// A no-op if the indices are equal or out of range.
    pub fn transfer(&mut self, from: usize, to: usize, element: ElementId, amount: f64) -> f64 {
        let n = self.cells.len();
        if from == to || from >= n || to >= n {
            return 0.0;
        }
        let moved = self.cells[from].remove(element, amount);
        self.cells[to].add(element, moved);
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Atomic numbers used in the tests.
    const FE: ElementId = 26;
    const O: ElementId = 8;

    fn seeded() -> HexWorld {
        HexWorld::from_cells(
            2,
            vec![
                Composition::from_iter([(FE, 100.0), (O, 50.0)]),
                Composition::from_iter([(O, 25.0)]),
                Composition::new(),
            ],
        )
    }

    #[test]
    fn totals_sum_the_cells() {
        let w = seeded();
        assert_eq!(w.cell_count(), 3);
        assert_eq!(w.total(), 175.0);
        assert_eq!(w.cell(0).unwrap().amount(FE), 100.0);
    }

    #[test]
    fn transfer_moves_material_and_conserves_total() {
        let mut w = seeded();
        let before = w.total();
        let moved = w.transfer(0, 1, FE, 30.0);
        assert_eq!(moved, 30.0);
        assert_eq!(w.cell(0).unwrap().amount(FE), 70.0);
        assert_eq!(w.cell(1).unwrap().amount(FE), 30.0);
        assert_eq!(w.total(), before, "transport conserves total mass");
        // Over-asking takes only what's there; same-cell / out-of-range are no-ops.
        assert_eq!(w.transfer(0, 1, FE, 999.0), 70.0);
        assert_eq!(w.transfer(1, 1, O, 5.0), 0.0);
        assert_eq!(w.transfer(0, 9, O, 5.0), 0.0);
        assert_eq!(w.total(), before, "still conserved");
    }

    #[test]
    fn serde_round_trip_captures_state() {
        let w = seeded();
        let json = serde_json::to_string(&w).unwrap();
        let back: HexWorld = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
    }
}
