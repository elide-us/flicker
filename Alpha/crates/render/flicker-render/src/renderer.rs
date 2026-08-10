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

use anyhow::{Context, Result};
use glam::{Mat4, Vec2};
use winit::dpi::PhysicalSize;
use winit::window::{Fullscreen, Window};

use glam::Vec3;

use crate::mesh::{Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, SceneLighting};
use crate::pipeline_billboard::BillboardPipeline;
use crate::pipeline_lines::LinesPipeline;
use crate::pipeline_ground_fog::{GroundFog, GroundFogPipeline, GroundFogUniform};
use crate::pipeline_mesh::{create_depth_view, LoadedMesh, MeshPipeline, SceneUniform};
use crate::pipeline_mesh_textured::{
    PbrMaps, TexturedMeshHandle, TexturedMeshPipeline, TexturedVertex,
};
use crate::pipeline_skinned::{SkinnedMeshHandle, SkinnedMeshPipeline, SkinnedVertex};
use crate::pipeline_sky::{SkyPipeline, SkyUniform};
use crate::pipeline_sprite::SpritePipeline;
use crate::pipeline_text::TextPipeline;
use crate::pipeline_triangle::TrianglePipeline;
use crate::pipeline_ui::UiPipeline;
use crate::pipeline_volumetric::{VolumetricDisk, VolumetricDiskUniform, VolumetricPipeline};
use crate::texture::{LoadedTexture, TextureHandle};

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
    /// Pixel size (drives the sub-scene's aspect + text layout).
    size: Vec2,
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

    /// Depth attachment shared by every pipeline in the main pass. The
    /// 3D pipeline writes/tests it; the 2D pipelines neither write nor
    /// test (their depth-stencil state is "always pass, no write").
    #[allow(dead_code)]
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,

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
    /// Current frame-global lighting/atmosphere (sun/moon/ambient/fog/grade),
    /// uploaded in `end_frame`. Defaults to the pre-uniform hardcoded look, so
    /// a scene that never calls [`Renderer::set_scene`] renders unchanged.
    scene: SceneLighting,
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
    /// How many draw calls have been queued since the last [`Renderer::begin_frame`].
    /// Every `draw_*` entry point bumps it (via [`Renderer::note_draw`] — a new
    /// entry point MUST call it too). Exists for exactly one diagnosis: an
    /// offscreen pass ([`Renderer::render_to_texture`]) resets the shared queues,
    /// so main-frame draws queued BEFORE it are silently destroyed — this counter
    /// is what lets that destruction be LOUD instead.
    frame_draws: u32,

    /// Offscreen render targets, indexed by [`RenderTargetHandle`] (slot pool).
    render_targets: Vec<Option<RenderTarget>>,
    free_target_slots: Vec<u32>,
    /// Whether the sky draws in the pass currently being encoded — set by `prepare_frame`,
    /// read by `encode_passes`, so the offscreen and swapchain paths share one encode path.
    sky_this_frame: bool,
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
        let mesh = MeshPipeline::new(&device, surface_format, min_uniform_offset_alignment);
        let mesh_textured =
            TexturedMeshPipeline::new(&device, &queue, surface_format, min_uniform_offset_alignment);
        let skinned = SkinnedMeshPipeline::new(&device, &queue, surface_format);
        let lines = LinesPipeline::new(
            &device,
            surface_format,
            mesh.camera_buffer(),
            wgpu::CompareFunction::LessEqual,
        );
        // Overlay lines ignore depth (Always) — for drawing a skeleton (or other debug
        // gizmos) ON TOP of the mesh so it shows through instead of being occluded.
        let lines_overlay = LinesPipeline::new(
            &device,
            surface_format,
            mesh.camera_buffer(),
            wgpu::CompareFunction::Always,
        );
        let billboard = BillboardPipeline::new(&device, surface_format);
        let sky = SkyPipeline::new(&device, surface_format);
        let mut volumetric = VolumetricPipeline::new(&device, surface_format);
        // Bind the scene depth so the volumetric can sample it (clamp rays at bodies).
        volumetric.set_depth(&device, &depth_view);
        let mut ground_fog = GroundFogPipeline::new(&device, surface_format);
        ground_fog.set_depth(&device, &depth_view);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            screen: Vec2::new(width as f32, height as f32),
            clear_color: [0.05, 0.06, 0.08, 1.0],
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
            volumetric_params: None,
            ground_fog_params: None,
            depth_texture,
            depth_view,
            textures: Vec::new(),
            free_texture_slots: Vec::new(),
            meshes: Vec::new(),
            free_mesh_slots: Vec::new(),
            camera: None,
            scene: SceneLighting::default(),
            draw_sky: false,
            current_layer: 0.0,
            current_clip: None,
            frame_draws: 0,
            render_targets: Vec::new(),
            free_target_slots: Vec::new(),
            sky_this_frame: false,
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
        // Rebind the recreated depth view into the depth-sampling passes.
        self.volumetric.set_depth(&self.device, &self.depth_view);
        self.ground_fog.set_depth(&self.device, &self.depth_view);
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
        self.current_layer = 0.0;
        self.current_clip = None;
        self.frame_draws = 0;
    }

    /// Count one queued draw. Called by every `draw_*` entry point — see
    /// [`Self::frame_draws`] for why a new entry point must call it too.
    #[inline]
    fn note_draw(&mut self) {
        self.frame_draws += 1;
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
        self.draw_text_role(text, position, size, color, crate::FontRole::Body, false, false, -1.0, None);
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

    /// Set the frame-global lighting & atmosphere (sun/moon directional
    /// lights, ambient, fog, colour grade) for the 3D mesh pass. Cached and
    /// uploaded in [`Renderer::end_frame`], mirroring [`Renderer::set_camera`];
    /// the camera position is injected automatically (for fog distance), so
    /// callers supply only the lights/ambient/fog/grade. Persists until the
    /// next call — set it once per frame. A scene that never calls this renders
    /// with the default (former hardcoded) lighting.
    pub fn set_scene(&mut self, scene: SceneLighting) {
        self.scene = scene;
    }

    /// Request the procedural sky behind the 3D scene this frame. Fakes
    /// atmospheric scattering from the current [`SceneLighting`] (sun/moon
    /// directions + colours and the `sky_zenith`/`sky_horizon` palette) — a
    /// fullscreen pass drawn first, so terrain, lines, billboards, and the 2D
    /// UI all layer on top. Per-frame, like `draw_mesh`: call it each frame
    /// you want a sky; omit it (menus/loading) to keep the flat `clear_color`.
    /// A no-op unless a camera is also set this frame (it needs the view ray).
    pub fn draw_sky(&mut self) {
        self.note_draw();
        self.draw_sky = true;
    }

    /// Draw a **volumetric dust disk** this frame: a raymarched protoplanetary-disk cloud
    /// (dark dust + warm star-lit inner glow) whose density dissipates inside-out with
    /// `params.formation` and carves gaps around `params.bodies`. Drawn just after the sky and
    /// composited over it. Per-frame, like `draw_sky`; a no-op unless a camera is set (it needs
    /// the view ray).
    pub fn set_volumetric_disk(&mut self, params: VolumetricDisk) {
        self.volumetric_params = Some(params);
    }

    /// Draw a **volumetric ground fog** this frame: a raymarched horizontal fog slab of animated
    /// drifting noise, depth-aware (occluded by geometry) and correctly self-compositing (no
    /// billboard/quad layering artifacts). Composited over the scene in the overlay pass, just
    /// after the volumetric disk. Per-frame, like [`Renderer::draw_sky`]; a no-op unless a camera
    /// is set (it needs the view ray). See [`GroundFog`] for the band / colour / wind params.
    pub fn set_ground_fog(&mut self, params: GroundFog) {
        self.ground_fog_params = Some(params);
    }

    /// Queue a mesh for rendering this frame.
    ///
    /// `model` is the cluster-local-to-world transform; the camera (set
    /// via [`Renderer::set_camera`]) supplies the view and projection.
    /// `options` controls fill vs wireframe and the tint.
    pub fn draw_mesh(&mut self, mesh: MeshHandle, model: Mat4, options: MeshDrawOptions) {
        self.note_draw();
        self.mesh.push(mesh, model, options.tint, options.wireframe, options.gloss);
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
        self.mesh_textured
            .push(mesh, texture, maps, model, options.tint, options.gloss, false);
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
        self.skinned
            .draw_instanced(&self.device, &self.queue, mesh, models, palettes, bone_count);
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
        RenderTarget {
            color,
            depth_view,
            depth_texture,
            size: Vec2::new(w as f32, h as f32),
        }
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
        if let Some(rt) = pool_free(&mut self.render_targets, &mut self.free_target_slots, target.0)
        {
            self.free_texture(rt.color);
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
        let old_color = self.render_targets[target.0 as usize]
            .as_ref()
            .expect("checked present above")
            .color;
        // Build the replacement BEFORE freeing the old colour, so the new colour never
        // aliases the old slot.
        let fresh = self.make_render_target(w, h);
        self.render_targets[target.0 as usize] = Some(fresh);
        self.free_texture(old_color);
    }

    /// Render a **self-contained sub-scene** into an offscreen `target`, clearing it to
    /// `clear` (RGBA 0..1; `[0.0; 4]` = a transparent cut-out). Inside `f`, call the normal
    /// `set_camera` / `set_scene` / `draw_*` methods — they queue the sub-scene, which is
    /// drawn into the target's colour texture and submitted immediately; the result is then
    /// sampleable via [`Self::target_texture`].
    ///
    /// **Call before queuing your main-frame draws:** this resets the per-frame draw queues
    /// on entry and exit, so a `render_to_texture` after you've queued main-frame geometry
    /// would drop it (offscreen passes conventionally run first anyway). A volumetric disk in
    /// an offscreen sub-scene is not supported (it samples the main depth buffer).
    pub fn render_to_texture(
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

        // ── THE ORDERING CONTRACT, ENFORCED LOUDLY ──
        // An offscreen pass is a PRE-pass: it borrows the shared per-frame queues,
        // so everything queued for the main frame before this point is about to be
        // destroyed by the `begin_frame` below. That used to happen SILENTLY — a
        // scene that declared its RTT pass after its main draws simply lost them
        // and shipped a part-blank frame with no explanation. The draws are still
        // lost (preserving them means stashing every pipeline's queue AND the
        // camera/scene state — a real redesign, tracked in MCP), but now the frame
        // says why, and names the fix.
        if self.frame_draws > 0 {
            tracing::warn!(
                "render_to_texture: {} main-frame draw(s) were queued before this \
                 offscreen pass and are being DROPPED — declare offscreen passes \
                 (FrameGraph::execute) before any main-frame draw",
                self.frame_draws
            );
        }

        self.begin_frame(); // fresh sub-frame queues
        f(self); // the caller queues the sub-scene
        if let Err(e) = self.prepare_frame(size) {
            tracing::warn!("render_to_texture: prepare failed: {e:?}");
            self.begin_frame();
            return;
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
            if let Err(e) = self.encode_passes(&mut encoder, color_view, depth_view, clear) {
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
        self.sky_this_frame = self.draw_sky && self.camera.is_some();
        if let Some(cam) = self.camera {
            let aspect = if size.y > 0.0 { size.x / size.y } else { 1.0 };
            let view_projection = cam.view_projection(aspect);
            self.mesh.set_camera_matrix(&self.queue, view_projection);
            self.mesh_textured
                .set_camera_matrix(&self.queue, view_projection);
            self.skinned.set_camera_matrix(&self.queue, view_projection);
            self.billboard
                .set_camera(&self.queue, cam.view(), view_projection);
            let inv_vp = view_projection.inverse();
            if self.sky_this_frame {
                self.sky
                    .set_uniform(&self.queue, scene_to_sky_uniform(&self.scene, inv_vp, camera_pos));
            }
            if let Some(params) = &self.volumetric_params {
                self.volumetric.set_uniform(
                    &self.queue,
                    VolumetricDiskUniform::from_params(params, inv_vp, camera_pos),
                );
            }
            if let Some(fog) = &self.ground_fog_params {
                self.ground_fog
                    .set_uniform(&self.queue, GroundFogUniform::from_params(fog, inv_vp, camera_pos));
            }
        }
        self.mesh
            .set_scene_uniform(&self.queue, scene_to_uniform(&self.scene, camera_pos));
        self.mesh_textured
            .set_scene_uniform(&self.queue, scene_to_uniform(&self.scene, camera_pos));
        self.skinned
            .set_scene_uniform(&self.queue, scene_to_uniform(&self.scene, camera_pos));

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

    /// Encode the opaque + overlay passes into `encoder`, targeting `color_view` (+
    /// `depth_view`), clearing colour to `clear`. Immutable — reads the prepared pipeline
    /// state — so it serves both the swapchain view and an offscreen target view.
    fn encode_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        clear: [f64; 4],
    ) -> Result<()> {
        // Pass 1 — opaque scene: sky, 3D meshes (write depth), lines, world-space billboards.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.opaque_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
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
                self.sky.render(&mut pass);
            }
            self.mesh.render(&mut pass, &self.meshes);
            self.mesh_textured.render(&mut pass, &self.textures);
            self.skinned.render(&mut pass);
            self.lines.render(&mut pass);
            // Overlay lines last in the opaque pass — depth-Always, so they sit on top of
            // the mesh (skeleton-over-mesh debug view).
            self.lines_overlay.render(&mut pass);
            self.billboard.render(&mut pass, &self.textures);
        }

        // Pass 2 — depth-aware volumetric (reads depth), then 2D overlays in painter's order.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.overlay_pass"),
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
                    depth_ops: None, // read-only → the volumetric may sample this depth in-pass
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if self.volumetric_params.is_some() && self.camera.is_some() {
                self.volumetric.render(&mut pass);
            }
            if self.ground_fog_params.is_some() && self.camera.is_some() {
                self.ground_fog.render(&mut pass);
            }
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
                self.ui.render_layer(&mut pass, layer);
                self.triangle.render_layer(&mut pass, layer);
                self.sprite.render_layer(&mut pass, layer, &self.textures);
                self.text
                    .render_layer(&mut pass, layer)
                    .context("text render failed")?;
            }
        }
        Ok(())
    }

    /// Encode and submit the swapchain frame. Recoverable surface losses reconfigure and
    /// skip the frame.
    pub fn end_frame(&mut self) -> Result<()> {
        self.prepare_frame(self.screen)?;

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

        self.encode_passes(&mut encoder, &view, &self.depth_view, self.clear_color)?;

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // The atlas may want to trim itself between frames.
        self.text.atlas.trim();

        Ok(())
    }
}

/// Convert the friendly [`SceneLighting`] (Vec3 fields) into the GPU's
/// `vec4`-padded [`SceneUniform`], injecting the camera world position and
/// packing the two scalars into the reserved `.w` lanes (`fog_color.w` =
/// density, `grade.w` = strength).
fn scene_to_uniform(s: &SceneLighting, camera_pos: Vec3) -> SceneUniform {
    SceneUniform {
        sun_dir: [s.sun_dir.x, s.sun_dir.y, s.sun_dir.z, 0.0],
        sun_color: [s.sun_color.x, s.sun_color.y, s.sun_color.z, 0.0],
        moon_dir: [s.moon_dir.x, s.moon_dir.y, s.moon_dir.z, 0.0],
        moon_color: [s.moon_color.x, s.moon_color.y, s.moon_color.z, 0.0],
        ambient: [s.ambient.x, s.ambient.y, s.ambient.z, 0.0],
        camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
        fog_color: [s.fog_color.x, s.fog_color.y, s.fog_color.z, s.fog_density],
        grade: [s.grade.x, s.grade.y, s.grade.z, s.grade_strength],
        point_pos: [s.point_pos.x, s.point_pos.y, s.point_pos.z, 0.0],
        point_color: [s.point_color.x, s.point_color.y, s.point_color.z, 0.0],
    }
}

/// Build the procedural-sky uniform from the scene lighting plus this frame's
/// inverse view-projection and camera position. Shares the sun/moon and the
/// `sky_zenith`/`sky_horizon` palette with the mesh lighting so the sky and
/// the lit terrain read as one atmosphere.
fn scene_to_sky_uniform(s: &SceneLighting, inv_view_proj: Mat4, camera_pos: Vec3) -> SkyUniform {
    SkyUniform {
        inv_view_proj: inv_view_proj.to_cols_array_2d(),
        camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
        sun_dir: [s.sun_dir.x, s.sun_dir.y, s.sun_dir.z, 0.0],
        sun_color: [s.sun_color.x, s.sun_color.y, s.sun_color.z, 0.0],
        moon_dir: [s.moon_dir.x, s.moon_dir.y, s.moon_dir.z, 0.0],
        moon_color: [s.moon_color.x, s.moon_color.y, s.moon_color.z, 0.0],
        zenith: [s.sky_zenith.x, s.sky_zenith.y, s.sky_zenith.z, 0.0],
        horizon: [s.sky_horizon.x, s.sky_horizon.y, s.sky_horizon.z, 0.0],
        star_rotation: s.star_rotation.to_cols_array_2d(),
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
