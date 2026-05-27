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
use winit::window::Window;

use crate::mesh::{Camera, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex};
use crate::pipeline_mesh::{create_depth_view, LoadedMesh, MeshPipeline};
use crate::pipeline_sprite::SpritePipeline;
use crate::pipeline_text::TextPipeline;
use crate::pipeline_triangle::TrianglePipeline;
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

    /// Depth attachment shared by every pipeline in the main pass. The
    /// 3D pipeline writes/tests it; the 2D pipelines neither write nor
    /// test (their depth-stencil state is "always pass, no write").
    #[allow(dead_code)]
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,

    textures: Vec<LoadedTexture>,
    meshes: Vec<LoadedMesh>,
    /// Current camera (cached so the runner can request the aspect-
    /// dependent view-projection in `end_frame`). `None` means "no
    /// camera set this frame" — `draw_mesh` still works but the matrix
    /// from the previous `set_camera` carries over.
    camera: Option<Camera>,
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
            depth_texture,
            depth_view,
            textures: Vec::new(),
            meshes: Vec::new(),
            camera: None,
        })
    }

    /// Return the underlying winit window — useful for cursor/title changes.
    pub fn window(&self) -> &Window {
        &self.window
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
    }

    /// Current logical size of the rendering surface, in pixels.
    pub fn size(&self) -> Vec2 {
        self.screen
    }

    /// Upload an RGBA8 image and return a handle. The pixel buffer length
    /// must equal `width * height * 4`.
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
        let id = self.textures.len() as u32;
        self.textures.push(tex);
        TextureHandle(id)
    }

    /// Upload a 3D mesh and return a handle. The mesh persists across
    /// frames; subsequent `draw_mesh(handle, ...)` calls reuse the same
    /// GPU buffers.
    pub fn upload_mesh(&mut self, vertices: &[MeshVertex], indices: MeshIndices<'_>) -> MeshHandle {
        let loaded = self.mesh.upload(&self.device, vertices, indices);
        let id = self.meshes.len() as u32;
        self.meshes.push(loaded);
        MeshHandle(id)
    }

    /// Reset all per-frame draw queues. Called by the runner at the start
    /// of every frame. Mesh **storage** (uploaded vertex/index buffers)
    /// is retained; only the per-frame mesh **draw queue** clears.
    pub fn begin_frame(&mut self) {
        self.triangle.clear();
        self.sprite.clear();
        self.text.clear();
        self.mesh.clear();
    }

    /// Submit a solid-colored triangle. Vertices are pixel coordinates with the
    /// origin at the top-left.
    pub fn draw_triangle(&mut self, a: Vec2, b: Vec2, c: Vec2, color: [f32; 4]) {
        self.triangle.push(self.screen, a, b, c, color);
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
        self.sprite
            .push(self.screen, texture, position, size, color);
    }

    /// Submit a string of text. `position` is the top-left baseline in pixels; `size`
    /// is the font size in pixels; `color` is RGBA in 0..1.
    pub fn draw_text(&mut self, text: &str, position: Vec2, size: f32, color: [f32; 4]) {
        self.text.push(text, position.x, position.y, size, color);
    }

    /// Set the 3D camera used for subsequent `draw_mesh` calls. Typically
    /// called once per frame before any `draw_mesh`.
    pub fn set_camera(&mut self, camera: &Camera) {
        self.camera = Some(*camera);
    }

    /// Queue a mesh for rendering this frame.
    ///
    /// `model` is the cluster-local-to-world transform; the camera (set
    /// via [`Renderer::set_camera`]) supplies the view and projection.
    /// `options` controls fill vs wireframe and the tint.
    pub fn draw_mesh(&mut self, mesh: MeshHandle, model: Mat4, options: MeshDrawOptions) {
        self.mesh.push(mesh, model, options.tint, options.wireframe);
    }

    /// Encode and submit the frame. Returns errors from the surface acquisition
    /// or text-pipeline preparation; recoverable surface losses are handled
    /// internally by reconfiguring.
    pub fn end_frame(&mut self) -> Result<()> {
        // Update camera-derived view-projection (if a camera is set).
        if let Some(cam) = self.camera {
            let aspect = if self.screen.y > 0.0 {
                self.screen.x / self.screen.y
            } else {
                1.0
            };
            self.mesh
                .set_camera_matrix(&self.queue, cam.view_projection(aspect));
        }

        // Upload buffered geometry/text.
        self.triangle.prepare(&self.device, &self.queue);
        self.sprite.prepare(&self.device, &self.queue);
        self.mesh.prepare(&self.device, &self.queue);
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

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("flicker.main_pass"),
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

            // 3D first so 2D layers on top.
            self.mesh.render(&mut pass, &self.meshes);
            self.triangle.render(&mut pass);
            self.sprite.render(&mut pass, &self.textures);
            self.text.render(&mut pass).context("text render failed")?;
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        // The atlas may want to trim itself between frames.
        self.text.atlas.trim();

        Ok(())
    }
}
