//! Derived layer/hex **classification** — the "(composition + conditions) → material" read
//! (unification ruling R7, first cut). A layer is a **volume of material**; *what it is* —
//! solid rock, molten rock, liquid water, ice, a gas envelope — is DERIVED here from its
//! composition + temperature + pressure, **never an assigned kind**. The viewer draws this
//! read; the sim stores only the conserved material + heat. Effects, not results: the sim
//! applies heat and phase change; this merely *observes* the outcome. A molten hex, a water
//! hex and a gas hex are the same thing (a filled volume) in different phases — this names it.
//!
//! Reuses the material vocabulary: element densities, `materials.json` signatures + colours,
//! and compound gas densities. Temperature is real **Kelvin** now, so the phase thresholds are
//! the physical melting / boiling points (rock solidus, iron melt, water 273/373 K).

use flicker_materials::{MaterialDef, Tables};
use flicker_worldstate::{Composition, CompoundLedger};

use crate::cooling::{T_CONDENSE, T_SOLIDUS};

/// The physical phase a volume of material is in — the same underlying stuff, different state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Solid,
    Molten,
    Liquid,
    Gas,
}

/// A derived classification of a layer/hex: what it IS right now, for display + volume.
#[derive(Clone, Debug)]
pub struct LayerClass {
    pub phase: Phase,
    /// Human label — "basalt", "molten rock", "water", "ice", "CO₂ air", …
    pub name: String,
    /// Display colour (from the matched material / phase).
    pub color: [f32; 3],
    /// Phase-appropriate bulk density (g/cm³) — the volume/thickness driver (gas ≪ liquid < solid).
    pub density_g_cm3: f32,
}

// Phase thresholds in real KELVIN. Rock melt reuses the shared crust solidus [`T_SOLIDUS`]
// and water boil the shared [`T_CONDENSE`] (one source with the cooling thresholds — no drift).
/// Iron's melting point (K, 1538 °C) — a metal body is molten above it.
const T_METAL_MELT: f32 = 1811.0;
/// Water freezes below this (K, 0 °C).
const T_WATER_FREEZE: f32 = 273.0;
/// A dominant element this dense (g/cm³) marks a metal body (the differentiated core).
const METAL_DENSITY: f32 = 6.0;
/// Below this pressure (top of the column) volatiles are gas regardless of temperature — the
/// low-pressure air, where even water is vapour.
const LOW_PRESSURE: f32 = 0.15;
/// The Water compound id (`compounds.json`).
const WATER: u16 = 1;

/// The known atmospheric gas compounds → (label, colour). Ids are stable `compounds.json`.
fn gas_look(id: u16) -> Option<(&'static str, [f32; 3])> {
    Some(match id {
        2 => ("CO₂ air", [0.70, 0.62, 0.45]),
        91 => ("N₂ air", [0.60, 0.72, 0.86]),
        92 => ("SO₂ air", [0.88, 0.82, 0.40]),
        93 => ("HCl air", [0.74, 0.85, 0.55]),
        94 => ("CH₄ air", [0.55, 0.60, 0.74]),
        95 => ("NH₃ air", [0.72, 0.68, 0.82]),
        _ => return None,
    })
}

/// Mass-weighted mean element density of a composition (g/cm³) — the solid-packing density.
fn solid_density(comp: &Composition, tables: &Tables) -> f32 {
    let (mut m, mut w) = (0.0f64, 0.0f64);
    for (el, amount) in comp.iter() {
        if let Some(e) = tables.element_by_number(el) {
            m += amount * e.density_g_cm3 as f64;
            w += amount;
        }
    }
    if w > 0.0 {
        (m / w) as f32
    } else {
        0.0
    }
}

/// Best-matching `materials.json` entry for a composition — the rock/soil/ore identity — by
/// signature overlap with the elements actually present. Falls back to a generic rock.
fn match_material(comp: &Composition, tables: &Tables) -> (String, [f32; 3], f32) {
    let present: Vec<&str> = comp
        .iter()
        .filter(|(_, a)| *a > 0.0)
        .filter_map(|(el, _)| tables.element_by_number(el).map(|e| e.symbol.as_str()))
        .collect();
    let mut best: Option<(&MaterialDef, usize)> = None;
    for m in tables.materials() {
        if m.signature.is_empty() {
            continue;
        }
        let score = m
            .signature
            .iter()
            .filter(|s| present.contains(&s.as_str()))
            .count();
        // Require the whole signature to be present (a real match), prefer the most specific.
        if score == m.signature.len()
            && best
                .map(|(b, _)| m.signature.len() > b.signature.len())
                .unwrap_or(true)
        {
            best = Some((m, score));
        }
    }
    match best {
        Some((m, _)) => (m.name.clone(), m.color, m.density_g_cm3),
        None => (
            "rock".into(),
            [0.50, 0.50, 0.52],
            solid_density(comp, tables).max(2.5),
        ),
    }
}

/// Classify a volume of material (its `comp` + formed `compounds`) at temperature `temp` (K)
/// and normalized `pressure` (1 = deep interior, 0 = space) into what it currently IS.
/// The one derived-identity read the viewer draws — pure, encodes no causal rule.
pub fn classify(
    comp: &Composition,
    compounds: &CompoundLedger,
    temp: f32,
    pressure: f32,
    tables: &Tables,
) -> LayerClass {
    if comp.total() <= 0.0 {
        return LayerClass {
            phase: Phase::Gas,
            name: "void".into(),
            color: [0.10, 0.10, 0.12],
            density_g_cm3: 0.0,
        };
    }

    // A gas body: dominated by a non-water atmospheric gas compound.
    if let Some(gid) = compounds.dominant() {
        if gid != WATER {
            if let Some((name, color)) = gas_look(gid) {
                let density = tables
                    .compound_by_id(gid)
                    .and_then(|c| c.density_g_cm3)
                    .unwrap_or(0.0015)
                    .max(1e-4);
                return LayerClass {
                    phase: Phase::Gas,
                    name: name.into(),
                    color,
                    density_g_cm3: density,
                };
            }
        }
    }

    // A water body (the Water compound is what fills it). Phase by temperature + pressure:
    // low-pressure air holds it as vapour, otherwise it condenses / freezes.
    if compounds.dominant() == Some(WATER) {
        return if pressure < LOW_PRESSURE || temp > T_CONDENSE {
            LayerClass {
                phase: Phase::Gas,
                name: "steam".into(),
                color: [0.85, 0.88, 0.92],
                density_g_cm3: 0.0006,
            }
        } else if temp > T_WATER_FREEZE {
            LayerClass {
                phase: Phase::Liquid,
                name: "water".into(),
                color: [0.15, 0.40, 0.78],
                density_g_cm3: 1.0,
            }
        } else {
            LayerClass {
                phase: Phase::Solid,
                name: "ice".into(),
                color: [0.80, 0.90, 0.96],
                density_g_cm3: 0.92,
            }
        };
    }

    // A metal body (dense siderophiles — the differentiated core): molten above iron's melt.
    let dom_density = comp
        .dominant()
        .and_then(|el| tables.element_by_number(el))
        .map(|e| e.density_g_cm3)
        .unwrap_or(0.0);
    if dom_density >= METAL_DENSITY {
        return if temp > T_METAL_MELT {
            LayerClass {
                phase: Phase::Molten,
                name: "molten metal".into(),
                color: [0.95, 0.55, 0.25],
                density_g_cm3: dom_density * 0.92,
            }
        } else {
            LayerClass {
                phase: Phase::Solid,
                name: "metal".into(),
                color: [0.55, 0.55, 0.60],
                density_g_cm3: dom_density,
            }
        };
    }

    // Otherwise a rock/silicate body: the matched material, molten above the solidus.
    let (name, color, density) = match_material(comp, tables);
    if temp > T_SOLIDUS {
        // Glow toward hot orange; molten rock is a little less dense than the solid.
        let hot = [
            color[0] * 0.4 + 0.51,
            color[1] * 0.4 + 0.18,
            color[2] * 0.4 + 0.07,
        ];
        LayerClass {
            phase: Phase::Molten,
            name: format!("molten {name}"),
            color: hot,
            density_g_cm3: density * 0.9,
        }
    } else {
        LayerClass {
            phase: Phase::Solid,
            name,
            color,
            density_g_cm3: density,
        }
    }
}
