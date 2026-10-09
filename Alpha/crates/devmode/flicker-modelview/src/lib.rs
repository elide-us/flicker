//! **MODEL VIEW** — the ONE "draw a mesh" scene: a `surface` node in any scene names it, and
//! it is drawn in that panel.
//!
//! Aaron, 2026-09-09 (the ruling this crate executes): *"The whole purpose of a surface is
//! to be a complete unit … surfaces are complete scene rendering objects. In the case of a
//! scene that contains a scene, the sub scene is managed by the root scene, and inherits
//! context from intent."* A quad view is therefore not four holes a bench paints into — it
//! is FOUR INSTANCES OF THIS SCENE, each handed a different `projection` param.
//!
//! What that buys, concretely: the panel's chrome (its corner view label — which is also
//! its FLIP control — and the ISOLATE row) is authored in THIS scene's own tree and drawn
//! inside THIS scene's target, so the host's composite sprite can never paint over it. Two
//! attempts to draw the same chrome from the parent tree were blank in-window for exactly
//! that reason (incident 09E5A30F: within one 2D layer the encoder paints panels, then
//! sprites, then text — the composite is a sprite at the chrome's layer).
//!
//! ## The shape
//! Thin, in the Populous/Clayworks shape: the tree is DATA (`scenes/model_view.scene.json`),
//! the pair script (`scripts/model_view.lua`) gates the two chrome slices and writes the
//! label's caption, [`ui`] holds the roster, and this file does four things a frame —
//! publish the Model, walk the tree, dispatch the frame's signals through the walker and the
//! panel, and fold the ONE results drain. The picture is a [`RigView`] declared as this
//! scene's ROOT STAGE ([`RigView::render_root`]), so under
//! [`flicker::render::FrameGraph::sub_scene`] it lands in the seating panel's target with no
//! second blit.
//!
//! ## One file, either depth
//! Aaron, 2026-09-28: *"Surface is Scene is Panel"* — the scene file sits in `scenes/` like
//! every other and does not know whether it is the root of the screen or nested in a panel.
//! NESTED: a `surface` node names it (`"scene": "model_view"` + flat `scene_<param>` props);
//! its host builds it from the manifest's def with that node's params ([`ModelView::new`]),
//! seats it, and reaches it through the TYPED channel the ruling calls "context from
//! intent" — [`set_context`](ModelView::set_context) in, [`isolate`](ModelView::isolate) /
//! [`flipped`](ModelView::flipped) / [`ray`](ModelView::ray) out. The DOCUMENT stays with the
//! host; a model view is a viewer. ROOT DEPTH: the roster factory [`scene`] builds it from
//! the file's own `params`, a full-screen model view that `Goto{"model_view"}` resolves
//! through the manifest like any scene.

pub mod ui;

use std::time::Duration;

use flicker::render::{Camera, FrameGraph, Rect, Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    render_hud, run_ui, SceneDef, SurfacePointer, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_globe::Arrows;
use flicker_input_core::{AbstractControls, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_rigview::{Draw, Projection, RigView};
use glam::Vec3;

/// The panel's isolation state — the three controls of the ISOLATE row, read back by the
/// host (which draws the cut plane in its other panels). Draw-only: no document reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Isolate {
    /// Show only the near side's limbs.
    pub limb: bool,
    /// Cut everything behind the cull plane.
    pub cull: bool,
    /// Where that cut sits: 0..1 of the subject's depth from the panel's near face.
    pub cull_at: f32,
}

impl Default for Isolate {
    fn default() -> Self {
        Self {
            limb: false,
            cull: false,
            // A slider at 1.0 cuts nothing, so ticking CULL never blanks the panel.
            cull_at: 1.0,
        }
    }
}

/// WHAT THE HOST HANDS THE PANEL each frame — the typed "context from intent" channel.
///
/// Everything here is the HOST's: it owns the document, composes the geometry, and decides
/// what this instance should be looking at. The panel owns only how it is looked at (its
/// camera, its flip, its cut) and its own chrome.
#[derive(Clone, Debug)]
pub struct ViewContext {
    /// This frame's draw items, over mesh handles the HOST uploaded and frees.
    pub draws: Vec<Draw>,
    /// Depth-tested line batches — the ground grid, collision volumes.
    pub lines: Arrows,
    /// Line batches drawn over the meshes without a depth test — the skeleton, the
    /// selection ball, the gizmo handles.
    pub overlay: Arrows,
    /// The framed subject's centre and bounding radius…
    pub centre: Vec3,
    pub radius: f32,
    /// …and its axis-aligned half extents, which is the depth the cut slider spans.
    pub half_extent: Vec3,
    /// A backdrop override, or `None` for the stage's authored clear.
    pub clear: Option<[f64; 4]>,
    /// The pad's look tuple (see `RigView::look_from`).
    pub look: (f32, f32, f32),
    /// The player's analog sensitivities, when the host has fresh ones to hand over.
    pub controls: Option<AbstractControls>,
    /// The HOST's manipulator has this panel's pointer this frame — hold the camera
    /// still. A bench that drags a joint in a panel means the drag, not an orbit; the
    /// panel still SEES the pointer (its chrome, its [`ray`](ModelView::ray) and
    /// [`pointer`](ModelView::pointer) are what the host's gizmo reads), it just does not
    /// move its own camera with it. `false` — the ordinary case — leaves the camera the
    /// panel's own.
    pub camera_held: bool,
}

impl Default for ViewContext {
    fn default() -> Self {
        Self {
            draws: Vec::new(),
            lines: Vec::new(),
            overlay: Vec::new(),
            centre: Vec3::ZERO,
            radius: 1.0,
            half_extent: Vec3::ONE,
            clear: None,
            look: (0.0, 0.0, 0.0),
            controls: None,
            camera_held: false,
        }
    }
}

/// One panel of a model view — see the module docs.
pub struct ModelView {
    /// The authored tree, walked every frame.
    tree: UiNode,
    script: ScriptHost,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    ui_intents: UiIntents,
    hud_commands: Vec<HudCommand>,
    /// The HOST's theme textures — a nested surface's scene never builds a `Theme` of its
    /// own (there is one per application, and the host already owns it).
    textures: Vec<TextureHandle>,
    /// The picture: the rig view this scene declares as its root stage.
    rig: RigView,
    /// The host's params, echoed into the Model so the script's gates read one source.
    chrome: bool,
    label: bool,
    /// The isolate row's committed values (rule 3A04B4CE: every bound control round-trips).
    isolate: Isolate,
    /// The framing already applied, so a re-frame (which resets the pan) happens only when
    /// the subject actually moved or resized — never every frame under the user's hand.
    framed: Option<(Vec3, f32, Vec3)>,
    /// This frame's root pointer sample, kept for [`Self::ray`] / [`Self::pointer`].
    pointer: Option<SurfacePointer>,
    /// The pad's look tuple the host last handed over.
    look: (f32, f32, f32),
    /// The host's manipulator holds this panel's camera this frame — see
    /// [`ViewContext::camera_held`].
    camera_held: bool,
}

impl ModelView {
    /// Build a panel from the manifest's parsed def and the HOST's per-instance params (the
    /// `scene_<name>` props on the `surface` node that names this scene, keyed by `<name>`;
    /// empty at root depth). The pair script ships with the crate and loads here, exactly as
    /// every bench loads its own.
    ///
    /// A param the node does not name falls back to the scene file's `params` block, then to
    /// the compiled default: `projection` = the perspective picker, `label` = on, and
    /// `chrome` = whether the projection is orthographic (an isolate row means nothing on the
    /// picker, which never cuts).
    pub fn new(def: &SceneDef, params: &ValueMap) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let tree = def
            .tree
            .clone()
            .expect("model_view.scene.json declares a tree");
        let ui_intents = UiIntents::of(&tree);
        let script = ScriptHost::new(ui::SCRIPT, ui::SCRIPT_NAME)
            .expect("model_view.lua loads (it ships with the crate)");
        let name = text_param(params, def, ui::P_PROJECTION).unwrap_or("persp");
        let projection = ui::projection_of(name).unwrap_or_else(|| {
            tracing::warn!(
                "model_view: `projection` = '{name}' is not one of {:?} — this panel falls \
                 back to the perspective picker, so the host is showing the wrong view",
                ui::PROJECTIONS.map(|(n, _)| n)
            );
            Projection::Perspective
        });
        Self {
            rig: RigView::new(ui::STAGE, &ui_styles, projection),
            chrome: bool_param(params, def, ui::P_CHROME).unwrap_or(projection.is_ortho()),
            label: bool_param(params, def, ui::P_LABEL).unwrap_or(true),
            tree,
            script,
            ui_styles,
            ui_state: UiState::default(),
            ui_intents,
            hud_commands: Vec::new(),
            textures: Vec::new(),
            isolate: Isolate::default(),
            framed: None,
            pointer: None,
            look: (0.0, 0.0, 0.0),
            camera_held: false,
        }
    }

    // ── The typed channel: context IN ───────────────────────────────────────

    /// Hand the panel what it should be looking at this frame. See [`ViewContext`].
    ///
    /// Call it BEFORE the frame's `update`: the framing is what gives the camera — and so
    /// [`ray`](Self::ray) and [`cull_plane`](Self::cull_plane), which the host reads back
    /// the same frame — the subject they are measured against.
    pub fn set_context(&mut self, ctx: ViewContext) {
        let ViewContext {
            draws,
            lines,
            overlay,
            centre,
            radius,
            half_extent,
            clear,
            look,
            controls,
            camera_held,
        } = ctx;
        self.rig.set_draws(draws);
        self.rig.set_lines(lines);
        self.rig.set_overlay(overlay);
        // `set_frame` RESETS the pan, so re-framing every frame would fight the user's own
        // panning: it happens only when the subject the host is showing actually changed.
        let framing = (centre, radius, half_extent);
        if self.framed != Some(framing) {
            self.rig.set_frame(centre, radius);
            self.rig.set_extent(half_extent);
            self.framed = Some(framing);
        }
        self.rig.set_clear(clear);
        if let Some(controls) = controls {
            self.rig.set_controls(controls);
        }
        self.look = look;
        self.camera_held = camera_held;
    }

    /// This frame's draw items on their own — the ONE piece of [`ViewContext`] a host may
    /// hand over after `update`, because uploading a mesh needs `&mut Renderer` and that
    /// only exists in [`Scene::render`]. Same seam, same field: a host with its draws
    /// ready at context time simply fills [`ViewContext::draws`] instead.
    pub fn set_draws(&mut self, draws: Vec<Draw>) {
        self.rig.set_draws(draws);
    }

    /// FRAME a part of the subject: pan and zoom onto a ball of `radius` at `centre`
    /// without disturbing the subject framing [`set_context`](Self::set_context) applies,
    /// so the next re-frame still returns to the whole subject. The host's verb — a bench
    /// frames the selected joint in every panel at once.
    pub fn focus(&mut self, centre: Vec3, radius: f32) {
        self.rig.focus(centre, radius);
    }

    /// The HOST's theme textures, for the panel's own chrome. A nested surface's scene never
    /// builds a `Theme`: there is one per application and the host already owns it.
    pub fn set_textures(&mut self, textures: Vec<TextureHandle>) {
        self.textures = textures;
    }

    // ── The typed channel: results OUT ──────────────────────────────────────

    /// The isolate row's committed values.
    #[must_use]
    pub fn isolate(&self) -> Isolate {
        self.isolate
    }

    /// Whether this panel views from the opposite side (BOTTOM / RIGHT / BACK).
    #[must_use]
    pub fn flipped(&self) -> bool {
        self.rig.flipped()
    }

    #[must_use]
    pub fn projection(&self) -> Projection {
        self.rig.projection()
    }

    /// The live cut as a WORLD plane `(normal, d)` — a point `x` is culled when
    /// `normal · x > d`. The host's OTHER panels see this plane edge-on and draw it.
    #[must_use]
    pub fn cull_plane(&self) -> Option<(Vec3, f32)> {
        self.rig.cull_plane()
    }

    /// A world-space ray through this frame's pointer, for the host's picking. `None` while
    /// the cursor is elsewhere or this panel's own chrome claimed it.
    #[must_use]
    pub fn ray(&self) -> Option<(Vec3, Vec3)> {
        self.rig.ray_at(self.pointer.as_ref())
    }

    /// This frame's pointer sample for the panel itself — what the host's gizmo reads to
    /// tell a press from a drag.
    #[must_use]
    pub fn pointer(&self) -> Option<&SurfacePointer> {
        self.pointer.as_ref()
    }

    /// The panel's camera — the host projects with it (marker placement, screen-space hit
    /// sizes) rather than re-deriving one that could drift from the picture.
    #[must_use]
    pub fn camera(&self) -> Camera {
        self.rig.camera()
    }

    // ── The frame ───────────────────────────────────────────────────────────

    /// This frame's Model: the raw runtime facts, then the script's `derive()` (the corner
    /// label's caption) and `arrange()` (the two chrome gates) folded in.
    fn publish(&self) -> ValueMap {
        let mut m = ValueMap::new();
        m.set(ui::M_PROJECTION, ui::projection_name(self.rig.projection()));
        m.set(ui::M_FLIPPED, self.rig.flipped());
        m.set(ui::M_CHROME, self.chrome);
        m.set(ui::M_LABEL, self.label);
        // The bound controls echo their committed values back (rule 3A04B4CE).
        m.set(ui::LIMB, self.isolate.limb);
        m.set(ui::CULL, self.isolate.cull);
        m.set(ui::CULL_AT, self.isolate.cull_at);
        if let Err(e) = self.script.set_model(&m) {
            tracing::error!("model_view: publishing the model to the script failed: {e}");
        }
        match self.script.derive() {
            Ok(Some(derived)) => m.extend(derived),
            Ok(None) => {}
            Err(e) => tracing::error!("model_view.lua derive() failed: {e}"),
        }
        match self.script.arrange() {
            Ok(Some(arrangement)) => m.extend(arrangement.to_model()),
            Ok(None) => {}
            Err(e) => tracing::error!("model_view.lua arrange() failed: {e}"),
        }
        m
    }

    /// Fold this frame's ONE results drain: the isolate row's committed values (so each
    /// control re-publishes what the user just set), and the corner label's flip.
    fn apply_results(&mut self, results: &ValueMap) {
        if results.get(ui::LIMB).is_some() {
            self.isolate.limb = results.is_on(ui::LIMB);
        }
        if results.get(ui::CULL).is_some() {
            self.isolate.cull = results.is_on(ui::CULL);
        }
        if let Some(at) = results.number(ui::CULL_AT) {
            self.isolate.cull_at = at as f32;
        }
        if results.is_on(ui::FLIP) {
            self.rig.flip();
        }
    }
}

impl Scene for ModelView {
    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        // Nested in a `surface`, this scene's "screen" is its PANEL: the host's `SubScene`
        // wrapper has already made the pointer local and reports the seat's size here. At
        // root depth there is no seat, and the window is the screen.
        let viewport = signals.viewport_or(renderer.size());
        let model = self.publish();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen: viewport,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(
            &self.tree,
            &model,
            &self.ui_styles,
            &snap,
            &mut self.ui_state,
        );
        let over_hud = frame.results.is_on("hud_hit");
        // The ROOT pointer is the panel's: the walker's barrier hands it over only when no
        // chrome claimed the cursor, which is what keeps a press on the isolate row out of
        // the camera (and out of the host's picking).
        self.pointer = frame.root_pointer().cloned();
        self.hud_commands = frame.commands;

        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&self.tree, &model)
            .with_intents(&self.ui_intents);
        {
            // The panel sits BELOW the walker: navigation is decided first, and what is
            // left of the look/zoom signals is the camera's. Signals only reach a nested
            // surface's scene while its surface is the focused pane, so this is already gated.
            let mut chain: [&mut dyn InputHandler; 2] = [&mut walker, &mut self.rig];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let mut results = frame.results;
        for name in walker.take_fired() {
            results.set(name, true);
        }
        drop(walker);
        self.apply_results(&results);

        // The panel fills its scene's whole screen, which IS the host's slot: seating it at
        // the viewport is what gives the camera its aspect and `ray()` its pixels.
        self.rig.seat_at(
            Rect {
                pos: Vec2::ZERO,
                size: viewport,
            },
            0.0,
            [1.0; 4],
        );
        self.rig
            .set_cull(self.isolate.cull.then_some(self.isolate.cull_at));
        // `focused: None` with no pane declared: a model view OWNS its camera — the host
        // already decided this scene may hear anything at all. The one exception is a
        // pointer the HOST's manipulator has taken (`camera_held`): the panel still holds
        // the sample for its chrome and for the host's `ray()`, but its camera does not
        // move with a drag that means something else.
        let camera_pointer = self.pointer.as_ref().filter(|_| !self.camera_held);
        self.rig
            .update(dt.as_secs_f32(), camera_pointer, self.look, None);
        Transition::None
    }

    fn render<'f>(&'f mut self, _renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        let Self {
            rig,
            hud_commands,
            textures,
            ..
        } = self;
        // Declared exactly as a top-level scene declares its root surface and its HUD; the
        // host's `FrameGraph::sub_scene` scope is what lands both in the panel's target, the
        // chrome over the picture, inside ONE texture.
        rig.render_root(fg);
        if let Some(&white) = textures.first() {
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.rig.free(renderer);
    }
}

/// One TEXT param: the host's node wins, else the scene file's `params` default.
fn text_param<'a>(params: &'a ValueMap, def: &'a SceneDef, key: &str) -> Option<&'a str> {
    params.text(key).or_else(|| def.param_str(key))
}

/// One BOOLEAN param, the same way round. `None` leaves the compiled default standing.
fn bool_param(params: &ValueMap, def: &SceneDef, key: &str) -> Option<bool> {
    match params.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => def.params.get(key).and_then(serde_json::Value::as_bool),
    }
}

/// Build a full-screen model view as a boxed `Scene` — the manifest resolves
/// `model_view.scene.json` and hands its def here. No `surface` node names it at this depth,
/// so every knob comes from the file's own `params`.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(ModelView::new(def, &ValueMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel a host would seat, built from the SHIPPED pair with the params a
    /// `surface` node would carry.
    fn panel(pairs: &[(&str, Value)]) -> ModelView {
        let def = SceneDef::parse(ui::ID, ui::SCENE).expect("the shipped scene parses");
        let mut params = ValueMap::new();
        for (k, v) in pairs {
            params.set(*k, v.clone());
        }
        ModelView::new(&def, &params)
    }

    /// An orthographic panel with all its chrome asked for — the quad view's TOP.
    fn ortho(projection: &str) -> ModelView {
        panel(&[
            ("projection", Value::Text(projection.into())),
            ("chrome", Value::Bool(true)),
            ("label", Value::Bool(true)),
        ])
    }

    /// Walk the panel headlessly at `screen`, with the pointer where it is told.
    fn walk_at(
        p: &mut ModelView,
        screen: Vec2,
        mouse: Vec2,
        clicked: bool,
        down: bool,
    ) -> flicker::ui::UiFrame {
        let model = p.publish();
        let snap = UiInput {
            mouse,
            clicked,
            down,
            right_down: false,
            screen,
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        run_ui(&p.tree, &model, &p.ui_styles, &snap, &mut p.ui_state)
    }

    /// A 300-square panel with the pointer parked off it — the quad-view cell's size.
    const CELL: Vec2 = Vec2::new(300.0, 300.0);
    fn walk(p: &mut ModelView) -> flicker::ui::UiFrame {
        walk_at(p, CELL, Vec2::new(-1.0, -1.0), false, false)
    }

    /// THE SHIPPED PAIR LOADS — both halves, and through the real runtime path: the scene
    /// file parses with the one `stages.rig` recipe, the script satisfies the module
    /// contract, the roster factory builds the scene at root depth, and a panel no node
    /// configures takes the file's own `params`.
    #[test]
    fn the_shipped_pair_parses_and_loads() {
        let def = SceneDef::parse(ui::ID, ui::SCENE).expect("the shipped scene parses");
        assert_eq!(def.behaviour, ui::ID, "the file names this behaviour");
        assert!(!def.boot, "the model view is never the boot scene");
        assert!(def.tree.is_some(), "it authors a tree");
        assert!(
            def.stages().is_some_and(|s| s.contains_key(ui::STAGE)),
            "the ONE stage the panel draws under is `stages.{}`",
            ui::STAGE
        );
        ScriptHost::new(ui::SCRIPT, ui::SCRIPT_NAME).expect("the pair script loads");
        let _root: Box<dyn Scene> = scene(&def);
        assert_eq!(
            ModelView::new(&def, &ValueMap::new()).projection(),
            Projection::Perspective,
            "the file's `params` default is the picker"
        );
    }

    /// `arrange()` LIGHTS THE CHROME ON THE ORTHOGRAPHIC PANELS ONLY: top / side / front
    /// carry the isolate row and the corner label, the perspective picker carries neither
    /// (it never cuts and has no opposite side) — and a host that asks for no chrome gets
    /// none even on an ortho panel.
    #[test]
    fn arrange_lights_the_chrome_on_the_orthographic_panels_only() {
        for (name, projection) in ui::PROJECTIONS {
            let m = ortho(name).publish();
            assert_eq!(
                m.is_on(ui::CHROME_ON),
                projection.is_ortho(),
                "{name}: the isolate row"
            );
            assert_eq!(
                m.is_on(ui::LABEL_ON),
                projection.is_ortho(),
                "{name}: the corner label"
            );
        }
        let m = panel(&[
            ("projection", Value::Text("top".into())),
            ("chrome", Value::Bool(false)),
            ("label", Value::Bool(false)),
        ])
        .publish();
        assert!(!m.is_on(ui::CHROME_ON), "the host asked for no isolate row");
        assert!(!m.is_on(ui::LABEL_ON), "the host asked for no label");
    }

    /// `derive()` WRITES THE CORNER LABEL and the flip turns it over: TOP↔BOTTOM,
    /// LEFT↔RIGHT, FRONT↔BACK. The perspective picker has no side, so it captions nothing.
    #[test]
    fn derive_flips_the_corner_label() {
        for (name, near, far) in [
            ("top", "$mv_top", "$mv_bottom"),
            ("side", "$mv_left", "$mv_right"),
            ("front", "$mv_front", "$mv_back"),
        ] {
            let mut p = ortho(name);
            assert_eq!(p.publish().text(ui::VIEW_LABEL), Some(near), "{name}");
            p.rig.flip();
            assert!(p.flipped(), "{name} views the other side");
            assert_eq!(
                p.publish().text(ui::VIEW_LABEL),
                Some(far),
                "{name} flipped"
            );
        }
        let mut p = ortho("persp");
        p.rig.flip();
        assert!(!p.flipped(), "the picker has no other side");
        assert_eq!(
            p.publish().text(ui::VIEW_LABEL),
            Some(""),
            "and so captions nothing"
        );
    }

    /// THE PARAMS BUILD THE PANEL: a text `projection` picks the camera, the two booleans
    /// parse, an unknown projection WARNS and falls back to the picker rather than guessing,
    /// and an unstated `chrome` defaults to "whether this panel is orthographic".
    #[test]
    fn the_params_build_the_panel_and_an_unknown_projection_falls_back() {
        for (name, projection) in ui::PROJECTIONS {
            let p = panel(&[("projection", Value::Text(name.into()))]);
            assert_eq!(p.projection(), projection, "{name}");
            assert_eq!(
                p.chrome,
                projection.is_ortho(),
                "{name}: an unstated `chrome` follows the projection"
            );
            assert!(p.label, "the file's `params` default the label on");
        }
        let p = panel(&[("projection", Value::Text("isometric".into()))]);
        assert_eq!(
            p.projection(),
            Projection::Perspective,
            "an unknown projection falls back to the picker"
        );
        let p = panel(&[
            ("projection", Value::Text("front".into())),
            ("chrome", Value::Bool(false)),
            ("label", Value::Bool(false)),
        ]);
        assert!(!p.chrome && !p.label, "the host's booleans win");
    }

    /// THE CHROME IS LAID OUT INSIDE THE PANEL, WITH REAL EXTENT (the containment invariant
    /// this scene inherited from Clayworks, 380BDCC8 — where it could not be honoured,
    /// because the chrome lived in the host's tree under the panel's composite).
    ///
    /// At a 300-square quad cell every control resolves to a box that is visible, hittable
    /// and wholly inside the panel; and a panel whose host asked for no chrome lays out
    /// none at all.
    #[test]
    fn the_chrome_lands_inside_the_panel_with_real_extent() {
        let mut p = ortho("top");
        let frame = walk(&mut p);
        for id in [
            ui::NODE_FLIP,
            ui::NODE_ISOLATE,
            ui::NODE_LIMB,
            ui::NODE_CULL,
            ui::NODE_CULL_AT,
        ] {
            let r = frame
                .rect(id)
                .unwrap_or_else(|| panic!("`{id}` is laid out"));
            assert!(
                r.size.x > 8.0 && r.size.y > 6.0,
                "`{id}` has extent: {:?}",
                r.size
            );
            assert!(
                r.pos.x >= 0.0
                    && r.pos.y >= 0.0
                    && r.pos.x + r.size.x <= CELL.x
                    && r.pos.y + r.size.y <= CELL.y,
                "`{id}` is inside the panel: {:?} + {:?} in {CELL:?}",
                r.pos,
                r.size
            );
        }
        let mut bare = panel(&[
            ("projection", Value::Text("top".into())),
            ("chrome", Value::Bool(false)),
            ("label", Value::Bool(false)),
        ]);
        let frame = walk(&mut bare);
        for id in [ui::NODE_FLIP, ui::NODE_ISOLATE, ui::NODE_LIMB] {
            assert!(frame.rect(id).is_none(), "`{id}` is gated off");
        }
    }

    /// THE CORNER LABEL IS THE FLIP CONTROL: a tap on it turns the panel over, the caption
    /// follows, and the camera crosses to the other side.
    #[test]
    fn a_tap_on_the_corner_label_flips_the_panel() {
        let mut p = ortho("front");
        p.set_context(ViewContext {
            centre: Vec3::new(0.0, 0.0, 90.0),
            radius: 90.0,
            half_extent: Vec3::splat(90.0),
            ..Default::default()
        });
        p.rig.seat_at(
            Rect {
                pos: Vec2::ZERO,
                size: CELL,
            },
            0.0,
            [1.0; 4],
        );
        let front = p.camera().position;
        let r = walk(&mut p)
            .rect(ui::NODE_FLIP)
            .expect("the label is laid out");
        let on_label = r.pos + r.size * 0.5;
        let _ = walk_at(&mut p, CELL, on_label, false, false);
        let tapped = walk_at(&mut p, CELL, on_label, true, true);
        assert!(tapped.results.is_on(ui::FLIP), "the tap fires `flip`");
        p.apply_results(&tapped.results);
        assert!(p.flipped(), "the panel views the other side");
        assert_eq!(p.publish().text(ui::VIEW_LABEL), Some("$mv_back"));
        let back = p.camera().position;
        assert!(
            (front.y + back.y).abs() < 1e-3 && front.y != back.y,
            "the camera crossed through the look-at: {front} → {back}"
        );
    }

    /// A HELD CAMERA DOES NOT ORBIT ON THE HOST'S DRAG, and still hands the host the
    /// sample it is dragging with. `camera_held` is how a bench says "this pointer is my
    /// manipulator's this frame": the panel keeps `pointer()` / `ray()` (the gizmo reads
    /// them) and stops moving its own camera with the same travel.
    #[test]
    fn a_held_camera_keeps_the_pointer_but_does_not_orbit() {
        let drag = SurfacePointer {
            id: "ap_view_persp".into(),
            root: true,
            cursor: Vec2::new(150.0, 150.0),
            local: Vec2::new(150.0, 150.0),
            delta: Vec2::new(40.0, 0.0),
            left: true,
            right: false,
            pressed: false,
            wheel: 0.0,
            captured: true,
            rect: Rect {
                pos: Vec2::ZERO,
                size: CELL,
            },
        };
        // The same drag, once with the camera free and once with it held.
        let mut moved = Vec3::ZERO;
        for (held, label) in [(false, "free"), (true, "held")] {
            let mut p = ortho("persp");
            p.set_context(ViewContext {
                centre: Vec3::ZERO,
                radius: 90.0,
                half_extent: Vec3::splat(90.0),
                camera_held: held,
                ..Default::default()
            });
            p.rig.seat_at(
                Rect {
                    pos: Vec2::ZERO,
                    size: CELL,
                },
                0.0,
                [1.0; 4],
            );
            let before = p.camera().position;
            p.pointer = Some(drag.clone());
            p.camera_held = held;
            p.rig
                .update(0.016, p.pointer.as_ref().filter(|_| !held), p.look, None);
            let after = p.camera().position;
            assert!(
                p.pointer().is_some() && p.ray().is_some(),
                "{label}: the panel keeps the sample either way"
            );
            if held {
                assert_eq!(after, before, "a held camera stands still");
                assert_ne!(
                    moved,
                    Vec3::ZERO,
                    "the free camera is what it is held against"
                );
            } else {
                moved = after - before;
                assert_ne!(after, before, "a free camera orbits on the same drag");
            }
        }
    }

    /// THE BINDS ROUND-TRIP (rule 3A04B4CE): ticking LIMB through the real tree lands in the
    /// panel's own state, and the next publish echoes it back so the control shows it.
    #[test]
    fn the_isolate_binds_echo_back_into_the_model() {
        let mut p = ortho("side");
        assert!(!p.publish().is_on(ui::LIMB), "it starts clear");
        let r = walk(&mut p)
            .rect(ui::NODE_LIMB)
            .expect("the LIMB checkbox is laid out");
        let on_box = r.pos + Vec2::new(8.0, r.size.y * 0.5);
        let _ = walk_at(&mut p, CELL, on_box, false, false);
        let ticked = walk_at(&mut p, CELL, on_box, true, true);
        p.apply_results(&ticked.results);
        assert!(p.isolate().limb, "the tick reached the panel's state");
        assert!(
            p.publish().is_on(ui::LIMB),
            "and the next publish echoes it back to the control"
        );
        // The cut travels the same channel: a committed slider value becomes the panel's.
        let mut results = ValueMap::new();
        results.set(ui::CULL, true);
        results.set(ui::CULL_AT, 0.25_f64);
        p.apply_results(&results);
        assert_eq!(p.isolate().cull_at, 0.25);
        p.rig
            .set_cull(p.isolate().cull.then_some(p.isolate().cull_at));
        assert!(p.cull_plane().is_some(), "an active cut reports its plane");
    }

    /// EVERY CAPTION THE PAIR SHIPS IS IN THE STRINGTABLE — the drift gate for display copy
    /// the SCRIPT names: a `$token` with no entry renders raw, and no walker can warn about
    /// a caption that only exists inside a Lua table.
    #[test]
    fn every_caption_token_the_pair_names_is_in_the_stringtable() {
        let table: serde_json::Value =
            serde_json::from_str(include_str!("../../../../content/data/stringtable.json"))
                .expect("the stringtable is JSON");
        let mut tokens: Vec<String> = Vec::new();
        // The SCRIPT names captions and nothing else, so every `$…` in it is one.
        let mut rest = ui::SCRIPT;
        while let Some(at) = rest.find('$') {
            rest = &rest[at + 1..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            if end > 0 {
                tokens.push(rest[..end].to_string());
            }
        }
        // The TREE's `$…` are mostly theme tokens (a style block's colours); only the
        // display props are captions.
        fn captions(node: &serde_json::Value, out: &mut Vec<String>) {
            for key in ["label", "text", "prefix"] {
                if let Some(v) = node.get(key).and_then(|v| v.as_str()) {
                    if let Some(t) = v.strip_prefix('$') {
                        out.push(t.to_string());
                    }
                }
            }
            for child in node
                .get("children")
                .and_then(|c| c.as_array())
                .unwrap_or(&Vec::new())
            {
                captions(child, out);
            }
        }
        let scene: serde_json::Value =
            serde_json::from_str(ui::SCENE).expect("the shipped scene is JSON");
        captions(&scene["tree"], &mut tokens);
        assert!(tokens.len() >= 8, "the pair names its captions: {tokens:?}");
        for token in tokens {
            assert!(
                table.get(&token).is_some(),
                "`${token}` has no stringtable entry — it would render raw"
            );
        }
    }
}
