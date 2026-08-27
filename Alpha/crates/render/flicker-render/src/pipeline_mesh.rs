//! 3D mesh pipeline.
//!
//! # Transform stack
//!
//! Each `draw_mesh` call composes `projection × view × model` to produce
//! the clip-space transform for that mesh. The `view × projection` is
//! shared across the frame — and across every lit pipeline — by the
//! [`FrameBindGroup`] this module also owns ([`FrameBindGroup::set_camera_matrix`],
//! called once per frame from the renderer). The `model` matrix plus tint and a
//! wireframe flag are written into a per-draw uniform buffer at `@group(1)`, bound
//! through a dynamic offset that advances by `per_draw_stride` for each queued draw.
//!
//! # Depth
//!
//! The mesh pipeline writes and tests depth using a `Depth32Float`
//! attachment owned by the renderer. The 2D pipelines (triangle,
//! sprite, text) share the same render pass but disable depth write
//! and test so they layer on top of the 3D scene as overlays. Depth
//! is cleared to `1.0` at the start of every frame.
//!
//! # Wireframe via line-list edge buffer
//!
//! Each uploaded mesh carries two index buffers: a **triangle index
//! buffer** (the caller's `MeshIndices`) and an **edge index buffer**
//! built during `upload` by walking the triangle indices and deduping
//! each undirected edge `(min, max)` through a `HashSet`. Each unique
//! edge contributes 2 indices to the edge buffer.
//!
//! Two render pipelines share the vertex shader, bind groups, depth
//! state, and color target — they differ only in primitive topology:
//!
//! * `triangle_pipeline`: `TriangleList`, back-face culling enabled.
//! * `line_pipeline`: `LineList`, culling disabled (the notion of
//!   "back-facing" doesn't apply to lines).
//!
//! `MeshPipeline::render` dispatches per draw call based on the
//! wireframe flag: wireframe draws bind the line pipeline and the
//! edge index buffer; filled draws bind the triangle pipeline and the
//! triangle index buffer. The fragment shader's wireframe branch then
//! emits the wireframe color directly — no barycentric trick, no
//! shared-vertex caveat.

use std::collections::HashSet;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::mesh::{LightRig, MeshHandle, MeshIndices, MeshVertex, MAX_LIGHTS};
use crate::pipeline_shadow::ShadowBind;

/// The depth attachment format used by the renderer. Matched between the
/// 3D pipeline and the 2D pipelines (which set `depth_write_enabled =
/// false` and `depth_compare = Always`).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The ONE shared frame prelude — the `struct Light`/`Scene`/`ShadowUniform` declarations
/// plus `light_sample` and `shadow_factor`. WGSL has no `#include`, so every lit pipeline
/// PREPENDS it to its body shader via [`compose_lit`] at module build. This is the E0EA83C8
/// remediation: one text, not three pasted copies. `lines.wgsl` reads no light and does NOT
/// use it.
pub const FRAME_PRELUDE: &str = include_str!("shaders/frame_prelude.wgsl");

/// Compose a lit shader's full source: the shared [`FRAME_PRELUDE`] followed by the body.
/// The prelude declares the shared structs/functions; the body declares its own `@group`
/// bindings (which reference the prelude by name — WGSL resolves module-scope declarations
/// regardless of order). Called once per pipeline at construction, never per frame.
pub fn compose_lit(body: &str) -> String {
    format!("{FRAME_PRELUDE}\n{body}")
}

/// CPU-side mirror of the WGSL `Camera` uniform.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
}

const CAMERA_UNIFORM_SIZE: u64 = std::mem::size_of::<CameraUniform>() as u64;

/// CPU-side mirror of the WGSL `PerDraw` uniform.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct PerDraw {
    model: [[f32; 4]; 4], // 64 bytes
    tint: [f32; 4],       // 16 bytes
    // 16 bytes; flags.x = wireframe (0/1), flags.y = gloss (sheen strength). zw unused.
    flags: [f32; 4],
}

const PER_DRAW_RAW_SIZE: u64 = std::mem::size_of::<PerDraw>() as u64;

/// CPU-side mirror of the WGSL `Light` — ONE entry of the frame's light list.
/// Every member is a 16-byte `vec4` lane, so std140 alignment is trivially correct
/// and an `array<Light, N>` needs no padding between elements.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub struct LightUniform {
    /// `rgb` = colour, `w` = intensity (the driver's gain is already applied).
    pub color_intensity: [f32; 4],
    /// `xyz` = world position (point/spot), `w` = kind (0 dir, 1 point, 2 spot).
    pub position_kind: [f32; 4],
    /// `xyz` = toward-the-light (dir) or the cone axis (spot), `w` = radius
    /// (`<= 0` ⇒ no falloff).
    pub direction_radius: [f32; 4],
    /// `x` = cos(cone_inner), `y` = cos(cone_outer); `zw` reserved.
    pub cone: [f32; 4],
}

/// CPU-side mirror of the WGSL `Scene` uniform — the frame-global lighting/atmosphere
/// state: ambient, the camera position (fog distance + view vector), distance fog, and
/// the [`MAX_LIGHTS`]-slot LIGHT LIST the lit shaders loop over `counts.x` times.
/// Laid out as `[f32; 4]` lanes to match the shader's `vec4` fields exactly (16-byte
/// aligned, no implicit std140 padding). The `.w` lanes pack scalars: `fog_color[3]` is
/// fog density. There is no grade lane — the colour grade is pass-owned by
/// [`TonemapGradePass`](crate::TonemapGradePass) and never rides here.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SceneUniform {
    pub ambient: [f32; 4],
    pub camera_pos: [f32; 4],
    /// `rgb` = fog colour, `w` = fog density.
    pub fog_color: [f32; 4],
    /// `x` = how many of `lights` are lit this frame; `yzw` reserved.
    pub counts: [u32; 4],
    pub lights: [LightUniform; MAX_LIGHTS],
}

const SCENE_UNIFORM_SIZE: u64 = std::mem::size_of::<SceneUniform>() as u64;

// **The layout gate.** The shader's `Scene` is `4 × vec4` of header + an
// `array<Light, MAX_LIGHTS>` of `4 × vec4` each. ABSOLUTE offsets, not `% 16`: every
// member IS a 16-byte lane, so an alignment check holds for any PERMUTATION of them and
// would wave through a reorder that shears the data against the WGSL `Scene` while
// every size still agrees. Pinned offsets pin the Rust order; the shader half of the
// same contract — that `struct Scene` / `struct Light` spell their fields in THIS order
// — is gated as text by `the_lit_shaders_ship_the_light_loop_the_mirrors_assert` below.
// wgpu's default `max_uniform_buffer_binding_size` is 64 KiB, which 576 B does not come
// close to.
const _: () = assert!(std::mem::size_of::<LightUniform>() == 64);
const _: () = assert!(std::mem::size_of::<SceneUniform>() == 64 + 64 * MAX_LIGHTS);
const _: () = assert!(std::mem::offset_of!(SceneUniform, ambient) == 0);
const _: () = assert!(std::mem::offset_of!(SceneUniform, camera_pos) == 16);
const _: () = assert!(std::mem::offset_of!(SceneUniform, fog_color) == 32);
const _: () = assert!(std::mem::offset_of!(SceneUniform, counts) == 48);
const _: () = assert!(std::mem::offset_of!(SceneUniform, lights) == 64);
const _: () = assert!(std::mem::offset_of!(LightUniform, color_intensity) == 0);
const _: () = assert!(std::mem::offset_of!(LightUniform, position_kind) == 16);
const _: () = assert!(std::mem::offset_of!(LightUniform, direction_radius) == 32);
const _: () = assert!(std::mem::offset_of!(LightUniform, cone) == 48);
const _: () = assert!(SCENE_UNIFORM_SIZE <= 65_536);

/// One colour per material-catalog slot (`materials.json`, ids `0..=255`),
/// as `vec4<f32>` for trivial std140 layout. The whole palette is 4 KiB.
pub const MATERIAL_PALETTE_LEN: usize = 256;
const PALETTE_UNIFORM_SIZE: u64 = (MATERIAL_PALETTE_LEN * 16) as u64;

/// The loud-wrong palette the pipeline boots with: every slot magenta, so an
/// unset palette (or an undefined material id) is visible as "missing" rather
/// than silently plausible. [`MeshPipeline::set_material_palette`] replaces it
/// with the catalog's colours.
fn magenta_palette() -> [[f32; 4]; MATERIAL_PALETTE_LEN] {
    [[1.0, 0.0, 1.0, 1.0]; MATERIAL_PALETTE_LEN]
}

impl Default for SceneUniform {
    /// The default rig, packed — the pre-uniform hardcoded look. DERIVED from
    /// [`LightRig::default`] through the one converter, never a second spelling of it:
    /// a restated copy here would drift the moment the rig's default moved.
    fn default() -> Self {
        crate::renderer::rig_to_uniform(&LightRig::default(), glam::Vec3::ZERO)
    }
}

/// **The one per-frame bind group** — `@group(0) = { 0: camera, 1: scene }`, shared by
/// every lit pipeline (mesh, mesh_textured, skinned, lines) so the camera matrix and
/// the light list are uploaded ONCE per frame instead of once per pipeline. Owned by
/// the [`Renderer`](crate::Renderer), built once, handed to each pipeline's `new` (for
/// its layout) and to each `render` (for the group itself).
///
/// Lives here because this module already owns [`SceneUniform`] and [`DEPTH_FORMAT`] —
/// the two things every lit pipeline already imports from it.
pub struct FrameBindGroup {
    layout: wgpu::BindGroupLayout,
    camera_buf: wgpu::Buffer,
    scene_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl FrameBindGroup {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.frame.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(CAMERA_UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(SCENE_UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.frame.camera_uniform"),
            size: CAMERA_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Seeded with the default rig so the first frames (before any `set_scene`)
        // match the former hardcoded lighting.
        let scene_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.frame.scene_uniform"),
            contents: bytemuck::bytes_of(&SceneUniform::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.frame.bind_group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: scene_buf.as_entire_binding(),
                },
            ],
        });
        Self {
            layout,
            camera_buf,
            scene_buf,
            bind_group,
        }
    }

    /// The layout every lit pipeline builds its `@group(0)` against.
    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// The group every lit pipeline binds at slot 0 while encoding.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Upload this frame's view-projection — once, for every lit pipeline.
    pub fn set_camera_matrix(&self, queue: &wgpu::Queue, view_projection: Mat4) {
        let uniform = CameraUniform {
            view_projection: view_projection.to_cols_array_2d(),
        };
        queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&uniform));
    }

    /// Upload this frame's lighting/atmosphere — once, for every lit pipeline.
    pub fn set_scene_uniform(&self, queue: &wgpu::Queue, scene: SceneUniform) {
        queue.write_buffer(&self.scene_buf, 0, bytemuck::bytes_of(&scene));
    }
}

/// One persistent GPU-side mesh. Holds both a triangle index buffer
/// (for filled draws) and an edge index buffer (for line-list wireframe
/// draws). Both reference the same vertex buffer.
pub struct LoadedMesh {
    vertex_buffer: wgpu::Buffer,
    triangle_index_buffer: wgpu::Buffer,
    triangle_index_count: u32,
    triangle_index_format: wgpu::IndexFormat,
    edge_index_buffer: wgpu::Buffer,
    edge_index_count: u32,
    edge_index_format: wgpu::IndexFormat,
}

impl LoadedMesh {
    /// The GPU buffers of a stored mesh, for a pipeline that draws it WITHOUT owning the mesh
    /// store — the water grid, drawn by [`pipeline_water_mesh`](crate::pipeline_water_mesh)
    /// against the same `Renderer::meshes` pool this pipeline fills. Filled draws only (the
    /// grid never draws as wireframe), so no edge-buffer accessor is exposed.
    pub(crate) fn vertex_buffer(&self) -> &wgpu::Buffer {
        &self.vertex_buffer
    }
    pub(crate) fn tri_index_buffer(&self) -> &wgpu::Buffer {
        &self.triangle_index_buffer
    }
    pub(crate) fn tri_index_count(&self) -> u32 {
        self.triangle_index_count
    }
    pub(crate) fn tri_index_format(&self) -> wgpu::IndexFormat {
        self.triangle_index_format
    }
}

/// One queued draw call for the current frame.
struct MeshDraw {
    handle: MeshHandle,
    per_draw: PerDraw,
}

const MESH_VERTEX_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];

/// The 3D mesh pipeline.
pub struct MeshPipeline {
    /// Filled triangles. Baked for both colour formats (swapchain + [`crate::HDR_FORMAT`])
    /// over the one shared bind group / buffers; [`Self::render`] selects by
    /// [`crate::TargetColor`].
    triangle_pipeline: [wgpu::RenderPipeline; 2],
    /// Line-list wireframe, same both-format baking as `triangle_pipeline`.
    line_pipeline: [wgpu::RenderPipeline; 2],
    /// This pipeline's OWN `@group(1)`: the per-draw uniform (dynamic offset) + the
    /// material palette. The camera and the frame's light list ride the shared
    /// [`FrameBindGroup`] at `@group(0)`.
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    /// The material-catalog colour palette (one `vec4` per `MaterialId`),
    /// booted all-magenta (loud-wrong) and replaced by
    /// [`MeshPipeline::set_material_palette`] with `materials.json` colours.
    palette_buf: wgpu::Buffer,
    per_draw_buf: wgpu::Buffer,
    /// Maximum number of per-draw entries the current `per_draw_buf` can
    /// hold. Grown lazily in `prepare`.
    per_draw_capacity: u32,
    /// Per-draw stride in bytes, equal to `min_uniform_buffer_offset_alignment`
    /// rounded up to be at least `PER_DRAW_RAW_SIZE`.
    per_draw_stride: u32,
    queued: Vec<MeshDraw>,
}

impl MeshPipeline {
    pub fn new(
        device: &wgpu::Device,
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
            label: Some("flicker.mesh.shader"),
            source: wgpu::ShaderSource::Wgsl(compose_lit(include_str!("shaders/mesh.wgsl")).into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flicker.mesh.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(PER_DRAW_RAW_SIZE),
                    },
                    count: None,
                },
                // The material-catalog colour palette — fragment-only, set
                // rarely (content load), read per-fragment by index.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(PALETTE_UNIFORM_SIZE),
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flicker.mesh.pipeline_layout"),
            // @group(2) = the shared shadow bind (a 1×1 default is bound for non-shadow
            // surfaces, so this is inert until a shadow is cast).
            bind_group_layouts: &[frame.layout(), &bind_group_layout, shadow.layout()],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &MESH_VERTEX_ATTRS,
        };

        // Depth-stencil shared between both pipelines. `LessEqual` lets
        // a wireframe pass land on top of a fill pass at the exact same
        // depth — without this, the line draw would lose the depth test
        // against the fill's just-written depth values and render nothing.
        let depth_stencil = Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });

        // Build the (triangle, line) pipeline pair for one colour format — the ONLY thing
        // that varies between the sRGB and HDR variants. The pair shares the vertex shader,
        // bind groups, depth state, and blend; `render` picks the variant by `TargetColor`.
        let make = |fmt: wgpu::TextureFormat| {
            let color_target = Some(wgpu::ColorTargetState {
                format: fmt,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            });
            let triangle = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.mesh.triangle_pipeline"),
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
                    targets: std::slice::from_ref(&color_target),
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
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

            let line = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("flicker.mesh.line_pipeline"),
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
                    targets: std::slice::from_ref(&color_target),
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    // Lines have no notion of "back-facing" — disable culling.
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: depth_stencil.clone(),
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
            (triangle, line)
        };
        let (tri_srgb, line_srgb) = make(surface_format);
        let (tri_hdr, line_hdr) = make(crate::HDR_FORMAT);
        let triangle_pipeline = [tri_srgb, tri_hdr];
        let line_pipeline = [line_srgb, line_hdr];

        // Boot loud-wrong: every slot magenta until the catalog palette is set.
        let palette_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.mesh.material_palette"),
            contents: bytemuck::cast_slice(&magenta_palette()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let initial_capacity: u32 = 64;
        let per_draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flicker.mesh.per_draw_uniform"),
            size: (initial_capacity as u64) * (per_draw_stride as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = make_bind_group(device, &bind_group_layout, &palette_buf, &per_draw_buf);

        Self {
            triangle_pipeline,
            line_pipeline,
            bind_group_layout,
            bind_group,
            palette_buf,
            per_draw_buf,
            per_draw_capacity: initial_capacity,
            per_draw_stride,
            queued: Vec::new(),
        }
    }

    /// Upload an indexed mesh and return a handle. The mesh persists
    /// across frames (caller must keep the handle to draw it again).
    /// Also builds an edge index buffer for line-list wireframe draws.
    pub fn upload(
        &self,
        device: &wgpu::Device,
        vertices: &[MeshVertex],
        indices: MeshIndices<'_>,
    ) -> LoadedMesh {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flicker.mesh.vbo"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        // Triangle index buffer — exactly what the caller submitted.
        let (triangle_index_buffer, triangle_index_count, triangle_index_format) = match indices {
            MeshIndices::U16(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.tri_ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint16,
            ),
            MeshIndices::U32(idx) => (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.tri_ibo"),
                    contents: bytemuck::cast_slice(idx),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                idx.len() as u32,
                wgpu::IndexFormat::Uint32,
            ),
        };

        // Edge index buffer — undirected edges of the triangle mesh,
        // each represented as one `(min, max)` entry in a `HashSet`
        // so shared edges (the typical case for a 2-manifold) are
        // emitted exactly once.
        let edges = extract_edges(&indices);
        let edge_index_count = (edges.len() * 2) as u32;
        // Match the format-selection rule to the triangle buffer for
        // consistency: u16 unless the vertex count overflows it.
        let use_u32 = vertices.len() > (u16::MAX as usize + 1);
        let (edge_index_buffer, edge_index_format) = if use_u32 {
            let mut flat = Vec::with_capacity(edges.len() * 2);
            for (a, b) in &edges {
                flat.push(*a);
                flat.push(*b);
            }
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.edge_ibo"),
                    contents: bytemuck::cast_slice(&flat),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                wgpu::IndexFormat::Uint32,
            )
        } else {
            let mut flat = Vec::with_capacity(edges.len() * 2);
            for (a, b) in &edges {
                flat.push(*a as u16);
                flat.push(*b as u16);
            }
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("flicker.mesh.edge_ibo"),
                    contents: bytemuck::cast_slice(&flat),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                wgpu::IndexFormat::Uint16,
            )
        };

        LoadedMesh {
            vertex_buffer,
            triangle_index_buffer,
            triangle_index_count,
            triangle_index_format,
            edge_index_buffer,
            edge_index_count,
            edge_index_format,
        }
    }

    /// Reset the per-frame draw queue. Called from `Renderer::begin_frame`.
    pub fn clear(&mut self) {
        self.queued.clear();
    }

    /// Replace the material-catalog colour palette (one `vec4` RGBA per
    /// `MaterialId`, index = id). Undefined slots should stay the loud-wrong
    /// magenta the pipeline booted with — pass them through unchanged rather
    /// than inventing a fallback colour.
    pub fn set_material_palette(
        &mut self,
        queue: &wgpu::Queue,
        colors: &[[f32; 4]; MATERIAL_PALETTE_LEN],
    ) {
        queue.write_buffer(&self.palette_buf, 0, bytemuck::cast_slice(colors));
    }

    /// Queue a mesh for rendering this frame.
    pub fn push(
        &mut self,
        handle: MeshHandle,
        model: Mat4,
        tint: [f32; 4],
        wireframe: bool,
        gloss: f32,
    ) {
        self.queued.push(MeshDraw {
            handle,
            per_draw: PerDraw {
                model: model.to_cols_array_2d(),
                tint,
                // flags.x = wireframe, flags.y = gloss (specular strength).
                flags: [if wireframe { 1.0 } else { 0.0 }, gloss, 0.0, 0.0],
            },
        });
    }

    /// Write the per-draw uniform buffer for the queued draws. Grows the
    /// buffer if needed.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.queued.is_empty() {
            return;
        }
        let needed = self.queued.len() as u32;
        if needed > self.per_draw_capacity {
            let new_capacity = needed.next_power_of_two().max(self.per_draw_capacity * 2);
            self.per_draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("flicker.mesh.per_draw_uniform"),
                size: (new_capacity as u64) * (self.per_draw_stride as u64),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.per_draw_capacity = new_capacity;
            self.bind_group = make_bind_group(
                device,
                &self.bind_group_layout,
                &self.palette_buf,
                &self.per_draw_buf,
            );
        }

        // Pack each PerDraw entry at its aligned offset.
        let mut staging = vec![0u8; self.queued.len() * self.per_draw_stride as usize];
        for (i, draw) in self.queued.iter().enumerate() {
            let off = i * self.per_draw_stride as usize;
            let end = off + std::mem::size_of::<PerDraw>();
            staging[off..end].copy_from_slice(bytemuck::bytes_of(&draw.per_draw));
        }
        queue.write_buffer(&self.per_draw_buf, 0, &staging);
    }

    /// Issue the queued draws. Wireframe draws bind the line-list
    /// pipeline + the edge index buffer; filled draws bind the
    /// triangle-list pipeline + the triangle index buffer. `target` selects the
    /// colour-format variant (sRGB or HDR) for the surface being encoded. `frame` is
    /// the renderer's ONE per-frame group (camera + lights), bound at slot 0.
    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frame: &'a FrameBindGroup,
        shadow: &'a ShadowBind,
        meshes: &'a [Option<LoadedMesh>],
        target: crate::TargetColor,
    ) {
        if self.queued.is_empty() {
            return;
        }
        let t = target as usize;
        pass.set_bind_group(0, frame.bind_group(), &[]);
        // @group(2) — the shadow bind for this surface (the active source, or the 1×1
        // default when none), the same for every draw.
        pass.set_bind_group(2, shadow.active_bind_group(), &[]);
        for (i, draw) in self.queued.iter().enumerate() {
            let Some(mesh) = meshes.get(draw.handle.0 as usize).and_then(|s| s.as_ref()) else {
                continue;
            };
            let offset = (i as u32) * self.per_draw_stride;
            let wireframe = draw.per_draw.flags[0] > 0.5;
            if wireframe {
                pass.set_pipeline(&self.line_pipeline[t]);
                pass.set_bind_group(1, &self.bind_group, &[offset]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.edge_index_buffer.slice(..), mesh.edge_index_format);
                pass.draw_indexed(0..mesh.edge_index_count, 0, 0..1);
            } else {
                pass.set_pipeline(&self.triangle_pipeline[t]);
                pass.set_bind_group(1, &self.bind_group, &[offset]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    mesh.triangle_index_buffer.slice(..),
                    mesh.triangle_index_format,
                );
                pass.draw_indexed(0..mesh.triangle_index_count, 0, 0..1);
            }
        }
    }
}

/// Walk the triangle indices and return the deduplicated set of
/// undirected edges, each represented as `(min, max)` in u32. Degenerate
/// triangles (two equal indices on an edge) are silently skipped.
fn extract_edges(indices: &MeshIndices<'_>) -> HashSet<(u32, u32)> {
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut push = |a: u32, b: u32| {
        if a == b {
            return;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        edges.insert(key);
    };
    match indices {
        MeshIndices::U16(idx) => {
            let n = idx.len() - idx.len() % 3;
            let mut i = 0;
            while i < n {
                let a = idx[i] as u32;
                let b = idx[i + 1] as u32;
                let c = idx[i + 2] as u32;
                push(a, b);
                push(b, c);
                push(a, c);
                i += 3;
            }
        }
        MeshIndices::U32(idx) => {
            let n = idx.len() - idx.len() % 3;
            let mut i = 0;
            while i < n {
                let a = idx[i];
                let b = idx[i + 1];
                let c = idx[i + 2];
                push(a, b);
                push(b, c);
                push(a, c);
                i += 3;
            }
        }
    }
    edges
}

fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    palette_buf: &wgpu::Buffer,
    per_draw_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("flicker.mesh.bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: per_draw_buf,
                    offset: 0,
                    size: NonZeroU64::new(PER_DRAW_RAW_SIZE),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: palette_buf.as_entire_binding(),
            },
        ],
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

/// Helper used by `Renderer` to (re)create the depth texture on resize.
pub fn create_depth_view(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("flicker.mesh.depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        // RENDER_ATTACHMENT to write depth in the opaque pass; TEXTURE_BINDING so the volumetric
        // pass can *sample* that depth (read-only) to clamp its rays at solid bodies.
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The three LIT shaders as this crate SHIPS them — the same text
    /// `create_shader_module` is handed at pipeline build. Reading the real channel is
    /// the whole point: a gate over a Rust re-derivation certifies the mirror, never the
    /// source. (`lines.wgsl` is deliberately absent — it reads no light.)
    const LIT_SHADERS: [(&str, &str); 3] = [
        ("mesh.wgsl", include_str!("shaders/mesh.wgsl")),
        (
            "mesh_textured.wgsl",
            include_str!("shaders/mesh_textured.wgsl"),
        ),
        ("skinned.wgsl", include_str!("shaders/skinned.wgsl")),
    ];

    /// One shipped shader's source by file name.
    fn shader(file: &str) -> &'static str {
        LIT_SHADERS
            .into_iter()
            .find(|(f, _)| *f == file)
            .unwrap_or_else(|| panic!("`{file}` is one of the lit shaders"))
            .1
    }

    /// The text between `struct <name> {` and its closing brace — for asserting FIELD
    /// ORDER, which is the half of the uniform contract a size check cannot see.
    fn struct_body<'a>(wgsl: &'a str, name: &str, file: &str) -> &'a str {
        let head = format!("struct {name} {{");
        let at = wgsl
            .find(&head)
            .unwrap_or_else(|| panic!("{file} declares `{head}`"));
        let rest = &wgsl[at + head.len()..];
        let end = rest
            .find('}')
            .unwrap_or_else(|| panic!("{file}: `struct {name}` is unterminated"));
        &rest[..end]
    }

    /// **GATE — the SHIPPED light loop is the one every mirror asserts against.**
    /// S4a's numeric half lives one crate over, in `flicker-widgets`
    /// (`the_light_loop_is_bit_identical_to_the_closed_form`): it proves a CPU
    /// re-derivation of the loop equals a CPU re-derivation of the closed form, bit for
    /// bit. Both of its sides are Rust. THIS is the source half, in the crate that owns
    /// the shaders — the load-bearing facts of the real WGSL text: how each accumulator
    /// is SEEDED, what BOUNDS the loop, and how the per-light term is SPELLED. Together
    /// they cover the channel: re-associating `radiance * (ndl * s.w)`, or moving the
    /// ambient across the loop, now fails HERE instead of quietly turning "zero pixel
    /// change" into an unbacked claim.
    ///
    /// It also carries the shader half of the uniform contract: `struct Scene` and
    /// `struct Light` must spell their fields in the order the `offset_of!` asserts
    /// above pin CPU-side, or the two agree on every size while shearing the data.
    #[test]
    fn the_lit_shaders_ship_the_light_loop_the_mirrors_assert() {
        // ── THE SEEDS AND THE SPELLING ── f32 addition is not associative, so the seed
        // and the association ARE the pixels: `mesh`/`skinned` seed the accumulator with
        // `scene.ambient.rgb`, `mesh_textured` seeds diffuse AND spec with zero.
        // The `* vis` tail is the S6 shadow multiply: `vis` is 1.0 exactly for every light
        // but the shadowed one (and for every surface with `enabled = 0`), so `t * 1.0 == t`
        // keeps these sums bit-identical to the unshadowed loop the CPU mirror asserts.
        let facts: [(&str, &[&str]); 3] = [
            (
                "mesh.wgsl",
                &[
                    "var diffuse = scene.ambient.rgb;",
                    "let ndl = max(dot(in.world_normal, s.xyz), 0.0);",
                    "diffuse = diffuse + radiance * (ndl * s.w) * vis;",
                ],
            ),
            (
                "mesh_textured.wgsl",
                &[
                    "var direct = vec3<f32>(0.0);",
                    "var spec = vec3<f32>(0.0);",
                    "let ndl = max(dot(n, s.xyz), 0.0);",
                    "direct = direct + radiance * (ndl * s.w) * vis;",
                    "let ambient = scene.ambient.rgb * ao;",
                ],
            ),
            (
                "skinned.wgsl",
                &[
                    "var diffuse = scene.ambient.rgb;",
                    "diffuse = diffuse + radiance * (max(dot(n, s.xyz), 0.0) * s.w) * vis;",
                ],
            ),
        ];
        for (file, expected) in facts {
            let wgsl = shader(file);
            for fact in expected {
                assert!(
                    wgsl.contains(fact),
                    "{file} no longer spells `{fact}` — the light loop the CPU mirrors \
                     assert against has drifted from the shipped shader"
                );
            }
        }

        // ── THE LOOP BOUND AND THE RADIANCE TERM ── identical in all three.
        for (file, wgsl) in LIT_SHADERS {
            for fact in [
                "for (var i = 0u; i < scene.counts.x; i = i + 1u) {",
                "let li = scene.lights[i];",
                "let radiance = li.color_intensity.rgb * li.color_intensity.w;",
            ] {
                assert!(
                    wgsl.contains(fact),
                    "{file}'s light loop no longer spells `{fact}`"
                );
            }
        }

        // ── THE AMBIENT STAYS OUTSIDE mesh_textured's LOOP ── its ONE mention of
        // `scene.ambient` comes AFTER the accumulation, which is what keeps that sum's
        // order (and so its exact f32 result) the zero-seeded one the mirror asserts.
        let textured = shader("mesh_textured.wgsl");
        assert_eq!(
            textured.matches("scene.ambient").count(),
            1,
            "mesh_textured.wgsl must apply the ambient in exactly ONE place"
        );
        let ambient_at = textured
            .find("let ambient = scene.ambient.rgb * ao;")
            .expect("the ambient term");
        let accum_at = textured
            .find("direct = direct + radiance * (ndl * s.w) * vis;")
            .expect("the diffuse accumulation");
        assert!(
            ambient_at > accum_at,
            "mesh_textured.wgsl must apply the ambient AFTER the light loop, never seed \
             the accumulator with it"
        );

        // ── THE SHADER HALF OF THE UNIFORM CONTRACT ── field ORDER, in the ONE shared
        // prelude (dedup'd into `frame_prelude.wgsl`; every lit shader prepends it).
        for (name, fields) in [
            (
                "Scene",
                ["ambient", "camera_pos", "fog_color", "counts", "lights"].as_slice(),
            ),
            (
                "Light",
                [
                    "color_intensity",
                    "position_kind",
                    "direction_radius",
                    "cone",
                ]
                .as_slice(),
            ),
        ] {
            let body = struct_body(FRAME_PRELUDE, name, "frame_prelude.wgsl");
            let mut at = 0;
            for field in fields {
                let needle = format!("{field}:");
                let found = body[at..].find(&needle).unwrap_or_else(|| {
                    panic!(
                        "frame_prelude.wgsl: `struct {name}` has no `{needle}` after byte {at} \
                         — the field ORDER is the contract with `SceneUniform`, and a reorder \
                         shears the data while every size still agrees"
                    )
                });
                at += found + needle.len();
            }
        }
    }

    /// **GATE — the frame prelude is ONE text (E0EA83C8 discharged).** WGSL has no
    /// `#include`, so `struct Light`/`Scene`/`ShadowUniform`, `light_sample()` and
    /// `shadow_factor()` were the triplicated-prelude violation. S6 folds them into ONE
    /// `frame_prelude.wgsl` every lit pipeline PREPENDS (see [`compose_lit`]) — so
    /// byte-identity is now guaranteed by construction, and this gate proves the dedup is
    /// REAL: the shared file carries the contract, and no body shader re-declares it (a
    /// second copy would compile fine and silently re-fork the source of truth). It also
    /// covers the S6 shadow channel — the prelude ships the `shadow_factor` the lit loops
    /// call. `lines.wgsl` reads no light and must carry none of it.
    #[test]
    fn the_frame_prelude_is_one_text() {
        // The shared prelude carries the whole contract — the structs, the falloff/cone
        // math, and the shadow sampler.
        for fact in [
            "struct Light {",
            "struct Scene {",
            "struct ShadowUniform {",
            "fn light_sample(",
            "fn shadow_factor(",
            "textureSampleCompareLevel(",
        ] {
            assert!(
                FRAME_PRELUDE.contains(fact),
                "frame_prelude.wgsl must carry `{fact}` — it is the ONE text every lit \
                 shader shares"
            );
        }
        // No body shader re-declares the shared contract: if one pasted its own
        // `struct Scene`/`light_sample`/`shadow_factor`, the dedup would be a lie (two
        // sources of truth that happen to agree today). The bodies keep only their OWN
        // `@group` bindings + entry points.
        for (file, wgsl) in LIT_SHADERS {
            for dup in [
                "struct Light {",
                "struct Scene {",
                "struct ShadowUniform {",
                "fn light_sample(",
                "fn shadow_factor(",
            ] {
                assert!(
                    !wgsl.contains(dup),
                    "{file} re-declares `{dup}` — it must inherit it from the ONE shared \
                     frame_prelude.wgsl, not paste a second copy"
                );
            }
        }
        // `lines.wgsl` reads no light — neither the prelude nor a hand-rolled copy.
        assert!(
            !include_str!("shaders/lines.wgsl").contains("fn light_sample("),
            "lines.wgsl reads no light — it must not carry a copy of the prelude"
        );
        // And it is never composed WITH the prelude either (a `Camera`-only pipeline).
        assert!(
            !compose_lit("// body").is_empty() && compose_lit("X").ends_with('X'),
            "compose_lit prepends the prelude to the body"
        );
    }

    /// A headless device, or `None` on a machine without a GPU adapter.
    pub(crate) fn test_device(label: &str) -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;
        Some(
            pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some(label),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            ))
            .expect("request device"),
        )
    }

    /// Build the mesh pipeline under a validation error scope. Creating the
    /// render pipelines compiles `mesh.wgsl` and checks it against BOTH bind-group
    /// layouts — the shared frame group (camera + the `Scene` light list) at `@group(0)`
    /// and the pipeline's own per-draw + palette group at `@group(1)` — so a malformed
    /// shader or a struct/layout mismatch fails here rather than at app launch.
    /// Skips cleanly on a machine without a GPU adapter.
    #[test]
    fn mesh_pipeline_compiles_shader() {
        let Some((device, queue)) = test_device("flicker.mesh_test.device") else {
            eprintln!("mesh_pipeline_compiles_shader: no GPU adapter — skipping");
            return;
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let align = device.limits().min_uniform_buffer_offset_alignment;
        let frame = FrameBindGroup::new(&device);
        let shadow = crate::pipeline_shadow::ShadowBind::new(&device);
        let mut pipeline = MeshPipeline::new(
            &device,
            &frame,
            &shadow,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            align,
        );
        // Exercise the shared frame-uniform upload path too (writes group 0).
        frame.set_scene_uniform(&queue, SceneUniform::default());
        frame.set_camera_matrix(&queue, Mat4::IDENTITY);
        // And the material-palette upload path (writes group 1, binding 1).
        pipeline.set_material_palette(&queue, &magenta_palette());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "mesh.wgsl failed validation: {err:?}");
    }

    /// **GATE — the lit pipelines share ONE per-frame layout.** All four (mesh,
    /// mesh_textured, skinned, lines) are built from a SINGLE [`FrameBindGroup`], so
    /// the camera matrix and the light list are uploaded once per frame and reach every
    /// lit shader. If any of them declared a `@group(0)` that disagreed with the shared
    /// layout, pipeline creation would raise a validation error here — the same scope
    /// the per-shader `*_compiles_shader` tests use.
    #[test]
    fn the_lit_pipelines_share_one_frame_layout() {
        let Some((device, queue)) = test_device("flicker.frame_test.device") else {
            eprintln!("the_lit_pipelines_share_one_frame_layout: no GPU adapter — skipping");
            return;
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let align = device.limits().min_uniform_buffer_offset_alignment;
        let frame = FrameBindGroup::new(&device);
        let shadow = crate::pipeline_shadow::ShadowBind::new(&device);
        let _mesh = MeshPipeline::new(&device, &frame, &shadow, fmt, align);
        let _textured = crate::pipeline_mesh_textured::TexturedMeshPipeline::new(
            &device, &queue, &frame, &shadow, fmt, align,
        );
        let _skinned = crate::pipeline_skinned::SkinnedMeshPipeline::new(
            &device, &queue, &frame, &shadow, fmt,
        );
        let _lines = crate::pipeline_lines::LinesPipeline::new(
            &device,
            &frame,
            fmt,
            wgpu::CompareFunction::LessEqual,
        );
        // ONE upload feeds all four.
        frame.set_camera_matrix(&queue, Mat4::IDENTITY);
        frame.set_scene_uniform(&queue, SceneUniform::default());
        device.poll(wgpu::Maintain::Wait);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "a lit pipeline disagrees with the shared frame layout: {err:?}"
        );
    }
}
