//! [`Tables`] — the loaded vocabulary, indexed for lookup, plus the
//! composition-weighted [`Tables::blend_traits`].

use std::collections::HashMap;

use crate::element::{Element, ElementId};
use crate::material::MaterialDef;
use crate::source::{MaterialError, TableSource};

/// Effective element-level traits of a composition — the Σ fractionᵢ·traitᵢ
/// blend. Viscosity is absent on purpose: it is a material-only property (a raw
/// composition has none until it forms a material that overrides these).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ElementTraits {
    pub hardness: f32,
    pub brittleness: f32,
    pub water_capacity: f32,
}

impl ElementTraits {
    /// The all-zero blend — returned for an empty or all-unknown composition.
    pub const ZERO: Self = Self {
        hardness: 0.0,
        brittleness: 0.0,
        water_capacity: 0.0,
    };
}

/// `Σ fractionᵢ·traitᵢ` over `(element, amount)` pairs, normalized by the total
/// amount. Non-positive amounts contribute nothing; an empty or all-zero input
/// yields [`ElementTraits::ZERO`]. The shared core of the symbol- and
/// number-keyed blends, after each has resolved its keys to elements.
fn blend_elements<'a>(items: impl IntoIterator<Item = (&'a Element, f64)>) -> ElementTraits {
    let (mut total, mut h, mut b, mut w) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (e, amount) in items {
        if amount <= 0.0 {
            continue;
        }
        total += amount;
        h += amount * e.hardness as f64;
        b += amount * e.brittleness as f64;
        w += amount * e.water_capacity as f64;
    }
    if total <= 0.0 {
        return ElementTraits::ZERO;
    }
    ElementTraits {
        hardness: (h / total) as f32,
        brittleness: (b / total) as f32,
        water_capacity: (w / total) as f32,
    }
}

/// The loaded material vocabulary: element and material rows, indexed for O(1)
/// lookup by the keys the simulation uses (element symbol / atomic number,
/// material id / name). Build it once from a [`TableSource`] and query it; it
/// is read-only after construction.
pub struct Tables {
    elements: Vec<Element>,
    materials: Vec<MaterialDef>,
    by_symbol: HashMap<String, usize>,
    by_number: HashMap<u8, usize>,
    material_by_id: HashMap<u8, usize>,
    material_by_name: HashMap<String, usize>,
}

impl Tables {
    /// Load and index the vocabulary from a [`TableSource`]. This is the single
    /// construction path — `source` decides whether the rows came from JSON, a
    /// web service, or a DB.
    pub fn from_source(source: &impl TableSource) -> Result<Self, MaterialError> {
        let elements = source.load_elements()?;
        let materials = source.load_materials()?;
        Ok(Self::from_rows(elements, materials))
    }

    /// Index already-loaded rows. Exposed for callers that obtained the rows by
    /// other means (and for tests); most code uses [`Self::from_source`].
    pub fn from_rows(elements: Vec<Element>, materials: Vec<MaterialDef>) -> Self {
        let by_symbol = elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.symbol.clone(), i))
            .collect();
        let by_number = elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.number, i))
            .collect();
        let material_by_id = materials
            .iter()
            .enumerate()
            .map(|(i, m)| (m.id, i))
            .collect();
        let material_by_name = materials
            .iter()
            .enumerate()
            .map(|(i, m)| (m.name.clone(), i))
            .collect();
        Self {
            elements,
            materials,
            by_symbol,
            by_number,
            material_by_id,
            material_by_name,
        }
    }

    /// All element rows, in load order.
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    /// All material rows, in load order.
    pub fn materials(&self) -> &[MaterialDef] {
        &self.materials
    }

    /// The element with the given chemical symbol (e.g. `"Fe"`).
    pub fn element(&self, symbol: &str) -> Option<&Element> {
        self.by_symbol.get(symbol).map(|&i| &self.elements[i])
    }

    /// The element with the given atomic number.
    pub fn element_by_number(&self, number: u8) -> Option<&Element> {
        self.by_number.get(&number).map(|&i| &self.elements[i])
    }

    /// The material with the given index id.
    pub fn material(&self, id: u8) -> Option<&MaterialDef> {
        self.material_by_id.get(&id).map(|&i| &self.materials[i])
    }

    /// The material with the given display name (e.g. `"Granite"`).
    pub fn material_by_name(&self, name: &str) -> Option<&MaterialDef> {
        self.material_by_name.get(name).map(|&i| &self.materials[i])
    }

    /// Composition-weighted blend of element base traits — `Σ fractionᵢ·traitᵢ`
    /// over the elements in `comp` that exist in the table, keyed by **symbol**.
    /// `comp` is an iterator of `(element symbol, absolute amount)`; composition
    /// is absolute amounts, not fractions, so this normalizes by their sum.
    /// Unknown symbols and non-positive amounts are skipped; an empty or
    /// all-unknown composition returns [`ElementTraits::ZERO`].
    ///
    /// This is the *fallback* basis for a raw composition; a formed material's
    /// authoritative traits override it. The classifier that decides *which*
    /// material a composition forms is a separate, deferred concern.
    pub fn blend_traits<'a, I>(&self, comp: I) -> ElementTraits
    where
        I: IntoIterator<Item = (&'a str, f64)>,
    {
        blend_elements(
            comp.into_iter()
                .filter_map(|(symbol, amount)| self.element(symbol).map(|e| (e, amount))),
        )
    }

    /// As [`Self::blend_traits`], but keyed by **atomic number** — the form the
    /// ledger stores compositions in, so the simulation blends a cell's
    /// composition without a symbol round-trip. Unknown numbers are skipped.
    pub fn blend_traits_by_number<I>(&self, comp: I) -> ElementTraits
    where
        I: IntoIterator<Item = (ElementId, f64)>,
    {
        blend_elements(
            comp.into_iter()
                .filter_map(|(number, amount)| self.element_by_number(number).map(|e| (e, amount))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::PhysicalState;
    use crate::source::JsonTableSource;

    /// Load the repo's real tables from `data/materials` (relative to this
    /// crate). The loader's whole job is to read those files, so the tests
    /// exercise it against them.
    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        let source = JsonTableSource::new(dir);
        Tables::from_source(&source).expect("repo data/materials loads")
    }

    #[test]
    fn loads_the_full_vocabulary() {
        let t = tables();
        // 27 elements (Mg added for formation-sim relevance — design ceiling 30),
        // 20 resolved materials (handoff §2).
        assert_eq!(t.elements().len(), 27);
        assert_eq!(t.materials().len(), 20);
    }

    #[test]
    fn element_lookups_resolve() {
        let t = tables();
        let fe = t.element("Fe").expect("iron present");
        assert_eq!(fe.name, "Iron");
        assert_eq!(fe.number, 26);
        assert_eq!(fe.state, PhysicalState::Solid);
        assert_eq!(fe.hardness, 4.0);
        // by_number agrees with by_symbol.
        assert_eq!(t.element_by_number(26).unwrap().symbol, "Fe");
        assert_eq!(t.element_by_number(1).unwrap().name, "Hydrogen");
        // A gas state parses.
        assert_eq!(t.element("O").unwrap().state, PhysicalState::Gas);
        assert!(t.element("Xx").is_none());
    }

    #[test]
    fn material_lookups_resolve() {
        let t = tables();
        let water = t.material(60).expect("water present");
        assert_eq!(water.name, "Water");
        assert_eq!(water.viscosity, 0.05);
        assert_eq!(water.signature, vec!["H", "O"]);
        // Air (id 0) has an empty signature; that must not break parsing.
        assert!(t.material(0).unwrap().signature.is_empty());
        // Ores carry an extracted element; non-ores don't.
        assert_eq!(t.material(40).unwrap().extracted_element.as_deref(), Some("Fe"));
        assert!(t.material(10).unwrap().extracted_element.is_none());
        assert_eq!(t.material_by_name("Granite").unwrap().id, 10);
        assert!(t.material(200).is_none());
    }

    #[test]
    fn blend_of_a_single_element_is_that_element() {
        let t = tables();
        let fe = t.element("Fe").unwrap();
        let blend = t.blend_traits([("Fe", 7000.0)]);
        assert_eq!(blend.hardness, fe.hardness);
        assert_eq!(blend.brittleness, fe.brittleness);
        assert_eq!(blend.water_capacity, fe.water_capacity);
    }

    #[test]
    fn blend_is_amount_weighted_and_normalized() {
        let t = tables();
        let (fe, si) = (t.element("Fe").unwrap(), t.element("Si").unwrap());
        // Absolute amounts (handoff's `Fe 7000, Si 8000` style) → fraction blend.
        let (af, asi) = (7000.0f64, 8000.0f64);
        let blend = t.blend_traits([("Fe", af), ("Si", asi)]);
        let total = af + asi;
        let expect_h = ((af * fe.hardness as f64 + asi * si.hardness as f64) / total) as f32;
        assert!((blend.hardness - expect_h).abs() < 1e-4);
        // Lies strictly between the two endpoints.
        assert!(blend.hardness > fe.hardness.min(si.hardness));
        assert!(blend.hardness < fe.hardness.max(si.hardness));
    }

    #[test]
    fn blend_skips_unknowns_and_nonpositive() {
        let t = tables();
        let fe = t.element("Fe").unwrap();
        // Unknown symbol and a zero amount contribute nothing → pure iron.
        let blend = t.blend_traits([("Fe", 100.0), ("Zz", 999.0), ("Si", 0.0)]);
        assert_eq!(blend.hardness, fe.hardness);
        // Nothing usable → ZERO, never NaN.
        let empty = t.blend_traits(std::iter::empty());
        assert_eq!(empty, ElementTraits::ZERO);
        let all_unknown = t.blend_traits([("Zz", 1.0)]);
        assert_eq!(all_unknown, ElementTraits::ZERO);
    }

    #[test]
    fn blend_by_number_matches_by_symbol() {
        let t = tables();
        // Fe = 26, Si = 14; the two keyings must agree exactly.
        let by_symbol = t.blend_traits([("Fe", 7000.0), ("Si", 8000.0)]);
        let by_number = t.blend_traits_by_number([(26u8, 7000.0), (14u8, 8000.0)]);
        assert_eq!(by_symbol, by_number);
        // Unknown atomic numbers are skipped, like unknown symbols.
        assert_eq!(t.blend_traits_by_number([(200u8, 5.0)]), ElementTraits::ZERO);
        assert_eq!(t.blend_traits_by_number(std::iter::empty()), ElementTraits::ZERO);
    }
}
