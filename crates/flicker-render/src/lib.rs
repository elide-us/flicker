//! flicker-render: wgpu device, surface, sprite batcher, and texture atlas.
//!
//! Exposes a single [`Renderer`] type with retained per-frame draw queues for
//! solid-color triangles, textured quads ("sprites"), and text.

mod pipeline_sprite;
mod pipeline_text;
mod pipeline_triangle;
mod renderer;
mod texture;

pub use renderer::Renderer;
pub use texture::TextureHandle;

// Re-export the math types we expose in our public API so callers don't have
// to pin glam themselves.
pub use glam::Vec2;
