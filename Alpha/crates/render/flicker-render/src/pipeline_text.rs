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
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};

use crate::pipeline_mesh::DEPTH_FORMAT;

/// The concrete UI type face selected for a line of text, by semantic role
/// (the Prism design language). Faces are registered by the app via
/// [`Renderer::register_ui_font`](crate::Renderer::register_ui_font); text whose
/// role has no registered face falls back to a system font, so this never fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontRole {
    /// Display / headings & names.
    Display,
    /// Labels, caps, small meta.
    Label,
    /// Body / prose (the default).
    #[default]
    Body,
    /// Runic decorations (the `Prism Rune` face — Elder Futhark corner glyphs).
    Rune,
}

impl FontRole {
    /// The font-family name this role selects — the internal family name the
    /// matching `Alpha/content/sensorium/fonts` face is registered under.
    fn family(self) -> &'static str {
        match self {
            FontRole::Display => "Prism Display",
            FontRole::Label => "Prism Label",
            FontRole::Body => "Prism Body",
            FontRole::Rune => "Prism Rune",
        }
    }

    /// The weight this role draws at. `bold` selects the heavier cut on the roles
    /// that ship one (Display 600→700); roles with a single weight ignore it and
    /// the nearest registered face is chosen.
    fn weight(self, bold: bool) -> Weight {
        match (self, bold) {
            (FontRole::Display, true) => Weight(700),
            (FontRole::Display, false) => Weight(600),
            (FontRole::Label, _) => Weight(600),
            (FontRole::Body, _) => Weight(400),
            (FontRole::Rune, _) => Weight(400),
        }
    }

    /// Letter-spacing as a fraction of the em. Cinzel caps (Label) read as carved
    /// inscription only when tracked; prose and headings stay at `0.0`.
    fn tracking(self) -> f32 {
        match self {
            FontRole::Label => 0.16,
            FontRole::Display | FontRole::Body | FontRole::Rune => 0.0,
        }
    }
}

struct QueuedText {
    buffer: Buffer,
    left: f32,
    top: f32,
    color: Color,
    layer: f32,
    clip: Option<[f32; 4]>,
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
    /// Framebuffer size cached in `prepare` — the scissor reset before glyphon draws.
    screen: (u32, u32),
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
            screen: (0, 0),
        }
    }

    pub fn clear(&mut self) {
        self.queued.clear();
        self.bands.clear();
    }

    /// Load a UI font from TTF/OTF bytes into the font database. The face's own
    /// internal family name (e.g. `"Prism Display"`) is what a [`FontRole`]
    /// selects at draw time; keep [`FontSystem::new`]'s system fonts as the
    /// fallback for glyphs a UI face lacks.
    pub fn register_font(&mut self, bytes: &[u8]) {
        self.font_system.db_mut().load_font_data(bytes.to_vec());
    }

    /// Shape one run into a fresh buffer with the face selected by `role` +
    /// `italic`/`bold`. The family carries the Prism role; weight and italic map
    /// onto the matching registered face (`FontRole::weight` / cosmic-text picks
    /// the nearest), falling back to a system face for any glyph a UI face lacks.
    fn shape(&mut self, text: &str, size: f32, role: FontRole, italic: bool, bold: bool) -> Buffer {
        let attrs = Attrs::new()
            .family(Family::Name(role.family()))
            .weight(role.weight(bold))
            .style(if italic { Style::Italic } else { Style::Normal });
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(size, size * 1.2));
        buffer.set_size(&mut self.font_system, None, None);
        buffer.set_text(&mut self.font_system, text, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }

    /// Queue a string for rendering at `layer`. `position` is the top-left of the
    /// text in pixels. `size` is the font size in pixels. `color` is RGBA in 0..1.
    /// `role` selects the face; `italic`/`bold` select its style within the family.
    #[allow(clippy::too_many_arguments)] // low-level text submit; each param is distinct
    pub fn push(
        &mut self,
        text: &str,
        left: f32,
        top: f32,
        size: f32,
        color: [f32; 4],
        layer: f32,
        role: FontRole,
        italic: bool,
        bold: bool,
        tracking: f32,
        clip: Option<[f32; 4]>,
    ) {
        let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        let c = Color::rgba(to_u8(color[0]), to_u8(color[1]), to_u8(color[2]), to_u8(color[3]));

        // Per-call `tracking` overrides the role default when non-negative (a data
        // cell passes 0.0 to stay tight so fixed-width numeric columns align).
        let track = (if tracking < 0.0 { role.tracking() } else { tracking }) * size;
        if track == 0.0 {
            let buffer = self.shape(text, size, role, italic, bold);
            self.queued.push(QueuedText { buffer, left, top, color: c, layer, clip });
            return;
        }
        // Tracked caps: cosmic-text has no letter-spacing, so lay out one glyph at
        // a time, advancing by the glyph's own width plus the role's spacing.
        // Label strings are short, so the per-glyph buffers stay cheap.
        let mut x = left;
        for ch in text.chars() {
            if ch.is_whitespace() {
                x += size * 0.28 + track;
                continue;
            }
            let mut tmp = [0u8; 4];
            let buffer = self.shape(ch.encode_utf8(&mut tmp), size, role, italic, bold);
            let w = buffer.layout_runs().next().map(|r| r.line_w).unwrap_or(size * 0.5);
            self.queued.push(QueuedText { buffer, left: x, top, color: c, layer, clip });
            x += w + track;
        }
    }

    /// The distinct layers present this frame, ascending.
    pub fn layers(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands.iter().map(|(layer, _)| *layer)
    }

    /// Measure `text` at font `size`: the max line width and total height in
    /// pixels. Shapes a throwaway buffer (no upload), so it can be called for
    /// layout before drawing. Mirrors the shaping *and tracking* in [`Self::push`]
    /// so the measurement matches what gets drawn.
    pub fn measure(
        &mut self,
        text: &str,
        size: f32,
        role: FontRole,
        italic: bool,
        bold: bool,
        tracking: f32,
    ) -> (f32, f32) {
        let line_height = size * 1.2;
        // Per-call `tracking` overrides the role default when non-negative (a data
        // cell passes 0.0 to stay tight so fixed-width numeric columns align).
        let track = (if tracking < 0.0 { role.tracking() } else { tracking }) * size;
        if track == 0.0 {
            let buffer = self.shape(text, size, role, italic, bold);
            let mut width = 0.0_f32;
            let mut lines = 0_usize;
            for run in buffer.layout_runs() {
                width = width.max(run.line_w);
                lines += 1;
            }
            return (width, lines.max(1) as f32 * line_height);
        }
        // Tracked single line — mirror push's per-glyph advance.
        let mut width = 0.0_f32;
        for ch in text.chars() {
            if ch.is_whitespace() {
                width += size * 0.28 + track;
                continue;
            }
            let mut tmp = [0u8; 4];
            let buffer = self.shape(ch.encode_utf8(&mut tmp), size, role, italic, bold);
            width += buffer.layout_runs().next().map(|r| r.line_w).unwrap_or(size * 0.5) + track;
        }
        ((width - track).max(0.0), line_height)
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<(), glyphon::PrepareError> {
        self.bands.clear();
        self.screen = (width, height);
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
                    bounds: text_bounds(q.clip, width, height),
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
        // Reset any scissor a prior 2D pipeline left this layer so glyphon clips text
        // only by each area's `bounds` (that is where a scroll region masks its labels).
        crate::pipeline_ui::set_scissor(
            pass,
            glam::Vec2::new(self.screen.0 as f32, self.screen.1 as f32),
            None,
        );
        self.renderers[index].render(&self.atlas, &self.viewport, pass)
    }
}

/// A glyphon clip rect for one text area: `clip` (px x,y,w,h) clamped to the
/// framebuffer, or the whole frame when `None`. glyphon clips each area to its
/// `bounds`, so a scroll region masks its labels without a wgpu scissor.
fn text_bounds(clip: Option<[f32; 4]>, width: u32, height: u32) -> TextBounds {
    let (w, h) = (width as i32, height as i32);
    match clip {
        Some([x, y, cw, ch]) => TextBounds {
            left: (x as i32).clamp(0, w),
            top: (y as i32).clamp(0, h),
            right: ((x + cw) as i32).clamp(0, w),
            bottom: ((y + ch) as i32).clamp(0, h),
        },
        None => TextBounds { left: 0, top: 0, right: w, bottom: h },
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
