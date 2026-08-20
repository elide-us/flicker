//! The `Sim` scene: the cinematic camera flies in from outside a dissipating dust
//! cloud, which clears (inside-out + annular gaps at each orbit) to reveal the
//! fixed Prism system — the sun, eight planets, and Home's moon, all slowly
//! orbiting. A flicker-shell client: Esc opens the pause menu.

use std::time::Duration;

use flicker::render::{
    CompositeTarget, FrameGraph, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Rect,
    RenderTargetHandle, Renderer, SceneLighting, TextureHandle, Vec3, VolumetricDisk,
    MAX_VOLUMETRIC_BODIES,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    render_hud, run_ui, strings, SceneDef, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_flight::{Flight, FlightPlayer};
use flicker_input_core::{
    AbstractControls, ActionSignal, EventKind, GamepadConfig, InputContext, InputMap, InputState,
    MouseButton,
};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

use crate::camera::{LookDelta, OrbitCam};
use crate::route::RootHandler;
use crate::system::{self, BodyKind, Planet, SYSTEM_INNER, SYSTEM_OUTER};

/// The bundled intro cinematic (an authored `.flight`), loaded at runtime so it
/// can be retuned in the file without recompiling.
fn intro_flight_path() -> std::path::PathBuf {
    flicker_core::roots::roots()
        .package()
        .join("flights/intro.flight")
}

/// Radians per second of camera yaw/pitch at full right-stick deflection — the stick
/// look RATE (the scene multiplies by `dt`). Mouse look is a per-pixel delta instead
/// (see [`OrbitCam`]), so the two look channels tune independently.
const STICK_LOOK_RATE: f32 = 2.5;

/// Fraction-per-second the free camera's left stick dollies at full deflection — the
/// zoom RATE (×`dt`), the pad twin of the mouse wheel. Only the off-rail `Flying`
/// context binds the left stick to zoom, so on the rail this resolves to zero.
const ZOOM_STICK_RATE: f32 = 1.5;

/// The scene's PAIR SCRIPT (`SceneName.lua` — the scene's component logic).
const SOLARBIRTH_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/solarbirth.lua");

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
    // ── Input (input-P3, 0569DA9B): the scene owns NO resolver/bindings. The central
    // PUMP resolves this frame's events for the scene's declared `input_context()`
    // (FlightPath on the rail / Flying off it) and hands them in via `SceneInput`;
    // continuous look/throttle/zoom come from the pump's `signals.axis`/`pointer_delta`.
    theme: Option<Theme>,

    // ── The declarative HUD (S10): a walker tree replaces the immediate
    // `draw_text` readout. The tree + the screen's declared intents are cached
    // at enter; the script host that built them is dropped there. ──
    /// The AUTHORED tree off the manifest's def (the kernel parsed the scene file;
    /// this bench is the behaviour that plays it). `enter` patches + installs it.
    authored: Option<UiNode>,
    /// The scene file's own `styles` blocks (five-line split) — merged over the
    /// shared theme root when `enter` loads styles.
    scene_styles: Option<serde_json::Value>,
    /// The PAIR SCRIPT host (`solarbirth.lua`): `derive()` composes the phase line
    /// + hint; the engine publishes raw flight variables and resolved copy tokens.
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

    // ── The 3D viewport (Option-A graphical scene): the bespoke 3D is drawn into an
    // offscreen target sized to the rect the walker reserves for the full-screen
    // `solarbirth_view` rtt, then composited into that rect; the readout panels draw
    // over it. ──
    /// The rtt rect the walker reserved this frame (`None` before the first `update`);
    /// read in `render` to size + composite the pass, and to gate camera drag/zoom.
    viewport: Option<Rect>,
    /// The offscreen target the 3D renders into (sized to `viewport`); freed in `exit`.
    sky_target: Option<RenderTargetHandle>,
    sky_size: (u32, u32),
}

impl Sim {
    pub fn new(def: &SceneDef) -> Self {
        let path = intro_flight_path();
        let flight = Flight::load(&path)
            .unwrap_or_else(|e| panic!("loading bundled intro flight {}: {e:#}", path.display()));
        Self {
            cam: OrbitCam::new(SYSTEM_OUTER),
            planets: system::roster(),
            planet_meshes: Vec::new(),
            moon_mesh: None,
            ring_mesh: None,
            flight: FlightPlayer::new(flight),
            cinematic: true,
            anim_time: 0.0,
            // This scene IS a flight-camera vehicle (MCP 3B4DB4C2) with TWO modes: it
            // STARTS on the rail (`FlightPath`) and drops to `Flying` on a look gesture.
            // The mode is `cinematic` (above); the scene DECLARES the matching context via
            // `input_context()` and the PUMP resolves it — no scene-owned binding stack.
            theme: None,
            authored: def.tree.clone(),
            scene_styles: def.styles.clone(),
            script: match ScriptHost::new(SOLARBIRTH_SCRIPT, "solarbirth.lua") {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("solarbirth.lua failed to load — raw HUD values only: {e}");
                    None
                }
            },
            ui_tree: None,
            ui_intents: UiIntents::default(),
            ui_styles: serde_json::Value::Object(Default::default()),
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            fired_sigs: Vec::new(),
            white: None,
            viewport: None,
            sky_target: None,
            sky_size: (0, 0),
        }
    }

    /// Restart the cinematic fly-in from the opening pose, re-entering the rail.
    fn replay(&mut self) {
        self.flight.restart();
        // Back onto the rail: `cinematic = true` makes `input_context()` declare
        // `FlightPath` again, and the pump resolves that map next frame.
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
        // The ENGINE publishes raw flight variables + RESOLVED copy tokens; the PAIR
        // SCRIPT (`solarbirth.lua`) composes the phase line (five-line split).
        let mut raw = ValueMap::new()
            .with("segment", self.flight.segment_name().to_string())
            .with("progress_pct", f64::from(self.flight.progress() * 100.0))
            .with("sys", strings::resolve("$sb_the_prism_system").into_owned())
            .with(
                "approaching",
                strings::resolve("$sb_approaching").into_owned(),
            )
            .with("settled", strings::resolve("$sb_settled").into_owned());

        // Publish the LIVE control bindings so the HUD/footer show the actual key
        // (kbm) or glyph (pad) bound to each signal — never a hardcoded string
        // (MCP 1A292918 T5, 5B9A8B50). Resolved from the ACTIVE context map (the
        // same one the pause overlay takes), so a rebind or device switch shows next
        // frame. `bind_Interact`/`glyph_Interact` (Replay) + `bind_Menu` + the device.
        let ctx = if self.cinematic {
            "FlightPath"
        } else {
            "Flying"
        };
        let map = flicker_shell::input_profile()
            .context_map(ctx)
            .cloned()
            .unwrap_or_else(InputMap::flying);
        flicker_shell::publish_signal_bindings(
            &mut raw,
            &map,
            [ActionSignal::Interact, ActionSignal::Menu],
        );

        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("solarbirth: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("solarbirth.lua derive() failed: {e}"),
            }
        }
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
    fn dust(&self) -> VolumetricDisk {
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
        VolumetricDisk {
            inner: SYSTEM_INNER,
            outer: DUST_OUTER,  // engulf the system with a halo margin
            snow_line: 4.6, // a visual density feature (the Earth→Light gap), not a physics boundary here
            scale_height: 0.10, // taller billows — a cloud, not a pancake
            density: 3.5,   // denser → darker, more occluding, stronger god-rays
            formation: self.flight.progress(),
            time: self.flight.progress() * 10.0, // a few inner-disk rotations of swirl over the fly-in
            tint: Vec3::new(0.038, 0.033, 0.052), // dark dust
            glow: Vec3::new(0.85, 0.44, 0.22),   // warm heart, seen through the denser dust
            gaps,
        }
    }

    /// The Home planet (and its live world position at `anim_time`), for the moon.
    fn home_pos(&self) -> Option<(f32, Vec3)> {
        self.planets
            .iter()
            .find(|p| p.moon)
            .map(|p| (p.radius, system::planet_pos(p, self.anim_time)))
    }
}

/// A gentle per-planet tilt for its ring plane so rings read as tilted discs.
fn ring_tilt() -> Mat4 {
    Mat4::from_rotation_x(0.42) * Mat4::from_rotation_z(0.14)
}

/// Build the cinematic's HUD from the AUTHORED tree (off the manifest's def),
/// emitting one roster legend row per planet into the authored `roster_legend`
/// container. `None` if the scene file declared no tree — the cinematic still
/// plays, just without the readout.
fn hud_tree(authored: Option<&UiNode>, planets: &[Planet]) -> Option<UiNode> {
    let mut tree = authored?.clone();
    if let Some(legend) = find_by_id_mut(&mut tree, "roster_legend") {
        legend.children = legend_rows(planets);
    }
    Some(tree)
}

/// One legend row per planet (inner → outer): an 8px indent + the planet's
/// `roster_<i>` line (display DATA the Model carries, pre-formatted by `hud_model`)
/// tinted by its name's HUD palette path. Repeated-per-datum arrangement is the
/// scene's job (201F4F51 mechanism 6); the row structure is a plain `row` of kinds.
fn legend_rows(planets: &[Planet]) -> Vec<UiNode> {
    planets
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let indent = UiNode {
                component: "stack".to_string(),
                size: Some(8.0),
                ..Default::default()
            };
            let mut text = UiNode {
                component: "text".to_string(),
                grow: Some(1.0),
                ..Default::default()
            };
            text.props.insert(
                "text_bind".to_string(),
                Value::Text(format!("roster_{}", i + 1)),
            );
            text.props
                .insert("text_size".to_string(), Value::Number(13.0));
            text.props.insert(
                "color".to_string(),
                Value::Text(format!("solarbirth.roster.{}", p.name.to_lowercase())),
            );
            UiNode {
                component: "row".to_string(),
                size: Some(18.0),
                children: vec![indent, text],
                ..Default::default()
            }
        })
        .collect()
}

/// Find the first descendant (or self) whose `id` matches, mutably — the seam the
/// scene fills its authored `roster_legend` container through.
fn find_by_id_mut<'a>(node: &'a mut UiNode, id: &str) -> Option<&'a mut UiNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter_mut().find_map(|c| find_by_id_mut(c, id))
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

        // The HUD is DATA now (201F4F51): styles + the bench's template-free scene-def
        // (`solarbirth.scene.json`), built once. The 3D fills the `solarbirth_view` rtt
        // viewport; the roster legend is emitted per planet into `roster_legend`.
        self.ui_styles = flicker::ui::load_shared_styles(self.scene_styles.as_ref());
        self.ui_tree = hud_tree(self.authored.as_ref(), &self.planets);
        self.ui_intents = self.ui_tree.as_ref().map(UiIntents::of).unwrap_or_default();
    }

    /// The live input context (input-P3): the scene DECLARES its flight-camera mode so
    /// the pump resolves the matching map — `FlightPath` on the rail (left-stick
    /// throttle), `Flying` off it (left-stick zoom, look pans). Read by the runner
    /// BEFORE `update`, so a mode flip inside `update` takes effect next frame.
    fn input_context(&self) -> Option<InputContext> {
        Some(if self.cinematic {
            InputContext::FlightPath
        } else {
            InputContext::Flying
        })
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        r: &Renderer,
    ) -> Transition {
        // Walk the cached HUD tree: layout + hit-test + draw in one pass. `over_hud`
        // is the readout PANEL claiming the pointer — it gates the camera so a drag on
        // the readout doesn't orbit; the open sky (the rtt viewport) never claims.
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
            let frame = run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
            over_hud = frame.results.is_on("hud_hit");
            // The walker reserves the full-screen 3D viewport's rect; `render` sizes +
            // composites the offscreen pass into it, and the camera gates to it.
            self.viewport = frame.rtt_rect("solarbirth_view");
            self.hud_commands = frame.commands;
        }

        // ── The input seam (input-P3, 0569DA9B): the PUMP already resolved this frame's
        // events for the scene's declared `input_context()` (FlightPath on the rail /
        // Flying off it) — the scene owns no Resolver. Dispatch the pump's
        // `signals.events` through the chain; the walker layer's DECLARED `on_menu`
        // intent (S10) is the pause-open edge. No focusable tree + no context-pushing
        // handler here, so nothing to reconcile — the runner applies the route's (empty)
        // context requests to the shared stack after `update` returns. ──
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        self.fired_sigs = walker.take_fired();

        // The screen DECLARED `on_menu = "pause_open"` (S9/S10): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto the
        // shell pause push — the root's hardcoded Menu arm is gone.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.theme.expect("theme built in enter");
            // The scene owns no bindings; take the active context's map from the shared
            // profile for the pause overlay (both flight maps bind Menu, so Esc resumes).
            let ctx_name = if self.cinematic {
                "FlightPath"
            } else {
                "Flying"
            };
            let pause_map = flicker_shell::input_profile()
                .context_map(ctx_name)
                .cloned()
                .unwrap_or_else(InputMap::flying);
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &pause_map,
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }

        // Interact (controller West / keyboard E) restarts the fly-in — a MAPPED signal
        // from the pump's events (not a raw Space poll), MCP 5B9A8B50.
        if signals
            .events
            .iter()
            .any(|e| e.signal == ActionSignal::Interact && e.kind == EventKind::Press)
        {
            self.replay();
        }

        // Camera: the flight camera has TWO modes/contexts (MCP 3B4DB4C2). Look is a
        // BOUND signal in both — right stick (a rate, ×dt) + mouse RIGHT-drag (a
        // frame-absolute pixel delta); left-click stays free for select-target. ON the
        // rail (`FlightPath`) the flight drives the pose and the left stick is THROTTLE;
        // a look gesture DROPS OUT to the free camera (`Flying`), where the left stick is
        // ZOOM and look PANS.
        let dts = dt.as_secs_f32();
        // Continuous look/zoom/throttle come from the PUMP's active-context bindings
        // (`signals.axis` / `signals.pointer_delta`) — the scene queries them instead of
        // resolving itself. The active context IS the scene's `input_context()`, so on
        // the rail (`FlightPath`) zoom binds nothing and reads zero; off it (`Flying`)
        // throttle does. Look is shared by both maps.
        let stick_yaw = (signals.axis(ActionSignal::LookRight, input)
            - signals.axis(ActionSignal::LookLeft, input))
            * STICK_LOOK_RATE
            * dts;
        let stick_pitch = (signals.axis(ActionSignal::LookDown, input)
            - signals.axis(ActionSignal::LookUp, input))
            * STICK_LOOK_RATE
            * dts;
        let mouse_dx = signals.pointer_delta(ActionSignal::LookRight, input)
            - signals.pointer_delta(ActionSignal::LookLeft, input);
        let mouse_dy = signals.pointer_delta(ActionSignal::LookDown, input)
            - signals.pointer_delta(ActionSignal::LookUp, input);
        let look = LookDelta {
            stick_yaw,
            stick_pitch,
            mouse_dx,
            mouse_dy,
        };
        let zoom = (signals.axis(ActionSignal::ZoomIn, input)
            - signals.axis(ActionSignal::ZoomOut, input))
            * ZOOM_STICK_RATE
            * dts;
        // Pan/zoom apply only OFF the rail (`active = !cinematic`); on the rail this is a
        // no-op and the flight drives the pose below. Gated to the rtt viewport (and
        // blocked over the readout panel) now that the 3D is a viewport, not the window.
        self.cam
            .update(input, look, zoom, self.viewport, over_hud, !self.cinematic);
        if self.cinematic {
            // A LOOK gesture on the OPEN SKY drops out of the cinematic to the free
            // camera: an RMB-drag begun inside the viewport (not over a panel), or a
            // right-stick deflection. Left-click no longer grabs — it is reserved for
            // select-target.
            let m = input.mouse_position;
            let in_view = self.viewport.is_some_and(|r| {
                m.x >= r.pos.x
                    && m.x <= r.pos.x + r.size.x
                    && m.y >= r.pos.y
                    && m.y <= r.pos.y + r.size.y
            });
            let grabbed = (input.mouse_pressed(MouseButton::Right) && !over_hud && in_view)
                || stick_yaw != 0.0
                || stick_pitch != 0.0;
            if grabbed {
                // Drop out to the free camera: next frame `input_context()` returns
                // `Flying` and the pump resolves that map (a 1-frame skew the scene owns).
                self.cinematic = false;
            } else {
                // Left-stick (or W/S) throttle modulates the fly-in speed 0.25×..5×
                // (analog): the flight already carries a motion vector — this scales it.
                let throttle = signals.axis(ActionSignal::MoveForward, input)
                    - signals.axis(ActionSignal::MoveBackward, input);
                let speed = (1.0 + throttle * 4.0).clamp(0.25, 5.0);
                let p = self.flight.advance(dts * speed);
                self.cam.set_pose(p.yaw, p.pitch, p.distance);
            }
        }

        // The planets orbit on their own free-running clock (independent of the
        // camera flight).
        self.anim_time += dts;
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Option-A graphical scene: the 3D goes into the rtt viewport the walker
        // reserved — an offscreen pass composited into that rect — and the readout
        // panels (walker commands stashed in `update`) draw over it.
        if let Some(rect) = self.viewport {
            let w = (rect.size.x.round() as u32).max(1);
            let h = (rect.size.y.round() as u32).max(1);
            match self.sky_target {
                Some(_) if self.sky_size == (w, h) => {}
                Some(t) => {
                    renderer.resize_render_target(t, w, h);
                    self.sky_size = (w, h);
                }
                None => {
                    self.sky_target = Some(renderer.create_render_target(w, h));
                    self.sky_size = (w, h);
                }
            }
            if let Some(target) = self.sky_target {
                // Precompute the whole sub-scene as OWNED data; the frame-graph closure
                // captures it by move, so no borrow of `self` survives into `execute`.
                let camera = self.cam.camera();
                let lighting = SceneLighting {
                    sun_dir: Vec3::new(0.0, -1.0, 0.0),
                    sun_color: Vec3::ZERO,
                    moon_dir: Vec3::new(0.0, -1.0, 0.0),
                    moon_color: Vec3::ZERO,
                    // Sun = a point light at the origin; a faint ambient floor keeps
                    // night sides off pure black.
                    ambient: Vec3::splat(0.07),
                    point_pos: Vec3::ZERO,
                    point_color: Vec3::new(1.0, 0.94, 0.84), // warm starlight
                    sky_zenith: Vec3::new(0.004, 0.006, 0.014),
                    sky_horizon: Vec3::new(0.007, 0.010, 0.022),
                    ..SceneLighting::default()
                };
                let disk = self.dust();
                let anim = self.anim_time;
                let orbits: Vec<Vec<(Vec3, Vec3)>> = self
                    .planets
                    .iter()
                    .map(|p| system::orbit_ellipse(p, 128))
                    .collect();
                // Planets (+ Air's ring) and Home's moon, flattened to (mesh, model, opts).
                let mut draws: Vec<(MeshHandle, Mat4, MeshDrawOptions)> = Vec::new();
                for (p, &mesh) in self.planets.iter().zip(self.planet_meshes.iter()) {
                    let pos = system::planet_pos(p, anim);
                    draws.push((
                        mesh,
                        Mat4::from_translation(pos) * Mat4::from_scale(Vec3::splat(p.radius)),
                        MeshDrawOptions::default(),
                    ));
                    if p.rings {
                        if let Some(rh) = self.ring_mesh {
                            let tint = [0.85, 0.78, 0.42, 1.0]; // Air's warm ring
                            draws.push((
                                rh,
                                Mat4::from_translation(pos)
                                    * ring_tilt()
                                    * Mat4::from_scale(Vec3::splat(p.radius)),
                                MeshDrawOptions {
                                    tint,
                                    ..Default::default()
                                },
                            ));
                        }
                    }
                }
                if let (Some((home_r, home_pos)), Some(mmesh)) = (self.home_pos(), self.moon_mesh) {
                    let a = anim * MOON_OMEGA;
                    let orbit_r = home_r * MOON_ORBIT_MULT;
                    let off = Vec3::new(
                        orbit_r * a.cos(),
                        orbit_r * MOON_INCL.sin() * a.sin(),
                        orbit_r * MOON_INCL.cos() * a.sin(),
                    );
                    draws.push((
                        mmesh,
                        Mat4::from_translation(home_pos + off)
                            * Mat4::from_scale(Vec3::splat(MOON_RADIUS)),
                        MeshDrawOptions::default(),
                    ));
                }

                let mut fg = FrameGraph::new();
                let layer = renderer.layer();
                // The offscreen sub-scene: deep-space sky, the dust cloud (the sun is
                // rendered inside it so the dust occludes it), orbit rings, then bodies.
                fg.target(target, [0.006, 0.008, 0.014, 1.0], move |r| {
                    r.set_camera(&camera);
                    r.draw_sky();
                    r.set_scene(lighting);
                    r.set_volumetric_disk(disk);
                    for pts in &orbits {
                        r.draw_lines(pts, [0.30, 0.36, 0.52, 0.16]);
                    }
                    for (mesh, model, opts) in &draws {
                        r.draw_mesh(*mesh, *model, *opts);
                    }
                });
                fg.composite_panel(
                    target,
                    CompositeTarget::Screen,
                    rect,
                    layer,
                    [1.0; 4],
                    None,
                    None,
                );
                fg.execute(renderer);
            }
        }

        // The readout panels, over the composited viewport.
        if let Some(white) = self.white {
            render_hud(renderer, &self.hud_commands, white, &[]);
        }
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        // Give the offscreen target back — else it holds GPU memory for a picture
        // nobody is looking at once the scene leaves.
        if let Some(t) = self.sky_target.take() {
            renderer.free_render_target(t);
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
        let f = flicker_flight::Flight::load(intro_flight_path()).expect("intro.flight parses");
        assert_eq!(f.segments.len(), 2, "glide + coast");
        assert!(f.loops(), "the coast tail loops");
    }

    /// Build the scene-def HUD (the template-free `solarbirth.scene.json` tree + the
    /// emitted roster legend) and walk a frame: the vocabulary gate holds, the root
    /// declares the pause intent, the readout renders one row per planet, and the
    /// readout PANEL claims the pointer (so a drag on it doesn't orbit — the camera
    /// gates on `!hud_hit`).
    /// THE PAIR-SCRIPT REGRESSION GATE: the real solarbirth.lua loads and its
    /// derive() composes the phase line from the raw flight variables — a script
    /// the host rejects (or a derive that throws) leaves raw numbers under the
    /// display keys, which is exactly the silent in-window text breakage.
    #[test]
    fn the_pair_script_derives_the_phase_line() {
        let def = SceneDef::parse(
            "solarbirth",
            include_str!("../../../../content/sensorium/scenes/solarbirth.scene.json"),
        )
        .expect("solarbirth.scene.json loads");
        let sim = Sim::new(&def);
        assert!(
            sim.script.is_some(),
            "solarbirth.lua loads (the pair script)"
        );
        let m = sim.hud_model();
        let phase = m
            .text("phase")
            .expect("derive() yields the composed phase TEXT");
        assert!(
            phase.contains('·'),
            "the phase line is composed ('{phase}')"
        );
    }

    #[test]
    fn hud_tree_is_well_formed_and_draws_the_roster() {
        use flicker::render::Vec2;
        use flicker::ui::run_ui;

        let planets = system::roster();
        let def = SceneDef::parse(
            "solarbirth",
            include_str!("../../../../content/sensorium/scenes/solarbirth.scene.json"),
        )
        .expect("solarbirth.scene.json loads");
        let authored = def.tree.clone().expect("it declares a tree");
        let tree =
            hud_tree(Some(&authored), &planets).expect("solarbirth.scene.json builds the HUD");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "the scene tree names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "the scene tree ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The MODEL-CHANNEL strings gate (S10's blind side): display copy published
        // from Rust into the Model bypasses the tree gate above, so the crate
        // self-gates its OWN source — every `.set`/`.with` value must be a resolved
        // `$token`, a data shape, or carry an explicit `strings-gate-exempt` reason.
        let flags = strings::raw_model_publish_literals(include_str!("scene.rs"));
        assert!(
            flags.is_empty(),
            "raw display copy published into the Model: {flags:?}"
        );
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));

        // The scene's OWN style blocks ride its file (five-line split) — the
        // exact merge `enter` runs.
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        // The phase line, composed around its tokens exactly as `hud_model` does
        // ("glide" stands in for the data-driven segment name).
        let mut model = ValueMap::new().with(
            "phase",
            format!(
                "{} · {} · {} 40%",
                strings::resolve("$sb_the_prism_system"),
                "glide",
                strings::resolve("$sb_approaching"),
            ),
        );
        for (i, p) in planets.iter().enumerate() {
            model.set(format!("roster_{}", i + 1), Sim::roster_row(p));
        }
        let snap = UiInput {
            mouse: Vec2::new(30.0, 30.0), // parked ON the readout panel
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
        // The readout (title + phase + roster header + one row per planet) plus the
        // footer's control-hint labels + MENU button all render as text.
        assert!(
            texts >= 3 + planets.len(),
            "readout + footer text renders ({texts} lines for {} planets)",
            planets.len()
        );
        // The readout is a PANEL over the full-screen rtt now: the pointer on it is a
        // UI hit, so the camera (which gates on `!hud_hit`) won't orbit there — the
        // opposite of the old bare-text drag-through.
        assert!(
            frame.results.is_on("hud_hit"),
            "the readout panel claims the pointer (camera drag is blocked over it)"
        );
        // The full-screen 3D viewport must reserve a slot — a source-LESS `rtt` is
        // skipped by the walker (`rtt_rect` → None → the scene draws nothing), which
        // is exactly the blank-viewport bug this guards against at build time.
        let vp = frame
            .rtt_rect("solarbirth_view")
            .expect("the rtt viewport reserved a slot");
        assert!(
            vp.size.x > 100.0 && vp.size.y > 100.0,
            "the viewport has real extent: {:?}",
            vp.size
        );
    }
}
