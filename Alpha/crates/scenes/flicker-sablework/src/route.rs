//! The dispatcher — the ONE place a control changes the recipe.
//!
//! Both input channels arrive here as one `ValueMap` of results: a pointer click
//! sets a node id, and a fired `ActionSignal` sets its intent name, so a pad and
//! a mouse are indistinguishable by the time an edit happens. Nothing else in the
//! bench mutates the recipe, which is what makes "did this frame change the
//! instrument?" a single answer — and that answer is what gates the re-bake.
//!
//! [`apply`] returns **true only when the recipe actually changed**. Hovering a
//! slider, switching maps, and selecting a voice all return false: they change
//! what you are looking at, not what the instrument sounds like, so they cost no
//! bake.

use flicker::script::{Value, ValueMap};
use flicker_texture::{BlendMode, MapKind, NoiseKind, CHANNEL_COUNT};

use crate::{Sablework, MAP_IDS};

/// Read a slider's written-back value, if the walker published one this frame.
fn slid(results: &ValueMap, key: &str) -> Option<f64> {
    results.number(key)
}

/// A checkbox's written-back state. Distinct from [`ValueMap::is_on`], which
/// cannot tell "absent" from "present and false" — and for a toggle those are
/// opposite instructions: leave it alone, versus turn it off.
fn toggled(results: &ValueMap, key: &str) -> Option<bool> {
    match results.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Step an enum list forward, wrapping — what a pill button does when clicked.
fn step<T: PartialEq + Copy>(all: &[T], current: T) -> T {
    let i = all.iter().position(|v| *v == current).unwrap_or(0);
    all[(i + 1) % all.len()]
}

/// Apply this frame's results. Returns whether the RECIPE changed (and therefore
/// whether a re-bake is owed).
pub fn apply(bench: &mut Sablework, results: &ValueMap) -> bool {
    let mut edited = false;

    // ── the rack ──
    for i in 0..CHANNEL_COUNT {
        let n = i + 1;

        // Touching any of a voice's controls also SELECTS it, so the right-hand
        // fine knobs always describe the voice you are working on. Selection
        // itself is a view change and never re-bakes.
        let touched = |key: &str, results: &ValueMap| -> bool {
            results.is_on(key) || results.number(key).is_some()
        };

        if let Some(v) = toggled(results, &format!("ch{n}_on")) {
            if bench.recipe.channels[i].enabled != v {
                bench.recipe.channels[i].enabled = v;
                edited = true;
            }
            bench.sel_ch = i;
        }
        if results.is_on(&format!("ch{n}_source")) {
            let ch = &mut bench.recipe.channels[i];
            ch.source = step(&NoiseKind::ALL, ch.source);
            bench.sel_ch = i;
            edited = true;
        }
        if results.is_on(&format!("ch{n}_blend")) {
            let ch = &mut bench.recipe.channels[i];
            ch.blend = step(&BlendMode::ALL, ch.blend);
            bench.sel_ch = i;
            edited = true;
        }
        for (key, apply_to) in [
            ("scale", 0usize),
            ("octaves", 1),
            ("warp", 2),
            ("amount", 3),
        ] {
            let id = format!("ch{n}_{key}");
            if let Some(v) = slid(results, &id) {
                let ch = &mut bench.recipe.channels[i];
                let before = *ch;
                match apply_to {
                    // Scale IS the lattice period, so it must stay a whole number
                    // of cells: a fractional period cannot close a seam.
                    0 => ch.scale = v.round().clamp(1.0, 64.0) as u32,
                    1 => ch.octaves = v.round().clamp(1.0, 8.0) as u32,
                    2 => ch.warp = v.clamp(0.0, 2.0),
                    _ => ch.amount = v.clamp(0.0, 1.0),
                }
                if *ch != before {
                    edited = true;
                }
                bench.sel_ch = i;
            }
        }
        if touched(&format!("ch{n}_name"), results) {
            bench.sel_ch = i;
        }
    }

    // ── the output stage ──
    let out = &mut bench.recipe.out;
    for (key, slot) in [
        ("relief", 0usize),
        ("roughness", 1),
        ("roughness_mod", 2),
        ("metalness", 3),
        ("metalness_mod", 4),
        ("ao", 5),
    ] {
        if let Some(v) = slid(results, key) {
            let v = v as f32;
            let target = match slot {
                0 => &mut out.relief,
                1 => &mut out.roughness,
                2 => &mut out.roughness_mod,
                3 => &mut out.metalness,
                4 => &mut out.metalness_mod,
                _ => &mut out.ao,
            };
            // Signed knobs run -1..1; the rest 0..1. The Lua's `min` already says
            // so, but the recipe is the thing that must stay valid whatever a
            // caller writes into it.
            let lo = if slot == 2 || slot == 4 { -1.0 } else { 0.0 };
            let v = v.clamp(lo, 1.0);
            if (*target - v).abs() > f32::EPSILON {
                *target = v;
                edited = true;
            }
        }
    }

    // ── the selected voice's fine knobs ──
    let sel = bench.sel_ch.min(CHANNEL_COUNT - 1);
    let ch = &mut bench.recipe.channels[sel];
    let before = *ch;
    if let Some(v) = slid(results, "sel_lacunarity") {
        ch.lacunarity = v.clamp(1.0, 4.0);
    }
    if let Some(v) = slid(results, "sel_gain") {
        ch.gain = v.clamp(0.0, 1.0);
    }
    if let Some(v) = slid(results, "sel_contrast") {
        ch.contrast = v.clamp(0.0, 4.0);
    }
    if let Some(v) = toggled(results, "sel_invert") {
        ch.invert = v;
    }
    if *ch != before {
        edited = true;
    }

    // ── view-only: which map the swatch shows ──
    // Clicking a tab picks it outright; the bumpers step. Both arrive here as
    // result NAMES, so the pad and the pointer are the same event.
    for (i, id) in MAP_IDS.iter().enumerate() {
        if results.is_on(id) {
            bench.sel_map = i.min(MapKind::ALL.len() - 1);
        }
    }
    let maps = MAP_IDS.len();
    if results.is_on("map_next") {
        bench.sel_map = (bench.sel_map + 1) % maps;
    }
    if results.is_on("map_prev") {
        bench.sel_map = (bench.sel_map + maps - 1) % maps;
    }

    // ── the title bar ──
    // The material binding and the bake rung are RECIPE and OUTPUT settings, not
    // view state, but neither changes the IMAGE — a re-bake would show the same
    // pixels — so neither asks for one.
    if results.is_on("material") {
        bench.step_material();
    }
    if results.is_on("bake_size") {
        bench.step_size();
    }
    if results.is_on("commit") {
        bench.start_commit();
    }
    if results.is_on("reseed") {
        // A new seed re-rolls every field at once while keeping the whole rack —
        // the "same instrument, different performance" control.
        bench.recipe.seed = bench.recipe.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        edited = true;
    }
    let patches = presets_len();
    if results.is_on("patch_next") {
        bench.patch = (bench.patch + 1) % patches;
        bench.recipe = presets_at(bench.patch);
        edited = true;
    }
    if results.is_on("patch_prev") {
        bench.patch = (bench.patch + patches - 1) % patches;
        bench.recipe = presets_at(bench.patch);
        edited = true;
    }

    edited
}

fn presets_len() -> usize {
    flicker_texture::presets::all().len()
}

fn presets_at(i: usize) -> flicker_texture::TextureRecipe {
    let mut all = flicker_texture::presets::all();
    all.swap_remove(i.min(all.len() - 1))
}
