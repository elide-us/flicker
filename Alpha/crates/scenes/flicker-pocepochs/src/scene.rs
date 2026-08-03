//! The world-gen scene, stripped to **Epoch 1 (seed) + Epoch 2 (molten convection)**.
//!
//! Epoch-1 generation controls: **R** reseeds a fresh planet, **`[` / `]`** shrink / grow the
//! planet, **V** cycles the view (material → heat → layer stack; **Up** slices the stack
//! open), and the element distribution is read on the left. Epoch-2 sim controls: **Space** starts / stops the
//! convection run (each tick = one pass ≈ [`MY_PER_TICK`] My, the time for the crust to move
//! one hex), **Down** resets to the Epoch-1 seed. **Esc** opens the pause menu.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flicker_input_core::{
    AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState, Key,
};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, TextureHandle, Vec2,
};
use flicker::scene::{Scene, Transition};
use flicker::script::{ComponentLibrary, HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    load_styles, load_ui_json, render_hud, run_ui_with, UiInput, UiIntents, UiState, WalkerHandler,
    UI_COMPONENT_MODULES,
};
use flicker_input_core::{Fired, Resolver};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_worldengine::{observe, LayerKind, Simulation, MY_PER_TICK};

use crate::camera::OrbitCam;
use crate::globe::{self, ViewMode};
use crate::route::RootHandler;

/// The declarative HUD tree (`hud_pocepochs.lua`: readout text + the
/// life-supporting-conditions gauge panel) + the shared UI-element layout.
const HUD_SCRIPT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/scripts/hud_pocepochs.lua");
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_elements.json");

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
    /// The declarative HUD host (`hud_pocepochs.lua`), RETAINED as the Lua
    /// component library the walker dispatches per-node DRAW to; `None` if the
    /// script failed to load (the scene-drawn panels still draw).
    script: Option<ScriptHost>,
    /// The HUD's component tree, parsed ONCE at enter; walked every frame with
    /// fresh Model bindings.
    ui_tree: Option<UiNode>,
    /// The screen's declarative bindings (S9): `on_menu = "pause_open"`.
    ui_intents: UiIntents,
    /// Token-resolved `ui_elements.json` styles (dotted `style` paths resolve here).
    ui_styles: serde_json::Value,
    /// Retained walker interaction state.
    ui_state: UiState,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
    prev_play: bool,
    prev_down: bool,
    prev_view: bool,
    prev_cut: bool,
    prev_reseed: bool,
    prev_size_down: bool,
    prev_size_up: bool,

    /// Per-context action maps + active-context stack (spec §5/§9). This viewer only
    /// ever sits in `World`; its map is `wasd_and_mouse`, whose `Menu`→Escape binding is
    /// what the `Resolver` turns into the pause edge (the promoted `prev_menu`).
    bindings: ContextualBindings,
    gamepad_config: GamepadConfig,
    /// The input seam: a stateful edge `Resolver` (the promoted `prev_menu`), a REUSED
    /// `Fired` scratch buffer (no per-frame alloc), the router's request queue, and a
    /// monotonic per-frame `input_tick` — the resolver's `TickTime`, kept distinct from
    /// the simulation `tick` (which is not monotonic: reset returns it to 0).
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    input_tick: u64,
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
            ui_tree: None,
            ui_intents: UiIntents::default(),
            ui_styles: serde_json::Value::Object(Default::default()),
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            fired_sigs: Vec::new(),
            prev_play: false,
            prev_down: false,
            prev_view: false,
            prev_cut: false,
            prev_reseed: false,
            prev_size_down: false,
            prev_size_up: false,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            input_tick: 0,
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

    /// The per-frame HUD model: the readout lines (pre-formatted strings — Rust
    /// owns the formatting), the life-supporting observer's reading of the
    /// current world (per axis: the gauge value + name/status/caption binds and
    /// their colour paths), the aggregate verdict, and the transient
    /// `sig_<name>` mirror of last frame's fired intents. Pure read; the
    /// observer encodes no causal rule.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::new();

        // ── readout (top-left) ──
        m.set("stats", self.stats.as_str());
        let (word, color) = if self.playing {
            ("PLAYING", "pocepochs.playing.color")
        } else {
            ("PAUSED", "pocepochs.paused.color")
        };
        m.set("play_state", word);
        m.set("play_state_color", color);
        let cut_hint = if self.mode == ViewMode::Layers {
            format!("  ·  Up slice: {}", if self.cutaway { "on" } else { "off" })
        } else {
            String::new()
        };
        m.set(
            "hints",
            format!(
                "·  Space play/pause  ·  Down reset  ·  R reseed  ·  [ ] size  ·  V view: {}{}  ·  drag · wheel · Esc",
                self.mode.label(),
                cut_hint,
            ),
        );

        // ── life-supporting conditions (bottom-right) ──
        if let Some(w) = self.sim.world(self.tick) {
            let h = observe(w);
            for (i, ax) in h.axes.iter().enumerate() {
                let n = i + 1;
                let live = ax.signal.is_some();
                let (status, status_color) = if live {
                    if ax.in_band() {
                        ("in band", "pocepochs.hab.status_in")
                    } else {
                        ("out of band", "pocepochs.hab.status_out")
                    }
                } else {
                    ("no signal yet", "pocepochs.hab.status_dead")
                };
                m.set(format!("a{n}_name"), ax.name);
                m.set(
                    format!("a{n}_name_color"),
                    if live { "pocepochs.hab.name_live" } else { "pocepochs.hab.name_dead" },
                );
                m.set(format!("a{n}_v"), ax.signal.unwrap_or(-1.0)); // −1 = no signal yet
                m.set(format!("a{n}_lolab"), ax.low_label);
                m.set(format!("a{n}_hilab"), ax.high_label);
                m.set(format!("a{n}_status"), status);
                m.set(format!("a{n}_status_color"), status_color);
            }
            let total = h.axes.len();
            if h.life_supporting {
                m.set("life", true);
                m.set("verdict", "LIFE-SUPPORTING");
                m.set("verdict_color", "pocepochs.hab.verdict_life");
            } else {
                m.set("no_life", true);
                m.set("verdict", format!("{} / {total} axes in band", h.axes_in_band));
                m.set("verdict_color", "pocepochs.hab.verdict_count");
            }
            m.set("observed", format!("{} / {total} observed", h.axes_live));
            m.set("air", format!("air: {}", h.atmosphere_kind));
        }

        UiIntents::mirror_into(&mut m, &self.fired_sigs);
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

        // The declarative HUD (S10): styles + the `hud_pocepochs.lua` tree, built
        // once. Each axis's band (lo/hi) is STATIC observer data, published once
        // as the `HAB` data global and baked into the gauge nodes as props;
        // live values ride the Model each frame.
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        match ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES) {
            Ok(script) => {
                load_ui_json(&script, HUD_UI_ELEMENTS); // layout (`UI.pocepochs`)
                self.sim.ensure(0);
                let bands: Vec<serde_json::Value> = self
                    .sim
                    .world(0)
                    .map(|w| {
                        observe(w)
                            .axes
                            .iter()
                            .map(|ax| serde_json::json!({ "lo": ax.lo, "hi": ax.hi }))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Err(e) = script.set_global_json("HAB", &serde_json::Value::Array(bands)) {
                    tracing::error!("HAB global publish failed: {e}");
                }
                match script.ui_tree() {
                    Ok(Some(tree)) => {
                        self.ui_intents = UiIntents::of(&tree);
                        self.ui_tree = Some(tree);
                    }
                    Ok(None) => tracing::error!("HUD script exposes no `tree()` — no HUD"),
                    Err(e) => tracing::error!("HUD tree build failed ({e}); no HUD"),
                }
                self.script = Some(script);
            }
            Err(e) => tracing::warn!("HUD script load failed ({HUD_SCRIPT}): {e} — no HUD"),
        }
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.free_meshes(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // Walk the cached HUD tree: layout + hit-test + draw in one pass. The
        // habitability panel is a styled container, so the pointer over it sets
        // `hud_hit` — fed to the walker layer below as this frame's
        // pointer-consume (the camera stays a raw poll, unchanged).
        let mut over_hud = false;
        if let Some(tree) = self.ui_tree.as_ref() {
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                screen: renderer.size(),
                typed: String::new(),
                backspace: false,
                wheel: input.mouse_wheel_delta,
            };
            let lib = self.script.as_ref().map(|h| h as &dyn ComponentLibrary);
            let frame = run_ui_with(tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib);
            over_hud = frame.results.is_on("hud_hit");
            self.hud_commands = frame.commands;
        }

        // ── Input seam (spec §5/§9): ONE resolve + ONE dispatch replaces the hand-rolled
        // `Esc` edge (`prev_menu`). The active context is always `World` here (no chat /
        // modal ever pushes `TextEntry`); the walker layer's DECLARED `on_menu` intent
        // (S10) is the pause-open edge. `ev` is the REUSED `Fired` buffer; the
        // `InputEvent` list is a short-lived local because it borrows this frame's
        // snapshot (RT-7: a steady-state frame resolves zero edges). ──
        self.input_tick = self.input_tick.wrapping_add(1);
        self.ev.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad_config,
            input,
            self.input_tick,
            &mut self.ev,
        );
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done

        self.route.requests.clear();
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Standard post-dispatch seam; no handler pushes context intents here.
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // The screen DECLARED `on_menu = "pause_open"` (S9/S10): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto the
        // shell pause push — the root's hardcoded Menu arm is gone. Return before
        // polling the camera / viewer keys, exactly as the old early-return did.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &InputMap::default(),
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }

        // Orbit camera — raw pointer drag + wheel. No ActionSignal binding (exactly like
        // the reference's raw mouse-look), so it stays a direct read, not a `signal_held`
        // poll.
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

        // ── The HUD: the walker commands stashed by `update` (readout text +
        // the life-supporting gauge panel), then the two panels still
        // scene-drawn (FLAGGED, S10): the surface-view legend and the
        // element-distribution readout — their per-row swatch colours are DATA
        // (legend entries / element_rgb), and the walker's colour channel is
        // dotted style paths by design. ──
        renderer.set_layer(10.0);
        if let Some(white) = self.white {
            render_hud(renderer, &self.hud_commands, white, &[]);
        }
        let gold = [0.722, 0.592, 0.353, 1.0]; // Prism bronze (structural accent)
        let text = [0.85, 0.87, 0.92, 1.0];

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

    /// Load the real `hud_pocepochs.lua` + the shared layout and walk a frame
    /// against the LIVE observer model: the vocabulary gate holds, the root
    /// declares the pause intent, and the readout + habitability panel render —
    /// catches Lua/contract breakage headlessly (no window).
    #[test]
    fn hud_tree_is_well_formed_and_draws_from_the_observer_model() {
        use flicker::render::Vec2;
        use flicker::ui::run_ui;
        use flicker_input_core::ActionSignal;

        // The HUD's display copy is `$token`s now (S10 strings gate); load the
        // shipped table so the walked commands carry the resolved en-us text.
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");
        let script = ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES)
            .expect("hud_pocepochs.lua loads");
        load_ui_json(&script, HUD_UI_ELEMENTS);
        let mut scene = WorldScene::new();
        scene.sim.ensure(0);
        let bands: Vec<serde_json::Value> = scene
            .sim
            .world(0)
            .map(|w| {
                observe(w)
                    .axes
                    .iter()
                    .map(|ax| serde_json::json!({ "lo": ax.lo, "hi": ax.hi }))
                    .collect()
            })
            .unwrap_or_default();
        assert!(!bands.is_empty(), "the observer exposes the condition axes");
        script
            .set_global_json("HAB", &serde_json::Value::Array(bands))
            .expect("HAB publishes");
        let tree = script.ui_tree().expect("tree builds").expect("script exposes tree()");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "hud_pocepochs.lua names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "hud_pocepochs.lua ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));

        let styles = load_styles(HUD_UI_ELEMENTS);
        let model = scene.hud_model();
        let snap = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::new());
        let has = |s: &str| {
            frame.commands.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
        };
        assert!(has("FLICKER · PLANET SIMULATION"), "readout title renders");
        assert!(has("PAUSED"), "the state word rides its bind");
        assert!(has("LIFE-SUPPORTING CONDITIONS"), "habitability panel renders");
        assert!(
            frame.commands.iter().any(|c| matches!(c, HudCommand::Rect { .. })),
            "the gauges emitted their bars"
        );
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
