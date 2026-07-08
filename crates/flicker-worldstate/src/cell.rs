//! [`Cell`] — one surface column's ledger entry ("what a pixel actually is",
//! system spec §4) — and its persistent [`Effects`].

use flicker_materials::{ElementId, ElementTraits, MaterialId, Tables};
use serde::{Deserialize, Serialize};

use crate::composition::Composition;

/// Persistent surface **effects**. Water, ice, and lava are *not* stacked layers
/// but tracked state (handoff §4): their per-pass motion is viscosity-gated and
/// belongs to the (deferred) sweep — this struct is only the state it reads and
/// writes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Effects {
    /// Surface water saturation. The classified material's `water_capacity`
    /// bounds it; crossing a threshold spawns a Rivulet — but that threshold and
    /// the Rivulet engine are deferred (handoff §5-6).
    pub water_saturation: f32,
    /// Frozen-water effect amount.
    pub ice: f32,
    /// Molten effect amount.
    pub lava: f32,
}

impl Effects {
    /// No water, ice, or lava.
    pub const DRY: Self = Self {
        water_saturation: 0.0,
        ice: 0.0,
        lava: 0.0,
    };
}

/// One surface column's material ledger. **Surface-only:** [`composition`] is
/// the surface cluster's element vector; everything beneath is the 100%-solid
/// [`bulk_composition`], materialized only as players dig/reveal (handoff §4).
/// [`surface_material`] is the classified face — `None` until the (deferred)
/// classifier runs. Shape (height) is never stored: it is a bake-time function
/// of this ledger (system spec §4).
///
/// [`composition`]: Cell::composition
/// [`bulk_composition`]: Cell::bulk_composition
/// [`surface_material`]: Cell::surface_material
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// Surface element composition — the conserved per-column truth.
    pub composition: Composition,
    /// The solid column beneath the surface (sparse; empty until revealed).
    pub bulk_composition: Composition,
    /// Classified surface material id, or `None` if not yet classified.
    pub surface_material: Option<MaterialId>,
    /// Surface effect state (water / ice / lava).
    pub effects: Effects,
}

impl Cell {
    /// A cell with the given surface composition and nothing else set (dry, no
    /// bulk revealed, unclassified). The shape epoch seeding produces.
    pub fn from_surface(composition: Composition) -> Self {
        Self {
            composition,
            ..Self::default()
        }
    }

    /// Effective element traits of the surface composition — the
    /// `Σ fractionᵢ·traitᵢ` blend (tier ②'s "derived aggregate traits",
    /// handoff §1), computed through the vocabulary `tables`. This is the
    /// *fallback* basis: once [`Cell::surface_material`] is classified, that
    /// formed material's authoritative traits override it. An empty composition
    /// returns [`ElementTraits::ZERO`].
    pub fn surface_traits(&self, tables: &Tables) -> ElementTraits {
        tables.blend_traits_by_number(self.composition.iter())
    }

    /// The dominant surface element — for a dominant-element tint or a coarse
    /// "what is this mostly" query.
    pub fn dominant_element(&self) -> Option<ElementId> {
        self.composition.dominant()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::JsonTableSource;

    const SI: ElementId = 14;
    const O: ElementId = 8;
    const FE: ElementId = 26;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    #[test]
    fn from_surface_leaves_everything_else_default() {
        let cell = Cell::from_surface(Composition::from_iter([(SI, 100.0)]));
        assert_eq!(cell.composition.amount(SI), 100.0);
        assert!(cell.bulk_composition.is_empty());
        assert_eq!(cell.surface_material, None);
        assert_eq!(cell.effects, Effects::DRY);
    }

    #[test]
    fn surface_traits_blend_through_the_vocabulary() {
        let t = tables();
        // A pure-iron surface blends to iron's own base traits.
        let cell = Cell::from_surface(Composition::from_iter([(FE, 1000.0)]));
        let fe = t.element("Fe").unwrap();
        let traits = cell.surface_traits(&t);
        assert_eq!(traits.hardness, fe.hardness);
        assert_eq!(traits.brittleness, fe.brittleness);
        // A mixed surface lands between its constituents' hardness.
        let mixed = Cell::from_surface(Composition::from_iter([(SI, 8000.0), (O, 2000.0)]));
        let mt = mixed.surface_traits(&t);
        let (si, o) = (t.element("Si").unwrap(), t.element("O").unwrap());
        assert!(mt.hardness <= si.hardness.max(o.hardness));
        assert!(mt.hardness >= si.hardness.min(o.hardness));
        // An empty surface has zero traits, not NaN.
        assert_eq!(Cell::default().surface_traits(&t), ElementTraits::ZERO);
    }

    #[test]
    fn dominant_element_reads_the_composition() {
        let cell = Cell::from_surface(Composition::from_iter([(SI, 900.0), (O, 600.0), (FE, 50.0)]));
        assert_eq!(cell.dominant_element(), Some(SI));
        assert_eq!(Cell::default().dominant_element(), None);
    }

    #[test]
    fn serde_json_round_trip() {
        let mut cell = Cell::from_surface(Composition::from_iter([(FE, 7000.0), (SI, 8000.0)]));
        cell.bulk_composition.add(FE, 50_000.0);
        cell.surface_material = Some(10); // Granite
        cell.effects.water_saturation = 0.3;
        let json = serde_json::to_string(&cell).unwrap();
        let back: Cell = serde_json::from_str(&json).unwrap();
        assert_eq!(cell, back);
    }
}
