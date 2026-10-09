//! The **lit preview** — the material as a surface, not as six flat images.
//!
//! Roughness, metalness and normal relief barely read in a flat swatch: they are
//! all statements about how light LEAVES a surface, so judging them needs light
//! moving across one. This is the view that makes the output stage dialable
//! instead of guesswork.
//!
//! # It reuses the swatch's textures
//!
//! The bench already keeps the six maps on the GPU, each created in the right
//! format (base colour sRGB, the rest linear — which is exactly the split
//! [`draw_textured_mesh_pbr`](Renderer::draw_textured_mesh_pbr) needs). So this
//! module uploads nothing: it borrows those same handles. The maps are the 3×3
//! tiled buffers, so the sphere wears three repeats around it — the tiling reads
//! here too, and a seam would show as a stripe down the surface.
//!
//! # A turntable, not a camera
//!
//! The sample spins under a fixed light rather than the camera orbiting it. That
//! is the right control for a swatch (you want the highlight to travel across the
//! surface), it needs no camera class, and it means this is not a fifth copy of
//! the `OrbitCam` already duplicated across four scenes.
//!
//! # What it draws is the authored stage
//!
//! The look — lighting, clear, framing — is the [`StageDef`] the bench names
//! ([`STAGE_SOURCE`]), compiled by the one stage compiler; the `material` layer IS
//! the sample body. A stage that authors no `material` layer draws nothing and says
//! so, because a declaration nothing consumes is how this view once shipped lit by
//! a constant while its own `"lighting": "studio"` sat unused.

use flicker::render::{
    build_textured_verts, CompositeTarget, FrameGraph, Mat4, MeshDrawOptions, MeshIndices, PbrMaps,
    Rect, RenderTargetHandle, Renderer, StageDef, StageInputs, TextureHandle, TexturedMeshHandle,
    Vec2,
};
use flicker::ui::SurfaceSlot;

/// The stage source this view is authored under (`stages.<source>` in the loaded
/// styles). The look — lighting, camera framing — is DATA there, not constants here.
pub const STAGE_SOURCE: &str = "sablework_lit";
/// The one layer kind the lit preview draws: the material sample body.
const LIT_LAYERS: &[&str] = &["material"];
/// Turns per second of the turntable. Slow: the point is to watch a highlight
/// travel, and a fast spin reads as motion rather than as surface.
const SPIN_RATE: f32 = 0.15;

/// Which body the material is shown on.
///
/// Both are needed and neither is redundant: a sphere sweeps every surface angle
/// past the light at once, which is how you judge roughness and metalness; a
/// plane shows the texture undistorted, which is how you judge the pattern and
/// the seam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Body {
    #[default]
    Sphere,
    Plane,
}

impl Body {
    pub fn toggled(self) -> Self {
        match self {
            Body::Sphere => Body::Plane,
            Body::Plane => Body::Sphere,
        }
    }

    /// Stable id — the `$token` suffix the button's label resolves through.
    pub fn id(self) -> &'static str {
        match self {
            Body::Sphere => "sphere",
            Body::Plane => "plane",
        }
    }
}

/// One sample body's geometry: parallel position / normal / UV arrays, one entry
/// per vertex of a triangle soup. Named because the bare tuple is what
/// `build_textured_verts` consumes and it appears at every producer.
type Soup = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>);

/// A UV sphere as a triangle SOUP (three sequential vertices per triangle).
///
/// Soup rather than indexed geometry because [`build_textured_verts`] assigns one
/// tangent per consecutive triple; sharing vertices across triangles would average
/// tangents across the UV seam and put a visible crease down the sphere exactly
/// where the texture wraps.
fn sphere_soup(sectors: usize, stacks: usize) -> Soup {
    let (mut pos, mut nrm, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    let at = |i: usize, j: usize| -> ([f32; 3], [f32; 2]) {
        let u = i as f32 / sectors as f32;
        let v = j as f32 / stacks as f32;
        let (phi, theta) = (u * std::f32::consts::TAU, v * std::f32::consts::PI);
        let p = [
            theta.sin() * phi.cos(),
            theta.cos(),
            theta.sin() * phi.sin(),
        ];
        // `1 - v` so the texture's top row lands at the sphere's north pole, the
        // same top-origin convention the rig format uses.
        (p, [u, 1.0 - v])
    };
    for j in 0..stacks {
        for i in 0..sectors {
            let (a, auv) = at(i, j);
            let (b, buv) = at(i + 1, j);
            let (c, cuv) = at(i + 1, j + 1);
            let (d, duv) = at(i, j + 1);
            for (p, t) in [(a, auv), (b, buv), (c, cuv), (a, auv), (c, cuv), (d, duv)] {
                pos.push(p);
                // A unit sphere's position IS its normal.
                nrm.push(p);
                uv.push(t);
            }
        }
    }
    (pos, nrm, uv)
}

/// A unit plane standing upright, as a triangle soup.
fn plane_soup() -> Soup {
    let s = 1.15;
    let quad = [
        ([-s, -s], [0.0, 1.0]),
        ([s, -s], [1.0, 1.0]),
        ([s, s], [1.0, 0.0]),
        ([-s, -s], [0.0, 1.0]),
        ([s, s], [1.0, 0.0]),
        ([-s, s], [0.0, 0.0]),
    ];
    let (mut pos, mut nrm, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    for ([x, y], t) in quad {
        pos.push([x, y, 0.0]);
        nrm.push([0.0, 0.0, 1.0]);
        uv.push(t);
    }
    (pos, nrm, uv)
}

/// The lit preview's GPU resources and its turntable phase.
pub struct LitPreview {
    sphere: Option<TexturedMeshHandle>,
    plane: Option<TexturedMeshHandle>,
    target: Option<RenderTargetHandle>,
    size: (u32, u32),
    /// The stage's undrawn layers were named once; say it once, not per frame.
    layers_checked: bool,
    /// Turntable angle, radians. Advanced by `dt` so the spin is frame-rate
    /// independent.
    spin: f32,
    pub body: Body,
    pub spinning: bool,
}

impl Default for LitPreview {
    fn default() -> Self {
        Self {
            sphere: None,
            plane: None,
            target: None,
            size: (0, 0),
            layers_checked: false,
            spin: 0.0,
            body: Body::default(),
            spinning: true,
        }
    }
}

impl LitPreview {
    /// Upload the two sample bodies, once.
    ///
    /// Called LAZILY from [`render`](Self::render) rather than from the scene's
    /// `enter`. That is deliberate: an explicit init step is a wiring order the
    /// next caller can forget, and forgetting it here renders NOTHING — no pass,
    /// no composite, just the well's own dark panel, with no error to follow.
    /// This shipped exactly that way once. Building on first use removes the step.
    fn ensure_built(&mut self, r: &mut Renderer) {
        if self.sphere.is_some() {
            return;
        }
        let mut upload = |(pos, nrm, uv): Soup| {
            let verts = build_textured_verts(0..pos.len(), |i| pos[i], |i| nrm[i], |i| uv[i]);
            let indices: Vec<u32> = (0..verts.len() as u32).collect();
            r.upload_textured_mesh(&verts, MeshIndices::U32(&indices))
        };
        self.sphere = Some(upload(sphere_soup(48, 24)));
        self.plane = Some(upload(plane_soup()));
    }

    /// Whether the sample bodies are on the GPU yet.
    pub fn built(&self) -> bool {
        self.sphere.is_some() && self.plane.is_some()
    }

    /// Advance the turntable.
    pub fn tick(&mut self, dt: std::time::Duration) {
        if self.spinning {
            self.spin = (self.spin + dt.as_secs_f32() * SPIN_RATE * std::f32::consts::TAU)
                .rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Declare this frame's offscreen pass and composite it into the seat the walker
    /// reserved for the bench's `surface` node — at the node's own sub-layer above
    /// `base_layer`, with its tint, and honouring its rate (a `poster` surface keeps its
    /// last image: the poster rule).
    ///
    /// `maps` are the bench's six preview textures in [`MapKind::ALL`] order — the
    /// SAME handles the flat swatch draws, so nothing is uploaded twice.
    pub fn render(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'_>,
        slot: &SurfaceSlot,
        base_layer: f32,
        maps: &[TextureHandle],
        stage: &StageDef,
    ) {
        if !self.layers_checked {
            self.layers_checked = true;
            let undrawn = stage.layers_outside(LIT_LAYERS);
            if !undrawn.is_empty() {
                tracing::warn!(
                    "stages.{STAGE_SOURCE} authors {undrawn:?} layers the lit preview does not draw"
                );
            }
            if !stage.has_layer("material") {
                tracing::warn!(
                    "stages.{STAGE_SOURCE} authors no `material` layer — the sample is not drawn"
                );
            }
            // The framing is the stage's, applied by the frame graph from the definition
            // — so a source that authors none leaves the sample at whatever camera the
            // scene last set, which is a picture nobody chose.
            if stage.camera.is_none() {
                tracing::warn!(
                    "stages.{STAGE_SOURCE} authors no `camera` — the sample is framed by \
                     whatever set the camera last"
                );
            }
        }
        if !stage.has_layer("material") {
            return;
        }
        self.ensure_built(r);
        let rect = Rect {
            pos: Vec2::new(slot.x, slot.y),
            size: Vec2::new(slot.w, slot.h),
        };
        let (w, h) = stage.attachments.pixels(rect.size);
        match self.target {
            Some(_) if self.size == (w, h) => {}
            Some(t) => {
                r.resize_render_target(t, w, h);
                self.size = (w, h);
            }
            None => {
                self.target = Some(r.create_render_target(w, h));
                self.size = (w, h);
            }
        }
        let (Some(target), Some(mesh)) = (
            self.target,
            match self.body {
                Body::Sphere => self.sphere,
                Body::Plane => self.plane,
            },
        ) else {
            return;
        };
        // Base colour is index 0; the PBR maps follow `MapKind::ALL`. `Height` has
        // no slot in the pipeline — it is terrain displacement data, not a shading
        // input — so it is deliberately not bound. `Emit` (index 6) IS bound: the
        // whole point of a glow is that you can see it on the lit sample.
        let Some(&albedo) = maps.first() else { return };
        let pbr = PbrMaps {
            normal: maps.get(1).copied(),
            roughness: maps.get(2).copied(),
            metalness: maps.get(3).copied(),
            ao: maps.get(4).copied(),
            emit: maps.get(6).copied(),
        };
        // The SAMPLE turns, not the camera: the light stays put and the highlight travels
        // across the surface, which is the whole point of the view. The stage's lighting and
        // framing are applied by the graph from the definition. Liveness is the seat's
        // `rate`, driven by the renderer's per-surface clock — a poster keeps its last image
        // (the composite below still runs); a `live` sample turns every frame.
        let model = Mat4::from_rotation_y(self.spin);
        fg.surface(
            CompositeTarget::Target(target),
            stage,
            StageInputs::default(),
            slot.rate,
            move |r| {
                r.draw_textured_mesh_pbr(mesh, albedo, pbr, model, MeshDrawOptions::default());
            },
        );
        fg.composite_panel(
            target,
            CompositeTarget::Screen,
            rect,
            base_layer + slot.layer,
            slot.tint,
            None,
            None,
        );
    }

    /// Give the target back — a bench that leaves the Lit tab, or the scene, holds
    /// GPU memory for a picture nobody is looking at otherwise.
    pub fn free(&mut self, r: &mut Renderer) {
        if let Some(t) = self.target.take() {
            r.free_render_target(t);
            self.size = (0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::render::Vec3;

    /// Both bodies must be triangle SOUP — a whole number of triangles, one tangent
    /// per triple. A shared-vertex mesh would average tangents across the UV seam
    /// and crease the sphere exactly where the texture wraps.
    #[test]
    fn both_bodies_are_whole_triangle_soups() {
        for (pos, nrm, uv) in [sphere_soup(12, 6), plane_soup()] {
            assert_eq!(pos.len(), nrm.len());
            assert_eq!(pos.len(), uv.len());
            assert!(
                pos.len() >= 3 && pos.len() % 3 == 0,
                "{} vertices is not soup",
                pos.len()
            );
        }
    }

    /// The sphere is a UNIT sphere with outward normals — the pipeline lights it
    /// from the vertex normal, so an inward one would render inside-out.
    #[test]
    fn the_sphere_is_unit_with_outward_normals() {
        let (pos, nrm, uv) = sphere_soup(16, 8);
        for (p, n) in pos.iter().zip(&nrm) {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "radius {len}");
            let dot = p[0] * n[0] + p[1] * n[1] + p[2] * n[2];
            assert!(dot > 0.0, "normal points inward");
        }
        for [u, v] in uv {
            assert!(
                (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v),
                "uv {u},{v} off-sheet"
            );
        }
    }

    /// The turntable wraps and is frame-rate independent — the same elapsed time in
    /// one step or many must land on the same angle, or the spin speed would depend
    /// on how the frames happened to fall.
    #[test]
    fn the_turntable_wraps_and_ignores_frame_rate() {
        let mut a = LitPreview::default();
        a.tick(std::time::Duration::from_secs_f32(1.0));
        let mut b = LitPreview::default();
        for _ in 0..60 {
            b.tick(std::time::Duration::from_secs_f32(1.0 / 60.0));
        }
        assert!((a.spin - b.spin).abs() < 1e-3, "{} vs {}", a.spin, b.spin);
        assert!(
            a.spin < std::f32::consts::TAU,
            "the angle must stay wrapped"
        );

        // Stopped means stopped.
        let before = a.spin;
        a.spinning = false;
        a.tick(std::time::Duration::from_secs_f32(1.0));
        assert_eq!(a.spin, before);
    }

    /// THE AUTHORED STAGE IS ACTUALLY READ, and it lights — and frames — something.
    ///
    /// The view first shipped BLACK for two reasons at once: the sample meshes
    /// were never built (an init step nobody called), and the `"lighting":
    /// "studio"` this bench authored was ignored in favour of a Rust constant. A
    /// stage config nothing reads is an authored name that resolves to nothing —
    /// so the shipped stage must compile, be lit, frame the sample, and author the
    /// one layer this view draws.
    #[test]
    fn the_authored_stage_is_read_and_lights_the_sample() {
        // The scene's own style blocks ride the shipped scene file (the five-line
        // split); the stage block lives in the ui_stages.json satellite until the
        // renderer campaign moves scene-owned stages into the scene file.
        let def = flicker::ui::SceneDef::parse("sablework", crate::SW_SCENE)
            .expect("the shipped sablework.scene.json parses");
        let styles = flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        );
        let stage = flicker::ui::stage_def(&styles, STAGE_SOURCE)
            .unwrap_or_else(|| panic!("stages.{STAGE_SOURCE} is not authored"));

        // It must actually EMIT light. All-black terms render a black panel,
        // which is indistinguishable from the view being broken.
        let lum = |v: Vec3| v.x + v.y + v.z;
        assert!(
            lum(stage.lighting.sky_sun().color) > 0.1,
            "the stage has no sun"
        );
        assert!(
            lum(stage.lighting.ambient) > 0.0,
            "the stage has no ambient floor"
        );
        assert!(
            stage.lighting.sky_sun().direction.length() > 0.5,
            "the sun points nowhere"
        );
        let cam = stage.camera.expect("the lit stage frames its sample");
        assert!(cam.dist > 0.0, "the camera sits on the sample");
        assert!(
            stage.has_layer("material"),
            "the stage authors the `material` layer this view draws"
        );
        assert!(
            stage.layers_outside(LIT_LAYERS).is_empty(),
            "and nothing the lit preview cannot draw"
        );
    }

    #[test]
    fn the_body_toggle_round_trips() {
        let b = Body::default();
        assert_eq!(b.toggled().toggled(), b);
        assert_ne!(Body::Sphere.id(), Body::Plane.id());
    }
}
