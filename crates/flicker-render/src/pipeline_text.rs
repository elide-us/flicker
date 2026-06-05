//! Text pipeline backed by [`glyphon`].
//!
//! Each queued draw owns a fresh `glyphon::Buffer` (cheap-ish for short text)
//! tagged with a `layer` (the painter's-order sort key shared by every 2D
//! pipeline). Because glyphon's `TextRenderer::render` draws *all* of a
//! renderer's prepared areas at once, per-layer interleaving needs one
//! `TextRenderer` per distinct layer: `prepare` partitions the queue by layer
//! and prepares one pooled renderer for each, and the main renderer draws each
//! at its layer's turn (`Renderer::end_frame`). The pool is reused across
//! frames and is tiny in practice (one entry per on-screen scene/overlay that
//! draws text). All pooled renderers share the one atlas; flicker's UI glyph
//! set is small and static, so the atlas does not repack mid-frame.

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
    layer: f32,
}

pub struct TextPipeline {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub atlas: TextAtlas,
    pub viewport: Viewport,
    /// One renderer per distinct layer drawn this frame; grown on demand and
    /// reused across frames.
    renderers: Vec<TextRenderer>,
    queued: Vec<QueuedText>,
    /// `(layer, renderer index)` for each distinct layer prepared this frame,
    /// ascending. Built in `prepare`.
    bands: Vec<(f32, usize)>,
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
        // Start the pool with a single renderer; `prepare` grows it to one per
        // distinct layer on demand.
        let renderers = vec![new_text_renderer(&mut atlas, device)];

        Self {
            font_system,
            swash_cache,
            atlas,
            viewport,
            renderers,
            queued: Vec::new(),
            bands: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.queued.clear();
        self.bands.clear();
    }

    /// Queue a string for rendering at `layer`. `position` is the top-left of the
    /// text in pixels. `size` is the font size in pixels. `color` is RGBA in 0..1.
    pub fn push(
        &mut self,
        text: &str,
        left: f32,
        top: f32,
        size: f32,
        color: [f32; 4],
        layer: f32,
    ) {
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
            layer,
        });
    }

    /// The distinct layers present this frame, ascending.
    pub fn layers(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands.iter().map(|(layer, _)| *layer)
    }

    /// Measure `text` at font `size`: the max line width and total height in
    /// pixels. Shapes a throwaway buffer (no upload), so it can be called for
    /// layout before drawing. Mirrors the shaping in [`Self::push`] so the
    /// measurement matches what gets drawn.
    pub fn measure(&mut self, text: &str, size: f32) -> (f32, f32) {
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
        let mut width = 0.0_f32;
        let mut lines = 0_usize;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            lines += 1;
        }
        (width, lines.max(1) as f32 * metrics.line_height)
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<(), glyphon::PrepareError> {
        self.bands.clear();
        self.viewport.update(queue, Resolution { width, height });
        if self.queued.is_empty() {
            return Ok(());
        }

        // Distinct layers, ascending — one pooled renderer prepared per layer.
        let mut layers: Vec<f32> = self.queued.iter().map(|q| q.layer).collect();
        layers.sort_by(f32::total_cmp);
        layers.dedup();

        while self.renderers.len() < layers.len() {
            let renderer = new_text_renderer(&mut self.atlas, device);
            self.renderers.push(renderer);
        }

        for (index, &layer) in layers.iter().enumerate() {
            let areas: Vec<TextArea> = self
                .queued
                .iter()
                .filter(|q| q.layer == layer)
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

            self.renderers[index].prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )?;
            self.bands.push((layer, index));
        }

        Ok(())
    }

    /// Draw only the text submitted at `layer` (no-op if none).
    pub fn render_layer<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        layer: f32,
    ) -> Result<(), glyphon::RenderError> {
        let Some(&(_, index)) = self.bands.iter().find(|(l, _)| *l == layer) else {
            return Ok(());
        };
        self.renderers[index].render(&self.atlas, &self.viewport, pass)
    }
}

/// Build a 2D-overlay text renderer: it shares the depth attachment with the 3D
/// pipeline but neither writes nor tests depth, so 2D ordering is governed
/// entirely by layer/submission (painter's order), never the depth buffer.
fn new_text_renderer(atlas: &mut TextAtlas, device: &wgpu::Device) -> TextRenderer {
    let depth_stencil = Some(wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: false,
        depth_compare: wgpu::CompareFunction::Always,
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    });
    TextRenderer::new(
        atlas,
        device,
        wgpu::MultisampleState::default(),
        depth_stencil,
    )
}
