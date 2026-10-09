//! The `Sim` scene: the cinematic camera flies in from outside a dissipating dust
//! cloud, which clears (inside-out + annular gaps at each orbit) to reveal the
//! fixed Prism system — the sun, eight planets, and Home's moon, all slowly
//! orbiting. A flicker-shell client: Esc opens the pause menu.

use std::time::Duration;

use flicker::render::{
    CompositeTarget, FrameGraph, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Rect,
    RenderTargetHandle, Renderer, StageDef, StageInputs, TextureHandle, Vec3,
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
};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

use crate::camera::{LookDelta, OrbitCam};
use crate::route::RootHandler;
use crate::system::{self, BodyKind, Planet, SYSTEM_OUTER};

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
    /// The RECIPE the viewport renders — `stages.<source>` for the source the view node
    /// names, compiled ONCE at enter. It says what is drawn around this scene's own
    /// content: the deep-space sky behind, the dust cloud over. `None` = the source
    /// named nothing that compiles (warned there), and the 3D is not drawn.
    stage: Option<StageDef>,
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
            stage: None,
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

    /// Annular lanes carved **only at the giants' orbits** — the "clearing" (cosmetic,
    /// no accounting). Simulation OUTPUT, so no file authors it: the recipe owns the
    /// cloud's geometry and colour, this owns what the forming bodies cut out of it.
    /// The inner worlds no longer clear rings, so the dense cloud stays continuous but
    /// for a few dramatic gaps that let starlight break through as god-rays.
    fn dust_gaps(&self) -> Vec<(f32, f32)> {
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
        gaps
    }

    /// **The per-frame channel into the recipe.** The `solarbirth_sky` stage authors the
    /// dust cloud whole and BINDS the two numbers only the simulation knows: the
    /// formation clock (the fly-in's progress, driving inside-out dissipation) and the
    /// swirl clock (a few inner-disk rotations over the fly-in). Published under the
    /// keys the recipe's `*_bind`s name — the gate below proves the two halves agree.
    fn dust_inputs(&self) -> StageInputs {
        let mut inputs = StageInputs::default();
        inputs
            .set("dust_formation", self.flight.progress())
            .set("dust_time", self.flight.progress() * 10.0)
            .gaps(self.dust_gaps());
        inputs
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

/// The stage SOURCE the 3D view node names (`solarbirth_view`'s `source`). The scene
/// FILE is the one spelling of the recipe's name — the node that reserves the viewport
/// also says what renders into it — so this reads the same string the walker does
/// instead of a Rust constant that could drift from it. The root of the tree is a
/// `surface` too, and authors no source, so this finds the nested view.
fn view_source(node: &UiNode) -> Option<&str> {
    if node.component == "surface" {
        if let Some(Value::Text(source)) = node.props.get("source") {
            return Some(source.as_str());
        }
    }
    node.children.iter().find_map(view_source)
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

        // The RECIPE, compiled ONCE: the view node names a stage source and the one
        // stage compiler turns it into the passes that run around this scene's drawing
        // (the deep-space sky behind, the dust cloud over). `stage_def` warns every
        // authoring problem it finds; a source that resolves to nothing costs the whole
        // viewport, so say so here rather than rendering an empty rect in silence.
        let stage = match self.ui_tree.as_ref().and_then(|t| view_source(t)) {
            Some(source) => flicker::ui::stage_def(&self.ui_styles, source),
            None => {
                tracing::error!(
                    "solarbirth: no `surface` node names a stage `source` — nothing says \
                     what the viewport renders"
                );
                None
            }
        };
        if stage.is_none() {
            tracing::error!("solarbirth: the viewport's stage did not compile — no 3D is drawn");
        }
        self.stage = stage;
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
        let mut pointer = None;
        if let Some(tree) = self.ui_tree.as_ref() {
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                right_down: input.mouse_right,
                screen: r.size(),
                wheel: input.mouse_wheel_delta,
                exclusive: false,
                motion: Default::default(),
            };
            let frame = run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
            over_hud = frame.results.is_on("hud_hit");
            // The walker reserves the full-screen 3D viewport's rect; `render` sizes +
            // composites the offscreen pass into it, and the camera gates to it.
            self.viewport = frame.surface_rect("solarbirth_view");
            // The pointer SAMPLE for the sky surface — the walker's barrier (A8C9F02B
            // §4b): none while the cursor is on the readout or the footer.
            pointer = frame.surface_pointer("solarbirth_view").cloned();
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
        // no-op and the flight drives the pose below. The pointer half is the walker's
        // surface sample (none over the readout/footer — the barrier).
        self.cam
            .update(pointer.as_ref(), look, zoom, !self.cinematic);
        if self.cinematic {
            // A LOOK gesture on the OPEN SKY drops out of the cinematic to the free
            // camera: an RMB-drag begun inside the viewport (not over a panel), or a
            // right-stick deflection. Left-click no longer grabs — it is reserved for
            // select-target.
            let grabbed = pointer.as_ref().is_some_and(|p| p.pressed && p.right)
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

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // Option-A graphical scene: the 3D goes into the rtt viewport the walker
        // reserved — an offscreen pass composited into that rect — and the readout
        // panels (walker commands stashed in `update`) draw over it. The RECIPE sizes
        // its own surface (its colour attachment carries the scale), so ask it before
        // the target is touched — and no stage means no 3D at all.
        let sized = match (self.viewport, self.stage.as_ref()) {
            (Some(rect), Some(stage)) => Some((rect, stage.attachments.pixels(rect.size))),
            _ => None,
        };
        if let Some((rect, (w, h))) = sized {
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
            if let (Some(target), Some(stage)) = (self.sky_target, self.stage.as_ref()) {
                // Precompute the whole sub-scene as OWNED data; the frame-graph closure
                // captures it by move, so no borrow of `self` survives into `execute`.
                let camera = self.cam.camera();
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

                let layer = fg.base_layer();
                // The offscreen sub-scene. The RECIPE draws everything around this
                // closure — the deep-space sky behind, the dust cloud over it (whose warm
                // inner glow IS the star, so the dust occludes it) — under the stage's own
                // lighting rig; the scene contributes its orbit rings and bodies, plus the
                // per-frame numbers the recipe binds. The camera stays the scene's, because
                // the maintainer flies it: the stage deliberately authors no framing.
                fg.surface(
                    CompositeTarget::Target(target),
                    stage,
                    self.dust_inputs(),
                    stage.rate,
                    move |r| {
                        r.set_camera(&camera);
                        for pts in &orbits {
                            r.draw_lines(pts, [0.30, 0.36, 0.52, 0.16]);
                        }
                        for (mesh, model, opts) in &draws {
                            r.draw_mesh(*mesh, *model, *opts);
                        }
                    },
                );
                fg.composite_panel(
                    target,
                    CompositeTarget::Screen,
                    rect,
                    layer,
                    [1.0; 4],
                    None,
                    None,
                );
            }
        }

        // The readout panels, over the composited viewport — the screen surface's final
        // 2D, declared as one overlay that runs after the composite.
        if let Some(white) = self.white {
            let hud_commands = &self.hud_commands;
            fg.overlay(move |r| render_hud(r, hud_commands, white, &[]));
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
    use flicker::render::PassKind;
    use flicker_input_core::ActionSignal;

    /// The styles root the runtime builds for THIS scene: the shared theme, its
    /// satellites (`ui_stages.json`, where the `deep_space` rig lives) and the scene
    /// file's own blocks — including its `stages` section, merged into the shared stage
    /// library exactly as `enter` merges them.
    fn shipped_styles(def: &SceneDef) -> serde_json::Value {
        flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        )
    }

    fn shipped_def() -> SceneDef {
        SceneDef::parse(
            "solarbirth",
            include_str!("../../../../content/sensorium/scenes/solarbirth.scene.json"),
        )
        .expect("solarbirth.scene.json loads")
    }

    /// **GATE — the view names a stage whose recipe draws the sky and the dust.**
    /// The cinematic used to hand-write its whole sub-scene in Rust behind a `surface`
    /// node that named NO source, which put it outside every gate that inspects authored
    /// stages. Now the node names `solarbirth_sky` and the recipe IS the picture, so
    /// this walks the real channel end to end: the shipped scene file parses, the source
    /// the node names compiles with no problems, the order derived from what each pass
    /// reads and writes is sky → scene → dust (the disk after the depth the scene
    /// writes), and every `*_bind` the recipe spells is a key the scene actually
    /// publishes — built through the same `dust_inputs` that `render` hands the graph,
    /// so a rename on either side fails here instead of silently freezing the cloud.
    #[test]
    fn the_view_names_a_stage_whose_recipe_draws_the_sky_and_the_dust() {
        let def = shipped_def();
        let styles = shipped_styles(&def);
        let tree = def.tree.clone().expect("it declares a tree");
        let source = view_source(&tree).expect("the 3D view node names a stage `source`");

        let (stage, problems) = flicker::ui::compile_stage(&styles, source)
            .unwrap_or_else(|| panic!("stages.{source} is authored"));
        assert!(
            problems.is_empty(),
            "stages.{source} has authoring problems:\n  {}",
            problems.join("\n  ")
        );

        // The EXECUTED order, derived from reads/writes — no file spells a pass number.
        let (order, cyclic) = stage.pass_order();
        assert!(!cyclic, "stages.{source}: the recipe's passes cycle");
        let kinds: Vec<&str> = order
            .iter()
            .map(|&i| stage.recipe()[i].kind.kind())
            .collect();
        assert_eq!(
            kinds,
            ["sky", "scene", "volumetric_disk", "tonemap_grade"],
            "stages.{source} must draw the sky behind the bodies and the dust over them, \
             then resolve the HDR working colour through the tonemap last"
        );

        // The dust's radii are ART numbers living in the scene file, but they are not
        // free numbers: they are DERIVED from the orrery's system extent (the file's
        // `_comment` states the derivation). Retuning the roster's outermost orbit
        // without retuning the cloud leaves a halo that no longer wraps the system, so
        // the drift fails HERE rather than showing up as a cloud edge inside the orbits.
        let PassKind::VolumetricDisk(dust) = &stage.recipe()[order[2]].kind else {
            unreachable!("pass 2 (of sky, scene, dust, tonemap) is the dust")
        };
        assert_eq!(
            dust.disk.inner,
            system::SYSTEM_INNER,
            "stages.{source} `inner` must track the orrery's SYSTEM_INNER"
        );
        assert!(
            (dust.disk.outer - SYSTEM_OUTER * 1.4).abs() < 1e-3,
            "stages.{source} `outer` must stay 1.4× the orrery's SYSTEM_OUTER \
             ({} vs {})",
            dust.disk.outer,
            SYSTEM_OUTER * 1.4
        );

        // The cinematic's grade is STATIC — an authored tint at an authored strength, with NO
        // `*_bind`. Now that the tonemap CAN be bound (the Prism Test Room's grade rides a
        // golden-hour warmth), "no binds = today's behaviour" is a claim worth pinning: this
        // resolves the shipped pass against the scene's own inputs and proves it comes back as
        // the numbers the file authors, whatever is published.
        let PassKind::TonemapGrade(grade) = &stage.recipe()[order[3]].kind else {
            unreachable!("pass 3 (of sky, scene, dust, tonemap) is the tonemap")
        };
        assert!(
            grade.binds.is_empty(),
            "the cinematic's grade is authored art, not a per-frame bind"
        );
        assert_eq!(
            grade.resolve(&Sim::new(&def).dust_inputs()),
            (Vec3::new(1.06, 1.0, 0.92), 0.12, 1.0),
            "a tonemap with no binds resolves to exactly its authored grade"
        );

        // Every per-frame value the recipe BINDS must be one the scene publishes.
        let mut bound: Vec<&str> = Vec::new();
        for pass in stage.recipe() {
            match &pass.kind {
                PassKind::VolumetricDisk(v) => {
                    bound.extend(v.binds.iter().map(|(_, key)| key.as_str()))
                }
                PassKind::GroundFog(f) => bound.extend(f.binds.iter().map(|(_, key)| key.as_str())),
                PassKind::TonemapGrade(t) => {
                    bound.extend(t.binds.iter().map(|(_, key)| key.as_str()))
                }
                _ => {}
            }
        }
        assert!(
            !bound.is_empty(),
            "stages.{source} binds nothing — the dust would never dissipate"
        );
        let sim = Sim::new(&def);
        let inputs = sim.dust_inputs();
        let published: Vec<&str> = inputs.keys().collect();
        for key in bound {
            assert!(
                published.contains(&key),
                "stages.{source} binds `{key}`, which the scene never publishes \
                 (it publishes {published:?})"
            );
        }
    }

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
        let def = shipped_def();
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

    /// DEVELOPMENT-TIER GATES (Aaron 2026-09-05, ruling 977B4D38): the hard-coded handoff
    /// conditions of a refactor — tests that read this crate's own source and assert a
    /// transition holds. `cargo test -- --skip gates::` is the production tier (every OS);
    /// `cargo test -- gates::` runs only these (one OS in CI). A gate names the transition
    /// it enforces and is deleted when that transition closes.
    mod gates {
        use super::*;

        #[test]
        fn hud_tree_is_well_formed_and_draws_the_roster() {
            use flicker::render::Vec2;
            use flicker::ui::run_ui;

            let planets = system::roster();
            let def = shipped_def();
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
                right_down: false,
                screen: Vec2::new(1920.0, 1080.0),
                wheel: 0.0,
                exclusive: false,
                motion: Default::default(),
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
            // skipped by the walker (`surface_rect` → None → the scene draws nothing), which
            // is exactly the blank-viewport bug this guards against at build time.
            let vp = frame
                .surface_rect("solarbirth_view")
                .expect("the rtt viewport reserved a slot");
            assert!(
                vp.size.x > 100.0 && vp.size.y > 100.0,
                "the viewport has real extent: {:?}",
                vp.size
            );
        }
    }
}
