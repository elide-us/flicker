//! **The hex world as ONE component.** A bench asks for a globe, hands it what
//! the planet is made of, tells it where on screen it lives, and puts it in the
//! input chain. Everything else — the meshes, the offscreen target, the authored
//! stage, the camera, and which signals the camera is allowed to hear — belongs
//! to this object.
//!
//! It exists because both benches that draw a planet had grown their own copy of
//! the same seven fields (`cam` · `arrows` · `globe_rect` · `globe_view` ·
//! `stage` + a mesh list + a stale flag) and their own `render` prologue to go
//! with them. Two copies of one picture is how the two pictures drift; and the
//! camera half had drifted already — Populous reached past the input map for
//! `gamepad(0).right_stick()` and applied the deadzone a second time, which is
//! the shape of bug you only find by measuring.
//!
//! # The shell LIST is the interface
//!
//! [`ShellSpec`] describes one shell: a tiling, a radius, an inset, and a colour
//! per cell. God Mode publishes N sparse ones (core, mantle, two crust beds, a
//! veil per gas) rebuilt whenever the simulation advances; Populous publishes
//! two static ones and never touches them again. Same call, same object — the
//! difference is the LIST, not the code.
//!
//! # The camera asks for a SIGNAL
//!
//! `Look*` and `Zoom*` are bound in the shared input map like everything else,
//! and this world reads them through [`ContextualBindings`] — never off a
//! device. It answers them only while its own panel holds the walker's focus, so
//! the same stick that flies the planet is panel navigation everywhere else, and
//! the deflection runs through the player's own `AbstractControls` so the
//! settings panel reaches the globe.

use flicker::render::{
    Camera, FrameGraph, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Rect, Renderer,
    StageDef, StageLayer, Vec3,
};
use flicker::ui::{SurfacePointer, SurfaceSlot};
use flicker_input_core::{AbstractControls, ActionSignal};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

use crate::view::{Arrows, GlobeView, Seat, GLOBE_LAYERS};

/// One baked shell set's CPU meshes, each with its draw options, waiting for
/// a renderer.
type BakedSet = Vec<(Vec<MeshVertex>, Vec<u32>, MeshDrawOptions)>;
use crate::{build, graticule, OrbitCam, RADIUS};

/// The set name [`GlobeWorld::set_shells`] bakes into — the whole world of a
/// single-view caller.
pub const DEFAULT_SET: &str = "default";

/// One shell of a world: what it is drawn over, how far out, how far its tiles
/// pull in, and what colour each cell is. `color` returning `None` leaves a hole
/// — that is how a crust shell exists only where there is crust.
pub struct ShellSpec<'a> {
    pub dirs: &'a [Vec3],
    pub outlines: &'a [Vec<Vec3>],
    pub radius: f32,
    pub inset: f32,
    pub color: Box<dyn Fn(usize) -> Option<[f32; 3]> + 'a>,
    /// **Per-cell radius**, overriding `radius` where it answers. A shell whose
    /// top follows each column's OWN accumulated stack height rather than one
    /// sphere — a crust that stands proud where it is thick, an ocean that lies
    /// where it is deep. `None` (the ordinary case) is the sphere at `radius`.
    ///
    /// This is the variant `flicker-pocepochs` carried in a private `globe.rs`
    /// until it was absorbed; the second framing lives on the ONE shell spec so
    /// there is nothing left to fork.
    pub cell_radius: Option<Box<dyn Fn(usize) -> f32 + 'a>>,
    /// **How this shell is DRAWN**: the default is the opaque lit fill every
    /// ground shell uses. A WATER shell sets `tint` with an alpha below 1 —
    /// the mesh pipeline alpha-blends — and some `gloss` for the wet sheen.
    /// Blending has no sort: order translucent shells LAST in the list,
    /// bottom-up (deep before shallow before surface).
    pub opts: MeshDrawOptions,
    /// **Column depth**, making the shell VOLUMETRIC: each cell becomes a
    /// CLOSED solid — top face at the cell's radius, bottom face `depth(i)`
    /// beneath it, side walls between, their edges along the corner directions
    /// (straight lines out from the centre of the world, so the top hex is
    /// naturally a little wider than the bottom). `None` — the ordinary case —
    /// is the cap-only shell every planet view draws. Per-cell like
    /// `cell_radius`, because a layer's THICKNESS is per-column physics: this
    /// is the hex-stack ledger's drawing form.
    pub depth: Option<Box<dyn Fn(usize) -> f32 + 'a>>,
}

/// **A globe, whole.** The mesh stack, the offscreen target, the authored stage,
/// the orbit camera, and the rect the walker reserved for it.
pub struct GlobeWorld {
    view: GlobeView,
    cam: OrbitCam,
    stage: StageDef,
    /// Uploaded shell SETS, by name, each in draw order — a world can hold
    /// several baked views of one planet (a bench's data tabs) and swap
    /// between them for free. A caller that never names a set lives entirely
    /// in [`DEFAULT_SET`] through [`set_shells`](GlobeWorld::set_shells).
    sets: std::collections::HashMap<String, Vec<(MeshHandle, MeshDrawOptions)>>,
    /// Which set draws.
    active: String,
    /// CPU meshes baked and not yet uploaded, per set. Built at
    /// [`bake`](GlobeWorld::bake) time so the caller needs no renderer,
    /// uploaded at the top of `render` where one exists.
    pending: Vec<(String, BakedSet)>,
    /// The stage's own line geometry — the authored graticule, built once.
    stage_arrows: Arrows,
    /// What is actually drawn: the stage's frame plus whatever the scene added.
    arrows: Arrows,
    /// Where — and how — the walker seated this globe's surface this frame; `None`
    /// while off screen (or for a ROOT world), which is also what stops the offscreen
    /// pass from costing anything.
    seat: Option<Seat>,
    /// The scene's own "my data moved" flag, held here so a bench keeps no
    /// second copy of it.
    dirty: bool,
    /// The `tab_group` of the panel this world lives in. The camera answers the
    /// look signals only while the WALKER's cursor is on that panel; a world
    /// that names no panel is pointer-flown only.
    panel: Option<String>,
    owns_camera: bool,
    /// The live look deflection this frame — computed by the SCENE from its signal
    /// source (the pump's `signals.axis`, or a bench's own `bindings.signal_axis`) and
    /// handed to [`update`](GlobeWorld::update). The six Look/Zoom signals stay the
    /// camera's knowledge (see [`look_from`](GlobeWorld::look_from)); the deadzone lives
    /// on the source now, not here.
    look: (f32, f32, f32),
}

impl GlobeWorld {
    /// A world authored by `stages.<source>` in the loaded styles, opening with the
    /// planet filling `fill` of the viewport's height (`None` keeps the camera's plain
    /// three-radii framing). An unauthored source still shows a picture — the scene's
    /// own shells, default-lit — because a typo in a style file should cost the
    /// authored look, never the planet (the compiler has already warned).
    pub fn new(source: &str, styles: &serde_json::Value, fill: Option<f32>) -> Self {
        let stage = flicker::ui::stage_def(styles, source).unwrap_or_else(|| {
            tracing::warn!("stages.{source}: the globe shows the scene's own shells instead");
            StageDef {
                layers: vec![StageLayer::Shells],
                ..StageDef::default()
            }
        });
        let undrawn = stage.layers_outside(GLOBE_LAYERS);
        if !undrawn.is_empty() {
            tracing::warn!("stages.{source} authors {undrawn:?} layers a globe does not draw");
        }
        let mut cam = OrbitCam::new(RADIUS);
        if let Some(fill) = fill {
            cam = cam.with_fill(fill);
        }
        // The authored reference frame, built ONCE: a graticule is a function of
        // the radius alone, and the radius is a constant.
        let stage_arrows: Arrows = stage
            .layers
            .iter()
            .filter_map(|l| match l {
                StageLayer::Graticule { radius_scale } => Some(graticule(RADIUS * radius_scale)),
                _ => None,
            })
            .flatten()
            .collect();
        Self {
            view: GlobeView::default(),
            cam,
            stage,
            sets: std::collections::HashMap::new(),
            active: DEFAULT_SET.to_string(),
            pending: Vec::new(),
            arrows: stage_arrows.clone(),
            stage_arrows,
            seat: None,
            dirty: false,
            panel: None,
            owns_camera: false,
            look: (0.0, 0.0, 0.0),
        }
    }

    /// Name the panel whose focus hands this world the camera (builder form).
    pub fn in_panel(mut self, tab_group: impl Into<String>) -> Self {
        self.panel = Some(tab_group.into());
        self
    }

    /// The authored look this world was built from — what a bench reads instead
    /// of writing its own colours down.
    pub fn stage(&self) -> &StageDef {
        &self.stage
    }

    /// The stage's own STATIC shells, materialized over one tiling: the author
    /// decided the radius, the inset and the colour; the caller supplies only
    /// the world's geometry. A stage with no `shell` layers returns nothing,
    /// which is exactly right for a bench whose shells come from a simulation.
    pub fn authored_shells<'a>(
        &self,
        dirs: &'a [Vec3],
        outlines: &'a [Vec<Vec3>],
    ) -> Vec<ShellSpec<'a>> {
        self.stage
            .layers
            .iter()
            .filter_map(|l| match *l {
                StageLayer::Shell {
                    radius_scale,
                    inset,
                    color,
                } => Some(ShellSpec {
                    dirs,
                    outlines,
                    radius: RADIUS * radius_scale,
                    inset,
                    color: Box::new(move |_| Some(color)),
                    cell_radius: None, // an authored shell is a sphere
                    depth: None,       // …and a cap, not a column
                    opts: MeshDrawOptions::default(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Publish what the planet is made of — the one-set sugar every
    /// single-view world uses: bakes into [`DEFAULT_SET`] and shows it.
    pub fn set_shells(&mut self, shells: Vec<ShellSpec<'_>>) {
        self.bake(DEFAULT_SET, shells);
        self.show(DEFAULT_SET);
    }

    /// Build a NAMED shell set (CPU only; uploaded at the next `render`)
    /// without changing which set draws. A bench with several data views of
    /// one planet bakes each when its DATA changes — so switching views is
    /// [`show`](GlobeWorld::show), a free swap, never a rebuild stalling the
    /// frame with the old picture on screen.
    pub fn bake(&mut self, key: &str, shells: Vec<ShellSpec<'_>>) {
        let built = shells
            .into_iter()
            .map(|s| {
                let ShellSpec {
                    dirs,
                    outlines,
                    radius,
                    inset,
                    color,
                    cell_radius,
                    depth,
                    opts,
                } = s;
                // The sphere and the per-column stack are the SAME builder:
                // one answers `radius` for every cell, the other answers the
                // column's own height. A shell with DEPTH is the volumetric
                // framing — closed columns instead of caps.
                let top = move |i: usize| cell_radius.as_ref().map_or(radius, |f| f(i));
                let (v, i) = match depth {
                    Some(d) => crate::build_columns(dirs, outlines, top, d, inset, color),
                    None => build(dirs, outlines, top, inset, color),
                };
                (v, i, opts)
            })
            .filter(|(_, i, _)| !i.is_empty())
            .collect();
        // A re-bake of a key replaces any bake of it still waiting.
        self.pending.retain(|(k, _)| k != key);
        self.pending.push((key.to_string(), built));
        self.dirty = false;
    }

    /// Draw the named set from here on. A key that was never baked draws
    /// NOTHING — loudly warned, and visibly wrong (rule 4BB12A75), never a
    /// stale other view standing in.
    pub fn show(&mut self, key: &str) {
        if key != self.active {
            if !self.sets.contains_key(key) && !self.pending.iter().any(|(k, _)| k == key) {
                tracing::warn!("globe set `{key}` was never baked — the globe will draw empty");
            }
            self.active = key.to_string();
        }
    }

    /// Line geometry drawn over the world, grouped by colour — headings, plate
    /// seams, a bench's own toggled overlays. The stage's authored frame is
    /// always drawn under whatever is passed here.
    pub fn set_arrows(&mut self, arrows: Arrows) {
        self.arrows.clear();
        self.arrows.extend(self.stage_arrows.iter().cloned());
        self.arrows.extend(arrows);
    }

    /// The data behind the picture moved; the shells no longer match it.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Has the world been told its shells are stale since they were last set?
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Seat the world in the slot the walker reserved for its `surface` node this frame
    /// (`UiFrame::surface(id)`) — the ONE place a bench tells the world where it is, and
    /// the place the node's layer, tint and liveness reach the composite. `None` = not
    /// on screen. A ROOT world (the planet IS the screen) never seats.
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.seat = slot.map(Seat::from);
    }

    /// The rect the world was last seated at.
    pub fn rect(&self) -> Option<Rect> {
        self.seat.map(|s| s.rect)
    }

    /// The player's LOOK settings (sensitivity + the two invert flags) reach the
    /// camera. The pad deadzone that used to live here moved to the signal SOURCE:
    /// the pump (or a bench's own resolver) deadzones the axis BEFORE it becomes the
    /// `look` tuple handed to [`update`](GlobeWorld::update).
    pub fn set_controls(&mut self, controls: AbstractControls) {
        self.cam.set_controls(controls);
    }

    /// Aim the orbit at `target` — what an INSPECTOR world does: a column shown
    /// at its true radius is orbited about its own region, not the far-away
    /// centre of the planet it came from. A whole-planet world never calls this
    /// (the default target is the origin the planet is centred on).
    pub fn aim(&mut self, target: Vec3) {
        self.cam.look_at(target);
    }

    /// Re-frame the camera around a region of `radius`, optionally opening with
    /// the region filling `fill` of the viewport (same meaning as `new`'s
    /// fill). Pose, target and the player's controls are kept — this is what a
    /// view calls when the thing it frames changes size, not a reset.
    pub fn set_frame(&mut self, radius: f32, fill: Option<f32>) {
        self.cam.set_frame(radius);
        if let Some(fill) = fill {
            self.cam.refill(fill);
        }
    }

    /// One frame of camera motion. `pointer` is the walker's sample for this globe's
    /// surface (`UiFrame::surface_pointer(id)`, or `root_pointer()` for a root world) —
    /// the barrier: present only while the cursor is over the planet with no UI over it,
    /// or while a press that began there is still held. `look` is the (yaw, pitch, zoom)
    /// deflection the SCENE resolved from its signal source (see
    /// [`look_from`](GlobeWorld::look_from)); the walker's ENTERED pane decides whether
    /// it is ours: a world in a pane owns the camera while that pane is entered, a ROOT
    /// world (no pane) owns it whenever NO pane is entered — the root is entered by
    /// default (Aaron 2026-08-21).
    pub fn update(
        &mut self,
        dt: f32,
        pointer: Option<&SurfacePointer>,
        look: (f32, f32, f32),
        focused: Option<&str>,
    ) {
        self.owns_camera = match (self.panel.as_deref(), focused) {
            (Some(panel), Some(f)) => panel == f,
            (None, None) => true,
            _ => false,
        };
        self.look = if self.owns_camera {
            look
        } else {
            (0.0, 0.0, 0.0)
        };
        let (dx, dy, dz) = self.look;
        if dx != 0.0 || dy != 0.0 {
            self.cam.orbit(dx, dy, dt);
        }
        if dz != 0.0 {
            self.cam.zoom(dz, dt);
        }
        self.cam.apply_pointer(pointer);
    }

    /// Resolve the three camera axes (yaw, pitch, zoom) from a signal SOURCE — the
    /// pump's `FrameInput::axis` for a scene on the pump (input-P3), or a bench's own
    /// `ContextualBindings::signal_axis`. The six Look/Zoom signals are the camera's
    /// knowledge, kept in ONE place; the source (and its deadzone) is the caller's. The
    /// result is handed straight to [`update`](GlobeWorld::update).
    pub fn look_from(mut axis: impl FnMut(ActionSignal) -> f32) -> (f32, f32, f32) {
        (
            axis(ActionSignal::LookRight) - axis(ActionSignal::LookLeft),
            axis(ActionSignal::LookUp) - axis(ActionSignal::LookDown),
            axis(ActionSignal::ZoomIn) - axis(ActionSignal::ZoomOut),
        )
    }

    /// The camera the world is seen from.
    pub fn camera(&self) -> Camera {
        self.cam.camera()
    }

    /// The cell being looked at: the one whose direction is closest to the
    /// camera. The globe is centred on the origin, so "toward the camera" is the
    /// point of it the maintainer is actually facing — a question about a globe,
    /// which is why it is answered here rather than in a scene.
    pub fn facing(&self, dirs: &[Vec3]) -> Option<usize> {
        let toward = self.cam.camera().position.normalize_or_zero();
        if toward.length_squared() < 0.5 || dirs.is_empty() {
            return None;
        }
        let mut best = (f32::MIN, 0usize);
        for (i, d) in dirs.iter().enumerate() {
            let facing = d.dot(toward);
            if facing > best.0 {
                best = (facing, i);
            }
        }
        Some(best.1)
    }

    /// Draw the world into the slot the walker seated it in. A no-op while the
    /// surface is off screen, which is what makes an unshown globe free. `base_layer`
    /// is the scene's band; the composite lands at `base + slot.layer`.
    pub fn render<'f>(&'f mut self, r: &mut Renderer, fg: &mut FrameGraph<'f>, base_layer: f32) {
        self.upload_pending(r);
        let Some(seat) = self.seat else { return };
        // Split the borrow: the view is written while the stage, the meshes and
        // the arrows are read into the same graph.
        let Self {
            view,
            cam,
            stage,
            sets,
            active,
            arrows,
            ..
        } = self;
        let meshes = sets.get(active).map_or(&[][..], Vec::as_slice);
        view.render(r, fg, seat, base_layer, cam.camera(), stage, meshes, arrows);
    }

    /// Draw the world as the ROOT surface's element — straight into the swapchain,
    /// no target, no blit. For a bench whose planet IS the screen (Epoch
    /// Simulation); a windowed globe uses [`render`](Self::render). Needs no seat —
    /// the pointer comes from the walker's root sample.
    pub fn render_root<'f>(&'f mut self, r: &mut Renderer, fg: &mut FrameGraph<'f>) {
        self.upload_pending(r);
        let Self {
            cam,
            stage,
            sets,
            active,
            arrows,
            ..
        } = self;
        let meshes = sets.get(active).map_or(&[][..], Vec::as_slice);
        GlobeView::render_root(fg, cam.camera(), stage, meshes, arrows);
    }

    /// Upload every baked set still waiting, each freeing the set it replaces.
    fn upload_pending(&mut self, r: &mut Renderer) {
        for (key, built) in self.pending.drain(..) {
            if let Some(old) = self.sets.remove(&key) {
                for (h, _) in old {
                    r.free_mesh(h);
                }
            }
            let handles = built
                .iter()
                .map(|(v, i, opts)| (r.upload_mesh(v, MeshIndices::U32(i)), *opts))
                .collect();
            self.sets.insert(key, handles);
        }
    }

    /// Give the GPU back: every baked set and the offscreen target.
    pub fn free(&mut self, r: &mut Renderer) {
        for (_, set) in self.sets.drain() {
            for (h, _) in set {
                r.free_mesh(h);
            }
        }
        self.view.free(r);
    }
}

/// The world in the input chain, below the walker: while its panel holds the
/// cursor the look and zoom signals are the camera's and stop there; everywhere
/// else they pass, and mean whatever the next handler says they mean.
impl InputHandler for GlobeWorld {
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        let camera_signal = matches!(
            ev.signal,
            ActionSignal::LookUp
                | ActionSignal::LookDown
                | ActionSignal::LookLeft
                | ActionSignal::LookRight
                | ActionSignal::ZoomIn
                | ActionSignal::ZoomOut
        );
        if camera_signal && self.owns_camera {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_input_core::device::{GamepadAxis, Key};
    // Test-only now: the scenes resolve the look tuple from these and hand the world the
    // result (input-P3) — the world itself no longer takes bindings/gamepad.
    use flicker_input_core::{
        ContextualBindings, EventKind, GamepadConfig, InputContext, InputMap, InputState,
    };

    fn styles(stage: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "stages": { "test_globe": stage } })
    }

    fn tiling() -> (Vec<Vec3>, Vec<Vec<Vec3>>) {
        let dirs = vec![Vec3::Y, Vec3::X];
        let ring = |axis: Vec3| -> Vec<Vec3> {
            (0..6)
                .map(|k| {
                    let a = (k as f32 / 6.0) * std::f32::consts::TAU;
                    (axis + Vec3::new(a.cos() * 0.2, 0.0, a.sin() * 0.2) * 0.2).normalize()
                })
                .collect()
        };
        (dirs.clone(), dirs.iter().map(|d| ring(*d)).collect())
    }

    /// **The stage block drives the shells and the clear.** Two authored shell
    /// layers, a graticule and a clear colour resolve to exactly that many draw
    /// layers, that many materialized shells, that reference frame and that
    /// clear — and an unknown `draw` kind is dropped LOUDLY rather than drawing
    /// nothing in silence (rule 4BB12A75). Every one of these values sat
    /// authored and unread in `ui_theme.json` before this pass.
    #[test]
    fn the_stage_block_drives_the_shells_and_the_clear() {
        let s = styles(serde_json::json!({
            "clear": [0.1, 0.2, 0.3, 1.0],
            "layers": [
                { "draw": "shell", "radius_scale": 0.998, "inset": 0.0, "color": [0.0, 0.0, 0.1, 1.0] },
                { "draw": "shell", "radius_scale": 1.0, "inset": 0.12, "color": [0.4, 0.5, 0.6, 1.0] },
                { "draw": "graticule", "radius_scale": 1.022 },
                { "draw": "no_such_kind" }
            ]
        }));
        let world = GlobeWorld::new("test_globe", &s, None);
        let stage = world.stage();
        let clear = stage.clear.expect("the stage authors a clear");
        for (got, want) in clear.iter().zip([0.1, 0.2, 0.3, 1.0]) {
            assert!(
                (got - want).abs() < 1e-6,
                "the authored clear is read: {got} vs {want}"
            );
        }
        assert_eq!(
            stage.layers.len(),
            3,
            "three known layers; the unknown kind is skipped"
        );
        assert_eq!(
            stage.layers[0],
            StageLayer::Shell {
                radius_scale: 0.998,
                inset: 0.0,
                color: [0.0, 0.0, 0.1]
            }
        );
        assert_eq!(
            stage.layers[1],
            StageLayer::Shell {
                radius_scale: 1.0,
                inset: 0.12,
                color: [0.4, 0.5, 0.6]
            }
        );
        assert_eq!(
            stage.layers[2],
            StageLayer::Graticule {
                radius_scale: 1.022
            }
        );

        // …and the authored shells materialize over a caller's tiling at those
        // radii, in authored order, with no colour named anywhere but the file.
        let (dirs, outlines) = tiling();
        let shells = world.authored_shells(&dirs, &outlines);
        assert_eq!(shells.len(), 2, "one ShellSpec per authored `shell` layer");
        assert!((shells[0].radius - RADIUS * 0.998).abs() < 1e-3);
        assert_eq!(shells[0].inset, 0.0);
        assert_eq!((shells[1].color)(0), Some([0.4, 0.5, 0.6]));
        assert!((shells[1].inset - 0.12).abs() < 1e-6);

        // A source that authors nothing keeps a picture: the scene's own shells.
        let bare = GlobeWorld::new("missing", &s, None);
        assert_eq!(bare.stage().layers, vec![StageLayer::Shells]);
        assert!(bare.authored_shells(&dirs, &outlines).is_empty());
    }

    /// **The world draws THE shared graticule** — the reference frame is a
    /// stage LAYER now, so every bench that authors one gets the identical
    /// value at the clearance its style file asks for, and nothing bench-local
    /// can drift from it. (Named home of the gate that used to live in the
    /// Populous scene.)
    #[test]
    fn the_world_draws_the_shared_graticule() {
        let s = styles(serde_json::json!({
            "layers": [{ "draw": "shells" }, { "draw": "graticule", "radius_scale": 1.022 }]
        }));
        let world = GlobeWorld::new("test_globe", &s, None);
        let shared = graticule(RADIUS * 1.022);
        assert_eq!(world.arrows.len(), shared.len(), "five shared ink groups");
        for ((ca, sa), (cb, sb)) in world.arrows.iter().zip(&shared) {
            assert_eq!(ca, cb, "same ink");
            assert_eq!(sa.len(), sb.len(), "same line counts");
        }
        // A bench's own overlay rides OVER the frame, never instead of it.
        let mut world = world;
        world.set_arrows(vec![([1.0, 0.0, 0.0, 1.0], vec![(Vec3::X, Vec3::Y)])]);
        assert_eq!(world.arrows.len(), shared.len() + 1);

        // A stage that authors no graticule draws none — the frame is a
        // decision in the style file, not something the code assumes.
        let bare = GlobeWorld::new(
            "test_globe",
            &styles(serde_json::json!({ "layers": [{ "draw": "shells" }] })),
            None,
        );
        assert!(bare.arrows.is_empty());
    }

    /// **Named sets: meshes follow DATA, showing is a swap, and a key that
    /// was never baked draws EMPTY — never a stale other view standing in
    /// (rule 4BB12A75).** The CPU state machine end to end: `set_shells` is
    /// the DEFAULT_SET sugar; `bake` builds without changing what draws and a
    /// re-bake replaces its own pending build; `show` swaps the active key;
    /// an unbaked key resolves to NO meshes — the loudly-warned empty globe,
    /// not the previous view.
    #[test]
    fn baked_sets_swap_and_an_unbaked_key_draws_empty_not_stale() {
        let s = styles(serde_json::json!({ "layers": [{ "draw": "shells" }] }));
        let mut world = GlobeWorld::new("test_globe", &s, None);
        let (dirs, outlines) = tiling();
        let spec = |rgb: [f32; 3]| ShellSpec {
            dirs: &dirs,
            outlines: &outlines,
            radius: 100.0,
            inset: 0.0,
            color: Box::new(move |_| Some(rgb)),
            cell_radius: None,
            depth: None,
            opts: MeshDrawOptions::default(),
        };

        // The one-set sugar: bakes DEFAULT_SET and shows it.
        world.set_shells(vec![spec([1.0, 0.0, 0.0])]);
        assert_eq!(world.active, DEFAULT_SET);
        assert_eq!(world.pending.len(), 1);
        assert_eq!(world.pending[0].0, DEFAULT_SET);

        // Baking a named view builds it WITHOUT changing what draws…
        world.bake("plates", vec![spec([0.0, 1.0, 0.0])]);
        assert_eq!(world.active, DEFAULT_SET, "baking never swaps the view");
        assert_eq!(world.pending.len(), 2);
        // …and a re-bake of the same key replaces its own pending build
        // rather than queueing a second one.
        world.bake("plates", vec![spec([0.0, 0.0, 1.0])]);
        assert_eq!(world.pending.len(), 2, "a re-bake replaces, never stacks");

        // Showing a baked key swaps; the draw path resolves ITS meshes.
        world.show("plates");
        assert_eq!(world.active, "plates");
        assert!(
            world.pending.iter().any(|(k, m)| k == "plates" && !m.is_empty()),
            "the shown set has a build to draw"
        );

        // THE CONTRACT: a key that was never baked still swaps — and resolves
        // to NOTHING to draw (the warned, visibly-wrong empty globe), never
        // the previous view standing in as if it were this one.
        world.show("no_such_view");
        assert_eq!(world.active, "no_such_view");
        assert!(!world.sets.contains_key(&world.active));
        assert!(
            !world.pending.iter().any(|(k, _)| k == "no_such_view"),
            "nothing baked under the key: the draw resolves empty"
        );
    }

    fn bench_bindings() -> ContextualBindings {
        ContextualBindings::new(InputMap::wasd_and_mouse())
    }

    /// **The camera orbits from a BOUND SIGNAL and never from a device.** The
    /// real bench map, a real `InputState` with the right stick deflected: the
    /// signal resolves through `signal_axis` and the camera turns. Nothing in
    /// this path names a gamepad.
    #[test]
    fn the_globe_camera_orbits_from_a_bound_signal_and_never_from_a_device() {
        let s = styles(serde_json::json!({ "layers": [{ "draw": "shells" }] }));
        let mut world = GlobeWorld::new("test_globe", &s, None).in_panel("view");
        let bindings = bench_bindings();
        let cfg = GamepadConfig::default();
        let mut input = InputState::new();
        input.gamepad_mut(0).set_axis(GamepadAxis::RightStickX, 0.9);
        input.gamepad_mut(0).set_axis(GamepadAxis::RightStickY, 0.9);
        assert!(
            bindings.signal_axis(ActionSignal::LookRight, &input, &cfg) > 0.0,
            "the bench map binds the right stick to the look signals"
        );

        let before = world.camera().position;
        // The scene resolves the look tuple from its signal source (the pump does this in
        // the migrated scenes); the world takes the tuple and gates it on focus.
        let look_axes = GlobeWorld::look_from(|s| bindings.signal_axis(s, &input, &cfg));
        world.update(0.5, None, look_axes, Some("view"));
        assert!(
            (world.camera().position - before).length() > 1.0,
            "a bound look signal turned the planet"
        );
    }

    /// **The camera moves only while the world panel holds focus.** The named
    /// replacement for the Populous scene's `orbit_input` gate: the SAME
    /// deflection moves nothing when the walker's cursor is on another panel,
    /// and the look signals then pass through this handler to whatever is
    /// below it.
    #[test]
    fn the_camera_moves_only_while_the_world_panel_holds_focus() {
        let s = styles(serde_json::json!({ "layers": [{ "draw": "shells" }] }));
        let mut world = GlobeWorld::new("test_globe", &s, None).in_panel("view");
        let bindings = bench_bindings();
        let mut input = InputState::new();
        input.gamepad_mut(0).set_axis(GamepadAxis::RightStickX, 1.0);

        let raw = InputState::new();
        let mut rc = RouteCtx::default();
        let look = InputEvent::new(
            ActionSignal::LookRight,
            EventKind::Press,
            InputContext::World,
            &raw,
        );

        // The scene resolves the look tuple from its bindings (the pump does this in the
        // migrated scenes); the world takes the tuple and gates it on focus.
        let cfg = GamepadConfig::default();
        let look_axes = GlobeWorld::look_from(|s| bindings.signal_axis(s, &input, &cfg));
        for elsewhere in [None, Some("other_panel")] {
            let before = world.camera().position;
            world.update(0.5, None, look_axes, elsewhere);
            assert_eq!(
                world.camera().position,
                before,
                "{elsewhere:?} does not fly the planet"
            );
            assert_eq!(
                world.handle(&look, &mut rc),
                Flow::Pass,
                "and the signal is not ours"
            );
        }

        let before = world.camera().position;
        world.update(0.5, None, look_axes, Some("view"));
        assert!(
            (world.camera().position - before).length() > 1.0,
            "the focused panel flies it"
        );
        assert_eq!(
            world.handle(&look, &mut rc),
            Flow::Consumed,
            "and owns the signal"
        );
        // Anything that is not a camera signal always passes.
        let confirm = InputEvent::new(
            ActionSignal::Confirm,
            EventKind::Press,
            InputContext::World,
            &raw,
        );
        assert_eq!(world.handle(&confirm, &mut rc), Flow::Pass);
    }

    /// **A deadzoned stick is thresholded EXACTLY ONCE.** Regression gate for a
    /// measured defect: the scene re-tested the already-deadzoned stick against
    /// the same deadzone, so the dead centre was carved out twice and the first
    /// slice of real travel did nothing. Just past the deadzone the signal must
    /// already read as live and RESCALED — the snapshot's answer, unmodified.
    #[test]
    fn a_deadzoned_stick_is_thresholded_exactly_once() {
        let bindings = bench_bindings();
        let cfg = GamepadConfig::default();
        let dz = cfg.right_stick_deadzone;
        let mut input = InputState::new();

        input
            .gamepad_mut(0)
            .set_axis(GamepadAxis::RightStickX, dz * 0.5);
        assert_eq!(
            bindings.signal_axis(ActionSignal::LookRight, &input, &cfg),
            0.0,
            "inside the dead centre, nothing"
        );

        let nudge = dz + (1.0 - dz) * 0.1;
        input
            .gamepad_mut(0)
            .set_axis(GamepadAxis::RightStickX, nudge);
        let v = bindings.signal_axis(ActionSignal::LookRight, &input, &cfg);
        assert!(v > 0.0, "just past the deadzone the stick is already live");
        let snapshot = input.gamepad(0).unwrap().right_stick().x;
        assert!(
            (v - snapshot).abs() < 1e-6,
            "the reported deflection IS the snapshot's — {v} vs {snapshot}"
        );
        assert!(v < nudge, "and it is the RESCALED travel, not the raw axis");

        // The other half of the axis is a different signal, not a sign.
        assert_eq!(
            bindings.signal_axis(ActionSignal::LookLeft, &input, &cfg),
            0.0
        );
        // A digital binding is a full deflection while it is held.
        input.set_key(Key::Up, true);
        assert_eq!(bindings.signal_axis(ActionSignal::NavUp, &input, &cfg), 1.0);
    }

    /// **`facing` answers the same question `facing_cell` did.** The cell whose
    /// direction is nearest the camera, and `None` before there is a world to
    /// ask about.
    #[test]
    fn facing_reports_the_cell_nearest_the_camera() {
        let s = styles(serde_json::json!({ "layers": [{ "draw": "shells" }] }));
        let world = GlobeWorld::new("test_globe", &s, None);
        assert_eq!(world.facing(&[]), None, "no world, no answer");
        let toward = world.camera().position.normalize_or_zero();
        let dirs = vec![-toward, Vec3::Y, toward];
        assert_eq!(
            world.facing(&dirs),
            Some(2),
            "the cell pointing at the camera"
        );
    }
}
