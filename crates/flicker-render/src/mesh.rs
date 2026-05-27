//! Public 3D-mesh types: `MeshVertex`, `MeshHandle`, `MeshIndices`,
//! `MeshDrawOptions`, and `Camera`.
//!
//! The mesh pipeline lives in `pipeline_mesh.rs`; this module just defines
//! the data types that cross the renderer's public boundary.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

/// One vertex of a 3D mesh.
///
/// The byte layout (`28` bytes — `position[3] + normal[3] + material[1]`)
/// mirrors `flicker_voxel::Vertex` exactly so a downstream crate can
/// cast a voxel-contour vertex slice into a `MeshVertex` slice without
/// copying. Declaring it here (rather than depending on `flicker-voxel`)
/// keeps the render crate independent of voxel internals.
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
