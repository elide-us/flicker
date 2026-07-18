//! [`CompoundLedger`] — a compound-keyed mass vector, the **second conserved
//! ledger** of the material pipeline (Elements → **Compounds** → Materials).
//!
//! Where [`crate::Composition`] holds absolute *element* mass, this holds absolute
//! *compound* mass (`CompoundId` → amount): how much of each real compound (H₂O,
//! SiO₂, CaCO₃, Fe₂O₃, an ore, …) the epoch simulation has formed in a cell. It is
//! populated by the epoch-aligned compound *former* (elements → compounds by
//! stoichiometry) and by additive **delivery** (most water arrives from the outer
//! system). Like the element ledger it changes only through conservation-safe
//! [`add`](CompoundLedger::add) / [`remove`](CompoundLedger::remove).
//!
//! The invariant tying the two ledgers together lives in the former: the element
//! mass locked inside the compounds here never exceeds what the element ledger
//! holds — compounds are an accounting *of* the element budget, not new matter
//! (delivery excepted, which adds to both).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable catalog id of a compound (matches `compounds.json` / `CompoundDef::id`).
pub type CompoundId = u16;

/// Absolute mass per compound. Stored sparsely, ordered by id, so iteration and
/// serialization are deterministic. `f64` for the same reason the element ledger
/// is: many small per-pass transfers accumulate over geological time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CompoundLedger {
    amounts: BTreeMap<CompoundId, f64>,
}

impl CompoundLedger {
    /// An empty ledger (no compounds formed).
    pub fn new() -> Self {
        Self::default()
    }

    /// Mass of one compound (`0.0` if absent).
    pub fn amount(&self, compound: CompoundId) -> f64 {
        self.amounts.get(&compound).copied().unwrap_or(0.0)
    }

    /// Total mass across all compounds.
    pub fn total(&self) -> f64 {
        self.amounts.values().sum()
    }

    /// Number of distinct compounds present.
    pub fn len(&self) -> usize {
        self.amounts.len()
    }

    /// True if no compound holds any mass.
    pub fn is_empty(&self) -> bool {
        self.amounts.is_empty()
    }

    /// Deposit `amount` of `compound` (non-positive is a no-op; removal is the
    /// conservation-safe [`Self::remove`]).
    pub fn add(&mut self, compound: CompoundId, amount: f64) {
        if amount <= 0.0 {
            return;
        }
        *self.amounts.entry(compound).or_insert(0.0) += amount;
    }

    /// Remove up to `amount` of `compound`, returning the mass actually removed —
    /// never more than was present. Drops the key at zero to keep storage sparse.
    pub fn remove(&mut self, compound: CompoundId, amount: f64) -> f64 {
        if amount <= 0.0 {
            return 0.0;
        }
        let Some(have) = self.amounts.get_mut(&compound) else {
            return 0.0;
        };
        let taken = amount.min(*have);
        *have -= taken;
        if *have <= 0.0 {
            self.amounts.remove(&compound);
        }
        taken
    }

    /// The compound holding the greatest mass, if any — the coarse "what is this
    /// mostly made of" query. On a tie the higher id wins (deterministic).
    pub fn dominant(&self) -> Option<CompoundId> {
        self.amounts
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(&compound, _)| compound)
    }

    /// Iterate `(compound, amount)` in ascending id order.
    pub fn iter(&self) -> impl Iterator<Item = (CompoundId, f64)> + '_ {
        self.amounts.iter().map(|(&compound, &amount)| (compound, amount))
    }
}

impl FromIterator<(CompoundId, f64)> for CompoundLedger {
    fn from_iter<T: IntoIterator<Item = (CompoundId, f64)>>(iter: T) -> Self {
        let mut c = CompoundLedger::new();
        for (compound, amount) in iter {
            c.add(compound, amount);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_remove_conserve_and_stay_sparse() {
        let mut led = CompoundLedger::new();
        led.add(1, 100.0); // e.g. Water
        led.add(1, 50.0);
        led.add(15, 200.0); // e.g. Hematite
        assert_eq!(led.amount(1), 150.0);
        assert_eq!(led.total(), 350.0);
        assert_eq!(led.len(), 2);
        assert_eq!(led.remove(1, 999.0), 150.0);
        assert_eq!(led.amount(1), 0.0);
        assert_eq!(led.len(), 1);
        assert_eq!(led.dominant(), Some(15));
    }

    #[test]
    fn serde_round_trip() {
        let c = CompoundLedger::from_iter([(1u16, 7000.0), (15u16, 8000.0)]);
        let json = serde_json::to_string(&c).unwrap();
        let back: CompoundLedger = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
