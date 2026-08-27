//! Public 3D-mesh types: `MeshVertex`, `MeshHandle`, `MeshIndices`,
//! `MeshDrawOptions`, and `Camera`.
//!
//! The mesh pipeline lives in `pipeline_mesh.rs`; this module just defines
//! the data types that cross the renderer's public boundary.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

/// One vertex of a 3D mesh.
///
/// Byte layout is `28` bytes — `position[3] + normal[3] + material[1]`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// Opaque handle to an uploaded mesh stored on the renderer. Meshes
/// persist across frames; only the per-frame draw queue resets.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeshHandle(pub(crate) u32);

/// Borrowed view of an index buffer; passed to `Renderer::upload_mesh`.
/// The renderer picks the matching `wgpu::IndexFormat` automatically.
pub enum MeshIndices<'a> {
    U16(&'a [u16]),
    U32(&'a [u32]),
}

impl MeshIndices<'_> {
    pub fn len(&self) -> usize {
        match self {
            MeshIndices::U16(v) => v.len(),
            MeshIndices::U32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Per-`draw_mesh` options. Defaults: solid fill, no tint.
#[derive(Copy, Clone, Debug)]
pub struct MeshDrawOptions {
    /// `false` — render filled triangles with Lambertian shading from the
    /// material-hash base color.
    /// `true` — render the same triangles as a barycentric-edge overlay
    /// (fixed wireframe color, fragments away from triangle edges are
    /// discarded). The same mesh handle can be drawn twice — once filled,
    /// once as wires — for a fill + wireframe overlay effect.
    pub wireframe: bool,
    /// Multiplied with the Lambertian-shaded base color. `[1.0; 4]` is
    /// "no tint".
    pub tint: [f32; 4],
    /// Glossiness `0..1`: `0` is matte (Lambertian only, the default); higher adds a soft **limb
    /// sheen** from the rig's FIRST non-directional light (a star, a fire) for liquid / icy /
    /// wet-looking surfaces — a Fresnel grazing-edge brightening, *not* a mirror hot-spot
    /// (which reads as a marble at planet scale). The sheen strengthens with gloss.
    pub gloss: f32,
}

impl Default for MeshDrawOptions {
    fn default() -> Self {
        Self {
            wireframe: false,
            tint: [1.0, 1.0, 1.0, 1.0],
            gloss: 0.0,
        }
    }
}

/// 3D camera. Produces view and projection matrices given an aspect ratio.
///
/// Right-handed, Y-up — matches `flicker-voxel`'s coordinate convention.
/// Use `Camera::default()` for a sensible orbiting starter pose.
#[derive(Copy, Clone, Debug)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
    /// `None` = perspective (uses `fov_y_radians`). `Some(h)` = ORTHOGRAPHIC with vertical view
    /// `height` = `h` world units (the horizontal extent follows the aspect ratio) — the editor's
    /// front/side/top panels. All existing constructors default this to `None`.
    pub ortho_height: Option<f32>,
}

impl Camera {
    /// World-to-camera view matrix.
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    /// Camera-to-clip projection matrix. `aspect` is `width / height`. Orthographic when
    /// `ortho_height` is set (a box `height` tall, `height × aspect` wide), else perspective.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        match self.ortho_height {
            Some(height) => {
                let half_h = height * 0.5;
                let half_w = half_h * aspect;
                Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, self.near, self.far)
            }
            None => Mat4::perspective_rh(self.fov_y_radians, aspect, self.near, self.far),
        }
    }

    /// Combined `projection × view`. Multiplied with a model matrix
    /// per draw to produce the final clip-space transform.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// World-space pick ray `(origin, unit dir)` through a cursor pixel, or `None` for a
    /// degenerate viewport. Unprojects through the INVERSE view-projection rather than
    /// rebuilding a basis from the camera's forward vector, so it is convention-proof:
    /// it cannot drift out of step with `view_projection`, and it works for any camera
    /// (orbit or fly) without knowing which. `cursor` is in pixels, y-down from the
    /// top-left — the usual window convention.
    ///
    /// Promoted from the copies in `flicker-pocclusters` / `examples/voxel-cluster`
    /// (`build_pick_ray`) and `flicker-packeditor` (`pick_node`) — 2026-07-16. Those
    /// predate this and can migrate onto it; nothing new should hand-roll a fourth.
    pub fn pick_ray(&self, cursor: Vec2, viewport: Vec2) -> Option<(Vec3, Vec3)> {
        if viewport.x <= 0.0 || viewport.y <= 0.0 {
            return None;
        }
        let inv = self.view_projection(viewport.x / viewport.y).inverse();
        // +0.5 puts the ray through the pixel's CENTRE, not its top-left corner.
        let ndc = Vec2::new(
            2.0 * (cursor.x + 0.5) / viewport.x - 1.0,
            1.0 - 2.0 * (cursor.y + 0.5) / viewport.y,
        );
        // wgpu NDC z ∈ [0,1]: 0 = near plane, 1 = far.
        let near = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let far = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
        let dir = (far - near).normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        Some((near, dir))
    }
}

/// Ray–triangle intersection (Möller–Trumbore). Returns the parametric `t` along
/// `(origin, dir)` for the FRONT-face hit, or `None` if the ray misses, hits the back face
/// within numerical tolerance, or lands behind the origin. Front-face-only matches what the
/// renderer actually shows, so a pick can't select a surface the viewer can't see.
///
/// Promoted 2026-07-16 from byte-identical copies in `flicker-pocclusters` and
/// `examples/voxel-cluster`; both can migrate onto this.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    // Back-face / parallel-ray rejection. Positive det = front face (CCW from `origin`).
    if det <= 1e-7 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let bu = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&bu) {
        return None;
    }
    let q = s.cross(edge1);
    let bv = inv_det * dir.dot(q);
    if bv < 0.0 || bu + bv > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    if t > 1e-4 {
        Some(t)
    } else {
        None
    }
}

impl Camera {
    /// Camera positioned to orbit `target` at `distance`, looking
    /// inward. `yaw` rotates around the world Y axis; `pitch` is
    /// elevation in radians (positive = looking down from above).
    /// `pitch` is clamped to `(-1.5, 1.5)` radians to avoid gimbal
    /// flip near the poles. Other camera parameters (`up`, FOV, near,
    /// far) inherit from [`Camera::default`].
    pub fn orbit(target: Vec3, distance: f32, yaw: f32, pitch: f32) -> Self {
        let pitch = pitch.clamp(-1.5, 1.5);
        let position = target
            + Vec3::new(
                distance * pitch.cos() * yaw.sin(),
                distance * pitch.sin(),
                distance * pitch.cos() * yaw.cos(),
            );
        Self {
            position,
            target,
            up: Vec3::Y,
            ..Self::default()
        }
    }

    /// Turn this camera ORTHOGRAPHIC with the given vertical view `height` (world units) — for the
    /// editor's front/side/top panels. Keeps position/target/up (the view direction) unchanged.
    pub fn with_ortho_height(mut self, height: f32) -> Self {
        self.ortho_height = Some(height);
        self
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(300.0, 200.0, 300.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
            ortho_height: None,
        }
    }
}

/// How many lights one [`LightRig`] carries — the fixed roster the `Scene` uniform
/// ships and the lit shaders loop over. Today's ceiling was 3 (sun/moon/point) and a
/// fireplace room wants 4–6; the loop is `count`-bounded, so empty slots cost nothing
/// per fragment, and raising this is a one-line change with no shader edit.
pub const MAX_LIGHTS: usize = 8;

/// What a [`Light`] *is* — which of `direction` / `position` / `cone_*` it reads.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// Parallel light from infinity (sun, moon). Reads `direction` only; no falloff.
    Dir,
    /// Omnidirectional light at `position`. Reads `position` + `radius`.
    Point,
    /// A cone from `position` along `direction`. Reads `position`, `direction`,
    /// `radius`, `cone_inner`/`cone_outer`.
    Spot,
}

/// How a [`Driver`] varies its light's intensity over the stage clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DriverKind {
    /// Seeded 1-D value noise that only ever DIMS — a fire, a failing lamp.
    Flicker,
    /// A sine — a beacon, a heartbeat.
    Pulse,
}

/// A per-light intensity modulation, evaluated CPU-side once per stage per frame
/// ([`LightRig::driven`]). CPU-only: what reaches the GPU is the already-driven
/// intensity, so a shader never carries a clock.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Driver {
    pub kind: DriverKind,
    /// Cycles (pulse) / noise samples (flicker) per second of stage clock.
    pub speed: f32,
    /// How deep the modulation cuts. `0.0` ⇒ the gain is the literal `1.0`, so the
    /// light is bit-for-bit undriven.
    pub depth: f32,
    /// Seeds the noise / the pulse phase, so two lamps in a room are not in lockstep.
    pub seed: u32,
}

impl Driver {
    /// This driver's intensity gain at stage time `t` (seconds). Pure and
    /// deterministic in `(kind, speed, depth, seed, t)` — no wall clock, no state.
    ///
    /// `depth == 0.0` returns the literal `1.0`, which is what makes an undriven
    /// light's intensity survive [`LightRig::driven`] unchanged in every bit.
    pub fn gain(&self, t: f32) -> f32 {
        if self.depth == 0.0 {
            return 1.0;
        }
        match self.kind {
            // A sine about 1.0 — symmetric, so a pulse both brightens and dims.
            DriverKind::Pulse => {
                let phase = hash01(self.seed, -1) * std::f32::consts::TAU;
                1.0 + self.depth * (std::f32::consts::TAU * self.speed * t + phase).sin()
            }
            // Seeded value noise, mapped so the gain never exceeds 1.0: a fire dims,
            // it does not overshoot the radiance the author set.
            DriverKind::Flicker => {
                let u = t * self.speed;
                let cell = u.floor();
                let f = u - cell;
                let i = cell as i32;
                let (a, b) = (hash01(self.seed, i), hash01(self.seed, i.wrapping_add(1)));
                let n = a + (b - a) * (f * f * (3.0 - 2.0 * f)); // smoothstep interpolation
                1.0 - self.depth * (1.0 - n)
            }
        }
    }
}

/// Integer hash → `[0, 1)`. A splitmix-style avalanche over `(seed, cell)`, so a
/// driver's noise is reproducible from its seed alone on any machine.
fn hash01(seed: u32, cell: i32) -> f32 {
    let mut x = (cell as u32)
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(seed.wrapping_mul(0x85EB_CA6B));
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    // 24 bits of mantissa — exactly representable, so the division is exact.
    (x >> 8) as f32 / 16_777_216.0
}

/// ONE light of a [`LightRig`]. The rig is a LIST: a sun is a `Dir` light and a
/// brazier is a `Point` light, in the same array, shaded by the same loop.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    /// Radiance hue (linear RGB). The magnitude an author wants rides `intensity`.
    pub color: Vec3,
    /// Scalar multiplier on `color`. `1.0` is the legacy convention (colour carries
    /// the magnitude); a light with real falloff wants it in the tens.
    pub intensity: f32,
    /// World position — `Point` / `Spot` only.
    pub position: Vec3,
    /// `Dir`: the direction **toward** the light (normalized). `Spot`: the cone axis,
    /// pointing **away** from the light.
    pub direction: Vec3,
    /// Falloff radius. `<= 0.0` ⇒ **no falloff at all** (the absence of an authored
    /// radius, not a sentinel) — which is how today's falloff-less point light
    /// survives the move to the list unchanged.
    pub radius: f32,
    /// Cone half-angle in RADIANS, `Spot` only: full brightness inside it.
    pub cone_inner: f32,
    /// Cone half-angle in RADIANS, `Spot` only: zero outside it, `smoothstep` from
    /// `cone_outer` to `cone_inner` between.
    pub cone_outer: f32,
    /// Optional intensity modulation, applied by [`LightRig::driven`].
    pub driver: Option<Driver>,
}

impl Default for Light {
    /// A black directional light pointing straight up: present, states every term,
    /// contributes nothing. The rig's empty slots and its "off" lights are this.
    fn default() -> Self {
        Self {
            kind: LightKind::Dir,
            color: Vec3::ZERO,
            intensity: 1.0,
            position: Vec3::ZERO,
            direction: Vec3::Y,
            radius: 0.0,
            cone_inner: 0.0,
            cone_outer: 0.0,
            driver: None,
        }
    }
}

impl Light {
    /// A directional light: `direction` points TOWARD the light.
    pub fn dir(direction: Vec3, color: Vec3) -> Self {
        Self {
            kind: LightKind::Dir,
            color,
            direction,
            ..Self::default()
        }
    }

    /// A point light at `position`. `radius <= 0` ⇒ no distance falloff.
    pub fn point(position: Vec3, color: Vec3) -> Self {
        Self {
            kind: LightKind::Point,
            color,
            position,
            ..Self::default()
        }
    }

    /// What actually reaches a surface before falloff: `color * intensity`. With the
    /// legacy `intensity = 1.0` this is `color` in every bit.
    pub fn radiance(&self) -> Vec3 {
        self.color * self.intensity
    }
}

/// Frame-global lighting & atmosphere — the LIGHT LIST plus the sky/fog state one
/// stage renders under. Set once per frame with [`crate::Renderer::set_scene`]; the
/// renderer fills in the camera position itself (for fog distance), so callers supply
/// only the lights, ambient, and fog.
///
/// The rig is ONE representation: there is no separate sun/moon/point — a sun is
/// `lights[0]` with [`LightKind::Dir`], and the sky pass reads that SLOT back through
/// [`LightRig::sky_sun`]. Slots 0 and 1 are the sky slots in one addressing scheme:
/// what `sky_sun`/`sky_moon` read, and what a celestial cycle composing over the rig
/// overwrites by index. A light below the horizon is faded by handing it a near-black
/// colour (no explicit night branch in the shader). `fog_*` IS implemented (the mesh
/// shaders apply it; `fog_density` 0 leaves it inert). The colour GRADE is not here at
/// all — it is pass-owned by [`TonemapGradePass`](crate::TonemapGradePass), the ONE
/// representation of it.
#[derive(Copy, Clone, Debug)]
pub struct LightRig {
    /// The roster. Only the first `count` entries are lit; the rest are inert.
    pub lights: [Light; MAX_LIGHTS],
    /// How many of `lights` this rig actually carries (`<= MAX_LIGHTS`).
    pub count: u8,
    /// Flat ambient floor added before the light terms.
    pub ambient: Vec3,
    /// Procedural-sky colour straight up (linear RGB). Used by the sky pass
    /// ([`crate::Renderer::draw_sky`]); ignored when no sky is requested.
    pub sky_zenith: Vec3,
    /// Procedural-sky colour at the horizon band (linear RGB). The sky pass
    /// gradients `sky_horizon`→`sky_zenith` by view elevation.
    pub sky_horizon: Vec3,
    /// Distance-fog colour (linear RGB) — the exponential distance fog the mesh
    /// shaders apply (`mesh.wgsl`); `fog_density` 0 leaves it inert.
    pub fog_color: Vec3,
    /// Distance-fog density. `0.0` ⇒ no fog.
    pub fog_density: f32,
    /// World→celestial rotation for the procedural night sky (stars + Milky
    /// Way). A view ray transformed by this lands in a sky-fixed frame, so the
    /// stars rotate with time of day and tilt with latitude. Identity leaves
    /// the star field locked to the camera. Only the sky pass uses it.
    pub star_rotation: Mat4,
}

impl Default for LightRig {
    /// Matches the renderer's seeded default — the pre-uniform hardcoded look, stated
    /// as the three lights the legacy `sun`/`moon`/`point` keys have always meant: a
    /// warm-white sun over a `0.3` ambient, a black moon, a black point light.
    fn default() -> Self {
        let mut lights = [Light::default(); MAX_LIGHTS];
        lights[0] = Light::dir(Vec3::new(0.5, 1.0, 0.3).normalize(), Vec3::splat(0.7));
        lights[1] = Light::dir(Vec3::Y, Vec3::ZERO);
        lights[2] = Light::point(Vec3::ZERO, Vec3::ZERO);
        Self {
            lights,
            count: 3,
            ambient: Vec3::splat(0.3),
            sky_zenith: Vec3::new(0.012, 0.016, 0.030),
            sky_horizon: Vec3::new(0.030, 0.040, 0.085),
            fog_color: Vec3::ZERO,
            fog_density: 0.0,
            star_rotation: Mat4::IDENTITY,
        }
    }
}

impl LightRig {
    /// Append one light. `false` (and a warn) when the roster is already full — a
    /// dropped light is loud, never silent.
    pub fn push(&mut self, light: Light) -> bool {
        let i = self.count as usize;
        if i >= MAX_LIGHTS {
            tracing::warn!("LightRig: {MAX_LIGHTS} lights already — {light:?} is dropped");
            return false;
        }
        self.lights[i] = light;
        self.count += 1;
        true
    }

    /// This rig at stage time `t`: every driven light's `intensity` scaled by its
    /// driver's gain. Pure, `Copy` → `Copy`, no allocation — called once per stage per
    /// frame by [`FrameGraph::surface`](crate::FrameGraph::surface). A rig with no
    /// drivers comes back bit-for-bit identical.
    pub fn driven(&self, t: f32) -> Self {
        let mut out = *self;
        for light in out.lights.iter_mut().take(self.count as usize) {
            if let Some(driver) = light.driver {
                light.intensity *= driver.gain(t);
            }
        }
        out
    }

    /// The rig's KEY light for the sky pass: **slot 0**.
    ///
    /// The sun and the moon are SLOTS, not a filtered order. `lights[0]` IS the sun and
    /// `lights[1]` IS the moon — ONE addressing scheme, the one the legacy compile order
    /// (`sun`→slot 0, `moon`→slot 1), the `hearth` idiom and
    /// `CelestialState::over` (which overwrites those two by INDEX every frame) all
    /// already assume. A slot past `count`, or one holding a non-[`LightKind::Dir`]
    /// light, yields a black [`Light`] so the sky darkens rather than reading something
    /// that is not a sky light. Parking a fixed light in slot 0 or 1 is reported as a
    /// problem by the preset compiler, so an eaten light is loud rather than missing.
    pub fn sky_sun(&self) -> Light {
        self.sky_slot(0)
    }

    /// The rig's moon for the sky pass: **slot 1**. See [`Self::sky_sun`].
    pub fn sky_moon(&self) -> Light {
        self.sky_slot(1)
    }

    fn sky_slot(&self, slot: usize) -> Light {
        let light = self.lights[slot];
        if slot < self.count as usize && light.kind == LightKind::Dir {
            light
        } else {
            Light::default()
        }
    }

    /// The orthographic view-projection of a directional light's SHADOW MAP — **the ONE
    /// matrix** the producer stage renders the casters with and the consumer's lit passes
    /// sample against, so the two can never disagree. `light_index` is the rig slot (a
    /// [`LightKind::Dir`] light, whose `direction` points TOWARD the light); `center` is
    /// the world point the box is fitted around (the field centre); `extent` is the
    /// half-size of the SQUARE box (world units). Reuses [`Camera`]'s orthographic
    /// projection at aspect 1 (shadow maps are square) — the projection is LINEAR in
    /// depth, so the generous near/far costs no precision. A slot holding a black/absent
    /// light falls back to a straight-down view rather than a degenerate matrix.
    pub fn shadow_view_proj(&self, light_index: usize, center: Vec3, extent: f32) -> Mat4 {
        let dir = self
            .lights
            .get(light_index)
            .map(|l| l.direction)
            .unwrap_or(Vec3::Y)
            .normalize_or_zero();
        let dir = if dir == Vec3::ZERO { Vec3::Y } else { dir };
        let extent = if extent.is_finite() && extent > 0.0 {
            extent
        } else {
            1.0
        };
        // Eye placed back along the toward-light direction; the box is 2·extent per side.
        let dist = extent * 2.0;
        let eye = center + dir * dist;
        // An up vector that is not parallel to the view direction (a near-vertical sun).
        let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let cam = Camera {
            position: eye,
            target: center,
            up,
            near: 0.1,
            far: dist + extent * 2.0,
            ..Camera::default()
        }
        .with_ortho_height(extent * 2.0);
        cam.view_projection(1.0)
    }
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    fn cam() -> Camera {
        Camera::orbit(Vec3::ZERO, 300.0, 0.0, 0.0)
    }

    /// The centre pixel's ray must run from the camera straight at its target — the one
    /// case we can assert independently of any convention.
    #[test]
    fn centre_pixel_ray_points_at_the_target() {
        let c = cam();
        let vp = Vec2::new(1920.0, 1080.0);
        let (o, d) = c
            .pick_ray(Vec2::new(vp.x * 0.5 - 0.5, vp.y * 0.5 - 0.5), vp)
            .unwrap();
        let want = (c.target - c.position).normalize();
        assert!(
            d.dot(want) > 0.9999,
            "centre ray must aim at the target: {d:?} vs {want:?}"
        );
        assert!(
            (o - c.position).length() < c.far,
            "ray starts near the camera, not behind it"
        );
    }

    /// A pick ray must actually hit geometry under the cursor, and the ray/triangle pair
    /// must agree — this is what a scene pick relies on end to end.
    #[test]
    fn centre_ray_hits_a_triangle_at_the_target() {
        let c = cam();
        let vp = Vec2::new(800.0, 600.0);
        let (o, d) = c
            .pick_ray(Vec2::new(vp.x * 0.5 - 0.5, vp.y * 0.5 - 0.5), vp)
            .unwrap();
        // A big quad-ish triangle spanning the origin, facing the camera (+Z).
        let (a, b, cc) = (
            Vec3::new(-50.0, -50.0, 0.0),
            Vec3::new(50.0, -50.0, 0.0),
            Vec3::new(0.0, 50.0, 0.0),
        );
        let t = ray_triangle(o, d, a, b, cc).expect("centre ray must hit a triangle at the origin");
        let hit = o + d * t;
        assert!(
            hit.length() < 1.0,
            "hit should land at the target, got {hit:?}"
        );
    }

    /// Back faces are rejected — a pick must not select a surface facing away.
    #[test]
    fn back_faces_are_rejected() {
        let (o, d) = (Vec3::new(0.0, 0.0, 100.0), -Vec3::Z);
        // Reversed winding = back face from this ray.
        let (a, b, c) = (
            Vec3::new(-50.0, -50.0, 0.0),
            Vec3::new(0.0, 50.0, 0.0),
            Vec3::new(50.0, -50.0, 0.0),
        );
        assert!(
            ray_triangle(o, d, a, b, c).is_none(),
            "back face must not be picked"
        );
    }

    /// Geometry behind the cursor must never be picked.
    #[test]
    fn geometry_behind_the_ray_is_rejected() {
        let (o, d) = (Vec3::new(0.0, 0.0, 100.0), Vec3::Z); // pointing AWAY from the origin
        let (a, b, c) = (
            Vec3::new(-50.0, -50.0, 0.0),
            Vec3::new(50.0, -50.0, 0.0),
            Vec3::new(0.0, 50.0, 0.0),
        );
        assert!(
            ray_triangle(o, d, a, b, c).is_none(),
            "geometry behind the ray must miss"
        );
    }
}

#[cfg(test)]
mod rig_tests {
    use super::*;

    /// The default rig IS the legacy sun/moon/point trio, in that order — the ONE
    /// mapping every legacy preset compiles through, so a slot that moved would be
    /// caught here rather than by a stage rendering from the wrong light.
    #[test]
    fn the_default_rig_is_the_legacy_trio_in_slot_order() {
        let rig = LightRig::default();
        assert_eq!(rig.count, 3, "sun, moon, point — black ones included");
        assert_eq!(rig.lights[0].kind, LightKind::Dir);
        assert_eq!(rig.lights[1].kind, LightKind::Dir);
        assert_eq!(rig.lights[2].kind, LightKind::Point);
        assert_eq!(rig.lights[2].radius, 0.0, "no falloff, exactly as before");
        for l in &rig.lights[..3] {
            assert_eq!(l.intensity, 1.0, "the colour carries the magnitude");
        }
        // The sky reads SLOTS 0 and 1 — the one addressing scheme.
        assert_eq!(rig.sky_sun(), rig.lights[0]);
        assert_eq!(rig.sky_moon(), rig.lights[1]);
        assert_eq!(rig.sky_sun().radiance(), rig.lights[0].color, "intensity 1");
    }

    /// **GATE — the sky slots are SLOTS.** A rig whose slot 0 is not a directional
    /// light hands the sky a black [`Light`] rather than hunting down some later
    /// directional one: `sky_sun`/`sky_moon` and a celestial cycle's index writes are
    /// ONE scheme, so the sky can never read a light the cycle is about to overwrite.
    /// A slot past `count` is black for the same reason. And `push` fills the roster
    /// then refuses, loudly.
    #[test]
    fn the_roster_is_bounded_and_a_missing_key_light_is_black() {
        let mut rig = LightRig {
            lights: [Light::default(); MAX_LIGHTS],
            count: 0,
            ..LightRig::default()
        };
        assert!(rig.push(Light::point(Vec3::ZERO, Vec3::ONE)));
        assert_eq!(
            rig.sky_sun().color,
            Vec3::ZERO,
            "slot 0 is a point light ⇒ black sun"
        );
        assert_eq!(
            rig.sky_moon().color,
            Vec3::ZERO,
            "slot 1 is past `count` ⇒ black moon"
        );
        // A directional light LATER in the roster is NOT the sun — slot 0 still is.
        assert!(rig.push(Light::dir(Vec3::Y, Vec3::ONE)));
        assert_eq!(
            rig.sky_sun().color,
            Vec3::ZERO,
            "a dir light in slot 1 is not promoted to the sun"
        );
        assert_eq!(rig.sky_moon(), rig.lights[1], "…it is the MOON slot");
        while rig.count < MAX_LIGHTS as u8 {
            assert!(rig.push(Light::point(Vec3::ZERO, Vec3::ONE)));
        }
        assert!(
            !rig.push(Light::point(Vec3::ZERO, Vec3::ONE)),
            "past the cap"
        );
        assert_eq!(rig.count, MAX_LIGHTS as u8);
    }

    /// **GATE — drivers are deterministic in their seed, and inert at depth 0.**
    /// A flicker must reproduce from `(seed, t)` alone (no wall clock, no state), two
    /// seeds must differ, a flicker must only ever DIM, and `depth == 0` must return
    /// the literal `1.0` — the proof that an undriven rig reaches the GPU unchanged in
    /// every bit.
    #[test]
    fn drivers_are_deterministic_for_a_seed() {
        let d = |kind, depth, seed| Driver {
            kind,
            speed: 7.0,
            depth,
            seed,
        };
        let a = d(DriverKind::Flicker, 0.35, 1);
        let b = d(DriverKind::Flicker, 0.35, 2);
        let mut differ = false;
        for step in 0..64 {
            let t = step as f32 * 0.031;
            assert_eq!(
                a.gain(t).to_bits(),
                a.gain(t).to_bits(),
                "same (seed, t) ⇒ same bits"
            );
            let g = a.gain(t);
            assert!(
                (1.0 - 0.35..=1.0).contains(&g),
                "a flicker only dims: {g} at t={t}"
            );
            differ |= a.gain(t).to_bits() != b.gain(t).to_bits();
            // Depth 0 is the literal 1.0, for both kinds — no epsilon.
            assert_eq!(
                d(DriverKind::Flicker, 0.0, 1).gain(t).to_bits(),
                1.0f32.to_bits()
            );
            assert_eq!(
                d(DriverKind::Pulse, 0.0, 1).gain(t).to_bits(),
                1.0f32.to_bits()
            );
        }
        assert!(differ, "two seeds must not run in lockstep");

        // …and the rig-level proof: an undriven rig, and a depth-0 driven one, come
        // back from `driven` with their intensities bit-identical.
        let rig = LightRig::default();
        for t in [0.0, 1.0, 12.75] {
            for (before, after) in rig.lights.iter().zip(rig.driven(t).lights.iter()) {
                assert_eq!(before.intensity.to_bits(), after.intensity.to_bits());
            }
        }
        let mut driven = LightRig::default();
        driven.lights[2].driver = Some(d(DriverKind::Pulse, 0.0, 9));
        assert_eq!(
            driven.driven(3.5).lights[2].intensity.to_bits(),
            driven.lights[2].intensity.to_bits(),
            "depth 0 leaves the intensity untouched"
        );
        // A real driver actually moves it.
        driven.lights[2].driver = Some(d(DriverKind::Flicker, 0.5, 9));
        let moved = (0..40).any(|i| {
            driven.driven(i as f32 * 0.05).lights[2].intensity.to_bits()
                != driven.lights[2].intensity.to_bits()
        });
        assert!(moved, "a depth-0.5 flicker must actually modulate");
    }
}
