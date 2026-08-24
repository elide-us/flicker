//! Volumetric ground-fog pass.
//!
//! A single fullscreen-triangle draw that **raymarches** a thin horizontal fog slab of animated
//! fbm noise (see `shaders/ground_fog.wgsl`). Runs in the overlay pass, depth-aware (samples the
//! scene depth so geometry occludes the fog), and composites with premultiplied-alpha "over".
//! Because density is integrated *along the ray*, overlapping fog composites correctly — the
//! order-independent-transparency problem the billboard/quad approach has, avoided by
//! construction. A reusable ground-fog / atmospheric-layer effect.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::pipeline_mesh::DEPTH_FORMAT;

/// Per-frame parameters for the ground fog — the public API input (the renderer turns this + the
/// camera into the GPU uniform). Distances are world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundFog {
    /// Fog colour (linear RGB) — e.g. cool moonlit blue-white.
    pub color: Vec3,
    /// Bottom of the fog slab (world Y).
    pub bottom: f32,
    /// Top of the fog slab (world Y). Fog fades from dense at `bottom` to clear at `top`.
    pub top: f32,
    /// Overall density / extinction strength.
    pub density: f32,
    /// Spatial noise frequency (larger = smaller, busier blobs).
    pub noise_scale: f32,
    /// How much of the plane is fogged, `0..1` (a density threshold; higher = more coverage).
    pub coverage: f32,
    /// Wind (XZ) that drifts the fog across the ground.
    pub wind: Vec2,
    /// Animation clock (seconds).
    pub time: f32,
    /// Vertical falloff power (`1` = linear top-fade, higher = fog hugs the bottom).
    pub height_power: f32,
    /// XZ rectangle the fog is **localized** to (min corner). Outside it the fog is 0.
    pub bounds_min: Vec2,
    /// XZ rectangle the fog is localized to (max corner).
    pub bounds_max: Vec2,
    /// Feather distance (world units) over which the fog fades to 0 as it approaches the
    /// rectangle edge — a soft margin. `0` = a hard edge; large bounds = effectively global.
    pub edge_fade: f32,
    /// Depth (world units) the fog **spills below the slab at the edges** — a falling
    /// "curtain" that pours over the rim, like fog off a floating island. `0` = flat fog.
    pub fall_depth: f32,
    /// Downward flow speed of the falling curtain (animates the pour).
    pub flow: f32,
}

impl Default for GroundFog {
    fn default() -> Self {
        Self {
            color: Vec3::new(0.55, 0.62, 0.78),
            bottom: -1.0,
            top: 0.0,
            density: 1.0,
            noise_scale: 0.25,
            coverage: 0.55,
            wind: Vec2::new(0.4, 0.1),
            time: 0.0,
            height_power: 1.5,
            bounds_min: Vec2::splat(-1.0e6),
            bounds_max: Vec2::splat(1.0e6),
            edge_fade: 1.0,
            fall_depth: 0.0,
            flow: 0.0,
        }
    }
}

/// CPU-side mirror of the WGSL `Fog` uniform. `vec4` lanes for trivial std140.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GroundFogUniform {
    inv_view_proj: [[f32; 4]; 4],
    /// `(x, y, z, edge_fade)`
    camera_pos: [f32; 4],
    /// `(r, g, b, density)`
    color: [f32; 4],
    /// `(bottom, top, noise_scale, coverage)`
    band: [f32; 4],
    /// `(wind_x, wind_z, time, height_power)`
    wind: [f32; 4],
    /// `(min_x, min_z, max_x, max_z)` — the XZ rectangle the fog is localized to.
    bounds: [f32; 4],
    /// `(fall_depth, flow, _, _)` — the edge "curtain" that spills off the rim.
    spill: [f32; 4],
}

const FOG_UNIFORM_SIZE: u64 = std::mem::size_of::<GroundFogUniform>() as u64;

impl Default for GroundFogUniform {
    fn default() -> Self {
        Self::from_params(&GroundFog::default(), Mat4::IDENTITY, Vec3::ZERO)
    }
}

impl GroundFogUniform {
    /// Build the GPU uniform from the per-frame params + the camera's inverse view-projection.
    pub fn from_params(p: &GroundFog, inv_view_proj: Mat4, camera_pos: Vec3) -> Self {
        Self {
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, p.edge_fade],
            color: [p.color.x, p.color.y, p.color.z, p.density],
            band: [p.bottom, p.top, p.noise_scale, p.coverage],
            wind: [p.wind.x, p.wind.y, p.time, p.height_power],
            bounds: [
                p.bounds_min.x,
                p.bounds_min.y,
                p.bounds_max.x,
                p.bounds_max.y,
            ],
            spill: [p.fall_depth, p.flow, 0.0, 0.0],
        }
    }
}

/// The ground-fog pipeline. A uniform + the depth texture of the SURFACE being drawn, no vertex
/// buffer. Every surface has its own depth (the window's, recreated on resize; each offscreen
/// target's), so the bind groups are built on demand and cached per depth id via
/// [`bind_depth`](GroundFogPipeline::bind_depth) — the renderer selects the one for the pass it
/// is encoding.
pub struct GroundFogPipeline {
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

impl GroundFogPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.ground_fog.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ground_fog.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.ground_fog.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(FOG_UNIFORM_SIZE),
                    },
                    count: None,
                },
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
            label: Some("flicker.ground_fog.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Bake for both colour formats over the one shared layout; only the colour-target
        // format differs. `render` picks the variant by `TargetColor`.
        let make = |fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.ground_fog.pipeline"),
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
                        // Premultiplied "over": `out = src.rgb + dst·(1−src.a)`.
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
                // A volume: never write depth, always pass (depth read via the bound texture).
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
            label: Some("flicker.ground_fog.uniform"),
            size: FOG_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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
                label: Some("flicker.ground_fog.bind_group"),
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

    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: GroundFogUniform) {
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

    /// Compile `ground_fog.wgsl` against the bind-group layout under a validation scope, so a
    /// malformed shader fails here rather than at app launch. Skips without a GPU.
    #[test]
    fn ground_fog_pipeline_compiles_shader() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
        else {
            eprintln!("ground_fog_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("flicker.ground_fog_test.device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .expect("request device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = GroundFogPipeline::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
        pipeline.set_uniform(&queue, GroundFogUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "ground_fog.wgsl failed validation: {err:?}");
    }
}
