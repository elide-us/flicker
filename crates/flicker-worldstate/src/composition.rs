//! [`Composition`] — an element-keyed mass vector, the conserved quantity of
//! the whole material model.
//!
//! The erosion sweep and the Rivulets move element mass between compositions
//! (system spec §4-6), and that mass must never drift — "shape can differ every
//! bake; mass cannot." So the only ways to change a composition are a deposit
//! ([`Composition::add`]) and a conservation-safe take ([`Composition::remove`],
//! which never yields more than is present and reports what it took). There is
//! no setter that could silently create or destroy matter.

use std::collections::BTreeMap;

use flicker_materials::ElementId;
use serde::{Deserialize, Serialize};

/// Absolute mass per element (`ElementId` → amount). Stored sparsely — only
/// elements actually present — and ordered by atomic number, so iteration and
/// serialization are deterministic. Amounts are `f64`: many small per-pass
/// transfers accumulate over geological time and the conservation invariant is
/// unforgiving of rounding.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    amounts: BTreeMap<ElementId, f64>,
}

impl Composition {
    /// An empty composition (no mass).
    pub fn new() -> Self {
        Self::default()
    }

    /// Mass of one element (`0.0` if absent).
    pub fn amount(&self, element: ElementId) -> f64 {
        self.amounts.get(&element).copied().unwrap_or(0.0)
    }

    /// Total mass across all elements.
    pub fn total(&self) -> f64 {
        self.amounts.values().sum()
    }

    /// Number of distinct elements present.
    pub fn len(&self) -> usize {
        self.amounts.len()
    }

    /// True if no element holds any mass.
    pub fn is_empty(&self) -> bool {
        self.amounts.is_empty()
    }

    /// Deposit `amount` of `element`. Non-positive `amount` is a no-op — taking
    /// mass out goes through [`Self::remove`] so removal stays conservation-safe
    /// (you can never drive an element below zero by "adding" a negative).
    pub fn add(&mut self, element: ElementId, amount: f64) {
        if amount <= 0.0 {
            return;
        }
        *self.amounts.entry(element).or_insert(0.0) += amount;
    }

    /// Remove up to `amount` of `element`, returning the mass **actually
    /// removed** — never more than was present, because a column cannot yield
    /// matter it does not hold. This is the transaction primitive: a player
    /// excavating and a river eroding are the same call (system spec §4). The
    /// element is dropped from the map once it reaches zero, keeping storage
    /// sparse.
    pub fn remove(&mut self, element: ElementId, amount: f64) -> f64 {
        if amount <= 0.0 {
            return 0.0;
        }
        let Some(have) = self.amounts.get_mut(&element) else {
            return 0.0;
        };
        let taken = amount.min(*have);
        *have -= taken;
        if *have <= 0.0 {
            self.amounts.remove(&element);
        }
        taken
    }

    /// Fold another composition into this one — a confluence/merge. Adds each
    /// element's mass, so the total becomes the sum of the two totals. The
    /// conservation-honest way to reconcile quantities at a junction (the lesson
    /// of system spec §6: account on the quantity, derive the geometry).
    pub fn add_composition(&mut self, other: &Composition) {
        for (&element, &amount) in &other.amounts {
            self.add(element, amount);
        }
    }

    /// The element holding the greatest mass, if any — the basis for a
    /// dominant-element tint and a coarse "what is this mostly" query. On an
    /// exact tie the higher atomic number wins (deterministic by iteration
    /// order).
    pub fn dominant(&self) -> Option<ElementId> {
        self.amounts
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(&element, _)| element)
    }

    /// Iterate `(element, amount)` in ascending atomic-number order.
    pub fn iter(&self) -> impl Iterator<Item = (ElementId, f64)> + '_ {
        self.amounts.iter().map(|(&element, &amount)| (element, amount))
    }
}

impl FromIterator<(ElementId, f64)> for Composition {
    fn from_iter<T: IntoIterator<Item = (ElementId, f64)>>(iter: T) -> Self {
        let mut c = Composition::new();
        for (element, amount) in iter {
            c.add(element, amount);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Atomic numbers used in the tests: Si = 14, Fe = 26, O = 8.
    const SI: ElementId = 14;
    const FE: ElementId = 26;
    const O: ElementId = 8;

    #[test]
    fn empty_is_zero() {
        let c = Composition::new();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
        assert_eq!(c.total(), 0.0);
        assert_eq!(c.amount(FE), 0.0);
        assert_eq!(c.dominant(), None);
    }

    #[test]
    fn add_accumulates() {
        let mut c = Composition::new();
        c.add(FE, 100.0);
        c.add(FE, 50.0);
        c.add(SI, 200.0);
        assert_eq!(c.amount(FE), 150.0);
        assert_eq!(c.amount(SI), 200.0);
        assert_eq!(c.total(), 350.0);
        assert_eq!(c.len(), 2);
        // Non-positive deposits do nothing and never create a key.
        c.add(O, 0.0);
        c.add(O, -10.0);
        assert_eq!(c.amount(O), 0.0);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn remove_is_clamped_and_reports_what_it_took() {
        let mut c = Composition::from_iter([(FE, 100.0)]);
        // Partial take.
        assert_eq!(c.remove(FE, 30.0), 30.0);
        assert_eq!(c.amount(FE), 70.0);
        // Over-take yields only what was present, and drops the element.
        assert_eq!(c.remove(FE, 999.0), 70.0);
        assert_eq!(c.amount(FE), 0.0);
        assert!(c.is_empty());
        // Removing an absent element takes nothing.
        assert_eq!(c.remove(SI, 5.0), 0.0);
    }

    #[test]
    fn moving_mass_between_compositions_conserves_total() {
        let mut src = Composition::from_iter([(FE, 1000.0), (SI, 500.0)]);
        let mut dst = Composition::new();
        let before = src.total() + dst.total();
        // Erode 250 of iron from src to dst — the rivulet transport primitive.
        let moved = src.remove(FE, 250.0);
        dst.add(FE, moved);
        assert_eq!(moved, 250.0);
        assert_eq!(src.total() + dst.total(), before, "mass leaked or bred");
        assert_eq!(src.amount(FE), 750.0);
        assert_eq!(dst.amount(FE), 250.0);
    }

    #[test]
    fn merge_conserves_and_sums() {
        let mut a = Composition::from_iter([(FE, 100.0), (SI, 200.0)]);
        let b = Composition::from_iter([(FE, 50.0), (O, 25.0)]);
        let before = a.total() + b.total();
        a.add_composition(&b);
        assert_eq!(a.amount(FE), 150.0);
        assert_eq!(a.amount(SI), 200.0);
        assert_eq!(a.amount(O), 25.0);
        assert_eq!(a.total(), before);
    }

    #[test]
    fn dominant_picks_the_heaviest() {
        let c = Composition::from_iter([(SI, 800.0), (O, 400.0), (FE, 100.0)]);
        assert_eq!(c.dominant(), Some(SI));
    }

    #[test]
    fn iter_is_atomic_number_ordered() {
        let c = Composition::from_iter([(FE, 1.0), (O, 1.0), (SI, 1.0)]);
        let order: Vec<ElementId> = c.iter().map(|(e, _)| e).collect();
        assert_eq!(order, vec![O, SI, FE]); // 8, 14, 26
    }

    #[test]
    fn serde_json_round_trip() {
        let c = Composition::from_iter([(FE, 7000.0), (SI, 8000.0)]);
        let json = serde_json::to_string(&c).unwrap();
        let back: Composition = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
