//! Build the planet mesh.
//!
//! Most views fan-triangulate **one flat-shaded polygon per cell** (cells read as
//! solid colour blocks). The **Terrain** view instead subdivides each hex and
//! samples `flicker-worldgen`'s [`FieldSampler`] per sub-vertex — displacing the
//! surface radially by the material-**hardness**-driven relief and tinting by that
//! hardness — so you see the within-hex *shape the composition produces*, not flat
//! colour. Triangles wind outward (the pipeline back-face culls with a CCW front).

use flicker::render::{MeshVertex, Vec3};
use flicker_worldgen::FieldSampler;

use crate::color::{cell_material, hardness_terrain, ViewMode};
use crate::world::{ranges_for, WorldData};

/// Outer radius of the rendered planet (world units).
pub const RADIUS: f32 = 200.0;

// --- Terrain (sub-hex relief) tuning. Visual; adjust if the relief reads wrong. ---
/// Barycentric subdivisions per hex fan-triangle (n² sub-triangles each). Higher =
/// finer within-hex detail, more triangles.
const TERRAIN_SUBDIV: usize = 3;
/// `dir * SAMPLE_SCALE` is the position fed to the field — higher = finer detail
/// packed into each hex.
const SAMPLE_SCALE: f32 = 50_000.0;
/// Per-hex (macro) elevation `[-1, 1]` → radial bump.
const MACRO_BUMP: f32 = 0.06 * RADIUS;
/// Field ridge/fold (micro) relief → radial bump, clamped.
const MICRO_GAIN: f32 = 0.06;
const MICRO_CLAMP: f32 = 0.10 * RADIUS;

/// Vertices + indices for the whole planet at `radius`, showing epoch layer
/// `epoch` coloured by `mode`. `sampler` drives the Terrain view's sub-hex relief.
pub fn build(
    data: &WorldData,
    epoch: usize,
    mode: ViewMode,
    radius: f32,
    sampler: &FieldSampler,
) -> (Vec<MeshVertex>, Vec<u32>) {
    if mode == ViewMode::Terrain {
        return build_terrain(data, epoch, radius, sampler);
    }

    let states = &data.layers[epoch.min(data.layers.len() - 1)];
    let ranges = ranges_for(states);
    let mut verts: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (i, outline) in data.outlines.iter().enumerate() {
        if outline.len() < 3 {
            continue; // skip any degenerate/boundary cell
        }
        let outward = data.sphere.dirs[i];
        let normal = outward.to_array();
        let material = cell_material(mode, &states[i], &ranges);
        let center = outward * radius;

        let base = verts.len() as u32;
        verts.push(MeshVertex { position: center.to_array(), normal, material });
        for c in outline {
            verts.push(MeshVertex { position: (*c * radius).to_array(), normal, material });
        }

        let n = outline.len();
        for k in 0..n {
            let c0 = outline[k] * radius;
            let c1 = outline[(k + 1) % n] * radius;
            let i_center = base;
            let i0 = base + 1 + k as u32;
            let i1 = base + 1 + ((k + 1) % n) as u32;
            // Wind so the triangle faces outward (front = CCW, back-culled).
            if (c0 - center).cross(c1 - center).dot(outward) >= 0.0 {
                indices.extend([i_center, i0, i1]);
            } else {
                indices.extend([i_center, i1, i0]);
            }
        }
    }

    (verts, indices)
}

/// The Terrain view: each hex is subdivided and the [`FieldSampler`] sampled per
/// sub-vertex for relief (radial displacement) + hardness tint. The per-hex macro
/// elevation/orogeny is smoothed across neighbours first so the base doesn't step
/// at hex seams; the sub-hex ridge detail is continuous because it's a function of
/// the shared 3D position.
fn build_terrain(
    data: &WorldData,
    epoch: usize,
    radius: f32,
    sampler: &FieldSampler,
) -> (Vec<MeshVertex>, Vec<u32>) {
    let states = &data.layers[epoch.min(data.layers.len() - 1)];
    let n = states.len();

    // Neighbour-smoothed macro elevation + orogeny (reduce per-hex stepping).
    let blended: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let (mut e, mut o, mut c) = (states[i].elevation, states[i].orogeny, 1.0f32);
            for &nb in &data.sphere.neighbors[i] {
                let s = &states[nb as usize];
                e += s.elevation;
                o += s.orogeny;
                c += 1.0;
            }
            (e / c, o / c)
        })
        .collect();

    let mut verts: Vec<MeshVertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for (i, outline) in data.outlines.iter().enumerate() {
        if outline.len() < 3 {
            continue;
        }
        let center = data.sphere.dirs[i];
        let (be, bo) = blended[i];
        let state = &states[i];
        let submerged = state.water_depth > 0.0;

        // Sample a unit direction → (displaced world position, hardness tint).
        let sample = |dir: Vec3| -> (Vec3, u32) {
            let s = sampler.sample_blended_at(state, dir * SAMPLE_SCALE, 0.0, bo);
            let micro = (MICRO_GAIN * s.elevation).clamp(-MICRO_CLAMP, MICRO_CLAMP);
            let disp = MACRO_BUMP * be + micro;
            (dir * (radius + disp), hardness_terrain(s.hardness, s.vein, submerged))
        };

        let m = outline.len();
        for k in 0..m {
            subdivide(center, outline[k], outline[(k + 1) % m], TERRAIN_SUBDIV, &sample, &mut verts, &mut indices);
        }
    }

    (verts, indices)
}

/// Barycentrically subdivide the spherical triangle `(a, b, c)` (unit dirs) into
/// `sub²` sub-triangles, emitting each via `sample`.
fn subdivide<F: Fn(Vec3) -> (Vec3, u32)>(
    a: Vec3,
    b: Vec3,
    c: Vec3,
    sub: usize,
    sample: &F,
    verts: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
) {
    let nsub = sub.max(1);
    let pt = |i: usize, j: usize| -> Vec3 {
        let (wa, wb, wc) = ((nsub - i - j) as f32, i as f32, j as f32);
        ((a * wa + b * wb + c * wc) / nsub as f32).normalize()
    };
    for i in 0..nsub {
        for j in 0..(nsub - i) {
            tri(pt(i, j), pt(i, j + 1), pt(i + 1, j), sample, verts, indices);
            if j < nsub - i - 1 {
                tri(pt(i, j + 1), pt(i + 1, j + 1), pt(i + 1, j), sample, verts, indices);
            }
        }
    }
}

/// Emit one sub-triangle (3 unshared verts with a geometric, outward-facing normal
/// so the relief shows under lighting).
fn tri<F: Fn(Vec3) -> (Vec3, u32)>(
    d0: Vec3,
    d1: Vec3,
    d2: Vec3,
    sample: &F,
    verts: &mut Vec<MeshVertex>,
    indices: &mut Vec<u32>,
) {
    let (p0, c0) = sample(d0);
    let (p1, c1) = sample(d1);
    let (p2, c2) = sample(d2);
    let outward = (d0 + d1 + d2).normalize_or_zero();
    let face = (p1 - p0).cross(p2 - p0);
    let base = verts.len() as u32;
    let mv = |p: Vec3, nrm: Vec3, m: u32| MeshVertex {
        position: p.to_array(),
        normal: nrm.to_array(),
        material: m,
    };
    if face.dot(outward) >= 0.0 {
        let nrm = unit_or(face, outward);
        verts.push(mv(p0, nrm, c0));
        verts.push(mv(p1, nrm, c1));
        verts.push(mv(p2, nrm, c2));
    } else {
        let nrm = unit_or(-face, outward);
        verts.push(mv(p0, nrm, c0));
        verts.push(mv(p2, nrm, c2));
        verts.push(mv(p1, nrm, c1));
    }
    indices.extend([base, base + 1, base + 2]);
}

/// `v` normalized, falling back to `fallback` for a degenerate (zero) vector.
fn unit_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let n = v.normalize_or_zero();
    if n == Vec3::ZERO {
        fallback
    } else {
        n
    }
}
