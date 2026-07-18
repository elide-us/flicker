//! The world-gen scene, stripped to **Epoch 1 (seed) + Epoch 2 (molten convection)**.
//!
//! Epoch-1 generation controls: **R** reseeds a fresh planet, **`[` / `]`** shrink / grow the
//! planet, **V** cycles the view (material → heat → layer stack; **Up** slices the stack
//! open), and the element distribution is read on the left. Epoch-2 sim controls: **Space** starts / stops the
//! convection run (each tick = one pass ≈ [`MY_PER_TICK`] My, the time for the crust to move
//! one hex), **Down** resets to the Epoch-1 seed. **Esc** opens the pause menu.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flicker::app::{AbstractControls, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, TextureHandle, Vec2,
};
use flicker::scene::{Scene, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_widgets, render_hud};
use flicker_shell::{PauseScene, Theme};
use flicker_worldengine::{observe, LayerKind, Simulation, MY_PER_TICK};

use crate::camera::OrbitCam;
use crate::globe::{self, ViewMode};

/// The life-supporting-conditions HUD (Lua, composed from the shared widget toolkit).
const HAB_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/habitability_hud.lua");

/// Default planet size (grid frequency, ~49.65 mi/hex). ½ Earth — snappy but planet-scale.
const PLANET_FREQ: u32 = 48;
/// Planet-size range + step for the `[` / `]` controls (grid frequency; 96 ≈ full Earth).
const SIZE_MIN: u32 = 12;
const SIZE_MAX: u32 = 96;
const SIZE_STEP: u32 = 6;
/// Sim ticks advanced per second while playing.
const PLAY_TICKS_PER_SEC: f32 = 6.0;

/// A fresh random base seed each launch (the Epoch-1 distribution differs per run); reset
/// returns to tick 0 of this same seed.
fn clock_seed() -> u64 {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub struct WorldScene {
    sim: Simulation,
    cam: OrbitCam,
    tick: u64,
    /// Current world seed (R rolls a new one).
    seed: u64,
    /// Current planet size (grid frequency; `[` / `]` change it).
    freq: u32,
    /// The seed's global element distribution: `(atomic number, symbol, percent)`, sorted
    /// descending, only the relevant (non-negligible) elements. Conserved across ticks, so it
    /// is a property of the Epoch-1 seed; recomputed on reseed / resize.
    element_dist: Vec<(u8, String, f32)>,
    meshes: Vec<MeshHandle>,
    dirty: bool,
    stats: String,
    theme: Option<Theme>,
    white: Option<TextureHandle>,
    /// Surface colouring (`V` cycles material → heat → layer stack).
    mode: ViewMode,
    /// Whether the layer stack is sliced open (`Up` toggles) — a wedge cut through the shells.
    cutaway: bool,
    playing: bool,
    play_accum: f32,
    /// The Lua HUD host for the life-supporting-conditions panel (the shared widget toolkit
    /// drives its gauges); `None` if the script failed to load (the rest of the HUD still draws).
    script: Option<ScriptHost>,
    prev_menu: bool,
    prev_play: bool,
    prev_down: bool,
    prev_view: bool,
    prev_cut: bool,
    prev_reseed: bool,
    prev_size_down: bool,
    prev_size_up: bool,
}

impl WorldScene {
    pub fn new() -> Self {
        let seed = clock_seed();
        let sim = Simulation::from_repo_seeded(PLANET_FREQ, seed)
            .expect("tick sim loads from Alpha/content/data");
        let mut scene = Self {
            sim,
            cam: OrbitCam::new(globe::RADIUS),
            tick: 0,
            seed,
            freq: PLANET_FREQ,
            element_dist: Vec::new(),
            meshes: Vec::new(),
            dirty: true,
            stats: String::new(),
            theme: None,
            white: None,
            mode: ViewMode::Material, // Epoch 1 = the material cloud
            cutaway: false,
            playing: false, // start paused at the Epoch-1 seed; Space begins Epoch 2
            play_accum: 0.0,
            script: None,
            prev_menu: false,
            prev_play: false,
            prev_down: false,
            prev_view: false,
            prev_cut: false,
            prev_reseed: false,
            prev_size_down: false,
            prev_size_up: false,
        };
        scene.element_dist = scene.compute_element_dist();
        scene.refresh();
        scene
    }

    /// Rebuild the sim at the current seed + size, back to the Epoch-1 seed, paused.
    fn rebuild(&mut self) {
        self.sim = Simulation::from_repo_seeded(self.freq, self.seed).expect("rebuild tick sim");
        self.tick = 0;
        self.playing = false;
        self.play_accum = 0.0;
        self.element_dist = self.compute_element_dist();
        self.refresh();
    }

    /// Reseed Epoch 1: roll a new random seed → a fresh planet (same size).
    fn reseed(&mut self) {
        self.seed = clock_seed();
        self.rebuild();
    }

    /// Grow / shrink the planet by `delta` steps of grid frequency (same seed).
    fn resize(&mut self, delta: i32) {
        let f = (self.freq as i32 + delta * SIZE_STEP as i32).clamp(SIZE_MIN as i32, SIZE_MAX as i32);
        if f as u32 != self.freq {
            self.freq = f as u32;
            self.rebuild();
        }
    }

    /// The seed's global element distribution — `(number, symbol, percent)` for the relevant
    /// (≥ 0.5 %) elements, sorted descending. Summed over the Epoch-1 seed cells; convection
    /// conserves element mass, so this is stable across ticks (an Epoch-1 property).
    fn compute_element_dist(&mut self) -> Vec<(u8, String, f32)> {
        self.sim.ensure(0);
        let cells = &self.sim.world(0).expect("tick-0 seed").cells;
        let tables = self.sim.tables();
        let mut totals: std::collections::BTreeMap<u8, f64> = std::collections::BTreeMap::new();
        for c in cells {
            for (el, amt) in c.composition.iter() {
                *totals.entry(el).or_insert(0.0) += amt;
            }
        }
        let total: f64 = totals.values().sum();
        if total <= 0.0 {
            return Vec::new();
        }
        let mut list: Vec<(u8, String, f32)> = totals
            .into_iter()
            .map(|(el, amt)| {
                let sym = tables
                    .element_by_number(el)
                    .map(|e| e.symbol.clone())
                    .unwrap_or_else(|| el.to_string());
                (el, sym, (amt / total * 100.0) as f32)
            })
            .filter(|(_, _, pct)| *pct >= 0.5) // the relevant (non-defaulted) elements
            .collect();
        list.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        list
    }

    /// Move the shown tick (re-simming + refreshing if it changed).
    fn go_to_tick(&mut self, tick: u64) {
        if tick != self.tick {
            self.tick = tick;
            self.refresh();
        }
    }

    /// Ensure the current tick is computed, cache the readout, flag the mesh for rebuild.
    fn refresh(&mut self) {
        self.sim.ensure(self.tick);
        // The readout is the condition state, not an epoch number: the cooling clock `T` and
        // which layers have emerged from it across the planet.
        let stats = {
            let w = self.sim.world(self.tick).expect("ensured this tick");
            let n = w.cells.len();
            let count = |k: LayerKind| w.cells.iter().filter(|c| c.column.find(k).is_some()).count();
            format!(
                "tick {}  ·  {:.0} My  ·  T {:.0} K  ·  {} cells  ·  core {} · crust {} · ocean {} · atm {}",
                self.tick,
                self.tick as f32 * MY_PER_TICK,
                w.temp,
                n,
                count(LayerKind::Core),
                count(LayerKind::Crust),
                count(LayerKind::Ocean),
                count(LayerKind::Atmosphere),
            )
        };
        self.stats = stats;
        self.dirty = true;
    }

    /// Publish the life-supporting observer's reading of the current world into the HUD
    /// `Model` — one flat set of scalars per condition axis (name, position, band, in/out,
    /// live), plus the aggregate verdict. Pure read; the observer encodes no causal rule.
    fn habitability_model(&self) -> ValueMap {
        let mut m = ValueMap::new();
        if let Some(w) = self.sim.world(self.tick) {
            let h = observe(w);
            for (i, ax) in h.axes.iter().enumerate() {
                let n = i + 1;
                m = m
                    .with(format!("a{n}_name"), ax.name)
                    .with(format!("a{n}_v"), ax.signal.unwrap_or(-1.0)) // −1 = no signal yet
                    .with(format!("a{n}_lo"), ax.lo)
                    .with(format!("a{n}_hi"), ax.hi)
                    .with(format!("a{n}_lolab"), ax.low_label)
                    .with(format!("a{n}_hilab"), ax.high_label)
                    .with(format!("a{n}_in"), ax.in_band());
            }
            m = m
                .with("axes_total", h.axes.len())
                .with("axes_in", h.axes_in_band)
                .with("axes_live", h.axes_live)
                .with("life", h.life_supporting)
                .with("atm_kind", h.atmosphere_kind);
        }
        m
    }

    /// Advance the convection run while playing ([`PLAY_TICKS_PER_SEC`] ticks/sec).
    fn advance_play(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        self.play_accum += dt * PLAY_TICKS_PER_SEC;
        if self.play_accum < 1.0 {
            return;
        }
        let steps = self.play_accum.floor() as u64;
        self.play_accum -= steps as f32;
        self.go_to_tick(self.tick + steps);
    }

    /// Release every uploaded shell mesh.
    fn free_meshes(&mut self, renderer: &mut Renderer) {
        for h in self.meshes.drain(..) {
            renderer.free_mesh(h);
        }
    }
}

impl Default for WorldScene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for WorldScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0]; // deep space
        let theme = Theme::build(renderer);
        self.white = Some(theme.lua_textures()[0].1); // id 0 = "white"
        self.theme = Some(theme);
        // The life-supporting-conditions panel is a Lua HUD over the shared widget toolkit.
        match ScriptHost::from_file(HAB_SCRIPT) {
            Ok(script) => {
                load_widgets(&script);
                self.script = Some(script);
            }
            Err(e) => tracing::warn!("habitability HUD load failed ({HAB_SCRIPT}): {e}"),
        }
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.free_meshes(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        // Esc → pause menu (edge-detected).
        let menu = input.key_down(Key::Escape);
        if menu && !self.prev_menu {
            self.prev_menu = menu;
            let theme = self.theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &InputMap::default(),
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }
        self.prev_menu = menu;

        self.cam.update(input, true);

        let play = input.key_down(Key::Space);
        let down = input.key_down(Key::Down);
        let view = input.key_down(Key::V);
        let reseed = input.key_down(Key::R);
        let size_down = input.key_down(Key::LeftBracket);
        let size_up = input.key_down(Key::RightBracket);
        let cut_key = input.key_down(Key::Up);
        // Space: start / stop the Epoch-2 convection run.
        if play && !self.prev_play {
            self.playing = !self.playing;
            self.play_accum = 0.0;
        }
        // Down: reset to the Epoch-1 seed.
        if down && !self.prev_down {
            self.playing = false;
            self.go_to_tick(0);
        }
        // R: reseed Epoch 1 — a fresh random planet.
        if reseed && !self.prev_reseed {
            self.reseed();
        }
        // [ / ]: shrink / grow the planet (Epoch-1 size).
        if size_down && !self.prev_size_down {
            self.resize(-1);
        }
        if size_up && !self.prev_size_up {
            self.resize(1);
        }
        // V: cycle the surface view (material → heat → layer stack).
        if view && !self.prev_view {
            self.mode = globe::cycle_view(self.mode);
            self.dirty = true;
        }
        // Up: slice the layer stack open (a wedge cutaway through the shells).
        if cut_key && !self.prev_cut {
            self.cutaway = !self.cutaway;
            self.dirty = true;
        }
        self.prev_play = play;
        self.prev_down = down;
        self.prev_view = view;
        self.prev_cut = cut_key;
        self.prev_reseed = reseed;
        self.prev_size_down = size_down;
        self.prev_size_up = size_up;

        self.advance_play(dt.as_secs_f32());
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Rebuild the shell meshes from the current tick's world: one sparse shell per layer
        // for the layer stack, a single surface shell otherwise — the poc-chemistry approach.
        if self.dirty {
            self.free_meshes(renderer);
            self.sim.ensure(self.tick);
            let mode = self.mode;
            let cut = self.cutaway;
            let world = self.sim.world(self.tick).expect("ensured this tick");
            let tables = self.sim.tables();
            let built: Vec<(Vec<MeshVertex>, Vec<u32>)> = {
                let cells = &world.cells;
                let dirs = &self.sim.sphere().dirs;
                let outlines = self.sim.outlines();
                let mut b: Vec<(Vec<MeshVertex>, Vec<u32>)> = Vec::new();
                match mode {
                    ViewMode::Material => b.push(globe::build_shell(dirs, outlines, |_| globe::RADIUS, |i| {
                        Some(globe::material_color(&cells[i]))
                    })),
                    ViewMode::Heat => b.push(globe::build_shell(dirs, outlines, |_| globe::RADIUS, |i| {
                        Some(globe::cell_heat_color(&cells[i]))
                    })),
                    ViewMode::Layers => {
                        // Each cell's layers are CLASSIFIED (composition + temp + pressure → what
                        // they ARE) and stacked OUTWARD at their PHYSICAL thickness (volume =
                        // mass ÷ density). The mantle IS the base ball (core = data inside it,
                        // never a nested sphere). The cutaway drops a wedge from the outer layers.
                        let stacks: Vec<Vec<globe::StackLayer>> =
                            cells.iter().map(|c| globe::cell_stack(c, tables)).collect();
                        for kind in [
                            LayerKind::Mantle,
                            LayerKind::Crust,
                            LayerKind::Ocean,
                            LayerKind::Atmosphere,
                        ] {
                            let shell = globe::build_shell(
                                dirs,
                                outlines,
                                |i| {
                                    stacks[i]
                                        .iter()
                                        .find(|s| s.kind == kind)
                                        .map(|s| s.outer_r)
                                        .unwrap_or(globe::RADIUS)
                                },
                                |i| {
                                    if cut && kind != LayerKind::Mantle && globe::in_wedge(dirs[i]) {
                                        return None;
                                    }
                                    stacks[i].iter().find(|s| s.kind == kind).map(|s| s.color)
                                },
                            );
                            if !shell.1.is_empty() {
                                b.push(shell);
                            }
                        }
                    }
                }
                b
            };
            self.meshes = built
                .into_iter()
                .map(|(v, i)| renderer.upload_mesh(&v, MeshIndices::U32(&i)))
                .collect();
            self.dirty = false;
        }

        renderer.set_camera(&self.cam.camera());
        for &h in &self.meshes {
            renderer.draw_mesh(h, Mat4::IDENTITY, MeshDrawOptions::default());
        }

        // Stats + controls.
        renderer.set_layer(10.0);
        let gold = [0.83, 0.67, 0.39, 1.0];
        let text = [0.85, 0.87, 0.92, 1.0];
        let dim = [0.6, 0.63, 0.68, 1.0];
        renderer.draw_text("FLICKER · PLANET SIMULATION", Vec2::new(24.0, 24.0), 26.0, gold);
        renderer.draw_text(&self.stats, Vec2::new(24.0, 60.0), 17.0, text);
        let (word, col) = if self.playing {
            ("PLAYING", [0.55, 0.85, 0.55, 1.0])
        } else {
            ("PAUSED", [0.92, 0.78, 0.42, 1.0])
        };
        renderer.draw_text(word, Vec2::new(24.0, 88.0), 14.0, col);
        let cut_hint = if self.mode == ViewMode::Layers {
            format!("  ·  Up slice: {}", if self.cutaway { "on" } else { "off" })
        } else {
            String::new()
        };
        renderer.draw_text(
            &format!(
                "·  Space play/pause  ·  Down reset  ·  R reseed  ·  [ ] size  ·  V view: {}{}  ·  drag · wheel · Esc",
                self.mode.label(),
                cut_hint,
            ),
            Vec2::new(96.0, 88.0),
            14.0,
            dim,
        );

        // Surface-view legend (top-right) — display only, no controls.
        if let Some(white) = self.white {
            let entries = globe::legend_entries(self.mode);
            let (pad, sw, row_h, panel_w) = (10.0f32, 14.0f32, 20.0f32, 210.0f32);
            let panel_h = pad + 22.0 + entries.len() as f32 * row_h + pad;
            let px = renderer.size().x - panel_w - 16.0;
            let py = 20.0;
            renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.85]);
            let title = self.mode.label().to_uppercase();
            renderer.draw_text(&title, Vec2::new(px + pad, py + pad), 14.0, gold);
            let mut ry = py + pad + 24.0;
            for (label, c) in &entries {
                renderer.draw_sprite(white, Vec2::new(px + pad, ry + 2.0), Vec2::new(sw, sw), [c[0], c[1], c[2], 1.0]);
                renderer.draw_text(label, Vec2::new(px + pad + sw + 8.0, ry), 13.0, text);
                ry += row_h;
            }
        }

        // Element-distribution readout (left) — the Epoch-1 seed composition (relevant,
        // non-default elements), each with its material swatch + share.
        if let Some(white) = self.white {
            if !self.element_dist.is_empty() {
                let (pad, sw, row_h, panel_w) = (10.0f32, 12.0f32, 18.0f32, 200.0f32);
                let panel_h = pad + 22.0 + self.element_dist.len() as f32 * row_h + pad;
                let (px, py) = (16.0f32, 120.0f32);
                renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.85]);
                renderer.draw_text("ELEMENT DISTRIBUTION", Vec2::new(px + pad, py + pad), 14.0, gold);
                let mut ry = py + pad + 24.0;
                for (num, sym, pct) in &self.element_dist {
                    let c = globe::element_rgb(*num);
                    renderer.draw_sprite(white, Vec2::new(px + pad, ry + 2.0), Vec2::new(sw, sw), [c[0], c[1], c[2], 1.0]);
                    renderer.draw_text(&format!("{sym}   {pct:.1}%"), Vec2::new(px + pad + sw + 8.0, ry), 13.0, text);
                    ry += row_h;
                }
            }
        }

        // Life-supporting-conditions panel (Lua HUD over the shared widget toolkit): the
        // five condition gauges + the aggregate verdict, read live from the observer.
        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            let _ = script.set_model(&self.habitability_model());
            let size = renderer.size();
            match script.draw(size.x, size.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::warn!("habitability HUD draw error: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_epoch2_convection_and_resets_to_epoch1() {
        let mut scene = WorldScene::new();
        assert!(!scene.playing, "starts paused at the Epoch-1 seed");
        assert_eq!(scene.tick, 0);

        // Play advances the tick — Epoch-2 convection runs.
        scene.playing = true;
        scene.advance_play(5.0);
        assert!(scene.tick > 0, "playback advanced the convection sim");

        // Reset returns to the Epoch-1 seed.
        scene.go_to_tick(0);
        assert_eq!(scene.tick, 0, "reset to Epoch 1");
    }

    #[test]
    fn habitability_hud_loads_and_draws_from_the_observer_model() {
        // The panel's Lua loads against the real shared widget toolkit, and the observer
        // model built from the live world drives a draw without error — catches Lua/contract
        // breakage headlessly (no window).
        let script = ScriptHost::from_file(HAB_SCRIPT).expect("habitability_hud.lua loads");
        load_widgets(&script);
        let scene = WorldScene::new();
        script.set_model(&scene.habitability_model()).expect("publish observer model");
        let cmds = script.draw(1280.0, 720.0).expect("panel draws");
        assert!(!cmds.is_empty(), "the panel emitted draw commands");
    }

    #[test]
    fn epoch1_element_distribution_is_populated_and_sensible() {
        let scene = WorldScene::new();
        let dist = &scene.element_dist;
        assert!(!dist.is_empty(), "the Epoch-1 element distribution readout is empty");
        // Sorted descending, every share is a real percentage, and the total is bounded.
        for w in dist.windows(2) {
            assert!(w[0].2 >= w[1].2, "distribution not sorted descending");
        }
        let sum: f32 = dist.iter().map(|(_, _, p)| p).sum();
        assert!(sum > 50.0 && sum <= 100.5, "shares should cover most of the planet, got {sum:.1}%");
    }

    #[test]
    fn resize_changes_the_planet_and_reseed_changes_the_composition() {
        let mut scene = WorldScene::new();
        let f0 = scene.freq;
        scene.resize(-1); // shrink
        assert!(scene.freq < f0, "resize should shrink the planet");
        assert_eq!(scene.tick, 0, "resize returns to the Epoch-1 seed");
        let seed0 = scene.seed;
        scene.reseed();
        assert_ne!(scene.seed, seed0, "reseed rolls a new seed");
        assert_eq!(scene.freq, f0 - SIZE_STEP, "reseed keeps the current planet size");
    }
}
