//! Text pipeline backed by [`glyphon`].
//!
//! Each queued draw owns a fresh `glyphon::Buffer` (cheap-ish for short text).
//! During `prepare`, all buffers are turned into `TextArea`s and uploaded; on
//! `render`, the glyphon `TextRenderer` issues a single draw call against the
//! atlas.

use glyphon::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport,
};

use crate::pipeline_mesh::DEPTH_FORMAT;

struct QueuedText {
    buffer: Buffer,
    left: f32,
    top: f32,
    color: Color,
}

pub struct TextPipeline {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub atlas: TextAtlas,
    pub viewport: Viewport,
    pub text_renderer: TextRenderer,
    queued: Vec<QueuedText>,
}

impl TextPipeline {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = glyphon::Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, surface_format);
        // 2D overlay — share the depth attachment with the 3D pipeline but
        // neither write nor test depth so text always layers on top.
        let depth_stencil = Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });
        let text_renderer = TextRenderer::new(
            &mut atlas,
            device,
            wgpu::MultisampleState::default(),
            depth_stencil,
        );

        Self {
            font_system,
            swash_cache,
            atlas,
            viewport,
            text_renderer,
            queued: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.queued.clear();
    }

    /// Queue a string for rendering. `position` is the top-left of the text in pixels.
    /// `size` is the font size in pixels. `color` is RGBA in 0..1.
    pub fn push(&mut self, text: &str, left: f32, top: f32, size: f32, color: [f32; 4]) {
        let metrics = Metrics::new(size, size * 1.2);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, None, None);
        buffer.set_text(
            &mut self.font_system,
            text,
            Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        let c = Color::rgba(
            to_u8(color[0]),
            to_u8(color[1]),
            to_u8(color[2]),
            to_u8(color[3]),
        );

        self.queued.push(QueuedText {
            buffer,
            left,
            top,
            color: c,
        });
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<(), glyphon::PrepareError> {
        self.viewport.update(queue, Resolution { width, height });

        let areas: Vec<TextArea> = self
            .queued
            .iter()
            .map(|q| TextArea {
                buffer: &q.buffer,
                left: q.left,
                top: q.top,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: width as i32,
                    bottom: height as i32,
                },
                default_color: q.color,
                custom_glyphs: &[],
            })
            .collect();

        self.text_renderer.prepare(
            device,
            queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            areas,
            &mut self.swash_cache,
        )
    }

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
    ) -> Result<(), glyphon::RenderError> {
        if self.queued.is_empty() {
            return Ok(());
        }
        self.text_renderer.render(&self.atlas, &self.viewport, pass)
    }
}
