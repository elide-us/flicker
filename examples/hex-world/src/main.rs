//! HexWorld — **the whole flat hex graph as a living world** (POC).
//!
//! The entire hex array from [`topology`] laid out flat — no sphere projection.
//! Each hemisphere is a spiral of concentric rings: the **north pole** sits at
//! the centre of the left cluster spiralling out to its equator ring, and the
//! **south pole** at the centre of the right cluster spiralling in — exactly the
//! two-disc layout from the notebook sketch. (Cross-hemisphere "virtual"
//! adjacency lives in the graph's `edge_refs`, not in this flat picture.)
//!
//! Every hex owns a [`LayerStack`] and runs the full water cycle + climate; the
//! surface is drawn biome-coloured (the realized composite) with a drifting
//! cloud deck. Terrain is continuous within a hemisphere because each tile
//! samples its own world window of the shared heightmap.
//!
//! **Fly controls:** W/S forward/back, A/D left/right, R/F up/down, hold
//! **right mouse** to look (pitch/yaw). Esc quits.
//!
//! Reusable nucleus is in [`layers`]; the flat graph is in [`topology`].

mod layers;
#[allow(dead_code)] // parked graph: only layout uses it here; adjacency kept for later.
mod topology;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    TextureHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};
use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{Epoch1, Epoch1Params};
use flicker_worldstate::Composition;

use layers::{LayerStack, G, HEX_HALF_W, HEX_SIZE};
use topology::{HexCoord, HexMap, Hemisphere};

const SIM_DT: f32 = 0.05;
/// Rings per hemisphere (R=3 → 74 hexes total).
const RINGS: u32 = 3;
/// The whole world is settled once at startup, then only hexes within this many
/// rings of the **north pole** keep animating live — the rest are frozen so the
/// per-frame mesh churn stays small.
const ANIMATE_RINGS: u32 = 1;
/// Ticks run on every hex at startup so the static backdrop looks settled.
const SETTLE_TICKS: u32 = 50;
/// Meshes rebuild on this real-time cadence, decoupled from the 20 Hz sim, so a
/// world of tiles doesn't re-upload every frame.
const REBUILD_INTERVAL: f32 = 0.15;

const MOVE_SPEED: f32 = 950.0;
const LOOK_SENS: f32 = 0.005;
const WY_CLOUD: f32 = 55.0;
/// Height of the hex-index billboards — clear above the cloud deck.
const BILLBOARD_Y: f32 = 120.0;
/// Height of the reference graticule overlay (above clouds, below billboards).
const GRATICULE_Y: f32 = 95.0;
/// Meridian spokes per disc (Earth-style time-zone bands).
const TIME_ZONES: usize = 24;

// --- Epoch 1 geology stack (the exploded layer view) ---
/// Materials vocabulary directory (the JSON tables), relative to this example.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
/// World seed for the Epoch 1 element distribution.
const GEO_SEED: u64 = 0x0EC0_DE01;
/// Duplicate surface planes stacked below the surface — placeholders for the
/// sub-surface layers later epochs will fill.
const GEO_DUP_LAYERS: usize = 8;
/// Total stacked planes: the surface, its duplicates, and the Epoch 1 map.
const GEO_PLANES: usize = GEO_DUP_LAYERS + 2;
/// Index of the bottom plane — the Epoch 1 dominant-element map.
const GEO_EPOCH_PLANE: usize = GEO_PLANES - 1;
/// Vertical gap between stacked planes, world units.
const GEO_SPACING: f32 = 200.0;

// Inspect (split-out column) display mapping. The column is the world's real
// 256-layer y-axis (the 8-bit `ClusterId` y-field), drawn at `COL_PER_LAYER`
// display units per cluster-layer, rising from `COL_BASE` over the selected hex.
const COL_BASE: f32 = 110.0;
const COL_PER_LAYER: f32 = 3.5;
/// Display Y of (fractional) cluster-layer `layer` within the split-out column.
fn layer_py(layer: f32) -> f32 {
    COL_BASE + layer * COL_PER_LAYER
}
/// Translucent wall material per band (bottom→top): the three zones — below
/// (molten / rock / soil), terrain (lowland / hill / alpine), atmosphere (lower
/// air / cloud deck / thin air) — so the column's vertical structure reads.
const BAND_MAT: [u32; layers::BANDS] = [
    layers::M_LAVA,          // 0 molten floor (GM layer)
    layers::M_STONE,         // 1 deep / resource veins
    layers::M_LAND,          // 2 shallow subsurface / caves
    layers::M_GRASSLAND,     // 3 lowlands
    layers::M_FOREST,        // 4 hills
    layers::M_TUNDRA,        // 5 highlands / alpine
    layers::M_WATER_SHALLOW, // 6 lower troposphere
    layers::M_UV,            // 7 mid / cloud deck
    layers::M_AURORA,        // 8 thin air → space
];

/// A coloured group of world-space line segments (one graticule curve set).
type LineGroup = (Vec<(Vec3, Vec3)>, [f32; 4]);

/// One hex's data and its flat-layout placement.
struct HexTile {
    place: Vec2,
    stack: LayerStack,
}

/// One uploaded mesh: handle, draw tint, model transform (places it in the map).
struct Sheet {
    handle: MeshHandle,
    tint: [f32; 4],
    model: Mat4,
}

#[inline]
fn at(field: &[f32], i: usize, j: usize) -> f32 {
    field[j * G + i]
}

/// Flat XZ offset of ring `pos` within ring `ring`, by walking the hex ring at
/// radius `ring`. Axial coords map to **flat-top** pixel space via the (NE, N)
/// basis — flat-top hexes have no E/W neighbour (those are the points), so the
/// two basis directions are NE and the vertical N.
fn ring_offset(ring: u32, pos: u32) -> Vec2 {
    if ring == 0 {
        return Vec2::ZERO;
    }
    const DIRS: [(i32, i32); 6] = [(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)];
    let (mut q, mut r) = (DIRS[4].0 * ring as i32, DIRS[4].1 * ring as i32);
    let mut cells = Vec::with_capacity(6 * ring as usize);
    for &(dq, dr) in &DIRS {
        for _ in 0..ring {
            cells.push((q, r));
            q += dq;
            r += dr;
        }
    }
    let (cq, cr) = cells[pos as usize % cells.len()];
    let aq = Vec2::new(1.5 * HEX_SIZE, HEX_HALF_W); // flat-top NE (axial +q)
    let ar = Vec2::new(0.0, 2.0 * HEX_HALF_W); // flat-top N (axial +r)
    aq * cq as f32 + ar * cr as f32
}

/// Flat position of a hex: its hemisphere's cluster centre plus its ring offset.
/// North cluster sits left, south cluster right, separated so equators meet.
/// Flat-layout centre (the pole) of a hemisphere's disc. The two discs are drawn
/// side by side as independent polar maps — no implied join between them.
fn cluster_center(hemi: Hemisphere) -> Vec2 {
    let sep = 1.5 * HEX_SIZE * (RINGS as f32 + 0.5);
    match hemi {
        Hemisphere::North => Vec2::new(-sep, 0.0),
        Hemisphere::South => Vec2::new(sep, HEX_HALF_W),
    }
}

fn hex_flat_pos(coord: HexCoord) -> Vec2 {
    cluster_center(coord.hemi) + ring_offset(coord.ring, coord.pos)
}

/// Radial distance from a disc's pole to ring `value` (fractional allowed). Ring
/// pitch is the flat-top hex centre-to-centre distance.
fn ring_radius(value: f32) -> f32 {
    value * 2.0 * HEX_HALF_W
}

/// Ring value of an Earth latitude on the disc: pole (90°) → 0, equator (0°) →
/// `R + 0.5` (half a hex out in the teeth — the same mapping `celestial_dir` uses).
fn latitude_ring(lat_deg: f32) -> f32 {
    (90.0 - lat_deg.abs()) / 90.0 * (RINGS as f32 + 0.5)
}

/// A horizontal circle of line segments at height `GRATICULE_Y`, around `center`.
fn circle(center: Vec2, radius: f32, segs: usize) -> Vec<(Vec3, Vec3)> {
    (0..segs)
        .map(|i| {
            let a0 = i as f32 / segs as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / segs as f32 * std::f32::consts::TAU;
            (
                Vec3::new(center.x + radius * a0.cos(), GRATICULE_Y, center.y + radius * a0.sin()),
                Vec3::new(center.x + radius * a1.cos(), GRATICULE_Y, center.y + radius * a1.sin()),
            )
        })
        .collect()
}

/// `count` radial meridian spokes from the pole out to `r_outer`.
fn meridians(center: Vec2, r_outer: f32, count: usize) -> Vec<(Vec3, Vec3)> {
    let start = -std::f32::consts::FRAC_PI_2; // longitude 0 = the pos-0 direction (−Z)
    (0..count)
        .map(|i| {
            let a = start + i as f32 / count as f32 * std::f32::consts::TAU;
            (
                Vec3::new(center.x, GRATICULE_Y, center.y),
                Vec3::new(center.x + r_outer * a.cos(), GRATICULE_Y, center.y + r_outer * a.sin()),
            )
        })
        .collect()
}

/// Reference graticule for both discs: (segments, colour) groups — equator,
/// tropics, polar circles, and time-zone meridians. Static; built once.
fn build_graticule() -> Vec<LineGroup> {
    let eq_r = ring_radius(latitude_ring(0.0));
    let trop_r = ring_radius(latitude_ring(23.5));
    let polar_r = ring_radius(latitude_ring(66.5));
    let (mut equator, mut tropic, mut polar, mut grid) = (vec![], vec![], vec![], vec![]);
    for hemi in [Hemisphere::North, Hemisphere::South] {
        let c = cluster_center(hemi);
        equator.extend(circle(c, eq_r, 96));
        tropic.extend(circle(c, trop_r, 96));
        polar.extend(circle(c, polar_r, 72));
        grid.extend(meridians(c, eq_r, TIME_ZONES));
    }
    vec![
        (equator, [1.0, 0.25, 0.25, 1.0]),  // equator — red
        (tropic, [1.0, 0.65, 0.2, 0.9]),    // tropics — orange
        (polar, [0.4, 0.85, 1.0, 0.9]),     // polar circles — cyan
        (grid, [0.75, 0.75, 0.85, 0.5]),    // time-zone meridians — dim
    ]
}

fn mk(renderer: &mut Renderer, mesh: (Vec<MeshVertex>, Vec<u32>), tint: [f32; 4], model: Mat4) -> Option<Sheet> {
    let (verts, inds) = mesh;
    if inds.is_empty() {
        return None;
    }
    Some(Sheet {
        handle: renderer.upload_mesh(&verts, MeshIndices::U32(&inds)),
        tint,
        model,
    })
}

/// Push one tile's biome-coloured surface + cloud deck, placed in the map.
fn push_tile(out: &mut Vec<Sheet>, t: &HexTile, renderer: &mut Renderer) {
    let s = &t.stack;
    let model = Mat4::from_translation(Vec3::new(t.place.x, 0.0, t.place.y));
    let realized = layers::build_sheet(0.0, |i, j| s.realized(i, j).0, |i, j| s.realized(i, j).1, |_, _| true);
    out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], model));
    let cloud = layers::build_sheet(WY_CLOUD, |i, j| at(&s.cloud, i, j) * 20.0, |_, _| layers::M_CLOUD, |i, j| at(&s.cloud, i, j) > 0.15);
    out.extend(mk(renderer, cloud, [1.0, 1.0, 1.0, 0.5], model));
}

/// The biome surface plane for the geology stack — every hex's realized surface
/// (no cloud deck), built once and reused at each stacked layer's Y offset.
fn build_stack_surface(tiles: &[HexTile], renderer: &mut Renderer) -> Vec<Sheet> {
    let mut out = Vec::new();
    for t in tiles {
        let model = Mat4::from_translation(Vec3::new(t.place.x, 0.0, t.place.y));
        let realized = layers::build_sheet(
            0.0,
            |i, j| t.stack.realized(i, j).0,
            |i, j| t.stack.realized(i, j).1,
            |_, _| true,
        );
        out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], model));
    }
    out
}

/// HSV→RGB, all channels in `[0, 1]`.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = (h.fract() + 1.0).fract() * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h6 % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// A distinct colour per element — a golden-ratio hue by atomic number, so the
/// elements read as clearly different tints. (The periodic table carries no
/// colour; this is a debug palette for the dominant-element map.)
fn element_color(atomic_number: u8) -> [f32; 4] {
    let hue = (atomic_number as f32 * 0.618_034).fract();
    let [r, g, b] = hsv_to_rgb(hue, 0.62, 0.95);
    [r, g, b, 1.0]
}

/// The Epoch 1 plane tint for a hex: its dominant element's colour, or grey for
/// an empty composition.
fn hex_color(comp: &Composition) -> [f32; 4] {
    match comp.dominant() {
        Some(n) => element_color(n),
        None => [0.5, 0.5, 0.5, 1.0],
    }
}

/// Build sheets for the tiles whose animated-flag matches `animated`.
fn build_subset(tiles: &[HexTile], mask: &[bool], animated: bool, renderer: &mut Renderer) -> Vec<Sheet> {
    let mut out = Vec::new();
    for (t, &m) in tiles.iter().zip(mask) {
        if m == animated {
            push_tile(&mut out, t, renderer);
        }
    }
    out
}

/// Build sheets for an explicit set of tile indices (the Local view's ≤7 tiles).
fn build_tiles(tiles: &[HexTile], indices: &[usize], renderer: &mut Renderer) -> Vec<Sheet> {
    let mut out = Vec::new();
    for &i in indices {
        push_tile(&mut out, &tiles[i], renderer);
    }
    out
}

/// Local-view camera pose framing the focus hex centred at `place`.
fn local_cam_pose(place: Vec2) -> (Vec3, f32, f32) {
    (
        Vec3::new(place.x, LOCAL_CAM_UP, place.y - LOCAL_CAM_BACK),
        0.0,
        LOCAL_CAM_PITCH,
    )
}

/// A floating layer label: world position, text, colour.
type Label = (Vec3, &'static str, [f32; 4]);

/// Pick the hex under the cursor: cast a ray from the camera through the cursor,
/// intersect the (near-flat) ground plane, return the nearest hex centre.
fn pick_hex(camera: &Camera, cursor: Vec2, size: Vec2, tiles: &[HexTile]) -> Option<usize> {
    let inv = camera.view_projection(size.x / size.y).inverse();
    let nx = cursor.x / size.x * 2.0 - 1.0;
    let ny = 1.0 - cursor.y / size.y * 2.0;
    let near = inv.project_point3(Vec3::new(nx, ny, 0.0));
    let far = inv.project_point3(Vec3::new(nx, ny, 1.0));
    let dir = far - near;
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = -near.y / dir.y;
    if t < 0.0 {
        return None;
    }
    let hit = near + dir * t;
    let p = Vec2::new(hit.x, hit.z);
    let (i, d2) = tiles
        .iter()
        .enumerate()
        .map(|(i, t)| (i, (t.place - p).length_squared()))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
    // Reject clicks well outside any hex.
    (d2 < (HEX_SIZE * 1.2).powi(2)).then_some(i)
}

/// The `BANDS + 1` display-Y boundaries of the column's bands — the real
/// cluster-layer partition ([`layers::BAND_BOUNDS`]) mapped through [`layer_py`].
/// `column_lines` rings every boundary; `build_inspect` walls between them.
fn band_boundaries() -> [f32; layers::BANDS + 1] {
    let mut by = [0.0f32; layers::BANDS + 1];
    for (b, y) in by.iter_mut().enumerate() {
        *y = layer_py(layers::BAND_BOUNDS[b] as f32);
    }
    by
}

/// XZ of flat-top hex corner `k` around `place`.
fn hex_corner(place: Vec2, k: usize) -> (f32, f32) {
    let a = k as f32 * std::f32::consts::FRAC_PI_3;
    (place.x + HEX_SIZE * a.cos(), place.y + HEX_SIZE * a.sin())
}

/// The 6 translucent side faces of a hexagonal prism band `[y0, y1]` around
/// `place`, material `mat`. Double-sided so it's visible from inside and out.
fn hex_walls(place: Vec2, y0: f32, y1: f32, mat: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(24);
    let mut inds = Vec::with_capacity(72);
    for k in 0..6 {
        let (x0, z0) = hex_corner(place, k);
        let (x1, z1) = hex_corner(place, (k + 1) % 6);
        let (mx, mz) = ((x0 + x1) * 0.5 - place.x, (z0 + z1) * 0.5 - place.y);
        let nl = (mx * mx + mz * mz).sqrt().max(1e-6);
        let normal = [mx / nl, 0.0, mz / nl];
        let b = verts.len() as u32;
        for (x, y, z) in [(x0, y0, z0), (x1, y0, z1), (x1, y1, z1), (x0, y1, z0)] {
            verts.push(MeshVertex { position: [x, y, z], normal, material: mat });
        }
        inds.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]); // outward
        inds.extend_from_slice(&[b, b + 2, b + 1, b, b + 3, b + 2]); // inward
    }
    (verts, inds)
}

/// Edge lines of the whole column: a hex ring at every band boundary, plus the
/// six full-height vertical edges.
fn column_lines(place: Vec2) -> Vec<(Vec3, Vec3)> {
    let by = band_boundaries();
    let c = |k: usize, y: f32| {
        let (x, z) = hex_corner(place, k);
        Vec3::new(x, y, z)
    };
    let mut segs = Vec::new();
    for &y in &by {
        for k in 0..6 {
            segs.push((c(k, y), c((k + 1) % 6, y)));
        }
    }
    let (y0, y1) = (by[0], by[by.len() - 1]);
    for k in 0..6 {
        segs.push((c(k, y0), c(k, y1)));
    }
    segs
}

/// Build the split-out **vertical column** + labels for one hex, lined up in the
/// world over `place`. The column is the world's real 256-layer y-axis: nine
/// translucent zone-coloured bands (below / terrain / atmosphere), the surface
/// relief seated at its normalized layer in the terrain third, the cloud deck in
/// the air bands, and the per-band temperature profile + air-band moisture as
/// flat heatmaps at their true altitudes. This is the verification instrument
/// for the Step-2 vertical model; band-to-band physics arrives in Step 3.
fn build_inspect(s: &LayerStack, place: Vec2, renderer: &mut Renderer) -> (Vec<Sheet>, Vec<Label>) {
    let off = Vec3::new(place.x, 0.0, place.y);
    let m = Mat4::from_translation(off);
    let nrm = |v: f32, lo: f32, hi: f32| if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
    let mut out = Vec::new();

    // The realized top-down map at ground level — the "you are here" reference.
    let realized = layers::build_sheet(0.0, |i, j| s.realized(i, j).0, |i, j| s.realized(i, j).1, |_, _| true);
    out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], m));

    // Surface relief, seated at each column's true normalized layer in the
    // terrain third — lowlands sit low, peaks high, all inside the middle third.
    let surface = layers::build_sheet(
        0.0,
        |i, j| layer_py(s.surface_layer(j * layers::G + i)),
        |i, j| s.realized(i, j).1,
        |_, _| true,
    );
    out.extend(mk(renderer, surface, [1.0, 1.0, 1.0, 0.95], m));

    // Per-band temperature profile: one flat heatmap at each band's altitude,
    // on a shared ramp so the column reads as one gradient — hot molten floor,
    // through the surface, lapse-cooling air, the ozone inversion, the top swing.
    let (btlo, bthi) = layers::minmax(&s.band_temp);
    for b in 0..layers::BANDS {
        let y = layer_py(layers::band_mid(b));
        let temp = layers::build_sheet(
            y,
            |_, _| 0.0,
            |i, j| layers::pack_ramp(layers::M_ICE, layers::M_LAVA, nrm(layers::band_at(&s.band_temp, i, j, b), btlo, bthi)),
            |_, _| true,
        );
        out.extend(mk(renderer, temp, [1.0, 1.0, 1.0, 0.5], m));
    }

    // Air-band moisture: a heatmap in each atmosphere-zone band, offset above
    // the temperature sheet so the two don't z-fight.
    let (bmlo, bmhi) = layers::minmax(&s.band_moisture);
    for b in 6..layers::BANDS {
        let y = layer_py(layers::band_mid(b)) + 4.0;
        let moist = layers::build_sheet(
            y,
            |_, _| 0.0,
            |i, j| layers::pack_ramp(layers::M_FOAM, layers::M_WATER_DEEP, nrm(layers::band_at(&s.band_moisture, i, j, b), bmlo, bmhi)),
            |i, j| layers::band_at(&s.band_moisture, i, j, b) > 1e-4,
        );
        out.extend(mk(renderer, moist, [1.0, 1.0, 1.0, 0.5], m));
    }

    // Cloud deck (substance) in the mid-atmosphere band.
    let cloud = layers::build_sheet(
        layer_py(layers::band_mid(7)),
        |i, j| at(&s.cloud, i, j) * 6.0,
        |_, _| layers::M_CLOUD,
        |i, j| at(&s.cloud, i, j) > 0.15,
    );
    out.extend(mk(renderer, cloud, [1.0, 1.0, 1.0, 0.45], m));

    // Translucent hexagonal walls per band — the 9-band column the data sits in.
    let by = band_boundaries();
    for b in 0..layers::BANDS {
        let walls = hex_walls(place, by[b], by[b + 1], BAND_MAT[b]);
        out.extend(mk(renderer, walls, [1.0, 1.0, 1.0, 0.16], Mat4::IDENTITY));
    }

    let lx = -HEX_HALF_W - 55.0;
    let lbl = |layer: f32| Vec3::new(lx, layer_py(layer), 0.0) + off;
    let labels = vec![
        (Vec3::new(lx, 95.0, 0.0) + off, "realized", [1.0, 1.0, 1.0, 1.0]),
        (lbl(layers::band_mid(0)), "molten floor (GM)", [1.0, 0.5, 0.3, 1.0]),
        (lbl(42.0), "underground · veins / caves", [0.75, 0.6, 0.5, 1.0]),
        (lbl(127.0), "terrain (surface band)", [0.6, 0.85, 0.55, 1.0]),
        (lbl(213.0), "atmosphere", [0.55, 0.8, 1.0, 1.0]),
        (lbl(248.0), "thin air → space", [0.6, 0.95, 0.8, 1.0]),
    ];
    (out, labels)
}

/// Lua control logic for the view/sim HUD (`scripts/hex_ui.lua`).
const UI_SCRIPT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/hex_ui.lua");
/// Declarative HUD layout/state the Lua script reads as the `UI` global.
const UI_ELEMENTS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui_elements.json");
/// Default fly-camera pose (also the "Reset camera" target).
const CAM_HOME: (Vec3, f32, f32) = (Vec3::new(0.0, 1500.0, -2600.0), 0.0, -0.6);
/// Local-view camera: it looks down at the focus hex from `UP` above and `BACK`
/// south, so the focus + its ≤6 neighbours frame the view (~3 hex diameters) —
/// the "render only the local seven tiles" mode. At true scale one hex is
/// ≈49.6 mi; the geometry stays at display scale (the literal world-unit rescale
/// is deferred — it needs a camera/precision retune), the scale is surfaced in
/// the HUD.
const LOCAL_CAM_UP: f32 = 720.0;
const LOCAL_CAM_BACK: f32 = 540.0;
const LOCAL_CAM_PITCH: f32 = -0.72;

struct World {
    tiles: Vec<HexTile>,
    /// Per-tile: does this hex keep animating after the startup settle?
    animated: Vec<bool>,
    /// Frozen backdrop, built once after settling.
    static_sheets: Vec<Sheet>,
    /// The live ring(s), rebuilt on the cadence.
    animated_sheets: Vec<Sheet>,
    /// Static reference graticule (latitude rings + meridians), per colour group.
    graticule: Vec<LineGroup>,
    sim_accum: f32,
    rebuild_accum: f32,
    dirty: bool,
    // Selection + its in-place split-out stack.
    selected: Option<usize>,
    inspect_sheets: Vec<Sheet>,
    inspect_labels: Vec<Label>,
    inspect_dirty: bool,
    // Fly camera.
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    prev_mouse: Vec2,
    dragging: bool,
    // Lua-driven view/sim HUD (surface = flicker-ui, control = hex_ui.lua,
    // state = ui_elements.json).
    script: Option<ScriptHost>,
    white: Option<TextureHandle>,
    sim_paused: bool,
    sim_speed: f32,
    show_graticule: bool,
    show_billboards: bool,
    view_mode: u32,
    /// Pointer is over the HUD this frame — suppress the world hex-pick.
    ui_capture: bool,
    // Local view: the focus hex (= `selected`) + its ≤6 neighbours, culled from
    // the whole-world graph (`map`) and framed by a close camera.
    map: HexMap,
    local_sheets: Vec<Sheet>,
    local_dirty: bool,
    /// World-view camera pose, saved while in Local view so it restores on exit.
    saved_cam: Option<(Vec3, f32, f32)>,
    // Epoch 1 geology stack: per-hex dominant-element colours, the reusable
    // surface plane + flat hex mesh, and the view toggle / layer isolation.
    hex_colors: Vec<[f32; 4]>,
    stack_surface: Vec<Sheet>,
    epoch_hex_mesh: Option<MeshHandle>,
    show_stack: bool,
    solo_layer: Option<usize>,
    prev_stack_toggle: bool,
    prev_solo_up: bool,
    prev_solo_down: bool,
}

impl World {
    fn new() -> Self {
        let map = HexMap::new(RINGS);
        let mut tiles = Vec::with_capacity(map.total() as usize);
        let mut animated = Vec::with_capacity(map.total() as usize);
        for i in 0..map.total() {
            let c = map.coord(i);
            let place = hex_flat_pos(c);
            tiles.push(HexTile {
                place,
                stack: LayerStack::generate(place),
            });
            animated.push(c.hemi == Hemisphere::North && c.ring <= ANIMATE_RINGS);
        }

        // Epoch 1: seed a composition per hex from its point on the sphere and
        // keep the dominant element's colour for the geology map. If the
        // vocabulary can't load, fall back to grey so the app still runs.
        let hex_colors = match Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)) {
            Ok(tables) => {
                let epoch = Epoch1::new(&tables, Epoch1Params::default(), GEO_SEED);
                (0..map.total())
                    .map(|i| {
                        let d = map.celestial_dir(i);
                        hex_color(&epoch.seed_hex(Vec3::new(d[0], d[1], d[2])))
                    })
                    .collect()
            }
            Err(e) => {
                tracing::error!("Epoch 1 seed failed ({e}); geology map will be grey");
                vec![[0.5, 0.5, 0.5, 1.0]; map.total() as usize]
            }
        };

        Self {
            tiles,
            animated,
            static_sheets: Vec::new(),
            animated_sheets: Vec::new(),
            graticule: build_graticule(),
            sim_accum: 0.0,
            rebuild_accum: REBUILD_INTERVAL,
            dirty: true,
            selected: None,
            inspect_sheets: Vec::new(),
            inspect_labels: Vec::new(),
            inspect_dirty: false,
            pos: CAM_HOME.0,
            yaw: CAM_HOME.1,
            pitch: CAM_HOME.2,
            prev_mouse: Vec2::ZERO,
            dragging: false,
            script: None,
            white: None,
            sim_paused: false,
            sim_speed: 1.0,
            show_graticule: true,
            show_billboards: true,
            view_mode: 1,
            ui_capture: false,
            map,
            local_sheets: Vec::new(),
            local_dirty: false,
            saved_cam: None,
            hex_colors,
            stack_surface: Vec::new(),
            epoch_hex_mesh: None,
            show_stack: false,
            solo_layer: None,
            prev_stack_toggle: false,
            prev_solo_up: false,
            prev_solo_down: false,
        }
    }

    fn local_view(&self) -> bool {
        self.view_mode == 2
    }

    /// The focus hex (`selected`) + its ≤6 graph neighbours — the ≤7 tiles the
    /// Local view renders. Empty if nothing is focused.
    fn local_set(&self) -> Vec<usize> {
        match self.selected {
            Some(f) => {
                let mut v: Vec<usize> =
                    self.map.neighbours(f as u32).into_iter().map(|j| j as usize).collect();
                v.push(f);
                v
            }
            None => Vec::new(),
        }
    }

    /// Tile whose centre is nearest the camera (the default Local focus).
    fn nearest_tile(&self) -> Option<usize> {
        let cam = Vec2::new(self.pos.x, self.pos.z);
        (0..self.tiles.len()).min_by(|&a, &b| {
            let da = (self.tiles[a].place - cam).length_squared();
            let db = (self.tiles[b].place - cam).length_squared();
            da.total_cmp(&db)
        })
    }

    /// Switch view mode, snapping the camera into / out of the local frame. The
    /// local meshes are (re)built and freed in `render_map` (where `&mut Renderer`
    /// is available); this only moves state.
    fn set_view_mode(&mut self, mode: u32) {
        if mode == 2 && self.view_mode != 2 {
            // → Local: save the overview camera, pick a focus if none, frame it.
            self.saved_cam = Some((self.pos, self.yaw, self.pitch));
            if self.selected.is_none() {
                self.selected = self.nearest_tile();
            }
            if let Some(f) = self.selected {
                (self.pos, self.yaw, self.pitch) = local_cam_pose(self.tiles[f].place);
            }
            self.local_dirty = true;
        } else if mode == 1 && self.view_mode == 2 {
            // → World: restore the overview camera.
            if let Some(saved) = self.saved_cam.take() {
                (self.pos, self.yaw, self.pitch) = saved;
            }
        }
        self.inspect_dirty = true; // rebuild (World) or clear (Local) the column
        self.view_mode = mode;
    }

    /// Cycle the isolated stack layer: `None` (show all) ↔ `0..GEO_PLANES`. `dir`
    /// is +1 (down toward the Epoch 1 plane) or −1 (up toward the surface).
    fn solo_step(&mut self, dir: i32) {
        let cur = self.solo_layer.map_or(-1, |l| l as i32);
        let next = (cur + dir).clamp(-1, GEO_PLANES as i32 - 1);
        self.solo_layer = (next >= 0).then_some(next as usize);
    }

    /// Draw the exploded geology stack: the surface biome plane, its duplicate
    /// placeholder planes (the same meshes at descending Y), and the Epoch 1
    /// dominant-element map at the bottom — honoring the isolated-layer filter.
    fn draw_stack(&self, renderer: &mut Renderer) {
        let visible = |l: usize| self.solo_layer.is_none_or(|s| s == l);
        for l in 0..GEO_EPOCH_PLANE {
            if !visible(l) {
                continue;
            }
            let shift = Mat4::from_translation(Vec3::new(0.0, -(l as f32) * GEO_SPACING, 0.0));
            for s in &self.stack_surface {
                renderer.draw_mesh(s.handle, shift * s.model, MeshDrawOptions { wireframe: false, tint: s.tint });
            }
        }
        // Epoch 1 plane: one flat hex per tile, tinted by its dominant element.
        if visible(GEO_EPOCH_PLANE) {
            if let Some(mesh) = self.epoch_hex_mesh {
                let y = -(GEO_EPOCH_PLANE as f32) * GEO_SPACING;
                for (t, &tint) in self.tiles.iter().zip(&self.hex_colors) {
                    let model = Mat4::from_translation(Vec3::new(t.place.x, y, t.place.y));
                    renderer.draw_mesh(mesh, model, MeshDrawOptions { wireframe: false, tint });
                }
            }
        }
    }

    /// The engine state the HUD script reads each frame (the `Model` global).
    fn ui_model(&self) -> ValueMap {
        ValueMap::new()
            .with("hex_count", self.tiles.len())
            .with("rings", RINGS)
            .with("pos_x", self.pos.x)
            .with("pos_y", self.pos.y)
            .with("pos_z", self.pos.z)
            .with("yaw", self.yaw)
            .with("pitch", self.pitch)
            .with("selected", self.selected.map_or(-1.0, |i| i as f64))
            .with("sim_paused", self.sim_paused)
            .with("sim_speed", self.sim_speed)
            .with("view_mode", self.view_mode)
            .with("sim_time", self.tiles.first().map_or(0.0, |t| t.stack.time))
            .with("local_tiles", if self.local_view() { self.local_set().len() } else { 0 })
    }

    /// Run the Lua HUD's `update`: feed input, apply the returned control values
    /// to engine state. Called before `update_map` so `ui_capture` can gate the
    /// world-pick.
    fn run_ui(&mut self, input: &InputState, r: &Renderer) {
        let size = r.size();
        // Get the results out first, releasing the `self.script` borrow before we
        // mutate self (set_view_mode needs `&mut self`).
        let res = match self.script.as_ref() {
            Some(s) => match s.update(input, size.x, size.y) {
                Ok(res) => res,
                Err(e) => {
                    tracing::error!("hex-world UI update failed: {e}");
                    return;
                }
            },
            None => return,
        };
        self.sim_paused = res.is_on("sim_paused");
        self.show_graticule = res.is_on("graticule");
        self.show_billboards = res.is_on("billboards");
        if let Some(v) = res.number("sim_speed") {
            self.sim_speed = v as f32;
        }
        if let Some(v) = res.number("view_mode") {
            let mode = v as u32;
            if mode != self.view_mode {
                self.set_view_mode(mode);
            }
        }
        self.ui_capture = res.is_on("ui_capture");
        if res.is_on("reset_camera") {
            (self.pos, self.yaw, self.pitch) = CAM_HOME;
        }
    }

    fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, cy * cp)
    }

    fn map_camera(&self) -> Camera {
        Camera {
            position: self.pos,
            target: self.pos + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 30000.0,
        }
    }

    /// Rebuild only the live ring's meshes (the frozen backdrop is left alone).
    fn rebuild_animated(&mut self, renderer: &mut Renderer) {
        for s in std::mem::take(&mut self.animated_sheets) {
            renderer.free_mesh(s.handle);
        }
        self.animated_sheets = build_subset(&self.tiles, &self.animated, true, renderer);
        self.dirty = false;
    }

    fn update_map(&mut self, dt: f32, input: &InputState, r: &Renderer) {
        // Look: hold right mouse, drag to pitch/yaw.
        if input.mouse_right {
            if self.dragging {
                let d = input.mouse_position - self.prev_mouse;
                self.yaw -= d.x * LOOK_SENS;
                self.pitch = (self.pitch - d.y * LOOK_SENS).clamp(-1.5, 1.5);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = input.mouse_position;

        // Geology stack: '\' toggles the exploded layer view; Up/Down isolate a
        // single layer (cycling all → surface → … → Epoch 1). Edge-detected so a
        // held key fires once.
        let toggle = input.key_down(Key::Backslash);
        if toggle && !self.prev_stack_toggle {
            self.show_stack = !self.show_stack;
            println!("geology stack: {}", if self.show_stack { "on" } else { "off" });
        }
        self.prev_stack_toggle = toggle;
        let up = input.key_down(Key::Up);
        if up && !self.prev_solo_up {
            self.solo_step(-1);
        }
        self.prev_solo_up = up;
        let down = input.key_down(Key::Down);
        if down && !self.prev_solo_down {
            self.solo_step(1);
        }
        self.prev_solo_down = down;

        // Left-click: select the hex under the cursor (click it again, or empty
        // space, to deselect). Its stack splits out in place. Suppressed when the
        // pointer is over the HUD (`ui_capture`), so clicking a control doesn't
        // also pick a hex behind it.
        if input.mouse_left_pressed && !self.ui_capture {
            let picked = pick_hex(&self.map_camera(), input.mouse_position, r.size(), &self.tiles);
            if self.local_view() {
                // Re-focus the local set on a clicked tile; never deselect (the
                // Local view always keeps a focus). Camera stays where it is.
                if let Some(i) = picked {
                    if self.selected != Some(i) {
                        self.selected = Some(i);
                        self.local_dirty = true;
                    }
                }
            } else {
                self.selected = if picked == self.selected { None } else { picked };
                self.inspect_dirty = true;
            }
        }

        // Move: 6-DOF fly.
        let fwd = self.forward();
        let (sy, cy) = self.yaw.sin_cos();
        let right = Vec3::new(-cy, 0.0, sy);
        let speed = MOVE_SPEED * dt;
        let mut v = Vec3::ZERO;
        if input.key_down(Key::W) {
            v += fwd;
        }
        if input.key_down(Key::S) {
            v -= fwd;
        }
        if input.key_down(Key::D) {
            v += right;
        }
        if input.key_down(Key::A) {
            v -= right;
        }
        if input.key_down(Key::R) {
            v += Vec3::Y;
        }
        if input.key_down(Key::F) {
            v -= Vec3::Y;
        }
        self.pos += v * speed;

        // Sim the live ring + the selected hex, at fixed step (capped). The HUD
        // pauses it and scales its rate (sim_speed); the fixed SIM_DT is kept so
        // each tick is deterministic — speed changes how much sim-time elapses.
        if !self.sim_paused {
            self.sim_accum += dt * self.sim_speed;
        }
        let mut steps = 0;
        while self.sim_accum >= SIM_DT && steps < 4 {
            for idx in 0..self.tiles.len() {
                if self.animated[idx] || Some(idx) == self.selected {
                    self.tiles[idx].stack.tick(SIM_DT);
                }
            }
            self.sim_accum -= SIM_DT;
            steps += 1;
        }
        if self.sim_accum > SIM_DT {
            self.sim_accum = 0.0;
        }

        self.rebuild_accum += dt;
        if self.rebuild_accum >= REBUILD_INTERVAL {
            self.rebuild_accum = 0.0;
            self.dirty = true;
            self.local_dirty = true;
            if self.selected.is_some() {
                self.inspect_dirty = true;
            }
        }
    }

    fn render_map(&mut self, renderer: &mut Renderer) {
        let local = self.local_view();

        // Mesh build: World rebuilds the live ring on its cadence; Local builds
        // the focus set (≤7 tiles) and frees it again when we leave Local.
        if local {
            if self.local_dirty {
                for s in std::mem::take(&mut self.local_sheets) {
                    renderer.free_mesh(s.handle);
                }
                self.local_sheets = build_tiles(&self.tiles, &self.local_set(), renderer);
                self.local_dirty = false;
            }
        } else {
            if !self.local_sheets.is_empty() {
                for s in std::mem::take(&mut self.local_sheets) {
                    renderer.free_mesh(s.handle);
                }
            }
            if self.dirty {
                self.rebuild_animated(renderer);
            }
        }

        // The split-out inspector column is a World-view tool; in Local view the
        // selection is just the focus, so the column is cleared, not built.
        if self.inspect_dirty {
            for s in std::mem::take(&mut self.inspect_sheets) {
                renderer.free_mesh(s.handle);
            }
            self.inspect_labels.clear();
            if !local {
                if let Some(i) = self.selected {
                    let (sheets, labels) = build_inspect(&self.tiles[i].stack, self.tiles[i].place, renderer);
                    self.inspect_sheets = sheets;
                    self.inspect_labels = labels;
                }
            }
            self.inspect_dirty = false;
        }

        let camera = self.map_camera();
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.4, 0.9, 0.35).normalize(),
            ..SceneLighting::default()
        });
        renderer.draw_sky();

        // Surface meshes: the Local focus set, the exploded geology stack, or the
        // flat whole-graph world.
        if local {
            for s in &self.local_sheets {
                renderer.draw_mesh(s.handle, s.model, MeshDrawOptions { wireframe: false, tint: s.tint });
            }
        } else if self.show_stack {
            self.draw_stack(renderer);
        } else {
            for s in self.static_sheets.iter().chain(&self.animated_sheets).chain(&self.inspect_sheets) {
                renderer.draw_mesh(s.handle, s.model, MeshDrawOptions { wireframe: false, tint: s.tint });
            }
        }

        // Planet-overview overlays (graticule + the focus column) — flat World
        // only; the exploded stack hides them.
        if !local && !self.show_stack {
            if self.show_graticule {
                for (segs, color) in &self.graticule {
                    renderer.draw_lines(segs, *color);
                }
            }
            if let Some(i) = self.selected {
                renderer.draw_lines(&column_lines(self.tiles[i].place), [0.6, 0.9, 1.0, 0.8]);
            }
        }

        let size = renderer.size();
        let vp = camera.view_projection(size.x / size.y);
        if self.show_billboards && !self.show_stack {
            renderer.set_layer(50.0);
            let labels: Vec<usize> = if local {
                self.local_set()
            } else {
                (0..self.tiles.len()).collect()
            };
            for i in labels {
                let t = &self.tiles[i];
                let world = Vec3::new(t.place.x, BILLBOARD_Y, t.place.y);
                if let Some(p) = project_to_screen(vp, world, size) {
                    let dist = (world - self.pos).length();
                    let px = (28_000.0 / dist).clamp(7.0, 16.0);
                    renderer.draw_text(&i.to_string(), p, px, [1.0, 0.95, 0.35, 1.0]);
                }
            }
        }
        // Labels for the split-out stack (World only; empty in Local).
        for &(pos, text, color) in &self.inspect_labels {
            if let Some(p) = project_to_screen(vp, pos, size) {
                renderer.draw_text(text, p, 16.0, color);
            }
        }

        // Lua-driven view/sim HUD, on top of everything (surface = flicker-ui's
        // render bridge, control = hex_ui.lua, state = ui_elements.json).
        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            if let Err(e) = script.set_model(&self.ui_model()) {
                tracing::error!("hex-world UI model publish failed: {e}");
            }
            renderer.set_layer(100.0);
            match script.draw(size.x, size.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::error!("hex-world UI draw failed: {e}"),
            }
        }
    }
}

impl Scene for World {
    fn enter(&mut self, renderer: &mut Renderer) {
        let live = self.animated.iter().filter(|&&a| a).count();
        println!(
            "HexWorld flat graph: {} hexes (R={RINGS}); settling, {live} animate live. Fly: WASD, R/F up/down, RMB look, Esc.",
            self.tiles.len()
        );
        println!(
            "Geology stack: '\\' toggles the exploded layer stack (Epoch 1 map at the bottom); Up/Down isolate one layer."
        );
        // Settle the whole world once so the frozen backdrop looks alive.
        for _ in 0..SETTLE_TICKS {
            for t in &mut self.tiles {
                t.stack.tick(SIM_DT);
            }
        }
        // Build the frozen backdrop now; the live ring is built on first render.
        self.static_sheets = build_subset(&self.tiles, &self.animated, false, renderer);

        // Geology-stack meshes: the settled biome surface (reused at every
        // stacked layer) and one flat near-white hex tinted per-draw for the
        // Epoch 1 plane.
        self.stack_surface = build_stack_surface(&self.tiles, renderer);
        let (fv, fi) = layers::build_sheet(0.0, |_, _| 0.0, |_, _| layers::M_FOAM, |_, _| true);
        self.epoch_hex_mesh = Some(renderer.upload_mesh(&fv, MeshIndices::U32(&fi)));

        // Load the Lua view/sim HUD: control logic + the embedded widget toolkit
        // + the declarative layout, then seed the model so frame 1's `update`
        // reads sane values.
        match ScriptHost::from_file(UI_SCRIPT_PATH) {
            Ok(s) => {
                load_ui_json(&s, UI_ELEMENTS_PATH);
                load_widgets(&s);
                let _ = s.set_model(&self.ui_model());
                self.script = Some(s);
            }
            Err(e) => tracing::error!("hex-world UI script load failed: {e}"),
        }
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
    }

    fn update(&mut self, dt: Duration, input: &InputState, r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        self.run_ui(input, r);
        self.update_map(dt.as_secs_f32(), input, r);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        self.render_map(renderer);
    }
}

/// Project a world point to screen pixels; `None` if behind the camera.
fn project_to_screen(vp: Mat4, world: Vec3, size: Vec2) -> Option<Vec2> {
    let clip = vp * world.extend(1.0);
    if clip.w <= 0.001 {
        return None;
    }
    Some(Vec2::new(
        (clip.x / clip.w * 0.5 + 0.5) * size.x,
        (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * size.y,
    ))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(World::new())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout_tessellates_without_overlap() {
        let map = HexMap::new(RINGS);
        let places: Vec<Vec2> = (0..map.total()).map(|i| hex_flat_pos(map.coord(i))).collect();
        // Adjacent hex centres are ~2·HEX_HALF_W apart; any closer means two
        // hexes collapsed onto the same spot (a ring-walk bug).
        for a in 0..places.len() {
            for b in a + 1..places.len() {
                let d = (places[a] - places[b]).length();
                assert!(d > HEX_HALF_W, "hexes {a},{b} overlap (d = {d:.1})");
            }
        }
    }

    #[test]
    fn hemispheres_interlock_at_the_equator() {
        // The closest north↔south hexes should be ~one hex pitch apart: touching
        // and interlocking (teeth), neither overlapping nor leaving a gap.
        let map = HexMap::new(RINGS);
        let mut min_cross = f32::INFINITY;
        for a in 0..map.total() {
            for b in 0..map.total() {
                if map.coord(a).hemi != map.coord(b).hemi {
                    let d = (hex_flat_pos(map.coord(a)) - hex_flat_pos(map.coord(b))).length();
                    min_cross = min_cross.min(d);
                }
            }
        }
        let pitch = 2.0 * HEX_HALF_W; // one hex centre-to-centre
        assert!(
            min_cross > HEX_HALF_W && min_cross < 1.3 * pitch,
            "equator join distance {min_cross:.1} (pitch {pitch:.1}) — gapped or overlapping"
        );
    }

    #[test]
    fn each_ring_is_full_and_separated_by_hemisphere() {
        let map = HexMap::new(RINGS);
        let (mut north, mut south) = (0u32, 0u32);
        for i in 0..map.total() {
            let c = map.coord(i);
            match c.hemi {
                Hemisphere::North => north += 1,
                Hemisphere::South => south += 1,
            }
        }
        // Symmetric hemispheres; total = 2 + 6·R·(R+1).
        assert_eq!(north, south);
        assert_eq!(north + south, 2 + 6 * RINGS * (RINGS + 1));
    }

    // Loads the real hex_ui.lua + ui_elements.json + the flicker-ui widget
    // toolkit and runs a frame headlessly — so a malformed layout, a broken
    // script, or a name the script reads but the data/model lacks fails the
    // build (the boundary-validation pattern from docs/ui.md).
    #[test]
    fn ui_script_runs_a_frame() {
        let s = ScriptHost::from_file(UI_SCRIPT_PATH).expect("hex_ui.lua loads");
        load_ui_json(&s, UI_ELEMENTS_PATH);
        load_widgets(&s);
        let model = ValueMap::new()
            .with("hex_count", 74u32)
            .with("rings", 3u32)
            .with("pos_x", 0.0f32)
            .with("pos_y", 0.0f32)
            .with("pos_z", 0.0f32)
            .with("yaw", 0.0f32)
            .with("pitch", 0.0f32)
            .with("selected", -1.0f64)
            .with("sim_paused", false)
            .with("sim_speed", 1.0f32)
            .with("view_mode", 1u32)
            .with("sim_time", 0.0f32);
        s.set_model(&model).expect("set_model");

        // Click the first checkbox ("Pause simulation", origin [16,152], box 18).
        let mut input = InputState::new();
        input.mouse_position = Vec2::new(25.0, 160.0);
        input.mouse_left_pressed = true;
        let res = s.update(&input, 960.0, 540.0).expect("update runs");
        assert!(res.is_on("sim_paused"), "clicking the checkbox toggles sim_paused");
        // Seeded overlays read on; pointer over the HUD captures the click.
        assert!(res.is_on("graticule") && res.is_on("billboards"));
        assert!(res.is_on("ui_capture"), "pointer over the HUD suppresses world-pick");

        let cmds = s.draw(960.0, 540.0).expect("draw runs");
        assert!(!cmds.is_empty(), "HUD should emit draw commands");
    }

    // The Local view culls to the focus hex + its ≤6 graph neighbours and snaps
    // the camera into / out of a close local frame. (No renderer needed — this
    // is the cull/focus/camera state logic.)
    #[test]
    fn local_view_focuses_the_seven_tiles() {
        let mut w = World::new();
        assert!(!w.local_view());

        // Entering Local with no selection picks the nearest tile as the focus
        // and saves the overview camera.
        w.set_view_mode(2);
        assert!(w.local_view());
        assert!(w.saved_cam.is_some(), "Local saves the overview camera");
        let focus = w.selected.expect("Local view picks a focus");

        // The rendered set is the focus + its neighbours (6 or 7 distinct tiles).
        let set = w.local_set();
        assert!(set.contains(&focus), "focus is in the local set");
        assert!((6..=7).contains(&set.len()), "local set has {} tiles", set.len());
        let mut uniq = set.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), set.len(), "local set has no duplicates");

        // Exiting Local restores the overview camera exactly.
        w.set_view_mode(1);
        assert!(!w.local_view());
        assert!(w.saved_cam.is_none());
        assert_eq!(w.pos, CAM_HOME.0, "exiting Local restores the overview camera");
    }
}
