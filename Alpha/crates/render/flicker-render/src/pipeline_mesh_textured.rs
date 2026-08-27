//! Textured 3D mesh pipeline — an albedo-textured, UV-mapped, PBR-lit sibling of the
//! flat [`MeshPipeline`](crate::pipeline_mesh).
//!
//! This is **additive**: the existing `MeshVertex` / `draw_mesh` path (voxel-cluster,
//! mesh-smoke, flicker-world) is untouched. It shares the flat pipeline's depth
//! attachment, camera/scene uniform *shape*, and lighting model, and adds a UV +
//! tangent vertex attribute plus a per-draw albedo texture (bind group 1) and an
//! optional PBR map set (normal / roughness / metalness / AO / emit, bindings 1–5).
//! Designed for reuse — a skinned character today, voxel-cluster surface textures later.
//!
//! **PBR maps.** The pipeline is one pipeline with **default 1×1 textures** for any
//! map a draw omits: a flat normal `(128,128,255)`, white roughness/AO, black
//! metalness, black emit.
//! So an albedo-only draw (the katana) reads as a matte dielectric (flat normal, rough,
//! non-metal, unoccluded), while a full-map draw (the character) gets surface relief +
//! a metal/rough specular response + AO. The albedo is sampled sRGB; the map textures
//! are **linear** (`Renderer::load_texture_linear`).
//!
//! Bind groups:
//! * group 0 — the renderer's ONE per-frame group: camera(@0) + the `Scene` light list(@1),
//!   shared with the flat mesh, skinned and lines pipelines
//!   ([`FrameBindGroup`](crate::pipeline_mesh::FrameBindGroup));
//! * group 1 — per-draw(@0, dynamic offset);
//! * group 2 — the combined material: albedo / normal / roughness / metalness / AO / emit
//!   (@0–@5) + one shared sampler(@6). The single-texture bind group
//!   `Renderer::load_texture[_linear]` builds against `texture_bind_group_layout` is kept
//!   for API compatibility and is not bound by this pipeline.
//!
//! Storage (uploaded meshes + free-list) lives inside the pipeline, so the `Renderer`
//! needs no new fields for it.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};
use wgpu::util::DeviceExt;

use crate::mesh::MeshIndices;
use crate::pipeline_mesh::{compose_lit, FrameBindGroup, DEPTH_FORMAT};
use crate::pipeline_shadow::ShadowBind;
use crate::texture::{LoadedTexture, TextureHandle};

/// Vertex for the textured mesh pipeline: position + normal + UV + tangent. Deformed
/// positions/normals (e.g. CPU-skinned) are re-uploaded; UVs are static. The tangent
/// (`xyz` + handedness `w`) builds the TBN for tangent-space normal mapping — the
/// consumer computes a per-triangle tangent from positions + UVs (the mesh is
/// non-deduplicated, so all three corners of a triangle share the same tangent).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct TexturedVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
}

/// Build a [`TexturedVertex`] list for a contiguous, **non-deduplicated** triangle range
/// (each 3 sequential vertices form one triangle — the converter emits geometry that
/// way). Computes a per-triangle tangent from the 3 positions + UVs (standard
/// `dP/dUV` solve) and assigns it, orthonormalized against each corner's normal, to all
/// three corners — no cross-vertex averaging needed. `w` carries the handedness sign so
/// the shader can reconstruct the bitangent. Positions/normals come from `pos`/`nrm`
/// (skinned or bind geometry); UVs from `uv`. All are indexed by absolute vertex index `j`.
///
/// Lives here, beside the vertex type it builds, because every textured-mesh caller needs
/// the same tangent basis: the paperdoll's body/prop/garment uploads and the asset-pipeline
/// editor's fit preview. An INDEXED mesh must be expanded to a flat triangle list first —
/// the consecutive-triple assumption is what lets one tangent serve all three corners.
pub fn build_textured_verts(
    range: std::ops::Range<usize>,
    pos: impl Fn(usize) -> [f32; 3],
    nrm: impl Fn(usize) -> [f32; 3],
    uv: impl Fn(usize) -> [f32; 2],
) -> Vec<TexturedVertex> {
    let count = range.len();
    let mut out: Vec<TexturedVertex> = range
        .map(|j| TexturedVertex {
            position: pos(j),
            normal: nrm(j),
            uv: uv(j),
            // Placeholder; overwritten per-triangle below.
            tangent: [1.0, 0.0, 0.0, 1.0],
        })
        .collect();

    // Each consecutive triple is one triangle (local indices 3k, 3k+1, 3k+2).
    let tris = count / 3;
    for tk in 0..tris {
        let i0 = tk * 3;
        let i1 = i0 + 1;
        let i2 = i0 + 2;
        let p0 = Vec3::from(out[i0].position);
        let p1 = Vec3::from(out[i1].position);
        let p2 = Vec3::from(out[i2].position);
        let uv0 = Vec2::from(out[i0].uv);
        let uv1 = Vec2::from(out[i1].uv);
        let uv2 = Vec2::from(out[i2].uv);

        let e1 = p1 - p0;
        let e2 = p2 - p0;
        let d1 = uv1 - uv0;
        let d2 = uv2 - uv0;
        let det = d1.x * d2.y - d2.x * d1.y;

        // Tangent = normalize(dP/dU). Degenerate UVs (det≈0) fall back to an arbitrary
        // basis so the TBN stays finite (the shader re-orthonormalizes anyway).
        let (tangent, sign) = if det.abs() > 1e-8 {
            let r = 1.0 / det;
            let t = (e1 * d2.y - e2 * d1.y) * r;
            let bt = (e2 * d1.x - e1 * d2.x) * r;
            // Handedness: +1 if the geometric bitangent agrees with N×T, else -1.
            let n = (Vec3::from(out[i0].normal)
                + Vec3::from(out[i1].normal)
                + Vec3::from(out[i2].normal))
            .normalize_or_zero();
            let sign = if n.cross(t).dot(bt) < 0.0 { -1.0 } else { 1.0 };
            let t = t.normalize_or_zero();
            let t = if t.length_squared() < 1e-12 {
                Vec3::X
            } else {
                t
            };
            (t, sign)
        } else {
            (Vec3::X, 1.0)
        };
        for li in [i0, i1, i2] {
            out[li].tangent = [tangent.x, tangent.y, tangent.z, sign];
        }
    }
    out
}

/// Optional PBR map handles for one textured-mesh draw. Any `None` slot samples the
/// pipeline's default 1×1 texture (flat normal / rough=1 / metal=0 / ao=1), so an
/// albedo-only caller (the katana) still draws correctly.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PbrMaps {
    pub normal: Option<TextureHandle>,
    pub roughness: Option<TextureHandle>,
    pub metalness: Option<TextureHandle>,
    pub ao: Option<TextureHandle>,
    /// Self-illumination — the content standard's `Emit` map, sRGB COLOUR data.
    /// `None` ⇒ the 1×1 black default, i.e. the surface emits nothing. A colour
    /// rather than a scalar because a glow has its own hue independent of the
    /// albedo under it (a blue rune cut into dark iron).
    pub emit: Option<TextureHandle>,
}

/// Opaque handle to a mesh uploaded to the textured pipeline. Distinct from
/// [`MeshHandle`](crate::MeshHandle) so the two stores never cross.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TexturedMeshHandle(pub(crate) u32);

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PerDraw {
    model: [[f32; 4]; 4],
    tint: [f32; 4],
    // flags.y = gloss (sheen strength), flags.z = soft-alpha blend mode. xw unused.
    flags: [f32; 4],
}
const PER_DRAW_RAW_SIZE: u64 = std::mem::size_of::<PerDraw>() as u64;

/// One uploaded textured mesh (persists across frames).
struct TexturedMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    index_format: wgpu::IndexFormat,
}

/// One queued draw for the current frame.
struct Draw {
    handle: TexturedMeshHandle,
    texture: TextureHandle,
    maps: PbrMaps,
    per_draw: PerDraw,
}

const VERTEX_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4];

pub struct TexturedMeshPipeline {
    /// Baked for both colour formats (swapchain + [`crate::HDR_FORMAT`]) over the one
    /// shared uniform / material bind groups; [`Self::render`] selects by
    /// [`crate::TargetColor`].
    pipeline: [wgpu::RenderPipeline; 2],
    /// This pipeline's OWN `@group(1)`: the per-draw uniform (dynamic offset). The
    /// camera and the frame's light list ride the shared [`FrameBindGroup`] at group 0.
    uniform_bgl: wgpu::BindGroupLayout,
    uniform_bind_group: wgpu::BindGroup,
    /// The **combined material** bind-group layout: albedo + normal + roughness +
    /// metalness + AO (bindings 0–4) + one shared sampler (binding 5). One group keeps
    /// the pipeline within the default `max_bind_groups` limit of 4 (0 uniforms, 1
    /// material). Exposed so `Renderer::load_texture[_linear]` can still build the legacy
    /// single-texture `mesh_bind_group` (used elsewhere) against a *separate* layout —
    /// this combined group is built per-draw in [`Self::prepare`] from texture views.
    material_bgl: wgpu::BindGroupLayout,
    /// The single-texture layout `Renderer::load_texture` builds `mesh_bind_group`
    /// against (kept for API compatibility; not bound by this pipeline).
    pub(crate) texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Linear filtering (unlike the sprite pipeline's nearest), for smooth albedo.
    pub(crate) sampler: wgpu::Sampler,
    /// Default 1×1 map texture views, sampled when a draw omits a map (flat normal /
    /// white roughness / black metalness / white AO). Owned here so a caller need not
    /// supply every map. Kept alive for the frame's combined bind groups.
    default_normal_view: wgpu::TextureView,
    default_white_view: wgpu::TextureView,
    default_black_view: wgpu::TextureView,
    /// 1×1 default textures kept alive (their views are referenced above).
    _default_textures: [wgpu::Texture; 3],
    /// Per-frame combined material bind groups, one per queued draw. Rebuilt each frame
    /// in `prepare` (they reference per-draw texture views), consumed in `render`.
    frame_material_bgs: Vec<wgpu::BindGroup>,
    per_draw_buf: wgpu::Buffer,
    per_draw_capacity: u32,
    per_draw_stride: u32,
    queued: Vec<Draw>,
    meshes: Vec<Option<TexturedMesh>>,
    free_slots: Vec<u32>,
}

impl TexturedMeshPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &FrameBindGroup,
        shadow: &ShadowBind,
        surface_format: wgpu::TextureFormat,
        min_uniform_offset_alignment: u32,
    ) -> Self {
        let per_draw_stride = round_up_to_alignment(
            PER_DRAW_RAW_SIZE as u32,
            min_uniform_offset_alignment.max(1),
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flicker.mesh_textured.shader"),
            source: wgpu::ShaderSource::Wgsl(
                compose_lit(include_str!("shaders/mesh_textured.wgsl")).into(),
            ),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.mesh_textured.uniform_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(PER_DRAW_RAW_SIZE),
                },
                count: None,
            }],
        });

        // Single-texture layout — the shape `Renderer::load_texture[_linear]` builds each
        // texture's `mesh_bind_group` against. This pipeline no longer binds those groups
        // (it uses the combined material group below), but the layout is kept public for
        // API/back-compat and so a loaded texture's view is available for the combined group.
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("flicker.mesh_textured.texture_bgl"),
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

        // Combined material layout: 6 textures (albedo, normal, roughness, metalness, AO,
        // emit) at bindings 0–5 + one shared sampler at binding 6. Packing the whole
        // material into ONE bind group keeps the pipeline within the default
        // `max_bind_groups` limit of 4 (group 0 = frame, 1 = per-draw, 2 = material).
        let material_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.mesh_textured.material_bgl"),
            entries: &[
                material_entry(0), // albedo
                material_entry(1), // normal
                material_entry(2), // roughness
                material_entry(3), // metalness
                material_entry(4), // ao
                material_entry(5), // emit
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flicker.mesh_textured.sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Default 1×1 map textures for draws that omit a map. Normal is stored LINEAR
        // (flat normal = (0.5,0.5,1) → (128,128,255)); the scalar maps are single-channel
        // constants replicated across RGBA.
        let (default_normal_tex, default_normal_view) = make_default_texture(
            device,
            queue,
            [128, 128, 255, 255],
            "flicker.mesh_textured.default_normal",
        );
        let (default_white_tex, default_white_view) = make_default_texture(
            device,
            queue,
            [255, 255, 255, 255],
            "flicker.mesh_textured.default_white",
        );
        let (default_black_tex, default_black_view) = make_default_texture(
            device,
            queue,
            [0, 0, 0, 255],
            "flicker.mesh_textured.default_black",
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.mesh_textured.pipeline_layout"),
            // @group(3) = the shared shadow bind (group 2 is the material set); a 1×1
            // default is bound for non-shadow surfaces, so it is inert until a shadow casts.
            bind_group_layouts: &[frame.layout(), &uniform_bgl, &material_bgl, shadow.layout()],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TexturedVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRS,
        };

        // Bake for both colour formats over the one shared layout; only the colour-target
        // format differs. `render` picks the variant by `TargetColor`.
        let make = |fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.mesh_textured.pipeline"),
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

        let initial_capacity: u32 = 16;
        let per_draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.mesh_textured.per_draw_uniform"),
            size: (initial_capacity as u64) * (per_draw_stride as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group = make_uniform_bind_group(device, &uniform_bgl, &per_draw_buf);

        Self {
            pipeline,
            uniform_bgl,
            uniform_bind_group,
            material_bgl,
            texture_bind_group_layout,
            sampler,
            default_normal_view,
            default_white_view,
            default_black_view,
            _default_textures: [default_normal_tex, default_white_tex, default_black_tex],
            frame_material_bgs: Vec::new(),
            per_draw_buf,
            per_draw_capacity: initial_capacity,
            per_draw_stride,
            queued: Vec::new(),
            meshes: Vec::new(),
            free_slots: Vec::new(),
        }
    }

    /// Upload an indexed textured mesh; persists until freed.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        vertices: &[TexturedVertex],
        indices: MeshIndices<'_>,
    ) -> TexturedMeshHandle {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.mesh_textured.vbo"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let (index_buffer, index_count, index_format) = match indices {
            MeshIndices::U16(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh_textured.ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint16,
            ),
            MeshIndices::U32(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh_textured.ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint32,
            ),
        };
        let mesh = TexturedMesh {
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
        TexturedMeshHandle(id)
    }

    /// Free a previously uploaded mesh, returning its slot to the pool.
    pub fn free(&mut self, handle: TexturedMeshHandle) {
        if let Some(slot) = self.meshes.get_mut(handle.0 as usize) {
            if slot.take().is_some() {
                self.free_slots.push(handle.0);
            }
        }
    }

    pub fn clear(&mut self) {
        self.queued.clear();
    }

    /// Queue a textured mesh for this frame, sampling `texture` as albedo with the
    /// given PBR `maps` (each `None` slot uses the pipeline default). `push` (albedo
    /// only) forwards here with empty maps.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        handle: TexturedMeshHandle,
        texture: TextureHandle,
        maps: PbrMaps,
        model: Mat4,
        tint: [f32; 4],
        gloss: f32,
        soft: bool,
    ) {
        self.queued.push(Draw {
            handle,
            texture,
            maps,
            per_draw: PerDraw {
                model: model.to_cols_array_2d(),
                tint,
                // flags.z = soft-alpha blend mode (clouds / ground decals; default cutout).
                flags: [0.0, gloss, if soft { 1.0 } else { 0.0 }, 0.0],
            },
        });
    }

    /// Upload the per-draw uniforms AND build this frame's combined material bind groups
    /// (one per queued draw) from the renderer's texture store — resolving each draw's
    /// albedo + PBR maps to their views, defaulting the omitted maps to the pipeline's
    /// 1×1 constants. `render` consumes these by index.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &[Option<LoadedTexture>],
    ) {
        self.frame_material_bgs.clear();
        if self.queued.is_empty() {
            return;
        }
        let needed = self.queued.len() as u32;
        if needed > self.per_draw_capacity {
            let new_capacity = needed.next_power_of_two().max(self.per_draw_capacity * 2);
            self.per_draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.mesh_textured.per_draw_uniform"),
                size: (new_capacity as u64) * (self.per_draw_stride as u64),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.per_draw_capacity = new_capacity;
            self.uniform_bind_group =
                make_uniform_bind_group(device, &self.uniform_bgl, &self.per_draw_buf);
        }
        let mut staging = vec![0u8; self.queued.len() * self.per_draw_stride as usize];
        for (i, draw) in self.queued.iter().enumerate() {
            let off = i * self.per_draw_stride as usize;
            let end = off + std::mem::size_of::<PerDraw>();
            staging[off..end].copy_from_slice(bytemuck::bytes_of(&draw.per_draw));
        }
        queue.write_buffer(&self.per_draw_buf, 0, &staging);

        // Build one combined material bind group per draw. A missing albedo (bad handle)
        // yields an empty bind group so `render` can skip that draw.
        self.frame_material_bgs.reserve(self.queued.len());
        for draw in &self.queued {
            let Some(albedo_view) = view_for(textures, draw.texture) else {
                // Push a placeholder default group; render skips draws whose mesh/tex is
                // gone, but we must keep index alignment, so bind a fully-default group.
                self.frame_material_bgs.push(self.make_material_bg(
                    device,
                    &self.default_white_view,
                    &self.default_normal_view,
                    &self.default_white_view,
                    &self.default_black_view,
                    &self.default_white_view,
                    &self.default_black_view,
                ));
                continue;
            };
            let normal_view = draw
                .maps
                .normal
                .and_then(|h| view_for(textures, h))
                .unwrap_or(&self.default_normal_view);
            let rough_view = draw
                .maps
                .roughness
                .and_then(|h| view_for(textures, h))
                .unwrap_or(&self.default_white_view);
            let metal_view = draw
                .maps
                .metalness
                .and_then(|h| view_for(textures, h))
                .unwrap_or(&self.default_black_view);
            let ao_view = draw
                .maps
                .ao
                .and_then(|h| view_for(textures, h))
                .unwrap_or(&self.default_white_view);
            // Black default: a draw that names no emit map glows nowhere.
            let emit_view = draw
                .maps
                .emit
                .and_then(|h| view_for(textures, h))
                .unwrap_or(&self.default_black_view);
            let bg = self.make_material_bg(
                device,
                albedo_view,
                normal_view,
                rough_view,
                metal_view,
                ao_view,
                emit_view,
            );
            self.frame_material_bgs.push(bg);
        }
    }

    /// Build one combined material bind group (6 texture views + the shared sampler).
    #[allow(clippy::too_many_arguments)]
    fn make_material_bg(
        &self,
        device: &wgpu::Device,
        albedo: &wgpu::TextureView,
        normal: &wgpu::TextureView,
        roughness: &wgpu::TextureView,
        metalness: &wgpu::TextureView,
        ao: &wgpu::TextureView,
        emit: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.mesh_textured.material_bind_group"),
            layout: &self.material_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(albedo),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(normal),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(roughness),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(metalness),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(ao),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(emit),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Issue the queued draws. Group 0 = the renderer's ONE per-frame group (camera +
    /// lights), group 1 = per-draw (dynamic offset), group 2 = this draw's combined
    /// material bind group (built in `prepare`). `textures` is only needed to skip draws
    /// whose mesh handle is gone.
    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frame: &'a FrameBindGroup,
        shadow: &'a ShadowBind,
        _textures: &'a [Option<LoadedTexture>],
        target: crate::TargetColor,
    ) {
        if self.queued.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline[target as usize]);
        pass.set_bind_group(0, frame.bind_group(), &[]);
        // @group(3) — the shadow bind for this surface (group 2 is per-draw material).
        pass.set_bind_group(3, shadow.active_bind_group(), &[]);
        for (i, draw) in self.queued.iter().enumerate() {
            let Some(mesh) = self
                .meshes
                .get(draw.handle.0 as usize)
                .and_then(|s| s.as_ref())
            else {
                continue;
            };
            let Some(material_bg) = self.frame_material_bgs.get(i) else {
                continue;
            };
            let offset = (i as u32) * self.per_draw_stride;
            pass.set_bind_group(1, &self.uniform_bind_group, &[offset]);
            pass.set_bind_group(2, material_bg, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }
}

/// Look up a texture's view in the renderer's store.
fn view_for(
    textures: &[Option<LoadedTexture>],
    handle: TextureHandle,
) -> Option<&wgpu::TextureView> {
    textures
        .get(handle.0 as usize)
        .and_then(|t| t.as_ref())
        .map(|t| &t.view)
}

/// Build a 1×1 solid-colour LINEAR texture and return `(texture, view)`. Used for the
/// pipeline's default PBR maps (sampled when a draw omits a map). Linear (non-sRGB) so
/// the exact byte values are read — a flat normal must decode to (0.5,0.5,1) → world
/// normal (0,0,1), not be gamma-shifted.
fn make_default_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn make_uniform_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    per_draw_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("flicker.mesh_textured.uniform_bind_group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: per_draw_buf,
                offset: 0,
                size: NonZeroU64::new(PER_DRAW_RAW_SIZE),
            }),
        }],
    })
}

fn round_up_to_alignment(value: u32, alignment: u32) -> u32 {
    let rem = value % alignment;
    if rem == 0 {
        value
    } else {
        value + alignment - rem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Building the pipeline compiles `mesh_textured.wgsl` and validates it against
    /// the bind-group layouts. Skips cleanly with no GPU adapter.
    #[test]
    fn textured_mesh_pipeline_compiles_shader() {
        let Some((device, queue)) =
            crate::pipeline_mesh::tests::test_device("flicker.mesh_textured_test.device")
        else {
            eprintln!("textured_mesh_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let align = device.limits().min_uniform_buffer_offset_alignment;
        let frame = FrameBindGroup::new(&device);
        let shadow = ShadowBind::new(&device);
        let _pipeline = TexturedMeshPipeline::new(
            &device,
            &queue,
            &frame,
            &shadow,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            align,
        );
        frame.set_scene_uniform(&queue, crate::pipeline_mesh::SceneUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "mesh_textured.wgsl failed validation: {err:?}"
        );
    }
}
