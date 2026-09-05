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
mod pipeline_bloom;
mod pipeline_ground_fog;
mod pipeline_lines;
mod pipeline_mesh;
mod pipeline_mesh_textured;
mod pipeline_shadow;
mod pipeline_skinned;
mod pipeline_sky;
mod pipeline_sprite;
mod pipeline_text;
mod pipeline_tonemap;
mod pipeline_triangle;
mod pipeline_ui;
mod pipeline_volumetric;
mod pipeline_water_mesh;
mod quad_grid;
mod renderer;
mod stage;
mod texture;

/// The HDR intermediate colour format the lit-3D pipelines BAKE their `Hdr` variant for:
/// 16-bit float per channel, so scene radiance is kept above 1.0 for the `tonemap_grade`
/// pass to roll off. A surface with no `hdr` attachment never allocates one and every
/// pipeline renders straight into the sRGB `color` as before — the format only varies at
/// the [`RenderPipeline`](wgpu::RenderPipeline) level (see [`TargetColor`]).
///
/// Defined THROUGH [`AttachmentFormat::texture_format`], which is the one authority on
/// what an authored format is: the scene files declare `"format": "rgba16f"` and the
/// allocation takes its format from that declaration, so this constant and the authored
/// word can never drift apart. (The `surface` argument is what `AttachmentFormat::Surface`
/// would resolve to and is irrelevant to `Rgba16f`; a stage declaring an `hdr` attachment
/// in any other format is a compile problem, so nothing else ever reaches here.)
pub const HDR_FORMAT: wgpu::TextureFormat =
    AttachmentFormat::Rgba16f.texture_format(wgpu::TextureFormat::Rgba8UnormSrgb);

/// Which colour attachment a 3D pipeline renders into for the pass being encoded. Each
/// lit-3D pipeline bakes BOTH format variants at construction (one for the swapchain
/// `surface_format`, one for [`HDR_FORMAT`]) over a single shared set of buffers /
/// bind groups / draw queues, and `render(…, target)` selects the variant — the ONLY
/// thing that differs between them is the colour-target format. `Srgb` is the byte-
/// identical pre-HDR path; `Hdr` targets the float attachment a `tonemap_grade` pass
/// then resolves back to `Srgb`. The discriminants index the `[_; 2]` variant arrays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetColor {
    Srgb = 0,
    Hdr = 1,
}

pub use frame_graph::{CompositeTarget, FrameGraph, Label, PanelFrame, Rect};
pub use mesh::{
    ray_triangle, Camera, Driver, DriverKind, Light, LightKind, LightRig, MeshDrawOptions,
    MeshHandle, MeshIndices, MeshVertex, MAX_LIGHTS,
};
pub use pipeline_ground_fog::GroundFog;
pub use pipeline_mesh::MATERIAL_PALETTE_LEN;
pub use pipeline_mesh_textured::{
    build_textured_verts, PbrMaps, TexturedMeshHandle, TexturedVertex,
};
pub use pipeline_skinned::{SkinnedMeshHandle, SkinnedMeshPipeline, SkinnedVertex};
pub use pipeline_text::FontRole;
pub use pipeline_volumetric::{VolumetricDisk, MAX_VOLUMETRIC_BODIES};
pub use pipeline_water_mesh::{Water, WaveKind, WaveSource, MAX_WAVE_SOURCES};
pub use quad_grid::{
    Orbit, QuadGrid, QuadStyle, QuadView, ViewportFiller, ViewportLayout, EDITOR_QUADS, ORBIT_FOV_Y,
};
pub use renderer::{line_quad, RenderTargetHandle, Renderer, FULL_TEXTURE};
pub use stage::{
    depth_plan, grid_segments, grid_segments_xy, ring_segments, Attachment, AttachmentFormat,
    Attachments, BloomPass, CompositePass, DepthPass, FogSlot, GroundFogPass, PassDef, PassKind,
    Rate, ShadowMapPass, StageCamera, StageDef, StageInputs, StageLayer, TonemapGradePass,
    TonemapSlot, VolumetricPass, VolumetricSlot, WaterPass, WaterSlot,
};
pub use texture::TextureHandle;

// Re-export the math types we expose in our public API so callers don't have
// to pin glam themselves.
pub use glam::{Mat4, Vec2, Vec3};
