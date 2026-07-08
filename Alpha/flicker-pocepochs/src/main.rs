//! flicker-pocepochs — a **single hex tile**, drawn as its full generation stack.
//!
//! A reference POC keeping two ideas alive: the **terrain / epoch layers** and the
//! **water-cycle + atmosphere stack** on one hex. (The old icosahedral / flat-map
//! topology and the specific map-based generation are gone — that technique is
//! superseded; only `layers.rs`'s water-cycle nucleus and the layered-stack idea
//! are kept.)
//!
//! One hex is shown as a vertical stack at real scale (1 unit = 1 cluster = 128 ft;
//! a hex is 2048 units ≈ 49.6 mi across), everything on, no toggles:
//! - **Six epoch planes** at the bottom — the world-gen epochs, each a per-cell
//!   **relief mesh** sampled from that epoch's `HexState` field, tinted by hardness
//!   + ore veins, the top plane by biome. Epoch 4 floods the basins with a sea.
//! - **Nine atmosphere band shells** above — the zone outlines, drawn translucent.
//!   The conservative water-cycle sim (`layers::LayerStack`) ticks underneath.
//!
//! The hex is generated for one seed and regenerated from the seed overlay. It's a
//! **flicker-shell client**: START launches this scene, Esc opens the pause menu.
//! Still POC.
//!
//! Fly: WASD move, R/F up/down, hold right mouse to look. Esc → pause.

mod layers;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{AbstractControls, Action, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    TextureHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, Transition};
use flicker_materials::{JsonTableSource, Tables};
use flicker_shell::{PauseScene, Theme};
use flicker_worldgen::{
    six_epoch_stack, Biome, CellSample, Epoch1, Epoch1Params, EpochCtx, FieldSampler, HexState,
    EPOCHS,
};
use flicker_worldstate::Composition;

use layers::{build_sheet, cell_local, pack_ramp, LayerStack, BANDS, BAND_BOUNDS, G, HEX_SIZE};

/// World seed for the Epoch 1 element distribution.
const SEED: u64 = 0x0EC0_DE01;
/// Materials vocabulary directory (the JSON tables), relative to this crate.
const MATERIALS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");

/// Fixed sim step; the water-cycle mechanics tick on it.
const SIM_DT: f32 = 0.05;
/// Ticks run at startup so the sim settles into a real state.
const SETTLE_TICKS: u32 = 50;

/// Vertical exaggeration of the atmosphere bands (1.0 = true altitude).
const VEXAG: f32 = 1.0;
/// Vertical gap between the **exploded** epoch planes, world units — an order of
/// magnitude larger than a band's real height so the six layers read distinctly.
const EPOCH_GAP: f32 = 640.0;
/// Translucency of the atmosphere band shells.
const BAND_ALPHA: f32 = 0.22;
/// Draw the 9 atmosphere band shells above the stack — always on (no toggle).
const SHOW_BANDS: bool = true;

/// Material index per band shell (bottom→top): the three zones — below (molten /
/// rock / soil), terrain (lowland / hill / alpine), atmosphere (lower air / cloud
/// deck / thin air). Colour only; the bands are drawn empty.
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

/// Default fly-camera pose framed on the single hex column: position, yaw, pitch.
const CAM_HOME: (Vec3, f32, f32) = (Vec3::new(0.0, -1400.0, -5200.0), 0.0, -0.12);
const MOVE_SPEED: f32 = 3000.0;
const LOOK_SENS: f32 = 0.005;

// Seed-control overlay (screen pixels): a seed readout + three regenerate buttons.
const UI_PANEL: (f32, f32, f32, f32) = (8.0, 8.0, 260.0, 92.0);
const BTN_NEW: (f32, f32, f32, f32) = (16.0, 54.0, 150.0, 28.0);
const BTN_DEC: (f32, f32, f32, f32) = (172.0, 54.0, 36.0, 28.0);
const BTN_INC: (f32, f32, f32, f32) = (214.0, 54.0, 36.0, 28.0);

/// Is `m` inside rect `(x, y, w, h)`?
fn hit(r: (f32, f32, f32, f32), m: Vec2) -> bool {
    m.x >= r.0 && m.x <= r.0 + r.2 && m.y >= r.1 && m.y <= r.1 + r.3
}

/// Draw a labelled overlay button, highlighted when the pointer is over it.
fn ui_button(renderer: &mut Renderer, white: TextureHandle, r: (f32, f32, f32, f32), label: &str, hover: bool) {
    let bg = if hover {
        [0.28, 0.34, 0.46, 0.95]
    } else {
        [0.16, 0.18, 0.24, 0.95]
    };
    renderer.draw_sprite(white, Vec2::new(r.0, r.1), Vec2::new(r.2, r.3), bg);
    let tw = renderer.measure_text(label, 14.0).x;
    renderer.draw_text(label, Vec2::new(r.0 + (r.2 - tw) * 0.5, r.1 + 7.0), 14.0, [0.95, 0.96, 1.0, 1.0]);
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

/// Vein intensity above which a cell is drawn as ore rather than rock.
const VEIN_SHOW: f32 = 0.08;

/// The ore-vein palette colour for the metal a vein carries.
fn ore_material(vein_element: Option<u8>) -> u32 {
    match vein_element {
        Some(26) => layers::M_ORE_IRON,   // Fe
        Some(29) => layers::M_ORE_COPPER, // Cu
        Some(79) => layers::M_ORE_GOLD,   // Au
        Some(47) => layers::M_ORE_SILVER, // Ag
        _ => layers::M_ORE_OTHER,
    }
}

/// Per-cell material word for a relief mesh: bare rock shaded by **hardness**
/// (soft/dark → hard/pale), and cells on an ore **vein** tinted toward the metal
/// by the vein intensity. Packed as a two-stop ramp the mesh shader resolves.
fn cell_material(c: &CellSample, vein_element: Option<u8>) -> u32 {
    if c.vein > VEIN_SHOW {
        pack_ramp(layers::M_ROCK_HARD, ore_material(vein_element), c.vein.clamp(0.0, 1.0))
    } else {
        let hard = (c.hardness / 10.0).clamp(0.0, 1.0);
        pack_ramp(layers::M_ROCK_SOFT, layers::M_ROCK_HARD, hard)
    }
}

/// World-unit span over which the Epoch-6 ground fades from biome colour to bare
/// rock / snow on the peaks.
const SNOW_SPAN: f32 = 150.0;

/// The biome's surface palette colour (Epoch 6).
fn biome_material(b: Biome) -> u32 {
    match b {
        Biome::Ocean => layers::M_WATER_MID, // (the sea overlay covers it anyway)
        Biome::Ice => layers::M_ICE,
        Biome::Tundra => layers::M_TUNDRA,
        Biome::Taiga => layers::M_TAIGA,
        Biome::Grassland => layers::M_GRASSLAND,
        Biome::Forest => layers::M_FOREST,
        Biome::Rainforest => layers::M_RAINFOREST,
        Biome::Savanna => layers::M_SAVANNA,
        Biome::Desert => layers::M_DESERT,
        Biome::Alpine => layers::M_ROCK_HARD,
    }
}

/// Ground (Epoch 6) per-cell colour: the hex's biome at the lowlands, fading to
/// bare rock / snow on cells rising toward the peaks.
fn ground_material(c: &CellSample, biome: Biome, sea_world: f32) -> u32 {
    let snow_t = ((c.elevation - sea_world) / SNOW_SPAN).clamp(0.0, 1.0);
    pack_ramp(biome_material(biome), layers::M_ROCK_HARD, snow_t)
}

/// splitmix64 step — jump to a fresh, unrelated-looking seed (the "new seed"
/// button). Deterministic, so a given hex is always reproducible.
fn next_seed(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The six epoch layers for **one hex**. The old map topology is gone; the epoch
/// pipeline just needs *some* neighbour graph, so we run it over a minimal 7-cell
/// "flower" (a centre + a 6-ring) with symmetric adjacency and take the centre
/// cell's stack. Generation fidelity isn't the point here — the layered-stack
/// idea is. Changing the seed yields a different hex.
fn build_hex(tables: &Tables, seed: u64) -> Vec<HexState> {
    let e1 = Epoch1::new(tables, Epoch1Params::default(), seed);
    // Centre (0) borders all six ring cells; each ring cell borders the centre and
    // its two ring neighbours — a small, symmetric, valid neighbour graph.
    let neighbors: Vec<Vec<u32>> = vec![
        vec![1, 2, 3, 4, 5, 6],
        vec![0, 6, 2],
        vec![0, 1, 3],
        vec![0, 2, 4],
        vec![0, 3, 5],
        vec![0, 4, 6],
        vec![0, 5, 1],
    ];
    // Dummy celestial directions fanned around a mid-latitude — enough spatial
    // variation for Epoch 1's distribution without a real sphere.
    let dirs: Vec<Vec3> = (0..7)
        .map(|i| {
            let a = i as f32 * std::f32::consts::FRAC_PI_3;
            Vec3::new(0.3 * a.cos(), 0.9, 0.3 * a.sin()).normalize()
        })
        .collect();
    let ctx = EpochCtx { tables, dirs: &dirs, neighbors: &neighbors, seed };
    let stack = six_epoch_stack(&e1, &ctx); // [epoch][cell]
    (0..EPOCHS).map(|e| stack[e][0].clone()).collect() // the centre hex's six layers
}

/// One hex: its six epoch states and their per-epoch relief meshes, the Epoch-4
/// sea, and the ticking water-cycle sim.
struct Tile {
    stack: LayerStack,
    epoch_states: Vec<HexState>,
    relief: Vec<MeshHandle>,
    /// Flat sea surface over the submerged cells (Epoch 4+); `None` if dry.
    water: Option<MeshHandle>,
}

struct World {
    tile: Tile,
    /// The 9 atmosphere band shells, at their real altitudes, coloured per zone.
    band_shells: Vec<MeshHandle>,
    /// Vocabulary — kept for the life of the scene so the hex can be regenerated.
    tables: Option<Tables>,
    /// World seed driving the epoch stack; the overlay edits it.
    seed: u64,
    /// A seed change requested by the overlay, applied in `render` (which holds
    /// the `&mut Renderer` needed to re-upload meshes).
    pending_seed: Option<u64>,
    /// 1×1 white texture for the overlay panel/buttons.
    white: Option<TextureHandle>,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    prev_mouse: Vec2,
    dragging: bool,
    sim_accum: f32,
    /// Shell pause plumbing: bindings (Menu = Esc), a press-edge latch, and the
    /// gothic theme built once for the pause overlay we push.
    bindings: InputMap,
    menu_prev: bool,
    ui_theme: Option<Theme>,
}

impl World {
    fn new() -> Self {
        let tables = Tables::from_source(&JsonTableSource::new(MATERIALS_DIR)).ok();
        let epoch_states = match tables.as_ref() {
            Some(t) => build_hex(t, SEED),
            None => {
                tracing::error!("vocabulary failed to load; the stack will be empty");
                vec![HexState::new(Composition::new()); EPOCHS]
            }
        };
        Self {
            tile: Tile {
                stack: LayerStack::generate(Vec2::ZERO),
                epoch_states,
                relief: Vec::new(),
                water: None,
            },
            band_shells: Vec::new(),
            tables,
            seed: SEED,
            pending_seed: None,
            white: None,
            pos: CAM_HOME.0,
            yaw: CAM_HOME.1,
            pitch: CAM_HOME.2,
            prev_mouse: Vec2::ZERO,
            dragging: false,
            sim_accum: 0.0,
            bindings: InputMap::wasd_and_mouse(),
            menu_prev: false,
            ui_theme: None,
        }
    }

    /// Rebuild the hex's six per-epoch relief meshes + the Epoch-4 sea for `seed`,
    /// freeing the old ones first (so regeneration doesn't leak GPU buffers).
    fn rebuild_meshes(tile: &mut Tile, tables: &Tables, renderer: &mut Renderer, seed: u64) {
        let sampler = FieldSampler::new(tables, seed);
        for m in tile.relief.drain(..) {
            renderer.free_mesh(m);
        }
        if let Some(w) = tile.water.take() {
            renderer.free_mesh(w);
        }

        // One relief mesh per epoch — the per-cell field sampled straight from that
        // epoch's state (no cross-hex blend: there's no neighbour to seam with).
        tile.relief = (0..EPOCHS)
            .map(|e| {
                let state = &tile.epoch_states[e];
                let cells: Vec<CellSample> = (0..G * G)
                    .map(|k| {
                        let (lx, lz) = cell_local(k % G, k / G);
                        sampler.sample_blended(state, Vec2::new(lx, lz), state.elevation, state.orogeny)
                    })
                    .collect();
                let is_ground = e == EPOCHS - 1;
                let sea_world = state.sea_level * sampler.tectonic_scale;
                let (v, idx) = build_sheet(
                    0.0,
                    |i, j| cells[j * G + i].elevation,
                    |i, j| {
                        let c = &cells[j * G + i];
                        if is_ground {
                            ground_material(c, state.biome, sea_world)
                        } else {
                            cell_material(c, state.vein_element)
                        }
                    },
                    |_, _| true,
                );
                renderer.upload_mesh(&v, MeshIndices::U32(&idx))
            })
            .collect();

        let e4 = &tile.epoch_states[3];
        let sea = e4.sea_level * sampler.tectonic_scale;
        let (wv, wi) = build_sheet(
            sea,
            |_, _| 0.0,
            |_, _| layers::M_WATER_MID,
            |i, j| {
                let (lx, lz) = cell_local(i, j);
                sampler
                    .sample_blended(e4, Vec2::new(lx, lz), e4.elevation, e4.orogeny)
                    .elevation
                    < sea
            },
        );
        tile.water = (!wi.is_empty()).then(|| renderer.upload_mesh(&wv, MeshIndices::U32(&wi)));
    }

    /// Regenerate the hex for `seed`: re-run the epoch stack and rebuild all meshes.
    fn regenerate(&mut self, renderer: &mut Renderer, seed: u64) {
        self.seed = seed;
        let Some(tables) = self.tables.as_ref() else {
            return;
        };
        self.tile.epoch_states = build_hex(tables, seed);
        Self::rebuild_meshes(&mut self.tile, tables, renderer, seed);
    }

    /// The seed-control overlay: the seed readout + three regenerate buttons.
    fn draw_overlay(&self, renderer: &mut Renderer) {
        let Some(white) = self.white else {
            return;
        };
        let m = self.prev_mouse;
        renderer.set_layer(100.0);
        renderer.draw_sprite(
            white,
            Vec2::new(UI_PANEL.0, UI_PANEL.1),
            Vec2::new(UI_PANEL.2, UI_PANEL.3),
            [0.0, 0.0, 0.0, 0.55],
        );
        renderer.draw_text("world seed — click to regenerate", Vec2::new(16.0, 14.0), 13.0, [0.85, 0.87, 0.95, 1.0]);
        renderer.draw_text(&format!("{:#010x}", self.seed), Vec2::new(16.0, 32.0), 18.0, [0.55, 0.9, 1.0, 1.0]);
        ui_button(renderer, white, BTN_NEW, "new seed", hit(BTN_NEW, m));
        ui_button(renderer, white, BTN_DEC, "-1", hit(BTN_DEC, m));
        ui_button(renderer, white, BTN_INC, "+1", hit(BTN_INC, m));
        renderer.set_layer(0.0);
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

    /// Fly camera + the water-cycle sim tick.
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

        // The water-cycle mechanics tick.
        self.sim_accum += dt;
        let mut steps = 0;
        while self.sim_accum >= SIM_DT && steps < 4 {
            self.tile.stack.tick(SIM_DT);
            self.sim_accum -= SIM_DT;
            steps += 1;
        }
        if self.sim_accum > SIM_DT {
            self.sim_accum = 0.0;
        }
    }

    fn draw_stack(&self, renderer: &mut Renderer) {
        let tile = &self.tile;
        if tile.relief.len() < EPOCHS {
            return;
        }
        // Six exploded epoch planes below 0 — Epoch 1 lowest, Epoch 6 nearest the
        // bands. Each is that epoch's own per-cell relief mesh; Epoch 4+ floods the
        // basins with a flat sea over the relief.
        for e in 0..EPOCHS {
            let y = -((EPOCHS - e) as f32) * EPOCH_GAP;
            let model = Mat4::from_translation(Vec3::new(0.0, y, 0.0));
            renderer.draw_mesh(
                tile.relief[e],
                model,
                MeshDrawOptions { tint: [1.0, 1.0, 1.0, 1.0], ..Default::default() },
            );
            if e >= 3 {
                if let Some(w) = tile.water {
                    renderer.draw_mesh(
                        w,
                        model,
                        MeshDrawOptions { tint: [1.0, 1.0, 1.0, 1.0], ..Default::default() },
                    );
                }
            }
        }
        // Nine atmosphere band shells at their real altitudes above the stack.
        if SHOW_BANDS {
            let model = Mat4::from_translation(Vec3::ZERO);
            let shell_tint = [1.0, 1.0, 1.0, BAND_ALPHA];
            for &shell in &self.band_shells {
                renderer.draw_mesh(shell, model, MeshDrawOptions { wireframe: false, tint: shell_tint, ..Default::default() });
            }
        }
    }
}

impl Scene for World {
    fn enter(&mut self, renderer: &mut Renderer) {
        // Settle the water-cycle sim so its state is real.
        for _ in 0..SETTLE_TICKS {
            self.tile.stack.tick(SIM_DT);
        }
        // Nine atmosphere band shells at their real altitudes.
        self.band_shells = (0..BANDS)
            .map(|b| {
                let y0 = BAND_BOUNDS[b] as f32 * VEXAG;
                let y1 = BAND_BOUNDS[b + 1] as f32 * VEXAG;
                let (v, i) = band_shell(y0, y1, BAND_MAT[b]);
                renderer.upload_mesh(&v, MeshIndices::U32(&i))
            })
            .collect();
        // A 1×1 white texture the seed overlay tints for its panel/buttons.
        self.white = Some(renderer.load_texture(&[255, 255, 255, 255], 1, 1));
        // Gothic theme for the shell pause overlay we push on Esc.
        self.ui_theme = Some(Theme::build(renderer));
        // Per-epoch relief + sea meshes for the current seed.
        if let Some(tables) = self.tables.as_ref() {
            Self::rebuild_meshes(&mut self.tile, tables, renderer, self.seed);
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        // Esc / Menu → push the shell pause overlay (edge-detected). The scene
        // manager freezes us while it's up, so the sim clock stops too.
        let menu_down = self.bindings.action_pressed(Action::Menu, input);
        let menu_pressed = menu_down && !self.menu_prev;
        self.menu_prev = menu_down;
        if menu_pressed {
            let theme = self.ui_theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &self.bindings,
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }

        // Overlay: left-click a seed button; queues a regenerate for `render`
        // (which holds the `&mut Renderer`).
        let m = input.mouse_position;
        if input.mouse_left_pressed {
            if hit(BTN_NEW, m) {
                self.pending_seed = Some(next_seed(self.seed));
            } else if hit(BTN_DEC, m) {
                self.pending_seed = Some(self.seed.wrapping_sub(1));
            } else if hit(BTN_INC, m) {
                self.pending_seed = Some(self.seed.wrapping_add(1));
            }
        }
        self.step(dt.as_secs_f32(), input);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(seed) = self.pending_seed.take() {
            self.regenerate(renderer, seed);
        }
        let camera = self.camera();
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.4, 0.9, 0.35).normalize(),
            ..SceneLighting::default()
        });
        renderer.draw_sky();
        self.draw_stack(renderer);
        self.draw_overlay(renderer);
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // The shell owns the front-end (splash → menu → settings/pause) + the run
    // loop; START launches our single-hex generation-stack scene.
    flicker_shell::run(flicker_shell::ShellConfig {
        game_scene: Box::new(|| Box::new(World::new())),
    })
}
