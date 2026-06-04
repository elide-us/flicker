//! Camera-facing billboard pipeline.
//!
//! Reusable world-positioned quad primitive. Each
//! [`Renderer::draw_billboard`](crate::Renderer::draw_billboard) call
//! pushes 6 vertices (two triangles, no indexing) into a per-frame
//! vertex buffer. The vertex shader orients the quad from the
//! camera's world-space right/up basis (supplied via a dedicated
//! camera uniform) so the quad always faces the camera while keeping
//! a constant world-space size.
//!
//! Quads are grouped by texture so that all billboards sharing a
//! texture render in a single draw call.
//!
//! # Depth
//!
//! Depth-tested with `LessEqual` and depth-writing. Surrounding 3D
//! mesh in front of the billboard occludes it, and the billboard
//! occludes 3D mesh behind it. 2D HUD overlays (which disable depth)
//! still layer on top.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::pipeline_mesh::DEPTH_FORMAT;
use crate::texture::TextureHandle;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct BillboardVertex {
    corner_offset: [f32; 2],
    world_position: [f32; 3],
    world_size: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

const BILLBOARD_VERTEX_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x3,
    2 => Float32x2,
    3 => Float32x2,
    4 => Float32x4,
];

/// CPU mirror of the WGSL `Camera` uniform. Holds the view-projection
/// matrix plus the world-space camera right/up vectors used to orient
/// each quad. Padded to 16-byte boundaries for std140-friendly layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
    camera_right_ws: [f32; 4],
    camera_up_ws: [f32; 4],
}

const CAMERA_UNIFORM_SIZE: u64 = std::mem::size_of::<CameraUniform>() as u64;

struct Run {
    texture: TextureHandle,
    vertex_offset: u32,
    vertex_count: u32,
}

pub struct BillboardPipeline {
    pipeline: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    vertices: Vec<BillboardVertex>,
    runs: Vec<Run>,
}

impl BillboardPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.billboard.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/billboard.wgsl").into()),
        });

        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("flicker.billboard.camera.bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("flicker.billboard.texture.bgl"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.billboard.pipeline_layout"),
            bind_group_layouts: &[&camera_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flicker.billboard.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BillboardVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &BILLBOARD_VERTEX_ATTRS,
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.billboard.pipeline"),
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
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.billboard.camera_uniform"),
            size: CAMERA_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.billboard.camera.bind_group"),
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let initial_capacity = (std::mem::size_of::<BillboardVertex>() * 6 * 32) as u64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.billboard.vbo"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            camera_buf,
            camera_bind_group,
            texture_bind_group_layout,
            sampler,
            vertex_buffer,
            vertex_buffer_capacity: initial_capacity,
            vertices: Vec::new(),
            runs: Vec::new(),
        }
    }

    /// Reset the per-frame draw queue.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.runs.clear();
    }

    /// Update the camera uniform. The view matrix's first two rows are
    /// the camera's world-space right and up vectors; the quad is
    /// oriented from them so it always faces the camera.
    pub fn set_camera(&self, queue: &wgpu::Queue, view: Mat4, view_projection: Mat4) {
        let right = view.row(0).truncate();
        let up = view.row(1).truncate();
        let uniform = CameraUniform {
            view_projection: view_projection.to_cols_array_2d(),
            camera_right_ws: [right.x, right.y, right.z, 0.0],
            camera_up_ws: [up.x, up.y, up.z, 0.0],
        };
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Queue one billboard.
    pub fn push(
        &mut self,
        texture: TextureHandle,
        world_position: Vec3,
        world_size: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [f32; 4],
    ) {
        let p = [world_position.x, world_position.y, world_position.z];
        let s = [world_size.x, world_size.y];

        let tl_off = [-0.5, 0.5];
        let bl_off = [-0.5, -0.5];
        let br_off = [0.5, -0.5];
        let tr_off = [0.5, 0.5];

        let tl_uv = [uv_min.x, uv_min.y];
        let bl_uv = [uv_min.x, uv_max.y];
        let br_uv = [uv_max.x, uv_max.y];
        let tr_uv = [uv_max.x, uv_min.y];

        let mk = |corner_offset: [f32; 2], uv: [f32; 2]| BillboardVertex {
            corner_offset,
            world_position: p,
            world_size: s,
            uv,
            color,
        };

        let vertex_offset = self.vertices.len() as u32;
        self.vertices.push(mk(tl_off, tl_uv));
        self.vertices.push(mk(bl_off, bl_uv));
        self.vertices.push(mk(br_off, br_uv));
        self.vertices.push(mk(tl_off, tl_uv));
        self.vertices.push(mk(br_off, br_uv));
        self.vertices.push(mk(tr_off, tr_uv));

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
        let needed = (self.vertices.len() * std::mem::size_of::<BillboardVertex>()) as u64;
        if needed > self.vertex_buffer_capacity {
            let new_capacity = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.billboard.vbo"),
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
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        let bytes = (self.vertices.len() * std::mem::size_of::<BillboardVertex>()) as u64;
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..bytes));
        for run in &self.runs {
            let Some(tex) = textures.get(run.texture.0 as usize) else {
                continue;
            };
            let Some(bg) = tex.billboard_bind_group.as_ref() else {
                continue;
            };
            pass.set_bind_group(1, bg, &[]);
            let start = run.vertex_offset;
            let end = start + run.vertex_count;
            pass.draw(start..end, 0..1);
        }
    }
}
