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

const WORLD_OFFSET: Vec2 = Vec2::new(1234.0, 5678.0);
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
const WY_CLOUD: f32 = 120.0;
/// Height of the hex-index billboards — clear above the cloud deck.
const BILLBOARD_Y: f32 = 280.0;
/// Height of the reference graticule overlay (above clouds, below billboards).
const GRATICULE_Y: f32 = 210.0;
/// Meridian spokes per disc (Earth-style time-zone bands).
const TIME_ZONES: usize = 24;

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
    let cloud = layers::build_sheet(WY_CLOUD, |i, j| at(&s.cloud, i, j) * 40.0, |_, _| layers::M_CLOUD, |i, j| at(&s.cloud, i, j) > 0.15);
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
                stack: LayerStack::generate(WORLD_OFFSET + place),
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
            pos: Vec3::new(0.0, 2100.0, -3700.0),
            yaw: 0.0,
            pitch: -0.5,
            prev_mouse: Vec2::ZERO,
            dragging: false,
        }
    }

    fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, cy * cp)
    }

    /// Rebuild only the live ring's meshes (the frozen backdrop is left alone).
    fn rebuild_animated(&mut self, renderer: &mut Renderer) {
        for s in std::mem::take(&mut self.animated_sheets) {
            renderer.free_mesh(s.handle);
        }
        self.animated_sheets = build_subset(&self.tiles, &self.animated, true, renderer);
        self.dirty = false;
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

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let dt = dt.as_secs_f32();

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

        // Sim only the live ring at fixed step (capped so a hitch can't spiral).
        self.sim_accum += dt;
        let mut steps = 0;
        while self.sim_accum >= SIM_DT && steps < 4 {
            for (t, &live) in self.tiles.iter_mut().zip(self.animated.iter()) {
                if live {
                    t.stack.tick(SIM_DT);
                }
            }
            self.sim_accum -= SIM_DT;
            steps += 1;
        }
        if self.sim_accum > SIM_DT {
            self.sim_accum = 0.0;
        }

        // Rebuild meshes on a slower, decoupled cadence.
        self.rebuild_accum += dt;
        if self.rebuild_accum >= REBUILD_INTERVAL {
            self.rebuild_accum = 0.0;
            self.dirty = true;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if self.dirty {
            self.rebuild_animated(renderer);
        }

        let camera = Camera {
            position: self.pos,
            target: self.pos + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 30000.0,
        };
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.4, 0.9, 0.35).normalize(),
            ..SceneLighting::default()
        });
        renderer.draw_sky();
        for s in self.static_sheets.iter().chain(&self.animated_sheets) {
            renderer.draw_mesh(
                s.handle,
                s.model,
                MeshDrawOptions {
                    wireframe: false,
                    tint: s.tint,
                },
            );
        }

        // Reference graticule overlay: equator / tropics / polar circles + meridians.
        for (segs, color) in &self.graticule {
            renderer.draw_lines(segs, *color);
        }

        // Hex-index billboards, floating above the cloud layer (world-array
        // index per tile), distance-scaled so they stay readable.
        let size = renderer.size();
        let vp = camera.view_projection(size.x / size.y);
        renderer.set_layer(50.0);
        for (i, t) in self.tiles.iter().enumerate() {
            let world = Vec3::new(t.place.x, BILLBOARD_Y, t.place.y);
            if let Some(p) = project_to_screen(vp, world, size) {
                let dist = (world - self.pos).length();
                let px = (160_000.0 / dist).clamp(16.0, 96.0);
                renderer.draw_text(&i.to_string(), p, px, [1.0, 0.95, 0.35, 1.0]);
            }
        }

        renderer.draw_text(
            "WASD move · R/F up/down · hold RMB to look · Esc quit",
            Vec2::new(12.0, 12.0),
            18.0,
            [1.0, 1.0, 1.0, 0.8],
        );
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
