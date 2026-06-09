//! Flat hex tile geometry — a pointy-top hexagon in the XZ plane, displaced in Y
//! by the heightmap. No sphere, no projection: the world render is a flat local
//! bubble (spec §2). The hexagon is placed at a 2D `center`; its heightmap is
//! sampled by world XZ so neighbouring tiles in the bubble share a continuous
//! surface.

use flicker::render::{MeshVertex, Vec3};
use flicker_primitive::heightmap::world_height;

/// Hex size: centre to N/S point, world units.
pub const HEX_SIZE: f32 = 100.0;
/// Flat-to-flat half-width = √3/2 · size.
pub const HEX_HALF_W: f32 = HEX_SIZE * 0.866_025;

const TILE_GRID: usize = 8;
const EDGE_RATIO: f32 = 0.5;
const HEIGHT_SCALE: f32 = 1.2;

/// Pointy-top row width at normalized lat `a = z / size`.
fn band_width(a: f32) -> f32 {
    let a = a.abs();
    if a <= EDGE_RATIO {
        1.0
    } else {
        ((1.0 - a) / (1.0 - EDGE_RATIO)).max(0.0)
    }
}

/// Build a flat pointy-top hex placed at `(px, pz)` in the XZ plane, with its
/// heightmap read from a sample window anchored at `(sx, sz)` — so the bubble's
/// tiles share a continuous surface while each standpoint samples its own place.
pub fn build_flat_hex(px: f32, pz: f32, sx: f32, sz: f32) -> (Vec<MeshVertex>, Vec<u32>) {
    let g = TILE_GRID;
    let stride = g + 1;
    let mut positions = Vec::with_capacity(stride * stride);
    for j in 0..=g {
        let z = (j as f32 / g as f32 * 2.0 - 1.0) * HEX_SIZE;
        let wf = band_width(z / HEX_SIZE);
        for i in 0..=g {
            let x = (i as f32 / g as f32 * 2.0 - 1.0) * HEX_HALF_W * wf;
            let h = world_height(sx + x, sz + z);
            positions.push(Vec3::new(px + x, (h - 128.0) * HEIGHT_SCALE, pz + z));
        }
    }

    let at = |i: usize, j: usize| positions[j * stride + i];
    let mut vertices = Vec::with_capacity(positions.len());
    for j in 0..=g {
        for i in 0..=g {
            let du = at((i + 1).min(g), j) - at(i.saturating_sub(1), j);
            let dv = at(i, (j + 1).min(g)) - at(i, j.saturating_sub(1));
            let mut n = du.cross(dv).normalize_or(Vec3::Y);
            if n.y < 0.0 {
                n = -n;
            }
            vertices.push(MeshVertex {
                position: at(i, j).to_array(),
                normal: n.to_array(),
                material: 12,
            });
        }
    }

    let mut indices = Vec::with_capacity(g * g * 6);
    let idx = |i: usize, j: usize| (j * stride + i) as u32;
    for j in 0..g {
        for i in 0..g {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i, j + 1), idx(i + 1, j + 1));
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    (vertices, indices)
}

/// The six flat layout offsets for a pointy-top hex's neighbours, indexed by the
/// `EDGE_*` constants in `topology`: [W, NW, NE, E, SE, SW].
pub fn edge_offsets() -> [(f32, f32); 6] {
    let w = 2.0 * HEX_HALF_W; // centre-to-centre, side neighbour
    let dx = HEX_HALF_W; // diagonal x
    let dz = 1.5 * HEX_SIZE; // diagonal z
    [
        (-w, 0.0),   // W
        (-dx, dz),   // NW
        (dx, dz),    // NE
        (w, 0.0),    // E
        (dx, -dz),   // SE
        (-dx, -dz),  // SW
    ]
}
