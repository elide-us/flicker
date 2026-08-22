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

/// First id of the reserved exotic-emissive insurance block (`248..=255`) —
/// ghost/exotic effects are shader-driven first; these 8 slots exist in case
/// shader-only proves insufficient (Aaron, ruled 2026-08-19). No material may
/// be defined in the block until released by ruling; the loader gates it.
pub const RESERVED_EXOTIC_FIRST: MaterialId = 248;

/// How a material's surface RENDERS — the closed 4-way axis (Aaron, ratified
/// 2026-08-19, amending the earlier 3-way). Exactly one class per material;
/// orthogonal to the free-form geological `category`. An unknown value in the
/// data fails deserialization loud — there is no fallback class.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderClass {
    /// Top-surface terrain; participates in voxel primary/secondary/blend
    /// transitions (biomes fade into each other).
    Blendable,
    /// Rocks/ores/minerals; renders as itself, NEVER blended — the visual
    /// signature is player-facing information (bauxite must look like bauxite).
    HardEdge,
    /// Gems, ice, water, oil; the alpha/refraction render path.
    Translucent,
    /// Glowing; colours restricted to the curated `palettes.json` emissive set.
    Emissive,
}

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
    /// The closed render axis — required on every material except the id-0
    /// Air placeholder (never drawn). `None` anywhere else is a content error,
    /// gated loud at [`crate::Tables::from_source`].
    #[serde(default)]
    pub render_class: Option<RenderClass>,
    /// Compound NAMES (exact, from the merged compound catalog — the same
    /// scheme as `rocks.json` modal keys) whose dominance classifies a
    /// container to this material — the classifier's PRIMARY key. Resolved and
    /// uniqueness-gated at [`crate::Tables::from_source`]; empty rows are
    /// reachable only through the `signature` fallback.
    #[serde(default)]
    pub represents: Vec<String>,
    /// Defining elements, roughly most→least dominant by mass; what the
    /// classifier matches a composition against when no represented compound
    /// dominates — the FALLBACK key. May be empty (e.g. `Air`).
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
