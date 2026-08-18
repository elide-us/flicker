//! 3D mesh pipeline.
//!
//! # Transform stack
//!
//! Each `draw_mesh` call composes `projection × view × model` to produce
//! the clip-space transform for that mesh. The `view × projection` is
//! shared across the frame via [`MeshPipeline::set_camera_matrix`]
//! (called once per frame from the renderer) and stored in a single
//! uniform buffer. The `model` matrix plus tint and a wireframe flag
//! are written into a per-draw uniform buffer; the bind group references
//! both, with the per-draw entry bound through a dynamic offset that
//! advances by `per_draw_stride` for each queued draw.
//!
//! # Depth
//!
//! The mesh pipeline writes and tests depth using a `Depth32Float`
//! attachment owned by the renderer. The 2D pipelines (triangle,
//! sprite, text) share the same render pass but disable depth write
//! and test so they layer on top of the 3D scene as overlays. Depth
//! is cleared to `1.0` at the start of every frame.
//!
//! # Wireframe via line-list edge buffer
//!
//! Each uploaded mesh carries two index buffers: a **triangle index
//! buffer** (the caller's `MeshIndices`) and an **edge index buffer**
//! built during `upload` by walking the triangle indices and deduping
//! each undirected edge `(min, max)` through a `HashSet`. Each unique
//! edge contributes 2 indices to the edge buffer.
//!
//! Two render pipelines share the vertex shader, bind groups, depth
//! state, and color target — they differ only in primitive topology:
//!
//! * `triangle_pipeline`: `TriangleList`, back-face culling enabled.
//! * `line_pipeline`: `LineList`, culling disabled (the notion of
//!   "back-facing" doesn't apply to lines).
//!
//! `MeshPipeline::render` dispatches per draw call based on the
//! wireframe flag: wireframe draws bind the line pipeline and the
//! edge index buffer; filled draws bind the triangle pipeline and the
//! triangle index buffer. The fragment shader's wireframe branch then
//! emits the wireframe color directly — no barycentric trick, no
//! shared-vertex caveat.

use std::collections::HashSet;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::mesh::{MeshHandle, MeshIndices, MeshVertex};

/// The depth attachment format used by the renderer. Matched between the
/// 3D pipeline and the 2D pipelines (which set `depth_write_enabled =
/// false` and `depth_compare = Always`).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// CPU-side mirror of the WGSL `Camera` uniform.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
}

const CAMERA_UNIFORM_SIZE: u64 = std::mem::size_of::<CameraUniform>() as u64;

/// CPU-side mirror of the WGSL `PerDraw` uniform.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PerDraw {
    model: [[f32; 4]; 4], // 64 bytes
    tint: [f32; 4],       // 16 bytes
    flags: [f32; 4],      // 16 bytes; flags.x = wireframe (0/1), rest reserved.
}

const PER_DRAW_RAW_SIZE: u64 = std::mem::size_of::<PerDraw>() as u64;

/// CPU-side mirror of the WGSL `Scene` uniform — the frame-global
/// lighting/atmosphere state (sun + moon directional lights, ambient,
/// and the fog/grade fields used by later slices). Laid out as `[f32; 4]`
/// lanes to match the shader's `vec4` fields exactly (16-byte aligned, no
/// implicit std140 padding). The `.w` lanes pack scalars: `fog_color[3]`
/// is fog density, `grade[3]` is grade strength.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SceneUniform {
    pub sun_dir: [f32; 4],
    pub sun_color: [f32; 4],
    pub moon_dir: [f32; 4],
    pub moon_color: [f32; 4],
    pub ambient: [f32; 4],
    pub camera_pos: [f32; 4],
    pub fog_color: [f32; 4],
    pub grade: [f32; 4],
    pub point_pos: [f32; 4],   // xyz = point-light world position; w unused
    pub point_color: [f32; 4], // rgb = point-light radiance; w unused
}

const SCENE_UNIFORM_SIZE: u64 = std::mem::size_of::<SceneUniform>() as u64;

impl Default for SceneUniform {
    /// Reproduces the pre-uniform hardcoded look: a single warm-white sun
    /// at `normalize(0.5, 1.0, 0.3)` scaled to `0.7`, over a `0.3` ambient,
    /// no moon, no fog, no grade — i.e. `base * (0.3 + 0.7 * lambert)`.
    fn default() -> Self {
        // normalize(0.5, 1.0, 0.3)
        let sun = [0.4319, 0.8638, 0.2591, 0.0];
        Self {
            sun_dir: sun,
            sun_color: [0.7, 0.7, 0.7, 0.0],
            moon_dir: [0.0, 1.0, 0.0, 0.0],
            moon_color: [0.0, 0.0, 0.0, 0.0],
            ambient: [0.3, 0.3, 0.3, 0.0],
            camera_pos: [0.0, 0.0, 0.0, 0.0],
            fog_color: [0.0, 0.0, 0.0, 0.0],
            grade: [0.0, 0.0, 0.0, 0.0],
            point_pos: [0.0, 0.0, 0.0, 0.0],
            point_color: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

/// One persistent GPU-side mesh. Holds both a triangle index buffer
/// (for filled draws) and an edge index buffer (for line-list wireframe
/// draws). Both reference the same vertex buffer.
pub struct LoadedMesh {
    vertex_buffer: wgpu::Buffer,
    triangle_index_buffer: wgpu::Buffer,
    triangle_index_count: u32,
    triangle_index_format: wgpu::IndexFormat,
    edge_index_buffer: wgpu::Buffer,
    edge_index_count: u32,
    edge_index_format: wgpu::IndexFormat,
}

/// One queued draw call for the current frame.
struct MeshDraw {
    handle: MeshHandle,
    per_draw: PerDraw,
}

const MESH_VERTEX_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];

/// The 3D mesh pipeline.
pub struct MeshPipeline {
    triangle_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    camera_buf: wgpu::Buffer,
    /// Frame-global lighting/atmosphere uniform (sun/moon/ambient/fog/grade),
    /// written each frame by [`MeshPipeline::set_scene_uniform`]. Lives in the
    /// same bind group as the camera and per-draw uniforms.
    scene_buf: wgpu::Buffer,
    per_draw_buf: wgpu::Buffer,
    /// Maximum number of per-draw entries the current `per_draw_buf` can
    /// hold. Grown lazily in `prepare`.
    per_draw_capacity: u32,
    /// Per-draw stride in bytes, equal to `min_uniform_buffer_offset_alignment`
    /// rounded up to be at least `PER_DRAW_RAW_SIZE`.
    per_draw_stride: u32,
    queued: Vec<MeshDraw>,
}

impl MeshPipeline {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        min_uniform_offset_alignment: u32,
    ) -> Self {
        let per_draw_stride = round_up_to_alignment(
            PER_DRAW_RAW_SIZE as u32,
            min_uniform_offset_alignment.max(1),
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.mesh.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.mesh.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(CAMERA_UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(PER_DRAW_RAW_SIZE),
                    },
                    count: None,
                },
                // Frame-global lighting/atmosphere — fragment-only, one value
                // per frame (no dynamic offset).
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(SCENE_UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.mesh.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &MESH_VERTEX_ATTRS,
        };

        // Depth-stencil shared between both pipelines. `LessEqual` lets
        // a wireframe pass land on top of a fill pass at the exact same
        // depth — without this, the line draw would lose the depth test
        // against the fill's just-written depth values and render nothing.
        let depth_stencil = Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let color_target = Some(wgpu::ColorTargetState {
            format: surface_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        });

        let triangle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.mesh.triangle_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: std::slice::from_ref(&vertex_layout),
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: std::slice::from_ref(&color_target),
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: depth_stencil.clone(),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.mesh.line_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: std::slice::from_ref(&vertex_layout),
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: std::slice::from_ref(&color_target),
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                // Lines have no notion of "back-facing" — disable culling.
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.mesh.camera_uniform"),
            size: CAMERA_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Seed the scene uniform with the default look so the first frames
        // (before any `set_scene`) match the former hardcoded lighting.
        let scene_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.mesh.scene_uniform"),
            contents: bytemuck::bytes_of(&SceneUniform::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let initial_capacity: u32 = 64;
        let per_draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.mesh.per_draw_uniform"),
            size: (initial_capacity as u64) * (per_draw_stride as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = make_bind_group(
            device,
            &bind_group_layout,
            &camera_buf,
            &scene_buf,
            &per_draw_buf,
        );

        Self {
            triangle_pipeline,
            line_pipeline,
            bind_group_layout,
            bind_group,
            camera_buf,
            scene_buf,
            per_draw_buf,
            per_draw_capacity: initial_capacity,
            per_draw_stride,
            queued: Vec::new(),
        }
    }

    /// Upload an indexed mesh and return a handle. The mesh persists
    /// across frames (caller must keep the handle to draw it again).
    /// Also builds an edge index buffer for line-list wireframe draws.
    pub fn upload(
        &self,
        device: &wgpu::Device,
        vertices: &[MeshVertex],
        indices: MeshIndices<'_>,
    ) -> LoadedMesh {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.mesh.vbo"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Triangle index buffer — exactly what the caller submitted.
        let (triangle_index_buffer, triangle_index_count, triangle_index_format) = match indices {
            MeshIndices::U16(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.tri_ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint16,
            ),
            MeshIndices::U32(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.tri_ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint32,
            ),
        };

        // Edge index buffer — undirected edges of the triangle mesh,
        // each represented as one `(min, max)` entry in a `HashSet`
        // so shared edges (the typical case for a 2-manifold) are
        // emitted exactly once.
        let edges = extract_edges(&indices);
        let edge_index_count = (edges.len() * 2) as u32;
        // Match the format-selection rule to the triangle buffer for
        // consistency: u16 unless the vertex count overflows it.
        let use_u32 = vertices.len() > (u16::MAX as usize + 1);
        let (edge_index_buffer, edge_index_format) = if use_u32 {
            let mut flat = Vec::with_capacity(edges.len() * 2);
            for (a, b) in &edges {
                flat.push(*a);
                flat.push(*b);
            }
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.edge_ibo"),
                    contents: bytemuck::cast_slice(&flat),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                wgpu::IndexFormat::Uint32,
            )
        } else {
            let mut flat = Vec::with_capacity(edges.len() * 2);
            for (a, b) in &edges {
                flat.push(*a as u16);
                flat.push(*b as u16);
            }
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.edge_ibo"),
                    contents: bytemuck::cast_slice(&flat),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                wgpu::IndexFormat::Uint16,
            )
        };

        LoadedMesh {
            vertex_buffer,
            triangle_index_buffer,
            triangle_index_count,
            triangle_index_format,
            edge_index_buffer,
            edge_index_count,
            edge_index_format,
        }
    }

    /// Reset the per-frame draw queue. Called from `Renderer::begin_frame`.
    pub fn clear(&mut self) {
        self.queued.clear();
    }

    /// Update the camera matrix used for the rest of this frame.
    pub fn set_camera_matrix(&mut self, queue: &wgpu::Queue, view_projection: Mat4) {
        let uniform = CameraUniform {
            view_projection: view_projection.to_cols_array_2d(),
        };
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Exposes the camera uniform buffer so sibling pipelines (e.g.
    /// the immediate-mode lines pipeline) can reuse the same
    /// view-projection without keeping their own copy.
    pub fn camera_buffer(&self) -> &wgpu::Buffer {
        &self.camera_buf
    }

    /// Update the frame-global lighting/atmosphere uniform for the rest of
    /// this frame. Mirrors [`MeshPipeline::set_camera_matrix`] — the renderer
    /// calls it once per frame from `end_frame`.
    pub fn set_scene_uniform(&mut self, queue: &wgpu::Queue, scene: SceneUniform) {
        queue.write_buffer(&self.scene_buf, 0, bytemuck::bytes_of(&scene));
    }

    /// Queue a mesh for rendering this frame.
    pub fn push(
        &mut self,
        handle: MeshHandle,
        model: Mat4,
        tint: [f32; 4],
        wireframe: bool,
        gloss: f32,
    ) {
        self.queued.push(MeshDraw {
            handle,
            per_draw: PerDraw {
                model: model.to_cols_array_2d(),
                tint,
                // flags.x = wireframe, flags.y = gloss (specular strength).
                flags: [if wireframe { 1.0 } else { 0.0 }, gloss, 0.0, 0.0],
            },
        });
    }

    /// Write the per-draw uniform buffer for the queued draws. Grows the
    /// buffer if needed.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.queued.is_empty() {
            return;
        }
        let needed = self.queued.len() as u32;
        if needed > self.per_draw_capacity {
            let new_capacity = needed.next_power_of_two().max(self.per_draw_capacity * 2);
            self.per_draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.mesh.per_draw_uniform"),
                size: (new_capacity as u64) * (self.per_draw_stride as u64),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.per_draw_capacity = new_capacity;
            self.bind_group = make_bind_group(
                device,
                &self.bind_group_layout,
                &self.camera_buf,
                &self.scene_buf,
                &self.per_draw_buf,
            );
        }

        // Pack each PerDraw entry at its aligned offset.
        let mut staging = vec![0u8; self.queued.len() * self.per_draw_stride as usize];
        for (i, draw) in self.queued.iter().enumerate() {
            let off = i * self.per_draw_stride as usize;
            let end = off + std::mem::size_of::<PerDraw>();
            staging[off..end].copy_from_slice(bytemuck::bytes_of(&draw.per_draw));
        }
        queue.write_buffer(&self.per_draw_buf, 0, &staging);
    }

    /// Issue the queued draws. Wireframe draws bind the line-list
    /// pipeline + the edge index buffer; filled draws bind the
    /// triangle-list pipeline + the triangle index buffer.
    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, meshes: &'a [Option<LoadedMesh>]) {
        if self.queued.is_empty() {
            return;
        }
        for (i, draw) in self.queued.iter().enumerate() {
            let Some(mesh) = meshes.get(draw.handle.0 as usize).and_then(|s| s.as_ref()) else {
                continue;
            };
            let offset = (i as u32) * self.per_draw_stride;
            let wireframe = draw.per_draw.flags[0] > 0.5;
            if wireframe {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[offset]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.edge_index_buffer.slice(..), mesh.edge_index_format);
                pass.draw_indexed(0..mesh.edge_index_count, 0, 0..1);
            } else {
                pass.set_pipeline(&self.triangle_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[offset]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    mesh.triangle_index_buffer.slice(..),
                    mesh.triangle_index_format,
                );
                pass.draw_indexed(0..mesh.triangle_index_count, 0, 0..1);
            }
        }
    }
}

/// Walk the triangle indices and return the deduplicated set of
/// undirected edges, each represented as `(min, max)` in u32. Degenerate
/// triangles (two equal indices on an edge) are silently skipped.
fn extract_edges(indices: &MeshIndices<'_>) -> HashSet<(u32, u32)> {
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut push = |a: u32, b: u32| {
        if a == b {
            return;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        edges.insert(key);
    };
    match indices {
        MeshIndices::U16(idx) => {
            let n = idx.len() - idx.len() % 3;
            let mut i = 0;
            while i < n {
                let a = idx[i] as u32;
                let b = idx[i + 1] as u32;
                let c = idx[i + 2] as u32;
                push(a, b);
                push(b, c);
                push(a, c);
                i += 3;
            }
        }
        MeshIndices::U32(idx) => {
            let n = idx.len() - idx.len() % 3;
            let mut i = 0;
            while i < n {
                let a = idx[i];
                let b = idx[i + 1];
                let c = idx[i + 2];
                push(a, b);
                push(b, c);
                push(a, c);
                i += 3;
            }
        }
    }
    edges
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_buf: &wgpu::Buffer,
    scene_buf: &wgpu::Buffer,
    per_draw_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("flicker.mesh.bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: per_draw_buf,
                    offset: 0,
                    size: NonZeroU64::new(PER_DRAW_RAW_SIZE),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: scene_buf.as_entire_binding(),
            },
        ],
    })
}

fn round_up_to_alignment(value: u32, alignment: u32) -> u32 {
    let rem = value % alignment;
    if rem == 0 {
        value
    } else {
        value + alignment - rem
    }
}

/// Helper used by `Renderer` to (re)create the depth texture on resize.
pub fn create_depth_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("flicker.mesh.depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        // RENDER_ATTACHMENT to write depth in the opaque pass; TEXTURE_BINDING so the volumetric
        // pass can *sample* that depth (read-only) to clamp its rays at solid bodies.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the mesh pipeline under a validation error scope. Creating the
    /// render pipelines compiles `mesh.wgsl` and checks it against the bind-
    /// group layout (camera + per-draw + the `Scene` uniform at binding 2), so
    /// a malformed shader or a struct/layout mismatch fails here rather than at
    /// app launch. Skips cleanly on a machine without a GPU adapter.
    #[test]
    fn mesh_pipeline_compiles_shader() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            eprintln!("mesh_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("flicker.mesh_test.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let align = device.limits().min_uniform_buffer_offset_alignment;
        let mut pipeline = MeshPipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb, align);
        // Exercise the scene-uniform upload path too (writes binding 2).
        pipeline.set_scene_uniform(&queue, SceneUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "mesh.wgsl failed validation: {err:?}");
    }
}
