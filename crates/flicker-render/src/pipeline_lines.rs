//! Immediate-mode line pipeline.
//!
//! Pure CPU-side line accumulation per frame. Callers submit line
//! segments via [`Renderer::draw_bounding_box`](crate::Renderer::draw_bounding_box)
//! (and any future gizmo helpers); on `prepare` the accumulated
//! vertices are uploaded to a single `LineList` vertex buffer, and on
//! `render` a single non-indexed draw call emits every line for the
//! frame. The pipeline shares the mesh pipeline's camera uniform so
//! both 3D meshes and lines see the same view-projection.
//!
//! Depth: shares the depth attachment with the mesh pipeline. Tests
//! depth with `LessEqual` (so lines get occluded by 3D geometry in
//! front of them) but does NOT write depth (so lines don't occlude
//! anything drawn later in the same pass).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::pipeline_mesh::DEPTH_FORMAT;

/// One vertex of an immediate-mode line. Position in world space,
/// color is the line's per-vertex color (segments use the same color
/// at both endpoints unless a caller varies them).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 4],
}

const LINE_VERTEX_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

pub struct LinesPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    vertex_buffer_capacity: u64,
    vertices: Vec<LineVertex>,
}

impl LinesPipeline {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        camera_buf: &wgpu::Buffer,
        depth_compare: wgpu::CompareFunction,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.lines.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/lines.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.lines.bgl"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.lines.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.lines.bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buf.as_entire_binding(),
            }],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &LINE_VERTEX_ATTRS,
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flicker.lines.pipeline"),
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
                topology: wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // Depth-tested but not depth-writing: lines should be
            // occluded by 3D geometry in front of them, but should not
            // themselves occlude later draws in the same pass.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let initial_capacity = (std::mem::size_of::<LineVertex>() * 64) as u64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.lines.vbo"),
            size: initial_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            vertex_buffer,
            vertex_buffer_capacity: initial_capacity,
            vertices: Vec::new(),
        }
    }

    /// Reset the per-frame line queue. Called from `Renderer::begin_frame`.
    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    /// Queue one line segment with the same color at both endpoints.
    pub fn push_segment(&mut self, a: Vec3, b: Vec3, color: [f32; 4]) {
        self.vertices.push(LineVertex {
            position: [a.x, a.y, a.z],
            color,
        });
        self.vertices.push(LineVertex {
            position: [b.x, b.y, b.z],
            color,
        });
    }

    /// Queue the 12 edges of an axis-aligned bounding box.
    pub fn push_box(&mut self, min: Vec3, max: Vec3, color: [f32; 4]) {
        let c = [
            Vec3::new(min.x, min.y, min.z), // 0
            Vec3::new(max.x, min.y, min.z), // 1
            Vec3::new(min.x, max.y, min.z), // 2
            Vec3::new(max.x, max.y, min.z), // 3
            Vec3::new(min.x, min.y, max.z), // 4
            Vec3::new(max.x, min.y, max.z), // 5
            Vec3::new(min.x, max.y, max.z), // 6
            Vec3::new(max.x, max.y, max.z), // 7
        ];
        // 4 X-aligned edges
        self.push_segment(c[0], c[1], color);
        self.push_segment(c[2], c[3], color);
        self.push_segment(c[4], c[5], color);
        self.push_segment(c[6], c[7], color);
        // 4 Y-aligned edges
        self.push_segment(c[0], c[2], color);
        self.push_segment(c[1], c[3], color);
        self.push_segment(c[4], c[6], color);
        self.push_segment(c[5], c[7], color);
        // 4 Z-aligned edges
        self.push_segment(c[0], c[4], color);
        self.push_segment(c[1], c[5], color);
        self.push_segment(c[2], c[6], color);
        self.push_segment(c[3], c[7], color);
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.vertices.is_empty() {
            return;
        }
        let needed = (self.vertices.len() * std::mem::size_of::<LineVertex>()) as u64;
        if needed > self.vertex_buffer_capacity {
            let new_capacity = needed.next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.lines.vbo"),
                size: new_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_buffer_capacity = new_capacity;
        }
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.vertices.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        let bytes = (self.vertices.len() * std::mem::size_of::<LineVertex>()) as u64;
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(0..bytes));
        pass.draw(0..self.vertices.len() as u32, 0..1);
    }
}
