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

/// The folder segment a material's *bundle* lives under: the slot id as a
/// zero-padded three-digit decimal (`10` → `"010"`).
///
/// WHY fixed width: the Quartermaster's review queue sorts a tier's children by
/// path (`fs_model.rs` `kids.sort()`), so a variable-width decimal would order
/// `10` before `9`. Three digits make that plain lexical sort NUMERIC for free
/// across the whole `0..=255` space, with no comparator anywhere.
#[must_use]
pub fn slot_dir(id: MaterialId) -> String {
    format!("{id:03}")
}

/// The slot a folder segment names, or `None` when the segment is not one.
///
/// STRICT on purpose: exactly three ASCII digits whose value fits `0..=255`.
/// `"10"`, `"0010"`, `"01a"`, `"256"`, `"999"`, `""` and `"Granite"` are all
/// rejected. That strictness IS the feature — it is what makes a legacy
/// NAME-keyed bundle folder (`materials/Granite/`) *detectable* as
/// not-a-slot-id rather than silently coerced into some nearby slot, so the
/// reviewer sees it instead of a wrong promotion.
#[must_use]
pub fn slot_of_dir(seg: &str) -> Option<MaterialId> {
    if seg.len() != 3 || !seg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    seg.parse::<MaterialId>().ok()
}

/// Whether a slot may hold an authored material bundle.
///
/// `false` for id 0 — that is `Air`, which is never drawn — and for the whole
/// reserved exotic-emissive block [`RESERVED_EXOTIC_FIRST`]`..=255`, which no
/// material may occupy until released by ruling (Aaron, 2026-08-19). Written
/// against the const and never a literal `248`, so the block's boundary moves
/// in exactly one place.
#[must_use]
pub const fn slot_is_writable(id: MaterialId) -> bool {
    id != 0 && id < RESERVED_EXOTIC_FIRST
}

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

/// The slot ↔ folder-segment codec: the one place a material id becomes a path
/// segment, so the bundle's writer and its reader cannot spell it differently.
#[cfg(test)]
mod slot_codec_tests {
    use super::*;

    #[test]
    fn every_slot_round_trips_through_its_folder_segment() {
        for id in 0..=MaterialId::MAX {
            let seg = slot_dir(id);
            assert_eq!(seg.len(), 3, "{id} rendered as {seg:?}, not three digits");
            assert_eq!(
                slot_of_dir(&seg),
                Some(id),
                "{seg:?} did not read back as slot {id}"
            );
        }
    }

    #[test]
    fn a_segment_that_is_not_a_slot_id_is_refused_rather_than_coerced() {
        for seg in [
            "Granite", "10", "0010", "01a", "256", "999", "", " 10", "10 ",
        ] {
            assert_eq!(slot_of_dir(seg), None, "{seg:?} was read as a slot id");
        }
    }

    #[test]
    fn air_and_the_reserved_block_are_not_writable_slots() {
        assert!(!slot_is_writable(0), "slot 0 is Air and is never drawn");
        for id in RESERVED_EXOTIC_FIRST..=MaterialId::MAX {
            assert!(
                !slot_is_writable(id),
                "slot {id} is inside the reserved exotic-emissive block"
            );
        }
        assert!(slot_is_writable(1));
        assert!(
            slot_is_writable(RESERVED_EXOTIC_FIRST - 1),
            "the last slot below the reserved block is writable"
        );
    }
}
