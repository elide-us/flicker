//! The chemistry scene — a **thin renderer** over the sim thread that draws the
//! world as a **stack of concentric layer meshes**.
//!
//! Each layer is its own mesh at its own radius, drawn where it exists and stacked
//! bottom→top: the **core** (inner sphere), the **mantle** shell above it (always
//! there, still convecting — never replaced), then each **crust bed** as a sparse
//! shell above the mantle (holes where the surface is still bare magma). Forming
//! crust *adds* an outer shell; it never recolours the mantle. The stack grows as
//! the chemistry produces layers (M3 water + atmosphere, M6 sediment beds, … each
//! just registers another shell). Occlusion shows the outermost shell that exists
//! at each spot; the cutaway wedge (next) slices the outer shells away to reveal
//! the interior.
//!
//! The sim runs on its own thread ([`crate::sim_thread`], spec §11); this scene
//! only sends commands (Space play/pause · R reseed · Down restart) and draws the
//! latest snapshot. **V** recolours the mantle shell (temperature / differentiation)
//! — an interior read, not a replacement of the stack.

use std::time::Duration;

use flicker_input_core::{
    AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState, Key,
};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, TextureHandle, Vec2, Vec3,
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

use crate::camera::OrbitCam;
use crate::globe::{self, RADIUS};
use crate::route::RootHandler;
use crate::sim_thread::{SimCommand, SimHandle, Snapshot, BED_CONTINENTAL, BED_OCEANIC};
use flicker_poc_chemistry::PlateEvent;

/// The declarative HUD tree (`hud_chemistry.lua`) + the shared UI-element layout.
const HUD_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Alpha/content/sensorium/scripts/hud_chemistry.lua"
);
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/sensorium/resources/ui_elements.json");

/// Radii of the shells (exaggerated for legibility — real crust is a hair-thin
/// rind on the mantle; here the gaps are opened up so the stack is visible and the
/// cutaway can show it in section). Bottom → top.
const R_CORE: f32 = 0.50 * RADIUS;
const R_MANTLE: f32 = 0.960 * RADIUS;
const R_OCEANIC: f32 = 0.985 * RADIUS;
const R_CONTINENTAL: f32 = 1.000 * RADIUS;

/// Shell base colours (before lighting).
const CORE_COLOR: [f32; 3] = [0.95, 0.45, 0.20]; // molten metal
const OCEANIC_COLOR: [f32; 3] = [0.15, 0.22, 0.33]; // dark mafic sea floor
const CONTINENTAL_COLOR: [f32; 3] = [0.60, 0.54, 0.40]; // pale silicic land

/// A fresh seed from the wall clock — a new initial condition (spec §3.5). Launch
/// and **R** roll a new one; a given run stays deterministic.
fn clock_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// How the **mantle** shell is coloured (an interior read — the crust shells above
/// it are always drawn regardless).
#[derive(Copy, Clone, PartialEq)]
enum MantleView {
    Temperature,
    Differentiation,
    Plates,
    Seams,
}

impl MantleView {
    fn label(self) -> &'static str {
        match self {
            MantleView::Temperature => "temperature",
            MantleView::Differentiation => "differentiation",
            MantleView::Plates => "plates",
            MantleView::Seams => "seams",
        }
    }
    fn cycle(self) -> Self {
        match self {
            MantleView::Temperature => MantleView::Differentiation,
            MantleView::Differentiation => MantleView::Plates,
            MantleView::Plates => MantleView::Seams,
            MantleView::Seams => MantleView::Temperature,
        }
    }
}

pub struct ChemScene {
    // ── sim (on its own thread) ──
    sim: SimHandle,
    seed: u64,

    // ── static topology (received once) ──
    dirs: Vec<Vec3>,
    outlines: Vec<Vec<Vec3>>,
    budget_dist: Vec<(u8, String, f64)>,
    ready: bool,

    // ── latest published frame ──
    snap: Option<Snapshot>,
    last_gen: u64,

    // ── view ──
    cam: OrbitCam,
    /// The core sphere — static, built once.
    core_mesh: Option<MeshHandle>,
    /// The dynamic layer shells (mantle + crust beds), rebuilt on each new frame.
    shell_meshes: Vec<MeshHandle>,
    dirty: bool,
    mantle_view: MantleView,

    // ── shell ──
    theme: Option<Theme>,
    white: Option<TextureHandle>,

    // ── input bus (spec §5/§9) ──
    /// World-context bindings (Esc → `Menu`); the resolver resolves the active map.
    /// The camera + the discrete keys below stay raw, so only `Menu` rides the bus.
    bindings: ContextualBindings,
    /// Gamepad axis/deadzone config, handed to the resolver and the pause overlay.
    gamepad_config: GamepadConfig,
    /// Stateful edge resolver — the single home of previous-frame state (replaces the
    /// hand-rolled `prev_menu` bool).
    resolver: Resolver,
    /// Reused `Fired` scratch buffer (no per-frame alloc — RT-7).
    ev: Vec<Fired>,
    /// The router's per-frame request queue (context/focus intents; none arise here).
    route: RouteCtx,
    /// Monotonic frame tick — the resolver's `TickTime` (NOT wall-clock — spec §3.2a).
    tick: u64,

    // ── bespoke discrete keys (stay raw off the snapshot — Group B data viewer) ──
    prev_play: bool,
    prev_down: bool,
    prev_view: bool,
    prev_reseed: bool,

    // ── The declarative HUD (S10): a walker tree replaces the immediate text
    // readout + conservation ledger. The host is retained as the Lua component
    // library; the tree + the screen's declared intents are cached at enter. ──
    script: Option<ScriptHost>,
    ui_tree: Option<UiNode>,
    ui_intents: UiIntents,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
}

impl ChemScene {
    pub fn new() -> Self {
        let seed = clock_seed();
        Self {
            sim: SimHandle::spawn(seed),
            seed,
            dirs: Vec::new(),
            outlines: Vec::new(),
            budget_dist: Vec::new(),
            ready: false,
            snap: None,
            last_gen: 0,
            cam: OrbitCam::new(RADIUS),
            core_mesh: None,
            shell_meshes: Vec::new(),
            dirty: false,
            mantle_view: MantleView::Temperature,
            theme: None,
            white: None,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            prev_play: false,
            prev_down: false,
            prev_view: false,
            prev_reseed: false,
            script: None,
            ui_tree: None,
            ui_intents: UiIntents::default(),
            ui_styles: serde_json::Value::Object(Default::default()),
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            fired_sigs: Vec::new(),
        }
    }

    /// The per-frame HUD model: every readout line pre-formatted (the tree's
    /// `text_bind`s display them verbatim), the `loading`/`loaded` state gates,
    /// the state-word + ledger-status colour paths (`color_bind`s), plus the
    /// transient `sig_<name>` mirror of last frame's fired intents.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::new();
        match self.snap.as_ref() {
            None => {
                m.set("loading", true);
            }
            Some(snap) => {
                let s = &snap.state;
                m.set("loaded", true);
                m.set(
                    "stats",
                    format!("tick {}  ·  {:.0} My  ·  {} cells", snap.tick, snap.tick_myr, snap.swept_cells),
                );
                let core_pct = s.core_mass_kg / s.planet_mass_kg.max(1.0) * 100.0;
                m.set(
                    "interior",
                    format!(
                        "core {core_pct:.1}%  ·  differentiated {:.0}%  ·  mantle {:.0} K  ·  {} plates  ·  radiogenic {:.0} TW",
                        s.differentiation_frac * 100.0,
                        s.mean_mantle_temp_k,
                        snap.plate_count,
                        s.radiogenic_power_tw,
                    ),
                );
                let (word, color) = if snap.playing {
                    ("PLAYING", "chemistry.playing.color")
                } else {
                    ("PAUSED", "chemistry.paused.color")
                };
                m.set("play_state", word);
                m.set("play_state_color", color);
                m.set(
                    "hints",
                    format!(
                        "·  Space play/pause  ·  R reseed  ·  Down reset  ·  V mantle: {}  ·  drag · wheel · Esc menu",
                        self.mantle_view.label()
                    ),
                );
                m.set(
                    "crust",
                    format!(
                        "crust {:.3}%  ·  continental {:.0}%  ·  mean elevation {:.0} m",
                        s.crust_frac * 100.0,
                        s.continental_frac * 100.0,
                        s.mean_elevation_m,
                    ),
                );

                // ── The conservation ledger (text-only; the walker panel shows it). ──
                let present = s.core_mass_kg
                    + s.mantle_mass_kg
                    + s.crust_mass_kg
                    + s.atmosphere_mass_kg
                    + s.ocean_mass_kg
                    + s.escaped_mass_kg;
                let expected = s.planet_mass_kg + s.delivered_mass_kg;
                let balanced = (present - expected).abs() <= 1e-6 * expected.max(1.0);
                let total = expected.max(1.0);
                let pct = |mass: f64| mass / total * 100.0;
                let (status, color) = if balanced {
                    ("balanced ✓", "chemistry.ok")
                } else {
                    ("BROKEN ✗", "chemistry.bad")
                };
                m.set("ledger_status", format!("Σ {}  ·  {}", fmt_mass(expected), status));
                m.set("ledger_status_color", color);
                let rows: [(&str, f64); 6] = [
                    ("Mantle", s.mantle_mass_kg),
                    ("Core", s.core_mass_kg),
                    ("Crust", s.crust_mass_kg),
                    ("Atmosphere", s.atmosphere_mass_kg),
                    ("Ocean", s.ocean_mass_kg),
                    ("Escaped", s.escaped_mass_kg),
                ];
                for (i, (label, mass)) in rows.iter().enumerate() {
                    m.set(format!("ledger_{}", i + 1), format!("{label:<11}{:>6.2}%", pct(*mass)));
                }
            }
        }
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    fn free_meshes(&mut self, renderer: &mut Renderer) {
        if let Some(h) = self.core_mesh.take() {
            renderer.free_mesh(h);
        }
        for h in self.shell_meshes.drain(..) {
            renderer.free_mesh(h);
        }
    }
}

impl Default for ChemScene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for ChemScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0]; // deep space
        let theme = Theme::build(renderer);
        self.white = Some(theme.lua_textures()[0].1); // id 0 = "white"
        self.theme = Some(theme);

        // The declarative HUD (S10): styles + the `hud_chemistry.lua` tree, built
        // once; values ride the Model each frame.
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        match ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES) {
            Ok(script) => {
                load_ui_json(&script, HUD_UI_ELEMENTS); // layout (`UI.chemistry`)
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
        // The sim thread shuts down when `self.sim` (SimHandle) drops.
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // Walk the cached HUD tree: layout + hit-test + draw in one pass. The
        // ledger panel is a styled container, so the pointer over it sets
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

        // ── The input seam (spec §5/§9): ONE resolve + ONE dispatch replaces the raw
        // `prev_menu` edge. The resolver owns the `Menu` (Esc) press edge; the walker
        // layer's DECLARED `on_menu` intent (S10) is the pause-open edge. The orbit
        // camera + the discrete data-viewer keys below stay on the raw snapshot. `ev`
        // is the REUSED `Fired` buffer; the `InputEvent` list is a short-lived local
        // (it borrows this frame's snapshot — RT-7).
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver
            .resolve_frame(&self.bindings, &self.gamepad_config, input, self.tick, &mut self.ev);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
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
        // shell pause push — the root's hardcoded Menu arm is gone.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                self.bindings.active_map(),
                &AbstractControls::default(),
                &self.gamepad_config,
            )));
        }

        if !self.ready {
            if let Some(s) = self.sim.take_static() {
                self.dirs = s.dirs;
                self.outlines = s.outlines;
                self.budget_dist = s.budget_dist;
                self.ready = true;
            }
        }

        let play = input.key_down(Key::Space);
        let down = input.key_down(Key::Down);
        let view = input.key_down(Key::V);
        let reseed = input.key_down(Key::R);
        if play && !self.prev_play {
            self.sim.send(SimCommand::TogglePlay);
        }
        if down && !self.prev_down {
            self.sim.send(SimCommand::Reset);
        }
        if reseed && !self.prev_reseed {
            self.seed = clock_seed();
            self.sim.send(SimCommand::Reseed(self.seed));
        }
        if view && !self.prev_view {
            self.mantle_view = self.mantle_view.cycle();
            self.dirty = true;
        }
        self.prev_play = play;
        self.prev_down = down;
        self.prev_view = view;
        self.prev_reseed = reseed;

        self.cam.update(input, true);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if !self.ready {
            // The loading banner rides the walker tree (`loading` visible_bind).
            self.draw_hud(renderer);
            return;
        }

        // The core shell is static — build it once.
        if self.core_mesh.is_none() && !self.dirs.is_empty() {
            let (v, i) = globe::build(&self.dirs, &self.outlines, R_CORE, |_| Some(CORE_COLOR));
            self.core_mesh = Some(renderer.upload_mesh(&v, MeshIndices::U32(&i)));
        }

        // Pull the newest frame; rebuild the dynamic shells if it advanced.
        if let Some(s) = self.sim.latest_if_newer(self.last_gen) {
            self.last_gen = s.gen;
            self.snap = Some(s);
            self.dirty = true;
        }

        if self.dirty {
            for h in self.shell_meshes.drain(..) {
                renderer.free_mesh(h);
            }
            if let Some(snap) = self.snap.as_ref() {
                let view = self.mantle_view;
                let dirs = &self.dirs;
                let outlines = &self.outlines;
                let cells = &snap.cells;
                let (tmin, tmax) = cells
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), c| (lo.min(c.temp_k), hi.max(c.temp_k)));
                let tspan = (tmax - tmin).max(1.0);
                let light = Vec3::new(0.4, 0.7, 0.55).normalize();
                let lit = |i: usize, base: [f32; 3]| {
                    let l = (dirs[i].dot(light) * 0.5 + 0.5).clamp(0.25, 1.0);
                    [base[0] * l, base[1] * l, base[2] * l]
                };

                // Mantle shell — always present, coloured by the selected interior field.
                let (mv, mi) = globe::build(dirs, outlines, R_MANTLE, |i| {
                    let c = &cells[i];
                    let base = match view {
                        MantleView::Temperature => temp_color((c.temp_k - tmin) / tspan),
                        MantleView::Differentiation => diff_color(c.differentiation),
                        MantleView::Plates => plate_color(c.plate),
                        MantleView::Seams => seam_color(c.seam),
                    };
                    Some(lit(i, base))
                });
                self.shell_meshes.push(renderer.upload_mesh(&mv, MeshIndices::U32(&mi)));

                // Oceanic crust shell — sparse.
                let (ov, oi) = globe::build(dirs, outlines, R_OCEANIC, |i| {
                    (cells[i].beds & BED_OCEANIC != 0).then(|| lit(i, OCEANIC_COLOR))
                });
                if !oi.is_empty() {
                    self.shell_meshes.push(renderer.upload_mesh(&ov, MeshIndices::U32(&oi)));
                }

                // Continental crust shell — sparse, outermost.
                let (cv, ci) = globe::build(dirs, outlines, R_CONTINENTAL, |i| {
                    (cells[i].beds & BED_CONTINENTAL != 0).then(|| lit(i, CONTINENTAL_COLOR))
                });
                if !ci.is_empty() {
                    self.shell_meshes.push(renderer.upload_mesh(&cv, MeshIndices::U32(&ci)));
                }
            }
            self.dirty = false;
        }

        renderer.set_camera(&self.cam.camera());
        let opts = MeshDrawOptions::default();
        if let Some(h) = self.core_mesh {
            renderer.draw_mesh(h, Mat4::IDENTITY, opts);
        }
        for &h in &self.shell_meshes {
            renderer.draw_mesh(h, Mat4::IDENTITY, opts);
        }
        self.draw_hud(renderer);
    }
}

impl ChemScene {
    /// The HUD: the walker commands stashed by `update` (loading banner, readout
    /// lines, conservation ledger) + the two panels still scene-drawn (FLAGGED,
    /// S10): the bulk-seed element swatches and the tectonic-event log — their
    /// per-row colours are DATA (element_rgb / per-event tints), and the walker's
    /// colour channel is dotted style paths by design.
    fn draw_hud(&self, renderer: &mut Renderer) {
        renderer.set_layer(10.0);
        if let Some(white) = self.white {
            render_hud(renderer, &self.hud_commands, white, &[]);
        }

        let Some(snap) = self.snap.as_ref() else {
            return;
        };
        let gold = [0.722, 0.592, 0.353, 1.0]; // Prism bronze (structural accent)
        let text = [0.85, 0.87, 0.92, 1.0];
        let Some(white) = self.white else {
            return;
        };

        // ── Bulk-seed element distribution (left). ──
        if !self.budget_dist.is_empty() {
            let (pad, sw, row_h, panel_w) = (12.0f32, 12.0f32, 18.0f32, 210.0f32);
            let panel_h = pad + 24.0 + self.budget_dist.len() as f32 * row_h + pad;
            let (px, py) = (16.0f32, 158.0f32);
            renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.9]);
            renderer.draw_text("BULK ACCRETION SEED", Vec2::new(px + pad, py + pad), 14.0, gold);
            let mut ry = py + pad + 24.0;
            for (num, sym, pct) in &self.budget_dist {
                let c = element_rgb(*num);
                renderer.draw_sprite(white, Vec2::new(px + pad, ry + 2.0), Vec2::new(sw, sw), [c[0], c[1], c[2], 1.0]);
                renderer.draw_text(&format!("{sym}   {pct:.1}%"), Vec2::new(px + pad + sw + 8.0, ry), 13.0, text);
                ry += row_h;
            }
        }

        // ── Tectonic events (right, under the conservation ledger) — the observer's
        //    live read-out of plates being born, merging, splitting, dying. ──
        if !snap.recent_events.is_empty() {
            let (pad, row_h, panel_w) = (12.0f32, 16.0f32, 260.0f32);
            let panel_h = pad + 24.0 + snap.recent_events.len() as f32 * row_h + pad;
            let px = renderer.size().x - panel_w - 16.0;
            let py = 210.0;
            renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.9]);
            renderer.draw_text("TECTONIC EVENTS", Vec2::new(px + pad, py + pad), 14.0, gold);
            let mut ry = py + pad + 24.0;
            for (myr, ev) in snap.recent_events.iter().rev() {
                let (txt, col) = fmt_event(ev);
                renderer.draw_text(&format!("{myr:>6.0} My  {txt}"), Vec2::new(px + pad, ry), 13.0, col);
                ry += row_h;
            }
        }
    }
}

/// Linear blend of two RGB triples.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Temperature ramp over a normalised value: cool deep-blue → red → white-hot.
fn temp_color(x: f32) -> [f32; 3] {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        lerp3([0.10, 0.16, 0.55], [0.90, 0.35, 0.12], x * 2.0)
    } else {
        lerp3([0.90, 0.35, 0.12], [1.0, 0.95, 0.85], (x - 0.5) * 2.0)
    }
}

/// Core-formation progress: undifferentiated slate → differentiated gold.
fn diff_color(d: f32) -> [f32; 3] {
    lerp3([0.12, 0.13, 0.18], [0.95, 0.75, 0.30], d.clamp(0.0, 1.0))
}

/// A stable, distinct hue per persistent plate id (golden-ratio rotation). Because
/// the observer keeps a plate's id across ticks, its colour no longer flickers as it
/// drifts. Diffuse lithosphere (id 0) is neutral grey.
fn plate_color(id: u32) -> [f32; 3] {
    if id == 0 {
        return [0.22, 0.23, 0.26];
    }
    let h = (id as f32 * 0.618_034).fract() * std::f32::consts::TAU;
    [0.45 + 0.4 * h.cos(), 0.45 + 0.4 * (h + 2.094).cos(), 0.45 + 0.4 * (h + 4.188).cos()]
}

/// Seam class → colour: divergent ridge (blue), convergent trench (red), transform
/// (amber); interior / diffuse is dim.
fn seam_color(code: u8) -> [f32; 3] {
    match code {
        1 => [0.30, 0.55, 0.95], // divergent — spreading ridge
        2 => [0.90, 0.30, 0.25], // convergent — trench / collision
        3 => [0.95, 0.80, 0.30], // transform — strike-slip
        _ => [0.20, 0.21, 0.24], // interior / diffuse
    }
}

/// Muted colour per element (atomic number) for the distribution swatches.
fn element_rgb(number: u8) -> [f32; 3] {
    match number {
        26 => [0.56, 0.28, 0.18],
        8 => [0.55, 0.55, 0.60],
        14 => [0.72, 0.66, 0.52],
        12 => [0.58, 0.72, 0.55],
        16 => [0.86, 0.78, 0.36],
        28 => [0.70, 0.72, 0.74],
        20 => [0.80, 0.78, 0.72],
        13 => [0.66, 0.66, 0.70],
        x => {
            let h = (x as f32 * 0.137).fract();
            [0.40 + 0.30 * h, 0.35, 0.52 - 0.20 * h]
        }
    }
}

/// Format a mass in kg with a compact mantissa/exponent (e.g. `5.972e24 kg`).
fn fmt_mass(kg: f64) -> String {
    if kg <= 0.0 {
        return "0 kg".to_string();
    }
    let exp = kg.log10().floor() as i32;
    let mantissa = kg / 10f64.powi(exp);
    format!("{mantissa:.3}e{exp} kg")
}

/// Format a plate life-event for the events panel, with a colour cue.
fn fmt_event(e: &PlateEvent) -> (String, [f32; 4]) {
    match e {
        PlateEvent::Born(id) => (format!("born   P{id}"), [0.55, 0.85, 0.55, 1.0]),
        PlateEvent::Died(id) => (format!("died   P{id}"), [0.68, 0.70, 0.75, 1.0]),
        PlateEvent::Merged { from, into } => {
            (format!("merge  {}→P{into}", from.len() + 1), [0.55, 0.72, 0.95, 1.0])
        }
        PlateEvent::Split { from, into } => {
            (format!("split  P{from}→{}", into.len()), [0.95, 0.80, 0.40, 1.0])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load the real `hud_chemistry.lua` + the shared layout and walk a frame in
    /// BOTH states: the vocabulary gate holds, the root declares the pause
    /// intent, the loading banner shows only while `loading`, and the loaded
    /// readout + ledger render their bound lines.
    #[test]
    fn hud_tree_is_well_formed_and_gates_its_states() {
        use flicker::ui::run_ui;
        use flicker_input_core::ActionSignal;

        // The HUD's display copy is `$token`s now (S10 strings gate); load the
        // shipped table so the walked commands carry the resolved en-us text.
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Alpha/content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");
        let script = ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES)
            .expect("hud_chemistry.lua loads");
        load_ui_json(&script, HUD_UI_ELEMENTS);
        let tree = script.ui_tree().expect("tree builds").expect("script exposes tree()");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "hud_chemistry.lua names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "hud_chemistry.lua ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));

        let styles = load_styles(HUD_UI_ELEMENTS);
        let snap = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let has = |cmds: &[HudCommand], s: &str| {
            cmds.iter().any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
        };

        // Loading state: the banner shows, the readout does not.
        let loading = ValueMap::new().with("loading", true);
        let cmds = run_ui(&tree, &loading, &styles, &snap, &mut UiState::new()).commands;
        assert!(has(&cmds, "GENERATING PLANET…"), "loading banner renders");
        assert!(!has(&cmds, "FLICKER · CHEMISTRY SIM (M2 · LAYER STACK)"), "readout gated off");

        // Loaded state: readout + ledger lines ride their binds.
        let loaded = ValueMap::new()
            .with("loaded", true)
            .with("stats", "tick 42  ·  84 My  ·  92162 cells")
            .with("interior", "core 31.0%  ·  differentiated 88%")
            .with("play_state", "PLAYING")
            .with("play_state_color", "chemistry.playing.color")
            .with("hints", "·  Space play/pause")
            .with("crust", "crust 1.2%")
            .with("ledger_status", "Σ 5.972e24 kg  ·  balanced ✓")
            .with("ledger_status_color", "chemistry.ok")
            .with("ledger_1", "Mantle      68.00%")
            .with("ledger_2", "Core        31.00%")
            .with("ledger_3", "Crust        0.50%")
            .with("ledger_4", "Atmosphere   0.30%")
            .with("ledger_5", "Ocean        0.10%")
            .with("ledger_6", "Escaped      0.10%");
        let cmds = run_ui(&tree, &loaded, &styles, &snap, &mut UiState::new()).commands;
        assert!(!has(&cmds, "GENERATING PLANET…"), "loading banner gated off");
        assert!(has(&cmds, "FLICKER · CHEMISTRY SIM (M2 · LAYER STACK)"), "title renders");
        assert!(has(&cmds, "PLAYING"), "state word rides its bind");
        assert!(has(&cmds, "CONSERVATION LEDGER"), "ledger panel renders");
        assert!(has(&cmds, "Mantle      68.00%"), "ledger rows ride their binds");
    }
}
