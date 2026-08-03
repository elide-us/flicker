//! One row of the 256-material index (`Alpha/content/data/materials.json`).
//!
//! A material is **not** an element: it is what an aggregate element
//! *composition* classifies to from a voxel's perspective (granite, sandstone,
//! dirt, water, …). The index space is `0..=255`; only a limited resolved set
//! exists today, the rest reserved to grow. The `signature` lists the defining
//! elements (roughly most→least dominant by mass) the future classifier matches
//! a composition against — that classifier is a flagged TBD and lives elsewhere.
//!
//! Trait fields here are **authoritative** for a formed material and override
//! the element-blend fallback ([`crate::Tables::blend_traits`]).

use serde::Deserialize;

/// A material's identity — its index into the 256-material space (`0..=255`).
/// A distinct alias from [`crate::ElementId`]: both are `u8`, but a material id
/// and an atomic number are different namespaces and must not be mixed.
pub type MaterialId = u8;

/// A single material definition. Unknown JSON fields are ignored so the table
/// can grow; `category` stays a free string (open-ended design vocabulary).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MaterialDef {
    /// Index into the 256-material space, `0..=255`.
    pub id: u8,
    /// Display name, e.g. `"Granite"`.
    pub name: String,
    /// Category, e.g. `"rock"` / `"soil"` / `"ore"` / `"liquid"` (free-form).
    pub category: String,
    /// Defining elements, roughly most→least dominant by mass; what the
    /// classifier matches a composition against. May be empty (e.g. `Air`).
    #[serde(default)]
    pub signature: Vec<String>,
    /// Erosion resistance, Mohs-like `0..=10` (authoritative).
    pub hardness: f32,
    /// Fracture → sediment generation, `0..=1` (authoritative).
    pub brittleness: f32,
    /// Porosity / water held, `0..=1` (authoritative).
    pub water_capacity: f32,
    /// Flow-effect motion rate per pass: `0` flows freely each pass (water) ..
    /// `1` static solid; oil/lava mid, ice creeps high. Material-only — there is
    /// no element-level viscosity, so a raw composition has none until it forms.
    pub viscosity: f32,
    /// Bulk density, g/cm³ (differentiation / weight).
    pub density_g_cm3: f32,
    /// Placeholder render colour `[r, g, b]`, each `0..=1`.
    pub color: [f32; 3],
    /// For ores, the element extracted from this material (e.g. `"Fe"`).
    #[serde(default)]
    pub extracted_element: Option<String>,
    /// Optional authoring note.
    #[serde(default)]
    pub note: Option<String>,
}
