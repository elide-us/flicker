//! Public 3D-mesh types: `MeshVertex`, `MeshHandle`, `MeshIndices`,
//! `MeshDrawOptions`, and `Camera`.
//!
//! The mesh pipeline lives in `pipeline_mesh.rs`; this module just defines
//! the data types that cross the renderer's public boundary.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

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
}

impl Default for MeshDrawOptions {
    fn default() -> Self {
        Self {
            wireframe: false,
            tint: [1.0, 1.0, 1.0, 1.0],
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
        }
    }
}
