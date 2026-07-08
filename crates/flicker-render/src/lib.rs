//! flicker-render: wgpu device, surface, 2D sprite/triangle/text batchers,
//! and the 3D mesh pipeline.
//!
//! Exposes a single [`Renderer`] type. Per-frame draw queues for solid
//! triangles, textured quads, text, and 3D meshes reset on
//! [`Renderer::begin_frame`]; **uploaded mesh storage** persists across
//! frames (only the per-frame mesh draw queue clears).
//!
//! 3D meshes render first in the main pass with a `Depth32Float`
//! attachment; 2D primitives render after with depth disabled. Within 2D,
//! draws carry an ambient `layer` ([`Renderer::set_layer`]) and are drawn in
//! ascending-layer painter's order — triangle → sprite → text per layer —
//! across all three 2D pipelines, so a higher-layer overlay's sprites *and*
//! text cover a lower layer's text. The depth buffer is never used for 2D
//! (mirrors DirectXTK's `DepthNone` `SpriteBatch` default).

mod mesh;
mod pipeline_billboard;
mod pipeline_ground_fog;
mod pipeline_lines;
mod pipeline_mesh;
mod pipeline_mesh_textured;
mod pipeline_skinned;
mod pipeline_sky;
mod pipeline_sprite;
mod pipeline_text;
mod pipeline_triangle;
mod pipeline_volumetric;
mod renderer;
mod texture;

#[cfg(test)]
mod layering_test;

pub use mesh::{Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, SceneLighting};
pub use pipeline_ground_fog::GroundFog;
pub use pipeline_mesh_textured::{PbrMaps, TexturedMeshHandle, TexturedVertex};
pub use pipeline_skinned::{SkinnedMeshHandle, SkinnedMeshPipeline, SkinnedVertex};
pub use pipeline_volumetric::{VolumetricDisk, MAX_VOLUMETRIC_BODIES};
pub use renderer::{RenderTargetHandle, Renderer};
pub use texture::TextureHandle;

// Re-export the math types we expose in our public API so callers don't have
// to pin glam themselves.
pub use glam::{Mat4, Vec2, Vec3};
