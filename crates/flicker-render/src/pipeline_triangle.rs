//! Solid-color triangle pipeline.
//!
//! Triangles are submitted via [`Renderer::draw_triangle`](crate::Renderer::draw_triangle)
//! with a `layer` (the painter's-order sort key shared by every 2D pipeline).
//! Coordinates are converted from pixel space (origin top-left, y down) to NDC
//! on the CPU as vertices are pushed. In `prepare` the triangles are stably
//! sorted by layer and grouped into one [`Band`] per distinct layer; the
//! renderer draws bands interleaved with the sprite/text bands of the same
//! layer (see `Renderer::end_frame`), so 2D ordering is pure CPU painter's
//! order — the depth buffer is never used (mirrors DirectXTK's `DepthNone`
//! default for `SpriteBatch`).

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use crate::pipeline_mesh::DEPTH_FORMAT;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2], // NDC
    color: [f32; 4],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

/// One triangle awaiting draw: its three vertices plus the layer they sort on.
struct Tri {
    layer: f32,
    verts: [Vertex; 3],
}

/// A contiguous block of one layer's vertices within the uploaded buffer.
struct Band {
    layer: f32,
    vertex_start: u32,
    vertex_count: u32,
}

/// Per-frame triangle pipeline state.
pub struct TrianglePipeline {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    /// Submitted triangles, in submission order, tagged with their layer.
    tris: Vec<Tri>,
    /// Scratch built in `prepare`: `tris` flattened into vertices, ordered by
    /// layer. Retained across frames to avoid reallocating.
    upload: Vec<Vertex>,
    /// One band per distinct layer, ascending — indexes into `upload`.
    bands: Vec<Band>,
}

impl TrianglePipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.triangle.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/triangle.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.triangle.pipeline_layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.triangle.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // 2D overlay — share the depth attachment with the 3D pipeline
            // but neither write nor test depth, so triangles always layer
            // on top of the 3D scene regardless of camera depth.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let initial_capacity = (std::mem::size_of::<Vertex>() * 3 * 64) as u64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.triangle.vbo"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            vertex_buffer,
            vertex_buffer_capacity: initial_capacity,
            tris: Vec::new(),
            upload: Vec::new(),
            bands: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.tris.clear();
        self.upload.clear();
        self.bands.clear();
    }

    /// Push one triangle at `layer`. `a`, `b`, `c` are in pixel coordinates
    /// (top-left origin).
    pub fn push(&mut self, screen: Vec2, a: Vec2, b: Vec2, c: Vec2, color: [f32; 4], layer: f32) {
        let to_ndc =
            |p: Vec2| -> [f32; 2] { [(p.x / screen.x) * 2.0 - 1.0, 1.0 - (p.y / screen.y) * 2.0] };
        self.tris.push(Tri {
            layer,
            verts: [
                Vertex {
                    position: to_ndc(a),
                    color,
                },
                Vertex {
                    position: to_ndc(b),
                    color,
                },
                Vertex {
                    position: to_ndc(c),
                    color,
                },
            ],
        });
    }

    /// The distinct layers present this frame, ascending. Used by the renderer
    /// to drive interleaved per-layer drawing across the 2D pipelines.
    pub fn layers(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands.iter().map(|b| b.layer)
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.upload.clear();
        self.bands.clear();
        if self.tris.is_empty() {
            return;
        }

        // Stable-sort indices by layer so triangles group into ascending bands
        // while preserving submission order within a layer.
        let mut order: Vec<usize> = (0..self.tris.len()).collect();
        order.sort_by(|&a, &b| self.tris[a].layer.total_cmp(&self.tris[b].layer));

        for &i in &order {
            self.upload.extend_from_slice(&self.tris[i].verts);
        }

        let mut idx = 0;
        let mut start = 0u32;
        while idx < order.len() {
            let layer = self.tris[order[idx]].layer;
            let mut count = 0u32;
            while idx < order.len() && self.tris[order[idx]].layer == layer {
                count += 3;
                idx += 1;
            }
            self.bands.push(Band {
                layer,
                vertex_start: start,
                vertex_count: count,
            });
            start += count;
        }

        let needed = (self.upload.len() * std::mem::size_of::<Vertex>()) as u64;
        if needed > self.vertex_buffer_capacity {
            let new_capacity = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.triangle.vbo"),
                size: new_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_buffer_capacity = new_capacity;
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.upload));
    }

    /// Draw only the triangles submitted at `layer` (no-op if none).
    pub fn render_layer<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, layer: f32) {
        let Some(band) = self.bands.iter().find(|b| b.layer == layer) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        let bytes = (self.upload.len() * std::mem::size_of::<Vertex>()) as u64;
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..bytes));
        pass.draw(
            band.vertex_start..band.vertex_start + band.vertex_count,
            0..1,
        );
    }
}
