//! Instanced GPU-skinning mesh pipeline — Slice 1 of the animation field viewer.
//!
//! Draws **one static skinned mesh as N instances in a single draw call**, each
//! instance with its own bone-matrix palette + model transform. This is the
//! "GPU palette skinning" step the animation handoffs earmarked as *deferred to
//! alpha*: the CPU (via `flicker-skeletal`'s pose layer) produces per-instance
//! palettes; the GPU vertex shader skins. It's the correct way to draw many
//! animated characters (crowds / the field of clips) — **not** per-model CPU skin
//! + per-frame vertex re-upload (the single-character POC path in the example).
//!
//! Layout (`shaders/skinned.wgsl`):
//! * group 0 — the renderer's ONE per-frame group: camera(@0) + the `Scene` light list(@1),
//!   shared with the mesh / mesh_textured / lines pipelines
//!   ([`FrameBindGroup`](crate::pipeline_mesh::FrameBindGroup));
//! * group 1 — `palettes`(@0) + `instances`(@1), both **read-only storage** read in
//!   the vertex stage (requires the adapter's `VERTEX_STORAGE` downlevel capability;
//!   native Metal/Vulkan/D3D12 have it — the M5 Pro + RTX 3060 boxes both do).
//!
//! Per frame the caller hands [`draw_instanced`](SkinnedMeshPipeline::draw_instanced)
//! a flat palette array (instance `i`'s bone `b` at `i*bone_count + b`) + one model
//! matrix per instance; the pipeline writes both storage buffers (growing them as
//! needed) and records a single instanced draw. **Upload strategy:** this slice uses
//! `queue.write_buffer` — correct, but a per-frame ring/linear allocator (à la
//! MiniEngine `Core/LinearAllocator.h` / `UploadBuffer.h`) is the follow-up for the
//! full field. Renderer integration + the field-viewer example are later slices.
//!
//! Additive: the flat / textured mesh pipelines are untouched.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::mesh::MeshIndices;
use crate::pipeline_mesh::{compose_lit, FrameBindGroup, DEPTH_FORMAT};
use crate::pipeline_shadow::ShadowBind;

/// Vertex for the skinned pipeline: the **bind pose** attributes, uploaded once and
/// never re-uploaded (the GPU deforms them). `joints`/`weights` are 4-influence LBS,
/// zero-padded, weights summing to 1 — the same convention as the `flicker.rig`
/// contract and `flicker-skeletal`'s CPU skinner.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct SkinnedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u32; 4],
    pub weights: [f32; 4],
}

/// Opaque handle to a mesh uploaded to the skinned pipeline. Distinct from the flat
/// and textured mesh handles so the stores never cross.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SkinnedMeshHandle(pub(crate) u32);

/// Per-instance record in the `instances` storage buffer. Mirrors `Instance` in
/// `skinned.wgsl` (mat4 + two u32 + 2×u32 pad → 80 bytes, 16-aligned element stride).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    palette_offset: u32,
    bone_count: u32,
    _pad: [u32; 2],
}
const INSTANCE_SIZE: u64 = std::mem::size_of::<InstanceRaw>() as u64;
const PALETTE_ELEM_SIZE: u64 = std::mem::size_of::<Mat4>() as u64; // 64

/// One uploaded skinned mesh (bind-pose vertex buffer + indices; persists across frames).
struct SkinnedMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    index_format: wgpu::IndexFormat,
}

const VERTEX_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Uint32x4, 4 => Float32x4
];

pub struct SkinnedMeshPipeline {
    /// Baked for both colour formats (swapchain + [`crate::HDR_FORMAT`]) over the one
    /// shared uniform / skin storage bind groups; [`Self::render`] selects by
    /// [`crate::TargetColor`].
    pipeline: [wgpu::RenderPipeline; 2],
    /// Kept: the skin bind group is rebuilt against it when the storage buffers grow.
    skin_bgl: wgpu::BindGroupLayout,
    skin_bind_group: wgpu::BindGroup,
    /// `array<mat4x4<f32>>` — all instances' bone palettes, concatenated. Rewritten
    /// each `draw_instanced`; grows (and rebuilds the skin bind group) when needed.
    palette_buf: wgpu::Buffer,
    palette_capacity: u32, // in matrices
    /// `array<Instance>` — per-instance model + palette offset.
    instance_buf: wgpu::Buffer,
    instance_capacity: u32, // in instances
    /// The mesh + instance count queued for this frame (one instanced draw).
    queued: Option<(SkinnedMeshHandle, u32)>,
    meshes: Vec<Option<SkinnedMesh>>,
    free_slots: Vec<u32>,
}

impl SkinnedMeshPipeline {
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        frame: &FrameBindGroup,
        shadow: &ShadowBind,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.skinned.shader"),
            source: wgpu::ShaderSource::Wgsl(
                compose_lit(include_str!("shaders/skinned.wgsl")).into(),
            ),
        });

        let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let skin_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.skinned.skin_bgl"),
            entries: &[storage_entry(0), storage_entry(1)],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.skinned.pipeline_layout"),
            // @group(2) = the shared shadow bind (a 1×1 default is bound for non-shadow
            // surfaces, so this is inert until a shadow is cast on a skinned receiver).
            bind_group_layouts: &[frame.layout(), &skin_bgl, shadow.layout()],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkinnedVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRS,
        };

        // Bake for both colour formats over the one shared layout; only the colour-target
        // format differs. `render` picks the variant by `TargetColor`.
        let make = |fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.skinned.pipeline"),
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
                        format: fmt,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
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
            })
        };
        let pipeline = [make(surface_format), make(crate::HDR_FORMAT)];

        let palette_capacity: u32 = 256;
        let instance_capacity: u32 = 32;
        let palette_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.skinned.palettes"),
            size: palette_capacity as u64 * PALETTE_ELEM_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.skinned.instances"),
            size: instance_capacity as u64 * INSTANCE_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let skin_bind_group = make_skin_bg(device, &skin_bgl, &palette_buf, &instance_buf);

        Self {
            pipeline,
            skin_bgl,
            skin_bind_group,
            palette_buf,
            palette_capacity,
            instance_buf,
            instance_capacity,
            queued: None,
            meshes: Vec::new(),
            free_slots: Vec::new(),
        }
    }

    /// Upload a bind-pose skinned mesh; persists until freed.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        vertices: &[SkinnedVertex],
        indices: MeshIndices<'_>,
    ) -> SkinnedMeshHandle {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.skinned.vbo"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let (index_buffer, index_count, index_format) = match indices {
            MeshIndices::U16(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.skinned.ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint16,
            ),
            MeshIndices::U32(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.skinned.ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint32,
            ),
        };
        let mesh = SkinnedMesh {
            vertex_buffer,
            index_buffer,
            index_count,
            index_format,
        };
        let id = if let Some(slot) = self.free_slots.pop() {
            self.meshes[slot as usize] = Some(mesh);
            slot
        } else {
            let slot = self.meshes.len() as u32;
            self.meshes.push(Some(mesh));
            slot
        };
        SkinnedMeshHandle(id)
    }

    pub fn free(&mut self, handle: SkinnedMeshHandle) {
        if let Some(slot) = self.meshes.get_mut(handle.0 as usize) {
            if slot.take().is_some() {
                self.free_slots.push(handle.0);
            }
        }
    }

    /// Drop the frame's queued instanced draw (call at frame start).
    pub fn clear(&mut self) {
        self.queued = None;
    }

    /// Upload this frame's per-instance palettes + model transforms and queue **one
    /// instanced draw** of `mesh`. `palettes` is the concatenated bone matrices —
    /// instance `i`'s bone `b` lives at `i*bone_count + b`, so `palettes.len()` must be
    /// `models.len() * bone_count`. Grows the storage buffers (and rebuilds the skin
    /// bind group) when this frame needs more than the current capacity.
    pub fn draw_instanced(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: SkinnedMeshHandle,
        models: &[Mat4],
        palettes: &[Mat4],
        bone_count: u32,
    ) {
        let count = models.len() as u32;
        if count == 0 || bone_count == 0 {
            self.queued = None;
            return;
        }
        debug_assert_eq!(
            palettes.len(),
            models.len() * bone_count as usize,
            "palettes must be models.len() * bone_count matrices"
        );

        let mut rebuild = false;
        if palettes.len() as u32 > self.palette_capacity {
            self.palette_capacity = (palettes.len() as u32).next_power_of_two();
            self.palette_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.skinned.palettes"),
                size: self.palette_capacity as u64 * PALETTE_ELEM_SIZE,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            rebuild = true;
        }
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.skinned.instances"),
                size: self.instance_capacity as u64 * INSTANCE_SIZE,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            rebuild = true;
        }
        if rebuild {
            self.skin_bind_group = make_skin_bg(
                device,
                &self.skin_bgl,
                &self.palette_buf,
                &self.instance_buf,
            );
        }

        queue.write_buffer(&self.palette_buf, 0, bytemuck::cast_slice(palettes));
        let instances: Vec<InstanceRaw> = models
            .iter()
            .enumerate()
            .map(|(i, m)| InstanceRaw {
                model: m.to_cols_array_2d(),
                palette_offset: i as u32 * bone_count,
                bone_count,
                _pad: [0, 0],
            })
            .collect();
        queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&instances));

        self.queued = Some((mesh, count));
    }

    /// Issue the queued instanced draw (group 0 = the renderer's ONE per-frame group,
    /// group 1 = palettes + instances). One `draw_indexed` for all instances. `target`
    /// selects the colour-format variant (sRGB or HDR) for the surface being encoded.
    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frame: &'a FrameBindGroup,
        shadow: &'a ShadowBind,
        target: crate::TargetColor,
    ) {
        let Some((handle, count)) = self.queued else {
            return;
        };
        let Some(mesh) = self.meshes.get(handle.0 as usize).and_then(|s| s.as_ref()) else {
            return;
        };
        pass.set_pipeline(&self.pipeline[target as usize]);
        pass.set_bind_group(0, frame.bind_group(), &[]);
        pass.set_bind_group(1, &self.skin_bind_group, &[]);
        // @group(2) — the shadow bind for this surface (the active source, or the default).
        pass.set_bind_group(2, shadow.active_bind_group(), &[]);
        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
        pass.draw_indexed(0..mesh.index_count, 0, 0..count);
    }
}

fn make_skin_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    palette_buf: &wgpu::Buffer,
    instance_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("flicker.skinned.skin_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: palette_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: instance_buf.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Builds the pipeline (compiles `skinned.wgsl`, validates the layouts incl.
    /// **storage buffers in the vertex stage**), uploads a 1-triangle mesh, and
    /// **executes a 2-instance skinned draw** to an offscreen target — all under a
    /// validation error scope. Skips cleanly with no GPU adapter. This is the headless
    /// proof of the instanced-skinning path (visual correctness is a windowed check).
    #[test]
    fn skinned_pipeline_compiles_and_draws_instanced() {
        let Some((device, queue)) =
            crate::pipeline_mesh::tests::test_device("flicker.skinned_test.device")
        else {
            eprintln!("skinned_pipeline: no GPU adapter — skipping");
            return;
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let frame = FrameBindGroup::new(&device);
        let shadow = ShadowBind::new(&device);
        let mut pipeline = SkinnedMeshPipeline::new(&device, &queue, &frame, &shadow, format);
        frame.set_scene_uniform(&queue, crate::pipeline_mesh::SceneUniform::default());
        frame.set_camera_matrix(&queue, Mat4::IDENTITY);

        // One triangle, all vertices bound to bone 0 at full weight.
        let v = |p: [f32; 3]| SkinnedVertex {
            position: p,
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            joints: [0, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        };
        let verts = [
            v([-0.5, -0.5, 0.0]),
            v([0.5, -0.5, 0.0]),
            v([0.0, 0.5, 0.0]),
        ];
        let mesh = pipeline.upload(&device, &verts, MeshIndices::U32(&[0, 1, 2]));

        // Two instances, one bone each (identity palette), offset in the field.
        let models = [
            Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)),
            Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)),
        ];
        let palettes = [Mat4::IDENTITY, Mat4::IDENTITY];
        pipeline.draw_instanced(&device, &queue, mesh, &models, &palettes, 1);

        // Offscreen colour + depth target, then execute the instanced draw.
        let target = |fmt, label| {
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
                format: fmt,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let color = target(format, "flicker.skinned_test.color");
        let depth = target(DEPTH_FORMAT, "flicker.skinned_test.depth");
        let cview = color.create_view(&Default::default());
        let dview = depth.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.skinned_test.pass"),
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
            pipeline.render(&mut pass, &frame, &shadow, crate::TargetColor::Srgb);
        }
        queue.submit([enc.finish()]);
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "skinned.wgsl / instanced draw failed validation: {err:?}"
        );
    }
}
