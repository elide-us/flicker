//! **The rig view** — the shared surface filler that draws a skeleton (and whatever
//! meshes hang on it) into a `surface` slot the walker reserved.
//!
//! One instance per PANEL: Clayworks' view is a flow container of four `surface`
//! nodes (perspective + the three orthographic projections), each seated from its own
//! slot and lit by its own `stages.<source>` block, so a panel is authored exactly like
//! a globe's viewport. The filler contract mirrors [`flicker_globe::GlobeWorld`]:
//! `new` / `in_panel` / `seat` / `rect` / `set_controls` / `update` / `render`, plus
//! `InputHandler` (consumes the look/zoom signals only while its pane holds the
//! walker's cursor). It renders through the SAME [`GlobeView`] stage pass.
//!
//! What it draws is DATA the behaviour hands it each frame — line batches
//! ([`Arrows`]: joint segments, frame axes, the ground grid, gizmo handles) and
//! [`Draw`] items over handles the behaviour owns — so the view holds no rig, no
//! document and no picking policy. It offers the camera and a pointer ray
//! ([`ray_at`](RigView::ray_at)) for the behaviour's own hit tests.
//!
//! KBM only for now (Aaron 2026-09-03): left-drag orbits the perspective panel, right-drag
//! pans, the wheel zooms; the pad's look signals reach the perspective panel through the
//! pump's continuous queries. An ORTHOGRAPHIC panel pans on the RIGHT button ONLY (Aaron
//! 2026-09-07): its left button is the consumer's — Clayworks drags the selected joint
//! with it — so a left press can never wander the view.
//!
//! AN ORTHOGRAPHIC PANEL CAN VIEW FROM EITHER SIDE (TOP↔BOTTOM, LEFT↔RIGHT, FRONT↔BACK):
//! [`RigView::flip`] crosses the camera through the look-at to the opposite one, and
//! [`RigView::flipped`] / [`RigView::projection`] report which, for a host to reflect in
//! its own chrome. The corner label and its press-to-flip control are no longer drawn
//! here: a sub-scene panel draws them as a real `button` in its OWN scene tree (Aaron
//! 2026-09-09 — a nested surface hosts a complete scene), so RigView holds no label
//! style and no press policy for one.

use flicker::render::{
    Camera, CompositeTarget, FrameGraph, MeshDrawOptions, MeshHandle, Orbit, PbrMaps, QuadView,
    Rate, Rect, Renderer, SkinnedMeshHandle, StageDef, StageInputs, TextureHandle,
    TexturedMeshHandle, EDITOR_QUADS,
};
use flicker::ui::{stage_def, SurfacePointer, SurfaceSlot};
use flicker_globe::view::Seat;
use flicker_globe::{Arrows, GlobeView, GlobeWorld};
use flicker_input_core::{AbstractControls, ActionSignal};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};
use glam::{Mat4, Vec2, Vec3};

pub mod doll;
pub mod gadget;
pub use doll::{Doll, DollRig, SkinnedBody, DOLL_LAYERS, LIVE_HZ};
pub use gadget::{Gadget, GadgetDelta, GadgetStyle, HandleState, Press};

/// Which of the editor's four projections a panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Top,
    Left,
    Front,
}

impl Projection {
    pub const ALL: [Projection; 4] = [Self::Perspective, Self::Top, Self::Left, Self::Front];

    fn quad(self) -> &'static QuadView {
        &EDITOR_QUADS[match self {
            Self::Perspective => 0,
            Self::Top => 1,
            Self::Left => 2,
            Self::Front => 3,
        }]
    }

    pub fn is_ortho(self) -> bool {
        self.depth_axis().is_some()
    }

    /// The world direction an ORTHOGRAPHIC panel looks ALONG — the camera sits at
    /// `target + dir * …`, so this is the picture's depth axis, the one direction the
    /// panel cannot show. `None` for the perspective panel, which has no single one.
    ///
    /// The [`Gadget`] hides (and refuses to pick) the handle that lies along it: a
    /// handle pointing at the camera projects to a point.
    pub fn depth_axis(self) -> Option<Vec3> {
        self.quad().ortho.map(|(dir, _)| dir)
    }
}

/// One thing a panel draws this frame, over handles the behaviour uploaded and frees.
#[derive(Clone, Debug)]
pub enum Draw {
    /// A flat-shaded mesh (tint / wireframe through `options`).
    Mesh {
        mesh: MeshHandle,
        world: Mat4,
        options: MeshDrawOptions,
    },
    /// A textured mesh under the PBR path (albedo + optional maps).
    Textured {
        mesh: TexturedMeshHandle,
        albedo: TextureHandle,
        maps: PbrMaps,
        world: Mat4,
    },
    /// A skinned mesh posed by `palette` (one matrix per bone, `bone_count` of them), plus the
    /// CPU-deformed CLOTH SUBMESH split off the same body when it carries one (spec 6C46CAB9).
    /// ONE item, ONE world matrix: the two halves are one body and must never drift apart.
    /// `material` (albedo + PBR maps) shades the skinned half through the ONE material path
    /// a textured mesh uses, and the cloth half through its textured twin `cloth_textured` (the
    /// same vertices with their UVs and tangents, rewritten each frame alongside `cloth`);
    /// `None` is the neutral steel that shows the skin's weighting, and the flat cloth.
    Skinned {
        mesh: SkinnedMeshHandle,
        world: Mat4,
        palette: Vec<Mat4>,
        bone_count: u32,
        cloth: Option<MeshHandle>,
        cloth_textured: Option<TexturedMeshHandle>,
        material: Option<(TextureHandle, PbrMaps)>,
    },
}

/// The pad's look tuple → panel motion rates.
const PAD_ORBIT_RATE: f32 = 1.6;
const PAD_PAN_RATE: f32 = 0.9;
const PAD_ZOOM_RATE: f32 = 1.2;

/// An orthographic panel's eye distance and visible height, as multiples of the zoomed
/// framing radius — the ONE place both live, so [`RigView::focus`] can invert them.
const ORTHO_EYE_PER_RADIUS: f32 = 4.0;
const ORTHO_HEIGHT_PER_RADIUS: f32 = 2.2;
/// How much of a panel's height a focused part fills ([`RigView::focus`]).
const FOCUS_FILL: f32 = 0.6;

pub struct RigView {
    view: GlobeView,
    stage: StageDef,
    projection: Projection,
    orbit: Orbit,
    seat: Option<Seat>,
    panel: Option<String>,
    owns_camera: bool,
    controls: AbstractControls,
    /// The framed subject: its centre and bounding radius (cm), set by the behaviour.
    centre: Vec3,
    radius: f32,
    lines: Arrows,
    /// Line batches drawn OVER the meshes (no depth test): the skeleton inside a body, the
    /// gizmo handles, the selection ball.
    overlay: Arrows,
    draws: Vec<Draw>,
    /// A liveness policy the BEHAVIOUR decides, overriding the seat's authored rate.
    /// `None` = the seat's. A [`Doll`] uses it to say "only the selected card animates";
    /// a panel the user is flying leaves it alone.
    rate: Option<Rate>,
    /// Whether the content changed this frame — the signal a [`Rate::Dirty`] surface
    /// re-renders on. Consumed by each `render`.
    dirty: bool,
    /// An orthographic panel viewing from the OPPOSITE side (BOTTOM, RIGHT, BACK).
    flipped: bool,
    /// A BACK-CULLING cut through an orthographic panel: `Some(t)` is the depth of the cut
    /// from the subject's NEAR face as a fraction of its depth along the view axis, so `0.0`
    /// leaves only the near face and `1.0` (or `None`) cuts nothing. Measured from the face
    /// the panel currently looks at, so a flip carries the cut to the other side.
    cull: Option<f32>,
    /// The subject's axis-aligned half extents (cm) — what a cut's depth spans. The framing
    /// radius on every axis until the behaviour states the real box ([`Self::set_extent`]).
    half_extent: Vec3,
    /// The stage's AUTHORED clear, kept so [`Self::set_clear`]'s override can be lifted.
    authored_clear: Option<[f64; 4]>,
}

impl RigView {
    /// A panel drawing under `stages.<source>` from the shared styles, in `projection`.
    pub fn new(source: &str, styles: &serde_json::Value, projection: Projection) -> Self {
        let stage = stage_def(styles, source).unwrap_or_else(|| {
            tracing::warn!("stages.{source}: absent — the rig view draws under a default stage");
            StageDef::default()
        });
        let authored_clear = stage.clear;
        Self {
            view: GlobeView::default(),
            stage,
            projection,
            orbit: default_orbit(Vec3::ZERO),
            seat: None,
            panel: None,
            owns_camera: false,
            controls: AbstractControls::default(),
            centre: Vec3::ZERO,
            radius: 100.0,
            lines: Vec::new(),
            overlay: Vec::new(),
            draws: Vec::new(),
            rate: None,
            dirty: false,
            flipped: false,
            cull: None,
            half_extent: Vec3::splat(100.0),
            authored_clear,
        }
    }

    /// Override what the panel clears to — a backdrop toggle (a dark model on a grey stage
    /// wants black; Aaron 2026-09-08) — or `None` to return to the stage's authored clear.
    pub fn set_clear(&mut self, clear: Option<[f64; 4]>) {
        let next = clear.or(self.authored_clear);
        if next != self.stage.clear {
            self.stage.clear = next;
            self.dirty = true;
        }
    }

    /// Whether an orthographic panel views from the opposite side.
    pub fn flipped(&self) -> bool {
        self.flipped
    }

    /// View from the opposite side — TOP↔BOTTOM, LEFT↔RIGHT, FRONT↔BACK. The perspective
    /// panel has no opposite and ignores it.
    pub fn flip(&mut self) {
        if self.projection.is_ortho() {
            self.flipped = !self.flipped;
            self.dirty = true;
        }
    }

    /// The pane (`tab_group`) whose cursor hands this panel the look signals.
    pub fn in_panel(mut self, tab_group: impl Into<String>) -> Self {
        self.panel = Some(tab_group.into());
        self
    }

    pub fn projection(&self) -> Projection {
        self.projection
    }

    pub fn stage(&self) -> &StageDef {
        &self.stage
    }

    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.seat = slot.map(Seat::from);
    }

    /// Seat the panel at a rect the HOST laid out rather than at a walker-reserved
    /// `surface` node — a [`Doll`] on a card the graph canvas placed, which has no node of
    /// its own to reserve it. Liveness then comes from [`Self::set_rate`], the only place
    /// a behaviour-owned seat can express it.
    pub fn seat_at(&mut self, rect: Rect, layer: f32, tint: [f32; 4]) {
        self.seat = Some(Seat {
            rect,
            layer,
            tint,
            rate: Rate::Live,
        });
    }

    pub fn rect(&self) -> Option<Rect> {
        self.seat.map(|s| s.rect)
    }

    /// Override the seated rate: the behaviour's own liveness policy (only the selected
    /// doll animates), or `None` to honour whatever the `surface` node authored.
    pub fn set_rate(&mut self, rate: Option<Rate>) {
        self.rate = rate;
    }

    /// Say whether this frame's content changed — what a [`Rate::Dirty`] surface
    /// re-renders on. Consumed by the next [`Self::render`].
    pub fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
    }

    pub fn set_controls(&mut self, controls: AbstractControls) {
        self.controls = controls;
    }

    /// Frame a subject: the camera looks at `centre` from a distance scaled by `radius`.
    /// Resets the pan; the zoom and orbit angles are the user's and survive.
    pub fn set_frame(&mut self, centre: Vec3, radius: f32) {
        self.centre = centre;
        self.radius = radius.max(1.0);
        self.half_extent = Vec3::splat(self.radius);
        self.orbit.pan = centre;
    }

    /// The subject's axis-aligned half extents about the framed centre — the depth a
    /// [`Self::set_cull`] slider spans, so its whole travel crosses the BODY (a torso is a
    /// fifth as deep as it is tall) rather than the bounding sphere. Call after
    /// [`Self::set_frame`], which resets it to the radius.
    pub fn set_extent(&mut self, half: Vec3) {
        self.half_extent = half.abs().max(Vec3::splat(0.01));
    }

    /// BACK CULLING: cut everything deeper than `t` of the subject's depth from the panel's
    /// near face (`0.0` = only the near face survives, `1.0` = nothing is cut). Orthographic
    /// panels only — the perspective panel is the picker and always shows the whole subject.
    /// The far plane does the cutting, so the meshes AND every line batch stop at the cut.
    pub fn set_cull(&mut self, cull: Option<f32>) {
        let cull = cull
            .filter(|_| self.projection.is_ortho())
            .map(|t| t.clamp(0.0, 1.0));
        if cull != self.cull {
            self.cull = cull;
            self.dirty = true;
        }
    }

    pub fn cull(&self) -> Option<f32> {
        self.cull
    }

    /// The live cut as a WORLD plane `(normal, d)`: a point `x` is culled when
    /// `normal · x > d`. The normal is the panel's view direction (into the picture), so the
    /// other orthographic panels see this plane edge-on — where the bench draws it as a line.
    /// `None` without an active cut.
    pub fn cull_plane(&self) -> Option<(Vec3, f32)> {
        let view = -self.eye_dir()?;
        let p = self.cut_point()?;
        Some((view, view.dot(p)))
    }

    /// FRAME a part of the subject: pan the look-at to `centre` and zoom so a ball of
    /// `radius` fills [`FOCUS_FILL`] of the panel's height — the selected hand, in every
    /// panel at once. The subject framing ([`Self::set_frame`]) is untouched, so
    /// [`Self::reset_framing`] still returns to the whole body.
    pub fn focus(&mut self, centre: Vec3, radius: f32) {
        let want_h = 2.0 * radius.max(0.5) / FOCUS_FILL;
        let zoom = if self.projection.is_ortho() {
            // `camera`: ortho_height = ORTHO_HEIGHT_PER_RADIUS · R · zoom.
            want_h / (ORTHO_HEIGHT_PER_RADIUS * self.radius)
        } else {
            // The visible height at the look-at depth is 2 · dist · tan(fov / 2), and the
            // orbit's dist is R · dist_scale · zoom.
            let fov = self.orbit.camera(self.radius).fov_y_radians;
            let dist = want_h / (2.0 * (fov * 0.5).tan());
            dist / (self.radius * self.orbit.dist_scale)
        };
        self.orbit.pan = centre;
        self.orbit.set_zoom(zoom);
        self.dirty = true;
    }

    /// The direction from the look-at point TOWARD an orthographic panel's eye, flip applied.
    /// `None` for the perspective panel.
    fn eye_dir(&self) -> Option<Vec3> {
        let (dir, _) = self.projection.quad().ortho?;
        Some(if self.flipped { -dir } else { dir })
    }

    /// Where the active cut crosses the view axis: `centre + view · (2t − 1) · e`, `e` the
    /// subject's half extent along that axis. `None` without an active cut.
    fn cut_point(&self) -> Option<Vec3> {
        let t = self.cull.filter(|t| *t < 1.0)?;
        let view = -self.eye_dir()?;
        let e = self.half_extent.dot(view.abs()).max(0.01);
        Some(self.centre + view * ((2.0 * t - 1.0) * e))
    }

    /// The radius the camera is ACTUALLY framing — what [`Self::set_frame`] settled on
    /// after its floor, which a metric subject (a [`Doll`] at ~0.9 m) sits under while a
    /// centimetre rig does not. A caller expressing a distance against the subject reads
    /// it back here rather than re-deriving the floor and drifting from it.
    pub fn framing_radius(&self) -> f32 {
        self.radius
    }

    /// Reset the camera to the framed subject's default view.
    pub fn reset_framing(&mut self) {
        self.orbit = default_orbit(self.centre);
    }

    /// Point the camera along an AUTHORED shot — `stages.<source>.camera`'s angles, with
    /// the distance expressed as a multiple of the subject radius so the one authored
    /// shot frames a rig of any size. A panel the user flies never calls this (its angles
    /// are the user's); a PREVIEW ([`Doll`], the bake view) is framed by the author.
    pub fn set_orbit(&mut self, yaw: f32, pitch: f32, dist_scale: f32) {
        self.orbit.yaw = yaw;
        self.orbit.pitch = pitch;
        self.orbit.dist_scale = dist_scale;
    }

    /// This frame's depth-tested line batches (colour, segments) — the ground grid, the
    /// collision volumes — replaced wholesale each frame.
    pub fn set_lines(&mut self, lines: Arrows) {
        self.lines = lines;
    }

    /// This frame's OVERLAY line batches, drawn over the meshes without a depth test —
    /// the skeleton, the selected joint's ball and the gizmo handles.
    pub fn set_overlay(&mut self, overlay: Arrows) {
        self.overlay = overlay;
    }

    /// This frame's draw items, over handles the behaviour owns (it uploads and frees
    /// them) — replaced wholesale each frame.
    pub fn set_draws(&mut self, draws: Vec<Draw>) {
        self.draws = draws;
    }

    pub fn owns_camera(&self) -> bool {
        self.owns_camera
    }

    /// The panel's camera: the orbit's perspective, or the projection's orthographic
    /// view sharing its look-at point and zoom.
    pub fn camera(&self) -> Camera {
        let persp = self.orbit.camera(self.radius);
        let Some((dir, up)) = self.projection.quad().ortho else {
            return persp;
        };
        let dir = if self.flipped { -dir } else { dir };
        let r = self.orbit.ortho_radius(self.radius).max(0.25);
        let position = persp.target + dir * (r * ORTHO_EYE_PER_RADIUS);
        // An active cut brings the far plane in to its depth (never in front of the eye).
        let far = match self.cut_point() {
            Some(p) => (p - position).dot(-dir).max(0.02),
            None => r * 12.0,
        };
        Camera {
            position,
            target: persp.target,
            up,
            near: 0.01,
            far,
            ortho_height: Some(r * ORTHO_HEIGHT_PER_RADIUS),
            ..persp
        }
    }

    /// A world-space ray through the pointer (the renderer's one `Camera::pick_ray`), for
    /// the behaviour's picking. `None` while the panel is unseated or the pointer is not
    /// over it.
    pub fn ray_at(&self, pointer: Option<&SurfacePointer>) -> Option<(Vec3, Vec3)> {
        let seat = self.seat?;
        let p = pointer?;
        self.camera().pick_ray(p.local, seat.rect.size)
    }

    /// Per frame: the pointer sample the walker's barrier handed this surface, the pad's
    /// look tuple (see [`GlobeWorld::look_from`]) and the focused pane's group — the
    /// panel answers the look only while its pane is the focused one.
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
        if self.owns_camera {
            let (dx, dy, dz) = look;
            let stick = Vec2::new(dx, -dy);
            if self.projection.is_ortho() {
                if dx != 0.0 || dy != 0.0 {
                    let cam = self.camera();
                    let h = self.rect().map_or(1.0, |r| r.size.y.max(1.0));
                    let (px, py) = self.controls.look_delta_stick(stick);
                    self.orbit
                        .pan_by_view(Vec2::new(px, py) * PAD_PAN_RATE * dt * h, &cam, h);
                }
            } else if dx != 0.0 || dy != 0.0 {
                let (yaw, pitch) = self.controls.look_delta_stick(stick);
                self.orbit
                    .orbit_by(Vec2::new(yaw, pitch) * PAD_ORBIT_RATE * dt / 0.006);
            }
            if dz != 0.0 {
                self.orbit.zoom_by(dz * PAD_ZOOM_RATE * dt);
            }
        }
        if let Some(p) = pointer.filter(|p| p.captured || p.wheel != 0.0) {
            let h = self.rect().map_or(1.0, |r| r.size.y.max(1.0));
            if self.projection.is_ortho() {
                // An orthographic panel has no orbit, and its LEFT button is the consumer's
                // (a placement, never a pan): only the right button pans.
                if p.right && !p.left {
                    let cam = self.camera();
                    self.orbit.pan_by_view(p.delta, &cam, h);
                }
                self.orbit.zoom_by(p.wheel);
            } else {
                self.orbit
                    .apply_pointer(p.delta, p.left, p.right, p.wheel, self.radius, h);
            }
        }
    }

    /// Declare the panel's pass into the walker's reserved rect (nothing while unseated).
    pub fn render<'f>(&'f mut self, r: &mut Renderer, fg: &mut FrameGraph<'f>, base_layer: f32) {
        let Some(mut seat) = self.seat else { return };
        let camera = self.camera();
        // The behaviour's liveness policy wins over the seat's authored rate: a doll's
        // "only the selected card animates" is a runtime fact the JSON cannot know.
        if let Some(rate) = self.rate {
            seat.rate = rate;
        }
        let mut inputs = StageInputs::default();
        inputs.with_dirty(std::mem::take(&mut self.dirty));
        let Self {
            view,
            stage,
            lines,
            overlay,
            draws,
            ..
        } = self;
        let draws = std::mem::take(draws);
        view.render_pass(
            r,
            fg,
            seat,
            base_layer,
            stage,
            inputs,
            None,
            Self::draw_pass(camera, draws, lines, overlay),
        );
    }

    /// Declare the panel as a ROOT STAGE — straight into the surface the caller is
    /// declaring against, with no target of its own, no composite and no blit.
    ///
    /// This is the SUB-SCENE shape (Aaron 2026-09-09: a nested surface hosts a complete
    /// scene). A scene whose whole picture IS this panel — `flicker_modelview::ModelView`,
    /// the one panel the quad view instantiates four times — declares it exactly as a
    /// top-level scene declares its root surface; under [`FrameGraph::sub_scene`] that lands
    /// in the host panel's target, where the scene's OWN chrome then draws over it (the
    /// chrome and the picture share one texture, so the 2D encoder's panels-under-sprites
    /// order can never hide it — incident 09E5A30F). There is no composite label here:
    /// a sub-scene panel's label is real chrome in its own tree.
    ///
    /// Framing still comes from the seat ([`Self::seat_at`] with the panel's viewport), so
    /// the camera's aspect and the pointer ray are the panel's. Unseated declares nothing.
    pub fn render_root<'f>(&'f mut self, fg: &mut FrameGraph<'f>) {
        let Some(seat) = self.seat else { return };
        let camera = self.camera();
        // The behaviour's liveness policy wins over the seat's authored rate, exactly as in
        // `render`; inside a sub-scene scope the HOST's seat rate is what actually clocks the
        // pass, and this is the value a top-level caller would honour.
        let rate = self.rate.unwrap_or(seat.rate);
        let mut inputs = StageInputs::default();
        inputs.with_dirty(std::mem::take(&mut self.dirty));
        let Self {
            stage,
            lines,
            overlay,
            draws,
            ..
        } = self;
        let draws = std::mem::take(draws);
        fg.surface(
            CompositeTarget::Screen,
            stage,
            inputs,
            rate,
            Self::draw_pass(camera, draws, lines, overlay),
        );
    }

    /// The ONE pass body both declarations run: the panel's camera, this frame's draw
    /// items, then the depth-tested line batches and the overlay ones. [`Self::render`]
    /// runs it inside the seat's offscreen target; [`Self::render_root`] runs it as a root
    /// stage — the same picture, differing only in where it lands.
    fn draw_pass<'f>(
        camera: Camera,
        draws: Vec<Draw>,
        lines: &'f Arrows,
        overlay: &'f Arrows,
    ) -> impl FnOnce(&mut Renderer) + 'f {
        move |r: &mut Renderer| {
            r.set_camera(&camera);
            for d in draws {
                match d {
                    Draw::Mesh {
                        mesh,
                        world,
                        options,
                    } => r.draw_mesh(mesh, world, options),
                    Draw::Textured {
                        mesh,
                        albedo,
                        maps,
                        world,
                    } => r.draw_textured_mesh_pbr(
                        mesh,
                        albedo,
                        maps,
                        world,
                        MeshDrawOptions::default(),
                    ),
                    Draw::Skinned {
                        mesh,
                        world,
                        palette,
                        bone_count,
                        cloth,
                        cloth_textured,
                        material,
                    } => {
                        // The cloth half rides the SAME world matrix in the same pass — its
                        // vertices were rewritten in place before this closure ran — under the
                        // body's material when it has one, flat otherwise.
                        match material {
                            Some((albedo, maps)) => {
                                r.draw_skinned_instanced_pbr(
                                    mesh,
                                    &[world],
                                    &palette,
                                    bone_count,
                                    albedo,
                                    maps,
                                );
                                match (cloth_textured, cloth) {
                                    (Some(twin), _) => r.draw_textured_mesh_pbr(
                                        twin,
                                        albedo,
                                        maps,
                                        world,
                                        MeshDrawOptions::default(),
                                    ),
                                    (None, Some(cloth)) => {
                                        r.draw_mesh(cloth, world, MeshDrawOptions::default())
                                    }
                                    (None, None) => {}
                                }
                            }
                            None => {
                                r.draw_skinned_instanced(mesh, &[world], &palette, bone_count);
                                if let Some(cloth) = cloth {
                                    r.draw_mesh(cloth, world, MeshDrawOptions::default());
                                }
                            }
                        }
                    }
                }
            }
            for (color, segments) in lines.iter() {
                r.draw_lines(segments, *color);
            }
            for (color, segments) in overlay.iter() {
                r.draw_lines_overlay(segments, *color);
            }
        }
    }

    /// Give the render target back (scene `exit`).
    pub fn free(&mut self, r: &mut Renderer) {
        self.view.free(r);
    }

    /// The look tuple from the pump's continuous queries — the globe's, shared.
    pub fn look_from(axis: impl FnMut(ActionSignal) -> f32) -> (f32, f32, f32) {
        GlobeWorld::look_from(axis)
    }
}

/// The panel's opening camera: the editor orbit's three-quarter view, pulled in a little
/// closer than the paperdoll's default, looking at `centre`.
fn default_orbit(centre: Vec3) -> Orbit {
    Orbit {
        dist_scale: 2.0,
        pan: centre,
        ..Orbit::default()
    }
}

impl InputHandler for RigView {
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

    fn styles() -> serde_json::Value {
        serde_json::json!({ "stages": { "rig_test": { "lighting": "studio", "clear": [0.0, 0.0, 0.0, 1.0] } } })
    }

    #[test]
    fn the_orthographic_panels_share_the_look_at_and_report_a_height() {
        for p in Projection::ALL {
            let mut v = RigView::new("rig_test", &styles(), p);
            v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
            let cam = v.camera();
            assert_eq!(
                cam.target,
                Vec3::new(0.0, 0.0, 90.0),
                "{p:?} looks at the centre"
            );
            assert_eq!(cam.ortho_height.is_some(), p.is_ortho(), "{p:?}");
        }
    }

    #[test]
    fn the_look_signals_belong_to_the_panel_only_while_its_pane_is_focused() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Perspective).in_panel("view");
        v.update(0.016, None, (1.0, 0.0, 0.0), Some("controls"));
        assert!(!v.owns_camera());
        let before = v.orbit.yaw;
        v.update(0.016, None, (1.0, 0.0, 0.0), Some("view"));
        assert!(v.owns_camera());
        assert_ne!(v.orbit.yaw, before, "the focused pane's panel orbits");
    }

    /// An orthographic panel pans on the RIGHT button only: a captured left drag leaves the
    /// look-at where it was (that button drags joints), a right drag moves it.
    #[test]
    fn an_orthographic_panel_pans_on_the_right_button_only() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        v.seat_at(
            Rect {
                pos: Vec2::ZERO,
                size: Vec2::new(200.0, 100.0),
            },
            0.0,
            [1.0; 4],
        );
        let drag = |left: bool, right: bool| SurfacePointer {
            id: "p".into(),
            root: false,
            cursor: Vec2::new(50.0, 50.0),
            local: Vec2::new(50.0, 50.0),
            delta: Vec2::new(20.0, 0.0),
            left,
            right,
            pressed: false,
            wheel: 0.0,
            captured: true,
            rect: Rect {
                pos: Vec2::ZERO,
                size: Vec2::new(200.0, 100.0),
            },
        };
        let home = v.camera().target;
        v.update(0.016, Some(&drag(true, false)), (0.0, 0.0, 0.0), None);
        assert_eq!(
            v.camera().target,
            home,
            "a left drag never pans an ortho panel"
        );
        v.update(0.016, Some(&drag(false, true)), (0.0, 0.0, 0.0), None);
        assert_ne!(v.camera().target, home, "a right drag pans it");
    }

    #[test]
    fn a_pointer_ray_through_the_panel_centre_passes_the_look_at_point() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        let rect = Rect {
            pos: Vec2::new(10.0, 10.0),
            size: Vec2::new(200.0, 100.0),
        };
        let slot = SurfaceSlot {
            id: "p".into(),
            source: "rig_test".into(),
            // A rig view FILLS its slot; it is not itself a sub scene (the model view
            // is the scene, and it seats its panel at its own viewport instead).
            scene: String::new(),
            params: Default::default(),
            x: 10.0,
            y: 10.0,
            w: 200.0,
            h: 100.0,
            layer: 0.0,
            rate: flicker::render::Rate::Live,
            tint: [1.0; 4],
            layout: flicker::render::ViewportLayout::Single,
        };
        v.seat(Some(&slot));
        // `pick_ray` aims through the pixel's CENTRE (+0.5), so the panel's exact middle
        // is the pixel half a step before it.
        let pointer = SurfacePointer {
            id: "p".into(),
            root: false,
            cursor: Vec2::new(109.5, 59.5),
            local: Vec2::new(99.5, 49.5),
            delta: Vec2::ZERO,
            left: false,
            right: false,
            pressed: false,
            wheel: 0.0,
            captured: false,
            rect,
        };
        let (origin, dir) = v.ray_at(Some(&pointer)).expect("a ray");
        // The ray runs along the view direction and its line contains the centre.
        let to_centre = Vec3::new(0.0, 0.0, 90.0) - origin;
        let off_axis = to_centre - dir * to_centre.dot(dir);
        assert!(off_axis.length() < 1e-2, "off-axis by {off_axis}");
        assert!(v.ray_at(None).is_none());
    }
    /// A cut of 1.0 (or none) leaves the far plane where it was and reports no plane; the
    /// perspective panel never cuts.
    #[test]
    fn a_cull_at_one_cuts_nothing() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        let uncut = v.camera().far;
        v.set_cull(Some(1.0));
        assert_eq!(v.camera().far, uncut);
        assert!(v.cull_plane().is_none());
        v.set_cull(Some(0.5));
        assert!(v.camera().far < uncut, "a half cut brings the far plane in");
        v.set_cull(None);
        assert_eq!(v.camera().far, uncut);
        let mut p = RigView::new("rig_test", &styles(), Projection::Perspective);
        p.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        let whole = p.camera().far;
        p.set_cull(Some(0.0));
        assert_eq!(p.cull(), None, "the picker never cuts");
        assert_eq!(p.camera().far, whole);
    }

    /// A cut of 0.0 puts the far plane on the subject's NEAR face: the front view (eye on −Y)
    /// keeps y = −e and culls everything behind it, half-way cuts at the median plane, and the
    /// stated extent — not the bounding radius — is the depth the slider spans.
    #[test]
    fn a_cull_at_zero_leaves_only_the_near_face() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        let cam = v.camera();
        let eye_depth = (cam.target - cam.position).length();
        v.set_cull(Some(0.0));
        let (n, d) = v.cull_plane().expect("an active cut");
        assert_eq!(n, Vec3::Y, "the front view looks along +Y");
        assert!((d + 90.0).abs() < 1e-3, "the near face is y = −R: d = {d}");
        assert!((v.camera().far - (eye_depth - 90.0)).abs() < 1e-3);
        let culled = |x: Vec3| n.dot(x) > d;
        assert!(culled(Vec3::new(0.0, 10.0, 90.0)));
        assert!(!culled(Vec3::new(0.0, -95.0, 90.0)));
        v.set_cull(Some(0.5));
        let (_, d) = v.cull_plane().expect("an active cut");
        assert!(d.abs() < 1e-3, "half-way is the median plane: d = {d}");
        // A torso is a fifth as deep as it is tall: the extent narrows the span.
        v.set_extent(Vec3::new(30.0, 15.0, 90.0));
        v.set_cull(Some(0.0));
        let (_, d) = v.cull_plane().expect("an active cut");
        assert!(
            (d + 15.0).abs() < 1e-3,
            "the near face is now y = −15: d = {d}"
        );
        assert!((v.camera().far - (eye_depth - 15.0)).abs() < 1e-3);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        let (_, d) = v.cull_plane().expect("an active cut");
        assert!(
            (d + 90.0).abs() < 1e-3,
            "re-framing resets the extent to the radius"
        );
    }

    /// The cut is measured from the face the panel currently looks at: flipping FRONT to BACK
    /// keeps the same depth of cut and culls the OTHER half of the body.
    #[test]
    fn the_cut_follows_the_flip() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        v.set_cull(Some(0.25));
        let far_front = v.camera().far;
        let (n, d) = v.cull_plane().expect("an active cut");
        let front = Vec3::new(0.0, -80.0, 90.0);
        let back = Vec3::new(0.0, 80.0, 90.0);
        assert!(
            n.dot(back) > d && n.dot(front) <= d,
            "FRONT keeps the front, culls the back"
        );
        v.flip();
        assert!(
            (v.camera().far - far_front).abs() < 1e-3,
            "the same depth of cut"
        );
        let (n, d) = v.cull_plane().expect("an active cut");
        assert_eq!(n, -Vec3::Y, "BACK looks along −Y");
        assert!(
            n.dot(front) > d && n.dot(back) <= d,
            "BACK keeps the back, culls the front"
        );
    }

    /// Focusing pans the look-at to the part and zooms so it fills the panel's fill fraction —
    /// in an orthographic panel by its stated height, in the perspective one at the look-at
    /// depth — without disturbing the subject framing a reset returns to.
    #[test]
    fn a_focus_frames_the_part_in_every_projection() {
        let part = Vec3::new(10.0, 0.0, 150.0);
        for p in Projection::ALL {
            let mut v = RigView::new("rig_test", &styles(), p);
            v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
            v.focus(part, 15.0);
            let cam = v.camera();
            assert_eq!(cam.target, part, "{p:?} looks at the part");
            let visible_h = match cam.ortho_height {
                Some(h) => h,
                None => {
                    2.0 * (cam.position - cam.target).length() * (cam.fov_y_radians * 0.5).tan()
                }
            };
            let want = 2.0 * 15.0 / FOCUS_FILL;
            assert!(
                (visible_h - want).abs() < 0.5,
                "{p:?} shows {visible_h} for {want}"
            );
            assert_eq!(v.framing_radius(), 90.0, "{p:?} keeps the subject framing");
            v.reset_framing();
            assert_eq!(v.camera().target, Vec3::new(0.0, 0.0, 90.0));
        }
    }

    /// THE ROOT-STAGE DECLARATION NEEDS NO RENDERER (which is the whole point: no target of
    /// its own, no composite, no blit — see [`RigView::render_root`]). An UNSEATED panel
    /// declares nothing and keeps its dirty signal for the frame that seats it; a seated one
    /// declares the pass, consuming this frame's dirty flag and its draw items.
    #[test]
    fn a_root_stage_panel_declares_without_a_renderer_and_drains_the_frame() {
        let rect = Rect {
            pos: Vec2::ZERO,
            size: Vec2::new(300.0, 300.0),
        };
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        v.set_frame(Vec3::new(0.0, 0.0, 90.0), 90.0);
        v.set_lines(vec![([1.0; 4], vec![(Vec3::ZERO, Vec3::X)])]);
        v.set_dirty(true);
        {
            let mut fg = FrameGraph::new();
            v.render_root(&mut fg);
        }
        assert!(
            v.dirty,
            "an unseated panel declares nothing and keeps its dirty signal"
        );
        v.seat_at(rect, 0.0, [1.0; 4]);
        {
            let mut fg = FrameGraph::new();
            v.render_root(&mut fg);
        }
        assert!(!v.dirty, "the declared pass consumed this frame's signal");
        assert!(v.draws.is_empty(), "and took this frame's draw items");
    }

    /// A clear override replaces the stage's authored clear and `None` restores it.
    #[test]
    fn a_clear_override_replaces_the_stages_clear_and_none_restores_it() {
        let mut v = RigView::new("rig_test", &styles(), Projection::Front);
        let authored = v.stage().clear;
        assert_eq!(
            authored,
            Some([0.0, 0.0, 0.0, 1.0]),
            "the test stage authors black"
        );
        v.set_clear(Some([0.1, 0.1, 0.13, 1.0]));
        assert_eq!(v.stage().clear, Some([0.1, 0.1, 0.13, 1.0]));
        v.set_clear(None);
        assert_eq!(v.stage().clear, authored, "the authored clear is back");
    }
}
