//! Procedural sky pipeline.
//!
//! A single fullscreen-triangle draw that fakes atmospheric scattering from
//! the day/night cycle — see `shaders/sky.wgsl`. It runs **first** in the
//! main pass (before the 3D mesh) so the terrain, lines, billboards, and 2D
//! UI all layer on top. It carries one frame-global [`SkyUniform`] (the
//! inverse view-projection + camera + sun/moon + sky palette) that the
//! renderer fills each frame from the same [`crate::LightRig`] that
//! drives the mesh shading, so the sky and the lit terrain stay coherent.
//!
//! Depth: shares the main pass's `Depth32Float` attachment but neither writes
//! nor meaningfully tests it (`depth_compare = Always`, `depth_write = false`)
//! — it's a background fill, and the mesh (drawn after, depth-writing) paints
//! over it wherever geometry exists. The colour target is opaque (no blend).

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::pipeline_mesh::DEPTH_FORMAT;

/// CPU-side mirror of the WGSL `Sky` uniform. `vec4` lanes throughout so
/// std140 alignment is trivially correct (the `mat4` is 16-byte aligned and
/// each following field is a full 16-byte row).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SkyUniform {
    pub inv_view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_color: [f32; 4],
    pub moon_dir: [f32; 4],
    pub moon_color: [f32; 4],
    pub zenith: [f32; 4],
    pub horizon: [f32; 4],
    pub star_rotation: [[f32; 4]; 4],
}

const SKY_UNIFORM_SIZE: u64 = std::mem::size_of::<SkyUniform>() as u64;

impl Default for SkyUniform {
    /// A dim night sky with no sun/moon and an identity transform — a safe
    /// seed for the first frame before the renderer writes a real one.
    fn default() -> Self {
        Self {
            inv_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            camera_pos: [0.0; 4],
            sun_dir: [0.0, 1.0, 0.0, 0.0],
            sun_color: [0.0; 4],
            moon_dir: [0.0, 1.0, 0.0, 0.0],
            moon_color: [0.0; 4],
            zenith: [0.012, 0.016, 0.030, 0.0],
            horizon: [0.030, 0.040, 0.085, 0.0],
            star_rotation: Mat4::IDENTITY.to_cols_array_2d(),
        }
    }
}

/// The procedural-sky pipeline. One uniform, one bind group, no vertex
/// buffer — the fullscreen triangle is generated from `@builtin(vertex_index)`.
///
/// The render pipeline is baked for BOTH colour formats (swapchain `surface_format` and
/// [`crate::HDR_FORMAT`]) over the one shared bind group; [`Self::render`] selects the
/// variant by [`crate::TargetColor`]. The bind group / uniform are format-independent.
pub struct SkyPipeline {
    pipeline: [wgpu::RenderPipeline; 2],
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
}

impl SkyPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.sky.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sky.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.sky.bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(SKY_UNIFORM_SIZE),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.sky.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Bake the pipeline for both colour formats over the one shared layout — only the
        // colour-target format differs. `render` picks the variant by `TargetColor`.
        let make = |fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.sky.pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: fmt,
                        // Opaque background fill — replace, no blend.
                        blend: None,
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
                // Background: never write depth, always pass. The mesh draws after
                // and paints over wherever geometry exists.
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
            })
        };
        let pipeline = [make(surface_format), make(crate::HDR_FORMAT)];

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.sky.uniform"),
            contents: bytemuck::bytes_of(&SkyUniform::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.sky.bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        Self {
            pipeline,
            bind_group,
            uniform_buf,
        }
    }

    /// Write this frame's sky uniform. Called from `Renderer::end_frame` when
    /// a sky is requested, mirroring the other pipelines' per-frame uploads.
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: SkyUniform) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Emit the fullscreen sky triangle. The renderer calls this first in the
    /// main pass, only on frames that requested a sky. `target` selects the colour-format
    /// variant of the pipeline (sRGB or HDR) for the surface being encoded.
    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, target: crate::TargetColor) {
        pass.set_pipeline(&self.pipeline[target as usize]);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the sky pipeline under a validation error scope — this compiles
    /// `sky.wgsl` and checks it against the bind-group layout, so a malformed
    /// shader fails here rather than at app launch. Skips without a GPU.
    #[test]
    fn sky_pipeline_compiles_shader() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            eprintln!("sky_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("flicker.sky_test.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = SkyPipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.set_uniform(&queue, SkyUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "sky.wgsl failed validation: {err:?}");
    }
}
