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
    Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};

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

// Inspect (split-view) exploded-stack heights, bottom→top.
const PY_GROUND: f32 = 240.0;
const PY_TEMP: f32 = 360.0;
const PY_HUMID: f32 = 470.0;
const PY_WATER: f32 = 590.0;
const PY_CLOUD: f32 = 710.0;
const PY_STRATO: f32 = 830.0;
const PY_THERMO: f32 = 950.0;
/// The exploded layer stack floats up from here, lined up over the selected hex.
const EXPLODE_BASE: f32 = 110.0;
/// Translucent wall material per layer band (bottom→top), tinting the column.
const BAND_MAT: [u32; 8] = [
    layers::M_ICE,
    layers::M_LAND,
    layers::M_LAVA,
    layers::M_WATER_SHALLOW,
    layers::M_WATER_MID,
    layers::M_CLOUD,
    layers::M_UV,
    layers::M_AURORA,
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

/// The 9 y-levels bounding the 8 column bands: the map surface, then each
/// exploded layer height. Derived so it tracks the `PY_*` constants.
fn band_boundaries() -> [f32; 9] {
    [
        0.0,
        EXPLODE_BASE,
        EXPLODE_BASE + PY_GROUND,
        EXPLODE_BASE + PY_TEMP,
        EXPLODE_BASE + PY_HUMID,
        EXPLODE_BASE + PY_WATER,
        EXPLODE_BASE + PY_CLOUD,
        EXPLODE_BASE + PY_STRATO,
        EXPLODE_BASE + PY_THERMO,
    ]
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

/// Build the exploded layer stack + labels for one hex, **lined up in the world**
/// over `place` (rising from [`EXPLODE_BASE`]). Every layer is shown explicitly:
/// substance as relief, influence fields and atmospheric bands as flat heatmaps.
fn build_inspect(s: &LayerStack, place: Vec2, renderer: &mut Renderer) -> (Vec<Sheet>, Vec<Label>) {
    let off = Vec3::new(place.x, EXPLODE_BASE, place.y);
    let m = Mat4::from_translation(off);
    let nrm = |v: f32, lo: f32, hi: f32| if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
    let mut out = Vec::new();

    let realized = layers::build_sheet(0.0, |i, j| s.realized(i, j).0, |i, j| s.realized(i, j).1, |_, _| true);
    out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], m));

    let snow = s.snow_line;
    let ground = layers::build_sheet(
        PY_GROUND,
        |i, j| (at(&s.ground, i, j) - 128.0) * layers::VSCALE,
        |i, j| if at(&s.ground, i, j) > snow { layers::M_STONE } else { layers::M_LAND },
        |_, _| true,
    );
    out.extend(mk(renderer, ground, [1.0, 1.0, 1.0, 0.7], m));

    let (tlo, thi) = layers::minmax(&s.temperature);
    let temp = layers::build_sheet(PY_TEMP, |_, _| 0.0, |i, j| layers::pack_ramp(layers::M_ICE, layers::M_LAVA, nrm(at(&s.temperature, i, j), tlo, thi)), |_, _| true);
    out.extend(mk(renderer, temp, [1.0, 1.0, 1.0, 0.92], m));

    let (hlo, hhi) = layers::minmax(&s.humidity);
    let humid = layers::build_sheet(PY_HUMID, |_, _| 0.0, |i, j| layers::pack_ramp(layers::M_FOAM, layers::M_WATER_DEEP, nrm(at(&s.humidity, i, j), hlo, hhi)), |_, _| true);
    out.extend(mk(renderer, humid, [1.0, 1.0, 1.0, 0.92], m));

    let water = layers::build_sheet(
        PY_WATER,
        |i, j| at(&s.water, i, j) * layers::VSCALE,
        |i, j| {
            let d = at(&s.water, i, j);
            if d > 28.0 { layers::M_WATER_DEEP } else if d > 10.0 { layers::M_WATER_MID } else { layers::M_WATER_SHALLOW }
        },
        |i, j| at(&s.water, i, j) > 0.5,
    );
    out.extend(mk(renderer, water, [1.0, 1.0, 1.0, 0.6], m));

    let cloud = layers::build_sheet(PY_CLOUD, |i, j| at(&s.cloud, i, j) * 40.0, |_, _| layers::M_CLOUD, |i, j| at(&s.cloud, i, j) > 0.15);
    out.extend(mk(renderer, cloud, [1.0, 1.0, 1.0, 0.45], m));

    let (slo, shi) = layers::minmax(&s.stratosphere);
    let strato = layers::build_sheet(PY_STRATO, |_, _| 0.0, |i, j| layers::pack_ramp(layers::M_ICE, layers::M_UV, nrm(at(&s.stratosphere, i, j), slo, shi)), |_, _| true);
    out.extend(mk(renderer, strato, [1.0, 1.0, 1.0, 0.9], m));

    let (xlo, xhi) = layers::minmax(&s.thermosphere);
    let thermo = layers::build_sheet(PY_THERMO, |_, _| 0.0, |i, j| layers::pack_ramp(layers::M_VOID, layers::M_AURORA, nrm(at(&s.thermosphere, i, j), xlo, xhi)), |_, _| true);
    out.extend(mk(renderer, thermo, [1.0, 1.0, 1.0, 0.92], m));

    // Translucent hexagonal walls per band — the 3-D column the layers sit in.
    let by = band_boundaries();
    for band in 0..by.len() - 1 {
        let walls = hex_walls(place, by[band], by[band + 1], BAND_MAT[band]);
        out.extend(mk(renderer, walls, [1.0, 1.0, 1.0, 0.16], Mat4::IDENTITY));
    }

    let lx = -HEX_HALF_W - 55.0;
    let labels = vec![
        (Vec3::new(lx, 95.0, 0.0) + off, "realized", [1.0, 1.0, 1.0, 1.0]),
        (Vec3::new(lx, PY_GROUND, 0.0) + off, "ground", [0.6, 0.8, 0.5, 1.0]),
        (Vec3::new(lx, PY_TEMP, 0.0) + off, "temperature", [1.0, 0.6, 0.3, 1.0]),
        (Vec3::new(lx, PY_HUMID, 0.0) + off, "humidity", [0.5, 0.7, 1.0, 1.0]),
        (Vec3::new(lx, PY_WATER, 0.0) + off, "water", [0.4, 0.7, 1.0, 1.0]),
        (Vec3::new(lx, PY_CLOUD, 0.0) + off, "cloud", [0.85, 0.87, 0.9, 1.0]),
        (Vec3::new(lx, PY_STRATO, 0.0) + off, "stratosphere (ozone)", [0.7, 0.5, 0.95, 1.0]),
        (Vec3::new(lx, PY_THERMO, 0.0) + off, "thermosphere", [0.3, 0.95, 0.6, 1.0]),
    ];
    (out, labels)
}

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
            pos: Vec3::new(0.0, 1500.0, -2600.0),
            yaw: 0.0,
            pitch: -0.6,
            prev_mouse: Vec2::ZERO,
            dragging: false,
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

        // Left-click: select the hex under the cursor (click it again, or empty
        // space, to deselect). Its stack splits out in place.
        if input.mouse_left_pressed {
            let picked = pick_hex(&self.map_camera(), input.mouse_position, r.size(), &self.tiles);
            self.selected = if picked == self.selected { None } else { picked };
            self.inspect_dirty = true;
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

        // Sim the live ring + the selected hex, at fixed step (capped).
        self.sim_accum += dt;
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
            if self.selected.is_some() {
                self.inspect_dirty = true;
            }
        }
    }

    fn render_map(&mut self, renderer: &mut Renderer) {
        if self.dirty {
            self.rebuild_animated(renderer);
        }
        // Rebuild the selected hex's split-out stack, lined up over its tile.
        if self.inspect_dirty {
            for s in std::mem::take(&mut self.inspect_sheets) {
                renderer.free_mesh(s.handle);
            }
            self.inspect_labels.clear();
            if let Some(i) = self.selected {
                let (sheets, labels) = build_inspect(&self.tiles[i].stack, self.tiles[i].place, renderer);
                self.inspect_sheets = sheets;
                self.inspect_labels = labels;
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
        for s in self.static_sheets.iter().chain(&self.animated_sheets).chain(&self.inspect_sheets) {
            renderer.draw_mesh(s.handle, s.model, MeshDrawOptions { wireframe: false, tint: s.tint });
        }
        for (segs, color) in &self.graticule {
            renderer.draw_lines(segs, *color);
        }
        if let Some(i) = self.selected {
            renderer.draw_lines(&column_lines(self.tiles[i].place), [0.6, 0.9, 1.0, 0.8]);
        }

        let size = renderer.size();
        let vp = camera.view_projection(size.x / size.y);
        renderer.set_layer(50.0);
        for (i, t) in self.tiles.iter().enumerate() {
            let world = Vec3::new(t.place.x, BILLBOARD_Y, t.place.y);
            if let Some(p) = project_to_screen(vp, world, size) {
                let dist = (world - self.pos).length();
                let px = (28_000.0 / dist).clamp(7.0, 16.0);
                renderer.draw_text(&i.to_string(), p, px, [1.0, 0.95, 0.35, 1.0]);
            }
        }
        // Labels for the split-out stack.
        for &(pos, text, color) in &self.inspect_labels {
            if let Some(p) = project_to_screen(vp, pos, size) {
                renderer.draw_text(text, p, 16.0, color);
            }
        }
        let hud = match self.selected {
            Some(i) => format!("hex {i} split out · click it again to close · WASD/RF fly · RMB look · Esc"),
            None => "WASD/RF fly · RMB look · left-click a hex to split out its layers · Esc quit".to_string(),
        };
        renderer.draw_text(&hud, Vec2::new(12.0, 12.0), 18.0, [1.0, 1.0, 1.0, 0.8]);
    }
}

impl Scene for World {
    fn enter(&mut self, renderer: &mut Renderer) {
        let live = self.animated.iter().filter(|&&a| a).count();
        println!(
            "HexWorld flat graph: {} hexes (R={RINGS}); settling, {live} animate live. Fly: WASD, R/F up/down, RMB look, Esc.",
            self.tiles.len()
        );
        // Settle the whole world once so the frozen backdrop looks alive.
        for _ in 0..SETTLE_TICKS {
            for t in &mut self.tiles {
                t.stack.tick(SIM_DT);
            }
        }
        // Build the frozen backdrop now; the live ring is built on first render.
        self.static_sheets = build_subset(&self.tiles, &self.animated, false, renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
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
}
