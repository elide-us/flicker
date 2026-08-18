//! [`Tables`] — the loaded vocabulary, indexed for lookup, plus the
//! composition-weighted [`Tables::blend_traits`].

use std::collections::HashMap;

use crate::compound::CompoundDef;
use crate::element::{Element, ElementId};
use crate::material::MaterialDef;
use crate::rock::RockDef;
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
    compounds: Vec<CompoundDef>,
    rocks: Vec<RockDef>,
    by_symbol: HashMap<String, usize>,
    by_number: HashMap<u8, usize>,
    material_by_id: HashMap<u8, usize>,
    material_by_name: HashMap<String, usize>,
    compound_by_name: HashMap<String, usize>,
    compound_by_id: HashMap<u16, usize>,
    rock_by_id: HashMap<String, usize>,
}

impl Tables {
    /// Load and index the vocabulary from a [`TableSource`]. This is the single
    /// construction path — `source` decides whether the rows came from JSON, a
    /// web service, or a DB.
    pub fn from_source(source: &impl TableSource) -> Result<Self, MaterialError> {
        let elements = source.load_elements()?;
        let materials = source.load_materials()?;
        let compounds = source.load_compounds()?;
        let rocks = source.load_rocks()?;
        Ok(Self::build_all(elements, materials, compounds, rocks))
    }

    /// Index already-loaded element + material rows (no compounds). Exposed for
    /// callers that obtained the rows by other means (and for tests); most code
    /// uses [`Self::from_source`].
    pub fn from_rows(elements: Vec<Element>, materials: Vec<MaterialDef>) -> Self {
        Self::build(elements, materials, Vec::new())
    }

    /// Index already-loaded element + material + compound rows.
    pub fn from_rows_full(
        elements: Vec<Element>,
        materials: Vec<MaterialDef>,
        compounds: Vec<CompoundDef>,
    ) -> Self {
        Self::build(elements, materials, compounds)
    }

    fn build(
        elements: Vec<Element>,
        materials: Vec<MaterialDef>,
        compounds: Vec<CompoundDef>,
    ) -> Self {
        Self::build_all(elements, materials, compounds, Vec::new())
    }

    fn build_all(
        elements: Vec<Element>,
        materials: Vec<MaterialDef>,
        compounds: Vec<CompoundDef>,
        rocks: Vec<RockDef>,
    ) -> Self {
        let rock_by_id = rocks
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.clone(), i))
            .collect();
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
        let compound_by_name = compounds
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect();
        let compound_by_id = compounds
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id, i))
            .collect();
        Self {
            elements,
            materials,
            compounds,
            by_symbol,
            by_number,
            material_by_id,
            material_by_name,
            compound_by_name,
            compound_by_id,
            rocks,
            rock_by_id,
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

    /// All compound rows, in load order (empty if the catalog isn't present).
    /// Every rock in the catalog.
    pub fn rocks(&self) -> &[RockDef] {
        &self.rocks
    }

    /// One rock by its slug.
    pub fn rock(&self, id: &str) -> Option<&RockDef> {
        self.rock_by_id.get(id).map(|&i| &self.rocks[i])
    }

    /// **How well a mineral assemblage resists being worn away and carried off**,
    /// `0` (strips instantly) .. `1` (stands as a ridge forever) — the
    /// modal-weighted average of the resistances of the rocks those minerals make.
    ///
    /// This is the read the outcrop model runs on, and the reason differential
    /// erosion is possible at all: a landscape of one uniform material erodes
    /// evenly and, once normalised, has not changed shape. The contrast between a
    /// resistant intrusion and the soft ground around it is what leaves a ridge
    /// standing after the plain has worn down.
    ///
    /// A rock is matched by how much of its own modal recipe the assemblage
    /// supplies, so a mixture is scored by the rocks it most resembles rather than
    /// being forced into one bin. An assemblage matching nothing in the catalog
    /// gets `default_resistance` — loud enough to notice in a view, and not
    /// silently zero (which would make unknown rock erode like salt).
    pub fn erosional_resistance(
        &self,
        minerals: impl Iterator<Item = (String, f64)>,
        default_resistance: f32,
    ) -> f32 {
        let assemblage: HashMap<String, f64> = minerals.collect();
        let total: f64 = assemblage.values().sum();
        if total <= 0.0 || self.rocks.is_empty() {
            return default_resistance;
        }
        let (mut weighted, mut weight) = (0.0f64, 0.0f64);
        for rock in &self.rocks {
            // How much of this rock's recipe the assemblage actually supplies.
            let mut match_frac = 0.0f64;
            for (mineral, &modal) in &rock.modal {
                let have = assemblage.get(mineral).copied().unwrap_or(0.0) / total;
                match_frac += have.min(modal as f64);
            }
            if match_frac > 0.0 {
                weighted += match_frac * rock.erosional_resistance as f64;
                weight += match_frac;
            }
        }
        if weight <= 0.0 {
            default_resistance
        } else {
            (weighted / weight) as f32
        }
    }

    pub fn compounds(&self) -> &[CompoundDef] {
        &self.compounds
    }

    /// The curated **mineable ores/gems** (`harvestable = true`) — the materials
    /// that must reach a concentrated vein somewhere (drives `ensure_ore_veins`).
    /// Everything else is refined diffusely from bulk voxels, no vein needed.
    pub fn harvestable_compounds(&self) -> impl Iterator<Item = &CompoundDef> {
        self.compounds.iter().filter(|c| c.harvestable)
    }

    /// The compound with the given name (e.g. `"Hematite"`).
    pub fn compound(&self, name: &str) -> Option<&CompoundDef> {
        self.compound_by_name.get(name).map(|&i| &self.compounds[i])
    }

    /// The compound with the given catalog id.
    pub fn compound_by_id(&self, id: u16) -> Option<&CompoundDef> {
        self.compound_by_id.get(&id).map(|&i| &self.compounds[i])
    }

    /// Per-element **mass fractions** of a compound (Σ = 1), from its formula
    /// element counts × their atomic masses — the stoichiometry the compound former
    /// uses to move element mass into compound mass. Keyed by atomic number.
    /// Elements not in the periodic table are skipped (so the remaining fractions
    /// renormalise over the known elements); an empty/all-unknown compound yields
    /// an empty vec.
    /// Formula-unit mass of a compound, u — the sum of its constituents' atomic
    /// masses (elements the periodic table does not know contribute nothing,
    /// mirroring [`compound_mass_fractions`]). Zero for an unknown formula.
    pub fn compound_molar_mass(&self, compound: &CompoundDef) -> f64 {
        compound
            .elements
            .iter()
            .filter_map(|e| {
                self.element(&e.symbol)
                    .map(|el| e.count as f64 * el.atomic_mass as f64)
            })
            .sum()
    }

    pub fn compound_mass_fractions(&self, compound: &CompoundDef) -> Vec<(ElementId, f64)> {
        let mut weights: Vec<(ElementId, f64)> = Vec::with_capacity(compound.elements.len());
        let mut total = 0.0;
        for e in &compound.elements {
            if let Some(el) = self.element(&e.symbol) {
                let w = e.count as f64 * el.atomic_mass as f64;
                if w > 0.0 {
                    weights.push((el.number, w));
                    total += w;
                }
            }
        }
        if total <= 0.0 {
            return Vec::new();
        }
        for (_, w) in &mut weights {
            *w /= total;
        }
        weights
    }

    /// The ore compounds whose extracted element is `symbol` — the minerals a
    /// player mines to obtain that element (e.g. `"Fe"` → Hematite).
    pub fn ores_of<'a>(&'a self, symbol: &'a str) -> impl Iterator<Item = &'a CompoundDef> + 'a {
        self.compounds
            .iter()
            .filter(move |c| c.extracted() == Some(symbol))
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
    use std::collections::HashSet;

    use super::*;
    use crate::element::PhysicalState;
    use crate::source::JsonTableSource;

    /// Load the repo's real tables from `Alpha/content/data` (relative to this
    /// crate). The loader's whole job is to read those files, so the tests
    /// exercise it against them.
    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/data");
        let source = JsonTableSource::new(dir);
        Tables::from_source(&source).expect("repo Alpha/content/data loads")
    }

    #[test]
    fn loads_the_full_vocabulary() {
        let t = tables();
        // 28 elements (Mg added for formation-sim relevance; Li synced 2026-07-11
        // per Book III ruling F3450870 — design ceiling 30), 20 resolved materials
        // (handoff §2).
        assert_eq!(t.elements().len(), 28);
        assert_eq!(t.materials().len(), 20);
        // 28 UNIQUE symbols and atomic numbers — a duplicate row would hide
        // inside the bare len() check (the by-symbol/by-number indexes silently
        // collapse duplicates).
        let symbols: HashSet<&str> = t.elements().iter().map(|e| e.symbol.as_str()).collect();
        let numbers: HashSet<u8> = t.elements().iter().map(|e| e.number).collect();
        assert_eq!(symbols.len(), 28);
        assert_eq!(numbers.len(), 28);
        // The two post-transcription additions are present — guards against a
        // stale-base regression re-dropping them (e.g. a branch forked before
        // the 2026-07-11 Li sync still carries a 27-element table).
        assert!(symbols.contains("Mg"), "Mg (added 2026-06) missing");
        assert!(symbols.contains("Li"), "Li (Book ruling F3450870) missing");
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
        assert_eq!(
            t.material(40).unwrap().extracted_element.as_deref(),
            Some("Fe")
        );
        assert!(t.material(10).unwrap().extracted_element.is_none());
        assert_eq!(t.material_by_name("Granite").unwrap().id, 10);
        assert!(t.material(200).is_none());
    }

    #[test]
    fn compounds_load_from_the_catalog() {
        let t = tables();
        // The Prism BookIII catalog (Common/Alloy/Biological/Mineral/Useful/Gemstone;
        // 77 rows after Feldspar id 42 retired 2026-07-13, superseded by the split
        // feldspars — retired ids are never reused) + the 12 sim-required minerals
        // merged in from the rock tier (Unification Ruling R6b — ONE mineral registry) = 89,
        // + the 5 sim-required atmospheric gas species (ids 91-95: N₂, SO₂, HCl, CH₄, NH₃)
        // added for the outgassing/atmosphere sim (2026-07-14) = 94,
        // + Oxygen (id 96) added for the biosphere (2026-08-05): free O₂ is a
        // BIOSIGNATURE — photosynthesis is the only thing in the sim that makes
        // it, and there was nothing in the catalog to credit = 95.
        assert_eq!(t.compounds().len(), 95);
        assert!(t.compound("Feldspar").is_none(), "Feldspar (42) is retired");
        // The organic vocabulary the biosphere books its tissue in. These three
        // shipped a placeholder C₁H₁O₁ until 2026-08-05, which made them
        // chemically IDENTICAL to the conserved ledger — a bed of wood and a bed
        // of sugar weighed the same and carried the same elements.
        for (name, formula) in [
            ("Cellulose", "C6H10O5"),
            ("Lignin", "C9H10O2"),
            ("Oils", "C57H104O6"),
        ] {
            let c = t.compound(name).unwrap_or_else(|| panic!("{name} present"));
            assert_eq!(c.formula, formula, "{name} carries real stoichiometry");
        }
        assert!(t.compound_by_id(42).is_none(), "id 42 must stay retired");
        // A known mineral resolves with its formula, elements, and extraction target.
        let hem = t.compound("Hematite").expect("Hematite present");
        assert_eq!(hem.formula, "Fe2O3");
        assert_eq!(hem.category, "mineral");
        assert!(hem.natural);
        assert_eq!(hem.extracted(), Some("Fe"));
        assert!(hem.contains("Fe") && hem.contains("O"));
        // Iron ore is discoverable via the extraction index.
        assert!(t.ores_of("Fe").any(|c| c.name == "Hematite"));
        // An alloy is crafted (not natural) and has no single extraction target.
        let brass = t.compound("Brass").expect("Brass present");
        assert!(!brass.natural);
        assert_eq!(brass.extracted(), None);
        // Every parsed constituent element exists in the periodic table (non-Prism
        // symbols like Apatite's F were dropped from the parsed element list).
        for c in t.compounds() {
            for e in &c.elements {
                assert!(
                    t.element(&e.symbol).is_some(),
                    "{}: unknown element {}",
                    c.name,
                    e.symbol
                );
            }
        }
    }

    #[test]
    fn merged_minerals_are_first_class_compounds() {
        let t = tables();
        // The 12 rock-forming minerals moved out of rocks.json (R6b) continue the
        // catalog id space and carry their physical fields.
        let oli = t.compound("Olivine").expect("Olivine present");
        assert_eq!(oli.id, 79);
        assert_eq!(oli.category, "mineral");
        assert!(oli.natural);
        assert_eq!(oli.hardness_mohs, Some(7.0));
        assert_eq!(oli.density_g_cm3, Some(3.27));
        assert_eq!(oli.brittleness, Some(0.7));
        assert!(oli.note.as_deref().unwrap_or("").contains("MANTLE"));
        assert!(oli.contains("Mg") && oli.contains("Si") && oli.contains("O"));
        assert_eq!(t.compound_by_id(90).unwrap().name, "Serpentine");
        // Book III rows carry physicals too (one-table directive) but stay
        // non-sim_required with no provenance note.
        let hem = t.compound("Hematite").unwrap();
        assert_eq!(hem.hardness_mohs, Some(6.0));
        assert!(hem.note.is_none() && !hem.sim_required);
        // The merge added NO harvestable flags — the curated ore/gem set is
        // untouched (spodumene/dolomite ore status awaits its own ruling).
        let minerals = [
            "Olivine",
            "Pyroxene",
            "Anorthite",
            "Albite",
            "Orthoclase",
            "Biotite",
            "Muscovite",
            "Magnetite",
            "Pyrite",
            "Dolomite",
            "Spodumene",
            "Serpentine",
        ];
        for name in minerals {
            let c = t.compound(name).unwrap_or_else(|| panic!("{name} missing"));
            assert!(
                (79..=90).contains(&c.id),
                "{name}: id {} outside 79..=90",
                c.id
            );
            assert!(c.sim_required, "{name} must be flagged sim_required");
            assert!(!c.harvestable, "{name} must stay unharvestable");
        }
    }

    /// ONE TABLE (Aaron, 2026-07-13): the physical fields are populated on every
    /// row of the registry — a new compound cannot ship without them, and `0.0`
    /// hardness/brittleness (non-solid convention) is the only sanctioned "n/a".
    #[test]
    fn physical_fields_cover_the_whole_registry() {
        let t = tables();
        for c in t.compounds() {
            let (h, d, b) = (c.hardness_mohs, c.density_g_cm3, c.brittleness);
            let h = h.unwrap_or_else(|| panic!("{}: hardness_mohs missing", c.name));
            let d = d.unwrap_or_else(|| panic!("{}: density_g_cm3 missing", c.name));
            let b = b.unwrap_or_else(|| panic!("{}: brittleness missing", c.name));
            assert!(
                (0.0..=10.0).contains(&h),
                "{}: Mohs {h} out of range",
                c.name
            );
            assert!(d > 0.0, "{}: density {d} not positive", c.name);
            assert!(
                (0.0..=1.0).contains(&b),
                "{}: brittleness {b} out of range",
                c.name
            );
        }
    }

    /// `rocks.json` (the modal rock recipes) keys each rock's `modal` map by exact
    /// compound *name* — nothing consumes it yet (poc-chemistry M6), so until a
    /// rocks loader exists this guards the reference scheme against typos and
    /// renames on either side.
    #[test]
    fn rock_modal_references_resolve_via_the_compound_catalog() {
        let t = tables();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/rocks.json"
        );
        let bytes = std::fs::read(path).expect("rocks.json readable");
        let file: serde_json::Value = serde_json::from_slice(&bytes).expect("rocks.json parses");
        // R6b: ONE mineral registry — rocks.json defines no minerals of its own.
        assert!(
            file.get("minerals").is_none(),
            "rocks.json must not define minerals (R6b)"
        );
        let rocks = file["rocks"].as_array().expect("rocks array");
        assert!(!rocks.is_empty());
        for rock in rocks {
            let id = rock["id"].as_str().unwrap_or("?");
            let modal = rock["modal"].as_object().expect("modal map");
            assert!(!modal.is_empty(), "rock {id}: empty modal");
            for name in modal.keys() {
                assert!(
                    t.compound(name).is_some(),
                    "rock {id}: modal key {name:?} is not a compound name"
                );
            }
        }
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
        assert_eq!(
            t.blend_traits_by_number([(200u8, 5.0)]),
            ElementTraits::ZERO
        );
        assert_eq!(
            t.blend_traits_by_number(std::iter::empty()),
            ElementTraits::ZERO
        );
    }
}
