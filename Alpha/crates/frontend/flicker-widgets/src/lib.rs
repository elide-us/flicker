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
/// is its centre. Each command's `layer` is applied **relative** to the
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
            } => {
                renderer.set_layer(base + layer);
                let left = match align {
                    TextAlign::Center => x - renderer.measure_text(text, *size).x * 0.5,
                    TextAlign::Right => x - renderer.measure_text(text, *size).x,
                    TextAlign::Left => *x,
                };
                renderer.draw_text(text, Vec2::new(left, *y), *size, *color);
            }
        }
    }
    renderer.set_layer(base);
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
        Ok(ui) => {
            if let Err(e) = script.set_global_json("UI", &ui) {
                tracing::error!("UI elements exposure failed: {e}");
            }
        }
        Err(e) => tracing::error!("ui_elements.json parse failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_script::ScriptHost;

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
