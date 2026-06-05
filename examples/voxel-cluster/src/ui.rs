//! Styled UI for the voxel-cluster example — a small, self-contained
//! "gothic horror" toolkit (Dark Souls-ish palette: tarnished gold and
//! rust-red lines on silver/black/grey surfaces). The raster art is
//! generated procedurally (no binary assets, deterministic, tunable, in the
//! spirit of `build_digit_atlas`). It backs the front-end scenes: the logo
//! wordmark, the main menu, the loading widget, and the pause modal.
//!
//! Example-local for now, but split so it can promote to a reusable
//! `flicker-ui` later: the palette/theme here is game-specific, while the
//! canvas, panel/button generators, layout, and hit-testing are generic.

use std::f32::consts::{PI, TAU};

use flicker::render::{Renderer, TextureHandle, Vec2};

// ===== palette =====

type Rgb = (u8, u8, u8);

const SURFACE_TOP: Rgb = (36, 39, 46);
const SURFACE_BOT: Rgb = (15, 17, 21);
const PLATE_TOP: Rgb = (48, 52, 60);
const PLATE_BOT: Rgb = (27, 30, 37);
const SILVER: Rgb = (150, 156, 166);
const GOLD: Rgb = (160, 126, 66);
/// Patina shadow for the tarnished-gold filigree (flat 2-tone with `GOLD`).
const GOLD_DK: Rgb = (96, 78, 44);
/// Carved slate for the stone frame: deep base + cool highlight; a faint
/// tone for the hairline weathering.
const SLATE_DK: Rgb = (32, 35, 40);
const SLATE_HI: Rgb = (104, 111, 121);
const SLATE_CRACK: Rgb = (22, 24, 28);
const INK: Rgb = (8, 9, 11);

/// Tarnished-gold title text.
pub const COL_TITLE: [f32; 4] = [0.83, 0.67, 0.39, 1.0];
/// Resting button label (cold silver).
const COL_LABEL: [f32; 4] = [0.78, 0.81, 0.86, 1.0];
/// Hovered button label (lit gold).
const COL_LABEL_HOVER: [f32; 4] = [0.96, 0.80, 0.42, 1.0];
/// Hovered button outline (gold).
const COL_GOLD_LINE: [f32; 4] = [0.85, 0.66, 0.32, 0.95];
/// Full-screen dim behind a modal (translucent over a frozen game).
const COL_SCRIM: [f32; 4] = [0.02, 0.02, 0.03, 0.64];
/// Opaque dark backdrop for a full-screen menu (nothing behind it).
const COL_BACKDROP: [f32; 4] = [0.035, 0.04, 0.05, 1.0];
/// Gold sheen overlaid on a hovered button.
const COL_SHEEN: [f32; 4] = [0.85, 0.66, 0.34, 0.15];
/// Loading-bar track (recessed dark) and tarnished-gold fill.
const COL_BAR_TRACK: [f32; 4] = [0.05, 0.06, 0.07, 1.0];
const COL_BAR_FILL: [f32; 4] = [0.63, 0.49, 0.26, 1.0];
/// Dropdown cell fill (resting) and hovered fill.
const COL_DD_FILL: [f32; 4] = [0.10, 0.11, 0.13, 0.97];
const COL_DD_HOT: [f32; 4] = [0.17, 0.19, 0.22, 0.98];

// ===== texture sizes (drawn 1:1, so the baked borders never distort) =====

const PANEL_W: u32 = 520;
const PANEL_H: u32 = 384;
/// Stone-frame thickness (px) around the recessed content well.
const FRAME: u32 = 38;
const BUTTON_W: u32 = 264;
const BUTTON_H: u32 = 54;

/// Dropdown row height (px) — about half the menu button — and label size, for
/// the compact settings dropdowns.
pub const DD_ROW_H: f32 = 26.0;
pub const DD_LABEL_SIZE: f32 = 15.0;

/// Stroke width (px) for the gold filigree curves.
const FIL_STROKE: f32 = 2.0;

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

    /// Add `d` to each RGB channel of an existing pixel (signed, clamped) —
    /// shades a bevel into already-painted stone without touching alpha.
    fn shade_px(&mut self, x: i32, y: i32, d: i32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        let f = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
        self.px[i] = f(self.px[i]);
        self.px[i + 1] = f(self.px[i + 1]);
        self.px[i + 2] = f(self.px[i + 2]);
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
    fn rect_outline(
        &mut self,
        origin: (usize, usize),
        size: (usize, usize),
        c: Rgb,
        a: u8,
        t: usize,
    ) {
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

    /// Flat slate fill with only a gentle large-scale value drift — simpler
    /// and less metallic than fine noise (subtle blended rock is enough).
    fn stone_fill(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w {
                let n = vnoise(x as f32 / 34.0, y as f32 / 30.0);
                self.put(
                    x as i32,
                    y as i32,
                    lerp_rgb(SLATE_DK, SLATE_HI, 0.32 + 0.18 * n),
                    255,
                );
            }
        }
    }

    /// Shade a raised 3D bevel into the outermost `width` px: lit on the
    /// top/left rim, shadowed on the bottom/right, with a noisy falloff so the
    /// chamfer reads as irregular chiselled stone rather than a clean CAD edge.
    /// `strength` is the peak per-channel shift.
    fn outer_bevel(&mut self, width: i32, strength: f32) {
        let (wi, hi) = (self.w as i32, self.h as i32);
        for y in 0..hi {
            for x in 0..wi {
                let top = (width - y).max(0);
                let left = (width - x).max(0);
                let bot = (width - (hi - 1 - y)).max(0);
                let right = (width - (wi - 1 - x)).max(0);
                let net = (top + left - bot - right) as f32;
                if net == 0.0 {
                    continue;
                }
                let jitter = 0.55 + 0.9 * vnoise(x as f32 / 6.0, y as f32 / 6.0);
                let d = (net / width as f32 * strength * jitter) as i32;
                if d != 0 {
                    self.shade_px(x, y, d);
                }
            }
        }
    }

    /// A faint jagged hairline crack walking from `start` along `dir`.
    fn crack(&mut self, seed: u32, start: (i32, i32), dir: f32, len: i32) {
        let (mut x, mut y) = (start.0 as f32, start.1 as f32);
        let mut a = dir;
        for i in 0..len {
            let (px, py) = (x.round() as i32, y.round() as i32);
            self.put(px, py, SLATE_CRACK, 255);
            a += (hash01(seed as i32 * 31 + i, px ^ py) - 0.5) * 0.9;
            x += a.cos();
            y += a.sin();
        }
    }

    /// Stamp a filled disc of diameter ~`t` — the brush for curved strokes.
    fn dot(&mut self, x: f32, y: f32, t: f32, col: Rgb) {
        let rad = t * 0.5;
        let r = rad.ceil() as i32;
        let (ix, iy) = (x.round() as i32, y.round() as i32);
        for dy in -r..=r {
            for dx in -r..=r {
                if (dx * dx + dy * dy) as f32 <= rad * rad + 0.4 {
                    self.put(ix + dx, iy + dy, col, 255);
                }
            }
        }
    }

    /// Stroke a circular arc (`a0`→`a1` radians, radius `radius`) about `c`.
    fn arc(&mut self, c: (f32, f32), radius: f32, a0: f32, a1: f32, t: f32, col: Rgb) {
        let steps = ((a1 - a0).abs() * radius).max(12.0) as i32;
        for i in 0..=steps {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            self.dot(c.0 + radius * a.cos(), c.1 + radius * a.sin(), t, col);
        }
    }

    /// Stroke an Archimedean spiral about `c` (radius `radii.0`→`radii.1`
    /// over `sweep`).
    fn spiral(&mut self, c: (f32, f32), radii: (f32, f32), a0: f32, sweep: f32, t: f32, col: Rgb) {
        let (r0, r1) = radii;
        let steps = (sweep.abs() * r0.max(r1)).max(16.0) as i32;
        for i in 0..=steps {
            let f = i as f32 / steps as f32;
            let a = a0 + sweep * f;
            let r = r0 + (r1 - r0) * f;
            self.dot(c.0 + r * a.cos(), c.1 + r * a.sin(), t, col);
        }
    }

    /// Flat two-tone gold arc: a `GOLD_DK` shadow offset under a `GOLD` stroke.
    fn g_arc(&mut self, c: (f32, f32), radius: f32, a0: f32, a1: f32) {
        self.arc((c.0 + 1.0, c.1 + 1.5), radius, a0, a1, FIL_STROKE, GOLD_DK);
        self.arc(c, radius, a0, a1, FIL_STROKE, GOLD);
    }

    /// Flat two-tone gold spiral (shadow under stroke).
    fn g_spiral(&mut self, c: (f32, f32), r0: f32, r1: f32, a0: f32, sweep: f32) {
        self.spiral(
            (c.0 + 1.0, c.1 + 1.5),
            (r0, r1),
            a0,
            sweep,
            FIL_STROKE,
            GOLD_DK,
        );
        self.spiral(c, (r0, r1), a0, sweep, FIL_STROKE, GOLD);
    }

    /// A flat gold frame line at `origin`/`size` over a thin inner shadow.
    fn g_frame(&mut self, origin: (usize, usize), size: (usize, usize)) {
        self.rect_outline(origin, size, GOLD, 255, 1);
        self.rect_outline(
            (origin.0 + 1, origin.1 + 1),
            (size.0 - 2, size.1 - 2),
            GOLD_DK,
            220,
            1,
        );
    }

    /// A corner scroll/curl rooted at panel-corner `(x, y)`; `(sx, sy)` point
    /// inward (toward the centre) and mirror the handedness across corners.
    fn corner_scroll(&mut self, x: f32, y: f32, sx: f32, sy: f32) {
        let cen = (x + sx * 15.0, y + sy * 15.0);
        let base = (-sy).atan2(-sx); // spiral centre → the corner
        self.g_spiral(cen, 13.0, 2.5, base, -(sx * sy) * 1.55 * TAU);
        self.g_arc(
            (x + sx * 46.0, y + sy * 22.0),
            28.0,
            base,
            base + sx * sy * 0.6,
        );
        self.g_arc(
            (x + sx * 22.0, y + sy * 46.0),
            28.0,
            base,
            base - sx * sy * 0.6,
        );
    }

    /// A symmetric twin-curl crest at an edge midpoint `(x, y)`. `horiz` = the
    /// edge runs horizontally (top/bottom rail), else vertical (left/right).
    fn center_flourish(&mut self, x: f32, y: f32, horiz: bool) {
        if horiz {
            self.g_spiral((x - 12.0, y), 7.0, 1.5, 0.0, 1.45 * TAU);
            self.g_spiral((x + 12.0, y), 7.0, 1.5, PI, -1.45 * TAU);
            self.g_arc((x, y + 13.0), 14.0, 1.18 * PI, 1.82 * PI);
        } else {
            self.g_spiral((x, y - 12.0), 7.0, 1.5, 0.5 * PI, 1.45 * TAU);
            self.g_spiral((x, y + 12.0), 7.0, 1.5, -0.5 * PI, -1.45 * TAU);
            self.g_arc((x + 13.0, y), 14.0, 0.68 * PI, 1.32 * PI);
        }
    }

    fn into_pixels(self) -> Vec<u8> {
        self.px
    }
}

// ===== color helpers =====

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
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

/// Hashed lattice value in `0.0..=1.0` at integer point `(x, y)`.
fn hash01(x: i32, y: i32) -> f32 {
    let mut h = (x as u32)
        .wrapping_mul(374_761_393)
        .wrapping_add((y as u32).wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65_535.0
}

/// Smooth value noise in `0.0..=1.0` — bilinear-interpolated lattice hash,
/// for the mottled carved-stone grain.
fn vnoise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (ux, uy) = (smooth(fx), smooth(fy));
    let a = hash01(xi, yi);
    let b = hash01(xi + 1, yi);
    let cc = hash01(xi, yi + 1);
    let d = hash01(xi + 1, yi + 1);
    let top = a + (b - a) * ux;
    let bot = cc + (d - cc) * ux;
    top + (bot - top) * uy
}

// ===== procedural textures =====

/// The modal panel: weathered dark-metal field with a vignette, a double
/// frame (rust outer / gold inner), brightened gold corner brackets, and a
/// gold rule under the title band.
fn build_panel() -> Vec<u8> {
    let (w, h) = (PANEL_W as usize, PANEL_H as usize);
    let f = FRAME as usize;
    let mut c = Canvas::new(w, h);

    // 1. Flat slate frame with a gentle drift + a couple of faint cracks.
    c.stone_fill();
    c.crack(7, (118, 0), 1.85, 120);
    c.crack(41, (w as i32 - 36, h as i32), -1.9, 120);

    // 2. Raised 3D bevel on the outer edge (lit top/left, shadowed bottom/
    //    right) with an irregular chiselled falloff for depth.
    c.outer_bevel(8, 16.0);

    // 3. Recessed content well: flat dark field with a soft vignette.
    let (cw, ch) = (w - 2 * f, h - 2 * f);
    for yy in 0..ch {
        let base = lerp_rgb(SURFACE_TOP, SURFACE_BOT, yy as f32 / (ch - 1) as f32);
        for xx in 0..cw {
            let edge = (xx.min(cw - 1 - xx)).min(yy.min(ch - 1 - yy)) as f32;
            let vig = 0.7 + 0.3 * (edge / 40.0).min(1.0);
            c.put((f + xx) as i32, (f + yy) as i32, scale(base, vig), 255);
        }
    }
    c.rect_outline((f - 2, f - 2), (cw + 4, ch + 4), INK, 150, 2); // recess shadow lip

    // 4. Tarnished-gold filigree: a thin frame line, corner scrolls, and a
    //    twin-curl crest centred on each edge.
    c.g_frame((f - 7, f - 7), (cw + 14, ch + 14));
    let (x0, y0) = ((f - 7) as f32, (f - 7) as f32);
    let (x1, y1) = ((w - f + 6) as f32, (h - f + 6) as f32);
    c.corner_scroll(x0, y0, 1.0, 1.0);
    c.corner_scroll(x1, y0, -1.0, 1.0);
    c.corner_scroll(x0, y1, 1.0, -1.0);
    c.corner_scroll(x1, y1, -1.0, -1.0);
    let (mx, my) = (w as f32 * 0.5, h as f32 * 0.5);
    c.center_flourish(mx, (f / 2) as f32, true);
    c.center_flourish(mx, (h - f / 2) as f32, true);
    c.center_flourish((f / 2) as f32, my, false);
    c.center_flourish((w - f / 2) as f32, my, false);

    // 5. Title cartouche inside the well, framed by flat gold rules — the
    //    runtime draws "PAUSED" here (`pause_layout::title_y` lands inside it).
    let (band_x, band_w) = (f + 6, cw - 12);
    let (band_y, band_h) = (f + 10, 60usize);
    c.fill_rect(
        (band_x, band_y),
        (band_w, band_h),
        shade(SURFACE_BOT, -8),
        255,
    );
    c.hline(band_x, band_x + band_w, band_y, GOLD, 255);
    c.hline(band_x, band_x + band_w, band_y + band_h, GOLD, 255);
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
            c.put(
                x as i32,
                y as i32,
                shade(base, grain(x + 7, y + 13, 6) + recess),
                255,
            );
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
pub enum ModalButton {
    Top,
    Bottom,
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

/// Screen-space placement of a centred panel with two stacked buttons,
/// recomputed from the viewport each frame so `update` (hit-testing) and
/// `render` (drawing) agree.
pub struct ModalLayout {
    panel: Rect,
    title_y: f32,
    top: Rect,
    bottom: Rect,
}

/// Centre the fixed-size panel on the screen and stack its two buttons.
pub fn modal_layout(screen: Vec2) -> ModalLayout {
    let (pw, ph) = (PANEL_W as f32, PANEL_H as f32);
    let px = ((screen.x - pw) * 0.5).round();
    let py = ((screen.y - ph) * 0.5).round();
    let frame = FRAME as f32;
    let (bw, bh) = (BUTTON_W as f32, BUTTON_H as f32);
    let bx = px + (pw - bw) * 0.5;
    ModalLayout {
        panel: Rect {
            x: px,
            y: py,
            w: pw,
            h: ph,
        },
        title_y: py + frame + 22.0,
        top: Rect {
            x: bx,
            y: py + frame + 108.0,
            w: bw,
            h: bh,
        },
        bottom: Rect {
            x: bx,
            y: py + frame + 184.0,
            w: bw,
            h: bh,
        },
    }
}

impl ModalLayout {
    /// Which button (if any) the cursor is over.
    #[must_use]
    pub fn hover(&self, cursor: Vec2) -> Option<ModalButton> {
        if self.top.contains(cursor) {
            Some(ModalButton::Top)
        } else if self.bottom.contains(cursor) {
            Some(ModalButton::Bottom)
        } else {
            None
        }
    }
}

/// Uploaded UI textures — just handles, so `Theme` is `Copy` and cheap to
/// share between scenes (the textures are uploaded once and never freed).
#[derive(Copy, Clone)]
pub struct Theme {
    panel: TextureHandle,
    button: TextureHandle,
    white: TextureHandle,
}

impl Theme {
    /// Generate + upload the gothic raster art, including the shared 1×1 white
    /// pixel (reused for scrim, backdrop, hover sheen, and outlines).
    pub fn build(renderer: &mut Renderer) -> Self {
        let white = renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1);
        let panel = renderer.load_texture(&build_panel(), PANEL_W, PANEL_H);
        let button = renderer.load_texture(&build_button(), BUTTON_W, BUTTON_H);
        Self {
            panel,
            button,
            white,
        }
    }

    /// The engine textures this theme exposes to a Lua screen, as
    /// `(name, handle)` in id order — the index is the id the script references
    /// via `Textures.<name>` (see [`ScriptHost::set_texture_ids`] and the
    /// consumer's `render_hud`). `white` is id 0 so it doubles as the rect fill.
    ///
    /// [`ScriptHost::set_texture_ids`]: flicker::script::ScriptHost::set_texture_ids
    pub fn lua_textures(&self) -> [(&'static str, TextureHandle); 3] {
        [
            ("white", self.white),
            ("panel", self.panel),
            ("button", self.button),
        ]
    }

    /// Translucent full-screen scrim — dims a frozen scene behind a modal.
    pub fn scrim(&self, r: &mut Renderer, screen: Vec2) {
        r.draw_sprite(self.white, Vec2::ZERO, screen, COL_SCRIM);
    }

    /// Opaque full-screen dark backdrop — for a menu with nothing behind it.
    pub fn backdrop(&self, r: &mut Renderer, screen: Vec2) {
        r.draw_sprite(self.white, Vec2::ZERO, screen, COL_BACKDROP);
    }

    /// A light full-screen black dim at `alpha` (0..1) — keeps the scene behind
    /// visible (e.g. to preview a resolution change before confirming it).
    pub fn dim(&self, r: &mut Renderer, screen: Vec2, alpha: f32) {
        r.draw_sprite(self.white, Vec2::ZERO, screen, [0.0, 0.0, 0.0, alpha]);
    }

    /// Draw the gothic panel: `title` in the cartouche and two labelled
    /// buttons (`labels.0` over `labels.1`) with hover lighting. The caller
    /// draws the backdrop/scrim first (see [`Self::scrim`] / [`Self::backdrop`]).
    pub fn draw_panel(
        &self,
        r: &mut Renderer,
        layout: &ModalLayout,
        title: &str,
        subtitle: Option<&str>,
        labels: (&str, &str),
        hover: Option<ModalButton>,
    ) {
        r.draw_sprite(
            self.panel,
            Vec2::new(layout.panel.x, layout.panel.y),
            Vec2::new(layout.panel.w, layout.panel.h),
            [1.0, 1.0, 1.0, 1.0],
        );
        centered_text(r, title, layout.panel, layout.title_y, 34.0, COL_TITLE);
        if let Some(sub) = subtitle {
            centered_text(r, sub, layout.panel, layout.title_y + 40.0, 18.0, COL_LABEL);
        }
        self.draw_button(r, &layout.top, labels.0, hover == Some(ModalButton::Top));
        self.draw_button(
            r,
            &layout.bottom,
            labels.1,
            hover == Some(ModalButton::Bottom),
        );
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

    /// Draw the loading screen: opaque backdrop, the gothic panel titled
    /// "LOADING", and a tarnished-gold progress bar filled to `progress`
    /// (0..=1) in the panel's content well.
    pub fn draw_loading(&self, r: &mut Renderer, screen: Vec2, progress: f32) {
        let layout = modal_layout(screen);
        self.backdrop(r, screen);
        r.draw_sprite(
            self.panel,
            Vec2::new(layout.panel.x, layout.panel.y),
            Vec2::new(layout.panel.w, layout.panel.h),
            [1.0, 1.0, 1.0, 1.0],
        );
        centered_text(r, "LOADING", layout.panel, layout.title_y, 34.0, COL_TITLE);

        let p = layout.panel;
        let frame = FRAME as f32;
        let bar = Rect {
            x: p.x + frame + 22.0,
            y: p.y + p.h * 0.52,
            w: p.w - 2.0 * (frame + 22.0),
            h: 22.0,
        };
        r.draw_sprite(
            self.white,
            Vec2::new(bar.x, bar.y),
            Vec2::new(bar.w, bar.h),
            COL_BAR_TRACK,
        );
        let fill = (bar.w * progress.clamp(0.0, 1.0)).round();
        if fill > 0.0 {
            r.draw_sprite(
                self.white,
                Vec2::new(bar.x, bar.y),
                Vec2::new(fill, bar.h),
                COL_BAR_FILL,
            );
        }
        outline(r, self.white, &bar, 2.0, COL_GOLD_LINE);
    }

    /// Draw a large centred wordmark (the logo splash) over the current
    /// backdrop — no panel.
    pub fn wordmark(&self, r: &mut Renderer, screen: Vec2, text: &str) {
        let size = 72.0;
        let container = Rect {
            x: 0.0,
            y: 0.0,
            w: screen.x,
            h: screen.y,
        };
        centered_text(
            r,
            text,
            container,
            screen.y * 0.5 - size * 0.5,
            size,
            COL_TITLE,
        );
    }
}

/// Estimate the label width and draw it horizontally centred in `container`,
/// with its top-left at `y`.
fn centered_text(
    r: &mut Renderer,
    text: &str,
    container: Rect,
    y: f32,
    size: f32,
    color: [f32; 4],
) {
    let w = r.measure_text(text, size).x;
    let x = (container.x + (container.w - w) * 0.5).max(container.x);
    r.draw_text(text, Vec2::new(x, y), size, color);
}

/// Draw a `t`-thick rectangle outline with the white texture (screen space).
fn outline(r: &mut Renderer, white: TextureHandle, rect: &Rect, t: f32, color: [f32; 4]) {
    r.draw_sprite(
        white,
        Vec2::new(rect.x, rect.y),
        Vec2::new(rect.w, t),
        color,
    );
    r.draw_sprite(
        white,
        Vec2::new(rect.x, rect.y + rect.h - t),
        Vec2::new(rect.w, t),
        color,
    );
    r.draw_sprite(
        white,
        Vec2::new(rect.x, rect.y),
        Vec2::new(t, rect.h),
        color,
    );
    r.draw_sprite(
        white,
        Vec2::new(rect.x + rect.w - t, rect.y),
        Vec2::new(t, rect.h),
        color,
    );
}

// ===== compact dropdown widget (settings panel) =====

/// A compact dropdown: a header showing the current choice that expands to a
/// list of rows. Drawn with primitives (no baked texture), so it sizes to any
/// row count. State is just open/closed; the selected index and the labels live
/// with the caller (e.g. the settings panel).
pub struct Dropdown {
    open: bool,
}

impl Dropdown {
    #[must_use]
    pub fn new() -> Self {
        Self { open: false }
    }

    /// Screen height the dropdown occupies right now (just the header, plus the
    /// rows when open) — for stacking widgets beneath it.
    #[must_use]
    pub fn height(&self, items: usize) -> f32 {
        if self.open {
            DD_ROW_H * (items as f32 + 1.0)
        } else {
            DD_ROW_H
        }
    }

    /// Handle a click at `cursor`. `anchor` is the header's top-left, `width`
    /// the box width, `items` the row count. A header click toggles open; a row
    /// click returns `Some(index)` and closes; any other click closes.
    pub fn click(&mut self, anchor: Vec2, width: f32, items: usize, cursor: Vec2) -> Option<usize> {
        let (header, rows) = dd_layout(anchor, width, items);
        if header.contains(cursor) {
            self.open = !self.open;
            return None;
        }
        if self.open {
            for (i, row) in rows.iter().enumerate() {
                if row.contains(cursor) {
                    self.open = false;
                    return Some(i);
                }
            }
            self.open = false;
        }
        None
    }

    /// Draw the dropdown: a header showing `items[selected]` plus a caret, and —
    /// when open — the rows (the active one lit, the hovered one highlighted).
    pub fn draw(
        &self,
        theme: &Theme,
        r: &mut Renderer,
        place: (Vec2, f32),
        items: &[String],
        selected: usize,
        cursor: Vec2,
    ) {
        let (anchor, width) = place;
        let (header, rows) = dd_layout(anchor, width, items.len());
        dd_cell(theme, r, &header, false);
        if let Some(label) = items.get(selected) {
            dd_label(r, label, &header, COL_LABEL);
        }
        dd_caret(r, &header, self.open);
        if self.open {
            for (i, row) in rows.iter().enumerate() {
                let hot = row.contains(cursor);
                dd_cell(theme, r, row, hot);
                let col = if i == selected {
                    COL_LABEL_HOVER
                } else {
                    COL_LABEL
                };
                dd_label(r, &items[i], row, col);
            }
        }
    }
}

impl Default for Dropdown {
    fn default() -> Self {
        Self::new()
    }
}

/// Header rect + one rect per row for a dropdown at `anchor`.
fn dd_layout(anchor: Vec2, width: f32, items: usize) -> (Rect, Vec<Rect>) {
    let header = Rect {
        x: anchor.x,
        y: anchor.y,
        w: width,
        h: DD_ROW_H,
    };
    let rows = (0..items)
        .map(|i| Rect {
            x: anchor.x,
            y: anchor.y + DD_ROW_H * (i as f32 + 1.0),
            w: width,
            h: DD_ROW_H,
        })
        .collect();
    (header, rows)
}

/// A dropdown cell: slate fill (lit when `hot`) + a thin gold border.
fn dd_cell(theme: &Theme, r: &mut Renderer, rect: &Rect, hot: bool) {
    let fill = if hot { COL_DD_HOT } else { COL_DD_FILL };
    r.draw_sprite(
        theme.white,
        Vec2::new(rect.x, rect.y),
        Vec2::new(rect.w, rect.h),
        fill,
    );
    outline(r, theme.white, rect, 1.0, COL_GOLD_LINE);
}

/// Left-aligned dropdown label, vertically centred in `rect`.
fn dd_label(r: &mut Renderer, text: &str, rect: &Rect, color: [f32; 4]) {
    r.draw_text(
        text,
        Vec2::new(rect.x + 10.0, rect.y + (rect.h - DD_LABEL_SIZE) * 0.5),
        DD_LABEL_SIZE,
        color,
    );
}

/// A small gold caret at the right of a dropdown header (down = closed,
/// up = open), drawn as a filled triangle so it needs no glyph.
fn dd_caret(r: &mut Renderer, header: &Rect, open: bool) {
    let cx = header.x + header.w - 14.0;
    let cy = header.y + header.h * 0.5;
    let s = 4.0;
    let (a, b, c) = if open {
        (
            Vec2::new(cx - s, cy + s * 0.6),
            Vec2::new(cx + s, cy + s * 0.6),
            Vec2::new(cx, cy - s * 0.6),
        )
    } else {
        (
            Vec2::new(cx - s, cy - s * 0.6),
            Vec2::new(cx + s, cy - s * 0.6),
            Vec2::new(cx, cy + s * 0.6),
        )
    };
    r.draw_triangle(a, b, c, COL_GOLD_LINE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn save(name: &str, w: u32, h: u32, px: Vec<u8>) {
        let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(w, h, px).expect("size matches");
        // Workspace `target/` (already git-ignored) so the preview never lands
        // in the tracked tree.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(name);
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
