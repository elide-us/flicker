//! [`EpochSnapshot`] — the immutable per-epoch state the forward-regenerative cache
//! stores and the timeline reads. One snapshot is a whole planet at the end of one
//! epoch's many-step run: the per-cell [`HexState`] array (aligned to the sphere's
//! dense cell order) plus the cross-hex records that first appear at that epoch
//! (tectonic [`Plate`]s from E3, drainage [`Watershed`]s from E6), and a
//! [`Provenance`] stamp recording how it was produced.
//!
//! It is a **named struct, not a bare `Vec`**, so the data model can grow — new
//! first-class cross-hex records (vein paths, onset events, the heightmap-strata
//! handles of E7-9) attach as new `#[serde(default)]` fields without breaking the
//! cache keys or older `.epoch` bakes.
//!
//! **Conservation.** The only strictly-conserved reservoir is each cell's bulk
//! [`HexState::composition`] — the Epoch-1 element mass no later epoch depletes
//! (crust, atmosphere, deposits and the water pool are *derived*). [`conserved_mass`]
//! sums it; the engine checks it holds from the seed layer through every epoch.
//!
//! [`conserved_mass`]: EpochSnapshot::conserved_mass

use flicker_worldgen::{HexState, Plate, Watershed};
use serde::{Deserialize, Serialize};

/// How a snapshot was produced — the reproducibility stamp captured in a `.epoch`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// The epoch this snapshot is the output of (1..=9).
    pub epoch: u8,
    /// The per-epoch seed that drove it (`seed_chain[epoch - 1]`).
    pub seed: u64,
    /// How many sub-steps the stepper ran (1 for the accelerated analytic epochs;
    /// the sweep/pass count for the iterative water-cycle and erosion epochs).
    #[serde(default)]
    pub steps: u32,
    /// Bulk conserved mass at this epoch (a stored conservation witness).
    #[serde(default)]
    pub conserved_mass: f64,
}

/// One epoch's immutable output — a whole planet's per-cell state plus the
/// cross-hex records that exist by that epoch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochSnapshot {
    /// The epoch this is the output of (1..=9).
    pub epoch: u8,
    /// Per-cell state, indexed by the sphere's dense cell order (`0..N`).
    pub cells: Vec<HexState>,
    /// Tectonic plates (cross-hex) — present from Epoch 3 on, else empty.
    #[serde(default)]
    pub plates: Vec<Plate>,
    /// Drainage basins (cross-hex) — present from Epoch 6 on, else empty.
    #[serde(default)]
    pub watersheds: Vec<Watershed>,
    /// The planet's **global thermal state** at this epoch marker, normalized `0..1`
    /// (`1` = the molten magma ocean at birth, decaying toward space as it cools). The
    /// continuous-sim master clock: convection vigor, differentiation, and the tectonics
    /// onset all key off it (`flicker_worldgen::cooling`). **Distinct from the per-hex
    /// [`HexState::temperature`]** (a surface temperature in °C written by Epoch 4) —
    /// this is one number for the whole planet's heat budget. `1.0` at the Epoch-1 seed;
    /// old `.epoch` bakes without it default to `0`.
    ///
    /// [`HexState::temperature`]: flicker_worldgen::HexState::temperature
    #[serde(default)]
    pub temperature: f32,
    /// How this snapshot was produced.
    pub provenance: Provenance,
}

impl EpochSnapshot {
    /// Number of cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the snapshot has no cells (it never does for a real world).
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// The conserved bulk element mass across the planet — `Σ cell.composition.total()`.
    /// Invariant across every epoch (the epochs derive crust/atmosphere/deposits/
    /// water from the bulk without depleting it).
    pub fn conserved_mass(&self) -> f64 {
        self.cells.iter().map(|c| c.composition.total()).sum()
    }
}

/// Whether two masses agree to within a relative epsilon — the conservation test the
/// engine runs from the seed layer forward. Uses a relative tolerance because the
/// planet total is large and f64 sums accumulate rounding.
pub fn masses_agree(a: f64, b: f64, rel_eps: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= rel_eps * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masses_agree_within_tolerance() {
        assert!(masses_agree(1_000_000.0, 1_000_000.001, 1e-6));
        assert!(!masses_agree(1_000_000.0, 1_000_100.0, 1e-6));
        assert!(masses_agree(0.0, 0.0, 1e-9));
    }
}
