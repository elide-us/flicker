//! HexWorld — the world-data **stack visualization** (a data view, not gameplay).
//!
//! Each hex is drawn as a vertical stack at **real scale** — one world unit =
//! one cluster = 128 ft, so a hex is 2048 units (≈49.6 mi) across and the column
//! is 256 units (≈6.2 mi) tall:
//!
//! - **Six epoch planes** at the bottom — the world-gen epochs, each a per-cell
//!   **relief mesh** sampled from that epoch's `HexState` field (the spatial
//!   hardness distribution → terrain: hard rock ridges, soft valleys), tinted by
//!   its dominant element. Epoch 1 composition, Epoch 2 differentiated crust,
//!   Epoch 3 plate-driven elevation; Epochs 4-6 copy Epoch 3 for now.
//! - **Nine surface-simulation bands** stacked on top — the existing band model
//!   drawn as colour-coded **empty shells**. The water-cycle sim still ticks
//!   underneath (its mechanics — heat, convection, precipitation — are kept); we
//!   simply no longer draw water/lava/ice as heightmaps, nor any band content.
//!   The sim's ground reads Epoch 6.
//!
//! Fly: WASD move, R/F up/down, hold right mouse to look, Esc quits.

mod layers;
#[allow(dead_code)] // adjacency/celestial kept; only layout + celestial_dir used here.
mod topology;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker_materials::{JsonTableSource, Tables};
use flicker_worldgen::{
    six_epoch_stack, Epoch1, Epoch1Params, EpochCtx, FieldSampler, HexState, EPOCHS,
};
use flicker_worldstate::Composition;

use layers::{build_sheet, cell_local, LayerStack, BANDS, BAND_BOUNDS, HEX_HALF_W, HEX_SIZE};
use topology::{HexCoord, HexMap, Hemisphere};

/// Rings per hemisphere (R=3 → 74 hexes).
const RINGS: u32 = 3;
/// World seed for the Epoch 1 element distribution.
const SEED: u64 = 0x0EC0_DE01;
/// Materials vocabulary directory (the JSON tables), relative to this example.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");

/// Fixed sim step; the kept water-cycle mechanics still tick (undrawn).
const SIM_DT: f32 = 0.05;
/// Ticks run on every hex at startup so the kept sim settles into a real state.
const SETTLE_TICKS: u32 = 50;
/// Hexes within this many rings of the north pole keep ticking after settle.
const ANIMATE_RINGS: u32 = 1;

/// Vertical exaggeration of the stack. `1.0` is true scale (a 256-unit column
/// under a 2048-wide hex — a thin slab); raise it to read the bands while flying.
const VEXAG: f32 = 1.0;
/// Vertical gap between the **exploded** epoch planes, world units. Deliberately
/// an order of magnitude larger than a band's real height (~28 u) so the six
/// epoch layers are easy to see and compare as their transforms diverge them.
/// Only the epoch stack is exploded — the sim bands stay at true altitude.
const EPOCH_GAP: f32 = 640.0;
/// Translucency of the (empty) band shells.
const BAND_ALPHA: f32 = 0.22;

// Material index per band shell (bottom→top): the three zones — below (molten /
// rock / soil), terrain (lowland / hill / alpine), atmosphere (lower air / cloud
// deck / thin air). Colour only; the bands are drawn empty.
const BAND_MAT: [u32; BANDS] = [
    layers::M_LAVA,
    layers::M_STONE,
    layers::M_LAND,
    layers::M_GRASSLAND,
    layers::M_FOREST,
    layers::M_TUNDRA,
    layers::M_WATER_SHALLOW,
    layers::M_UV,
    layers::M_AURORA,
];

/// Default fly-camera pose, retuned for real scale (the R=3 world spans ~±10k
/// units): position, yaw, pitch.
const CAM_HOME: (Vec3, f32, f32) = (Vec3::new(0.0, 9000.0, -17000.0), 0.0, -0.45);
const MOVE_SPEED: f32 = 11000.0;
const LOOK_SENS: f32 = 0.005;

/// Flat XZ offset of `pos` within `ring` (flat-top axial basis: NE and N).
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
    let aq = Vec2::new(1.5 * HEX_SIZE, HEX_HALF_W);
    let ar = Vec2::new(0.0, 2.0 * HEX_HALF_W);
    aq * cq as f32 + ar * cr as f32
}

/// Flat-layout centre of a hemisphere's disc — the two discs drawn side by side.
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

/// XZ of flat-top hex corner `k`, centred at the origin.
fn hex_corner(k: usize) -> (f32, f32) {
    let a = k as f32 * std::f32::consts::FRAC_PI_3;
    (HEX_SIZE * a.cos(), HEX_SIZE * a.sin())
}

/// The 6 side faces of an empty hexagonal-prism band `[y0, y1]` at the origin,
/// material `mat`. Double-sided so it reads from inside and out.
fn band_shell(y0: f32, y1: f32, mat: u32) -> (Vec<MeshVertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(24);
    let mut inds = Vec::with_capacity(72);
    for k in 0..6 {
        let (x0, z0) = hex_corner(k);
        let (x1, z1) = hex_corner((k + 1) % 6);
        let (mx, mz) = ((x0 + x1) * 0.5, (z0 + z1) * 0.5);
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

/// HSV→RGB, channels in `[0, 1]`.
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

/// A distinct colour per element — a golden-ratio hue by atomic number. (The
/// periodic table carries no colour; this is a debug palette.)
fn element_color(atomic_number: u8) -> [f32; 4] {
    let hue = (atomic_number as f32 * 0.618_034).fract();
    let [r, g, b] = hsv_to_rgb(hue, 0.62, 0.95);
    [r, g, b, 1.0]
}

/// Per-hex composition tint: the dominant element's colour, grey if empty. The
/// spatial structure now lives in the per-cell relief mesh; this tint is the
/// province colour drawn over it.
fn element_color_opt(dominant: Option<u8>) -> [f32; 4] {
    match dominant {
        Some(n) => element_color(n),
        None => [0.5, 0.5, 0.5, 1.0],
    }
}

/// One hex: its flat-layout position, its kept (undrawn) water-cycle sim, the six
/// epoch states (Epoch 1 first, Epoch 6 last), and the per-cell relief meshes
/// sampled from them — 3 distinct (Epoch 1/2/3); Epochs 4-6 reuse Epoch 3's.
struct Tile {
    place: Vec2,
    stack: LayerStack,
    epoch_states: Vec<HexState>,
    relief: Vec<MeshHandle>,
}

struct World {
    tiles: Vec<Tile>,
    /// Per-tile: does this hex keep ticking after the startup settle?
    animated: Vec<bool>,
    /// The 9 band shells, at their real altitudes, coloured per zone.
    band_shells: Vec<MeshHandle>,
    /// Vocabulary, kept until `enter` samples the per-cell fields, then dropped.
    tables: Option<Tables>,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    prev_mouse: Vec2,
    dragging: bool,
    sim_accum: f32,
}

impl World {
    fn new() -> Self {
        let map = HexMap::new(RINGS);
        let n = map.total() as usize;

        let tables = Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)).ok();

        // Per-hex unit-sphere direction → the six-epoch `HexState` stack. Each
        // tile keeps its six states; the per-cell fields are sampled from them in
        // `enter` (where the renderer is available).
        let stack: Vec<Vec<HexState>> = match &tables {
            Some(tables) => {
                let e1 = Epoch1::new(tables, Epoch1Params::default(), SEED);
                let dirs: Vec<Vec3> = (0..map.total())
                    .map(|i| {
                        let d = map.celestial_dir(i);
                        Vec3::new(d[0], d[1], d[2])
                    })
                    .collect();
                let neighbors: Vec<Vec<u32>> = (0..map.total()).map(|i| map.neighbours(i)).collect();
                let ctx = EpochCtx { tables, dirs: &dirs, neighbors: &neighbors, seed: SEED };
                six_epoch_stack(&e1, &ctx)
            }
            None => {
                tracing::error!("vocabulary failed to load; the stack will be empty");
                vec![vec![HexState::new(Composition::new()); n]; EPOCHS]
            }
        };

        let mut tiles = Vec::with_capacity(n);
        let mut animated = Vec::with_capacity(n);
        for i in 0..map.total() {
            let c = map.coord(i);
            let place = hex_flat_pos(c);
            let epoch_states = (0..EPOCHS).map(|e| stack[e][i as usize].clone()).collect();
            tiles.push(Tile {
                place,
                stack: LayerStack::generate(place),
                epoch_states,
                relief: Vec::new(),
            });
            animated.push(c.hemi == Hemisphere::North && c.ring <= ANIMATE_RINGS);
        }

        Self {
            tiles,
            animated,
            band_shells: Vec::new(),
            tables,
            pos: CAM_HOME.0,
            yaw: CAM_HOME.1,
            pitch: CAM_HOME.2,
            prev_mouse: Vec2::ZERO,
            dragging: false,
            sim_accum: 0.0,
        }
    }

    fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, cy * cp)
    }

    fn camera(&self) -> Camera {
        Camera {
            position: self.pos,
            target: self.pos + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 300_000.0,
        }
    }

    /// Fly camera + the kept sim's tick (undrawn).
    fn step(&mut self, dt: f32, input: &InputState) {
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

        // The kept water-cycle mechanics tick (undrawn) on the live ring.
        self.sim_accum += dt;
        let mut steps = 0;
        while self.sim_accum >= SIM_DT && steps < 4 {
            for (idx, tile) in self.tiles.iter_mut().enumerate() {
                if self.animated[idx] {
                    tile.stack.tick(SIM_DT);
                }
            }
            self.sim_accum -= SIM_DT;
            steps += 1;
        }
        if self.sim_accum > SIM_DT {
            self.sim_accum = 0.0;
        }
    }

    fn draw_stack(&self, renderer: &mut Renderer) {
        for tile in &self.tiles {
            if tile.relief.len() < 3 {
                continue;
            }
            // Six exploded epoch planes below 0 — Epoch 1 lowest, Epoch 6 nearest
            // the bands. Each is a per-cell relief mesh (hardness field → terrain),
            // tinted by the epoch's dominant element. Epochs 4-6 reuse Epoch 3's.
            for e in 0..EPOCHS {
                let mesh = tile.relief[e.min(2)];
                let tint = element_color_opt(tile.epoch_states[e].surface().dominant());
                let y = -((EPOCHS - e) as f32) * EPOCH_GAP;
                let model = Mat4::from_translation(Vec3::new(tile.place.x, y, tile.place.y));
                renderer.draw_mesh(mesh, model, MeshDrawOptions { wireframe: false, tint });
            }
            // Nine empty band shells at their real altitudes (0..Y_LAYERS).
            let model = Mat4::from_translation(Vec3::new(tile.place.x, 0.0, tile.place.y));
            let shell_tint = [1.0, 1.0, 1.0, BAND_ALPHA];
            for &shell in &self.band_shells {
                renderer.draw_mesh(shell, model, MeshDrawOptions { wireframe: false, tint: shell_tint });
            }
        }
    }
}

impl Scene for World {
    fn enter(&mut self, renderer: &mut Renderer) {
        let live = self.animated.iter().filter(|&&a| a).count();
        println!(
            "HexWorld stack viz: {} hexes (R={RINGS}), real scale (1 unit = 1 cluster = 128 ft). \
             {live} sim hexes tick live (undrawn). Fly: WASD, R/F up/down, RMB look, Esc.",
            self.tiles.len()
        );
        // Settle the kept sim so its (undrawn) state is real.
        for _ in 0..SETTLE_TICKS {
            for tile in &mut self.tiles {
                tile.stack.tick(SIM_DT);
            }
        }
        // Nine empty band shells at their real altitudes.
        self.band_shells = (0..BANDS)
            .map(|b| {
                let y0 = BAND_BOUNDS[b] as f32 * VEXAG;
                let y1 = BAND_BOUNDS[b + 1] as f32 * VEXAG;
                let (v, i) = band_shell(y0, y1, BAND_MAT[b]);
                renderer.upload_mesh(&v, MeshIndices::U32(&i))
            })
            .collect();
        // Per-tile relief meshes: sample each distinct epoch's per-cell field
        // (hardness → relief) into a resolved mesh. Epochs 4-6 reuse Epoch 3's.
        if let Some(tables) = self.tables.take() {
            let sampler = FieldSampler::new(&tables, SEED);
            for tile in self.tiles.iter_mut() {
                let place = tile.place;
                let relief: Vec<MeshHandle> = (0..3)
                    .map(|e| {
                        let state = &tile.epoch_states[e];
                        let (v, idx) = build_sheet(
                            0.0,
                            |i, j| {
                                let (lx, lz) = cell_local(i, j);
                                sampler.sample(state, place + Vec2::new(lx, lz)).elevation
                            },
                            |_, _| layers::M_FOAM,
                            |_, _| true,
                        );
                        renderer.upload_mesh(&v, MeshIndices::U32(&idx))
                    })
                    .collect();
                tile.relief = relief;
            }
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        self.step(dt.as_secs_f32(), input);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let camera = self.camera();
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.4, 0.9, 0.35).normalize(),
            ..SceneLighting::default()
        });
        renderer.draw_sky();
        self.draw_stack(renderer);
    }
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
        for a in 0..places.len() {
            for b in a + 1..places.len() {
                let d = (places[a] - places[b]).length();
                assert!(d > HEX_HALF_W, "hexes {a},{b} overlap (d = {d:.1})");
            }
        }
    }

    #[test]
    fn each_ring_is_full_and_separated_by_hemisphere() {
        let map = HexMap::new(RINGS);
        let (mut north, mut south) = (0u32, 0u32);
        for i in 0..map.total() {
            match map.coord(i).hemi {
                Hemisphere::North => north += 1,
                Hemisphere::South => south += 1,
            }
        }
        assert_eq!(north, south);
        assert_eq!(north + south, 2 + 6 * RINGS * (RINGS + 1));
    }

    #[test]
    fn epoch_stack_is_six_layers_per_hex() {
        let w = World::new();
        for tile in &w.tiles {
            assert_eq!(tile.epoch_states.len(), EPOCHS);
        }
    }
}
