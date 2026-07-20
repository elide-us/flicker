//! flicker-widgets — the reusable **UI widget toolkit**: the Lua HUD render bridge
//! and the shared immediate-mode widget set. (`flicker-ui` re-exports this crate.)
//!
//! This is the engine-library half of the project's UI pattern (see
//! `docs/ui.md`): **Lua owns layout/interaction**, **state lives in
//! `ui_elements.json`**, and the **render surface lives here** — turning the
//! plain-data [`HudCommand`]s a script emits into [`Renderer`] draw calls, and
//! handing the script the embedded [`WIDGETS_LUA`] toolkit. It depends on both
//! `flicker-script` (the Lua↔Rust seam) and `flicker-render` (the draw calls),
//! which is exactly why it is its own crate: the boundary contract forbids
//! `flicker-script` from ever touching a renderer handle.
//!
//! A consumer wires it in three calls per the seam:
//! ```ignore
//! // setup (once):
//! flicker_ui::load_ui_json(&host, "ui_elements.json"); // → the `UI` global
//! flicker_ui::load_widgets(&host);                      // → the `Widgets` global
//! // each frame:
//! let results = host.update(input, w, h)?;             // read interaction back
//! host.set_model(&model)?;                              // publish engine values
//! let cmds = host.draw(w, h)?;
//! flicker_ui::render_hud(renderer, &cmds, white, &textures);
//! ```

use std::path::Path;

use flicker_render::{Renderer, TextureHandle, Vec2};
use flicker_script::{HudCommand, ScriptHost, TextAlign};

/// The embedded reusable Lua widget toolkit (slider / stepper / dropdown /
/// button). Exposed to a script as the `Widgets` global by [`load_widgets`].
pub const WIDGETS_LUA: &str = include_str!("widgets.lua");

/// Render a Lua HUD command list with the engine — the single draw path shared
/// by every Lua-driven screen. [`HudCommand::Rect`] uses the 1×1 `white`
/// texture tinted by its colour; [`HudCommand::Sprite`] looks its texture up in
/// `textures` by the id the script got from the `Textures` global;
/// [`HudCommand::Text`] with [`TextAlign::Center`] is measured and offset so `x`
/// is its centre; [`HudCommand::Panel`] draws a rounded-rect + 2-stop gradient +
/// border in one SDF call via `draw_ui_panel`. Each command's `layer` is applied **relative** to the
/// renderer's current base layer, so a script can stack its own sub-layers
/// (e.g. a dropdown over a panel) without knowing its scene depth.
pub fn render_hud(
    renderer: &mut Renderer,
    commands: &[HudCommand],
    white: TextureHandle,
    textures: &[TextureHandle],
) {
    let base = renderer.layer();
    for command in commands {
        match command {
            HudCommand::Rect {
                x,
                y,
                w,
                h,
                color,
                layer,
            } => {
                renderer.set_layer(base + layer);
                renderer.draw_sprite(white, Vec2::new(*x, *y), Vec2::new(*w, *h), *color);
            }
            HudCommand::Sprite {
                tex,
                x,
                y,
                w,
                h,
                color,
                layer,
            } => {
                if let Some(&handle) = textures.get(*tex as usize) {
                    renderer.set_layer(base + layer);
                    renderer.draw_sprite(handle, Vec2::new(*x, *y), Vec2::new(*w, *h), *color);
                }
            }
            HudCommand::Text {
                x,
                y,
                text,
                size,
                color,
                layer,
                align,
                font,
            } => {
                renderer.set_layer(base + layer);
                let role = font_role(*font);
                let left = match align {
                    TextAlign::Center => x - renderer.measure_text_role(text, *size, role).x * 0.5,
                    TextAlign::Right => x - renderer.measure_text_role(text, *size, role).x,
                    TextAlign::Left => *x,
                };
                renderer.draw_text_role(text, Vec2::new(left, *y), *size, *color, role);
            }
            HudCommand::Panel {
                x,
                y,
                w,
                h,
                color,
                color2,
                grad,
                radius,
                border,
                border_color,
                feather,
                layer,
            } => {
                renderer.set_layer(base + layer);
                renderer.draw_ui_panel(
                    Vec2::new(*x, *y),
                    Vec2::new(*w, *h),
                    *color,
                    *color2,
                    *grad,
                    *radius,
                    *border,
                    *border_color,
                    *feather,
                );
            }
        }
    }
    renderer.set_layer(base);
}

/// Bridge the script-side [`flicker_script::FontRole`] onto the renderer's
/// [`flicker_render::FontRole`]. This crate is the one seam that depends on both
/// (the boundary keeps `flicker-script` free of any renderer type), so the two
/// mirror-image enums are mapped here — exactly like the `HudCommand` draw.
fn font_role(role: flicker_script::FontRole) -> flicker_render::FontRole {
    match role {
        flicker_script::FontRole::Display => flicker_render::FontRole::Display,
        flicker_script::FontRole::Label => flicker_render::FontRole::Label,
        flicker_script::FontRole::Body => flicker_render::FontRole::Body,
    }
}

/// Expose the embedded [`WIDGETS_LUA`] toolkit to `script` as the `Widgets`
/// global, so its screens can use the shared immediate-mode widgets.
/// Best-effort; logs on failure (scripts guard `if Widgets`).
pub fn load_widgets(script: &ScriptHost) {
    if let Err(e) = script.set_lua_module("Widgets", WIDGETS_LUA, "widgets.lua") {
        tracing::error!("widgets module load failed: {e}");
    }
}

/// Parse the `ui_elements.json` at `path` and expose it to `script` as the `UI`
/// global, so a screen reads its layout from named elements (`UI.hud.controls`)
/// instead of hardcoded constants. Logs and continues on failure (scripts guard
/// `if not UI`). Calling again hot-reloads the layout after an edit.
pub fn load_ui_json(script: &ScriptHost, path: impl AsRef<Path>) {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => load_ui_json_str(script, &text),
        Err(e) => tracing::error!("ui_elements.json read failed ({}): {e}", path.display()),
    }
}

/// Expose an **already-in-memory** `ui_elements.json` string to `script` as the
/// `UI` global — the same contract as [`load_ui_json`], for layouts embedded in
/// a crate (`include_str!`) rather than read from disk. Logs and continues on a
/// parse error (scripts guard `if not UI`).
pub fn load_ui_json_str(script: &ScriptHost, json: &str) {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(mut ui) => {
            resolve_tokens(&mut ui);
            if let Err(e) = script.set_global_json("UI", &ui) {
                tracing::error!("UI elements exposure failed: {e}");
            }
        }
        Err(e) => tracing::error!("ui_elements.json parse failed: {e}"),
    }
}

/// Expand `"$name"` design-token references against the `theme.tokens` map, in
/// place, before the tree reaches Lua. A token (e.g. `"$sap_base"`) is replaced
/// by its literal value — an rgba array or a scalar — so every screen reads one
/// palette source (the Prism design language) and the per-file colour copies are
/// retired. This is the whole of the theme layer: colours live once in
/// `theme.tokens`; sections reference them by name.
///
/// Rules that keep it robust: tokens are **literal only** (no `$`-alias chains),
/// so one recursive pass is order-independent; an unknown `$name` is left as the
/// string and warned (the script-smoke frame then surfaces the missing colour
/// when Lua indexes `color[1]`); no non-token string in the tree begins with `$`.
fn resolve_tokens(root: &mut serde_json::Value) {
    let tokens = root
        .get("theme")
        .and_then(|theme| theme.get("tokens"))
        .and_then(|tokens| tokens.as_object())
        .cloned()
        .unwrap_or_default();
    if tokens.is_empty() {
        return;
    }
    fn walk(value: &mut serde_json::Value, tokens: &serde_json::Map<String, serde_json::Value>) {
        match value {
            serde_json::Value::String(s) if s.starts_with('$') => match tokens.get(&s[1..]) {
                Some(replacement) => *value = replacement.clone(),
                None => tracing::warn!("ui_elements.json: unknown token {s}"),
            },
            serde_json::Value::Array(items) => items.iter_mut().for_each(|v| walk(v, tokens)),
            serde_json::Value::Object(map) => map.values_mut().for_each(|v| walk(v, tokens)),
            _ => {}
        }
    }
    walk(root, &tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_script::ScriptHost;

    #[test]
    fn resolve_tokens_expands_refs_and_leaves_unknowns() {
        let mut ui = serde_json::json!({
            "theme": { "tokens": { "sap_base": [0.1, 0.2, 0.3, 1.0], "ink": [0.9, 0.9, 0.8, 1.0] } },
            "modal": { "title": { "color": "$ink" }, "buttons": { "fill": "$sap_base" } },
            "screens": { "menu": { "title": "START", "overlay": "$sap_base" } },
            "oops": "$missing"
        });
        resolve_tokens(&mut ui);
        assert_eq!(ui["modal"]["title"]["color"], serde_json::json!([0.9, 0.9, 0.8, 1.0]));
        assert_eq!(ui["modal"]["buttons"]["fill"], serde_json::json!([0.1, 0.2, 0.3, 1.0]));
        assert_eq!(ui["screens"]["menu"]["overlay"], serde_json::json!([0.1, 0.2, 0.3, 1.0]));
        // literal strings (labels) are untouched; an unknown token is left as-is.
        assert_eq!(ui["screens"]["menu"]["title"], serde_json::json!("START"));
        assert_eq!(ui["oops"], serde_json::json!("$missing"));
    }

    #[test]
    fn widgets_lua_parses_and_evaluates() {
        // set_lua_module loads + eval()s the toolkit; a syntax error would surface
        // here. Success proves widgets.lua parses and returns the `W` table — the
        // cargo-checkable guard for the runtime-loaded Lua.
        let main = "return { update = function() return {} end, draw = function() return {} end }";
        let host = ScriptHost::new(main, "test-main").expect("host builds");
        host.set_lua_module("Widgets", WIDGETS_LUA, "widgets.lua")
            .expect("widgets.lua parses and evaluates to a table");
    }
}
