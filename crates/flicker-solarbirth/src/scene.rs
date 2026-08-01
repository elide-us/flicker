//! The `Sim` scene: the cinematic camera flies in from outside a dissipating dust
//! cloud, which clears (inside-out + annular gaps at each orbit) to reveal the
//! fixed Prism system — the sun, eight planets, and Home's moon, all slowly
//! orbiting. A flicker-shell client: Esc opens the pause menu.

use std::time::Duration;

use flicker_input_core::{AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, SceneLighting, TextureHandle, Vec3,
    VolumetricDisk, MAX_VOLUMETRIC_BODIES,
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
use flicker_flight::{Flight, FlightPlayer};

use crate::camera::OrbitCam;
use crate::route::RootHandler;
use crate::system::{self, BodyKind, Planet, SYSTEM_INNER, SYSTEM_OUTER};

/// The bundled intro cinematic (an authored `.flight`), loaded at runtime so it
/// can be retuned in the file without recompiling.
const INTRO_FLIGHT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/flights/intro.flight");

/// The declarative HUD tree (`hud_solarbirth.lua`) + the shared UI-element layout.
const HUD_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Alpha/content/sensorium/scripts/hud_solarbirth.lua"
);
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/sensorium/resources/ui_elements.json");

/// The moon orbits Home at this multiple of Home's radius, at this angular speed.
const MOON_ORBIT_MULT: f32 = 2.6;
const MOON_OMEGA: f32 = 0.9;
const MOON_INCL: f32 = 0.45;
const MOON_RADIUS: f32 = 0.11;
const MOON_COLOR: [f32; 3] = [0.66, 0.68, 0.72];

/// The dust cloud reaches well past the outermost planet, so the *formed* system
/// sits inside a big enveloping nebula — bigger and denser than a thin
/// protoplanetary ring (the planets already exist, so the cloud is drama, not
/// accretion). A cinematic (art) choice, tunable freely — see the art-vs-reality
/// rule; only the roster/bodies are ruled reality.
const DUST_OUTER: f32 = SYSTEM_OUTER * 1.4;

pub struct Sim {
    cam: OrbitCam,
    planets: Vec<Planet>,
    /// One sphere mesh per planet (index-aligned with `planets`), uploaded once.
    planet_meshes: Vec<MeshHandle>,
    moon_mesh: Option<MeshHandle>,
    ring_mesh: Option<MeshHandle>,
    /// The intro cinematic, played by the flicker-flight service — it drives the
    /// camera pose and the dust-clearing clock (its `progress()`).
    flight: FlightPlayer,
    /// While true the flight is choreographing the camera; the first drag hands
    /// manual orbit control back. Space re-arms it.
    cinematic: bool,
    /// Free-running clock (seconds) driving the planets' orbital motion.
    anim_time: f32,
    // ── Input seam (spec §9): the resolve▸dispatch bus this scene routes its pause
    // arbitration through. The old `menu_prev` edge bool is gone — the stateful edge
    // `Resolver` owns the Menu press edge; `ev` is a REUSED `Fired` scratch buffer (no
    // per-frame alloc — RT-7); `route` is the router's request queue; `tick` is the
    // resolver's monotonic `TickTime` (NOT wall-clock — spec §3.2a).
    bindings: ContextualBindings,
    gamepad_config: GamepadConfig,
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    tick: u64,
    /// Space replays the fly-in — a bespoke cinematic key (not a mapped `ActionSignal`),
    /// so it stays a direct edge-detected key read off the raw snapshot (spec §9).
    space_prev: bool,
    theme: Option<Theme>,

    // ── The declarative HUD (S10): a walker tree replaces the immediate
    // `draw_text` readout. The host is retained as the Lua component library;
    // the tree + the screen's declared intents are cached at enter. ──
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
    /// 1×1 white texture (the theme's) backing `render_hud`'s rect fills.
    white: Option<TextureHandle>,
}

impl Sim {
    pub fn new() -> Self {
        let flight = Flight::load(INTRO_FLIGHT)
            .unwrap_or_else(|e| panic!("loading bundled intro flight {INTRO_FLIGHT}: {e:#}"));
        Self {
            cam: OrbitCam::new(SYSTEM_OUTER),
            planets: system::roster(),
            planet_meshes: Vec::new(),
            moon_mesh: None,
            ring_mesh: None,
            flight: FlightPlayer::new(flight),
            cinematic: true,
            anim_time: 0.0,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            space_prev: false,
            theme: None,
            script: None,
            ui_tree: None,
            ui_intents: UiIntents::default(),
            ui_styles: serde_json::Value::Object(Default::default()),
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            fired_sigs: Vec::new(),
            white: None,
        }
    }

    /// Restart the cinematic fly-in from the opening pose.
    fn replay(&mut self) {
        self.flight.restart();
        self.cinematic = true;
    }

    /// One roster legend row, pre-formatted: `"Home  (rocky, moon)"`. Display
    /// DATA (Prism-ruled names + class tags), so it rides the Model — the tree's
    /// `roster_<i>` binds — rather than being baked into the tree as copy.
    fn roster_row(p: &Planet) -> String {
        let mut tags = vec![p.kind.label()];
        if p.moon {
            tags.push("moon");
        }
        if p.rings {
            tags.push("rings");
        }
        if p.occulted {
            tags.push("occulted");
        }
        format!("{}  ({})", p.name, tags.join(", "))
    }

    /// The per-frame HUD model: the pre-formatted flight-phase line, the roster
    /// legend rows, plus the transient `sig_<name>` mirror of last frame's
    /// fired intents.
    fn hud_model(&self) -> ValueMap {
        let seg = self.flight.segment_name();
        let phase = if self.flight.progress() >= 1.0 {
            format!("the Prism system · {seg} · settled")
        } else {
            format!("the Prism system · {seg} · approaching {:.0}%", self.flight.progress() * 100.0)
        };
        let mut m = ValueMap::new().with("phase", phase);
        for (i, p) in self.planets.iter().enumerate() {
            m.set(format!("roster_{}", i + 1), Self::roster_row(p));
        }
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    /// Configure the volumetric dust cloud for this frame: the disk geometry, the
    /// formation clock (inside-out dissipation), and annular lanes carved **only at
    /// the giants' orbits** (the "clearing" — cosmetic, no accounting). Bigger and
    /// denser than a thin accretion ring: the planets are already formed, so the
    /// cloud reads as one billowing nebula they sit inside, not a stack of rings.
    fn set_dust(&self, renderer: &mut Renderer) {
        // Only the giants (gas + ice) part the dust into lanes; the inner worlds no
        // longer clear rings, so the dense cloud stays continuous but for a few
        // dramatic gaps that let starlight break through as god-rays.
        let mut gaps: Vec<(f32, f32)> = self
            .planets
            .iter()
            .filter(|p| matches!(p.kind, BodyKind::GasGiant | BodyKind::IceGiant))
            .map(|p| {
                let width = (0.6 + p.a * 0.06).min(1.8);
                (p.a, width)
            })
            .collect();
        gaps.truncate(MAX_VOLUMETRIC_BODIES);
        renderer.set_volumetric_disk(VolumetricDisk {
            inner: SYSTEM_INNER,
            outer: DUST_OUTER, // engulf the system with a halo margin
            snow_line: 4.6,    // a visual density feature (the Earth→Light gap), not a physics boundary here
            scale_height: 0.10, // taller billows — a cloud, not a pancake
            density: 3.5,       // denser → darker, more occluding, stronger god-rays
            formation: self.flight.progress(),
            time: self.flight.progress() * 10.0, // a few inner-disk rotations of swirl over the fly-in
            tint: Vec3::new(0.038, 0.033, 0.052), // dark dust
            glow: Vec3::new(0.85, 0.44, 0.22),    // warm heart, seen through the denser dust
            gaps,
        });
    }

    /// The Home planet (and its live world position at `anim_time`), for the moon.
    fn home_pos(&self) -> Option<(f32, Vec3)> {
        self.planets
            .iter()
            .find(|p| p.moon)
            .map(|p| (p.radius, system::planet_pos(p, self.anim_time)))
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

/// A gentle per-planet tilt for its ring plane so rings read as tilted discs.
fn ring_tilt() -> Mat4 {
    Mat4::from_rotation_x(0.42) * Mat4::from_rotation_z(0.14)
}

impl Scene for Sim {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.006, 0.008, 0.014, 1.0]; // deep space

        // One sphere per planet, coloured by its school; the sun's point light
        // shades each from its own direction to the origin (correct terminators).
        self.planet_meshes = self
            .planets
            .iter()
            .map(|p| {
                let (v, i) = system::uv_sphere(p.color, 40, 24);
                renderer.upload_mesh(&v, MeshIndices::U32(&i))
            })
            .collect();

        let (mv, mi) = system::uv_sphere(MOON_COLOR, 24, 16);
        self.moon_mesh = Some(renderer.upload_mesh(&mv, MeshIndices::U32(&mi)));

        // A unit ring annulus (radii in planet-radii); tinted per planet at draw time.
        let (rv, ri) = system::ring_mesh(1.35, 2.05, 72, 9);
        self.ring_mesh = Some(renderer.upload_mesh(&rv, MeshIndices::U32(&ri)));

        // Gothic theme for the shell pause overlay we push on Esc.
        let theme = Theme::build(renderer);
        self.white = Some(theme.lua_textures()[0].1); // id 0 = "white"
        self.theme = Some(theme);

        // The declarative HUD (S10): styles + the `hud_solarbirth.lua` tree,
        // built once. The fixed roster rides the `ROSTER` data global (name +
        // tag line per planet, inner → outer); values ride the Model.
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        match ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES) {
            Ok(script) => {
                load_ui_json(&script, HUD_UI_ELEMENTS); // layout (`UI.solarbirth`)
                // Structure only: the name picks each row's colour path; the
                // row TEXT rides the `roster_<i>` Model binds (see `hud_model`).
                let roster: Vec<serde_json::Value> = self
                    .planets
                    .iter()
                    .map(|p| serde_json::json!({ "name": p.name }))
                    .collect();
                if let Err(e) =
                    script.set_global_json("ROSTER", &serde_json::Value::Array(roster))
                {
                    tracing::error!("ROSTER global publish failed: {e}");
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

    fn update(&mut self, dt: Duration, input: &InputState, r: &Renderer) -> Transition {
        // Walk the cached HUD tree: layout + hit-test + draw in one pass. Bare
        // text on space (no styled containers), so it never claims the pointer
        // (`hud_hit` stays false) and drag-to-orbit keeps working across it.
        let mut over_hud = false;
        if let Some(tree) = self.ui_tree.as_ref() {
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                screen: r.size(),
                typed: String::new(),
                backspace: false,
                wheel: input.mouse_wheel_delta,
            };
            let lib = self.script.as_ref().map(|h| h as &dyn ComponentLibrary);
            let frame = run_ui_with(tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib);
            over_hud = frame.results.is_on("hud_hit");
            self.hud_commands = frame.commands;
        }

        // ── The input seam (spec §9): ONE resolve + ONE dispatch replaces the old
        // `menu_prev` edge. `Menu` (Esc) resolves over the active World map, wraps to
        // an `InputEvent`, and dispatches through the chain; the walker layer's
        // DECLARED `on_menu` intent (S10) is the pause-open edge. `ev` is the REUSED
        // `Fired` buffer; the `InputEvent` list is a per-frame local because it
        // borrows this frame's snapshot (RT-7 holds — a steady-state frame resolves
        // zero edges). ──
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
        // Reconcile any router-owned context/focus intents (none this scene — the
        // chain has no context-pushing handler — but kept for the standard shape).
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

        // Space replays the fly-in from the top — a bespoke cinematic key (not a mapped
        // `ActionSignal`), so it stays a direct edge-detected key read (spec §9).
        let space = input.key_down(Key::Space);
        if space && !self.space_prev {
            self.replay();
        }
        self.space_prev = space;

        // Camera: the flight drives the pose until the first drag; a drag hands manual
        // orbit control back. The orbit camera stays POLLED off the raw snapshot (mouse
        // drag + wheel are not mapped controls). The flight advances only while driving.
        let dts = dt.as_secs_f32();
        self.cam.update(input, !self.cinematic);
        if self.cinematic {
            if input.mouse_left {
                self.cinematic = false;
            } else {
                let p = self.flight.advance(dts);
                self.cam.set_pose(p.yaw, p.pitch, p.distance);
            }
        }

        // The planets orbit on their own free-running clock (independent of the
        // camera flight).
        self.anim_time += dts;
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&self.cam.camera());

        // Deep-space galactic background: the sky pass renders a Milky Way band +
        // star field at "night", so we push the sun *and* moon lights below the
        // horizon (no discs) and set a near-black gradient. The dust composites
        // over it — dense dust occludes the stars into dark lanes.
        renderer.draw_sky();
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.0, -1.0, 0.0),
            sun_color: Vec3::ZERO,
            moon_dir: Vec3::new(0.0, -1.0, 0.0),
            moon_color: Vec3::ZERO,
            // The sun is a **point light at the origin**: every planet mesh is
            // shaded per-fragment from its own direction to it, over a faint
            // ambient floor so night sides aren't pure black.
            ambient: Vec3::splat(0.07),
            point_pos: Vec3::ZERO,
            point_color: Vec3::new(1.0, 0.94, 0.84), // warm starlight
            sky_zenith: Vec3::new(0.004, 0.006, 0.014),
            sky_horizon: Vec3::new(0.007, 0.010, 0.022),
            ..SceneLighting::default()
        });

        // The dust cloud (the sun is rendered *inside* this pass so the dust
        // occludes it — no separate star billboard, which would draw on top).
        self.set_dust(renderer);

        // Faint orbit-reference circles.
        for p in &self.planets {
            renderer.draw_lines(&system::orbit_ellipse(p, 128), [0.30, 0.36, 0.52, 0.16]);
        }

        // The planets: each a school-coloured sphere on its circular orbit, lit by
        // the sun point light. Air wears a tinted ring; Death stays near-black
        // (occulted). Home's moon rides a tilted orbit around it.
        let ring_mesh = self.ring_mesh;
        let moon_mesh = self.moon_mesh;
        for (p, &mesh) in self.planets.iter().zip(self.planet_meshes.iter()) {
            let pos = system::planet_pos(p, self.anim_time);
            let model = Mat4::from_translation(pos) * Mat4::from_scale(Vec3::splat(p.radius));
            renderer.draw_mesh(mesh, model, MeshDrawOptions::default());

            if p.rings {
                if let Some(rh) = ring_mesh {
                    let tint = [0.85, 0.78, 0.42, 1.0]; // Air's warm ring
                    let rmodel =
                        Mat4::from_translation(pos) * ring_tilt() * Mat4::from_scale(Vec3::splat(p.radius));
                    renderer.draw_mesh(rh, rmodel, MeshDrawOptions { tint, ..Default::default() });
                }
            }
        }

        // Home's moon.
        if let (Some((home_r, home_pos)), Some(mmesh)) = (self.home_pos(), moon_mesh) {
            let a = self.anim_time * MOON_OMEGA;
            let orbit_r = home_r * MOON_ORBIT_MULT;
            let off = Vec3::new(
                orbit_r * a.cos(),
                orbit_r * MOON_INCL.sin() * a.sin(),
                orbit_r * MOON_INCL.cos() * a.sin(),
            );
            let model =
                Mat4::from_translation(home_pos + off) * Mat4::from_scale(Vec3::splat(MOON_RADIUS));
            renderer.draw_mesh(mmesh, model, MeshDrawOptions::default());
        }

        // The declarative HUD (walker commands stashed by `update`).
        if let Some(white) = self.white {
            render_hud(renderer, &self.hud_commands, white, &[]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_input_core::ActionSignal;

    /// The bundled intro cinematic must parse — guards the asset at test time so a
    /// typo doesn't surface only as a runtime panic when the scene starts.
    #[test]
    fn bundled_intro_flight_loads() {
        let f = flicker_flight::Flight::load(INTRO_FLIGHT).expect("intro.flight parses");
        assert_eq!(f.segments.len(), 2, "glide + coast");
        assert!(f.loops(), "the coast tail loops");
    }

    /// Load the real `hud_solarbirth.lua` + the shared layout and walk a frame:
    /// the vocabulary gate holds, the root declares the pause intent, the roster
    /// legend renders one row per planet, and bare text never claims the pointer
    /// (so drag-to-orbit keeps working across the readout).
    #[test]
    fn hud_tree_is_well_formed_and_draws_the_roster() {
        use flicker::render::Vec2;
        use flicker::ui::run_ui;

        let script = ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES)
            .expect("hud_solarbirth.lua loads");
        load_ui_json(&script, HUD_UI_ELEMENTS);
        let planets = system::roster();
        let roster: Vec<serde_json::Value> = planets
            .iter()
            .map(|p| serde_json::json!({ "name": p.name }))
            .collect();
        script
            .set_global_json("ROSTER", &serde_json::Value::Array(roster))
            .expect("ROSTER publishes");
        let tree = script.ui_tree().expect("tree builds").expect("script exposes tree()");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "hud_solarbirth.lua names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "hud_solarbirth.lua ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));

        let styles = load_styles(HUD_UI_ELEMENTS);
        let mut model = ValueMap::new().with("phase", "the Prism system · glide · approaching 40%");
        for (i, p) in planets.iter().enumerate() {
            model.set(format!("roster_{}", i + 1), Sim::roster_row(p));
        }
        let snap = UiInput {
            mouse: Vec2::new(30.0, 30.0), // parked ON the readout text
            clicked: false,
            down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::new());
        let texts = frame
            .commands
            .iter()
            .filter(|c| matches!(c, HudCommand::Text { .. }))
            .count();
        // title + phase + hint + roster header + one row per planet.
        assert_eq!(texts, 4 + planets.len(), "every readout line renders");
        assert!(
            !frame.results.is_on("hud_hit"),
            "bare text on space never claims the pointer (drag-to-orbit works across it)"
        );
    }
}
