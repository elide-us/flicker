//! One row of the **compound catalog** (`Alpha/content/data/compounds.json` plus its
//! sibling `crust_compounds.json`, merged at load) — the
//! chemistry-facing vocabulary transcribed from Prism *BookIII* ("Common /
//! Alloy / Biological / Mineral / Useful / Gemstone Compounds"). A compound is a
//! named combination of elements (`SiO₂`, `CaCO₃`, `Fe₂O₃`, an alloy like
//! `CuSnZn`) with a category and, for the ores, the element it yields.
//!
//! The catalog is also the **one mineral registry** (Unification Ruling R6b,
//! 2026-07-13 — minerals ARE compounds): the 12 sim-required rock-forming
//! minerals that used to live in `rocks.json` are first-class rows here
//! (ids 79–90). `rocks.json` keeps only the modal rock recipes, keyed by
//! compound *name*. It is **ONE TABLE** (Aaron, 2026-07-13): the physical
//! fields ([`hardness_mohs`][`CompoundDef::hardness_mohs`], density,
//! brittleness) are populated on *every* row — no split populations; `0.0`
//! hardness/brittleness marks a non-solid, the element-table gas convention.
//!
//! This is the source-work behind the composition→material classification and the
//! crafting/extraction layer. The classifier that maps a cell's composition to a
//! formed compound is still deferred (BookIII §"Elements, Compounds, and
//! Classified Materials"); this crate supplies the vocabulary it will read.

use serde::{Deserialize, Serialize};

/// One element in a compound's formula: its symbol and subscript count (alloy
/// components with no subscript are count `1`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompoundElement {
    pub symbol: String,
    pub count: u32,
}

/// **A phase's stability limit** — bury it under at least this much pressure at
/// at least this temperature and it reorganises into something else.
///
/// The pair is read against a bed's `peak_pt`, the maximum it has ever
/// endured, because metamorphic grade is a high-water mark: rock that has been
/// deep does not un-become what the depth made it when it is exhumed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetamorphicRule {
    /// The phase this one becomes, by catalog name.
    pub to: String,
    /// Peak overburden pressure it must have carried, Pa.
    pub pressure_pa: f64,
    /// Peak temperature it must have reached, K.
    pub temp_k: f64,
}

/// A compound from the Prism catalog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompoundDef {
    /// Stable catalog id.
    pub id: u16,
    /// Display name, e.g. `"Hematite"`.
    pub name: String,
    /// Chemical formula as written (ASCII), e.g. `"Fe2O3"`, `"CaCO3"`, `"CuSnZn"`.
    #[serde(default)]
    pub formula: String,
    /// Which catalog section it came from: `common`, `alloy`, `biological`,
    /// `mineral`, `useful`, or `gemstone`.
    #[serde(default)]
    pub category: String,
    /// Constituent elements parsed from the formula.
    #[serde(default)]
    pub elements: Vec<CompoundElement>,
    /// For ores/minerals: the element the book says is extracted from it. `None`
    /// for compounds with no single extraction target.
    #[serde(default)]
    pub extracted_element: Option<String>,
    /// Whether this compound forms **naturally** in the world (minerals, common
    /// compounds, biologics) vs is **crafted** (alloys/steels). The world-gen
    /// classifier only forms the natural ones; the crafted ones are recipes.
    #[serde(default)]
    pub natural: bool,
    /// Whether this is a curated **mineable ore/gem** — a gameplay material that
    /// must reach a concentrated *vein* somewhere in the world (the
    /// `ensure_ore_veins` guarantee keys on this). Distinct from `natural`: most
    /// naturals (silica, water, CO₂) are refined diffusely from bulk voxels and
    /// need no vein; only `harvestable` ones (Hematite, Native Gold, Diamond…) are
    /// the "send-minions-to-mine-it" ore bodies. Not every element gets a node.
    #[serde(default)]
    pub harvestable: bool,
    /// One-line in-game uses summary.
    #[serde(default)]
    pub uses: String,
    /// Mohs scratch hardness — populated on **every** row (one-table directive,
    /// 2026-07-13); `0.0` = not applicable (non-solid), the element-table gas
    /// convention. `Option` only so a draft row still parses; completeness is
    /// test-enforced against the repo data.
    #[serde(default)]
    pub hardness_mohs: Option<f32>,
    /// Density in g/cm³, populated on every row (gases at STP, like the
    /// element table).
    #[serde(default)]
    pub density_g_cm3: Option<f32>,
    /// Brittleness `0..1`, populated on every row — the composition-side
    /// failure-mode input the erosion design reads (quench/cooling-rate is
    /// deliberately NOT here: per R4b it is per-layer formation provenance).
    #[serde(default)]
    pub brittleness: Option<f32>,
    /// Whether this row is a **sim-required** addition beyond the Book III
    /// crafting tables (the R6b mineral merge) rather than a transcribed book row.
    ///
    /// **This is PROVENANCE — which book a row came from — and says nothing
    /// about what the world can make.** It was briefly used as the
    /// crystallisation filter, which is why ore was a bare number and never a
    /// mineral, and why quartz could not form (2026-08-06). Use
    /// [`crystallizes`](Self::crystallizes) for that question.
    #[serde(default)]
    pub sim_required: bool,
    /// **Whether this phase forms by crystallisation** from a melt or from
    /// solution circulating in rock — the candidate set the `Crystallization`
    /// stage draws from.
    ///
    /// False for minerals with a different route: evaporites need standing
    /// water to dry out, coal is made from buried tissue by maturation, and
    /// bauxite is a weathering residue. Absent (false) on every non-mineral
    /// row — a gas or an alloy does not crystallise out of rock, and gemstones
    /// wait on the metamorphic chemistry that would form them.
    #[serde(default)]
    pub crystallizes: bool,
    /// **What this phase becomes when it is buried hard enough and hot enough**
    /// — its stability limit, read by the `Metamorphism` stage. Absent on
    /// phases with no prograde reaction in the catalog.
    #[serde(default)]
    pub metamorphic: Option<MetamorphicRule>,
    /// Free-form provenance/geology note (sim-required rows).
    #[serde(default)]
    pub note: Option<String>,
}

impl CompoundDef {
    /// The extracted element symbol, if this is an ore.
    pub fn extracted(&self) -> Option<&str> {
        self.extracted_element.as_deref().filter(|s| !s.is_empty())
    }

    /// Whether `symbol` is one of this compound's constituent elements.
    pub fn contains(&self, symbol: &str) -> bool {
        self.elements.iter().any(|e| e.symbol == symbol)
    }
}
