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
    /// Alpha-blended, depth-writing — the default (occludes/ordered against 3D mesh).
    pipeline: wgpu::RenderPipeline,
    /// Additive, **non**-depth-writing — for soft glows (star, dust, flashes). Additive
    /// billboards don't clip each other (they don't write depth), so overlapping soft
    /// quads stack into a glow instead of producing hard cutout artifacts.
    pipeline_additive: wgpu::RenderPipeline,
    camera_buf: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    vertices: Vec<BillboardVertex>,
    runs: Vec<Run>,
    // Separate queue for additive billboards, so they all render in one pass *after* the
    // alpha ones (glow on top) and keep their own submission order.
    vertex_buffer_additive: wgpu::Buffer,
    vertex_buffer_additive_capacity: u64,
    vertices_additive: Vec<BillboardVertex>,
    runs_additive: Vec<Run>,
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

        // Both variants share everything but their blend + depth-write state.
        let make_pipeline = |label: &str, blend: wgpu::BlendState, depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
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
                        blend: Some(blend),
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
                    depth_write_enabled: depth_write,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };

        let pipeline = make_pipeline(
            "flicker.billboard.pipeline",
            wgpu::BlendState::ALPHA_BLENDING,
            true,
        );
        // Additive: `out.rgb * out.a` is *added* to the target (glow), depth NOT written so
        // overlapping glows stack instead of clipping each other.
        let additive_blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline_additive =
            make_pipeline("flicker.billboard.pipeline.additive", additive_blend, false);

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
        let make_vbo = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.billboard.vbo"),
                size: initial_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        Self {
            pipeline,
            pipeline_additive,
            camera_buf,
            camera_bind_group,
            texture_bind_group_layout,
            sampler,
            vertex_buffer: make_vbo(),
            vertex_buffer_capacity: initial_capacity,
            vertices: Vec::new(),
            runs: Vec::new(),
            vertex_buffer_additive: make_vbo(),
            vertex_buffer_additive_capacity: initial_capacity,
            vertices_additive: Vec::new(),
            runs_additive: Vec::new(),
        }
    }

    /// Reset the per-frame draw queue.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.runs.clear();
        self.vertices_additive.clear();
        self.runs_additive.clear();
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

    /// Queue one alpha-blended (default) billboard.
    pub fn push(
        &mut self,
        texture: TextureHandle,
        world_position: Vec3,
        world_size: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [f32; 4],
    ) {
        push_quad(
            &mut self.vertices,
            &mut self.runs,
            texture,
            world_position,
            world_size,
            uv_min,
            uv_max,
            color,
        );
    }

    /// Queue one **additive** (glow) billboard — rendered after all alpha ones, without
    /// writing depth.
    pub fn push_additive(
        &mut self,
        texture: TextureHandle,
        world_position: Vec3,
        world_size: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [f32; 4],
    ) {
        push_quad(
            &mut self.vertices_additive,
            &mut self.runs_additive,
            texture,
            world_position,
            world_size,
            uv_min,
            uv_max,
            color,
        );
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        upload(
            device,
            queue,
            &mut self.vertex_buffer,
            &mut self.vertex_buffer_capacity,
            &self.vertices,
        );
        upload(
            device,
            queue,
            &mut self.vertex_buffer_additive,
            &mut self.vertex_buffer_additive_capacity,
            &self.vertices_additive,
        );
    }

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        textures: &'a [Option<crate::texture::LoadedTexture>],
    ) {
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        // Alpha-blended (depth-writing) billboards first…
        draw_runs(
            pass,
            &self.pipeline,
            &self.vertex_buffer,
            &self.vertices,
            &self.runs,
            textures,
        );
        // …then additive glow on top (no depth write).
        draw_runs(
            pass,
            &self.pipeline_additive,
            &self.vertex_buffer_additive,
            &self.vertices_additive,
            &self.runs_additive,
            textures,
        );
    }
}

/// Append one quad (two triangles) to `vertices`, coalescing into the last `runs` entry
/// when it shares the same texture.
#[allow(clippy::too_many_arguments)]
fn push_quad(
    vertices: &mut Vec<BillboardVertex>,
    runs: &mut Vec<Run>,
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

    let vertex_offset = vertices.len() as u32;
    vertices.push(mk(tl_off, tl_uv));
    vertices.push(mk(bl_off, bl_uv));
    vertices.push(mk(br_off, br_uv));
    vertices.push(mk(tl_off, tl_uv));
    vertices.push(mk(br_off, br_uv));
    vertices.push(mk(tr_off, tr_uv));

    match runs.last_mut() {
        Some(run) if run.texture == texture => run.vertex_count += 6,
        _ => runs.push(Run {
            texture,
            vertex_offset,
            vertex_count: 6,
        }),
    }
}

/// Grow + upload a vertex buffer for one queue.
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    vertices: &[BillboardVertex],
) {
    if vertices.is_empty() {
        return;
    }
    let needed = std::mem::size_of_val(vertices) as u64;
    if needed > *capacity {
        let new_capacity = needed.next_power_of_two();
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.billboard.vbo"),
            size: new_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        *capacity = new_capacity;
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(vertices));
}

/// Draw one queue's runs with the given pipeline + buffer.
fn draw_runs<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    pipeline: &'a wgpu::RenderPipeline,
    vertex_buffer: &'a wgpu::Buffer,
    vertices: &[BillboardVertex],
    runs: &'a [Run],
    textures: &'a [Option<crate::texture::LoadedTexture>],
) {
    if runs.is_empty() {
        return;
    }
    pass.set_pipeline(pipeline);
    let bytes = std::mem::size_of_val(vertices) as u64;
    pass.set_vertex_buffer(0, vertex_buffer.slice(0..bytes));
    for run in runs {
        let Some(tex) = textures
            .get(run.texture.0 as usize)
            .and_then(|t| t.as_ref())
        else {
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
