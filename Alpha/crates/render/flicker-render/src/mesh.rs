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
    /// sheen** from the point light (a star) for liquid / icy / wet-looking surfaces — a
    /// Fresnel grazing-edge brightening, *not* a mirror hot-spot (which reads as a marble at planet
    /// scale). The sheen strengthens with gloss.
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
}

impl Camera {
    /// World-to-camera view matrix.
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    /// Camera-to-clip projection matrix. `aspect` is `width / height`.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y_radians, aspect, self.near, self.far)
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
        }
    }
}

/// Frame-global lighting & atmosphere — the day/night cycle state the mesh
/// shader uses. Set once per frame with [`crate::Renderer::set_scene`];
/// the renderer fills in the camera position itself (for fog distance), so
/// callers only supply the lights, ambient, fog, and grade.
///
/// Two directional lights (`sun` + `moon`) each contribute a matte
/// Lambertian term; `*_dir` points **toward** the light and should be
/// normalized. A light below the horizon is faded by handing it a near-black
/// colour (no explicit night branch in the shader). `fog_*` and `grade_*`
/// are reserved for the fog / colour-grade slices and default to inert.
#[derive(Copy, Clone, Debug)]
pub struct SceneLighting {
    /// Direction toward the sun (normalized).
    pub sun_dir: Vec3,
    /// Sun radiance (linear RGB). Black ⇒ the sun is effectively off.
    pub sun_color: Vec3,
    /// Direction toward the moon (normalized).
    pub moon_dir: Vec3,
    /// Moon radiance (linear RGB). Black ⇒ the moon is effectively off.
    pub moon_color: Vec3,
    /// Flat ambient floor added before the directional terms.
    pub ambient: Vec3,
    /// Procedural-sky colour straight up (linear RGB). Used by the sky pass
    /// ([`crate::Renderer::draw_sky`]); ignored when no sky is requested.
    pub sky_zenith: Vec3,
    /// Procedural-sky colour at the horizon band (linear RGB). The sky pass
    /// gradients `sky_horizon`→`sky_zenith` by view elevation.
    pub sky_horizon: Vec3,
    /// Distance-fog colour (linear RGB). Reserved — applied in a later slice.
    pub fog_color: Vec3,
    /// Distance-fog density. `0.0` ⇒ no fog. Reserved for a later slice.
    pub fog_density: f32,
    /// Colour-grade tint (linear RGB). Reserved for a later slice.
    pub grade: Vec3,
    /// Colour-grade strength in `0..1`. `0.0` ⇒ no grade. Later slice.
    pub grade_strength: f32,
    /// World position of a **point light** (e.g. a central star). Each fragment is lit from
    /// `normalize(point_pos − world_position)`, so bodies at different positions are correctly
    /// lit from their own direction — unlike the parallel `sun`/`moon`. No distance falloff.
    pub point_pos: Vec3,
    /// Point-light radiance (linear RGB). Black ⇒ the point light is off (the default), so
    /// scenes that don't set it are unchanged.
    pub point_color: Vec3,
    /// World→celestial rotation for the procedural night sky (stars + Milky
    /// Way). A view ray transformed by this lands in a sky-fixed frame, so the
    /// stars rotate with time of day and tilt with latitude. Identity leaves
    /// the star field locked to the camera. Only the sky pass uses it.
    pub star_rotation: Mat4,
}

impl Default for SceneLighting {
    /// Matches the renderer's seeded default — the pre-uniform hardcoded
    /// look: a warm-white sun over a `0.3` ambient, no moon, no fog/grade.
    fn default() -> Self {
        Self {
            sun_dir: Vec3::new(0.5, 1.0, 0.3).normalize(),
            sun_color: Vec3::splat(0.7),
            moon_dir: Vec3::Y,
            moon_color: Vec3::ZERO,
            ambient: Vec3::splat(0.3),
            sky_zenith: Vec3::new(0.012, 0.016, 0.030),
            sky_horizon: Vec3::new(0.030, 0.040, 0.085),
            fog_color: Vec3::ZERO,
            fog_density: 0.0,
            grade: Vec3::ZERO,
            grade_strength: 0.0,
            point_pos: Vec3::ZERO,
            point_color: Vec3::ZERO, // off by default — scenes opt in
            star_rotation: Mat4::IDENTITY,
        }
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
        let (o, d) = c.pick_ray(Vec2::new(vp.x * 0.5 - 0.5, vp.y * 0.5 - 0.5), vp).unwrap();
        let want = (c.target - c.position).normalize();
        assert!(d.dot(want) > 0.9999, "centre ray must aim at the target: {d:?} vs {want:?}");
        assert!((o - c.position).length() < c.far, "ray starts near the camera, not behind it");
    }

    /// A pick ray must actually hit geometry under the cursor, and the ray/triangle pair
    /// must agree — this is what a scene pick relies on end to end.
    #[test]
    fn centre_ray_hits_a_triangle_at_the_target() {
        let c = cam();
        let vp = Vec2::new(800.0, 600.0);
        let (o, d) = c.pick_ray(Vec2::new(vp.x * 0.5 - 0.5, vp.y * 0.5 - 0.5), vp).unwrap();
        // A big quad-ish triangle spanning the origin, facing the camera (+Z).
        let (a, b, cc) = (
            Vec3::new(-50.0, -50.0, 0.0),
            Vec3::new(50.0, -50.0, 0.0),
            Vec3::new(0.0, 50.0, 0.0),
        );
        let t = ray_triangle(o, d, a, b, cc).expect("centre ray must hit a triangle at the origin");
        let hit = o + d * t;
        assert!(hit.length() < 1.0, "hit should land at the target, got {hit:?}");
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
        assert!(ray_triangle(o, d, a, b, c).is_none(), "back face must not be picked");
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
        assert!(ray_triangle(o, d, a, b, c).is_none(), "geometry behind the ray must miss");
    }
}
