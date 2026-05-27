//! Textured-quad ("sprite") pipeline.
//!
//! Each [`Renderer::draw_sprite`](crate::Renderer::draw_sprite) call appends six
//! vertices for one quad. Quads are grouped by texture so that all quads
//! sharing a texture render in a single draw call. Pixel coordinates are
//! converted to NDC at vertex-push time.

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use crate::pipeline_mesh::DEPTH_FORMAT;
use crate::texture::TextureHandle;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

impl Vertex {
    const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

pub struct SpritePipeline {
    pipeline: wgpu::RenderPipeline,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    /// Flat vertex list, in submission order. Grouped into draw ranges via `runs`.
    vertices: Vec<Vertex>,
    /// One run per contiguous block of quads sharing the same texture.
    runs: Vec<Run>,
}

struct Run {
    texture: TextureHandle,
    vertex_offset: u32,
    vertex_count: u32,
}

impl SpritePipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("flicker.sprite.bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flicker.sprite.sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.sprite.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sprite.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.sprite.pipeline_layout"),
            bind_group_layouts: &[&texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.sprite.pipeline"),
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
            // but neither write nor test depth so sprites always layer on
            // top of the 3D scene.
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

        let initial_capacity = (std::mem::size_of::<Vertex>() * 6 * 64) as u64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.sprite.vbo"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            texture_bind_group_layout,
            sampler,
            vertex_buffer,
            vertex_buffer_capacity: initial_capacity,
            vertices: Vec::new(),
            runs: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.runs.clear();
    }

    /// Push one quad at `position` (top-left in pixels) with the given pixel `size`,
    /// multiplied by `color` (RGBA in 0..1) in the fragment shader.
    pub fn push(
        &mut self,
        screen: Vec2,
        texture: TextureHandle,
        position: Vec2,
        size: Vec2,
        color: [f32; 4],
    ) {
        let to_ndc =
            |p: Vec2| -> [f32; 2] { [(p.x / screen.x) * 2.0 - 1.0, 1.0 - (p.y / screen.y) * 2.0] };

        let tl = position;
        let tr = position + Vec2::new(size.x, 0.0);
        let bl = position + Vec2::new(0.0, size.y);
        let br = position + size;

        let vertex_offset = self.vertices.len() as u32;
        let verts = [
            Vertex {
                position: to_ndc(tl),
                uv: [0.0, 0.0],
                color,
            },
            Vertex {
                position: to_ndc(bl),
                uv: [0.0, 1.0],
                color,
            },
            Vertex {
                position: to_ndc(br),
                uv: [1.0, 1.0],
                color,
            },
            Vertex {
                position: to_ndc(tl),
                uv: [0.0, 0.0],
                color,
            },
            Vertex {
                position: to_ndc(br),
                uv: [1.0, 1.0],
                color,
            },
            Vertex {
                position: to_ndc(tr),
                uv: [1.0, 0.0],
                color,
            },
        ];
        self.vertices.extend_from_slice(&verts);

        // Coalesce with the previous run if same texture, else start a new run.
        match self.runs.last_mut() {
            Some(run) if run.texture == texture => {
                run.vertex_count += 6;
            }
            _ => {
                self.runs.push(Run {
                    texture,
                    vertex_offset,
                    vertex_count: 6,
                });
            }
        }
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.vertices.is_empty() {
            return;
        }
        let needed = (self.vertices.len() * std::mem::size_of::<Vertex>()) as u64;
        if needed > self.vertex_buffer_capacity {
            let new_capacity = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.sprite.vbo"),
                size: new_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_buffer_capacity = new_capacity;
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
    }

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        textures: &'a [crate::texture::LoadedTexture],
    ) {
        if self.runs.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        let bytes = (self.vertices.len() * std::mem::size_of::<Vertex>()) as u64;
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..bytes));
        for run in &self.runs {
            let Some(tex) = textures.get(run.texture.0 as usize) else {
                continue;
            };
            pass.set_bind_group(0, &tex.bind_group, &[]);
            let start = run.vertex_offset;
            let end = start + run.vertex_count;
            pass.draw(start..end, 0..1);
        }
    }
}
