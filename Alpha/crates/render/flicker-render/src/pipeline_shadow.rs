//! Shadow-map SAMPLING resources — the shared `@group(2/3)` [`ShadowBind`] the lit
//! pipelines (mesh, mesh_textured, skinned) read to darken the one light a shadow is cast
//! for.
//!
//! This is NOT a pipeline of its own: the shadow DEPTH is produced by rendering the
//! casters into an ordinary offscreen target from the light's view (the producer stage,
//! `render_to_texture` with a light-view matrix). What lives here is the CONSUMER side —
//! a bind group of `{ uniform, depth texture, comparison sampler }` the lit shaders sample
//! through `shadow_factor()` (see `shaders/frame_prelude.wgsl`).
//!
//! It mirrors two patterns already in the tree: [`FrameBindGroup`](crate::pipeline_mesh)
//! for the shared per-frame group, and [`bind_depth`](crate::pipeline_ground_fog) for the
//! per-depth-id bind cache. A **default** 1×1 depth + identity matrix + `enabled = 0` is
//! bound for every surface that names no shadow, so shadow_factor returns exactly `1.0`
//! and the lit output is byte-identical to the no-shadow path.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use crate::pipeline_mesh::DEPTH_FORMAT;

/// CPU-side mirror of the WGSL `ShadowUniform` — the ONE light-view-projection (shared with
/// the producer camera) plus the sampling params. `vec4` lane for trivial std140.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ShadowUniform {
    light_view_proj: [[f32; 4]; 4],
    /// `(bias, enabled, texel_size, light_index)`.
    params: [f32; 4],
}

const SHADOW_UNIFORM_SIZE: u64 = std::mem::size_of::<ShadowUniform>() as u64;

impl ShadowUniform {
    /// An ENABLED shadow: sample `light_view_proj`'s depth with `bias`, a `texel_size`
    /// (1 / shadow-map dimension) for the PCF3 kernel, cast for rig slot `light`.
    pub fn enabled(light_view_proj: Mat4, bias: f32, texel_size: f32, light: u32) -> Self {
        Self {
            light_view_proj: light_view_proj.to_cols_array_2d(),
            params: [bias, 1.0, texel_size, light as f32],
        }
    }

    /// The DISABLED default: identity matrix, `enabled = 0` — `shadow_factor` short-circuits
    /// to `1.0`, so a surface naming no shadow is byte-identical to the no-shadow path.
    pub fn disabled() -> Self {
        Self {
            light_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            params: [0.0, 0.0, 0.0, 0.0],
        }
    }
}

impl Default for ShadowUniform {
    fn default() -> Self {
        Self::disabled()
    }
}

/// The shared shadow bind group + its per-depth-id cache. Owns ONE uniform buffer (shared
/// by every cached bind group), a comparison sampler, a 1×1 default depth texture, and the
/// DEFAULT bind group bound whenever no shadow is active. The lit pipelines build their
/// `@group(2)` (mesh/skinned) or `@group(3)` (mesh_textured) against [`Self::layout`] and
/// bind [`Self::active_bind_group`] while encoding.
pub struct ShadowBind {
    layout: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Kept alive so `default_view` stays valid.
    #[allow(dead_code)]
    default_depth: wgpu::Texture,
    /// The DEFAULT bind group's depth view (a 1×1 depth, never rendered — sampled only when
    /// `enabled = 0`, whose result is discarded).
    #[allow(dead_code)]
    default_view: wgpu::TextureView,
    default_bind: wgpu::BindGroup,
    /// One bind group per shadow-source depth id, built on demand and dropped via
    /// [`forget`](Self::forget) when the source target goes away.
    binds: HashMap<u64, wgpu::BindGroup>,
    /// The depth id the next encode samples; `None` = the default (no shadow this surface).
    active: Option<u64>,
}

impl ShadowBind {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.shadow.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(SHADOW_UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        // Depth sample type + a comparison sampler = hardware PCF via
                        // `textureSampleCompareLevel`.
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.shadow.uniform"),
            size: SHADOW_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Comparison sampler: `LessEqual` returns 1 (lit) when the sampled reference depth
        // is nearer-or-equal to the stored caster depth, else 0 (occluded).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flicker.shadow.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        // A 1×1 depth texture for the DEFAULT bind: never rendered into (sampled only when
        // `enabled = 0`, whose comparison result is discarded), so its garbage is harmless.
        let default_depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flicker.shadow.default_depth"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let default_view = default_depth.create_view(&wgpu::TextureViewDescriptor::default());
        let default_bind = make_bind(device, &layout, &uniform_buf, &sampler, &default_view);

        Self {
            layout,
            uniform_buf,
            sampler,
            default_depth,
            default_view,
            default_bind,
            binds: HashMap::new(),
            active: None,
        }
    }

    /// The layout every lit pipeline builds its shadow group against.
    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Select the shadow-source depth the next encode samples — the depth attachment of the
    /// producer surface, keyed by the renderer's depth id (built once per id, then cached).
    pub fn bind_shadow(
        &mut self,
        device: &wgpu::Device,
        depth_id: u64,
        depth_view: &wgpu::TextureView,
    ) {
        let (layout, uniform_buf, sampler) = (&self.layout, &self.uniform_buf, &self.sampler);
        self.binds
            .entry(depth_id)
            .or_insert_with(|| make_bind(device, layout, uniform_buf, sampler, depth_view));
        self.active = Some(depth_id);
    }

    /// Bind the DEFAULT (1×1 depth, whatever the uniform says) — the surface names no shadow.
    pub fn bind_default(&mut self) {
        self.active = None;
    }

    /// Drop the bind group of a shadow-source depth that no longer exists (a freed/resized
    /// target). Mirrors the depth-cache `forget` sites.
    pub fn forget(&mut self, depth_id: u64) {
        self.binds.remove(&depth_id);
        if self.active == Some(depth_id) {
            self.active = None;
        }
    }

    /// Upload the shadow params (the light-view matrix + bias/enabled/texel/light).
    pub fn set_uniform(&self, queue: &wgpu::Queue, uniform: ShadowUniform) {
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// The bind group the lit pipelines set at their shadow group: the active source, or the
    /// default when no shadow is active this surface.
    pub fn active_bind_group(&self) -> &wgpu::BindGroup {
        self.active
            .and_then(|id| self.binds.get(&id))
            .unwrap_or(&self.default_bind)
    }
}

fn make_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    depth_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("flicker.shadow.bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **GATE — a surface naming no shadow is DISABLED (enabled = 0), and the shipped
    /// WGSL short-circuits it to a shadow factor of exactly 1.0.** This is the S6
    /// zero-visual-change proof: the default bound for every non-shadow surface carries
    /// `enabled = 0`, `shadow_factor` returns `1.0` for it, and the lit loops multiply by
    /// that `vis` — `t * 1.0 == t` — so nothing on the untouched roster changes a pixel.
    /// Reads the real channels: the `disabled` uniform's lanes AND the shipped prelude text.
    #[test]
    fn the_default_shadow_is_disabled_and_returns_full_light() {
        // The default's `enabled` lane is 0; a cast shadow's is 1.
        assert_eq!(
            ShadowUniform::disabled().params[1],
            0.0,
            "the default (no-shadow) uniform must be disabled"
        );
        assert_eq!(
            ShadowUniform::enabled(Mat4::IDENTITY, 0.001, 0.5, 3).params,
            [0.001, 1.0, 0.5, 3.0],
            "an enabled shadow packs (bias, 1, texel, light)"
        );
        // The shipped prelude guards on `enabled` and returns full light when off, so the
        // multiply is identity for a disabled surface.
        let prelude = crate::pipeline_mesh::FRAME_PRELUDE;
        assert!(
            prelude.contains("if (shadow_uni.params.y < 0.5)") && prelude.contains("return 1.0;"),
            "shadow_factor must short-circuit to 1.0 when the shadow is disabled"
        );
    }

    /// **GATE (GPU-optional) — the shadow-sampling path compiles AND draws.** Builds the
    /// shared [`ShadowBind`] + the flat lit [`MeshPipeline`](crate::pipeline_mesh), binds a
    /// real shadow-source depth at `@group(2)` with an ENABLED uniform, and executes a mesh
    /// draw that samples it into an offscreen target — so a malformed `shadow_factor`, a
    /// layout mismatch, or a bad comparison-sampler binding fails HERE, not at app launch.
    /// Skips cleanly with no GPU adapter.
    #[test]
    fn shadow_sampling_pipeline_compiles_and_draws() {
        let Some((device, queue)) =
            crate::pipeline_mesh::tests::test_device("flicker.shadow_test.device")
        else {
            eprintln!("shadow_sampling_pipeline_compiles_and_draws: no GPU adapter — skipping");
            return;
        };
        use crate::mesh::{MeshHandle, MeshIndices, MeshVertex};
        use crate::pipeline_mesh::{FrameBindGroup, MeshPipeline, SceneUniform};

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let align = device.limits().min_uniform_buffer_offset_alignment;
        let frame = FrameBindGroup::new(&device);
        let mut shadow = ShadowBind::new(&device);
        let mut mesh = MeshPipeline::new(&device, &frame, &shadow, fmt, align);
        frame.set_scene_uniform(&queue, SceneUniform::default());
        frame.set_camera_matrix(&queue, Mat4::IDENTITY);

        let make_tex = |f, label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 16,
                    height: 16,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: f,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        // A shadow-source depth texture bound at @group(2) with an ENABLED uniform, so the
        // draw actually enters `shadow_factor`'s `textureSampleCompareLevel`.
        let shadow_depth = make_tex(DEPTH_FORMAT, "flicker.shadow_test.src_depth");
        let sview = shadow_depth.create_view(&Default::default());
        shadow.bind_shadow(&device, 42, &sview);
        shadow.set_uniform(
            &queue,
            ShadowUniform::enabled(Mat4::IDENTITY, 0.0015, 1.0 / 16.0, 0),
        );

        // One triangle, uploaded into a store the pipeline indexes by handle (the renderer
        // owns this store in production; here the test does).
        let v = |p: [f32; 3]| MeshVertex {
            position: p,
            normal: [0.0, 0.0, 1.0],
            material: 0,
        };
        let loaded = mesh.upload(
            &device,
            &[
                v([-0.5, -0.5, 0.0]),
                v([0.5, -0.5, 0.0]),
                v([0.0, 0.5, 0.0]),
            ],
            MeshIndices::U32(&[0, 1, 2]),
        );
        let store = vec![Some(loaded)];
        mesh.push(MeshHandle(0), Mat4::IDENTITY, [1.0; 4], false, 0.0);
        mesh.prepare(&device, &queue);

        let color = make_tex(fmt, "flicker.shadow_test.color");
        let depth = make_tex(DEPTH_FORMAT, "flicker.shadow_test.depth");
        let cview = color.create_view(&Default::default());
        let dview = depth.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.shadow_test.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cview,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &dview,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            mesh.render(&mut pass, &frame, &shadow, &store, crate::TargetColor::Srgb);
        }
        queue.submit([enc.finish()]);
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "the shadow-sampling path failed validation: {err:?}"
        );
    }
}
