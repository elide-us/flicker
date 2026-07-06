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
use crate::pipeline_mesh::{create_depth_view, LoadedMesh, MeshPipeline, SceneUniform};
use crate::pipeline_mesh_textured::{
    PbrMaps, TexturedMeshHandle, TexturedMeshPipeline, TexturedVertex,
};
use crate::pipeline_sky::{SkyPipeline, SkyUniform};
use crate::pipeline_sprite::SpritePipeline;
use crate::pipeline_text::TextPipeline;
use crate::pipeline_triangle::TrianglePipeline;
use crate::pipeline_volumetric::{VolumetricDisk, VolumetricDiskUniform, VolumetricPipeline};
use crate::texture::{LoadedTexture, TextureHandle};

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
    sprite: SpritePipeline,
    text: TextPipeline,
    mesh: MeshPipeline,
    mesh_textured: TexturedMeshPipeline,
    lines: LinesPipeline,
    billboard: BillboardPipeline,
    sky: SkyPipeline,
    volumetric: VolumetricPipeline,

    /// Depth attachment shared by every pipeline in the main pass. The
    /// 3D pipeline writes/tests it; the 2D pipelines neither write nor
    /// test (their depth-stencil state is "always pass, no write").
    #[allow(dead_code)]
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,

    textures: Vec<LoadedTexture>,
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
        let sprite = SpritePipeline::new(&device, surface_format);
        let text = TextPipeline::new(&device, &queue, surface_format);
        let min_uniform_offset_alignment = device.limits().min_uniform_buffer_offset_alignment;
        let mesh = MeshPipeline::new(&device, surface_format, min_uniform_offset_alignment);
        let mesh_textured =
            TexturedMeshPipeline::new(&device, &queue, surface_format, min_uniform_offset_alignment);
        let lines = LinesPipeline::new(&device, surface_format, mesh.camera_buffer());
        let billboard = BillboardPipeline::new(&device, surface_format);
        let sky = SkyPipeline::new(&device, surface_format);
        let mut volumetric = VolumetricPipeline::new(&device, surface_format);
        // Bind the scene depth so the volumetric can sample it (clamp rays at bodies).
        volumetric.set_depth(&device, &depth_view);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            screen: Vec2::new(width as f32, height as f32),
            clear_color: [0.05, 0.06, 0.08, 1.0],
            triangle,
            sprite,
            text,
            mesh,
            mesh_textured,
            lines,
            billboard,
            sky,
            volumetric,
            volumetric_params: None,
            depth_texture,
            depth_view,
            textures: Vec::new(),
            meshes: Vec::new(),
            free_mesh_slots: Vec::new(),
            camera: None,
            scene: SceneLighting::default(),
            draw_sky: false,
            current_layer: 0.0,
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
        // Rebind the recreated depth view into the volumetric's bind group.
        self.volumetric.set_depth(&self.device, &self.depth_view);
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
        let id = self.textures.len() as u32;
        self.textures.push(tex);
        TextureHandle(id)
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

    /// Reset all per-frame draw queues. Called by the runner at the start
    /// of every frame. Mesh **storage** (uploaded vertex/index buffers)
    /// is retained; only the per-frame mesh **draw queue** clears.
    pub fn begin_frame(&mut self) {
        self.triangle.clear();
        self.sprite.clear();
        self.text.clear();
        self.mesh.clear();
        self.mesh_textured.clear();
        self.lines.clear();
        self.billboard.clear();
        self.draw_sky = false;
        self.volumetric_params = None;
        self.current_layer = 0.0;
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

    /// Submit a solid-colored triangle. Vertices are pixel coordinates with the
    /// origin at the top-left.
    pub fn draw_triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: [f32; 4]) {
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
        self.sprite.push(
            self.screen,
            texture,
            position,
            size,
            color,
            self.current_layer,
        );
    }

    /// Submit a string of text. `position` is the top-left baseline in pixels; `size`
    /// is the font size in pixels; `color` is RGBA in 0..1.
    pub fn draw_text(&mut self, text: &str, position: Vec2, size: f32, color: [f32; 4]) {
        self.text.push(
            text,
            position.x,
            position.y,
            size,
            color,
            self.current_layer,
        );
    }

    /// Measure `text` at font `size`, returning its rendered size (max line
    /// width, total height) in pixels. For laying out UI before drawing —
    /// shapes a throwaway buffer, no upload.
    pub fn measure_text(&mut self, text: &str, size: f32) -> Vec2 {
        let (w, h) = self.text.measure(text, size);
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

    /// Queue a mesh for rendering this frame.
    ///
    /// `model` is the cluster-local-to-world transform; the camera (set
    /// via [`Renderer::set_camera`]) supplies the view and projection.
    /// `options` controls fill vs wireframe and the tint.
    pub fn draw_mesh(&mut self, mesh: MeshHandle, model: Mat4, options: MeshDrawOptions) {
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
        self.mesh_textured.push(
            mesh,
            texture,
            PbrMaps::default(),
            model,
            options.tint,
            options.gloss,
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
        self.mesh_textured
            .push(mesh, texture, maps, model, options.tint, options.gloss);
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
        for &(a, b) in segments {
            self.lines.push_segment(a, b, color);
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
        self.billboard
            .push_additive(texture, world_position, world_size, uv_min, uv_max, color);
    }

    /// Encode and submit the frame. Returns errors from the surface acquisition
    /// or text-pipeline preparation; recoverable surface losses are handled
    /// internally by reconfiguring.
    pub fn end_frame(&mut self) -> Result<()> {
        // Update camera-derived view-projection (if a camera is set). The sky
        // needs the *inverse* view-projection to turn each pixel back into a
        // world-space view ray; it's a no-op without a camera.
        let camera_pos = self.camera.map(|c| c.position).unwrap_or(Vec3::ZERO);
        let sky_this_frame = self.draw_sky && self.camera.is_some();
        if let Some(cam) = self.camera {
            let aspect = if self.screen.y > 0.0 {
                self.screen.x / self.screen.y
            } else {
                1.0
            };
            let view_projection = cam.view_projection(aspect);
            self.mesh.set_camera_matrix(&self.queue, view_projection);
            self.mesh_textured
                .set_camera_matrix(&self.queue, view_projection);
            self.billboard
                .set_camera(&self.queue, cam.view(), view_projection);
            let inv_vp = view_projection.inverse();
            if sky_this_frame {
                self.sky
                    .set_uniform(&self.queue, scene_to_sky_uniform(&self.scene, inv_vp, camera_pos));
            }
            if let Some(params) = &self.volumetric_params {
                self.volumetric.set_uniform(
                    &self.queue,
                    VolumetricDiskUniform::from_params(params, inv_vp, camera_pos),
                );
            }
        }

        // Upload the frame-global lighting/atmosphere uniform. The camera
        // position is injected here (for distance fog) so callers of
        // `set_scene` don't have to thread it through themselves.
        self.mesh
            .set_scene_uniform(&self.queue, scene_to_uniform(&self.scene, camera_pos));
        self.mesh_textured
            .set_scene_uniform(&self.queue, scene_to_uniform(&self.scene, camera_pos));

        // Upload buffered geometry/text.
        self.triangle.prepare(&self.device, &self.queue);
        self.sprite.prepare(&self.device, &self.queue);
        self.mesh.prepare(&self.device, &self.queue);
        self.mesh_textured
            .prepare(&self.device, &self.queue, &self.textures);
        self.lines.prepare(&self.device, &self.queue);
        self.billboard.prepare(&self.device, &self.queue);
        self.text
            .prepare(
                &self.device,
                &self.queue,
                self.config.width,
                self.config.height,
            )
            .context("text prepare failed")?;

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

        // Pass 1 — opaque scene: sky background, then 3D meshes (writing depth), lines, and
        // world-space billboards. The depth is **stored** so the volumetric pass can sample it.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.opaque_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.clear_color[0],
                            g: self.clear_color[1],
                            b: self.clear_color[2],
                            a: self.clear_color[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Procedural sky behind everything (fullscreen, no depth write), so the mesh paints
            // over it wherever bodies exist. Then 3D meshes (write depth), lines (test, no write),
            // and world-space depth-tested billboards.
            if sky_this_frame {
                self.sky.render(&mut pass);
            }
            self.mesh.render(&mut pass, &self.meshes);
            self.mesh_textured.render(&mut pass, &self.textures);
            self.lines.render(&mut pass);
            self.billboard.render(&mut pass, &self.textures);
        }

        // Pass 2 — the **depth-aware** volumetric over the opaque scene, then 2D overlays. Depth is
        // bound **read-only** here (no pass-2 pipeline writes depth), which lets the volumetric
        // *sample* the same depth buffer (bound in its group) to clamp its rays at solid bodies —
        // so the dust and star sit in correct depth with the bodies instead of always behind them.
        // 2D overlays paint last, on top of everything.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.overlay_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: None, // read-only → the volumetric may sample this depth in-pass
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if self.volumetric_params.is_some() && self.camera.is_some() {
                self.volumetric.render(&mut pass);
            }

            // 2D in painter's order: walk the union of layers used by the three 2D pipelines,
            // ascending, drawing triangle → sprite → text within each layer (unchanged).
            let mut layers: Vec<f32> = Vec::new();
            layers.extend(self.triangle.layers());
            layers.extend(self.sprite.layers());
            layers.extend(self.text.layers());
            layers.sort_by(f32::total_cmp);
            layers.dedup();
            for &layer in &layers {
                self.triangle.render_layer(&mut pass, layer);
                self.sprite.render_layer(&mut pass, layer, &self.textures);
                self.text
                    .render_layer(&mut pass, layer)
                    .context("text render failed")?;
            }
        }

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
