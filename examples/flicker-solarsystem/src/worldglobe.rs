//! Rendering a settled planet as a **flicker-world-style hex-sphere globe**, composed from its
//! bulk composition via **Epoch 1 only** (the composition-distribution seed — not the full epoch
//! chain).
//!
//! Reuses the genuine world-gen libraries — `flicker_worldgrid::icosphere_with_outlines` for the
//! ISEA hex topology and `flicker_worldgen::Epoch1` to spread the planet's element abundance over
//! the cells (heavy→equator, volatile→pole + regional fBm). Each cell is a flat hex polygon
//! (fan-triangulated centre→outline, wound outward to match the engine's back-face cull, exactly
//! as `flicker-world/src/globe.rs` does) coloured by its per-cell composition and carrying the
//! cell's outward normal, so the engine **star point light** shades it. The whole thing is built
//! on the unit sphere; the renderer scales/places it per body. Build-once-cache (composition is
//! fixed once a system settles), so this never runs per frame.

use std::collections::HashMap;

use flicker::render::MeshVertex;
use flicker_materials::{ElementId, Tables};
use flicker_worldgen::{Epoch1, Epoch1Params};
use flicker_worldgrid::icosphere_with_outlines;
use flicker_worldstate::Composition;

use crate::planet::pack_rgb;

/// Build a composed hex-globe mesh for a planet of the given element `abundance` (symbol→mass-%,
/// from the body's `to_epoch1_abundance`). `freq` sets the hex resolution; `seed` makes the
/// regional composition distribution deterministic per planet. Returns `(vertices, indices)` on
/// the **unit sphere** (radius 1) — the renderer scales it to the body.
pub fn build_globe(tables: &Tables, abundance: HashMap<String, f64>, freq: u32, seed: u64) -> (Vec<MeshVertex>, Vec<u32>) {
    let (sphere, outlines) = icosphere_with_outlines(freq);
    let e1 = Epoch1::new(tables, Epoch1Params { abundance, ..Epoch1Params::default() }, seed);

    let mut verts: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (i, outline) in outlines.iter().enumerate() {
        if outline.len() < 3 {
            continue; // skip degenerate/boundary cells
        }
        let outward = sphere.dirs[i];
        let normal = outward.to_array();
        // Epoch 1: this cell's composition from its unit-sphere direction → colour.
        let material = pack_rgb(composition_color(&e1.seed_hex(outward)));
        let center = outward; // unit radius

        let base = verts.len() as u32;
        verts.push(MeshVertex { position: center.to_array(), normal, material });
        for c in outline {
            verts.push(MeshVertex { position: c.to_array(), normal, material });
        }
        let n = outline.len();
        for k in 0..n {
            let c0 = outline[k];
            let c1 = outline[(k + 1) % n];
            let i_center = base;
            let i0 = base + 1 + k as u32;
            let i1 = base + 1 + ((k + 1) % n) as u32;
            // Wind so the triangle faces outward (front = CCW, back-culled) — as globe.rs does.
            if (c0 - center).cross(c1 - center).dot(outward) >= 0.0 {
                indices.extend_from_slice(&[i_center, i0, i1]);
            } else {
                indices.extend_from_slice(&[i_center, i1, i0]);
            }
        }
    }
    (verts, indices)
}

/// A cell's colour: a mass-weighted blend of its elements' muted "primordial" tints — mirrors
/// `flicker-world/src/color.rs`'s `ViewMode::Composition` so these globes read like flicker-world
/// planets. Epoch-1 worlds are pre-differentiation, so the palette is deliberately dark/muted.
fn composition_color(comp: &Composition) -> [f32; 3] {
    let total = comp.total();
    if total <= 0.0 {
        return [0.05, 0.04, 0.04];
    }
    let mut rgb = [0.0f32; 3];
    for (el, amount) in comp.iter() {
        let f = (amount / total) as f32;
        let c = element_rgb(el);
        rgb[0] += f * c[0];
        rgb[1] += f * c[1];
        rgb[2] += f * c[2];
    }
    [rgb[0] * 0.9, rgb[1] * 0.9, rgb[2] * 0.9]
}

/// Per-element primordial tint (atomic number → RGB) — copied from `flicker-world/src/color.rs`
/// so the look matches.
fn element_rgb(el: ElementId) -> [f32; 3] {
    match el {
        1 => [0.20, 0.28, 0.34],  // H  faint blue-grey
        6 => [0.09, 0.09, 0.10],  // C  near-black (carbon)
        7 => [0.24, 0.28, 0.30],  // N  pale grey
        8 => [0.22, 0.26, 0.34],  // O  blue-grey (silicate oxygen)
        11 => [0.34, 0.31, 0.20], // Na dull yellow
        13 => [0.36, 0.37, 0.40], // Al light grey
        14 => [0.31, 0.27, 0.21], // Si tan rock (silica)
        15 => [0.30, 0.25, 0.17], // P  brown
        16 => [0.44, 0.39, 0.15], // S  sulphur yellow
        17 => [0.26, 0.34, 0.24], // Cl faint green
        19 => [0.33, 0.24, 0.31], // K  faint violet
        20 => [0.42, 0.42, 0.39], // Ca pale stone
        22 => [0.34, 0.37, 0.41], // Ti steel
        26 => [0.48, 0.16, 0.09], // Fe molten rust-red
        _ => [0.18, 0.17, 0.16],  // dark rock
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{load_tables, Composition as MatComposition, MaterialClass};

    #[test]
    fn builds_a_closed_globe_from_a_composition() {
        let tables = load_tables();
        // An Earth-ish rocky body → its Epoch-1 abundance → a composed globe.
        let mut comp = MatComposition::of(MaterialClass::Silicate, 0.7);
        comp.add(&MatComposition::of(MaterialClass::Metal, 0.3));
        let abundance = comp.to_epoch1_abundance(&tables);
        let (verts, idx) = build_globe(&tables, abundance, 4, 123);
        assert!(!verts.is_empty() && idx.len() % 3 == 0, "non-empty triangle mesh");
        let n = verts.len() as u32;
        assert!(idx.iter().all(|&i| i < n), "indices in range");
        // Every vertex carries a direct-RGB surface colour + a unit normal (point-light shaded).
        for v in &verts {
            assert_eq!(v.material & 0xFFF, 0xFFF);
            let nlen = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
            assert!((nlen - 1.0).abs() < 1e-3, "outward unit normal");
        }
    }
}
