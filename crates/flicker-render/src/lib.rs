//! flicker-render: wgpu device, surface, 2D sprite/triangle/text batchers,
//! and the 3D mesh pipeline.
//!
//! Exposes a single [`Renderer`] type. Per-frame draw queues for solid
//! triangles, textured quads, text, and 3D meshes reset on
//! [`Renderer::begin_frame`]; **uploaded mesh storage** persists across
//! frames (only the per-frame mesh draw queue clears).
//!
//! 3D meshes render first in the main pass with a `Depth32Float`
//! attachment; 2D primitives render after with depth disabled, so they
//! always layer on top.

mod mesh;
mod pipeline_lines;
mod pipeline_mesh;
mod pipeline_sprite;
mod pipeline_text;
mod pipeline_triangle;
mod renderer;
mod texture;

pub use mesh::{Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex};
pub use renderer::Renderer;
pub use texture::TextureHandle;

// Re-export the math types we expose in our public API so callers don't have
// to pin glam themselves.
pub use glam::{Mat4, Vec2, Vec3};
