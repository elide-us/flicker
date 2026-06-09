//! One row of the periodic table (`data/materials/periodic_table.json`).
//!
//! Fields mirror the JSON, which mixes three provenances (see that file's
//! `_meta`): *book* fields transcribed from Prism BookIII (the canonical design
//! book — `valence_electrons` is the book's gameplay value, not strict IUPAC),
//! *physical* fields with real-world values (epoch 1 distributes by
//! mass/abundance, epoch 2 differentiates by `density_g_cm3`), and *trait*
//! fields — the base [`hardness`/`brittleness`/`water_capacity`] an element
//! contributes to a blend (see [`crate::Tables::blend_traits`]).
//!
//! [`hardness`]: Element::hardness
//! [`brittleness`]: Element::brittleness
//! [`water_capacity`]: Element::water_capacity

use serde::Deserialize;

/// An element's stable identity — its atomic number. Compositions and the
/// simulation key elements by this (not by symbol): it is small, stable, and
/// independent of table load order.
pub type ElementId = u8;

/// Bulk phase of an element at reference conditions. A closed physical set, so
/// an unrecognized value is a real error rather than a row to tolerate.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhysicalState {
    Solid,
    Liquid,
    Gas,
}

/// A single element. Unknown JSON fields are ignored (so the table can grow new
/// columns without breaking the loader); the `category` is kept as a free
/// string because the design vocabulary is open-ended and meant to expand.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Element {
    /// Display name, e.g. `"Iron"`.
    pub name: String,
    /// Atomic number (1..=92 in the current table).
    pub number: u8,
    /// Chemical symbol, e.g. `"Fe"` — the key compositions are written in.
    pub symbol: String,
    /// Book category, e.g. `"transition_metal"` (free-form; grows with design).
    pub category: String,
    /// Book-assigned common valence electron count (gameplay value, not IUPAC).
    pub valence_electrons: u8,
    /// Real-world atomic mass (epoch 1 distributes by mass/abundance).
    pub atomic_mass: f32,
    /// Real-world density, g/cm³ (epoch 2 differentiates by density).
    pub density_g_cm3: f32,
    /// Real-world melting point, °C.
    pub melting_point_c: f32,
    /// Bulk phase at reference conditions.
    pub state: PhysicalState,
    /// Erosion resistance, Mohs-like `0..=10` — base trait, blendable.
    pub hardness: f32,
    /// Fracture tendency, `0` ductile .. `1` shatters — base trait, blendable.
    pub brittleness: f32,
    /// Porosity / water held per unit, `0..=1` — base trait, blendable. This is
    /// a *property* (capacity); the live saturation is per-cluster sim state.
    pub water_capacity: f32,
    /// Free-text gameplay uses, transcribed from the book.
    pub uses: String,
}
