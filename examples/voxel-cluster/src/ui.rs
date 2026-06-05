//! Styled UI for the voxel-cluster example — a small, self-contained
//! "gothic horror" toolkit (Dark Souls-ish palette: tarnished gold and
//! rust-red lines on silver/black/grey surfaces). The raster art is
//! generated procedurally (no binary assets, deterministic, tunable, in the
//! spirit of `build_digit_atlas`); the only widget so far is the Escape
//! pause modal.
//!
//! Example-local for now, but split so it can promote to a reusable
//! `flicker-ui` later: the palette/theme here is game-specific, while the
//! canvas, panel/button generators, layout, and hit-testing are generic.

use flicker::render::{Renderer, TextureHandle, Vec2};

// ===== palette =====

type Rgb = (u8, u8, u8);

const SURFACE_TOP: Rgb = (36, 39, 46);
const SURFACE_BOT: Rgb = (15, 17, 21);
const PLATE_TOP: Rgb = (48, 52, 60);
const PLATE_BOT: Rgb = (27, 30, 37);
const SILVER: Rgb = (150, 156, 166);
const GOLD: Rgb = (160, 126, 66);
const GOLD_BRIGHT: Rgb = (200, 164, 96);
const RUST: Rgb = (150, 60, 42);
const INK: Rgb = (8, 9, 11);

/// Tarnished-gold title text.
pub const COL_TITLE: [f32; 4] = [0.83, 0.67, 0.39, 1.0];
/// Resting button label (cold silver).
const COL_LABEL: [f32; 4] = [0.78, 0.81, 0.86, 1.0];
/// Hovered button label (lit gold).
const COL_LABEL_HOVER: [f32; 4] = [0.96, 0.80, 0.42, 1.0];
/// Hovered button outline (gold).
const COL_GOLD_LINE: [f32; 4] = [0.85, 0.66, 0.32, 0.95];
/// Full-screen dim behind the modal.
const COL_SCRIM: [f32; 4] = [0.02, 0.02, 0.03, 0.64];
/// Gold sheen overlaid on a hovered button.
const COL_SHEEN: [f32; 4] = [0.85, 0.66, 0.34, 0.15];

// ===== texture sizes (drawn 1:1, so the baked borders never distort) =====

const PANEL_W: u32 = 460;
const PANEL_H: u32 = 300;
const BUTTON_W: u32 = 264;
const BUTTON_H: u32 = 54;

/// Rough proportional-font advance as a fraction of font size, used to
/// estimate a label's width for centring (glyphon exposes no measure API).
const GLYPH_ADVANCE: f32 = 0.56;

// ===== pixel canvas =====

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            px: vec![0; w * h * 4],
        }
    }

    #[inline]
    fn put(&mut self, x: i32, y: i32, c: Rgb, a: u8) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.px[i] = c.0;
        self.px[i + 1] = c.1;
        self.px[i + 2] = c.2;
        self.px[i + 3] = a;
    }

    fn hline(&mut self, x0: usize, x1: usize, y: usize, c: Rgb, a: u8) {
        for x in x0..x1 {
            self.put(x as i32, y as i32, c, a);
        }
    }

    fn vline(&mut self, y0: usize, y1: usize, x: usize, c: Rgb, a: u8) {
        for y in y0..y1 {
            self.put(x as i32, y as i32, c, a);
        }
    }

    /// `t`-thick rectangle outline at `origin` of size `(w, h)`.
    fn rect_outline(&mut self, origin: (usize, usize), size: (usize, usize), c: Rgb, a: u8, t: usize) {
        let ((x, y), (w, h)) = (origin, size);
        for i in 0..t {
            self.hline(x, x + w, y + i, c, a);
            self.hline(x, x + w, y + h - 1 - i, c, a);
            self.vline(y, y + h, x + i, c, a);
            self.vline(y, y + h, x + w - 1 - i, c, a);
        }
    }

    /// Solid filled rectangle at `origin` of size `(w, h)`.
    fn fill_rect(&mut self, origin: (usize, usize), size: (usize, usize), c: Rgb, a: u8) {
        for yy in origin.1..origin.1 + size.1 {
            self.hline(origin.0, origin.0 + size.0, yy, c, a);
        }
    }

    /// Filled diamond (rotated square) of radius `r` centred on `(cx, cy)` —
    /// a small gothic stud/rivet ornament.
    fn diamond(&mut self, cx: i32, cy: i32, r: i32, c: Rgb, a: u8) {
        for dy in -r..=r {
            let span = r - dy.abs();
            for dx in -span..=span {
                self.put(cx + dx, cy + dy, c, a);
            }
        }
    }

    fn into_pixels(self) -> Vec<u8> {
        self.px
    }
}

// ===== color helpers =====

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

fn lerp_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    (
        lerp_u8(a.0, b.0, t),
        lerp_u8(a.1, b.1, t),
        lerp_u8(a.2, b.2, t),
    )
}

/// Brighten (`d > 0`) or darken (`d < 0`) each channel, clamped.
fn shade(c: Rgb, d: i32) -> Rgb {
    let f = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
    (f(c.0), f(c.1), f(c.2))
}

/// Multiply each channel by `f` (for the vignette), saturating.
fn scale(c: Rgb, f: f32) -> Rgb {
    let g = |v: u8| (v as f32 * f) as u8;
    (g(c.0), g(c.1), g(c.2))
}

/// Deterministic per-pixel grain in roughly `[-amp, amp]`, for a weathered
/// metal/stone grain (no RNG — a hash of the coordinate).
fn grain(x: usize, y: usize, amp: i32) -> i32 {
    let mut h = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    ((h & 0xff) as i32 - 128) * amp / 128
}

// ===== procedural textures =====

/// The modal panel: weathered dark-metal field with a vignette, a double
/// frame (rust outer / gold inner), brightened gold corner brackets, and a
/// gold rule under the title band.
fn build_panel() -> Vec<u8> {
    let (w, h) = (PANEL_W as usize, PANEL_H as usize);
    let mut c = Canvas::new(w, h);
    // Weathered dark-metal field with a deep vignette (candlelit-crypt feel).
    for y in 0..h {
        let base = lerp_rgb(SURFACE_TOP, SURFACE_BOT, y as f32 / (h - 1) as f32);
        for x in 0..w {
            let edge = (x.min(w - 1 - x)).min(y.min(h - 1 - y)) as f32;
            let vig = 0.46 + 0.54 * (edge / 60.0).min(1.0);
            c.put(x as i32, y as i32, scale(shade(base, grain(x, y, 7)), vig), 255);
        }
    }
    // Raised outer bezel: lit top/left, shadowed bottom/right.
    c.hline(0, w, 0, SILVER, 90);
    c.vline(0, h, 0, SILVER, 90);
    c.hline(0, w, h - 1, INK, 165);
    c.vline(0, h, w - 1, INK, 165);
    // Double frame: a rust band, then a tarnished-gold hairline.
    c.rect_outline((4, 4), (w - 8, h - 8), RUST, 255, 3);
    c.rect_outline((11, 11), (w - 22, h - 22), GOLD, 235, 1);

    // Title cartouche: a recessed darker band framed by gold rules, where the
    // runtime draws "PAUSED" (`pause_layout::title_y` lands inside it).
    let (band_y, band_h) = (24usize, 58usize);
    c.fill_rect((14, band_y), (w - 28, band_h), shade(SURFACE_BOT, -7), 255);
    c.hline(14, w - 14, band_y, GOLD, 220);
    c.hline(14, w - 14, band_y + band_h, GOLD, 220);

    // Corner studs (gold diamonds with a dark centre) on the gold frame.
    for &(cx, cy) in &[
        (11i32, 11i32),
        (w as i32 - 12, 11),
        (11, h as i32 - 12),
        (w as i32 - 12, h as i32 - 12),
    ] {
        c.diamond(cx, cy, 5, GOLD_BRIGHT, 255);
        c.put(cx, cy, INK, 255);
    }
    c.into_pixels()
}

/// A raised button plate: a lighter metal gradient, a faint top/left silver
/// bevel and bottom/right ink shadow, and a gold hairline border.
fn build_button() -> Vec<u8> {
    let (w, h) = (BUTTON_W as usize, BUTTON_H as usize);
    let mut c = Canvas::new(w, h);
    for y in 0..h {
        let base = lerp_rgb(PLATE_TOP, PLATE_BOT, y as f32 / (h - 1) as f32);
        // Darken the central band so the label sits in a shallow hollow.
        let from_centre = (y as f32 / (h - 1) as f32 - 0.5).abs(); // 0 centre .. 0.5 edge
        let recess = -11 + (from_centre * 22.0) as i32;
        for x in 0..w {
            c.put(x as i32, y as i32, shade(base, grain(x + 7, y + 13, 6) + recess), 255);
        }
    }
    // Bevel: bright top/left, dark bottom/right.
    c.hline(0, w, 0, SILVER, 120);
    c.hline(0, w, 1, SILVER, 45);
    c.vline(0, h, 0, SILVER, 120);
    c.hline(0, w, h - 1, INK, 175);
    c.vline(0, h, w - 1, INK, 175);
    // Tarnished-gold border over a thin inner shadow for depth.
    c.rect_outline((2, 2), (w - 4, h - 4), GOLD, 240, 1);
    c.rect_outline((3, 3), (w - 6, h - 6), shade(INK, 18), 80, 1);
    c.into_pixels()
}

// ===== layout + widgets =====

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum PauseButton {
    Resume,
    Quit,
}

#[derive(Copy, Clone)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x <= self.x + self.w && p.y >= self.y && p.y <= self.y + self.h
    }
}

/// Screen-space placement of the pause modal, recomputed from the viewport
/// each frame so `update` (hit-testing) and `render` (drawing) agree.
pub struct PauseLayout {
    panel: Rect,
    title_y: f32,
    resume: Rect,
    quit: Rect,
}

/// Centre the fixed-size modal on the screen and stack its buttons.
pub fn pause_layout(screen: Vec2) -> PauseLayout {
    let (pw, ph) = (PANEL_W as f32, PANEL_H as f32);
    let px = ((screen.x - pw) * 0.5).round();
    let py = ((screen.y - ph) * 0.5).round();
    let (bw, bh) = (BUTTON_W as f32, BUTTON_H as f32);
    let bx = px + (pw - bw) * 0.5;
    PauseLayout {
        panel: Rect {
            x: px,
            y: py,
            w: pw,
            h: ph,
        },
        title_y: py + 44.0,
        resume: Rect {
            x: bx,
            y: py + 134.0,
            w: bw,
            h: bh,
        },
        quit: Rect {
            x: bx,
            y: py + 204.0,
            w: bw,
            h: bh,
        },
    }
}

impl PauseLayout {
    /// Which button (if any) the cursor is over.
    #[must_use]
    pub fn hover(&self, cursor: Vec2) -> Option<PauseButton> {
        if self.resume.contains(cursor) {
            Some(PauseButton::Resume)
        } else if self.quit.contains(cursor) {
            Some(PauseButton::Quit)
        } else {
            None
        }
    }
}

/// Uploaded UI textures, built once at init.
pub struct Theme {
    panel: TextureHandle,
    button: TextureHandle,
    white: TextureHandle,
}

impl Theme {
    /// Generate + upload the gothic raster art. `white` is the shared 1×1
    /// white pixel (reused for the scrim, hover sheen, and outlines).
    pub fn load(renderer: &mut Renderer, white: TextureHandle) -> Self {
        let panel = renderer.load_texture(&build_panel(), PANEL_W, PANEL_H);
        let button = renderer.load_texture(&build_button(), BUTTON_W, BUTTON_H);
        Self {
            panel,
            button,
            white,
        }
    }

    /// Draw the pause modal: scrim, panel, title, and the two buttons with
    /// hover lighting.
    pub fn draw_pause(
        &self,
        r: &mut Renderer,
        screen: Vec2,
        layout: &PauseLayout,
        hover: Option<PauseButton>,
    ) {
        r.draw_sprite(self.white, Vec2::ZERO, screen, COL_SCRIM);
        r.draw_sprite(
            self.panel,
            Vec2::new(layout.panel.x, layout.panel.y),
            Vec2::new(layout.panel.w, layout.panel.h),
            [1.0, 1.0, 1.0, 1.0],
        );
        centered_text(r, "PAUSED", layout.panel, layout.title_y, 34.0, COL_TITLE);
        self.draw_button(r, &layout.resume, "RESUME", hover == Some(PauseButton::Resume));
        self.draw_button(r, &layout.quit, "QUIT", hover == Some(PauseButton::Quit));
    }

    fn draw_button(&self, r: &mut Renderer, rect: &Rect, label: &str, hovered: bool) {
        r.draw_sprite(
            self.button,
            Vec2::new(rect.x, rect.y),
            Vec2::new(rect.w, rect.h),
            [1.0, 1.0, 1.0, 1.0],
        );
        if hovered {
            r.draw_sprite(
                self.white,
                Vec2::new(rect.x, rect.y),
                Vec2::new(rect.w, rect.h),
                COL_SHEEN,
            );
            outline(r, self.white, rect, 2.0, COL_GOLD_LINE);
        }
        let col = if hovered { COL_LABEL_HOVER } else { COL_LABEL };
        let size = 22.0;
        centered_text(r, label, *rect, rect.y + (rect.h - size) * 0.5, size, col);
    }
}

/// Estimate the label width and draw it horizontally centred in `container`,
/// with its top-left at `y`.
fn centered_text(r: &mut Renderer, text: &str, container: Rect, y: f32, size: f32, color: [f32; 4]) {
    let est_w = text.chars().count() as f32 * size * GLYPH_ADVANCE;
    let x = (container.x + (container.w - est_w) * 0.5).max(container.x);
    r.draw_text(text, Vec2::new(x, y), size, color);
}

/// Draw a `t`-thick rectangle outline with the white texture (screen space).
fn outline(r: &mut Renderer, white: TextureHandle, rect: &Rect, t: f32, color: [f32; 4]) {
    r.draw_sprite(white, Vec2::new(rect.x, rect.y), Vec2::new(rect.w, t), color);
    r.draw_sprite(
        white,
        Vec2::new(rect.x, rect.y + rect.h - t),
        Vec2::new(rect.w, t),
        color,
    );
    r.draw_sprite(white, Vec2::new(rect.x, rect.y), Vec2::new(t, rect.h), color);
    r.draw_sprite(
        white,
        Vec2::new(rect.x + rect.w - t, rect.y),
        Vec2::new(t, rect.h),
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn save(name: &str, w: u32, h: u32, px: Vec<u8>) {
        let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(w, h, px).expect("size matches");
        // Temp dir so the preview never lands in the repo working tree.
        let path = std::env::temp_dir().join(name);
        img.save(&path).expect("write png");
        eprintln!("wrote {}", path.display());
    }

    /// Encode the procedural UI textures to PNG next to the crate for visual
    /// inspection. Ignored by default (writes files); run explicitly with
    /// `cargo test -p voxel-cluster ui_preview -- --ignored`.
    #[test]
    #[ignore = "writes PNG previews of the procedural UI art for manual inspection"]
    fn ui_preview() {
        save("ui_preview_panel.png", PANEL_W, PANEL_H, build_panel());
        save("ui_preview_button.png", BUTTON_W, BUTTON_H, build_button());
    }
}
