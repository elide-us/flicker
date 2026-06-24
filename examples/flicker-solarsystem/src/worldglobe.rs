//! Rendering and **evolving** a body as a **flicker-world-style hex-sphere globe**.
//!
//! The system sim carries each body as a grid of [`HexState`] (the world-gen working state — bulk
//! `composition`, derived `crust`, plates, …). It is **materialised** from the body's composition
//! (Epoch 1 spread for solids / differential-rotation swirl for gas), then **evolved through the
//! system cutoff (Epoch 3)** by reusing the genuine `flicker_worldgen` epoch transforms: a solid
//! differentiates ([`step_solid`] → `Epoch2`), a gas giant advects its liquefied air ([`step_gas`]).
//! Each cell is a flat hex polygon (fan-triangulated centre→outline, wound outward to match the
//! engine's back-face cull) coloured by its [`HexState::surface`] expression and carrying the cell's
//! outward normal, so the engine **star point light** shades it. Built on the unit sphere; the
//! renderer scales/places it per body. The mesh is a disposable read-out of the grid.

use std::collections::HashMap;

use flicker::render::MeshVertex;
use flicker_celestial::evolution::{step_world, StepCtx};
use flicker_celestial::HexWorld;
use flicker_materials::{ElementId, Tables};
use flicker_worldgen::pipeline::NOMINAL_DURATION;
use flicker_worldgen::{Epoch1, Epoch1Params, Epoch2, EpochCtx, EpochTransform, HexState};
use flicker_worldgrid::icosphere_with_outlines;
use flicker_worldstate::Composition;
use glam::Vec3;

use crate::planet::pack_rgb;

/// A body's **cached** icosphere topology — built once when the body is materialised and reused for
/// every evolution step and mesh rebuild. The icosphere is expensive to build at high `freq` (~100k
/// cells for an Earth-sized world), so it must NOT be rebuilt per frame: this caches the transport
/// graph (`dirs`/`neighbors`) the steps need and the boundary polygons (`outlines`) the mesh needs.
pub struct Topo {
    pub freq: u32,
    pub dirs: Vec<Vec3>,
    pub neighbors: Vec<Vec<u32>>,
    pub outlines: Vec<Vec<Vec3>>,
}

impl Topo {
    /// Build the icosphere at `freq` **once**.
    pub fn new(freq: u32) -> Self {
        let (sphere, outlines) = icosphere_with_outlines(freq);
        Self { freq, dirs: sphere.dirs, neighbors: sphere.neighbors, outlines }
    }
}

/// Materialise a solid (rocky/icy) world into a [`HexState`] grid: **Epoch 1** spreads the body's
/// element `abundance` across the icosphere cells (heavy→equator, volatile→pole + regional fBm),
/// one conserved bulk [`Composition`] per cell, wrapped in a fresh [`HexState`] (undifferentiated —
/// no crust, no plates yet). `seed` makes the distribution deterministic per body. This grid is the
/// stored truth the evolution iterates ([`step_solid`]); the globe geometry is a disposable
/// read-out of its [`HexState::surface`] ([`globe_mesh`]).
pub fn materialize_solid(
    tables: &Tables,
    abundance: HashMap<String, f64>,
    topo: &Topo,
    seed: u64,
) -> Vec<HexState> {
    let e1 = Epoch1::new(tables, Epoch1Params { abundance, ..Epoch1Params::default() }, seed);
    topo.dirs.iter().map(|&d| HexState::new(e1.seed_hex(d))).collect()
}

/// Materialise a gas giant into a [`HexState`] grid: the body **is** the liquefied air, distributed
/// across the cells by differential-rotation swirl ([`gas_seed_hex`]) — no Epoch 1, no atmosphere
/// overlay. `n_bands` scales with the giant's size. The initial condition (S₀) the evolution then
/// transports forward via [`step_gas`]; a gas giant has no crust/plates, so `surface()` is the bulk.
pub fn materialize_gas(topo: &Topo, n_bands: f32) -> Vec<HexState> {
    topo.dirs.iter().map(|&d| HexState::new(gas_seed_hex(d, n_bands))).collect()
}

/// Advance a **solid** body one differentiation step at crystallisation `settle` ∈ [0,1]: run
/// [`flicker_worldgen::Epoch2`] (the `settle` mapped to its `duration` knob), draining the heavy
/// metals out of the surface **crust** into the bulk below as the molten era matures. `prev`'s bulk
/// `composition` is untouched (the conserved truth — Epoch 2 only derives the crust), so re-running
/// each step with a growing `settle` matures the differentiation; at `settle = 1` it is fully
/// differentiated and further steps are idempotent. (`seed` is for the shared `EpochCtx`; Epoch 2
/// itself is per-hex.)
pub fn step_solid(prev: &[HexState], tables: &Tables, topo: &Topo, seed: u64, settle: f64) -> Vec<HexState> {
    let ctx = EpochCtx { tables, dirs: &topo.dirs, neighbors: &topo.neighbors, seed };
    let duration = (settle.clamp(0.0, 1.0) * NOMINAL_DURATION as f64).round() as u32;
    Epoch2 { duration, ..Epoch2::default() }.apply(&ctx, prev)
}

/// Advance a **gaseous** body one step of `dt_myr`: its liquefied air ([`HexState::composition`]) is
/// transported by zonal differential-rotation advection through the conserved [`HexWorld`] bridge —
/// the swirl is *moved*, not repainted. Gas has no crust/plates, so only the composition changes and
/// `surface()` stays the bulk. `total()` is invariant.
pub fn step_gas(prev: &[HexState], topo: &Topo, dt_myr: f64) -> Vec<HexState> {
    let world = HexWorld::from_cells(topo.freq, prev.iter().map(|s| s.composition.clone()).collect());
    let ctx = StepCtx { dirs: &topo.dirs, neighbors: &topo.neighbors };
    let stepped = step_world(&world, dt_myr, &ctx);
    prev.iter()
        .zip(stepped.cells())
        .map(|(s, c)| {
            let mut s = s.clone();
            s.composition = c.clone();
            s
        })
        .collect()
}

/// Build a **flat-shaded** hex-globe mesh for one evolution state, colouring each cell from its
/// [`HexState::surface`] expression (the differentiated crust once it exists, else the bulk —
/// [`composition_color`]). Geometry is read from the cached `topo` (fan-triangulated cells wound
/// outward to match the back-face cull), so the icosphere is never rebuilt here. Returns
/// `(vertices, indices)` on the unit sphere; the renderer scales it. The evolution is shown by
/// **swapping** this mesh to the next state's at each step — flat hexes, no per-frame crossfade.
pub fn globe_mesh(state: &[HexState], topo: &Topo) -> (Vec<MeshVertex>, Vec<u32>) {
    debug_assert_eq!(state.len(), topo.outlines.len(), "state must match the topology");
    let mut verts: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (i, outline) in topo.outlines.iter().enumerate() {
        if outline.len() < 3 {
            continue; // skip degenerate/boundary cells
        }
        let outward = topo.dirs[i];
        let normal = outward.to_array();
        let material = pack_rgb(composition_color(state[i].surface()));
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

/// A gas giant's per-cell composition: liquefied air (H/He) banded by **differential rotation**
/// into zones and belts — the equator is carried faster than the poles, shearing the turbulence
/// into swirls — with dredged-up chromophores (sulfur + carbon) concentrated in the belts, which
/// is what makes a gas giant read tan rather than the blue of bare H/He. Baked once per cell
/// direction; there is no per-frame motion.
fn gas_seed_hex(dir: Vec3, n_bands: f32) -> Composition {
    let lat = dir.y;
    let lon = dir.z.atan2(dir.x);
    // Differential rotation: the longitude phase shifts with latitude, so the turbulent waves
    // drift band-to-band into swirls instead of flat stripes.
    let shear = lat * 6.0;
    let turb = (lon * 3.0 + shear).sin() * 0.12 + (lon * 7.0 - shear * 1.6).sin() * 0.06;
    let belt = (0.5 + 0.5 * ((lat + turb) * n_bands).sin()) as f64; // 0 pale zone .. 1 tan belt
    let mut c = Composition::new();
    c.add(1, 0.74); // H — the air
    c.add(2, 0.26); // He
    c.add(16, 0.55 * belt); // S — sulfur chromophore (tan), dredged into the belts
    c.add(6, 0.30 * belt); // C — hydrocarbon darkening
    c
}

/// A cell's colour: a mass-weighted blend of its elements' tints ([`element_rgb`]) — a direct
/// read-out of the material distributed into the cell, so the regional variety Epoch-1 lays down
/// (and the gas swirl) shows as colour.
fn composition_color(comp: &Composition) -> [f32; 3] {
    let mut rgb = [0.0f32; 3];
    let mut wsum = 0.0f32;
    for (el, amount) in comp.iter() {
        // Composed colour = mass (the accurate data) × pigment strength (the art): potent
        // ore/chromophore metals tint far above their mass — like trace chromium reddening a
        // ruby — so the veins Epoch-1 concentrates actually read, instead of averaging to mud.
        // Only present elements contribute, so this surfaces the distribution, never invents it.
        let w = amount as f32 * pigment(el);
        let c = element_rgb(el);
        rgb[0] += w * c[0];
        rgb[1] += w * c[1];
        rgb[2] += w * c[2];
        wsum += w;
    }
    if wsum <= 0.0 {
        return [0.05, 0.04, 0.04];
    }
    [rgb[0] / wsum * 0.9, rgb[1] / wsum * 0.9, rgb[2] / wsum * 0.9]
}

/// Visual tinting strength — the **art** weight on the accurate mass data. Potent ore /
/// chromophore metals colour far above their mass fraction (real pigments do: a ruby is trace
/// chromium), so a vein reads; bulk rock-formers (incl. iron) stay at plain mass weight.
fn pigment(el: ElementId) -> f32 {
    match el {
        16 => 3.0, // S  sulfur yellow
        24 => 5.0, // Cr chrome green
        27 => 5.0, // Co cobalt blue
        29 => 6.0, // Cu copper / verdigris
        47 => 5.0, // Ag silver
        78 => 4.0, // Pt platinum
        79 => 8.0, // Au gold — reads even as a trace vein
        82 => 3.0, // Pb galena
        92 => 4.0, // U  uranyl yellow-green
        _ => 1.0,  // bulk formers (incl. Fe) — plain mass weight
    }
}

/// Per-element tint (atomic number → RGB): grounded mineral / oxide / flame-test associations so
/// a cell's element mix reads as real colour variety — olivine-green Mg, chrome/nickel greens,
/// cobalt/titanium blues, sulfur/sodium yellows, potassium lilac, iron rust — instead of the old
/// rust-on-grey. Only elements actually present contribute, so colour stays a read-out of the
/// material distribution.
fn element_rgb(el: ElementId) -> [f32; 3] {
    match el {
        1 => [0.42, 0.52, 0.62],  // H   pale ice-blue
        6 => [0.12, 0.12, 0.13],  // C   graphite near-black
        7 => [0.46, 0.52, 0.56],  // N   pale grey-blue
        8 => [0.34, 0.42, 0.52],  // O   blue-grey (bound oxygen)
        11 => [0.74, 0.56, 0.30], // Na  sodium yellow
        12 => [0.44, 0.60, 0.46], // Mg  olivine green
        13 => [0.56, 0.58, 0.62], // Al  pale aluminium grey
        14 => [0.64, 0.56, 0.42], // Si  quartz sand
        15 => [0.58, 0.34, 0.26], // P   phosphor brick
        16 => [0.80, 0.70, 0.22], // S   sulfur yellow
        17 => [0.56, 0.68, 0.44], // Cl  pale green
        19 => [0.56, 0.40, 0.62], // K   potassium lilac
        20 => [0.74, 0.70, 0.60], // Ca  pale limestone
        22 => [0.56, 0.64, 0.74], // Ti  titanium blue-steel
        24 => [0.30, 0.56, 0.38], // Cr  chrome green
        26 => [0.56, 0.22, 0.12], // Fe  rust-red
        27 => [0.24, 0.36, 0.70], // Co  cobalt blue
        28 => [0.50, 0.58, 0.50], // Ni  nickel green-grey
        29 => [0.25, 0.66, 0.58], // Cu  verdigris teal-green
        30 => [0.62, 0.66, 0.70], // Zn  pale blue-white
        47 => [0.80, 0.82, 0.84], // Ag  bright silver
        50 => [0.62, 0.62, 0.66], // Sn  pewter grey
        78 => [0.72, 0.74, 0.76], // Pt  pale platinum
        79 => [0.86, 0.68, 0.20], // Au  gold
        82 => [0.42, 0.44, 0.50], // Pb  galena blue-grey
        92 => [0.56, 0.72, 0.24], // U   uranyl yellow-green
        _ => [0.30, 0.28, 0.26],  // unmapped → neutral rock
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{load_tables, Composition as MatComposition, MaterialClass};

    #[test]
    fn materialises_a_grid_and_builds_a_closed_globe_from_it() {
        let tables = load_tables();
        // An Earth-ish rocky body → its Epoch-1 abundance → a materialised hex grid.
        let mut comp = MatComposition::of(MaterialClass::Silicate, 0.7);
        comp.add(&MatComposition::of(MaterialClass::Metal, 0.3));
        let abundance = comp.to_epoch1_abundance(&tables);
        let topo = Topo::new(4);
        let world = materialize_solid(&tables, abundance, &topo, 123);
        // The stored grid is the full icosphere (10·freq²+2 cells), each carrying mass.
        assert_eq!(world.len(), 10 * 4 * 4 + 2);
        assert!(world.iter().map(|s| s.composition.total()).sum::<f64>() > 0.0, "every cell seeded with mass");

        // The mesh is a disposable flat-shaded read-out of that grid.
        let (verts, idx) = globe_mesh(&world, &topo);
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

    #[test]
    fn distinct_states_share_geometry_but_differ_in_colour() {
        let tables = load_tables();
        let mut comp = MatComposition::of(MaterialClass::Silicate, 0.7);
        comp.add(&MatComposition::of(MaterialClass::Metal, 0.3));
        let abundance = comp.to_epoch1_abundance(&tables);
        // Two distinct states (different province seeds) — the evolution swaps between such meshes.
        let topo = Topo::new(4);
        let a = materialize_solid(&tables, abundance.clone(), &topo, 1);
        let b = materialize_solid(&tables, abundance, &topo, 2);

        let (va, ia) = globe_mesh(&a, &topo);
        let (vb, ib) = globe_mesh(&b, &topo);
        // Same cached topology → identical geometry; only the per-cell colour (material) differs.
        assert_eq!(ia, ib, "winding/indices come from the shared topology");
        assert_eq!(va.len(), vb.len());
        let mat = |vs: &[MeshVertex]| vs.iter().map(|v| v.material).collect::<Vec<_>>();
        assert_ne!(mat(&va), mat(&vb), "different states read as different colours");
    }

    #[test]
    fn differentiation_drains_iron_from_the_surface_but_conserves_the_bulk() {
        const FE: ElementId = 26;
        let tables = load_tables();
        // An iron-rich rocky body — so the differentiation has iron to drain.
        let mut comp = MatComposition::of(MaterialClass::Metal, 0.6);
        comp.add(&MatComposition::of(MaterialClass::Silicate, 0.4));
        let abundance = comp.to_epoch1_abundance(&tables);
        let topo = Topo::new(3);
        let seed = materialize_solid(&tables, abundance, &topo, 7);

        let surface_iron_fraction = |g: &[HexState]| -> f64 {
            let fe: f64 = g.iter().map(|h| h.surface().amount(FE)).sum();
            let total: f64 = g.iter().map(|h| h.surface().total()).sum();
            if total > 0.0 { fe / total } else { 0.0 }
        };
        let bulk_iron = |g: &[HexState]| -> f64 { g.iter().map(|h| h.composition.amount(FE)).sum() };

        // Undifferentiated (settle 0) vs fully differentiated (settle 1).
        let young = step_solid(&seed, &tables, &topo, 7, 0.0);
        let old = step_solid(&seed, &tables, &topo, 7, 1.0);
        assert!(
            surface_iron_fraction(&old) < surface_iron_fraction(&young),
            "differentiation should drain iron out of the surface crust ({} → {})",
            surface_iron_fraction(&young),
            surface_iron_fraction(&old),
        );
        // The iron isn't destroyed — the bulk column is conserved, it just sits below the crust.
        assert!((bulk_iron(&old) - bulk_iron(&seed)).abs() < 1e-6, "differentiation conserves bulk iron");
        assert!(bulk_iron(&old) > 0.0, "iron stays in the bulk");
    }
}
