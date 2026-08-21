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
//! ascending-layer painter's order — ui-panel → triangle → sprite → text per
//! layer — across all four 2D pipelines, so a higher-layer overlay's panels,
//! sprites *and* text cover a lower layer's text. The depth buffer is never
//! used for 2D (mirrors DirectXTK's `DepthNone` `SpriteBatch` default).

mod frame_graph;
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
mod pipeline_ui;
mod pipeline_volumetric;
mod quad_grid;
mod renderer;
mod stage;
mod texture;

#[cfg(test)]
mod layering_test;

pub use frame_graph::{CompositeTarget, FrameGraph, Label, PanelFrame, Rect};
pub use mesh::{
    ray_triangle, Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, SceneLighting,
};
pub use pipeline_ground_fog::GroundFog;
pub use pipeline_mesh::MATERIAL_PALETTE_LEN;
pub use pipeline_mesh_textured::{
    build_textured_verts, PbrMaps, TexturedMeshHandle, TexturedVertex,
};
pub use pipeline_skinned::{SkinnedMeshHandle, SkinnedMeshPipeline, SkinnedVertex};
pub use pipeline_text::FontRole;
pub use pipeline_volumetric::{VolumetricDisk, MAX_VOLUMETRIC_BODIES};
pub use quad_grid::{Orbit, QuadGrid, QuadStyle, QuadView, EDITOR_QUADS, ORBIT_FOV_Y};
pub use renderer::{RenderTargetHandle, Renderer, FULL_TEXTURE};
pub use stage::{grid_segments, grid_segments_xy, ring_segments};
pub use texture::TextureHandle;

// Re-export the math types we expose in our public API so callers don't have
// to pin glam themselves.
pub use glam::{Mat4, Vec2, Vec3};
