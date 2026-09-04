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
//! KBM only for now (Aaron 2026-09-03): left-drag orbits the perspective panel and pans
//! an orthographic one, right-drag pans, the wheel zooms; the pad's look signals reach
//! the perspective panel through the pump's continuous queries.

use flicker::render::{
    Camera, FrameGraph, MeshDrawOptions, MeshHandle, Orbit, PbrMaps, QuadView, Rect, Renderer,
    SkinnedMeshHandle, StageDef, TextureHandle, TexturedMeshHandle, EDITOR_QUADS,
};
use flicker::ui::{stage_def, SurfacePointer, SurfaceSlot};
use flicker_globe::view::Seat;
use flicker_globe::{Arrows, GlobeView, GlobeWorld};
use flicker_input_core::{AbstractControls, ActionSignal};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};
use glam::{Mat4, Vec2, Vec3};

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
        self.quad().ortho.is_some()
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
    /// A skinned mesh posed by `palette` (one matrix per bone, `bone_count` of them).
    Skinned {
        mesh: SkinnedMeshHandle,
        world: Mat4,
        palette: Vec<Mat4>,
        bone_count: u32,
    },
}

/// The pad's look tuple → panel motion rates.
const PAD_ORBIT_RATE: f32 = 1.6;
const PAD_PAN_RATE: f32 = 0.9;
const PAD_ZOOM_RATE: f32 = 1.2;

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
}

impl RigView {
    /// A panel drawing under `stages.<source>` from the shared styles, in `projection`.
    pub fn new(source: &str, styles: &serde_json::Value, projection: Projection) -> Self {
        let stage = stage_def(styles, source).unwrap_or_else(|| {
            tracing::warn!("stages.{source}: absent — the rig view draws under a default stage");
            StageDef::default()
        });
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

    pub fn rect(&self) -> Option<Rect> {
        self.seat.map(|s| s.rect)
    }

    pub fn set_controls(&mut self, controls: AbstractControls) {
        self.controls = controls;
    }

    /// Frame a subject: the camera looks at `centre` from a distance scaled by `radius`.
    /// Resets the pan; the zoom and orbit angles are the user's and survive.
    pub fn set_frame(&mut self, centre: Vec3, radius: f32) {
        self.centre = centre;
        self.radius = radius.max(1.0);
        self.orbit.pan = centre;
    }

    /// Reset the camera to the framed subject's default view.
    pub fn reset_framing(&mut self) {
        self.orbit = default_orbit(self.centre);
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
        let r = self.orbit.ortho_radius(self.radius).max(0.25);
        Camera {
            position: persp.target + dir * (r * 4.0),
            target: persp.target,
            up,
            near: 0.01,
            far: r * 12.0,
            ortho_height: Some(r * 2.2),
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
                self.orbit.orbit_by(Vec2::new(yaw, pitch) * PAD_ORBIT_RATE * dt / 0.006);
            }
            if dz != 0.0 {
                self.orbit.zoom_by(dz * PAD_ZOOM_RATE * dt);
            }
        }
        if let Some(p) = pointer.filter(|p| p.captured || p.wheel != 0.0) {
            let h = self.rect().map_or(1.0, |r| r.size.y.max(1.0));
            if self.projection.is_ortho() {
                // An orthographic panel has no orbit: either button pans.
                if p.left || p.right {
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
        let Some(seat) = self.seat else { return };
        let camera = self.camera();
        let Self {
            view,
            stage,
            lines,
            overlay,
            draws,
            ..
        } = self;
        let draws = std::mem::take(draws);
        view.render_pass(r, fg, seat, base_layer, stage, move |r| {
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
                    } => r.draw_textured_mesh_pbr(mesh, albedo, maps, world, MeshDrawOptions::default()),
                    Draw::Skinned {
                        mesh,
                        world,
                        palette,
                        bone_count,
                    } => r.draw_skinned_instanced(mesh, &[world], &palette, bone_count),
                }
            }
            for (color, segments) in lines.iter() {
                r.draw_lines(segments, *color);
            }
            for (color, segments) in overlay.iter() {
                r.draw_lines_overlay(segments, *color);
            }
        });
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
            assert_eq!(cam.target, Vec3::new(0.0, 0.0, 90.0), "{p:?} looks at the centre");
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
}
