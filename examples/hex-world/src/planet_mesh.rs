//! Per-hex tile geometry, flattened onto the **focus hex's plane**.
//!
//! Each hex is still shaped as a pointy-top hexagon (point N/S, flat E/W, teeth
//! interlocking — see the topology + offset notes), but instead of resting on
//! its own sphere-tangent facet, every vertex is **gnomonic-projected onto the
//! focus hex's tangent plane**. The focus maps to its own center undistorted;
//! neighbours splay outward. The result reads as flat local ground, and when
//! the focus changes the whole patch reprojects onto the new plane — it snaps.

use crate::topology::{Hemisphere, HexCoord, Planet};
use flicker::render::{MeshVertex, Vec3};
use flicker_primitive::heightmap::world_height_seeded;
use std::f32::consts::PI;

/// Radius of the virtual planet, world units.
pub const PLANET_RADIUS: f32 = 1000.0;

/// Cells per tile edge (so `(G+1)²` vertices).
const TILE_GRID: usize = 8;
/// Fraction of the half-height occupied by the flat E/W band; beyond it the row
/// tapers to the N/S point. `0.5` ≈ a regular pointy-top hexagon.
const EDGE_RATIO: f32 = 0.5;
/// Latitude reach as a fraction of the ring spacing. `>0.5` makes the N/S points
/// overhang so teeth interlock across band boundaries and the equator overlaps.
const BAND_HALF: f32 = 0.64;
/// Radial displacement per heightmap unit (relief on the flattened plane).
const HEIGHT_SCALE: f32 = 2.2;
/// Degrees → heightmap voxel sample domain. Sets continent wavelength.
const SAMPLE_SCALE: f32 = 11.0;
/// Outlines drawn this far above the tile, to clear z-fighting.
const OUTLINE_LIFT: f32 = 3.0;
/// Fixed world seed so the planet is the same every run.
const WORLD_SEED: u64 = 0x0F1C_4E12;
/// Gnomonic guard: never divide by less than this (avoids blow-up past ~78°
/// off the focus; the local patch never reaches that).
const MIN_FACING: f32 = 0.2;

/// The plane every local tile is flattened onto: the focus hex's radial (`up`)
/// and its tangent-plane center. Gnomonic projection works in world space off
/// `up` alone, so no in-plane basis is needed here.
pub struct FocusFrame {
    pub up: Vec3,
    pub center: Vec3,
}

/// Tangent frame at the focus hex — the plane the local patch snaps to.
pub fn focus_frame(planet: &Planet, coord: HexCoord) -> FocusFrame {
    let up = Vec3::from_array(planet.unit_position(coord));
    FocusFrame {
        up,
        center: up * PLANET_RADIUS,
    }
}

/// Gnomonic projection of a sphere direction onto the focus plane: the point
/// where the ray from the planet center through `dir` meets the tangent plane.
fn gnomonic(dir: Vec3, focus: &FocusFrame) -> Vec3 {
    let denom = dir.dot(focus.up).max(MIN_FACING);
    dir * (PLANET_RADIUS / denom)
}

/// Tangent frame + half-extents at a hex center, used to *shape* the hexagon
/// (before it's flattened onto the focus plane).
fn tile_frame(planet: &Planet, coord: HexCoord) -> (Vec3, Vec3, Vec3, f32, f32) {
    let up = Vec3::from_array(planet.unit_position(coord));
    let east = up.cross(Vec3::Y).normalize_or(Vec3::X);
    let north = east.cross(up).normalize_or(Vec3::Z);

    let lat0 = planet.latitude_deg(coord).to_radians();
    let half_lat = (PI / 2.0 / (planet.rings() as f32 + 0.5)) * BAND_HALF;
    let half_lon = if coord.ring == 0 {
        half_lat
    } else {
        (2.0 * PI / (6.0 * coord.ring as f32)) * 0.5
    };
    let half_n = PLANET_RADIUS * half_lat;
    let half_e = if coord.ring == 0 {
        PLANET_RADIUS * half_lat
    } else {
        PLANET_RADIUS * lat0.cos().abs().max(0.05) * half_lon
    };
    (up, east, north, half_e, half_n)
}

/// Hexagon row width at normalized latitude `a = v / half_n`: full across the
/// flat E/W band, tapering to 0 at the N/S point.
fn band_width(a: f32) -> f32 {
    let a = a.abs();
    if a <= EDGE_RATIO {
        1.0
    } else {
        ((1.0 - a) / (1.0 - EDGE_RATIO)).max(0.0)
    }
}

/// Pole-adjacent rings flatten their pole-facing tip into the pentagon's flat
/// edge: +v faces the pole in the north, −v in the south. Returns
/// `(flatten_top, flatten_bottom)`. The 12 such tiles (6 per pole) are the
/// buckyball pentagons that close the caps.
fn pole_facing_flatten(coord: HexCoord) -> (bool, bool) {
    if coord.ring == 1 {
        match coord.hemi {
            Hemisphere::North => (true, false),
            Hemisphere::South => (false, true),
        }
    } else {
        (false, false)
    }
}

/// Build one hex, flattened onto `focus`'s plane: `(vertices, indices)`.
pub fn build_tile(
    planet: &Planet,
    coord: HexCoord,
    focus: &FocusFrame,
) -> (Vec<MeshVertex>, Vec<u32>) {
    if coord.ring == 0 {
        return build_pole(planet, coord, focus);
    }
    let (_up_c, east_c, north_c, half_e, half_n) = tile_frame(planet, coord);
    let center_c = Vec3::from_array(planet.unit_position(coord)) * PLANET_RADIUS;

    // Pentagon flattening for pole-adjacent rings: clip the pole-facing tip back
    // to the flat-band edge, so it ends in a flat edge instead of a point.
    let (flat_top, flat_bot) = pole_facing_flatten(coord);
    let v_top = if flat_top { half_n * EDGE_RATIO } else { half_n };
    let v_bot = if flat_bot { -half_n * EDGE_RATIO } else { -half_n };

    let g = TILE_GRID;
    let stride = g + 1;
    let mut positions = Vec::with_capacity(stride * stride);
    for j in 0..=g {
        let v = v_bot + (v_top - v_bot) * (j as f32 / g as f32);
        let wf = band_width(v / half_n);
        for i in 0..=g {
            let u = (i as f32 / g as f32 * 2.0 - 1.0) * half_e * wf;
            // Shape the vertex in the hex's own tangent frame, then flatten.
            let dir = (center_c + east_c * u + north_c * v).normalize();
            let lon = dir.z.atan2(dir.x).to_degrees();
            let lat = dir.y.clamp(-1.0, 1.0).asin().to_degrees();
            let h = world_height_seeded(lon * SAMPLE_SCALE, lat * SAMPLE_SCALE, WORLD_SEED);
            positions.push(gnomonic(dir, focus) + focus.up * (h - 128.0) * HEIGHT_SCALE);
        }
    }

    let at = |i: usize, j: usize| positions[j * stride + i];
    let mut vertices = Vec::with_capacity(positions.len());
    for j in 0..=g {
        for i in 0..=g {
            let du = at((i + 1).min(g), j) - at(i.saturating_sub(1), j);
            let dv = at(i, (j + 1).min(g)) - at(i, j.saturating_sub(1));
            let mut n = du.cross(dv).normalize_or(focus.up);
            if n.dot(focus.up) < 0.0 {
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
            let (v00, v10, v01, v11) = (idx(i, j), idx(i + 1, j), idx(i, j + 1), idx(i + 1, j + 1));
            indices.extend_from_slice(&[v00, v01, v10, v10, v01, v11]);
        }
    }

    (vertices, indices)
}

/// The pole crown's six vertices, as sphere directions: each is the shared
/// pole-facing corner of a ring-1 pentagon, so the crown's edges coincide with
/// the pentagons' flat tops and the cap seals with no gap.
fn crown_corner_dirs(planet: &Planet, pole: HexCoord) -> [Vec3; 6] {
    let v_sign = match pole.hemi {
        Hemisphere::North => 1.0,
        Hemisphere::South => -1.0,
    };
    let mut out = [Vec3::ZERO; 6];
    for (k, slot) in out.iter_mut().enumerate() {
        let r1 = HexCoord {
            hemi: pole.hemi,
            ring: 1,
            pos: k as u32,
        };
        let (_u, east, north, half_e, half_n) = tile_frame(planet, r1);
        let center = Vec3::from_array(planet.unit_position(r1)) * PLANET_RADIUS;
        let he = half_n * EDGE_RATIO * v_sign;
        // The ring-1 hex's pole-facing W corner (shared with the next hex's E).
        *slot = (center - east * half_e + north * he).normalize();
    }
    out
}

/// Build the pole as a hexagonal crown: a fan from the pole center to the six
/// shared pentagon flat-top corners, flattened onto `focus`'s plane.
fn build_pole(planet: &Planet, coord: HexCoord, focus: &FocusFrame) -> (Vec<MeshVertex>, Vec<u32>) {
    let place = |dir: Vec3| -> Vec3 {
        let lon = dir.z.atan2(dir.x).to_degrees();
        let lat = dir.y.clamp(-1.0, 1.0).asin().to_degrees();
        let h = world_height_seeded(lon * SAMPLE_SCALE, lat * SAMPLE_SCALE, WORLD_SEED);
        gnomonic(dir, focus) + focus.up * (h - 128.0) * HEIGHT_SCALE
    };
    let pole_dir = Vec3::from_array(planet.unit_position(coord));
    let mut positions = vec![place(pole_dir)];
    for d in crown_corner_dirs(planet, coord) {
        positions.push(place(d));
    }

    let normal = focus.up.to_array();
    let vertices: Vec<MeshVertex> = positions
        .iter()
        .map(|p| MeshVertex {
            position: p.to_array(),
            normal,
            material: 12,
        })
        .collect();

    // Wind the fan so its front faces the camera (toward the focus up).
    let face = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
    let flip = face.dot(focus.up) < 0.0;
    let mut indices = Vec::with_capacity(18);
    for k in 0..6u32 {
        let (a, b) = (1 + k, 1 + (k + 1) % 6);
        if flip {
            indices.extend_from_slice(&[0, b, a]);
        } else {
            indices.extend_from_slice(&[0, a, b]);
        }
    }
    (vertices, indices)
}

/// Perimeter corners, flattened onto `focus`'s plane, for the outline. Six for
/// a hex; five for a pole-adjacent pentagon (its pole-facing point becomes a
/// flat edge); the six crown corners for a pole. Order traces the perimeter.
pub fn hex_corners(planet: &Planet, coord: HexCoord, focus: &FocusFrame) -> Vec<Vec3> {
    if coord.ring == 0 {
        return crown_corner_dirs(planet, coord)
            .iter()
            .map(|&d| gnomonic(d, focus) + focus.up * OUTLINE_LIFT)
            .collect();
    }
    let (_up_c, east_c, north_c, half_e, half_n) = tile_frame(planet, coord);
    let center_c = Vec3::from_array(planet.unit_position(coord)) * PLANET_RADIUS;
    let he = half_n * EDGE_RATIO;
    let corner = |u: f32, v: f32| {
        let dir = (center_c + east_c * u + north_c * v).normalize();
        gnomonic(dir, focus) + focus.up * OUTLINE_LIFT
    };
    let (flat_top, flat_bot) = pole_facing_flatten(coord);

    let mut uv: Vec<(f32, f32)> = vec![(-half_e, he)]; // NW
    if !flat_top {
        uv.push((0.0, half_n)); // N point (dropped for a flat-top pentagon)
    }
    uv.push((half_e, he)); // NE
    uv.push((half_e, -he)); // SE
    if !flat_bot {
        uv.push((0.0, -half_n)); // S point (dropped for a flat-bottom pentagon)
    }
    uv.push((-half_e, -he)); // SW
    uv.into_iter().map(|(u, v)| corner(u, v)).collect()
}

/// World anchor for a hex's index label, floating just above the flattened tile.
pub fn tile_center(planet: &Planet, coord: HexCoord, focus: &FocusFrame) -> Vec3 {
    let dir = Vec3::from_array(planet.unit_position(coord));
    gnomonic(dir, focus) + focus.up * (OUTLINE_LIFT + 12.0)
}
