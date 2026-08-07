//! [`Budget`] — the immutable **bulk-planet accretion seed** (spec §3.1).
//!
//! At t=0 the planet is an undifferentiated hot ball; this is its total element
//! inventory in absolute kg. Core, mantle, crust, ocean and atmosphere are all
//! *partitions* the simulation produces from it — never seeded. This REPLACES
//! the old `abundance.json` (a *crustal* assay: 46% O, 5% Fe, no Mg lever), which
//! fed the answer in as the input and made a metallic core impossible.
//!
//! Every element is explicit; there is no `abundance_floor`. The weights are
//! mass-% of the total planet mass and sum to 100, so the budget totals exactly
//! [`PLANET_MASS_KG`](crate::config::PLANET_MASS_KG).

use std::collections::BTreeMap;
use std::path::Path;

use flicker_materials::{ElementId, JsonTableSource, MaterialError, Tables};
use serde::Deserialize;

use crate::config::content_data_dir;

/// Filename of the accretion seed within a content directory.
pub const ACCRETION_FILE: &str = "accretion.json";

/// Tolerance on the mass-% weight sum (the data rounds to ~99.999999).
const WEIGHT_TOL: f64 = 0.05;

/// What can go wrong loading the accretion seed.
#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error("reading accretion seed `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing accretion seed `{path}`: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("loading the periodic table: {0}")]
    Tables(#[from] MaterialError),
    #[error(
        "accretion seed names element `{0}`, which is absent from periodic_table.json — \
         sync it (e.g. Li per Book III ruling F3450870)"
    )]
    UnknownElement(String),
    #[error("accretion weights sum to {0:.6}, not 100 — every element must be explicit and total the planet mass")]
    WeightsUnbalanced(f64),
}

#[derive(Deserialize)]
struct AccretionFile {
    #[serde(rename = "_meta")]
    meta: AccretionMeta,
    elements: Vec<AccretionRow>,
}
#[derive(Deserialize)]
struct AccretionMeta {
    planet_mass_kg: f64,
}
#[derive(Deserialize)]
struct AccretionRow {
    symbol: String,
    weight: f64,
}

/// The conserved starting inventory: absolute accreted mass per element, keyed by
/// atomic number. **Immutable** once built — the right-hand side of the
/// conservation invariant (spec §4.3) never changes; only `Delivered` / `Escaped`
/// move over the run.
#[derive(Clone, Debug)]
pub struct Budget {
    accreted_kg: BTreeMap<ElementId, f64>,
    planet_mass_kg: f64,
}

impl Budget {
    /// Load the accretion seed from the repo's `Alpha/content/data`.
    pub fn from_repo() -> Result<Self, BudgetError> {
        let dir = content_data_dir();
        let tables = Tables::from_source(&JsonTableSource::new(&dir))?;
        Self::from_dir(&dir, &tables)
    }

    /// Load the accretion seed from `dir`, mapping element symbols → atomic number
    /// through `tables`. Errors if a symbol is absent from the periodic table (the
    /// exact failure the Li sync fixes) or if the weights do not sum to 100 — a
    /// floor is a way of *not deciding*, and the data is the source of truth.
    pub fn from_dir(dir: &Path, tables: &Tables) -> Result<Self, BudgetError> {
        let path = dir.join(ACCRETION_FILE);
        let bytes = std::fs::read(&path).map_err(|source| BudgetError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let file: AccretionFile =
            serde_json::from_slice(&bytes).map_err(|source| BudgetError::Parse {
                path: path.display().to_string(),
                source,
            })?;

        let weight_sum: f64 = file.elements.iter().map(|r| r.weight).sum();
        if (weight_sum - 100.0).abs() > WEIGHT_TOL {
            return Err(BudgetError::WeightsUnbalanced(weight_sum));
        }

        let planet_mass_kg = file.meta.planet_mass_kg;
        let mut accreted_kg = BTreeMap::new();
        for row in &file.elements {
            let el = tables
                .element(&row.symbol)
                .ok_or_else(|| BudgetError::UnknownElement(row.symbol.clone()))?;
            // Mass-% of the whole planet → absolute kg. Summed masses, never
            // fractions: conservation is add/subtract, no per-pass renormalisation.
            *accreted_kg.entry(el.number).or_insert(0.0) += row.weight / 100.0 * planet_mass_kg;
        }
        Ok(Self {
            accreted_kg,
            planet_mass_kg,
        })
    }

    /// A re-endowed copy: the same seed with per-element multipliers applied —
    /// the Starter's knobs. Built BEFORE a world is born from it, so "immutable
    /// once built" still means what it says: this is a different budget for a
    /// different planet, never an edit to a running one's right-hand side.
    ///
    /// The planet's mass FOLLOWS the elements — triple the iron and the world
    /// is simply heavier — because a budget whose parts no longer summed to its
    /// whole would break the conservation invariant on the first audit. Scales
    /// clamp at zero; an element the seed never held stays absent.
    pub fn rescaled(&self, scales: &[(ElementId, f64)]) -> Self {
        let mut accreted_kg = self.accreted_kg.clone();
        for &(element, factor) in scales {
            if let Some(mass) = accreted_kg.get_mut(&element) {
                *mass *= factor.max(0.0);
            }
        }
        let planet_mass_kg = accreted_kg.values().sum();
        Self { accreted_kg, planet_mass_kg }
    }

    /// Accreted mass of one element (0 if absent — but every element is explicit).
    pub fn accreted(&self, element: ElementId) -> f64 {
        self.accreted_kg.get(&element).copied().unwrap_or(0.0)
    }

    /// Total accreted mass — equals [`PLANET_MASS_KG`] to within float rounding.
    pub fn total(&self) -> f64 {
        self.accreted_kg.values().sum()
    }

    /// The planet mass this budget was scaled to.
    pub fn planet_mass_kg(&self) -> f64 {
        self.planet_mass_kg
    }

    /// Iterate `(element, accreted_kg)` in ascending atomic-number order.
    pub fn iter(&self) -> impl Iterator<Item = (ElementId, f64)> + '_ {
        self.accreted_kg.iter().map(|(&e, &m)| (e, m))
    }

    /// Number of distinct elements in the seed (28 after the Li sync).
    pub fn len(&self) -> usize {
        self.accreted_kg.len()
    }
    /// Whether the seed is empty (it never is once loaded).
    pub fn is_empty(&self) -> bool {
        self.accreted_kg.is_empty()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_rescaled_budget_is_a_different_planet_whose_parts_still_sum() {
        let b = Budget::from_repo().expect("seed loads");
        let fe = 26u8;
        let h = 1u8;
        let heavy = b.rescaled(&[(fe, 3.0), (h, 0.1)]);
        assert!((heavy.accreted(fe) - 3.0 * b.accreted(fe)).abs() < 1.0, "iron tripled");
        assert!((heavy.accreted(h) - 0.1 * b.accreted(h)).abs() < 1.0, "hydrogen decimated");
        // The whole follows the parts — conservation's right-hand side is
        // internally consistent, not pinned to the old mass.
        assert!(
            (heavy.total() - heavy.planet_mass_kg()).abs() < 1e-3 * heavy.total(),
            "total and planet mass agree after the re-endowment"
        );
        assert!(heavy.total() > b.total(), "more iron than lost hydrogen: heavier world");
        // And the untouched elements are untouched.
        assert_eq!(heavy.accreted(14), b.accreted(14), "silicon unchanged");
    }
    use super::*;
    use crate::config::PLANET_MASS_KG;

    #[test]
    fn loads_bulk_seed_from_repo() {
        let b = Budget::from_repo().expect("accretion.json loads from the repo");
        assert_eq!(b.len(), 28, "every element explicit — 28 after the Li sync");
        // Lithium (Z=3) resolves — the periodic-table sync landed.
        assert!(b.accreted(3) > 0.0, "lithium present (Z=3)");
        // The bulk planet is IRON-dominated (~32%), not oxygen — the whole point:
        // the old crustal seed had O at 46% and Fe at 5%.
        assert!(
            b.accreted(26) > b.accreted(8),
            "bulk planet is iron-dominated, not oxygen-dominated"
        );
        // Magnesium (Z=12) is a major reservoir (~14%), not a trace floor.
        assert!(b.accreted(12) > b.accreted(20), "Mg is a major element, not a floor");
        // Sums to Earth mass.
        let rel = (b.total() - PLANET_MASS_KG).abs() / PLANET_MASS_KG;
        assert!(rel < 1e-3, "budget sums to Earth mass (rel err {rel:.2e})");
    }
}
