//! **The doll** — the small-panel skinned-rig preview: one character, posed by a clip,
//! standing on its authored ground ring, in a seat that may be 34 px or 300 px across.
//!
//! Built ON [`RigView`], never beside it. A doll IS a rig view with three differences:
//! the author frames it (not the viewer, so it takes no camera input), it carries a clock
//! of its own, and it declares its own liveness. Everything else — the offscreen target,
//! the stage pass, the composite, the teardown — is the rig view's, which is the rig
//! view's `GlobeView` pass, which is the ONE pass every surface filler in the engine
//! renders through. There is no second stage pass here and no second target lifecycle.
//!
//! **Liveness is a rate, never a cache.** A LIVE doll asks for [`Rate::Hz`]; a POSTER doll
//! asks for [`Rate::Dirty`] and publishes `dirty` only when what it shows actually changed
//! (rig, clip, activity — a resize invalidates the target on its own). The renderer's
//! per-surface clock skips the pass otherwise and the target composites the image it
//! already holds. **N dolls on a page therefore cost ONE live pass, not N** — which is
//! what makes the design's six sizes affordable on a screen carrying a dozen of them.
//! Do not add a `drawn` flag or a poster texture: the clock owns this decision.
//!
//! **Nothing here is a colour.** The ground ring, the floor grid, the lighting and the
//! framing all come out of the authored `stages.<source>` block the [`RigView`] compiled.

use std::sync::Arc;

use flicker::render::{
    grid_segments, ring_segments, FrameGraph, MeshIndices, Rate, Rect, Renderer, SkinnedMeshHandle,
    SkinnedVertex, StageCamera, StageLayer,
};
use flicker::ui::SurfaceSlot;
use flicker_globe::Arrows;
use flicker_skeletal::format::{Model, Vertex};
use flicker_skeletal::{pose, skin};
use glam::{Mat4, Vec2, Vec3};

use crate::{Draw, Projection, RigView};

/// The stage layer kinds a doll draws. A source authoring anything else is named once,
/// at construction, rather than drawing nothing in silence.
pub const DOLL_LAYERS: &[&str] = &["skinned", "ring", "grid"];

/// How often a LIVE doll re-renders. Clips are authored at 60 Hz (the time canon), but a
/// 34 px doll on a list row does not need a frame per clip tick — the clock is what keeps
/// a page of dolls off the GPU, so spending half the budget by default would defeat it.
pub const LIVE_HZ: f32 = 30.0;

/// The ONE rig a screenful of dolls shares: the uploaded skinned mesh plus the model it
/// poses from. Behind an [`Arc`] because a page carries a dozen dolls and the GPU skins
/// every instance from its own bone palette — one mesh, one skeleton, N poses.
///
/// The host owns it: it uploads the mesh here and gives it back with [`DollRig::free`] on
/// scene exit, exactly as a behaviour owns the handles it hands [`RigView::set_draws`].
pub struct DollRig {
    model: Arc<Model>,
    mesh: Option<SkinnedMeshHandle>,
    /// Rest-pose ground offset. `Model::world` centres the rig on the origin, but the
    /// authored cameras (`target_y`) and rings (`y: 0`) are metric with the feet on the
    /// floor, so the doll is dropped onto it before it is drawn.
    ground: Mat4,
}

impl DollRig {
    /// Upload `model`'s skinned mesh and take the pose source with it. A model with no
    /// mesh yields a rig that poses nothing — it is still a valid (empty) doll, not a
    /// panic, because a bench can be opened before its content loads.
    pub fn upload(r: &mut Renderer, model: Arc<Model>) -> Self {
        let ground = ground_transform(model.world, &model.mesh.vertices);
        if model.mesh.vertices.is_empty() {
            tracing::warn!("doll: the rig has no mesh — its dolls will be empty");
            return Self {
                model,
                mesh: None,
                ground,
            };
        }
        let verts: Vec<SkinnedVertex> = model
            .mesh
            .vertices
            .iter()
            .map(|v| SkinnedVertex {
                position: v.p,
                normal: v.n,
                uv: v.uv,
                joints: v.joints,
                weights: v.weights,
            })
            .collect();
        // The converter emits a non-deduped sequential list when indices are absent.
        let indices: Vec<u32> = if model.mesh.indices.is_empty() {
            (0..verts.len() as u32).collect()
        } else {
            model.mesh.indices.clone()
        };
        let mesh = r.upload_skinned_mesh(&verts, MeshIndices::U32(&indices));
        tracing::info!(
            bones = model.bones.len(),
            verts = verts.len(),
            clips = model.clips.len(),
            "doll: rig uploaded"
        );
        Self {
            model,
            mesh: Some(mesh),
            ground,
        }
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn bone_count(&self) -> u32 {
        self.model.bones.len() as u32
    }

    /// The rest pose's bounding radius in engine space — what the authored framing's
    /// distance is expressed against, so one authored shot frames a rig of any size.
    pub fn radius(&self) -> f32 {
        self.model.orbit_radius.max(f32::MIN_POSITIVE)
    }

    /// The clip named, if the rig carries it — so a host can bind by name without
    /// reaching into the model.
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        self.model.clips.iter().position(|c| c.name == name)
    }

    /// The bone palette for `clip` at `time` seconds; the clip loops on its own duration,
    /// and an absent or out-of-range clip is the rest pose rather than a panic. CPU
    /// posing is cheap — the GPU does the vertex skinning.
    pub fn palette(&self, clip: Option<usize>, time: f32) -> Vec<Mat4> {
        let bones = &self.model.bones;
        let locals = match clip.and_then(|i| self.model.clips.get(i)) {
            Some(c) if c.duration_ticks > 0 => {
                let ticks = time * c.tick_rate_hz as f32;
                let tick = (ticks.floor() as i64).rem_euclid(c.duration_ticks as i64) as u32;
                pose::sample_local_poses(bones, c, tick, self.model.retarget)
            }
            Some(c) => pose::sample_local_poses(bones, c, 0, self.model.retarget),
            None => bones.iter().map(|b| b.local).collect(),
        };
        skin::palette(bones, &pose::global_transforms(bones, &locals))
    }

    /// Give the mesh back (scene `exit`). Taken, so a second teardown is a no-op.
    pub fn free(&mut self, r: &mut Renderer) {
        if let Some(m) = self.mesh.take() {
            r.free_skinned_mesh(m);
        }
    }

    /// Give the SHARED rig's mesh back through the last handle on it. Release every
    /// [`Doll`] holding a clone first; a rig still held by a live doll cannot be freed,
    /// and says so rather than freeing a mesh something is about to draw.
    pub fn release(rig: &mut Arc<Self>, r: &mut Renderer) {
        match Arc::get_mut(rig) {
            Some(rig) => rig.free(r),
            None => tracing::warn!(
                "doll: the rig is still held by a live doll — its mesh is NOT freed; \
                 release the dolls before the rig"
            ),
        }
    }

    fn draw(&self, clip: Option<usize>, time: f32) -> Option<Draw> {
        Some(Draw::Skinned {
            mesh: self.mesh?,
            world: self.ground,
            palette: self.palette(clip, time),
            bone_count: self.bone_count(),
        })
    }
}

/// A skinned-rig preview seated in one small panel.
pub struct Doll {
    view: RigView,
    /// The authored shot: yaw / pitch / distance / look-at height, straight off
    /// `stages.<source>.camera`. An unframed stage takes the portrait default.
    framing: StageCamera,
    rig: Option<Arc<DollRig>>,
    clip: Option<usize>,
    /// Play-head, seconds — the doll's OWN clock, advanced by [`Doll::tick`].
    time: f32,
    live: bool,
    hz: f32,
    /// Selected / pointed-at: lights the ground ring in its authored active colour.
    active: bool,
    /// Set by every change to what the image shows; consumed by the next `render`, which
    /// is what a [`Rate::Dirty`] poster re-renders on.
    dirty: bool,
    /// The seat size the framing was last fitted to — a re-seat at a new shape refits.
    size: Vec2,
}

impl Doll {
    /// A doll drawing under `stages.<source>` from the shared styles. Perspective always:
    /// an orthographic preview of a character reads as a technical drawing, and the
    /// authored shot is an orbit.
    pub fn new(source: &str, styles: &serde_json::Value) -> Self {
        let mut view = RigView::new(source, styles, Projection::Perspective);
        let undrawn = view.stage().layers_outside(DOLL_LAYERS);
        if !undrawn.is_empty() {
            tracing::warn!("doll: `{source}` authors {undrawn:?} layers the doll does not draw");
        }
        // The framing policy — "an unframed stage is a portrait" — is applied ONCE here,
        // to the definition, not re-decided in every frame's draw closure.
        let framing = view.stage().camera.unwrap_or_else(|| {
            tracing::warn!("doll: `{source}` authors no camera — taking the portrait framing");
            StageCamera::default()
        });
        // The ground is authored geometry: it is laid ONCE here and replaced only when
        // the doll's activity changes its colour. A doll rebuilding its ring every frame
        // would be recomputing a constant.
        let lines = ground_lines(&view.stage().layers, false);
        view.set_lines(lines);
        Self {
            view,
            framing,
            rig: None,
            clip: None,
            time: 0.0,
            live: false,
            hz: LIVE_HZ,
            active: false,
            dirty: true,
            size: Vec2::ZERO,
        }
    }

    /// The rate a LIVE doll asks for. Anything above the display rate is a waste; zero
    /// or below would make a "live" doll never draw, so it is refused.
    pub fn live_hz(mut self, hz: f32) -> Self {
        if hz > 0.0 {
            self.hz = hz;
        }
        self
    }

    /// The rig every doll on the page shares. Changing it changes the image.
    pub fn set_rig(&mut self, rig: Option<Arc<DollRig>>) {
        let same = match (&self.rig, &rig) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.rig = rig;
            self.dirty = true;
            self.refit();
        }
    }

    /// The clip this doll poses with — the idle it loops, or the one its card is bound
    /// to. `None` is the rest pose.
    pub fn set_clip(&mut self, clip: Option<usize>) {
        if self.clip != clip {
            self.clip = clip;
            self.dirty = true;
        }
    }

    pub fn clip(&self) -> Option<usize> {
        self.clip
    }

    /// Whether this doll animates. Only the one being watched should: a live doll is a
    /// GPU submit every `1/hz` seconds, a poster is none at all.
    pub fn set_live(&mut self, live: bool) {
        if self.live != live {
            self.live = live;
            // Going live must draw the current pose even if the clock says "not yet"; going
            // still must draw the frame it stopped on rather than keep the one before it.
            self.dirty = true;
        }
    }

    pub fn live(&self) -> bool {
        self.live
    }

    /// Selected / pointed-at — the authored ring's active colour. A still doll is a still
    /// doll, so the ring lights on the same condition that makes the slot animate.
    pub fn set_active(&mut self, active: bool) {
        if self.active != active {
            self.active = active;
            self.dirty = true;
            // Activity changes the ring's COLOUR, never its geometry — so this is the
            // only moment the ground is rebuilt.
            let lines = ground_lines(&self.view.stage().layers, active);
            self.view.set_lines(lines);
        }
    }

    /// The play-head, seconds. Set it to drive the doll off a transport the host owns
    /// (a TAE playhead) instead of its own clock.
    pub fn set_time(&mut self, time: f32) {
        if self.time != time {
            self.time = time;
            self.dirty = true;
        }
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    /// Advance this doll's own clock. A poster's clock is parked — it is showing one
    /// frame, and moving a play-head nobody renders is exactly the per-frame recompute
    /// the poster exists to avoid.
    pub fn tick(&mut self, dt: f32) {
        if self.live {
            self.time += dt;
        }
    }

    /// Seat the doll in the `surface` slot the walker reserved for it.
    pub fn seat(&mut self, slot: Option<&SurfaceSlot>) {
        self.view.seat(slot);
        self.fit(slot.map_or(Vec2::ZERO, |s| Vec2::new(s.w, s.h)));
    }

    /// Seat the doll at a rect the HOST laid out — a card a graph canvas placed, which
    /// has no `surface` node of its own to reserve it.
    pub fn seat_at(&mut self, rect: Rect, layer: f32, tint: [f32; 4]) {
        self.view.seat_at(rect, layer, tint);
        self.fit(rect.size);
    }

    /// Unseat: an off-page doll declares nothing and keeps its target for its return.
    pub fn unseat(&mut self) {
        self.view.seat(None);
    }

    pub fn rect(&self) -> Option<Rect> {
        self.view.rect()
    }

    /// The rate this doll asks the per-surface clock for. A live doll re-renders on the
    /// clock; a still one re-renders only when it says its image changed.
    pub fn rate(&self) -> Rate {
        if self.live {
            Rate::Hz(self.hz)
        } else {
            Rate::Dirty
        }
    }

    /// Declare this doll's pass and composite (nothing while unseated).
    pub fn render<'f>(&'f mut self, r: &mut Renderer, fg: &mut FrameGraph<'f>, base_layer: f32) {
        // **Pose only what will be drawn.** A still, unchanged doll's pass is skipped by
        // the clock, so posing its skeleton would be a 67-bone recompute per frame for an
        // image nobody renders — with a dozen dolls on the page, the whole cost the poster
        // exists to avoid. Every way the pass CAN run — the first frame, a resize, a new
        // rig / clip / play-head — raises `dirty`, so this is never a blank draw.
        let draws = match self.rig.as_ref().filter(|_| self.poses()) {
            Some(rig) => rig.draw(self.clip, self.time).into_iter().collect(),
            None => Vec::new(),
        };
        self.view.set_draws(draws);
        let rate = self.rate();
        self.view.set_rate(Some(rate));
        // A live doll's rate ignores `dirty`, but consuming it either way keeps the flag
        // from surviving a spell of liveness and forcing one stale draw on the way back.
        self.view.set_dirty(std::mem::take(&mut self.dirty));
        self.view.render(r, fg, base_layer);
    }

    /// Give the doll's render target back (scene `exit`, or a page that dropped it). The
    /// rig's mesh is the HOST's — [`DollRig::free`] returns that, once, for every doll.
    pub fn release(&mut self, r: &mut Renderer) {
        self.view.free(r);
        self.view.seat(None);
        // The next seat draws into a fresh target, so it must draw.
        self.dirty = true;
    }

    /// Whether this frame's image has to be POSED. A still, unchanged doll's pass is
    /// skipped by the clock, so its skeleton must not be sampled — and every way the pass
    /// can still run raises `dirty`, so this is never false on a frame that draws.
    fn poses(&self) -> bool {
        self.live || self.dirty
    }

    /// Fit the authored shot to a seat of this shape. Called on every seat, and a no-op
    /// unless the shape actually changed — the framing must not be recomputed per frame
    /// for a doll that has not moved.
    fn fit(&mut self, size: Vec2) {
        if self.size != size {
            self.size = size;
            // A new shape rebuilds the target at a new size, and a fresh target must draw.
            self.dirty = true;
            self.refit();
        }
    }

    /// Apply the authored framing against the seated rig at the seated shape.
    ///
    /// The camera's field of view is VERTICAL, so a seat wider than it is tall shows more
    /// than the shot asked for (harmless) while a NARROWER one crops the subject at the
    /// sides. Backing off by the aspect is the whole size adaptation the doll needs: the
    /// shot itself is in world units, so a 34 px seat and a 300 px seat frame the subject
    /// identically — the small one is simply the same picture with fewer pixels.
    fn refit(&mut self) {
        let subject = self.rig.as_ref().map_or(1.0, |r| r.radius());
        let fit = if self.size.x > 0.0 && self.size.y > 0.0 {
            (self.size.y / self.size.x).max(1.0)
        } else {
            1.0
        };
        self.view
            .set_frame(Vec3::new(0.0, self.framing.target_y, 0.0), subject * fit);
        // `dist` is authored in WORLD units; the orbit expresses it as a multiple of the
        // radius it settled on (which has a floor a metric doll sits under). Reading that
        // back is what keeps the authored shot exact for a rig of any size — and keeps the
        // aspect pull-back a pull-back instead of cancelling itself out.
        let scale = self.framing.dist * fit / self.view.framing_radius();
        self.view
            .set_orbit(self.framing.yaw, self.framing.pitch, scale);
    }
}

/// Drop the rig onto the floor: `Model::world` centres it on the origin, the authored
/// stages are metric with the feet at y = 0. Without this every doll floats above (or
/// sinks through) its own ring.
fn ground_transform(world: Mat4, vertices: &[Vertex]) -> Mat4 {
    let feet = vertices
        .iter()
        .map(|v| world.transform_point3(Vec3::from(v.p)).y)
        .fold(f32::INFINITY, f32::min);
    let drop = if feet.is_finite() { -feet } else { 0.0 };
    Mat4::from_translation(Vec3::new(0.0, drop, 0.0)) * world
}

/// The line geometry of a source's ground layers, in authored order and authored colour —
/// depth-tested against the doll, so a ring reads under its feet. Kinds the doll does not
/// draw were named at construction; here they simply contribute nothing.
fn ground_lines(layers: &[StageLayer], active: bool) -> Arrows {
    layers
        .iter()
        .filter_map(|l| match *l {
            StageLayer::Ring {
                radius,
                y,
                segments,
                color,
                color_active,
            } => Some((
                if active { color_active } else { color },
                ring_segments(Vec3::new(0.0, y, 0.0), radius, segments),
            )),
            StageLayer::Grid {
                spacing,
                extent,
                y,
                color,
            } => Some((color, grid_segments(spacing, extent, y))),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_skeletal::format::{Bone, Mesh, Model, Source};

    /// A stage authored the way a doll's is: lit, framed, standing on a ring.
    fn styles() -> serde_json::Value {
        serde_json::json!({ "stages": { "doll_test": {
            "lighting": "studio",
            "clear": [0.0, 0.0, 0.0, 0.0],
            "camera": { "kind": "orbit", "yaw": 0.55, "pitch": 0.18, "dist": 2.6, "target_y": 0.95 },
            "layers": [
                { "draw": "skinned" },
                { "draw": "ring", "radius": 0.45, "y": 0.0, "segments": 24,
                  "color": [0.5, 0.4, 0.2, 1.0], "color_active": [1.0, 0.8, 0.3, 1.0] }
            ]
        } } })
    }

    fn bone(name: &str) -> Bone {
        Bone {
            name: name.into(),
            parent: -1,
            local: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }
    }

    fn vert(y: f32) -> Vertex {
        Vertex {
            p: [0.0, y, 0.0],
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }

    /// A rig with no GPU: the mesh handle stays `None`, which is exactly the state a
    /// doll is in before its content loads.
    fn rig(radius: f32) -> Arc<DollRig> {
        Arc::new(DollRig {
            model: Arc::new(Model {
                bones: vec![bone("root"), bone("spine"), bone("head")],
                clips: Vec::new(),
                mesh: Mesh::default(),
                source: Source::default(),
                world: Mat4::IDENTITY,
                orbit_radius: radius,
                retarget: false,
                attach: Default::default(),
                collision: Default::default(),
            }),
            mesh: None,
            ground: Mat4::IDENTITY,
        })
    }

    fn seat(w: f32, h: f32) -> SurfaceSlot {
        SurfaceSlot {
            id: "d".into(),
            source: "doll_test".into(),
            x: 0.0,
            y: 0.0,
            w,
            h,
            layer: 0.0,
            rate: Rate::Live,
            tint: [1.0; 4],
            layout: flicker::render::ViewportLayout::Single,
        }
    }

    /// **The six sizes.** The design puts the same doll on the screen at 34, 42, 48, 92,
    /// 180 and 300 px; the shot is in world units, so every one of them must frame the
    /// subject IDENTICALLY — a small doll is the same picture with fewer pixels, never a
    /// differently-composed one. Regression gate against wiring pixels into the camera.
    #[test]
    fn the_framing_is_the_same_shot_at_every_one_of_the_six_sizes() {
        let mut seen: Option<(Vec3, Vec3)> = None;
        for px in [34.0f32, 42.0, 48.0, 92.0, 180.0, 300.0] {
            let mut d = Doll::new("doll_test", &styles());
            d.set_rig(Some(rig(0.9)));
            d.seat(Some(&seat(px, px)));
            let cam = d.view.camera();
            // The authored shot: looking at the chest, from the authored distance.
            assert!(
                (cam.target - Vec3::new(0.0, 0.95, 0.0)).length() < 1e-4,
                "{px}px looks at the authored target, got {}",
                cam.target
            );
            assert!(
                ((cam.position - cam.target).length() - 2.6).abs() < 1e-3,
                "{px}px stands at the authored distance, got {}",
                (cam.position - cam.target).length()
            );
            match seen {
                None => seen = Some((cam.position, cam.target)),
                Some((p, t)) => {
                    assert!((cam.position - p).length() < 1e-4, "{px}px moved the eye");
                    assert!((cam.target - t).length() < 1e-4, "{px}px moved the look-at");
                }
            }
        }
    }

    /// A rig of a different size gets the SAME authored shot, scaled to it — the reason
    /// the distance is expressed against the subject radius rather than baked in.
    #[test]
    fn the_authored_shot_scales_to_the_rig() {
        let mut small = Doll::new("doll_test", &styles());
        small.set_rig(Some(rig(0.45)));
        small.seat(Some(&seat(92.0, 92.0)));
        let mut big = Doll::new("doll_test", &styles());
        big.set_rig(Some(rig(1.8)));
        big.seat(Some(&seat(92.0, 92.0)));
        let d = |c: flicker::render::Camera| (c.position - c.target).length();
        // Same authored `dist` in world units for both — the orbit's dist_scale absorbed
        // the radius difference, so neither rig is framed from inside its own chest.
        assert!((d(small.view.camera()) - d(big.view.camera())).abs() < 1e-3);
        assert!((d(big.view.camera()) - 2.6).abs() < 1e-3);
    }

    /// The field of view is VERTICAL, so a seat NARROWER than it is tall must back the
    /// camera off or the subject is cropped at the sides. A wide seat is left alone —
    /// it already shows more than the shot asked for.
    #[test]
    fn a_narrow_seat_backs_the_camera_off_and_a_wide_one_does_not() {
        let d = |w: f32, h: f32| {
            let mut d = Doll::new("doll_test", &styles());
            d.set_rig(Some(rig(0.9)));
            d.seat(Some(&seat(w, h)));
            let c = d.view.camera();
            (c.position - c.target).length()
        };
        let square = d(92.0, 92.0);
        assert!(d(46.0, 92.0) > square * 1.5, "a half-width seat pulls back");
        assert!(
            (d(300.0, 92.0) - square).abs() < 1e-3,
            "a wide seat keeps the authored shot"
        );
    }

    /// **The live / poster contract.** A live doll asks for a rate on the clock; a still
    /// one asks to be re-rendered only when it says its image changed. This is the whole
    /// reason a page of a dozen dolls costs one pass.
    #[test]
    fn a_live_doll_asks_for_a_rate_and_a_poster_asks_for_dirty() {
        let mut d = Doll::new("doll_test", &styles());
        assert_eq!(
            d.rate(),
            Rate::Dirty,
            "a doll is a poster until told otherwise"
        );
        d.set_live(true);
        assert_eq!(d.rate(), Rate::Hz(LIVE_HZ));
        assert!(
            matches!(d.rate(), Rate::Hz(hz) if hz > 0.0),
            "a live rate is > 0"
        );
        d.set_live(false);
        assert_eq!(d.rate(), Rate::Dirty);
        // And the rate the clock is handed is a real one at every size.
        let refused = Doll::new("doll_test", &styles()).live_hz(0.0);
        assert_eq!(
            refused.hz, LIVE_HZ,
            "a zero rate would never draw — refused"
        );
    }

    /// Only what CHANGES the image raises `dirty`, and the render consumes it — otherwise
    /// a poster either never redraws (stale) or redraws forever (not a poster).
    #[test]
    fn only_a_real_change_dirties_a_poster() {
        let mut d = Doll::new("doll_test", &styles());
        d.set_rig(Some(rig(0.9)));
        d.seat(Some(&seat(92.0, 92.0)));
        d.dirty = false;

        d.set_clip(None);
        assert!(!d.dirty, "re-setting the same clip is not a change");
        d.set_clip(Some(3));
        assert!(d.dirty, "a new clip is a new image");
        d.dirty = false;

        d.set_active(false);
        assert!(!d.dirty, "re-setting the same activity is not a change");
        d.set_active(true);
        assert!(d.dirty, "the ring changed colour");
        d.dirty = false;

        let same = d.rig.clone();
        d.set_rig(same);
        assert!(!d.dirty, "the same rig is not a change");
        d.set_rig(Some(rig(0.9)));
        assert!(d.dirty, "a new rig is a new image");
    }

    /// The clock is the doll's own and only runs while it is live — a poster advancing a
    /// play-head nobody renders is exactly the per-frame recompute it exists to avoid.
    #[test]
    fn the_clip_clock_advances_only_while_live() {
        let mut d = Doll::new("doll_test", &styles());
        d.tick(0.5);
        assert_eq!(d.time(), 0.0, "a poster's clock is parked");
        d.set_live(true);
        d.tick(0.25);
        d.tick(0.25);
        assert!(
            (d.time() - 0.5).abs() < 1e-6,
            "a live doll runs its own clock"
        );
        d.set_live(false);
        d.tick(1.0);
        assert!((d.time() - 0.5).abs() < 1e-6, "and stops where it stopped");
    }

    /// The clip loops on its OWN duration and an unknown one is the rest pose — a doll
    /// bound to a clip the rig does not carry must not panic on a list refill.
    #[test]
    fn the_palette_loops_the_clip_and_falls_back_to_the_rest_pose() {
        let r = rig(0.9);
        let p = r.palette(None, 0.0);
        assert_eq!(p.len(), 3, "one matrix per bone");
        assert!(p.iter().all(|m| m.is_finite()));
        // An index past the end of an empty clip list is the rest pose, not a panic.
        assert_eq!(r.palette(Some(7), 1.5).len(), 3);
        assert_eq!(r.bone_count(), 3);
    }

    /// The ring's colour is the one piece of per-doll state the authored geometry carries;
    /// activity changes the colour, never the geometry.
    #[test]
    fn the_active_ring_changes_colour_not_geometry() {
        let d = Doll::new("doll_test", &styles());
        let layers = &d.view.stage().layers;
        let idle = ground_lines(layers, false);
        let lit = ground_lines(layers, true);
        assert_eq!(
            idle.len(),
            lit.len(),
            "activity changes colour, not geometry"
        );
        assert!(!idle.is_empty(), "the ring produced segments");
        assert_eq!(idle[0].1.len(), lit[0].1.len());
        assert_ne!(idle[0].0, lit[0].0, "an active ring is a different colour");
        // A layer kind the doll does not draw contributes nothing rather than a panic.
        assert!(ground_lines(&[StageLayer::Graticule { radius_scale: 1.0 }], false).is_empty());
        // And a degenerate ring is no ring, not a crash.
        let bad = [StageLayer::Ring {
            radius: -1.0,
            y: 0.0,
            segments: 24,
            color: [1.0; 4],
            color_active: [1.0; 4],
        }];
        assert!(ground_lines(&bad, false)[0].1.is_empty());
    }

    /// The rig is centred on the origin by `Model::world`; the authored stages put the
    /// feet at y = 0, so the ground transform must drop the doll by its lowest vertex.
    #[test]
    fn the_ground_transform_puts_the_feet_on_the_floor() {
        let g = ground_transform(Mat4::IDENTITY, &[vert(-0.9), vert(0.9)]);
        let lowest = g.transform_point3(Vec3::new(0.0, -0.9, 0.0));
        assert!(lowest.y.abs() < 1e-5, "lowest vertex lands on y = 0");
        // The whole rig shifts together — it is not scaled.
        let top = g.transform_point3(Vec3::new(0.0, 0.9, 0.0));
        assert!((top.y - 1.8).abs() < 1e-5, "the doll keeps its height");
        assert!(ground_transform(Mat4::IDENTITY, &[]).is_finite());
    }

    /// **The cardinal sin, gated.** A settled poster must not pose its skeleton: with a
    /// dozen dolls on a page that is a 67-bone recompute per frame for images nobody
    /// draws. Every way the pass CAN still run must raise the flag, or a doll draws blank.
    #[test]
    fn a_settled_poster_does_not_pose_but_everything_that_can_draw_does() {
        let mut d = Doll::new("doll_test", &styles());
        assert!(
            d.poses(),
            "the first frame always renders — it must be posed"
        );
        d.set_rig(Some(rig(0.9)));
        d.seat(Some(&seat(92.0, 92.0)));
        assert!(d.poses(), "a fresh target must draw");

        d.dirty = false;
        assert!(!d.poses(), "a settled poster poses nothing");

        // A resize rebuilds the target, and a fresh target must draw.
        d.seat(Some(&seat(300.0, 300.0)));
        assert!(d.poses(), "a resized doll must redraw");
        d.dirty = false;

        d.set_clip(Some(1));
        assert!(d.poses(), "a new clip must redraw");
        d.dirty = false;

        d.set_time(2.0);
        assert!(d.poses(), "a moved play-head must redraw");
        d.dirty = false;

        // Live is live: it poses whether or not anything else changed.
        d.set_live(true);
        d.dirty = false;
        assert!(d.poses(), "a live doll always poses");

        // And release leaves it ready to draw into the fresh target it will be given.
        d.set_live(false);
        d.dirty = false;
        d.unseat();
        assert!(!d.poses(), "an off-page doll poses nothing");
    }

    /// An unseated doll declares nothing, and `release` leaves it that way — the seam a
    /// host's `exit()` calls so no target outlives the bench that made it.
    #[test]
    fn an_unseated_doll_has_no_rect() {
        let mut d = Doll::new("doll_test", &styles());
        assert!(d.rect().is_none());
        d.seat(Some(&seat(92.0, 92.0)));
        assert_eq!(d.rect().map(|r| r.size), Some(Vec2::new(92.0, 92.0)));
        d.unseat();
        assert!(d.rect().is_none(), "an off-page doll reserves nothing");
    }
}
