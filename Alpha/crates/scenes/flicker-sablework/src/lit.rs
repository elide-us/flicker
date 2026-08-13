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

use flicker::render::{
    build_textured_verts, Camera, CompositeTarget, FrameGraph, Mat4, MeshDrawOptions, MeshIndices,
    PbrMaps, Rect, RenderTargetHandle, Renderer, SceneLighting, TextureHandle, TexturedMeshHandle,
    Vec3,
};

/// The stage source this view is authored under, in `ui_theme.json`'s `stages`
/// block. The look — lighting, camera framing — is DATA there, not constants here.
pub const STAGE_SOURCE: &str = "sablework_lit";
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
            spin: 0.0,
            body: Body::default(),
            spinning: true,
        }
    }
}

/// The authored look of the lit stage: where the light comes from and how the
/// sample is framed.
///
/// Read from `stages.<source>` rather than hardcoded, because that block is what
/// the bench DECLARES — a config nothing reads is an authored name that resolves
/// to nothing, which is how this view first shipped lit by a constant while its
/// own `"lighting": "studio"` sat unused.
#[derive(Clone, Copy, Debug)]
pub struct Stage {
    pub lighting: SceneLighting,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
}

impl Default for Stage {
    fn default() -> Self {
        Self { lighting: SceneLighting::default(), yaw: 0.6, pitch: 0.28, dist: 2.6 }
    }
}

impl Stage {
    /// Parse `stages.<source>` out of the loaded `ui_theme.json`.
    ///
    /// Best-effort: anything missing keeps the default, because a malformed style
    /// file must not leave the bench with a black panel and no explanation. What
    /// it must never do is silently ignore a value that IS authored.
    pub fn from_styles(styles: &serde_json::Value, source: &str) -> Self {
        let mut out = Stage::default();
        let Some(stage) = styles.get("stages").and_then(|s| s.get(source)) else {
            tracing::warn!("stages.{source} is not authored — the lit view uses defaults");
            return out;
        };
        // `lighting` NAMES a block in the shared `stages.lighting` table, the same
        // indirection every other stage source uses.
        if let Some(name) = stage.get("lighting").and_then(|v| v.as_str()) {
            match styles.get("stages").and_then(|s| s.get("lighting")).and_then(|l| l.get(name)) {
                Some(l) => {
                    let v3 = |k: &str| -> Option<Vec3> {
                        let a = l.get(k)?.as_array()?;
                        Some(Vec3::new(
                            a.first()?.as_f64()? as f32,
                            a.get(1)?.as_f64()? as f32,
                            a.get(2)?.as_f64()? as f32,
                        ))
                    };
                    if let Some(v) = v3("sun_dir") {
                        out.lighting.sun_dir = v.normalize_or_zero();
                    }
                    if let Some(v) = v3("sun") {
                        out.lighting.sun_color = v;
                    }
                    if let Some(v) = v3("moon_dir") {
                        out.lighting.moon_dir = v.normalize_or_zero();
                    }
                    if let Some(v) = v3("moon") {
                        out.lighting.moon_color = v;
                    }
                    if let Some(v) = v3("ambient") {
                        out.lighting.ambient = v;
                    }
                }
                None => tracing::warn!("stages.lighting.{name} is not authored"),
            }
        }
        if let Some(cam) = stage.get("camera") {
            let f = |k: &str| cam.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
            out.yaw = f("yaw").unwrap_or(out.yaw);
            out.pitch = f("pitch").unwrap_or(out.pitch);
            out.dist = f("dist").unwrap_or(out.dist);
        }
        out
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
            self.spin =
                (self.spin + dt.as_secs_f32() * SPIN_RATE * std::f32::consts::TAU).rem_euclid(std::f32::consts::TAU);
        }
    }

    /// Declare this frame's offscreen pass and composite it into `rect`.
    ///
    /// `maps` are the bench's six preview textures in [`MapKind::ALL`] order — the
    /// SAME handles the flat swatch draws, so nothing is uploaded twice.
    pub fn render(
        &mut self,
        r: &mut Renderer,
        fg: &mut FrameGraph<'_>,
        rect: Rect,
        layer: f32,
        maps: &[TextureHandle],
        stage: Stage,
    ) {
        self.ensure_built(r);
        let w = (rect.size.x.round() as u32).max(1);
        let h = (rect.size.y.round() as u32).max(1);
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
        let cam = Camera::orbit(Vec3::ZERO, stage.dist, stage.yaw, stage.pitch);
        let lighting = stage.lighting;
        // The SAMPLE turns, not the camera: the light stays put and the highlight
        // travels across the surface, which is the whole point of the view.
        let model = Mat4::from_rotation_y(self.spin);
        fg.target(target, [0.0, 0.0, 0.0, 0.0], move |r| {
            r.set_scene(lighting);
            r.set_camera(&cam);
            r.draw_textured_mesh_pbr(mesh, albedo, pbr, model, MeshDrawOptions::default());
        });
        fg.composite_panel(target, CompositeTarget::Screen, rect, layer, [1.0; 4], None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both bodies must be triangle SOUP — a whole number of triangles, one tangent
    /// per triple. A shared-vertex mesh would average tangents across the UV seam
    /// and crease the sphere exactly where the texture wraps.
    #[test]
    fn both_bodies_are_whole_triangle_soups() {
        for (pos, nrm, uv) in [sphere_soup(12, 6), plane_soup()] {
            assert_eq!(pos.len(), nrm.len());
            assert_eq!(pos.len(), uv.len());
            assert!(pos.len() >= 3 && pos.len() % 3 == 0, "{} vertices is not soup", pos.len());
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
            assert!((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v), "uv {u},{v} off-sheet");
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
        assert!(a.spin < std::f32::consts::TAU, "the angle must stay wrapped");

        // Stopped means stopped.
        let before = a.spin;
        a.spinning = false;
        a.tick(std::time::Duration::from_secs_f32(1.0));
        assert_eq!(a.spin, before);
    }

    /// THE AUTHORED STAGE IS ACTUALLY READ, and it lights something.
    ///
    /// The view first shipped BLACK for two reasons at once: the sample meshes
    /// were never built (an init step nobody called), and the `"lighting":
    /// "studio"` this bench authored was ignored in favour of a Rust constant. A
    /// stage config nothing reads is an authored name that resolves to nothing.
    #[test]
    fn the_authored_stage_is_read_and_lights_the_sample() {
        let styles = flicker::ui::load_styles_for(
            concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/sensorium/resources/ui_theme.json"
            ),
            Some(&crate::scene_styles()),
        );
        let stage = Stage::from_styles(&styles, STAGE_SOURCE);

        // It must differ from the bare default, or nothing was actually read.
        let fallback = Stage::default();
        assert!(
            stage.lighting.sun_dir != fallback.lighting.sun_dir
                || stage.lighting.ambient != fallback.lighting.ambient,
            "stages.{STAGE_SOURCE} was not read — the sample is lit by a constant"
        );

        // And it must actually EMIT light. All-black terms render a black panel,
        // which is indistinguishable from the view being broken.
        let lum = |v: Vec3| v.x + v.y + v.z;
        assert!(lum(stage.lighting.sun_color) > 0.1, "the stage has no sun");
        assert!(lum(stage.lighting.ambient) > 0.0, "the stage has no ambient floor");
        assert!(stage.lighting.sun_dir.length() > 0.5, "the sun points nowhere");
        assert!(stage.dist > 0.0, "the camera sits on the sample");
    }

    /// An unauthored source falls back rather than going dark — a malformed style
    /// file must not leave a black panel with no explanation.
    #[test]
    fn an_unknown_stage_falls_back_to_a_lit_default() {
        let stage = Stage::from_styles(&serde_json::json!({}), "nope");
        assert!(stage.lighting.sun_color.x > 0.0, "the fallback is unlit");
    }

    #[test]
    fn the_body_toggle_round_trips() {
        let b = Body::default();
        assert_eq!(b.toggled().toggled(), b);
        assert_ne!(Body::Sphere.id(), Body::Plane.id());
    }
}
