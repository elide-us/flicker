//! The flicker-shell UI theme — the shared Prism chrome the front-end screens
//! draw with. It (1) registers the three Prism serif faces, (2) uploads the
//! shared 1×1 white pixel + the **Muse** main-menu character (with a baked
//! left-edge alpha fade), and (3) still bakes the procedural carved-stone panel /
//! button / settings-panel sprite textures — kept as a 2D-sprite facility even
//! though the menu / pause / settings now draw their chrome with *vector* `panel`
//! commands (rounded-rect + gradient + border). `Theme::lua_textures` hands the
//! textures to a Lua screen by name; `draw_loading` renders the Rust loading
//! widget (reskinned to the same flat vector chrome). The menu / pause / confirm
//! / logo screens are Lua + the embedded `ui_elements.json`.

use flicker::render::{Renderer, TextureHandle, Vec2};

// ===== palette — the Prism design language =====
// Cold carved stone lit by sapphire rune-light; aged bronze is the only
// structural metal. Baked once into the panel/button chrome; mirrors the
// `theme.tokens` in Alpha/content/sensorium/resources/ui_elements.json (the Lua-side
// single source of the same palette).

type Rgb = (u8, u8, u8);

/// Recessed content-well gradient (sunk dark stone, top → bottom).
const SURFACE_TOP: Rgb = (20, 23, 31);
const SURFACE_BOT: Rgb = (11, 13, 18);
/// Button plate gradient — the sapphire slab (base → deep).
const PLATE_TOP: Rgb = (36, 63, 120);
const PLATE_BOT: Rgb = (21, 39, 68);
/// Cool stone bevel highlight.
const SILVER: Rgb = (120, 135, 162);
/// Aged bronze — the only structural metal (engraved channel + frame).
const GOLD: Rgb = (184, 151, 90);
/// Deep bronze patina shadow (flat 2-tone with `GOLD`).
const GOLD_DK: Rgb = (110, 90, 52);
/// Sapphire rune-light — the interactive accent (button edge, corner inlays).
const SAPPHIRE: Rgb = (58, 90, 160);
/// Lit rune-glow — the inlay's bright core.
const RUNE: Rgb = (111, 151, 255);
/// Carved slate for the stone frame: cool base + highlight.
const SLATE_DK: Rgb = (22, 26, 34);
const SLATE_HI: Rgb = (45, 52, 66);
const INK: Rgb = (8, 9, 12);

/// Ink title text.
pub const COL_TITLE: [f32; 4] = [0.906, 0.882, 0.824, 1.0];
/// Loading-bar rim (bronze).
const COL_GOLD_LINE: [f32; 4] = [0.722, 0.592, 0.353, 0.95];
/// Opaque dark backdrop for a full-screen menu (nothing behind it).
const COL_BACKDROP: [f32; 4] = [0.031, 0.035, 0.047, 1.0];
/// Loading-bar track (recessed dark) and sapphire fill.
const COL_BAR_TRACK: [f32; 4] = [0.055, 0.063, 0.086, 1.0];
const COL_BAR_FILL: [f32; 4] = [0.141, 0.247, 0.471, 1.0];

// Flat vector-panel colours for the (reskinned) loading screen — the Prism
// carved-stone look drawn with `draw_ui_panel` (rounded-rect + gradient + border
// + soft shadow) instead of the baked panel sprite. Mirror the `theme.tokens`
// (stone3→stone1 fill, edge2 border).
const LOAD_PANEL_TOP: [f32; 4] = [0.110, 0.125, 0.161, 1.0];
const LOAD_PANEL_BOT: [f32; 4] = [0.055, 0.063, 0.086, 1.0];
const LOAD_BORDER: [f32; 4] = [0.169, 0.188, 0.235, 1.0];
const LOAD_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.55];

/// The Muse — the main-menu character, embedded so every shell app inherits her.
const MUSE_IMAGE: &[u8] = include_bytes!("../../../../content/sensorium/assets/muse.png");

// ===== texture sizes (drawn 1:1, so the baked borders never distort) =====

const PANEL_W: u32 = 520;
// Tall enough for a title band + three stacked buttons (menu/pause) with even
// margins top and bottom; the POC's 384 crammed the 3rd button against the
// bottom filigree. Drawn 1:1 (see below), so the frame art scales without
// distorting. The confirm dialog (2 buttons) just carries a little more empty
// space below, which reads fine.
const PANEL_H: u32 = 420;
/// Settings panel: wider for the 3-tab layout.
const SETTINGS_PANEL_W: u32 = 800;
const SETTINGS_PANEL_H: u32 = 500;
/// Stone-frame thickness (px) around the recessed content well.
const FRAME: u32 = 38;
const BUTTON_W: u32 = 264;
const BUTTON_H: u32 = 54;

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

    /// Stamp a filled disc of diameter ~`t` — the brush for the rune inlay.
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

    /// An engraved bronze channel at `origin`/`size` over a thin inner shadow.
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

    /// A small sapphire "rune inlay" gem at a frame corner: a lit rune-glow
    /// core set in sapphire on a dark seat — the Prism carved-stone signature.
    fn rune_mark(&mut self, x: f32, y: f32) {
        self.dot(x, y, 8.0, shade(SAPPHIRE, -46));
        self.dot(x, y, 6.0, SAPPHIRE);
        self.dot(x, y, 3.0, RUNE);
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

    // 1. Flat carved-stone frame with a gentle value drift.
    c.stone_fill();

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

    // 4. A thin engraved bronze channel around the well + a sapphire rune
    //    inlay set at each corner (the Prism carved-stone signature).
    c.g_frame((f - 7, f - 7), (cw + 14, ch + 14));
    let (x0, y0) = ((f - 7) as f32, (f - 7) as f32);
    let (x1, y1) = ((w - f + 6) as f32, (h - f + 6) as f32);
    c.rune_mark(x0, y0);
    c.rune_mark(x1, y0);
    c.rune_mark(x0, y1);
    c.rune_mark(x1, y1);

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

/// Wider settings panel (800×500) with the same gothic styling as the
/// modal panel but scaled for the 3-tab layout.
fn build_settings_panel() -> Vec<u8> {
    let (w, h) = (SETTINGS_PANEL_W as usize, SETTINGS_PANEL_H as usize);
    let f = FRAME as usize;
    let mut c = Canvas::new(w, h);

    // 1. Flat carved-stone frame with a gentle value drift.
    c.stone_fill();

    // 2. Raised 3D bevel on the outer edge.
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
    c.rect_outline((f - 2, f - 2), (cw + 4, ch + 4), INK, 150, 2);

    // 4. A thin engraved bronze channel around the well + a sapphire rune
    //    inlay set at each corner (the Prism carved-stone signature).
    c.g_frame((f - 7, f - 7), (cw + 14, ch + 14));
    let (x0, y0) = ((f - 7) as f32, (f - 7) as f32);
    let (x1, y1) = ((w - f + 6) as f32, (h - f + 6) as f32);
    c.rune_mark(x0, y0);
    c.rune_mark(x1, y0);
    c.rune_mark(x0, y1);
    c.rune_mark(x1, y1);

    // 5. Title band at the top of the content well.
    let (band_x, band_w) = (f + 6, cw - 12);
    let (band_y, band_h) = (f + 6, 50usize);
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
    // Sapphire rune-edge over a thin inner shadow for depth.
    c.rect_outline((2, 2), (w - 4, h - 4), SAPPHIRE, 240, 1);
    c.rect_outline((3, 3), (w - 6, h - 6), shade(INK, 18), 80, 1);
    c.into_pixels()
}

/// Decode the embedded Muse PNG and bake a left→right alpha ramp into her
/// (otherwise opaque) pixels — transparent at the left edge, opaque by 42% of the
/// width — so the sprite dissolves toward screen-centre when the menu draws her at
/// the right margin. Falls back to a 1×1 white pixel if decoding fails (the menu
/// guards on the texture, so a failure just drops the character).
fn load_muse(renderer: &mut Renderer) -> TextureHandle {
    match image::load_from_memory(MUSE_IMAGE) {
        Ok(img) => {
            let mut rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let fade_end = (w as f32 * 0.42).max(1.0);
            for (x, _y, px) in rgba.enumerate_pixels_mut() {
                let t = (x as f32 / fade_end).clamp(0.0, 1.0);
                let s = t * t * (3.0 - 2.0 * t); // smoothstep — a soft, curved fade
                px[3] = (px[3] as f32 * s).round() as u8;
            }
            renderer.load_texture(&rgba, w, h)
        }
        Err(e) => {
            tracing::error!("failed to decode muse.png: {e}");
            renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1)
        }
    }
}

// ===== layout + widgets =====

#[derive(Copy, Clone)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Screen-space placement of the centred gothic panel — used by the loading
/// widget (the menu/pause/confirm modals are now Lua-driven). Recomputed from
/// the viewport each frame.
pub struct ModalLayout {
    panel: Rect,
    title_y: f32,
}

/// Centre the fixed-size panel on the screen.
pub fn modal_layout(screen: Vec2) -> ModalLayout {
    let (pw, ph) = (PANEL_W as f32, PANEL_H as f32);
    let px = ((screen.x - pw) * 0.5).round();
    let py = ((screen.y - ph) * 0.5).round();
    let frame = FRAME as f32;
    ModalLayout {
        panel: Rect {
            x: px,
            y: py,
            w: pw,
            h: ph,
        },
        title_y: py + frame + 22.0,
    }
}

/// Uploaded UI textures — just handles, so `Theme` is `Copy` and cheap to
/// share between scenes (the textures are uploaded once and never freed).
#[derive(Copy, Clone)]
pub struct Theme {
    panel: TextureHandle,
    settings_panel: TextureHandle,
    button: TextureHandle,
    white: TextureHandle,
    /// The main-menu character (left-edge alpha fade baked in). Exposed to Lua as
    /// `Textures.muse`; only the menu screen draws it.
    muse: TextureHandle,
}

impl Theme {
    /// Register the Prism UI faces + generate + upload the carved-stone raster
    /// art, including the shared 1×1 white pixel (reused for scrim, backdrop,
    /// hover sheen, and outlines).
    pub fn build(renderer: &mut Renderer) -> Self {
        // The six Prism faces (Alpha/content/sensorium/fonts) registered under their role
        // family names so `FontRole` + italic/bold select them: five instanced
        // text weights + the renamed `Prism Rune` (Noto Sans Runic) for glyphs.
        // Any glyph a face lacks falls back to a system font.
        renderer.register_ui_font(include_bytes!(
            "../../../../content/sensorium/fonts/CormorantGaramond-SemiBold.ttf" // Prism Display 600
        ));
        renderer.register_ui_font(include_bytes!(
            "../../../../content/sensorium/fonts/CormorantGaramond-Bold.ttf" // Prism Display 700 (bold)
        ));
        renderer.register_ui_font(include_bytes!(
            "../../../../content/sensorium/fonts/Cinzel-SemiBold.ttf" // Prism Label 600 (tracked caps)
        ));
        renderer.register_ui_font(include_bytes!(
            "../../../../content/sensorium/fonts/EBGaramond-Regular.ttf" // Prism Body 400
        ));
        renderer.register_ui_font(include_bytes!(
            "../../../../content/sensorium/fonts/EBGaramond-Italic.ttf" // Prism Body italic (flavor)
        ));
        renderer.register_ui_font(include_bytes!(
            "../../../../content/sensorium/fonts/NotoSansRunic-Prism.ttf" // Prism Rune (corner glyphs)
        ));

        let white = renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1);
        let panel = renderer.load_texture(&build_panel(), PANEL_W, PANEL_H);
        let settings_panel = renderer.load_texture(&build_settings_panel(), SETTINGS_PANEL_W, SETTINGS_PANEL_H);
        let button = renderer.load_texture(&build_button(), BUTTON_W, BUTTON_H);
        let muse = load_muse(renderer);
        Self {
            panel,
            settings_panel,
            button,
            white,
            muse,
        }
    }

    /// The engine textures this theme exposes to a Lua screen, as
    /// `(name, handle)` in id order — the index is the id the script references
    /// via `Textures.<name>` (see [`ScriptHost::set_texture_ids`] and the
    /// consumer's `render_hud`). `white` is id 0 so it doubles as the rect fill.
    ///
    /// [`ScriptHost::set_texture_ids`]: flicker::script::ScriptHost::set_texture_ids
    pub fn lua_textures(&self) -> [(&'static str, TextureHandle); 5] {
        [
            ("white", self.white),
            ("panel", self.panel),
            ("settings_panel", self.settings_panel),
            ("button", self.button),
            ("muse", self.muse),
        ]
    }

    /// Opaque full-screen dark backdrop — for a menu with nothing behind it.
    pub fn backdrop(&self, r: &mut Renderer, screen: Vec2) {
        r.draw_sprite(self.white, Vec2::ZERO, screen, COL_BACKDROP);
    }

    /// Draw the loading screen: opaque backdrop, the gothic panel titled
    /// "LOADING", and a tarnished-gold progress bar filled to `progress`
    /// (0..=1) in the panel's content well.
    pub fn draw_loading(&self, r: &mut Renderer, screen: Vec2, progress: f32) {
        let layout = modal_layout(screen);
        // Flat Prism chrome (vector): backdrop, soft drop shadow, then the panel —
        // drawn as ui-panels so they sort behind the sprite bar + text that follow.
        r.draw_ui_panel(Vec2::ZERO, screen, COL_BACKDROP, COL_BACKDROP, 0.0, 0.0, 0.0, [0.0; 4], 0.0);
        r.draw_ui_panel(
            Vec2::new(layout.panel.x - 4.0, layout.panel.y + 16.0),
            Vec2::new(layout.panel.w + 8.0, layout.panel.h + 8.0),
            LOAD_SHADOW,
            LOAD_SHADOW,
            0.0,
            17.0,
            0.0,
            [0.0; 4],
            44.0,
        );
        r.draw_ui_panel(
            Vec2::new(layout.panel.x, layout.panel.y),
            Vec2::new(layout.panel.w, layout.panel.h),
            LOAD_PANEL_TOP,
            LOAD_PANEL_BOT,
            1.0,
            5.0,
            1.0,
            LOAD_BORDER,
            0.0,
        );
        centered_text(r, "LOADING", layout.panel, layout.title_y, 34.0, COL_TITLE, true);

        let p = layout.panel;
        let frame = FRAME as f32;
        let bar = Rect {
            x: p.x + frame + 22.0,
            y: p.y + p.h * 0.52,
            w: p.w - 2.0 * (frame + 22.0),
            h: 22.0,
        };
        // Recessed track with a bronze rim + sapphire fill, drawn as vector
        // panels so they stay colour-correct (sRGB) like the panel above.
        r.draw_ui_panel(
            Vec2::new(bar.x, bar.y),
            Vec2::new(bar.w, bar.h),
            COL_BAR_TRACK,
            COL_BAR_TRACK,
            0.0,
            4.0,
            1.0,
            COL_GOLD_LINE,
            0.0,
        );
        let fill = (bar.w * progress.clamp(0.0, 1.0)).round();
        if fill > 0.0 {
            r.draw_ui_panel(
                Vec2::new(bar.x, bar.y),
                Vec2::new(fill, bar.h),
                COL_BAR_FILL,
                COL_BAR_FILL,
                0.0,
                4.0,
                0.0,
                [0.0; 4],
                0.0,
            );
        }
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
    bold: bool,
) {
    let role = flicker::render::FontRole::Display;
    let w = r.measure_text_role(text, size, role, false, bold, -1.0).x;
    let x = (container.x + (container.w - w) * 0.5).max(container.x);
    r.draw_text_role(text, Vec2::new(x, y), size, color, role, false, bold, -1.0);
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
            .join("../../../../target")
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
