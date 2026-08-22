//! [`Ledger`] — the sparse surface map of [`CellCoord`] → [`Cell`], and the
//! [`CellCoord`] that addresses it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cell::Cell;

/// Address of a surface column in the ledger — the `(x, z)` of its surface
/// cluster. The vertical `y` is dropped because the ledger is surface-only
/// (handoff §4). The voxel layer's thin `seam-to-voxel` maps a `ClusterId` to
/// this; the ledger itself stays decoupled from voxel storage (system spec §13),
/// so the dependency runs voxel → ledger, never the reverse.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellCoord {
    pub x: i32,
    pub z: i32,
}

impl CellCoord {
    /// A surface address from its `(x, z)` column coordinates.
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

/// The material ledger: a **sparse** surface field of [`Cell`]s. Only
/// materialized columns are stored; an absent coordinate means "not yet
/// materialized" — the default → materialize-on-touch → write-back lifecycle
/// (handoff §6). Ledgers are **inert between erosion sweeps** (system spec §5):
/// this type is just the retained store and its access doors; the sweep that
/// evolves it is deferred.
///
/// Persistence format is deliberately undecided here (spec §14 leaves it open).
/// The struct derives `serde` so a future format can serialize it; note that a
/// struct-keyed map does not round-trip through plain JSON — the eventual bake
/// format (e.g. an entries list, or the repo's compact-then-gzip convention)
/// will pin that.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    cells: HashMap<CellCoord, Cell>,
}

impl Ledger {
    /// An empty ledger (nothing materialized).
    pub fn new() -> Self {
        Self::default()
    }

    /// The cell at `coord`, or `None` if not yet materialized.
    pub fn get(&self, coord: CellCoord) -> Option<&Cell> {
        self.cells.get(&coord)
    }

    /// Mutable access to a materialized cell, or `None`.
    pub fn get_mut(&mut self, coord: CellCoord) -> Option<&mut Cell> {
        self.cells.get_mut(&coord)
    }

    /// Whether `coord` is materialized.
    pub fn contains(&self, coord: CellCoord) -> bool {
        self.cells.contains_key(&coord)
    }

    /// Materialize-on-touch: return the cell at `coord`, inserting a default
    /// (empty) one if absent. This is the single write door both epoch seeding
    /// and player/geology write-back go through (system spec §4).
    pub fn materialize(&mut self, coord: CellCoord) -> &mut Cell {
        self.cells.entry(coord).or_default()
    }

    /// Insert (or replace) the cell at `coord`, returning any previous cell.
    pub fn insert(&mut self, coord: CellCoord, cell: Cell) -> Option<Cell> {
        self.cells.insert(coord, cell)
    }

    /// Drop the cell at `coord` (e.g. lazy reclamation after a sweep frees it),
    /// returning it if present.
    pub fn remove(&mut self, coord: CellCoord) -> Option<Cell> {
        self.cells.remove(&coord)
    }

    /// Number of materialized cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// True if no cells are materialized.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Iterate materialized `(coord, cell)` pairs (unordered).
    pub fn iter(&self) -> impl Iterator<Item = (CellCoord, &Cell)> {
        self.cells.iter().map(|(&coord, cell)| (coord, cell))
    }

    /// Total element mass across every materialized cell — surface plus bulk.
    /// The accounting handle for the conservation invariant (system spec §4):
    /// the erosion sweep must keep this from drifting over geological time.
    pub fn total_mass(&self) -> f64 {
        self.cells
            .values()
            .map(|c| c.composition.total() + c.bulk_composition.total())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composition::Composition;
    use flicker_materials::ElementId;

    const FE: ElementId = 26;
    const SI: ElementId = 14;

    #[test]
    fn starts_empty() {
        let l = Ledger::new();
        assert!(l.is_empty());
        assert_eq!(l.len(), 0);
        assert_eq!(l.total_mass(), 0.0);
        assert!(l.get(CellCoord::new(0, 0)).is_none());
        assert!(!l.contains(CellCoord::new(0, 0)));
    }

    #[test]
    fn materialize_on_touch_inserts_and_persists() {
        let mut l = Ledger::new();
        let at = CellCoord::new(3, -7);
        // Absent before; touching it inserts a default cell.
        assert!(l.get(at).is_none());
        l.materialize(at).composition.add(FE, 500.0);
        assert_eq!(l.len(), 1);
        assert!(l.contains(at));
        // The mutation persisted at that coordinate.
        assert_eq!(l.get(at).unwrap().composition.amount(FE), 500.0);
        // A second materialize returns the *same* cell, not a fresh one.
        l.materialize(at).composition.add(FE, 100.0);
        assert_eq!(l.get(at).unwrap().composition.amount(FE), 600.0);
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn insert_remove_and_distinct_coords() {
        let mut l = Ledger::new();
        let a = CellCoord::new(0, 0);
        let b = CellCoord::new(1, 0);
        assert!(l
            .insert(a, Cell::from_surface(Composition::from_iter([(FE, 10.0)])))
            .is_none());
        assert!(l
            .insert(b, Cell::from_surface(Composition::from_iter([(SI, 20.0)])))
            .is_none());
        assert_eq!(l.len(), 2);
        let removed = l.remove(a).expect("a was present");
        assert_eq!(removed.composition.amount(FE), 10.0);
        assert_eq!(l.len(), 1);
        assert!(l.get(a).is_none());
        assert_eq!(l.get(b).unwrap().composition.amount(SI), 20.0);
    }

    #[test]
    fn total_mass_sums_surface_and_bulk_over_all_cells() {
        let mut l = Ledger::new();
        let c0 = l.materialize(CellCoord::new(0, 0));
        c0.composition.add(FE, 100.0);
        c0.bulk_composition.add(SI, 900.0);
        let c1 = l.materialize(CellCoord::new(5, 5));
        c1.composition.add(SI, 50.0);
        // 100 + 900 + 50.
        assert_eq!(l.total_mass(), 1050.0);
    }
}
