//! Top-level `Renderer` — owns the wgpu device/surface and all pipelines.
//!
//! Lifecycle each frame:
//! 1. The driver (typically `flicker-app`) calls [`Renderer::begin_frame`] to
//!    reset per-frame draw queues. Uploaded mesh storage persists across
//!    frames; only the per-frame mesh draw queue clears.
//! 2. User code calls `draw_triangle` / `draw_sprite` / `draw_text` /
//!    `draw_mesh` any number of times. The 3D-camera state is set via
//!    [`Renderer::set_camera`] (typically once per frame).
//! 3. The driver calls [`Renderer::end_frame`] which uploads vertex/text/
//!    per-draw data, encodes a single render pass with a `Depth32Float`
//!    attachment, and presents. 3D meshes render first so 2D primitives
//!    layer on top.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use glam::{Mat4, Vec2};
use winit::dpi::PhysicalSize;
use winit::window::{Fullscreen, Window};

use glam::Vec3;

use crate::mesh::{Camera, LightRig, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex};
use crate::pipeline_billboard::BillboardPipeline;
use crate::pipeline_bloom::{BloomPipeline, BloomUniform};
use crate::pipeline_ground_fog::{GroundFog, GroundFogPipeline, GroundFogUniform};
use crate::pipeline_lines::LinesPipeline;
use crate::pipeline_mesh::{
    create_depth_view, FrameBindGroup, LightUniform, LoadedMesh, MeshPipeline, SceneUniform,
};
use crate::pipeline_mesh_textured::{
    PbrMaps, TexturedMeshHandle, TexturedMeshPipeline, TexturedVertex,
};
use crate::pipeline_shadow::{ShadowBind, ShadowUniform};
use crate::pipeline_skinned::{SkinnedMeshHandle, SkinnedMeshPipeline, SkinnedVertex};
use crate::pipeline_sky::{SkyPipeline, SkyUniform};
use crate::pipeline_sprite::SpritePipeline;
use crate::pipeline_text::TextPipeline;
use crate::pipeline_tonemap::{GradeUniform, TonemapGradePipeline};
use crate::pipeline_triangle::TrianglePipeline;
use crate::pipeline_ui::UiPipeline;
use crate::pipeline_volumetric::{VolumetricDisk, VolumetricDiskUniform, VolumetricPipeline};
use crate::pipeline_water_mesh::{
    water_grid, Water, WaterMeshPipeline, WaterMeshUniform, WATER_GRID_N,
};
use crate::texture::{LoadedTexture, TextureHandle};
use crate::{AttachmentFormat, DepthPass, Rate, TargetColor};

/// The whole texture, as a [`Renderer::draw_sprite_uv`] source rect: `[u0, v0, u1, v1]`
/// in normalized texture space, origin top-left. This is what [`Renderer::draw_sprite`]
/// passes, and the identity an atlas lookup falls back to when a name resolves to no
/// sub-rect.
pub const FULL_TEXTURE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// Opaque handle to an offscreen render target created by
/// [`Renderer::create_render_target`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderTargetHandle(pub(crate) u32);

/// An offscreen render target: a colour texture (also registered in the texture store as a
/// sampleable [`TextureHandle`]) plus its own depth buffer. [`Renderer::render_to_texture`]
/// draws a self-contained sub-scene into it; the resulting colour texture is then sampled
/// through the normal sprite / billboard / mesh paths.
struct RenderTarget {
    /// The colour attachment, sampleable via the texture store.
    color: TextureHandle,
    depth_view: wgpu::TextureView,
    #[allow(dead_code)]
    depth_texture: wgpu::Texture,
    /// Identity of THIS depth texture for the depth-sampling passes' bind-group caches:
    /// unique per allocation (a resize makes a new one), so a cached bind group can never
    /// outlive the texture it samples.
    depth_id: u64,
    /// The HDR intermediate this target renders into when its recipe carries a
    /// `tonemap_grade` pass, allocated lazily on the first HDR frame and reused after
    /// (never per-frame). `None` = no HDR frame has been encoded for it yet — the
    /// byte-identical non-HDR path, which is every shipped surface in S3a.
    hdr: Option<HdrColor>,
    /// Pixel size (drives the sub-scene's aspect + text layout).
    size: Vec2,
    /// Whether a sub-scene has ever been rendered into this target. A never-drawn target
    /// ALWAYS renders once whatever its rate (or it would composite garbage); a resize
    /// makes a fresh target, so it clears back to `false`. This is the flag the three
    /// hand-rolled poster caches used to carry per surface — now owned once, here.
    drawn: bool,
    /// The renderer's [`Renderer::frame_clock`] value at this target's last render — the
    /// per-surface clock an `hz` surface measures its period against
    /// (see [`Renderer::surface_should_render`]).
    last_render: f64,
}

/// One HDR colour attachment (rgba16f): the texture kept alive, its render/sample view, and
/// a unique `id` for the tonemap pass's per-attachment bind cache (renewed on resize, like
/// [`RenderTarget::depth_id`]). Owned by the surface it belongs to — the window
/// ([`Renderer::hdr_color`]) or an offscreen [`RenderTarget`].
struct HdrColor {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    id: u64,
}

/// The two half-res ping-pong scratch targets ([`crate::HDR_FORMAT`]) the bloom pass blurs
/// through: bright writes **a**, blur H `a`->**b**, blur V `b`->**a**, composite adds **a**
/// back into the surface HDR. Renderer-owned scratch (like [`HdrColor`], never a scene-created
/// target), (re)allocated only when the half-res size changes — so a steady frame allocates
/// nothing — and dropped on resize / target free alongside the HDR attachment.
struct BloomScratch {
    #[allow(dead_code)]
    a_texture: wgpu::Texture,
    a_view: wgpu::TextureView,
    #[allow(dead_code)]
    b_texture: wgpu::Texture,
    b_view: wgpu::TextureView,
    /// The half-res pixel size these targets are built for; a change triggers a rebuild.
    size: (u32, u32),
}

/// The shadow source a surface CONSUMES this frame — the producer target whose depth the
/// lit passes sample, plus the ONE light-view-projection matrix (shared with the producer
/// camera) and the sampling `bias`/`light`. Set by [`Renderer::set_shadow_source`] inside
/// the consuming surface's draw closure; read by [`Renderer::prepare_frame`] to bind the
/// `@group(2/3)` shadow group. `None` ⇒ the default (1×1 depth, `enabled = 0`) is bound.
#[derive(Copy, Clone, Debug)]
struct ShadowSource {
    source: RenderTargetHandle,
    light_view_proj: Mat4,
    bias: f32,
    light: u32,
}

/// The renderer owns the GPU device, the surface, and every pipeline.
///
/// It is created via [`Renderer::new`] from a winit [`Window`] and lives for
/// the duration of the application.
pub struct Renderer {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Cached so we can hand it to pipelines and to pixel-to-NDC math.
    screen: Vec2,
    /// Background clear color (RGBA in 0..1).
    pub clear_color: [f64; 4],

    /// The per-surface clock, in seconds — advanced once per frame by [`Self::tick`] from
    /// the runner (NOT in [`Self::begin_frame`], which `render_to_texture` re-enters). Each
    /// surface stamps its [`RenderTarget::last_render`] from it, and an `hz` surface measures
    /// its period against it — see [`Self::surface_should_render`].
    frame_clock: f64,

    /// **THE one per-frame bind group** — camera + the frame's light list at
    /// `@group(0)`, shared by every lit pipeline (mesh, mesh_textured, skinned, lines).
    /// Built once, uploaded once per frame in [`Self::prepare_frame`], bound at slot 0
    /// by each pipeline's `render`. One buffer per value, one upload per frame — there
    /// is no second door into the scene uniform.
    frame: FrameBindGroup,

    triangle: TrianglePipeline,
    /// Vector UI-panel pipeline (rounded-rect + gradient + border SDF) — the
    /// flat Prism chrome. A 2D overlay drawn first per layer (behind the
    /// triangle/sprite/text of the same layer), like the other 2D pipelines.
    ui: UiPipeline,
    sprite: SpritePipeline,
    text: TextPipeline,
    mesh: MeshPipeline,
    mesh_textured: TexturedMeshPipeline,
    /// Instanced GPU-skinning pipeline: one static skinned mesh drawn as N
    /// instances (each with its own bone palette + model transform) in a single
    /// draw call. Additive to `mesh`/`mesh_textured`, renders in the opaque pass.
    skinned: SkinnedMeshPipeline,
    lines: LinesPipeline,
    lines_overlay: LinesPipeline,
    billboard: BillboardPipeline,
    sky: SkyPipeline,
    volumetric: VolumetricPipeline,
    ground_fog: GroundFogPipeline,
    /// The tonemap + colour-grade RESOLVE, run last for a surface that declares an `hdr`
    /// attachment (its recipe's `tonemap_grade` pass). Inert until a surface goes HDR — no
    /// shipped surface does in S3a.
    tonemap: TonemapGradePipeline,
    /// The HDR bloom post-effect (bright-pass + separable blur + additive composite), run in
    /// the HDR path AFTER everything writes `hdr` and BEFORE the tonemap resolves it (its
    /// recipe's `bloom` pass). Inert until a recipe raises [`Self::frame_bloom`]; the bright
    /// pass's read of the surface HDR is cached per HDR id like the tonemap's.
    bloom: BloomPipeline,
    /// **The shared `@group(2/3)` shadow bind** every lit pipeline samples — a per-source
    /// depth cache with a 1×1 `enabled = 0` default. Built once; the active source is
    /// selected per surface in [`Self::prepare_frame`] from [`Self::frame_shadow`]. Inert
    /// (default bound, byte-identical) for every surface that names no shadow.
    shadow: ShadowBind,
    /// The animated water-surface MESH pass — a wave-displaced grid drawn as depth-writing
    /// geometry in the opaque pass (it reads the shared frame group + a water uniform, and the
    /// sun out of the `Scene` uniform). Inert until a recipe's `water_surface` pass raises
    /// [`Self::frame_water`].
    water: WaterMeshPipeline,
    /// The water grid mesh ([`water_grid`]), uploaded ONCE the first frame a `water_surface`
    /// pass runs and reused thereafter (a `MeshHandle` in the shared pool, not a render
    /// target). `None` until then.
    water_grid: Option<MeshHandle>,

    /// Depth attachment shared by every pipeline in the main pass. The
    /// 3D pipeline writes/tests it; the 2D pipelines neither write nor
    /// test (their depth-stencil state is "always pass, no write").
    #[allow(dead_code)]
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    /// The window's HDR intermediate, allocated lazily the first frame the root surface
    /// goes HDR and dropped on resize (a new size = a new attachment + id). `None` = no HDR
    /// frame for the window — every shipped frame in S3a.
    hdr_color: Option<HdrColor>,
    /// Depth id of the window's depth texture (see [`RenderTarget::depth_id`]); renewed
    /// on resize so the depth-sampling passes drop their bind groups for the old one.
    main_depth_id: u64,
    /// The next depth / HDR id to hand out — one per depth OR HDR texture ever allocated
    /// (both caches key off the same monotone counter, so ids never collide).
    next_depth_id: u64,

    /// Uploaded textures, indexed by `TextureHandle`. A `None` slot is a freed
    /// entry available for reuse (see `free_texture_slots`): storage is a slot
    /// pool, not append-only, so a freed render-target colour returns its slot
    /// and its GPU texture (dropped here; wgpu frees it once the GPU is done
    /// reading). Mirrors the `meshes` pool below.
    textures: Vec<Option<LoadedTexture>>,
    /// Indices into `textures` that are `None` and ready to reuse.
    free_texture_slots: Vec<u32>,
    /// Uploaded meshes, indexed by `MeshHandle`. A `None` slot is a freed
    /// entry available for reuse (see `free_mesh_slots`): storage is a slot
    /// pool, not append-only, so an evicted LOD mesh returns its slot and
    /// its GPU buffers (dropped here; wgpu frees them once the GPU is done
    /// reading). This bounds storage by the number of *live* meshes.
    meshes: Vec<Option<LoadedMesh>>,
    /// Indices into `meshes` that are `None` and ready to reuse.
    free_mesh_slots: Vec<u32>,
    /// Current camera (cached so the runner can request the aspect-
    /// dependent view-projection in `end_frame`). `None` means "no
    /// camera set this frame" — `draw_mesh` still works but the matrix
    /// from the previous `set_camera` carries over.
    camera: Option<Camera>,
    /// Current frame-global lighting/atmosphere — the [`LightRig`] (light list,
    /// ambient, sky palette, fog), uploaded in `end_frame`. Defaults to the pre-uniform
    /// hardcoded look, so a scene that never calls [`Renderer::set_scene`] renders
    /// unchanged.
    scene: LightRig,
    /// This frame's volumetric-disk params (the cloud), or `None`. Reset each
    /// [`Renderer::begin_frame`]; set by [`Renderer::set_volumetric_disk`].
    volumetric_params: Option<VolumetricDisk>,
    /// This frame's ground-fog params, or `None`. Reset each [`Renderer::begin_frame`];
    /// set by [`Renderer::set_ground_fog`].
    ground_fog_params: Option<GroundFog>,
    /// Whether to draw the procedural sky behind the 3D scene this frame.
    /// Reset to `false` each [`Renderer::begin_frame`] and raised by
    /// [`Renderer::draw_sky`], so menus/loading (no 3D) keep their flat
    /// `clear_color` while the game requests a sky each active frame.
    draw_sky: bool,
    /// Ambient 2D layer applied to every `draw_sprite`/`draw_text`/
    /// `draw_triangle` until changed (the painter's-order sort key — higher
    /// draws on top). Reset to `0.0` each `begin_frame`; the scene manager
    /// raises it per scene so overlays sort above the scene beneath. See
    /// [`Renderer::set_layer`].
    current_layer: f32,
    /// Optional scissor rect (px: x, y, w, h) applied to subsequent 2D draws
    /// (`draw_ui_panel`/`draw_sprite`/`draw_text`) until changed — the clip a
    /// scroll region sets so its content is masked to its rect. `None` = full frame.
    current_clip: Option<[f32; 4]>,
    /// Whether the renderer is inside a DECLARED pass — an offscreen target's closure
    /// ([`Renderer::render_to_texture`]), a [`crate::FrameGraph::root`] / `overlay`
    /// element, or a screen composite (all wrapped by `begin_pass`/`end_pass` from
    /// `FrameGraph::execute`). ANY draw — 2D or 3D — queued outside one is a STRAY:
    /// counted in `stray_draws` and reported at [`Renderer::end_frame`]. The screen is a
    /// surface that is declared, not assumed — immediate-mode draws to the swapchain were
    /// the holdout the gate exists to name.
    in_pass: bool,
    /// Draws (2D or 3D) queued outside a declared pass this frame (see `in_pass`).
    stray_draws: u32,
    /// Consecutive frames that carried a stray — the report is rate-limited on it.
    stray_frames: u32,

    /// Offscreen render targets, indexed by [`RenderTargetHandle`] (slot pool).
    render_targets: Vec<Option<RenderTarget>>,
    free_target_slots: Vec<u32>,
    /// Whether the sky draws in the pass currently being encoded — set by `prepare_frame`,
    /// read by `encode_passes`, so the offscreen and swapchain paths share one encode path.
    sky_this_frame: bool,
    /// This frame's depth-pass plan — the depth-sampling passes in the order the RECIPE
    /// executes them, built by the one pure [`depth_plan`] and handed over by
    /// [`crate::FrameGraph::surface`] through [`Renderer::set_depth_plan`]. Reset each
    /// [`Renderer::begin_frame`]. Empty ⇒ `encode_passes` falls back to the fixed legacy
    /// `[Volumetric, GroundFog]` order (each still gated on its params), so a direct-setter
    /// caller outside a recipe is byte-identical to before.
    depth_plan: Vec<DepthPass>,
    /// `Some(format)` when this frame renders the lit passes into the HDR attachment and
    /// resolves them with the tonemap pass — raised by [`Renderer::set_tonemap_grade`]
    /// (the recipe's `tonemap_grade` pass), and the format IS the one the stage's declared
    /// `hdr` attachment names, resolved through
    /// [`AttachmentFormat::texture_format`](crate::AttachmentFormat::texture_format). One
    /// representation: the flag and the format it implies cannot disagree. Reset each
    /// [`Renderer::begin_frame`]; `None` is the byte-identical non-HDR encode.
    frame_hdr: Option<wgpu::TextureFormat>,
    /// This frame's grade params `(tint, strength, exposure)` for the tonemap resolve, or
    /// `None`. Uploaded in `prepare_frame`; set by [`Renderer::set_tonemap_grade`].
    frame_grade: Option<(Vec3, f32, f32)>,
    /// When a shadow PRODUCER surface is being encoded, the light-view-projection the lit
    /// passes render the casters with — REPLACES the camera-derived view-projection in
    /// [`Self::prepare_frame`], so the depth is written from the light's POV. Set by
    /// [`Renderer::begin_shadow_view`]; reset each [`Renderer::begin_frame`].
    shadow_view_override: Option<Mat4>,
    /// The shadow this surface CONSUMES this frame (the producer target + its matrix), or
    /// `None`. Set by [`Renderer::set_shadow_source`], read in `prepare_frame` to bind the
    /// shadow group; reset each [`Renderer::begin_frame`].
    frame_shadow: Option<ShadowSource>,
    /// This frame's water-surface params, or `None`. Reset each [`Renderer::begin_frame`];
    /// set by [`Renderer::set_water`] (a recipe's `water_surface` pass). Gates the water-mesh
    /// draw in the opaque pass exactly like `ground_fog_params` gates the fog.
    frame_water: Option<Water>,
    /// This frame's bloom art knobs `(threshold, knee, intensity, radius)`, or `None`. Reset
    /// each [`Renderer::begin_frame`]; set by [`Renderer::set_bloom`] (a recipe's `bloom`
    /// pass). Gates the bloom chain in `encode_passes` — a tuple like `frame_grade`, so the
    /// four numbers and the "is this frame bloomed" flag are one value.
    frame_bloom: Option<(f32, f32, f32, f32)>,
    /// The half-res ping-pong scratch the bloom blurs through, allocated lazily the first bloom
    /// frame and reused until the surface's half-res size changes (like [`Self::hdr_color`]).
    /// `None` = no bloom frame yet, or the size changed and it awaits a rebuild.
    bloom_scratch: Option<BloomScratch>,
}

/// Insert `value` into a slot-pool vec, reusing a freed index if one is available
/// else appending, and return the slot index. The reuse discipline shared by the
/// texture and render-target pools (and mirrored inline by the mesh pools), so
/// storage stays bounded by the number of *live* entries rather than the number
/// ever created. Pure — unit-tested in `slot_pool_tests`.
fn pool_alloc<T>(slots: &mut Vec<Option<T>>, free: &mut Vec<u32>, value: T) -> u32 {
    if let Some(slot) = free.pop() {
        slots[slot as usize] = Some(value);
        slot
    } else {
        let slot = slots.len() as u32;
        slots.push(Some(value));
        slot
    }
}

/// Free a slot-pool entry, returning its index to `free` for reuse and handing back
/// the removed value (so a caller can cascade — a render target frees its colour
/// texture). An already-free or out-of-range index is ignored (returns `None`).
fn pool_free<T>(slots: &mut [Option<T>], free: &mut Vec<u32>, id: u32) -> Option<T> {
    let taken = slots.get_mut(id as usize).and_then(Option::take);
    if taken.is_some() {
        free.push(id);
    }
    taken
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create wgpu surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("no compatible wgpu adapter found")?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("flicker.device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .context("failed to request wgpu device")?;

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width,
            height,
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (depth_texture, depth_view) = create_depth_view(&device, width, height);

        let triangle = TrianglePipeline::new(&device, surface_format);
        let ui = UiPipeline::new(&device, surface_format);
        let sprite = SpritePipeline::new(&device, surface_format);
        let text = TextPipeline::new(&device, &queue, surface_format);
        let min_uniform_offset_alignment = device.limits().min_uniform_buffer_offset_alignment;
        // The ONE per-frame group, built before the pipelines that share its layout.
        let frame = FrameBindGroup::new(&device);
        // The shared shadow bind, built before the lit pipelines that reference its layout.
        let shadow = ShadowBind::new(&device);
        let mesh = MeshPipeline::new(
            &device,
            &frame,
            &shadow,
            surface_format,
            min_uniform_offset_alignment,
        );
        let mesh_textured = TexturedMeshPipeline::new(
            &device,
            &queue,
            &frame,
            &shadow,
            surface_format,
            min_uniform_offset_alignment,
        );
        let skinned = SkinnedMeshPipeline::new(&device, &queue, &frame, &shadow, surface_format);
        let lines = LinesPipeline::new(
            &device,
            &frame,
            surface_format,
            wgpu::CompareFunction::LessEqual,
        );
        // Overlay lines ignore depth (Always) — for drawing a skeleton (or other debug
        // gizmos) ON TOP of the mesh so it shows through instead of being occluded.
        let lines_overlay = LinesPipeline::new(
            &device,
            &frame,
            surface_format,
            wgpu::CompareFunction::Always,
        );
        let billboard = BillboardPipeline::new(&device, surface_format);
        let sky = SkyPipeline::new(&device, surface_format);
        // The depth-sampling passes bind the depth of whichever SURFACE is being encoded
        // (see `render_to_texture` / `end_frame`); the window's depth is id 1.
        let main_depth_id = 1;
        let mut volumetric = VolumetricPipeline::new(&device, surface_format);
        volumetric.bind_depth(&device, main_depth_id, &depth_view);
        let mut ground_fog = GroundFogPipeline::new(&device, surface_format);
        ground_fog.bind_depth(&device, main_depth_id, &depth_view);
        // The tonemap resolves the HDR attachment into the sRGB swapchain; its HDR bind
        // groups are built lazily per surface (`bind_hdr`), so nothing is bound at boot.
        let tonemap = TonemapGradePipeline::new(&device, surface_format);
        // The bloom targets HDR_FORMAT only (it reads and writes the HDR attachment); its
        // bright-source bind + half-res scratch are built lazily the first bloom frame.
        let bloom = BloomPipeline::new(&device);
        // The water mesh shares the frame group (camera + lights) and the shadow default,
        // like the lit pipelines; its grid mesh is uploaded lazily the first watered frame.
        let water = WaterMeshPipeline::new(&device, &frame, &shadow, surface_format);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            screen: Vec2::new(width as f32, height as f32),
            clear_color: [0.05, 0.06, 0.08, 1.0],
            frame_clock: 0.0,
            frame,
            triangle,
            ui,
            sprite,
            text,
            mesh,
            mesh_textured,
            skinned,
            lines,
            lines_overlay,
            billboard,
            sky,
            volumetric,
            ground_fog,
            tonemap,
            bloom,
            shadow,
            water,
            water_grid: None,
            volumetric_params: None,
            ground_fog_params: None,
            depth_texture,
            depth_view,
            hdr_color: None,
            main_depth_id,
            next_depth_id: 2,
            textures: Vec::new(),
            free_texture_slots: Vec::new(),
            meshes: Vec::new(),
            free_mesh_slots: Vec::new(),
            camera: None,
            scene: LightRig::default(),
            draw_sky: false,
            current_layer: 0.0,
            current_clip: None,
            in_pass: false,
            stray_draws: 0,
            stray_frames: 0,
            render_targets: Vec::new(),
            free_target_slots: Vec::new(),
            sky_this_frame: false,
            depth_plan: Vec::new(),
            frame_hdr: None,
            frame_grade: None,
            shadow_view_override: None,
            frame_shadow: None,
            frame_water: None,
            frame_bloom: None,
            bloom_scratch: None,
        })
    }

    /// Return the underlying winit window — useful for cursor/title changes.
    pub fn window(&self) -> &Window {
        &self.window
    }

    /// Current monitor's physical size `(width, height)` in pixels, if the
    /// window is on a known monitor — for building a resolution list.
    pub fn monitor_size(&self) -> Option<(u32, u32)> {
        self.window.current_monitor().map(|m| {
            let s = m.size();
            (s.width, s.height)
        })
    }

    /// The distinct physical sizes `(width, height)` of the current monitor's video modes —
    /// the device-enumerated resolution rungs the settings panel offers. Empty when there
    /// is no current monitor (headless), so the caller falls back to a static list. May
    /// carry duplicate sizes (one per refresh rate); the caller dedupes.
    pub fn video_mode_sizes(&self) -> Vec<(u32, u32)> {
        self.window
            .current_monitor()
            .map(|m| {
                m.video_modes()
                    .map(|v| {
                        let s = v.size();
                        (s.width, s.height)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `true` when the window is in any fullscreen mode (borderless or
    /// exclusive); `false` when windowed.
    pub fn is_fullscreen(&self) -> bool {
        self.window.fullscreen().is_some()
    }

    /// The window's current outer position (top-left, physical px), or `None` if
    /// the platform can't report it. Used to persist the windowed placement.
    pub fn outer_position(&self) -> Option<(i32, i32)> {
        self.window.outer_position().ok().map(|p| (p.x, p.y))
    }

    /// Move the window's top-left to `(x, y)` in physical px (windowed mode). The
    /// caller clamps `(x, y)` to keep the window on-screen.
    pub fn set_outer_position(&self, x: i32, y: i32) {
        self.window
            .set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
    }

    /// The window's current inner (content) size in physical px.
    pub fn inner_size(&self) -> (u32, u32) {
        let s = self.window.inner_size();
        (s.width, s.height)
    }

    /// Switch to a windowed view at the given physical size. The actual resize
    /// arrives as a later `Resized` event (handled by [`Self::resize`]).
    pub fn set_windowed(&self, width: u32, height: u32) {
        self.window.set_fullscreen(None);
        let _ = self
            .window
            .request_inner_size(PhysicalSize::new(width, height));
    }

    /// Borderless (desktop-resolution) fullscreen on the current monitor.
    pub fn set_borderless_fullscreen(&self) {
        self.window
            .set_fullscreen(Some(Fullscreen::Borderless(None)));
    }

    /// Exclusive fullscreen at `(width, height)` if the current monitor has a
    /// matching video mode; otherwise falls back to borderless fullscreen.
    /// Returns `true` if an exact exclusive mode was selected.
    pub fn set_exclusive_fullscreen(&self, width: u32, height: u32) -> bool {
        let Some(monitor) = self.window.current_monitor() else {
            self.window
                .set_fullscreen(Some(Fullscreen::Borderless(None)));
            return false;
        };
        let mode = monitor.video_modes().find(|m| {
            let s = m.size();
            s.width == width && s.height == height
        });
        match mode {
            Some(m) => {
                self.window.set_fullscreen(Some(Fullscreen::Exclusive(m)));
                true
            }
            None => {
                self.window
                    .set_fullscreen(Some(Fullscreen::Borderless(Some(monitor))));
                false
            }
        }
    }

    /// Reconfigure the surface, update cached screen size, and recreate
    /// the depth texture.
    pub fn resize(&mut self, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.screen = Vec2::new(w as f32, h as f32);
        let (depth_texture, depth_view) = create_depth_view(&self.device, w, h);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        // A new depth texture is a new identity: the depth-sampling passes drop their
        // bind groups for the old one and bind the new at the next `end_frame`.
        self.volumetric.forget(self.main_depth_id);
        self.ground_fog.forget(self.main_depth_id);
        self.main_depth_id = self.next_depth_id;
        self.next_depth_id += 1;
        // The window's HDR attachment is size-bound too: drop it (and the tonemap's + bloom's
        // bind groups for it) so it re-allocates at the new size on the next HDR frame.
        if let Some(hdr) = self.hdr_color.take() {
            self.tonemap.forget(hdr.id);
            self.bloom.forget(hdr.id);
        }
        // The half-res bloom scratch tracks the window size — drop it so it rebuilds at the
        // new size on the next bloom frame (never a stale-size sample).
        self.bloom_scratch = None;
    }

    /// Current logical size of the rendering surface, in pixels.
    pub fn size(&self) -> Vec2 {
        self.screen
    }

    /// Upload an RGBA8 **sRGB** colour image (albedo, sprites, UI) and return a handle.
    /// The pixel buffer length must equal `width * height * 4`.
    pub fn load_texture(&mut self, pixels: &[u8], width: u32, height: u32) -> TextureHandle {
        let tex = LoadedTexture::from_rgba8(
            &self.device,
            &self.queue,
            &self.sprite.sampler,
            &self.sprite.texture_bind_group_layout,
            pixels,
            width,
            height,
        );
        self.register_texture(tex)
    }

    /// Upload an RGBA8 **linear** (non-colour) image — a normal / roughness / metalness
    /// / AO map for the textured-mesh PBR path — and return a handle. Same as
    /// [`Self::load_texture`] but the texture is `Rgba8Unorm` (no sRGB decode), so the
    /// bytes are sampled verbatim. The resulting handle binds as any of the PBR map
    /// slots on [`Self::draw_textured_mesh_pbr`].
    pub fn load_texture_linear(&mut self, pixels: &[u8], width: u32, height: u32) -> TextureHandle {
        let tex = LoadedTexture::from_rgba8_linear(
            &self.device,
            &self.queue,
            &self.sprite.sampler,
            &self.sprite.texture_bind_group_layout,
            pixels,
            width,
            height,
        );
        self.register_texture(tex)
    }

    /// Overwrite an already-uploaded texture's pixels **in place**, keeping its handle,
    /// view and every bind group built over it.
    ///
    /// For content that is regenerated repeatedly at a fixed size — a procedural texture
    /// under a live slider, a CPU-composited overlay — this is the difference between a
    /// write and an allocation: re-uploading through [`Self::load_texture`] would create a
    /// new GPU texture plus three bind groups every time, orphan the old slot, and hand
    /// back a new handle that every holder would have to be told about. Here the handle is
    /// stable, so a UI tree can name the texture once and keep naming it.
    ///
    /// `pixels` must be `width * height * 4` bytes for the texture's ORIGINAL size, and
    /// its interpretation still follows the format the texture was created with (an sRGB
    /// texture stays sRGB). Returns `false` — writing nothing — if the handle is unknown
    /// or the buffer does not match the texture's size, so a caller that resized has a
    /// signal to re-upload rather than a silently torn image.
    #[must_use]
    pub fn update_texture(&mut self, handle: TextureHandle, pixels: &[u8]) -> bool {
        let Some(Some(tex)) = self.textures.get(handle.0 as usize) else {
            return false;
        };
        let (w, h) = tex.size;
        if pixels.len() as u32 != w * h * 4 {
            return false;
        }
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &tex.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        true
    }

    /// Build the auxiliary billboard + textured-mesh bind groups for a freshly uploaded
    /// texture and push it into the store, returning its handle. Shared by the sRGB and
    /// linear upload paths.
    fn register_texture(&mut self, mut tex: LoadedTexture) -> TextureHandle {
        // Also build a bind group for the billboard pipeline so this
        // texture can be drawn as a world-space billboard atlas.
        tex.billboard_bind_group =
            Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("flicker.billboard.texture.bind_group"),
                layout: &self.billboard.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&tex.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.billboard.sampler),
                    },
                ],
            }));
        // And a bind group for the textured-mesh pipeline (linear sampler), so this
        // texture can be sampled as a mesh albedo or PBR map.
        tex.mesh_bind_group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flicker.mesh_textured.texture.bind_group"),
            layout: &self.mesh_textured.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.mesh_textured.sampler),
                },
            ],
        }));
        // Slot-pool insert (reuse a freed slot, else append) so a freed render-target
        // colour recycles its slot instead of leaking — see `pool_alloc`.
        TextureHandle(pool_alloc(
            &mut self.textures,
            &mut self.free_texture_slots,
            tex,
        ))
    }

    /// Upload a 3D mesh and return a handle. The mesh persists across
    /// frames; subsequent `draw_mesh(handle, ...)` calls reuse the same
    /// GPU buffers.
    ///
    /// **Conformance role (Memory & Resource Architecture spec):** this is
    /// the upload step of the CPU-mesh → upload → render path — the
    /// `MESH-1` / §7 `FALLBACK-1` CPU-meshing fallback, explicitly labeled
    /// as such. It is *not* the steady-state target: per `MESH-1` the
    /// per-cell contour/QEF should run as a GPU-compute pass producing
    /// `DeviceResident` geometry that renders with no CPU round trip (and
    /// no `upload_mesh`). This fallback is the only path built today and is
    /// kept expressible per `FALLBACK-1`.
    pub fn upload_mesh(&mut self, vertices: &[MeshVertex], indices: MeshIndices<'_>) -> MeshHandle {
        let loaded = self.mesh.upload(&self.device, vertices, indices);
        // Reuse a freed slot if one is available, else append. Reuse keeps
        // mesh storage bounded by the number of *live* meshes rather than
        // the number ever uploaded — LOD swaps recycle slots instead of
        // leaking one set of buffers per swap.
        let id = if let Some(slot) = self.free_mesh_slots.pop() {
            self.meshes[slot as usize] = Some(loaded);
            slot
        } else {
            let slot = self.meshes.len() as u32;
            self.meshes.push(Some(loaded));
            slot
        };
        MeshHandle(id)
    }

    /// Free a previously uploaded mesh, returning its slot to the reuse
    /// pool. Dropping the [`LoadedMesh`] drops its GPU buffers; wgpu defers
    /// the actual GPU-memory free until the device finishes reading them, so
    /// this is safe to call the same frame the mesh was last drawn — no
    /// fence, no frames-in-flight bookkeeping. A handle that is already free
    /// (or never existed) is ignored.
    pub fn free_mesh(&mut self, handle: MeshHandle) {
        if let Some(slot) = self.meshes.get_mut(handle.0 as usize) {
            if slot.take().is_some() {
                self.free_mesh_slots.push(handle.0);
            }
        }
    }

    /// Free a texture slot, returning it to the reuse pool. Dropping the
    /// [`LoadedTexture`] drops its GPU texture + bind groups; wgpu defers the
    /// actual free until the device finishes reading, so this is safe the same
    /// frame the texture was last drawn. An already-free or unknown handle is
    /// ignored. Private: the only freeable textures today are render-target
    /// colours (via [`Self::free_render_target`]); directly-uploaded textures
    /// (atlases, sprite sheets) live for the app's lifetime.
    fn free_texture(&mut self, handle: TextureHandle) {
        pool_free(&mut self.textures, &mut self.free_texture_slots, handle.0);
    }

    /// Advance the per-surface clock by this frame's delta. Called ONCE per frame by the
    /// runner, immediately before [`Self::begin_frame`] — deliberately NOT inside
    /// `begin_frame`, which [`Self::render_to_texture`] re-enters per offscreen pass and
    /// would tick N extra times a frame. A poster / `hz` surface measures its liveness
    /// against this clock (see [`Self::surface_should_render`]).
    pub fn tick(&mut self, dt: Duration) {
        self.frame_clock += dt.as_secs_f64();
    }

    /// Reset all per-frame draw queues. Called by the runner at the start
    /// of every frame. Mesh **storage** (uploaded vertex/index buffers)
    /// is retained; only the per-frame mesh **draw queue** clears.
    pub fn begin_frame(&mut self) {
        self.triangle.clear();
        self.ui.clear();
        self.sprite.clear();
        self.text.clear();
        self.mesh.clear();
        self.mesh_textured.clear();
        self.skinned.clear();
        self.lines.clear();
        self.lines_overlay.clear();
        self.billboard.clear();
        self.draw_sky = false;
        self.volumetric_params = None;
        self.ground_fog_params = None;
        self.depth_plan.clear();
        self.frame_hdr = None;
        self.frame_grade = None;
        self.shadow_view_override = None;
        self.frame_shadow = None;
        self.frame_water = None;
        self.frame_bloom = None;
        self.current_layer = 0.0;
        self.current_clip = None;
    }

    /// The shared per-draw hook EVERY `draw_*` entry point calls — 2D and 3D alike. A draw
    /// queued while not inside a declared pass ([`Self::in_pass`]) is a STRAY, counted here
    /// and reported at [`Self::end_frame`]. This is the ONE gate that makes "nothing draws
    /// outside a declared pass" an enforced runtime invariant rather than a comment; a new
    /// `draw_*` entry point must call it, or it escapes the gate.
    #[inline]
    fn note_draw(&mut self) {
        if !self.in_pass {
            self.stray_draws += 1;
        }
    }

    /// Enter / leave a declared pass on the MAIN frame — a frame graph root or overlay
    /// element, or a screen composite. Offscreen passes mark themselves around their
    /// closure in [`Self::render_to_texture`].
    pub(crate) fn begin_pass(&mut self) {
        self.in_pass = true;
    }

    pub(crate) fn end_pass(&mut self) {
        self.in_pass = false;
    }

    /// Set the ambient 2D layer for subsequent `draw_sprite`/`draw_text`/
    /// `draw_triangle` calls. Higher layers draw on top; ties break by
    /// submission order. 2D ordering is pure painter's order — the depth buffer
    /// is never used for 2D (mirrors DirectXTK's `DepthNone` `SpriteBatch`
    /// default). Reset to `0.0` each `begin_frame`. The scene manager sets this
    /// per scene (= stack position), so overlays sort above the scene beneath
    /// with no per-widget bookkeeping; within a scene, offset from
    /// [`Renderer::layer`] to stack sub-elements (e.g. a dropdown over a panel).
    pub fn set_layer(&mut self, layer: f32) {
        self.current_layer = layer;
    }

    /// The current ambient 2D layer (see [`Renderer::set_layer`]).
    pub fn layer(&self) -> f32 {
        self.current_layer
    }

    /// Clip subsequent 2D draws to `rect` (px: x, y, w, h) until [`Renderer::clear_clip`].
    /// Every `draw_ui_panel`/`draw_sprite`/`draw_text` submitted while a clip is set is
    /// masked to it (a scroll region's viewport); the clip is captured per-draw, so it
    /// survives the per-layer painter's-order sort. Reset each `begin_frame`.
    pub fn set_clip(&mut self, rect: [f32; 4]) {
        self.current_clip = Some(rect);
    }

    /// Clear the 2D scissor clip — subsequent draws fill the whole frame again.
    pub fn clear_clip(&mut self) {
        self.current_clip = None;
    }

    /// Submit a solid-colored triangle. Vertices are pixel coordinates with the
    /// origin at the top-left.
    pub fn draw_triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: [f32; 4]) {
        self.note_draw();
        self.triangle
            .push(self.screen, a, b, c, color, self.current_layer);
    }

    /// Submit a textured quad. `position` is the top-left in pixels; `size` is in pixels;
    /// `color` is an RGBA tint in 0..1 that is multiplied with the sampled texel
    /// (pass `[1.0; 4]` for "no tint").
    pub fn draw_sprite(
        &mut self,
        texture: TextureHandle,
        position: Vec2,
        size: Vec2,
        color: [f32; 4],
    ) {
        self.draw_sprite_uv(texture, position, size, color, FULL_TEXTURE);
    }

    /// Submit a textured quad drawn from a SUB-RECTANGLE of its texture — the atlas
    /// draw. `uv` is `[u0, v0, u1, v1]` in normalized texture space with the origin
    /// top-left; [`FULL_TEXTURE`] is the whole image, which is exactly what
    /// [`Renderer::draw_sprite`] passes.
    ///
    /// Many small images in one texture cost one bind rather than one each, because
    /// the sprite batch groups its quads by texture handle.
    pub fn draw_sprite_uv(
        &mut self,
        texture: TextureHandle,
        position: Vec2,
        size: Vec2,
        color: [f32; 4],
        uv: [f32; 4],
    ) {
        // Pivot is ignored when rotation is 0.0; ZERO keeps the axis-aligned path.
        self.draw_sprite_ex(texture, position, size, color, uv, 0.0, Vec2::ZERO);
    }

    /// Submit a **rotated** atlas quad. `rotation` is radians; `pivot` is the
    /// centre of rotation in the same top-left screen-pixel space as `position`.
    /// Pass `position + size * 0.5` to spin about the sprite's centre (the common
    /// case — a wheel, a tank track), or an arbitrary point for an off-centre
    /// pivot (a turret turning about its hull mount). Screen y is down, so a
    /// positive angle turns clockwise. Every other sprite entry point delegates
    /// here with `rotation = 0.0`, which is the unchanged axis-aligned fast path.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_sprite_ex(
        &mut self,
        texture: TextureHandle,
        position: Vec2,
        size: Vec2,
        color: [f32; 4],
        uv: [f32; 4],
        rotation: f32,
        pivot: Vec2,
    ) {
        self.note_draw();
        self.sprite.push(
            self.screen,
            texture,
            position,
            size,
            color,
            self.current_layer,
            self.current_clip,
            uv,
            rotation,
            pivot,
        );
    }

    /// Submit a **vector UI panel**: a rounded-rectangle filled with a solid or
    /// 2-stop linear gradient and ringed with an optional border, evaluated as a
    /// signed-distance field in one draw (the flat Prism chrome — panels,
    /// buttons, field wells). `position` is the top-left in pixels; `size` is in
    /// pixels. `color`/`color2` are the gradient stops (pass equal for a solid
    /// fill); `grad` selects the axis (`0.0` solid, `1.0` vertical, `2.0`
    /// horizontal); `radius` is the corner radius and `border` the border
    /// thickness (both in pixels, `0.0` to disable); `border_color` rings the
    /// edge; `feather` softens the outer edge in pixels (a soft drop shadow —
    /// `0.0` for a crisp panel). Sorts at the current [`Renderer::layer`].
    #[allow(clippy::too_many_arguments)]
    pub fn draw_ui_panel(
        &mut self,
        position: Vec2,
        size: Vec2,
        color: [f32; 4],
        color2: [f32; 4],
        grad: f32,
        radius: f32,
        border: f32,
        border_color: [f32; 4],
        feather: f32,
    ) {
        self.note_draw();
        self.ui.push(
            self.screen,
            position,
            size,
            color,
            color2,
            grad,
            radius,
            border,
            border_color,
            feather,
            self.current_layer,
            self.current_clip,
        );
    }

    /// Register a UI font (TTF/OTF bytes) so a [`FontRole`](crate::FontRole) can
    /// select it. Call once at startup with the Prism faces (the app owns the
    /// bytes); a role with no registered face falls back to a system font.
    pub fn register_ui_font(&mut self, bytes: &[u8]) {
        self.text.register_font(bytes);
    }

    /// Submit a string of text in the default body face. `position` is the
    /// top-left baseline in pixels; `size` is the font size in pixels; `color`
    /// is RGBA in 0..1.
    pub fn draw_text(&mut self, text: &str, position: Vec2, size: f32, color: [f32; 4]) {
        self.draw_text_role(
            text,
            position,
            size,
            color,
            crate::FontRole::Body,
            false,
            false,
            -1.0,
            None,
        );
    }

    /// Submit a string of text in the face selected by `role`
    /// ([`FontRole`](crate::FontRole)), styled by `italic`/`bold` within the
    /// family (`bold` selects the heavier cut on roles that ship one). `tracking`
    /// is the letter-spacing as an em fraction; a negative value uses the role's
    /// default (so numeric cells pass `0.0` to keep fixed-width columns aligned).
    #[allow(clippy::too_many_arguments)] // each param is a distinct text attribute
    pub fn draw_text_role(
        &mut self,
        text: &str,
        position: Vec2,
        size: f32,
        color: [f32; 4],
        role: crate::FontRole,
        italic: bool,
        bold: bool,
        tracking: f32,
        wrap: Option<f32>,
    ) {
        self.note_draw();
        self.text.push(
            text,
            position.x,
            position.y,
            size,
            color,
            self.current_layer,
            role,
            italic,
            bold,
            tracking,
            self.current_clip,
            wrap,
        );
    }

    /// Measure `text` at font `size` in the default body face, returning its
    /// rendered size (max line width, total height) in pixels. For laying out UI
    /// before drawing — shapes a throwaway buffer, no upload.
    pub fn measure_text(&mut self, text: &str, size: f32) -> Vec2 {
        self.measure_text_role(text, size, crate::FontRole::Body, false, false, -1.0)
    }

    /// Measure `text` at font `size` in the face selected by `role` and styled by
    /// `italic`/`bold`. Mirror of [`Self::measure_text`] for non-body faces
    /// (titles/labels); the style must match the eventual draw so alignment (which
    /// offsets by the measured width) stays correct.
    pub fn measure_text_role(
        &mut self,
        text: &str,
        size: f32,
        role: crate::FontRole,
        italic: bool,
        bold: bool,
        tracking: f32,
    ) -> Vec2 {
        let (w, h) = self.text.measure(text, size, role, italic, bold, tracking);
        Vec2::new(w, h)
    }

    /// Set the 3D camera used for subsequent `draw_mesh` calls. Typically
    /// called once per frame before any `draw_mesh`.
    pub fn set_camera(&mut self, camera: &Camera) {
        self.camera = Some(*camera);
    }

    /// Set the frame-global lighting & atmosphere — the [`LightRig`]: the light list,
    /// ambient, the sky palette and fog — for the 3D mesh pass. Cached and uploaded in
    /// [`Renderer::end_frame`], mirroring [`Renderer::set_camera`]; the camera position
    /// is injected automatically (for fog distance), so callers supply only the
    /// rig. The colour GRADE is not here — it is pass-owned by
    /// [`crate::TonemapGradePass`]. Persists until the next call — set it once per
    /// frame. A scene that never calls this renders with the default (former hardcoded)
    /// lighting.
    ///
    /// This is the ONE door into the scene uniform: a scene that owns its own lights
    /// composes them over [`Renderer::scene_lighting`] and sets the whole rig back.
    pub fn set_scene(&mut self, scene: LightRig) {
        self.scene = scene;
    }

    /// The lighting & atmosphere in force right now — whatever the last
    /// [`Renderer::set_scene`] left. A pass whose look FOLLOWS the scene rather
    /// than authoring its own reads it here (the ground fog takes this
    /// `fog_color` when its recipe authors none, so fog under a day/night cycle
    /// tracks the cycle without the scene reaching into the recipe).
    pub fn scene_lighting(&self) -> LightRig {
        self.scene
    }

    /// Upload the material-catalog colour palette for the 3D mesh pass — one
    /// RGBA per `MaterialId` (index = id, `materials.json` order). The pipeline
    /// boots all-magenta (loud-wrong), so a voxel scene sets this once from the
    /// loaded catalog at init; undefined slots should be left magenta rather
    /// than given an invented fallback colour. Persists until the next call.
    pub fn set_material_palette(
        &mut self,
        colors: &[[f32; 4]; crate::pipeline_mesh::MATERIAL_PALETTE_LEN],
    ) {
        self.mesh.set_material_palette(&self.queue, colors);
    }

    /// Request the procedural sky behind the 3D scene this frame. Fakes
    /// atmospheric scattering from the current [`LightRig`] (its SKY SLOTS 0 and 1 —
    /// see [`LightRig::sky_sun`] — and the `sky_zenith`/`sky_horizon` palette) — a
    /// fullscreen pass drawn first, so terrain, lines, billboards, and the 2D
    /// UI all layer on top. Per-frame, like `draw_mesh`: call it each frame
    /// you want a sky; omit it (menus/loading) to keep the flat `clear_color`.
    /// A no-op unless a camera is also set this frame (it needs the view ray).
    pub fn draw_sky(&mut self) {
        self.note_draw();
        self.draw_sky = true;
    }

    /// Hand this frame's depth-pass plan to the encoder — the depth-sampling passes in the
    /// order the RECIPE executes them, built by the one pure [`depth_plan`] from the
    /// already-ordered recipe and set by [`crate::FrameGraph::surface`] before it applies
    /// the passes. This is the ONLY thing that decides whether the disk or the fog
    /// composites first; the `set_volumetric_disk` / `set_ground_fog` setters carry the
    /// params and nothing else, so the order has one representation. An unset plan (a
    /// direct-setter caller outside a recipe) keeps the fixed legacy order.
    pub fn set_depth_plan(&mut self, plan: Vec<DepthPass>) {
        self.depth_plan = plan;
    }

    /// Draw a **volumetric dust disk** this frame: a raymarched protoplanetary-disk cloud
    /// (dark dust + warm star-lit inner glow) whose density dissipates inside-out with
    /// `params.formation` and carves gaps around `params.bodies`. Drawn just after the sky and
    /// composited over it. Per-frame, like `draw_sky`; a no-op unless a camera is set (it needs
    /// the view ray).
    pub fn set_volumetric_disk(&mut self, params: VolumetricDisk) {
        // A 3D element of the pass it is requested in — counted like any other draw, so
        // the declared-surface gate sees it.
        self.note_draw();
        self.volumetric_params = Some(params);
    }

    /// Draw a **volumetric ground fog** this frame: a raymarched horizontal fog slab of animated
    /// drifting noise, depth-aware (occluded by geometry) and correctly self-compositing (no
    /// billboard/quad layering artifacts). Composited over the scene in the overlay pass, just
    /// after the volumetric disk. Per-frame, like [`Renderer::draw_sky`]; a no-op unless a camera
    /// is set (it needs the view ray). See [`GroundFog`] for the band / colour / wind params.
    pub fn set_ground_fog(&mut self, params: GroundFog) {
        self.note_draw();
        self.ground_fog_params = Some(params);
    }

    /// Draw an **animated water surface** this frame: a wave-displaced grid MESH at
    /// `params.sea_level`, depth-tested + depth-writing (it occludes and is occluded by the
    /// terrain) with a real sun specular from light slot 0, composited premultiplied over the
    /// lit scene in the opaque pass. Raised by a recipe's `water_surface` pass
    /// ([`crate::PassKind::WaterSurface`]); per-frame, like the fog, and a no-op unless a
    /// camera is set. The grid mesh is uploaded ONCE on the first watered frame and reused. See
    /// [`Water`] for the params.
    pub fn set_water(&mut self, params: Water) {
        self.note_draw();
        // Upload the water grid once — a UNIT grid the shader reads as SCREEN space and projects
        // onto the sea plane, so it is camera/sea-level independent and never rebuilds.
        if self.water_grid.is_none() {
            let (verts, idx) = water_grid(WATER_GRID_N);
            self.water_grid = Some(self.upload_mesh(&verts, MeshIndices::U32(&idx)));
        }
        self.frame_water = Some(params);
    }

    /// Run the **HDR bloom** post-effect this frame: extract the bright parts of the surface's
    /// `hdr` colour above a soft-kneed `threshold` (with a `knee` for a smooth ramp), blur them
    /// separably, and add the glow back into `hdr` with `intensity`, `radius` scaling the blur
    /// spread. Raised by a recipe's `bloom` pass ([`crate::PassKind::Bloom`]); per-frame, like
    /// the tonemap, and a NO-OP unless the surface actually resolves through the tonemap (it
    /// reads and writes the `hdr` attachment that only an HDR surface owns). The half-res
    /// scratch is allocated lazily and reused, exactly like the HDR attachment.
    pub fn set_bloom(&mut self, threshold: f32, knee: f32, intensity: f32, radius: f32) {
        // A whole-surface pass of the surface it is requested in — counted like the tonemap, so
        // the declared-surface gate sees a bloom requested outside a declared pass.
        self.note_draw();
        self.frame_bloom = Some((threshold, knee, intensity, radius));
    }

    /// Resolve this surface through the tonemap + colour-grade pass this frame: render the
    /// lit 3D passes into the HDR (rgba16f) attachment and roll them off through the
    /// ACES-fitted curve into the sRGB colour, with an optional grade tint. Raised by a
    /// recipe's `tonemap_grade` pass ([`crate::PassKind::TonemapGrade`]). `grade` is a
    /// linear-RGB tint the resolve lerps toward by `grade_strength` (`0` = tonemap only);
    /// `exposure` is a linear multiply before the curve (`1.0` = neutral). Per-frame, like
    /// the sky / fog: the surface's HDR attachment is allocated lazily the first HDR frame
    /// and reused after, **in the format `hdr` names** — the stage's own declared `hdr`
    /// attachment format, resolved through [`AttachmentFormat::texture_format`], so the
    /// authored word is what gets allocated rather than a constant the renderer picked.
    /// A no-op visually unless the surface actually has an HDR attachment to resolve,
    /// which the stage compiler couples to this pass.
    pub fn set_tonemap_grade(
        &mut self,
        grade: Vec3,
        grade_strength: f32,
        exposure: f32,
        hdr: AttachmentFormat,
    ) {
        // A whole-surface pass of the surface it is requested in — counted like the sky, so
        // the declared-surface gate sees a tonemap requested outside a declared pass.
        self.note_draw();
        self.frame_hdr = Some(hdr.texture_format(self.config.format));
        self.frame_grade = Some((grade, grade_strength, exposure));
    }

    /// Enter the shadow-caster view of a PRODUCER stage: `light_view_proj` REPLACES the
    /// camera-derived view-projection for the surface being encoded, so the lit passes
    /// write depth from the light's point of view. This reuses the ordinary colour+depth
    /// [`Self::render_to_texture`] path (the colour attachment is wasted; a depth-only
    /// pipeline variant is a later optimization). It is the SAME matrix
    /// [`LightRig::shadow_view_proj`](crate::LightRig::shadow_view_proj) produces and the
    /// consumer's [`Self::set_shadow_source`] uploads, so producer and consumer can never
    /// disagree. Called by the scene inside the shadow surface's caster closure; reset each
    /// [`Self::begin_frame`].
    pub fn begin_shadow_view(&mut self, light_view_proj: Mat4) {
        // A whole-surface setup of the pass it is requested in — counted like the sky so
        // the declared-surface gate sees a shadow view requested outside a declared pass.
        self.note_draw();
        self.shadow_view_override = Some(light_view_proj);
    }

    /// Bind a shadow SOURCE for the surface being encoded: the producer `source` target
    /// whose depth the lit passes sample, the ONE `light_view_proj` matrix (shared with the
    /// producer camera), the sampling `bias`, and the rig slot `light` the shadow darkens.
    /// The bind + uniform upload happen in [`Self::prepare_frame`] (like the sky/fog
    /// setters), reading the source's depth through [`Self::target_depth`]. A surface that
    /// never calls this binds the 1×1 `enabled = 0` default, so its lit output is
    /// byte-identical to the no-shadow path. Called by the consuming surface's draw
    /// closure; reset each [`Self::begin_frame`].
    pub fn set_shadow_source(
        &mut self,
        source: RenderTargetHandle,
        light_view_proj: Mat4,
        bias: f32,
        light: u32,
    ) {
        self.note_draw();
        self.frame_shadow = Some(ShadowSource {
            source,
            light_view_proj,
            bias,
            light,
        });
    }

    /// The depth [`TextureView`](wgpu::TextureView) + its cache id of an offscreen render
    /// target — the ONE accessor the shadow CONSUMER reads to sample a producer surface's
    /// depth (the colour is already exposed by [`Self::target_texture`]). `None` if the
    /// handle was freed or never created.
    pub fn target_depth(&self, target: RenderTargetHandle) -> Option<(&wgpu::TextureView, u64)> {
        self.render_targets
            .get(target.0 as usize)
            .and_then(|t| t.as_ref())
            .map(|t| (&t.depth_view, t.depth_id))
    }

    /// Queue a mesh for rendering this frame.
    ///
    /// `model` is the cluster-local-to-world transform; the camera (set
    /// via [`Renderer::set_camera`]) supplies the view and projection.
    /// `options` controls fill vs wireframe and the tint.
    pub fn draw_mesh(&mut self, mesh: MeshHandle, model: Mat4, options: MeshDrawOptions) {
        self.note_draw();
        self.mesh
            .push(mesh, model, options.tint, options.wireframe, options.gloss);
    }

    /// Upload a textured 3D mesh (position + normal + UV) and return a handle. Persists
    /// across frames; drawn via [`Renderer::draw_textured_mesh`] with an albedo texture.
    /// Additive to [`Renderer::upload_mesh`] — separate storage and handle type, so the
    /// flat and textured mesh paths never cross. Reusable for any UV-mapped mesh
    /// (skinned characters now, voxel-cluster surfaces later).
    pub fn upload_textured_mesh(
        &mut self,
        vertices: &[TexturedVertex],
        indices: MeshIndices<'_>,
    ) -> TexturedMeshHandle {
        self.mesh_textured.upload(&self.device, vertices, indices)
    }

    /// Free a textured mesh, returning its slot to the reuse pool. Same semantics as
    /// [`Renderer::free_mesh`].
    pub fn free_textured_mesh(&mut self, handle: TexturedMeshHandle) {
        self.mesh_textured.free(handle);
    }

    /// Queue a textured mesh this frame, sampling `texture` as its albedo (UV-mapped)
    /// under the same lighting as [`Renderer::draw_mesh`]. `options.tint` multiplies the
    /// lit colour and `options.gloss` adds sheen; `options.wireframe` is ignored (the
    /// textured pipeline is fill-only). No PBR maps — the mesh reads as a matte dielectric
    /// (flat normal, rough, non-metal, unoccluded). For surface relief + a metal/rough
    /// specular response, use [`Renderer::draw_textured_mesh_pbr`].
    pub fn draw_textured_mesh(
        &mut self,
        mesh: TexturedMeshHandle,
        texture: TextureHandle,
        model: Mat4,
        options: MeshDrawOptions,
    ) {
        self.note_draw();
        self.mesh_textured.push(
            mesh,
            texture,
            PbrMaps::default(),
            model,
            options.tint,
            options.gloss,
            false,
        );
    }

    /// Queue a textured mesh in **soft-alpha** mode: the albedo texture's alpha *blends*
    /// (× `options.tint` alpha) instead of the default hard cutout — for clouds, ground
    /// decals, fog cards, any soft translucent textured quad. Same lighting/transform as
    /// [`Renderer::draw_textured_mesh`]; no PBR maps.
    pub fn draw_textured_mesh_soft(
        &mut self,
        mesh: TexturedMeshHandle,
        texture: TextureHandle,
        model: Mat4,
        options: MeshDrawOptions,
    ) {
        self.note_draw();
        self.mesh_textured.push(
            mesh,
            texture,
            PbrMaps::default(),
            model,
            options.tint,
            options.gloss,
            true,
        );
    }

    /// Queue a textured mesh this frame with a full PBR map set. Same as
    /// [`Renderer::draw_textured_mesh`] but additionally samples the `maps`
    /// (normal / roughness / metalness / AO) — any `None` slot uses the pipeline's
    /// default (flat normal / rough=1 / metal=0 / ao=1). The normal map perturbs the
    /// surface normal (via the vertex tangent), roughness+metalness drive a pragmatic
    /// specular highlight (reflective steel on the katana blade), and AO attenuates the
    /// ambient term. Load the map textures with [`Renderer::load_texture_linear`].
    pub fn draw_textured_mesh_pbr(
        &mut self,
        mesh: TexturedMeshHandle,
        texture: TextureHandle,
        maps: PbrMaps,
        model: Mat4,
        options: MeshDrawOptions,
    ) {
        self.note_draw();
        self.mesh_textured.push(
            mesh,
            texture,
            maps,
            model,
            options.tint,
            options.gloss,
            false,
        );
    }

    /// Upload a **bind-pose skinned mesh** (position/normal/uv + 4-influence
    /// joints/weights) to the instanced GPU-skinning pipeline and return a handle. The
    /// vertex buffer is uploaded once and never re-uploaded — the GPU deforms it from a
    /// per-instance bone palette each frame (the correct crowd/field technique, vs. the
    /// per-frame CPU-skin-and-reupload of one character). Persists across frames;
    /// additive to [`Renderer::upload_mesh`] / [`Renderer::upload_textured_mesh`]
    /// (separate storage + handle type). Draw it with [`Renderer::draw_skinned_instanced`].
    pub fn upload_skinned_mesh(
        &mut self,
        vertices: &[SkinnedVertex],
        indices: MeshIndices<'_>,
    ) -> SkinnedMeshHandle {
        self.skinned.upload(&self.device, vertices, indices)
    }

    /// Free a skinned mesh, returning its slot to the reuse pool. Same semantics as
    /// [`Renderer::free_mesh`].
    pub fn free_skinned_mesh(&mut self, handle: SkinnedMeshHandle) {
        self.skinned.free(handle);
    }

    /// Draw `mesh` as **N GPU-skinned instances in one instanced draw call** this frame.
    /// `models[i]` is instance `i`'s model→world transform; `palettes` is the flat
    /// concatenation of every instance's bone palette (instance `i`'s bone `b` at
    /// `i*bone_count + b`) — exactly what `flicker-skeletal`'s `skin::palette` produces
    /// per instance, so `palettes.len()` must equal `models.len() * bone_count`. The
    /// palettes + per-instance transforms upload to storage buffers now (grown as needed)
    /// and draw in the opaque pass under the same camera + scene lighting as
    /// [`Renderer::draw_mesh`]. **One skinned mesh per frame** — a second call this frame
    /// replaces the queued draw (the field-viewer's one-character-many-instances shape).
    /// A no-op with zero instances or a zero bone count.
    pub fn draw_skinned_instanced(
        &mut self,
        mesh: SkinnedMeshHandle,
        models: &[Mat4],
        palettes: &[Mat4],
        bone_count: u32,
    ) {
        self.note_draw();
        self.skinned.draw_instanced(
            &self.device,
            &self.queue,
            mesh,
            models,
            palettes,
            bone_count,
        );
    }

    /// Draw a wireframe axis-aligned bounding box this frame. Lives in
    /// 3D space at world coordinates `[min, max]`, drawn as the 12
    /// edges of the box at the given RGBA color. Uses the immediate-
    /// mode line-list pipeline, depth-tested against the 3D scene
    /// (lines occluded by mesh in front of them; lines don't write
    /// depth so they don't occlude later draws).
    ///
    /// Submitted immediate-mode — no upload, no handle, no
    /// persistence. Cheap (12 segments → 24 vertices). Multiple boxes
    /// per frame are fine.
    pub fn draw_bounding_box(&mut self, min: Vec3, max: Vec3, color: [f32; 4]) {
        self.note_draw();
        self.lines.push_box(min, max, color);
    }

    /// Draw an arbitrary list of line segments this frame, all tinted
    /// the same RGBA color. Uses the same line-list pipeline as
    /// [`Renderer::draw_bounding_box`] — depth-tested against the 3D
    /// scene but does not write depth. Immediate-mode, no handle, no
    /// persistence.
    ///
    /// Each `(a, b)` entry is one segment in world coordinates. The
    /// per-frame budget is governed by GPU bandwidth and the pipeline's
    /// vertex buffer growth strategy; tens of thousands of segments
    /// per frame are comfortable on modern hardware.
    pub fn draw_lines(&mut self, segments: &[(Vec3, Vec3)], color: [f32; 4]) {
        self.note_draw();
        for &(a, b) in segments {
            self.lines.push_segment(a, b, color);
        }
    }

    /// Like [`Self::draw_lines`] but drawn ON TOP of the 3D scene (depth test disabled),
    /// so the segments show through opaque geometry — for a skeleton overlay laid over the
    /// mesh, or other debug gizmos you want visible regardless of occlusion.
    pub fn draw_lines_overlay(&mut self, segments: &[(Vec3, Vec3)], color: [f32; 4]) {
        self.note_draw();
        for &(a, b) in segments {
            self.lines_overlay.push_segment(a, b, color);
        }
    }

    /// Queue a camera-facing world-space billboard this frame.
    ///
    /// `world_position` is the quad centre in world coordinates; `world_size`
    /// is its full width/height in world units. The quad always faces the
    /// camera (oriented from the camera's right/up basis) and stays a
    /// constant world size. `uv_min`/`uv_max` select a region of `texture`
    /// (full quad is `(0,0)`–`(1,1)`); `color` tints the sampled texel.
    /// Depth-tested against the 3D scene, so terrain in front occludes it.
    pub fn draw_billboard(
        &mut self,
        texture: TextureHandle,
        world_position: Vec3,
        world_size: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [f32; 4],
    ) {
        self.note_draw();
        self.billboard
            .push(texture, world_position, world_size, uv_min, uv_max, color);
    }

    /// Queue a camera-facing world-space billboard with **additive** blending — for soft
    /// glows (star bloom, dust clouds, impact flashes). Same arguments as
    /// [`Self::draw_billboard`], but the sampled texel is *added* to the target (scaled by
    /// its alpha) and **no depth is written**, so overlapping glows stack into a halo
    /// instead of clipping each other into hard cutouts. Drawn after all alpha billboards.
    pub fn draw_billboard_additive(
        &mut self,
        texture: TextureHandle,
        world_position: Vec3,
        world_size: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [f32; 4],
    ) {
        self.note_draw();
        self.billboard
            .push_additive(texture, world_position, world_size, uv_min, uv_max, color);
    }

    /// Create an offscreen render target of `width × height` and return its handle. Its
    /// colour texture is registered in the texture store, so [`Self::target_texture`] hands
    /// you a [`TextureHandle`] to draw it with (sprite / billboard / mesh). Draw a sub-scene
    /// into it with [`Self::render_to_texture`]. Uses the swapchain colour format + a private
    /// `Depth32Float`, so every 3D/2D pipeline renders into it unchanged.
    pub fn create_render_target(&mut self, width: u32, height: u32) -> RenderTargetHandle {
        let target = self.make_render_target(width, height);
        RenderTargetHandle(pool_alloc(
            &mut self.render_targets,
            &mut self.free_target_slots,
            target,
        ))
    }

    /// Build a fresh offscreen [`RenderTarget`] at `width × height`: a colour texture
    /// in the swapchain format (registered as a sampleable [`TextureHandle`]) plus a
    /// private `Depth32Float`. Shared by [`Self::create_render_target`] and
    /// [`Self::resize_render_target`].
    fn make_render_target(&mut self, width: u32, height: u32) -> RenderTarget {
        let w = width.max(1);
        let h = height.max(1);
        let color_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flicker.render_target.color"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let tex = LoadedTexture::from_view(
            &self.device,
            &self.sprite.sampler,
            &self.sprite.texture_bind_group_layout,
            color_tex,
            color_view,
            (w, h),
        );
        let color = self.register_texture(tex);
        let (depth_texture, depth_view) = create_depth_view(&self.device, w, h);
        let depth_id = self.next_depth_id;
        self.next_depth_id += 1;
        RenderTarget {
            color,
            depth_view,
            depth_texture,
            depth_id,
            // Allocated lazily on the first HDR frame (see `ensure_target_hdr`), never at
            // creation — so a target that never goes HDR (every shipped target in S3a)
            // costs no float texture.
            hdr: None,
            size: Vec2::new(w as f32, h as f32),
            // A fresh target has never been drawn — so a resize (which builds a fresh one)
            // clears the poster, exactly the invalidation the old caches hand-rolled.
            drawn: false,
            last_render: 0.0,
        }
    }

    /// Build a fresh HDR colour attachment at `width × height` in `format` — the format the
    /// stage's declared `hdr` attachment names (see [`Self::set_tonemap_grade`]), never a
    /// constant chosen here — with a new id from the shared depth/HDR counter. Its usage
    /// carries both RENDER_ATTACHMENT (the lit passes draw into it) and TEXTURE_BINDING
    /// (the tonemap pass samples it).
    fn make_hdr_color(&mut self, width: u32, height: u32, format: wgpu::TextureFormat) -> HdrColor {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flicker.hdr.color"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let id = self.next_depth_id;
        self.next_depth_id += 1;
        HdrColor { texture, view, id }
    }

    /// Build the two half-res bloom scratch targets at `w × h` ([`crate::HDR_FORMAT`], both
    /// RENDER_ATTACHMENT — the bright/blur passes draw them — and TEXTURE_BINDING — the
    /// blur/composite sample them). Renderer-owned scratch, never registered in the texture
    /// store (the bloom never composites them directly; the composite adds back into the HDR).
    fn make_bloom_scratch(&self, w: u32, h: u32) -> BloomScratch {
        let make = |label: &str| {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w.max(1),
                    height: h.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        };
        let (a_texture, a_view) = make("flicker.bloom.scratch.a");
        let (b_texture, b_view) = make("flicker.bloom.scratch.b");
        BloomScratch {
            a_texture,
            a_view,
            b_texture,
            b_view,
            size: (w.max(1), h.max(1)),
        }
    }

    /// Ensure the half-res bloom scratch exists at `bw × bh` (rebuilding + rebinding it only on
    /// a size change, so a steady frame allocates nothing — the [`Self::make_hdr_color`]
    /// discipline) and upload this frame's bloom uniform. Called from the surface-encode setup
    /// (`render_to_texture` / `end_frame`) right after the surface HDR is bound, once the frame
    /// raised a `bloom` pass.
    fn ensure_bloom(
        &mut self,
        bw: u32,
        bh: u32,
        threshold: f32,
        knee: f32,
        intensity: f32,
        radius: f32,
    ) {
        let (bw, bh) = (bw.max(1), bh.max(1));
        if self
            .bloom_scratch
            .as_ref()
            .is_none_or(|s| s.size != (bw, bh))
        {
            let scratch = self.make_bloom_scratch(bw, bh);
            self.bloom
                .bind_scratch(&self.device, &scratch.a_view, &scratch.b_view);
            self.bloom_scratch = Some(scratch);
        }
        self.bloom.set_uniform(
            &self.queue,
            BloomUniform::new(bw, bh, threshold, knee, intensity, radius),
        );
    }

    /// The sampleable colour [`TextureHandle`] of a render target — draw it as a sprite,
    /// billboard, or mesh texture. `None` if the handle was freed.
    pub fn target_texture(&self, target: RenderTargetHandle) -> Option<TextureHandle> {
        self.render_targets
            .get(target.0 as usize)
            .and_then(|t| t.as_ref())
            .map(|t| t.color)
    }

    /// Free an offscreen render target: return its slot to the pool **and** free its
    /// colour texture slot — closing the append-only leak (before, only the target
    /// handle was reclaimed; the colour texture lived forever). Mirrors
    /// [`Self::free_mesh`]; safe to call the same frame the target was last sampled
    /// (wgpu defers the GPU free). A handle already freed (or never created) is ignored.
    /// Afterwards the handle and any [`TextureHandle`] from [`Self::target_texture`]
    /// are stale — do not reuse them.
    pub fn free_render_target(&mut self, target: RenderTargetHandle) {
        if let Some(rt) = pool_free(
            &mut self.render_targets,
            &mut self.free_target_slots,
            target.0,
        ) {
            self.free_texture(rt.color);
            self.volumetric.forget(rt.depth_id);
            self.ground_fog.forget(rt.depth_id);
            // A shadow CONSUMER may have cached this target's depth — drop it too.
            self.shadow.forget(rt.depth_id);
            if let Some(hdr) = &rt.hdr {
                self.tonemap.forget(hdr.id);
                self.bloom.forget(hdr.id);
            }
        }
    }

    /// Resize an existing target in place: the [`RenderTargetHandle`] stays valid, but
    /// its colour texture is rebuilt (re-fetch via [`Self::target_texture`] after). A
    /// no-op if the size is unchanged or the handle is unknown/freed. Frees the old
    /// colour slot. Gives panels a way to track the window (the view targets were
    /// created once at the initial size — paperdoll's resize gap).
    pub fn resize_render_target(&mut self, target: RenderTargetHandle, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        match self.render_targets.get(target.0 as usize) {
            // Unchanged size → skip; rebuilding every frame would churn GPU textures.
            Some(Some(rt)) if rt.size == Vec2::new(w as f32, h as f32) => return,
            Some(Some(_)) => {}
            _ => return, // unknown or freed handle
        }
        let (old_color, old_depth_id, old_hdr_id) = {
            let rt = self.render_targets[target.0 as usize]
                .as_ref()
                .expect("checked present above");
            (rt.color, rt.depth_id, rt.hdr.as_ref().map(|h| h.id))
        };
        // Build the replacement BEFORE freeing the old colour, so the new colour never
        // aliases the old slot. The fresh target has `hdr: None`; it re-allocates at the new
        // size on its next HDR frame, and the old HDR bind group is dropped below.
        let fresh = self.make_render_target(w, h);
        self.render_targets[target.0 as usize] = Some(fresh);
        self.free_texture(old_color);
        self.volumetric.forget(old_depth_id);
        self.ground_fog.forget(old_depth_id);
        self.shadow.forget(old_depth_id);
        if let Some(id) = old_hdr_id {
            self.tonemap.forget(id);
            self.bloom.forget(id);
        }
    }

    /// THE per-surface liveness decision, made once per frame at each
    /// [`crate::FrameGraph::surface`] target by the one place holding both the clock and the
    /// target. Computes `since_last = frame_clock - last_render`, asks [`Rate::renders`]
    /// (feeding the target's own [`RenderTarget::drawn`] flag), and on a `true` stamps
    /// `drawn` / `last_render` so the poster rule (render once, then never) and the `hz` rule
    /// (re-render once a period) ride the clock. An unknown / freed target answers `false` —
    /// there is nothing to render, and the composite skips it gracefully. This decision living
    /// here is what let the three hand-rolled poster caches be deleted.
    pub(crate) fn surface_should_render(
        &mut self,
        target: RenderTargetHandle,
        rate: Rate,
        dirty: bool,
    ) -> bool {
        let clock = self.frame_clock;
        let Some(rt) = self
            .render_targets
            .get_mut(target.0 as usize)
            .and_then(|t| t.as_mut())
        else {
            return false;
        };
        let since_last = (clock - rt.last_render) as f32;
        if rate.renders(rt.drawn, since_last, dirty) {
            rt.drawn = true;
            rt.last_render = clock;
            true
        } else {
            false
        }
    }

    /// Render a **self-contained sub-scene** into an offscreen `target`, clearing it to
    /// `clear` (RGBA 0..1; `[0.0; 4]` = a transparent cut-out). Inside `f`, call the normal
    /// `set_camera` / `set_scene` / `draw_*` methods — they queue the sub-scene, which is
    /// drawn into the target's colour texture and submitted immediately; the result is then
    /// sampleable via [`Self::target_texture`].
    ///
    /// The offscreen driver behind [`crate::FrameGraph::target`], `pub(crate)` because the
    /// graph is the one caller: `execute` runs every offscreen pass FIRST, while the shared
    /// per-frame draw queues (which this resets on entry and exit) are still empty, so no
    /// main-frame geometry can be caught and dropped. The depth-sampling passes (the
    /// volumetric disk, the ground fog) sample THIS target's own depth, so a sub-scene may
    /// use them exactly like the main frame does.
    pub(crate) fn render_to_texture(
        &mut self,
        target: RenderTargetHandle,
        clear: [f64; 4],
        f: impl FnOnce(&mut Renderer),
    ) {
        let Some(rt) = self
            .render_targets
            .get(target.0 as usize)
            .and_then(|t| t.as_ref())
        else {
            return;
        };
        let (size, color) = (rt.size, rt.color);

        self.begin_frame(); // fresh sub-frame queues
        self.in_pass = true;
        f(self); // the caller queues the sub-scene
        self.in_pass = false;
        if let Err(e) = self.prepare_frame(size) {
            tracing::warn!("render_to_texture: prepare failed: {e:?}");
            self.begin_frame();
            return;
        }
        // The depth-sampling passes read THIS target's depth — not the window's.
        {
            let rt = self.render_targets[target.0 as usize]
                .as_ref()
                .expect("render target present");
            self.volumetric
                .bind_depth(&self.device, rt.depth_id, &rt.depth_view);
            self.ground_fog
                .bind_depth(&self.device, rt.depth_id, &rt.depth_view);
        }
        // If this sub-scene's recipe went HDR, ensure THIS target owns an HDR attachment
        // (allocated once, reused after) and bind the tonemap to it.
        if let Some(format) = self.frame_hdr {
            let ti = target.0 as usize;
            let need = self.render_targets[ti]
                .as_ref()
                .is_some_and(|rt| rt.hdr.is_none());
            if need {
                let hdr =
                    self.make_hdr_color(size.x.max(1.0) as u32, size.y.max(1.0) as u32, format);
                if let Some(rt) = self.render_targets[ti].as_mut() {
                    rt.hdr = Some(hdr);
                }
            }
            if let Some(hdr) = self.render_targets[ti]
                .as_ref()
                .and_then(|rt| rt.hdr.as_ref())
            {
                self.tonemap.bind_hdr(&self.device, hdr.id, &hdr.view);
                // Bloom's bright pass reads the SAME surface HDR (cached per HDR id).
                if self.frame_bloom.is_some() {
                    self.bloom.bind_bright(&self.device, hdr.id, &hdr.view);
                }
            }
            // Ensure the half-res bloom scratch + upload the uniform — own `&mut` borrow, after
            // the HDR borrow above ends.
            if let Some((t, k, i, r)) = self.frame_bloom {
                let bw = (size.x.max(1.0) as u32) / 2;
                let bh = (size.y.max(1.0) as u32) / 2;
                self.ensure_bloom(bw, bh, t, k, i, r);
            }
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("flicker.render_target_encoder"),
            });
        {
            let color_view = &self.textures[color.0 as usize]
                .as_ref()
                .expect("render target colour texture present")
                .view;
            let depth_view = &self.render_targets[target.0 as usize]
                .as_ref()
                .expect("render target present")
                .depth_view;
            // This target's HDR attachment (view + id), present only when it went HDR.
            let hdr = self.render_targets[target.0 as usize]
                .as_ref()
                .and_then(|rt| rt.hdr.as_ref())
                .map(|h| (&h.view, h.id));
            let surface = format!("render target {}", target.0);
            if let Err(e) =
                self.encode_passes(&mut encoder, &surface, color_view, depth_view, clear, hdr)
            {
                tracing::warn!("render_to_texture: encode failed: {e:?}");
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        self.begin_frame(); // leave the queues clean for the main frame
    }

    /// Upload all per-frame camera / scene uniforms + buffered geometry/text for a render of
    /// pixel size `size` (drives aspect + text layout). Shared by the swapchain frame
    /// ([`Self::end_frame`]) and offscreen targets ([`Self::render_to_texture`]).
    fn prepare_frame(&mut self, size: Vec2) -> Result<()> {
        let camera_pos = self.camera.map(|c| c.position).unwrap_or(Vec3::ZERO);
        // A shadow PRODUCER writes depth from the light's view; there is no sky/fog/billboard
        // in a caster pass, so the override short-circuits that whole block.
        self.sky_this_frame =
            self.draw_sky && self.camera.is_some() && self.shadow_view_override.is_none();
        if let Some(light_view_proj) = self.shadow_view_override {
            // ONE camera upload — the light's view-projection — for the lit pipelines that
            // render the casters into the shadow depth. Same matrix the consumer samples.
            self.frame.set_camera_matrix(&self.queue, light_view_proj);
        } else if let Some(cam) = self.camera {
            let aspect = if size.y > 0.0 { size.x / size.y } else { 1.0 };
            let view_projection = cam.view_projection(aspect);
            // ONE camera upload for every lit pipeline (they share `frame`'s buffer).
            self.frame.set_camera_matrix(&self.queue, view_projection);
            self.billboard
                .set_camera(&self.queue, cam.view(), view_projection);
            let inv_vp = view_projection.inverse();
            if self.sky_this_frame {
                self.sky.set_uniform(
                    &self.queue,
                    scene_to_sky_uniform(&self.scene, inv_vp, camera_pos),
                );
            }
            if let Some(params) = &self.volumetric_params {
                self.volumetric.set_uniform(
                    &self.queue,
                    VolumetricDiskUniform::from_params(params, inv_vp, camera_pos),
                );
            }
            if let Some(fog) = &self.ground_fog_params {
                self.ground_fog.set_uniform(
                    &self.queue,
                    GroundFogUniform::from_params(fog, inv_vp, camera_pos),
                );
            }
            if let Some(water) = &self.frame_water {
                // The projected-grid VS casts a ray from `camera_pos` through the screen-space
                // grid onto the sea plane, unprojecting with the same `inv_vp` the sky/fog use;
                // it re-projects with the shared @group(0) camera, and the FS reads the sun out
                // of the Scene uniform. The LIVE rig rides along so the water is
                // ENVIRONMENT-lit: it mirrors the same `sky_zenith`/`sky_horizon` palette the
                // sky pass paints (`scene_to_sky_uniform`, just above, reads the same fields)
                // and lights its body by the same `ambient` — so a day/night cycle rewriting
                // the rig moves the sea with it, with no second door into the water's params.
                self.water.set_uniform(
                    &self.queue,
                    WaterMeshUniform::from_params(water, inv_vp, camera_pos, &self.scene),
                );
            }
        }
        // ONE lighting upload for every lit pipeline.
        self.frame
            .set_scene_uniform(&self.queue, rig_to_uniform(&self.scene, camera_pos));
        // The shadow @group binding for this surface: a CONSUMER binds the named producer's
        // depth + an `enabled` uniform; EVERY other surface binds the 1×1 `enabled = 0`
        // default, so its lit shaders multiply by a shadow factor of exactly 1.0 —
        // byte-identical to the no-shadow path. Texel = 1 / shadow-map width (square map).
        match self.frame_shadow {
            Some(src) => {
                match self
                    .render_targets
                    .get(src.source.0 as usize)
                    .and_then(|t| t.as_ref())
                {
                    Some(rt) => {
                        self.shadow
                            .bind_shadow(&self.device, rt.depth_id, &rt.depth_view);
                        let texel = if rt.size.x > 0.0 {
                            1.0 / rt.size.x
                        } else {
                            0.0
                        };
                        self.shadow.set_uniform(
                            &self.queue,
                            ShadowUniform::enabled(src.light_view_proj, src.bias, texel, src.light),
                        );
                    }
                    // The named source was freed/never rendered — fall back to the default
                    // so the lit passes still bind a valid group (no shadow, not a crash).
                    None => {
                        self.shadow.bind_default();
                        self.shadow
                            .set_uniform(&self.queue, ShadowUniform::disabled());
                    }
                }
            }
            None => {
                self.shadow.bind_default();
                self.shadow
                    .set_uniform(&self.queue, ShadowUniform::disabled());
            }
        }
        // Pass-owned grade → the tonemap uniform, uploaded once per HDR frame.
        if let Some((tint, strength, exposure)) = self.frame_grade {
            self.tonemap
                .set_uniform(&self.queue, GradeUniform::new(tint, strength, exposure));
        }

        self.triangle.prepare(&self.device, &self.queue);
        self.ui.prepare(&self.device, &self.queue);
        self.sprite.prepare(&self.device, &self.queue);
        self.mesh.prepare(&self.device, &self.queue);
        self.mesh_textured
            .prepare(&self.device, &self.queue, &self.textures);
        self.lines.prepare(&self.device, &self.queue);
        self.lines_overlay.prepare(&self.device, &self.queue);
        self.billboard.prepare(&self.device, &self.queue);
        self.text
            .prepare(
                &self.device,
                &self.queue,
                size.x.max(1.0) as u32,
                size.y.max(1.0) as u32,
            )
            .context("text prepare failed")?;
        Ok(())
    }

    /// Encode a surface's passes into `encoder`, targeting `color_view` (the sRGB colour) +
    /// `depth_view`, clearing colour to `clear`. `hdr` is the surface's HDR attachment
    /// (view + cache id) when it has one. Immutable — reads the prepared pipeline state — so
    /// it serves both the swapchain view and an offscreen target view.
    ///
    /// When `frame_hdr` and an HDR attachment are both present, the lit passes render into
    /// the HDR colour, the tonemap resolves it into the sRGB colour, and the 2D overlays
    /// draw on top of that (crisp, never through HDR). Otherwise — every shipped frame in
    /// S3a — it is the byte-identical pre-HDR path: the lit passes and the 2D overlays share
    /// the same two passes into the sRGB colour, exactly as before.
    fn encode_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface: &str,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        clear: [f64; 4],
        hdr: Option<(&wgpu::TextureView, u64)>,
    ) -> Result<()> {
        // A frame that RAISED the tonemap but has no HDR attachment to resolve renders the
        // lit passes straight into the sRGB colour — plausible pixels, silently un-graded
        // and hard-clipped. Name the surface rather than degrade quietly (rule 4BB12A75),
        // in the same voice as the declared-surface gate in `end_frame`.
        if self.frame_hdr.is_some() && hdr.is_none() {
            tracing::warn!(
                "encode_passes: `{surface}` raised the tonemap_grade pass but owns NO hdr \
                 attachment — the lit passes fall back to the sRGB colour, unresolved and \
                 un-graded; declare the surface's `hdr` attachment in its stage"
            );
        }
        let use_hdr = self.frame_hdr.is_some() && hdr.is_some();
        // The lit passes target the HDR attachment when this frame resolves through the
        // tonemap; otherwise the sRGB colour directly (`col == color_view`, byte-identical).
        let (col, tc) = match hdr {
            Some((hdr_view, _)) if use_hdr => (hdr_view, TargetColor::Hdr),
            _ => (color_view, TargetColor::Srgb),
        };

        // Pass 1 — opaque scene: sky, 3D meshes (write depth), lines, world-space billboards.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.opaque_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: col,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0],
                            g: clear[1],
                            b: clear[2],
                            a: clear[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if self.sky_this_frame {
                self.sky.render(&mut pass, tc);
            }
            self.mesh
                .render(&mut pass, &self.frame, &self.shadow, &self.meshes, tc);
            self.mesh_textured
                .render(&mut pass, &self.frame, &self.shadow, &self.textures, tc);
            self.skinned
                .render(&mut pass, &self.frame, &self.shadow, tc);
            // The animated water MESH — real depth-writing geometry (occludes + is occluded),
            // premultiplied over the lit terrain, writing the surface's colour (hdr when the
            // surface resolves through the tonemap, so the sun specular survives to bloom).
            // Drawn here so the solid world is beneath it while the debug lines / billboards
            // still layer on top. Gated on a water pass + a camera; the grid is built lazily
            // in `set_water`.
            if self.frame_water.is_some() && self.camera.is_some() {
                if let Some(grid) = self.water_grid {
                    self.water
                        .render(&mut pass, &self.frame, &self.shadow, &self.meshes, grid, tc);
                }
            }
            self.lines.render(&mut pass, &self.frame, tc);
            // Overlay lines last in the opaque pass — depth-Always, so they sit on top of
            // the mesh (skeleton-over-mesh debug view).
            self.lines_overlay.render(&mut pass, &self.frame, tc);
            self.billboard.render(&mut pass, &self.textures, tc);
        }

        // Pass 2 — depth-aware passes (read depth) in recipe order. In the non-HDR path the
        // 2D overlays share this pass exactly as before; in the HDR path they move after the
        // tonemap resolve, because this pass targets the HDR colour.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.overlay_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: col,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: None, // read-only → the depth passes may sample it in-pass
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.encode_depth_passes(&mut pass, tc);
            if !use_hdr {
                self.encode_2d(&mut pass)?;
            }
        }

        // HDR only: resolve the HDR colour into the sRGB colour (tonemap + grade), then draw
        // the 2D overlays crisply on top (they never went through HDR).
        if use_hdr {
            // BLOOM — after everything wrote `hdr` (the opaque + overlay passes above) and
            // BEFORE the tonemap reads it: extract the bright HDR highlights, blur them, and add
            // the glow back into `hdr`. This physical slot realizes bloom's DERIVED `pass_order`
            // position (after every hdr writer, before `tonemap_grade`) — the same "gated bool
            // in the matching slot" shape the tonemap/water/fog passes use. A no-op unless a
            // `bloom` pass ran AND the half-res scratch is built; then the tonemap resolves the
            // bloomed `hdr`.
            if self.frame_bloom.is_some() {
                if let (Some((hdr_view, _)), Some(scratch)) = (hdr, self.bloom_scratch.as_ref()) {
                    self.bloom
                        .encode(encoder, hdr_view, &scratch.a_view, &scratch.b_view);
                }
            }
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("flicker.tonemap_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // The fullscreen resolve overwrites every pixel — Load is fine.
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    // A full-frame resolve carries no depth (the tonemap pipeline has none).
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.tonemap.render(&mut pass);
            }
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("flicker.overlay_2d_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: color_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: depth_view,
                        depth_ops: None,
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.encode_2d(&mut pass)?;
            }
        }
        Ok(())
    }

    /// The depth-aware passes (volumetric disk, ground fog) in this frame's [`depth_plan`]
    /// ([`Self::depth_plan`]), each still gated on its params + a camera — so the pixels
    /// match the fixed-order encoder for every surface that runs one of them. An empty plan
    /// (a direct-setter caller outside a recipe, or a frame with neither) falls back to the
    /// fixed legacy `[Volumetric, GroundFog]`, which with both params unset draws nothing —
    /// byte-identical to before.
    fn encode_depth_passes<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, tc: TargetColor) {
        let cam = self.camera.is_some();
        const LEGACY: [DepthPass; 2] = [DepthPass::Volumetric, DepthPass::GroundFog];
        let order: &[DepthPass] = if self.depth_plan.is_empty() {
            &LEGACY
        } else {
            &self.depth_plan
        };
        for dp in order {
            match dp {
                DepthPass::Volumetric => {
                    if self.volumetric_params.is_some() && cam {
                        self.volumetric.render(pass, tc);
                    }
                }
                DepthPass::GroundFog => {
                    if self.ground_fog_params.is_some() && cam {
                        self.ground_fog.render(pass, tc);
                    }
                }
            }
        }
    }

    /// The 2D overlays in painter's order — UI panels, then triangles, sprites, and text on
    /// top, all within each ascending layer. Always encoded into the sRGB colour, last.
    fn encode_2d<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> Result<()> {
        let mut layers: Vec<f32> = Vec::new();
        layers.extend(self.ui.layers());
        layers.extend(self.triangle.layers());
        layers.extend(self.sprite.layers());
        layers.extend(self.text.layers());
        layers.sort_by(f32::total_cmp);
        layers.dedup();
        for &layer in &layers {
            // UI panels first (backgrounds), then triangles, sprites (rects),
            // and text on top — all within the same layer.
            self.ui.render_layer(pass, layer);
            self.triangle.render_layer(pass, layer);
            self.sprite.render_layer(pass, layer, &self.textures);
            self.text
                .render_layer(pass, layer)
                .context("text render failed")?;
        }
        Ok(())
    }

    /// Encode and submit the swapchain frame. Recoverable surface losses reconfigure and
    /// skip the frame.
    pub fn end_frame(&mut self) -> Result<()> {
        // ── THE DECLARED-SURFACE GATE ──
        // Any draw — 2D or 3D — queued outside every declared pass still renders (it lands
        // in the main frame's queues), but it is the immediate-mode path the frame graph
        // retired: such content belongs in a `FrameGraph::target` (an offscreen surface),
        // a `FrameGraph::root` (the full-window screen surface), or a `FrameGraph::overlay`
        // (the screen surface's final 2D — a HUD replay). Reported on the first stray frame
        // and every 300th after, so a steady violation stays visible without flooding the log.
        if self.stray_draws > 0 {
            self.stray_frames += 1;
            if self.stray_frames % 300 == 1 {
                tracing::warn!(
                    "end_frame: {} draw(s) were queued OUTSIDE a declared pass this frame — \
                     declare full-window content with FrameGraph::root, an offscreen picture \
                     with FrameGraph::target, and a HUD/final 2D with FrameGraph::overlay",
                    self.stray_draws
                );
            }
            self.stray_draws = 0;
        } else {
            self.stray_frames = 0;
        }
        // The main frame samples the window's depth (an offscreen pass may have bound its
        // own target's depth since the last frame).
        self.volumetric
            .bind_depth(&self.device, self.main_depth_id, &self.depth_view);
        self.ground_fog
            .bind_depth(&self.device, self.main_depth_id, &self.depth_view);
        self.prepare_frame(self.screen)?;

        // If the root surface went HDR this frame, ensure the window owns an HDR attachment
        // (allocated once at the window size, reused after) and bind the tonemap to it.
        if let Some(format) = self.frame_hdr {
            if self.hdr_color.is_none() {
                let (w, h) = (self.screen.x.max(1.0) as u32, self.screen.y.max(1.0) as u32);
                self.hdr_color = Some(self.make_hdr_color(w, h, format));
            }
            if let Some(hdr) = self.hdr_color.as_ref() {
                self.tonemap.bind_hdr(&self.device, hdr.id, &hdr.view);
                // Bloom's bright pass reads the SAME window HDR (cached per HDR id).
                if self.frame_bloom.is_some() {
                    self.bloom.bind_bright(&self.device, hdr.id, &hdr.view);
                }
            }
            // Ensure the half-res bloom scratch + upload the uniform — own `&mut` borrow, after
            // the HDR borrow above ends.
            if let Some((t, k, i, r)) = self.frame_bloom {
                let (w, h) = (self.screen.x.max(1.0) as u32, self.screen.y.max(1.0) as u32);
                self.ensure_bloom(w / 2, h / 2, t, k, i, r);
            }
        }

        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(e) => return Err(anyhow::anyhow!("surface acquire failed: {e:?}")),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("flicker.frame_encoder"),
            });

        // The window's HDR attachment (view + id), present only when the root went HDR.
        let hdr = self.hdr_color.as_ref().map(|h| (&h.view, h.id));
        self.encode_passes(
            &mut encoder,
            "the window",
            &view,
            &self.depth_view,
            self.clear_color,
            hdr,
        )?;

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // The atlas may want to trim itself between frames.
        self.text.atlas.trim();

        Ok(())
    }
}

/// Convert the friendly [`LightRig`] (typed lights, `Vec3` fields) into the GPU's
/// `vec4`-padded [`SceneUniform`], injecting the camera world position and packing the
/// scalars into their `.w` lanes (`fog_color.w` = density, `color_intensity.w` =
/// intensity, `position_kind.w` = kind, `direction_radius.w` = radius). THE one
/// converter — [`SceneUniform::default`] goes through it too, so the boot look and a
/// set rig can never be two different spellings of the same rig.
pub(crate) fn rig_to_uniform(rig: &LightRig, camera_pos: Vec3) -> SceneUniform {
    let mut lights = [LightUniform::default(); crate::MAX_LIGHTS];
    for (u, l) in lights.iter_mut().zip(rig.lights.iter()) {
        let kind = match l.kind {
            crate::LightKind::Dir => 0.0,
            crate::LightKind::Point => 1.0,
            crate::LightKind::Spot => 2.0,
        };
        *u = LightUniform {
            color_intensity: [l.color.x, l.color.y, l.color.z, l.intensity],
            position_kind: [l.position.x, l.position.y, l.position.z, kind],
            direction_radius: [l.direction.x, l.direction.y, l.direction.z, l.radius],
            cone: [l.cone_inner.cos(), l.cone_outer.cos(), 0.0, 0.0],
        };
    }
    SceneUniform {
        ambient: [rig.ambient.x, rig.ambient.y, rig.ambient.z, 0.0],
        camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
        fog_color: [
            rig.fog_color.x,
            rig.fog_color.y,
            rig.fog_color.z,
            rig.fog_density,
        ],
        counts: [rig.count.min(crate::MAX_LIGHTS as u8) as u32, 0, 0, 0],
        lights,
    }
}

/// Build the procedural-sky uniform from the scene lighting plus this frame's
/// inverse view-projection and camera position. Its sun/moon ARE the rig's SLOTS 0 and
/// 1 ([`LightRig::sky_sun`] / [`LightRig::sky_moon`]) and its palette is the rig's, so
/// the sky and the lit terrain read as one atmosphere from one source — and the slots
/// this samples are exactly the ones a celestial cycle composing over the rig writes by
/// index, which is what makes the disc in the sky and the light on the ground the same
/// sun.
fn scene_to_sky_uniform(rig: &LightRig, inv_view_proj: Mat4, camera_pos: Vec3) -> SkyUniform {
    let (sun, moon) = (rig.sky_sun(), rig.sky_moon());
    let (sun_c, moon_c) = (sun.radiance(), moon.radiance());
    SkyUniform {
        inv_view_proj: inv_view_proj.to_cols_array_2d(),
        camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
        sun_dir: [sun.direction.x, sun.direction.y, sun.direction.z, 0.0],
        sun_color: [sun_c.x, sun_c.y, sun_c.z, 0.0],
        moon_dir: [moon.direction.x, moon.direction.y, moon.direction.z, 0.0],
        moon_color: [moon_c.x, moon_c.y, moon_c.z, 0.0],
        zenith: [rig.sky_zenith.x, rig.sky_zenith.y, rig.sky_zenith.z, 0.0],
        horizon: [rig.sky_horizon.x, rig.sky_horizon.y, rig.sky_horizon.z, 0.0],
        star_rotation: rig.star_rotation.to_cols_array_2d(),
    }
}

#[cfg(test)]
mod slot_pool_tests {
    use super::{pool_alloc, pool_free};

    #[test]
    fn reuses_freed_slots_before_growing() {
        let mut slots: Vec<Option<&str>> = Vec::new();
        let mut free: Vec<u32> = Vec::new();

        let a = pool_alloc(&mut slots, &mut free, "a");
        let b = pool_alloc(&mut slots, &mut free, "b");
        assert_eq!((a, b), (0, 1));
        assert_eq!(slots.len(), 2);

        // Free the first slot — its value comes back, the index is queued for reuse.
        assert_eq!(pool_free(&mut slots, &mut free, a), Some("a"));
        assert!(slots[0].is_none());

        // The next alloc REUSES slot 0 rather than growing — the leak fix.
        let c = pool_alloc(&mut slots, &mut free, "c");
        assert_eq!(c, 0);
        assert_eq!(slots.len(), 2, "reuse must not grow the pool");
        assert_eq!(slots[0], Some("c"));

        // Double-free and out-of-range are ignored (return None, no spurious reuse).
        assert_eq!(pool_free(&mut slots, &mut free, 0), Some("c"));
        assert_eq!(pool_free(&mut slots, &mut free, 0), None);
        assert_eq!(pool_free(&mut slots, &mut free, 99), None);
    }
}
