//! Volumetric protoplanetary-disk pass.
//!
//! A single fullscreen-triangle draw (like the sky) that **raymarches** a 3D dust-density
//! field — a flared annular disk with turbulence — accumulating dark extinction plus a warm,
//! star-lit inner glow. See `shaders/volumetric.wgsl`. It runs **after the sky** in the main
//! pass and composites over it with alpha blending, so the formed bodies/glow (drawn after)
//! layer on top. Depth: `Always` / no write (a background-ish volume).
//!
//! The density is **driven by the simulation**: it dissipates inside-out as formation
//! progresses and carves gaps around the forming planets (the `bodies` array) — so the cloud
//! visibly *is* the disk being consumed, not a decorative field on a timer.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::pipeline_mesh::DEPTH_FORMAT;

/// Max forming-planet gaps the shader carves (sized for the std140 array below).
pub const MAX_VOLUMETRIC_BODIES: usize = 32;

/// Per-frame parameters for the volumetric disk — the public API input (the renderer turns
/// this + the camera into the GPU uniform). Distances are world units (AU in the solar-system
/// example); `formation` and `time` come from the formation clock.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumetricDisk {
    pub inner: f32,
    pub outer: f32,
    pub snow_line: f32,
    /// Scale-height-at-1-unit factor (the disk flares: `h(r) = scale_height · r`).
    pub scale_height: f32,
    /// Overall density / extinction strength.
    pub density: f32,
    /// Formation progress `0..1` — drives inside-out dissipation.
    pub formation: f32,
    /// Animation clock (radians-ish) — drives the differential swirl.
    pub time: f32,
    /// Bulk dust tint.
    pub tint: Vec3,
    /// Warm star-lit inner-glow colour.
    pub glow: Vec3,
    /// Forming planets that carve **annular** gaps: `(orbital radius, gap width)` in world
    /// units. Truncated to [`MAX_VOLUMETRIC_BODIES`].
    pub gaps: Vec<(f32, f32)>,
}

impl Default for VolumetricDisk {
    fn default() -> Self {
        Self {
            inner: 0.3,
            outer: 15.0,
            snow_line: 2.7,
            scale_height: 0.06,
            density: 1.0,
            formation: 0.0,
            time: 0.0,
            tint: Vec3::new(0.10, 0.09, 0.10),
            glow: Vec3::new(1.0, 0.55, 0.25),
            gaps: Vec::new(),
        }
    }
}

/// CPU-side mirror of the WGSL `Disk` uniform. `vec4` lanes throughout for trivial std140.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct VolumetricDiskUniform {
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    /// `(inner, outer, snow_line, scale_height)`
    geom: [f32; 4],
    /// `(density, formation, time, body_count)`
    state: [f32; 4],
    tint: [f32; 4],
    glow: [f32; 4],
    /// `(orbital_radius, gap_width, _, _)` per forming planet — an annular gap.
    gaps: [[f32; 4]; MAX_VOLUMETRIC_BODIES],
}

const DISK_UNIFORM_SIZE: u64 = std::mem::size_of::<VolumetricDiskUniform>() as u64;

impl Default for VolumetricDiskUniform {
    fn default() -> Self {
        Self::from_params(&VolumetricDisk::default(), Mat4::IDENTITY, Vec3::ZERO)
    }
}

impl VolumetricDiskUniform {
    /// Build the GPU uniform from the per-frame params + the camera's inverse view-projection.
    pub fn from_params(p: &VolumetricDisk, inv_view_proj: Mat4, camera_pos: Vec3) -> Self {
        let mut gaps = [[0.0f32; 4]; MAX_VOLUMETRIC_BODIES];
        let n = p.gaps.len().min(MAX_VOLUMETRIC_BODIES);
        for (slot, (radius, width)) in gaps.iter_mut().zip(p.gaps.iter()).take(n) {
            *slot = [*radius, *width, 0.0, 0.0];
        }
        Self {
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
            geom: [p.inner, p.outer, p.snow_line, p.scale_height],
            state: [p.density, p.formation, p.time, n as f32],
            tint: [p.tint.x, p.tint.y, p.tint.z, 0.0],
            glow: [p.glow.x, p.glow.y, p.glow.z, 0.0],
            gaps,
        }
    }
}

/// The volumetric-disk pipeline. A uniform + the depth texture of the SURFACE being drawn, no
/// vertex buffer. The bind group depends on the depth view, and every surface has its own
/// (the window's, recreated on resize; each offscreen target's), so the groups are built on
/// demand and cached per depth id via [`bind_depth`](VolumetricPipeline::bind_depth) — the
/// renderer selects the one for the pass it is encoding.
pub struct VolumetricPipeline {
    /// Baked for both colour formats (swapchain + [`crate::HDR_FORMAT`]) over the one
    /// shared uniform + per-depth-id bind cache; [`Self::render`] selects by
    /// [`crate::TargetColor`].
    pipeline: [wgpu::RenderPipeline; 2],
    bind_group_layout: wgpu::BindGroupLayout,
    /// One bind group per depth texture the pass has sampled, keyed by the renderer's
    /// depth id; dropped through [`forget`](Self::forget) when that texture goes away.
    bind_groups: HashMap<u64, wgpu::BindGroup>,
    /// The depth id the next `render` samples — the surface currently being encoded.
    active: Option<u64>,
    uniform_buf: wgpu::Buffer,
}

impl VolumetricPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.volumetric.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/volumetric.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.volumetric.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(DISK_UNIFORM_SIZE),
                    },
                    count: None,
                },
                // The opaque pass's depth buffer, sampled (read-only) to clamp rays at bodies.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.volumetric.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Bake for both colour formats over the one shared layout; only the colour-target
        // format differs. `render` picks the variant by `TargetColor`.
        let make = |fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.volumetric.pipeline"),
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
                        // Volumetric "over": the shader outputs premultiplied in-scattered
                        // colour + coverage alpha, so `out = src.rgb + dst·(1−src.a)`.
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
                // A background-ish volume: never write depth, always pass.
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

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.volumetric.uniform"),
            size: DISK_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // A bind group is built the first time a surface's depth is bound (`bind_depth`);
        // until one is active, `render` is a no-op.

        Self {
            pipeline,
            bind_group_layout,
            bind_groups: HashMap::new(),
            active: None,
            uniform_buf,
        }
    }

    /// Select the depth the next `render` samples: the depth attachment of the SURFACE being
    /// drawn — the window's, or an offscreen target's own — keyed by the renderer's depth id.
    /// Built once per depth texture and cached, so a steady frame allocates nothing.
    pub fn bind_depth(
        &mut self,
        device: &wgpu::Device,
        depth_id: u64,
        depth_view: &wgpu::TextureView,
    ) {
        self.bind_groups.entry(depth_id).or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("flicker.volumetric.bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(depth_view),
                    },
                ],
            })
        });
        self.active = Some(depth_id);
    }

    /// Drop the bind group of a depth texture that no longer exists — a freed or resized
    /// target, or the window's depth after a resize.
    pub fn forget(&mut self, depth_id: u64) {
        self.bind_groups.remove(&depth_id);
        if self.active == Some(depth_id) {
            self.active = None;
        }
    }

    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: VolumetricDiskUniform) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, target: crate::TargetColor) {
        let Some(bind_group) = self.active.and_then(|id| self.bind_groups.get(&id)) else {
            return; // no surface depth bound yet
        };
        pass.set_pipeline(&self.pipeline[target as usize]);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile `volumetric.wgsl` against the bind-group layout under a validation scope, so a
    /// malformed shader fails here rather than at app launch. Skips without a GPU.
    #[test]
    fn volumetric_pipeline_compiles_shader() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            eprintln!("volumetric_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("flicker.volumetric_test.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = VolumetricPipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.set_uniform(&queue, VolumetricDiskUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "volumetric.wgsl failed validation: {err:?}");
    }
}
