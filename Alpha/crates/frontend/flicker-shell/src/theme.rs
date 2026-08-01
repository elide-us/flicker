//! The flicker-shell UI theme — the shared Prism chrome the front-end screens
//! draw with. It (1) registers the six Prism serif faces, (2) uploads the
//! shared 1×1 white pixel + the **Muse** main-menu character (with a baked
//! left-edge alpha fade), and (3) renders the Rust loading widget
//! ([`Theme::draw_loading`], flat vector chrome). Every colour it draws is a
//! **`theme.tokens`** lookup into the embedded `ui_elements.json` — the ONE
//! palette source the Lua screens read too, so the Rust-drawn loading screen can
//! never drift from the design language. `Theme::lua_textures` hands the
//! textures to a Lua screen by name.
//!
//! (The procedurally BAKED carved-stone panel / settings-panel / button sprite
//! textures — and their whole pixel-canvas bakery — died in S10: the menu /
//! pause / settings / confirm chrome is vector `panel` commands drawn by the
//! component walker, and no script referenced the baked sprites any more.)

use std::collections::HashMap;
use std::sync::LazyLock;

use flicker::render::{Renderer, TextureHandle, Vec2};

/// The shell's embedded UI-element layout — the SAME bytes `shell.rs` exposes to
/// Lua; parsed here once for the `theme.tokens` palette.
const UI_JSON: &str = include_str!("../../../../content/sensorium/resources/ui_elements.json");

/// The Prism palette (`theme.tokens`), parsed once from the embedded
/// `ui_elements.json`. Missing/malformed entries fall back to opaque magenta —
/// loudly visible, exactly like a bad token in the Lua path.
static TOKENS: LazyLock<HashMap<String, [f32; 4]>> = LazyLock::new(|| {
    let fallback = HashMap::new();
    let Ok(root) = serde_json::from_str::<serde_json::Value>(UI_JSON) else {
        tracing::error!("theme: embedded ui_elements.json did not parse — token palette empty");
        return fallback;
    };
    let Some(tokens) = root.get("theme").and_then(|t| t.get("tokens")).and_then(|t| t.as_object())
    else {
        tracing::error!("theme: embedded ui_elements.json has no theme.tokens");
        return fallback;
    };
    tokens
        .iter()
        .filter_map(|(name, v)| {
            let a = v.as_array()?;
            if a.len() < 4 {
                return None;
            }
            let mut c = [0.0f32; 4];
            for (i, ch) in c.iter_mut().enumerate() {
                *ch = a[i].as_f64()? as f32;
            }
            Some((name.clone(), c))
        })
        .collect()
});

/// One Prism token colour by name. An unknown name warns and returns opaque
/// magenta (visible + greppable — the strings-gate philosophy for colours).
fn tok(name: &str) -> [f32; 4] {
    match TOKENS.get(name) {
        Some(c) => *c,
        None => {
            tracing::warn!("theme: unknown token '{name}' — rendering magenta");
            [1.0, 0.0, 1.0, 1.0]
        }
    }
}

/// A token colour with its alpha overridden (e.g. the bronze bar rim at 0.95).
fn tok_a(name: &str, alpha: f32) -> [f32; 4] {
    let mut c = tok(name);
    c[3] = alpha;
    c
}

/// The Muse — the main-menu character, embedded so every shell app inherits her.
const MUSE_IMAGE: &[u8] = include_bytes!("../../../../content/sensorium/assets/muse.png");

// ===== loading-panel geometry (the one Rust-drawn widget) =====

const PANEL_W: u32 = 520;
// Tall enough for a title band + a bar with even margins; drawn as vector
// panels, so the size is pure layout (no baked frame art to distort).
const PANEL_H: u32 = 420;
/// Frame inset (px) the title/bar layout is derived from.
const FRAME: u32 = 38;

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

/// A geometry rect (px) for the loading layout.
#[derive(Copy, Clone)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Uploaded UI textures — just handles, so `Theme` is `Copy` and cheap to
/// share between scenes (the textures are uploaded once and never freed).
#[derive(Copy, Clone)]
pub struct Theme {
    white: TextureHandle,
    /// The main-menu character (left-edge alpha fade baked in). Exposed to Lua as
    /// `Textures.muse`; only the menu screen draws it.
    muse: TextureHandle,
}

impl Theme {
    /// Register the Prism UI faces + upload the shared textures: the 1×1 white
    /// pixel (rect fills / scrims) and the Muse.
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
        let muse = load_muse(renderer);
        Self { white, muse }
    }

    /// The engine textures this theme exposes to a Lua screen, as
    /// `(name, handle)` in id order — the index is the id the script references
    /// via `Textures.<name>` (see [`ScriptHost::set_texture_ids`] and the
    /// consumer's `render_hud`). `white` is id 0 so it doubles as the rect fill.
    ///
    /// [`ScriptHost::set_texture_ids`]: flicker::script::ScriptHost::set_texture_ids
    pub fn lua_textures(&self) -> [(&'static str, TextureHandle); 2] {
        [("white", self.white), ("muse", self.muse)]
    }

    /// Draw the loading screen: opaque backdrop, the flat Prism panel titled
    /// "LOADING", and a bronze-rimmed progress bar filled to `progress`
    /// (0..=1) — every colour a `theme.tokens` lookup.
    pub fn draw_loading(&self, r: &mut Renderer, screen: Vec2, progress: f32) {
        // Centre the fixed-size panel on the screen.
        let (pw, ph) = (PANEL_W as f32, PANEL_H as f32);
        let frame = FRAME as f32;
        let panel = Rect {
            x: ((screen.x - pw) * 0.5).round(),
            y: ((screen.y - ph) * 0.5).round(),
            w: pw,
            h: ph,
        };
        let title_y = panel.y + frame + 22.0;
        let backdrop = tok("stone0");
        let shadow = tok("shadow");
        // Flat Prism chrome (vector): backdrop, soft drop shadow, then the panel —
        // drawn as ui-panels so they sort behind the sprite bar + text that follow.
        r.draw_ui_panel(Vec2::ZERO, screen, backdrop, backdrop, 0.0, 0.0, 0.0, [0.0; 4], 0.0);
        r.draw_ui_panel(
            Vec2::new(panel.x - 4.0, panel.y + 16.0),
            Vec2::new(panel.w + 8.0, panel.h + 8.0),
            shadow,
            shadow,
            0.0,
            17.0,
            0.0,
            [0.0; 4],
            44.0,
        );
        r.draw_ui_panel(
            Vec2::new(panel.x, panel.y),
            Vec2::new(panel.w, panel.h),
            tok("stone3"),
            tok("stone1"),
            1.0,
            5.0,
            1.0,
            tok("edge2"),
            0.0,
        );
        centered_text(r, "LOADING", panel, title_y, 34.0, tok("ink_bright"), true);

        let bar = Rect {
            x: panel.x + frame + 22.0,
            y: panel.y + panel.h * 0.52,
            w: panel.w - 2.0 * (frame + 22.0),
            h: 22.0,
        };
        // Recessed track with a bronze rim + sapphire fill, drawn as vector
        // panels so they stay colour-correct (sRGB) like the panel above.
        let track = tok("stone1");
        r.draw_ui_panel(
            Vec2::new(bar.x, bar.y),
            Vec2::new(bar.w, bar.h),
            track,
            track,
            0.0,
            4.0,
            1.0,
            tok_a("bronze", 0.95),
            0.0,
        );
        let fill = (bar.w * progress.clamp(0.0, 1.0)).round();
        if fill > 0.0 {
            let sap = tok("sap_base");
            r.draw_ui_panel(
                Vec2::new(bar.x, bar.y),
                Vec2::new(fill, bar.h),
                sap,
                sap,
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
    r.draw_text_role(text, Vec2::new(x, y), size, color, role, false, bold, -1.0, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loading widget's palette is the embedded `theme.tokens` — every name
    /// it looks up must resolve (an unknown name renders magenta at runtime;
    /// this makes it a build failure instead).
    #[test]
    fn every_loading_token_resolves() {
        for name in ["stone0", "stone1", "stone3", "edge2", "sap_base", "ink_bright", "bronze", "shadow"] {
            assert!(TOKENS.contains_key(name), "theme.tokens is missing '{name}'");
        }
        // And the bronze rim keeps its authored alpha override.
        assert_eq!(tok_a("bronze", 0.95)[3], 0.95);
        assert_eq!(tok("stone0"), [0.031, 0.035, 0.047, 1.0], "tokens parse to their rgba");
    }
}
