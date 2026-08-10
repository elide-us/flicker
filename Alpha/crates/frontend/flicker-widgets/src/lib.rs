//! flicker-widgets — the engine's **UI toolkit**: the Rust component walker
//! (layout / draw / hit-test over a Lua-declared [`UiNode`] tree), the
//! [`component`] tier that owns every control's draw + hit, the template tier,
//! Surfaces, intents, the stringtable, and the [`render_hud`] draw bridge.
//! (`flicker` re-exports this crate as `flicker::ui`.)
//!
//! The UI pattern: **Lua declares** (a screen's `tree()` returns component
//! instances), **data rides the Model** (binds), **styles live in
//! `ui_elements.json`** (one palette, `theme.tokens`), and **Rust owns
//! everything else** — [`run_ui`] lays the cached tree out, draws and hit-tests
//! each control in [`component`], and returns [`HudCommand`]s for [`render_hud`].
//! Per-control behaviour was briefly a `ui/<kind>.lua` module tier; it came back
//! to the engine over 2026-08 and that tier is deleted (2026-08-10).
//! This crate depends on both `flicker-script` (the Lua↔Rust seam) and
//! `flicker-render` (the draw calls), which is exactly why it is its own crate:
//! the boundary contract forbids `flicker-script` from ever touching a renderer
//! handle.
//!
//! A walker consumer wires the seam like this:
//! ```ignore
//! // setup (once):
//! let styles = flicker_widgets::load_styles("ui_elements.json"); // token-resolved
//! let host = ScriptHost::from_file(path)?;
//! flicker_widgets::load_ui_json(&host, "ui_elements.json");   // → the `UI` global
//! let tree = host.ui_tree()?.expect("tree()");                 // cached once
//! let intents = UiIntents::of(&tree);                          // S9 declaration
//! // each frame:
//! let frame = run_ui(&tree, &model, &styles, &input, &mut state);
//! flicker_widgets::render_hud(renderer, &frame.commands, white, &textures);
//! ```
//!
//! ([`WIDGETS_LUA`] / [`load_widgets`] are the legacy immediate-mode residue —
//! one flagged consumer left; see their docs.)

use std::path::Path;

use flicker_render::{Renderer, TextureHandle, Vec2};
use flicker_script::{HudCommand, ScriptHost, TextAlign, UiNode};

/// The one shared prop-reader surface (`config::text` / `num` / `flag`) the walker and
/// the template builders both read their inputs through — no duplicate reader impls.
mod config;

/// The Rust **component walker** — Lua declares a [`UiNode`] tree; Rust owns
/// everything after that: layout, each control's draw + hit, the retained draw
/// cache, generic hit plumbing, and results routing. See [`component`].
///
/// [`UiNode`]: flicker_script::UiNode
pub mod component;
pub use component::{run_ui, DragPayload, RttSlot, UiFrame, UiInput, UiState};

/// The **router adapter** — [`WalkerHandler`] makes the walker one layer of the
/// `flicker-input-router` event bus (`hud_hit` → consume-pointer, focus writes
/// through [`UiState`]). See [`walker`].
pub mod walker;
pub use walker::{focusables_of, walker_owned, WalkerHandler};

/// The **declarative intents** (S9) — a screen ROOT's `on_<signal>: "result"`
/// props collected as [`UiIntents`], consumed by the walker layer
/// ([`WalkerHandler::with_intents`]) and folded into results + the `sig_<name>`
/// Model mirror by the scene. See [`intents`].
pub mod intents;
pub use intents::UiIntents;

/// The **template tier** — data protos (the embedded `ui_templates.json`) plus
/// the three surviving structural Rust builders (`frame` / `card` /
/// `option_grid`), each composing components into a [`UiNode`] subtree, invoked
/// by name from per-scene arrangement DATA. See [`template`].
///
/// [`UiNode`]: flicker_script::UiNode
pub mod template;
pub use template::{
    builtin_templates, expand, BuildCtx, Slots, TemplateDef, TemplateFn, TemplateRegistry,
};

/// The **scene file** — `{ boot, behaviour, params, tree, exits }` as authored
/// JSON, one `<Name>.scene.json` per scene: the composition a human assembles,
/// the Rust behaviour that plays it, and the routing that leaves it — plus
/// [`SceneManifest`], which indexes the folder they live in so the folder listing
/// IS the roster. See [`scene_def`].
pub mod scene_def;
pub use scene_def::{
    scene_id_from_file_name, SceneDef, SceneExit, SceneManifest, SCENE_FILE_SUFFIX,
};

/// The floating in-world **chat panel** builder — a bare `UiNode` builder (not a
/// registered template) a scene rebuilds each frame with a live rect + log so the
/// window can move/resize. See [`chat_panel`](chat_panel::chat_panel).
pub mod chat_panel;
pub use chat_panel::{chat_panel, ChatLineKind, ChatLineView, ChatView, RosterEntry};

/// The **Screen declaration** (S8) — a scene declares its screen's surfaces as
/// data and drives their `visible_bind` keys through one [`Surfaces`] helper
/// instead of hand-rolled show/hide chains. See [`surfaces`].
pub mod surfaces;

/// The Workflow — an Orchestration with an ordinal: linear step surfaces +
/// fail-loud document gates over one `Surfaces` exclusive group (Aaron 2026-08-01).
pub mod workflow;

/// The spine's REVERSIBLE half — undo/redo over commands a bench can apply and
/// take back. Domain-free: the commands live with the data they mutate.
pub mod history;
pub use history::{Command, CommandHistory, DEFAULT_DEPTH};
pub use surfaces::{OrchestrationRule, Surface, SurfaceChange, SurfaceOp, Surfaces};
pub use workflow::{workflows_from_json, Step, Workflow, WorkflowDef};

pub mod strings;

/// The embedded LEGACY immediate-mode widget toolkit (slider / stepper /
/// dropdown / button), exposed to a script as the `Widgets` global by
/// [`load_widgets`]. S10 residue: its ONE remaining consumer is
/// `flicker-world`'s `world_ui.lua` control HUD (flagged as the last
/// immediate-mode control surface); every other screen is a declarative
/// component tree. Deleted together with that conversion.
pub const WIDGETS_LUA: &str = include_str!("widgets.lua");

/// The **structural** component kinds — the ones the walker itself lays out and
/// draws. Every other legal kind is an interactive Component, owned by the engine
/// ([`RUST_COMPONENT_KINDS`]); `option` is neither — it is pure data an option strip
/// reads out of its own children. (`list` is a Component — the engine owns its draw
/// + hit — even though its column LAYOUT and viewport clip remain walker primitives.)
///
/// Together these are the complete vocabulary. A kind outside it is a typo: the walker
/// would anchor-overlay its children and draw nothing, silently, so
/// [`unknown_kinds`] exists to make that a test failure instead of a blank panel.
const STRUCTURAL_KINDS: &[&str] =
    &["screen", "cell", "row", "stack", "grid", "rtt", "text", "option"];

/// The **Rust component** kinds — interactive Components whose draw/hit/bind logic
/// lives in the ENGINE (`component.rs`), which is where a Component's logic belongs:
/// Aaron's ratified taxonomy 9C141E1C says *"the walker's per-control draw + hit +
/// bind code IS that Component's logic"*, and the 2026-08-09 ruling BF0AF0C9 restored
/// that after the 2026-07-30 inversion moved them into `ui/<kind>.lua`.
///
/// These are NOT [`STRUCTURAL_KINDS`]: a `button` owns semantics, a `row` does not.
/// The taxonomy's Primitive/Component line is real and this list keeps it visible.
///
/// The restoration is COMPLETE: the Lua component tier and its module list are gone
/// (2026-08-10), so this list is now the WHOLE interactive vocabulary rather than one
/// half of a migration meter. A new control is a new arm in `component.rs` and a new
/// entry here — there is no other tier to put one in.
const RUST_COMPONENT_KINDS: &[&str] = &[
    "button", "panel", "sprite", "rune_corners", "tooltip", "checkbox", "toggle", "radio", "tile",
    "pill_toggle", "tabs", "select", "slider", "stepper", "text_field", "list", "context_menu",
    "gauge", "resource_gauge", "stat_dot", "action_slot", "medallion", "badge",
];

/// Whether `kind` is an interactive Component — i.e. one the engine draws and hit-tests
/// rather than merely laying out.
///
/// The walker asks this at each decision the old Lua library used to gate: the draw
/// cache's `hot_matters` and the hit dispatch.
pub(crate) fn is_rust_component(kind: &str) -> bool {
    RUST_COMPONENT_KINDS.contains(&kind)
}

/// Every engine-tier component kind — the roster, for the gate that holds each control
/// to its obligations (a legal kind, and a hit answered in Rust). Test-only: the walker
/// itself asks [`is_rust_component`] about ONE kind at a time and never needs the list.
#[cfg(test)]
pub(crate) fn rust_component_kinds() -> &'static [&'static str] {
    RUST_COMPONENT_KINDS
}

/// Every component kind a tree may legally name — [`STRUCTURAL_KINDS`] plus the
/// [`RUST_COMPONENT_KINDS`]. That union is the complete vocabulary now that the Lua
/// component tier is gone; anything else is a typo, caught by [`unknown_kinds`].
///
/// `core` used to need an explicit exclusion here: `ui.core` — the shared emitter
/// LIBRARY, never a component — sat in the module list this function also consulted,
/// so a stray `core` node passed the gate and then drew nothing. With that list
/// deleted the exclusion is structural: `core` is in neither roster, so it is simply
/// unknown.
pub fn is_known_kind(kind: &str) -> bool {
    STRUCTURAL_KINDS.contains(&kind) || RUST_COMPONENT_KINDS.contains(&kind)
}

/// Every kind in `tree` the engine does not know, deduped — empty for a well-formed
/// tree. The drift gate for the authored vocabulary: a screen's test walks its real
/// tree through this, so a typo or a stale name fails the build rather than rendering
/// an invisible hole (and a `template` node that never expanded is caught too).
pub fn unknown_kinds(tree: &UiNode) -> Vec<String> {
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        if n.template.is_some() {
            out.push(format!("template:{}", n.template.as_deref().unwrap_or("?")));
        } else if !is_known_kind(&n.component) && !out.iter().any(|k| k == &n.component) {
            out.push(n.component.clone());
        }
        for c in &n.children {
            walk(c, out);
        }
        for group in n.slots.values() {
            for c in group {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// Every raw (non-`$token`) DISPLAY-string literal in `tree`, deduped in walk order —
/// the strings analog of the [`unknown_kinds`] vocabulary gate (S10 audit). A shipped
/// screen's test asserts this is EMPTY, so a hardcoded English string fails the build
/// instead of shipping unlocalisable text; new copy goes into
/// `Alpha/content/data/stringtable.json` as a `$token`.
///
/// Walks the SAME prop vocabulary the draw boundary resolves
/// (`DISPLAY_STR_PROPS`: label / text / title / subtitle / footer / placeholder /
/// hint / name / meta / prefix), over children and template slots. EXEMPT — not
/// display copy that needs a token:
///   * `$token` values (already stringtable refs; `$$…` escapes count too) and
///     empty strings;
///   * a prop whose node also carries its `<prop>_bind` twin (the literal is dead —
///     the bound Model string is what renders);
///   * single glyphs (`✕`, `×`, `‹`, arrows — one grapheme-ish char) and literals
///     with no alphabetic character at all (separators like `·`, `— / —`);
///   * pure `%`-format strings (`"%d"`, `"%.2f"`) — value formatting, not copy.
pub fn raw_display_literals(tree: &UiNode) -> Vec<String> {
    fn is_pure_format(s: &str) -> bool {
        // One-or-more `%<flags><width>[.prec]<conv>` units and nothing else.
        let mut chars = s.chars().peekable();
        let mut any = false;
        while let Some(c) = chars.next() {
            if c != '%' {
                return false;
            }
            while matches!(chars.peek(), Some('-' | '+' | ' ' | '#' | '0'..='9' | '.')) {
                chars.next();
            }
            match chars.next() {
                Some(conv) if conv.is_ascii_alphabetic() => any = true,
                Some('%') => {} // `%%` — the literal-percent escape
                _ => return false,
            }
        }
        any
    }
    fn exempt(node: &UiNode, key: &str, s: &str) -> bool {
        s.is_empty()
            || s.starts_with('$')
            || node.props.contains_key(&format!("{key}_bind"))
            || s.chars().count() == 1
            || !s.chars().any(char::is_alphabetic)
            || is_pure_format(s)
    }
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        for key in component::DISPLAY_STR_PROPS {
            if let Some(flicker_script::Value::Text(s)) = n.props.get(key) {
                if !exempt(n, key, s) && !out.iter().any(|have| have == s) {
                    out.push(s.clone());
                }
            }
        }
        for c in &n.children {
            walk(c, out);
        }
        for group in n.slots.values() {
            for c in group {
                walk(c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

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
                uv,
            } => {
                if let Some(&handle) = textures.get(*tex as usize) {
                    renderer.set_layer(base + layer);
                    renderer.draw_sprite_uv(
                        handle,
                        Vec2::new(*x, *y),
                        Vec2::new(*w, *h),
                        *color,
                        *uv,
                    );
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
                italic,
                bold,
                tracking,
                wrap,
            } => {
                renderer.set_layer(base + layer);
                let role = font_role(*font);
                let left = match align {
                    TextAlign::Center => {
                        x - renderer.measure_text_role(text, *size, role, *italic, *bold, *tracking).x
                            * 0.5
                    }
                    TextAlign::Right => {
                        x - renderer.measure_text_role(text, *size, role, *italic, *bold, *tracking).x
                    }
                    TextAlign::Left => *x,
                };
                renderer.draw_text_role(
                    text,
                    Vec2::new(left, *y),
                    *size,
                    *color,
                    role,
                    *italic,
                    *bold,
                    *tracking,
                    *wrap,
                );
            }
            HudCommand::TextCaret { x, y, w, h, prefix, size, color, layer, font, max_x } => {
                // The caret sits after the SHAPED width of the text before it — real
                // glyph measurement, here at the render bridge where the glyphs live,
                // never a char-count estimate (text ruling 2026-07-31).
                renderer.set_layer(base + layer);
                let role = font_role(*font);
                let cx = (x + renderer.measure_text_role(prefix, *size, role, false, false, -1.0).x)
                    .min(*max_x);
                renderer.draw_sprite(white, Vec2::new(cx, *y), Vec2::new(*w, *h), *color);
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
            HudCommand::Clip { rect } => match rect {
                Some(r) => renderer.set_clip(*r),
                None => renderer.clear_clip(),
            },
        }
    }
    renderer.set_layer(base);
    renderer.clear_clip();
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
        flicker_script::FontRole::Rune => flicker_render::FontRole::Rune,
    }
}

/// Expose the embedded [`WIDGETS_LUA`] toolkit to `script` as the `Widgets`
/// global — the LEGACY immediate-mode path (S10 residue; one flagged consumer:
/// `flicker-world`'s `world_ui.lua`). Best-effort; logs on failure (scripts
/// guard `if Widgets`). Deleted together with that last conversion.
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

/// Load `ui_elements.json` at `path`, expand its `$token` design-token
/// references, and return the resolved tree — the **styles** input for the Rust
/// component walker ([`run_ui`]), which resolves a node's dotted `style` path
/// against it (so colours stay single-sourced in `theme.tokens`, exactly like the
/// `UI` global [`load_ui_json`] hands Lua). Returns an empty object when the file
/// can't be read or parsed (the walker then falls back to its neutral defaults).
pub fn load_styles(path: impl AsRef<Path>) -> serde_json::Value {
    let path = path.as_ref();
    let mut ui = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(ui) => ui,
            Err(e) => {
                tracing::error!("ui_elements.json parse failed (styles): {e}");
                serde_json::Value::Object(Default::default())
            }
        },
        Err(e) => {
            tracing::error!("ui_elements.json read failed (styles) ({}): {e}", path.display());
            serde_json::Value::Object(Default::default())
        }
    };
    resolve_tokens(&mut ui);
    ui
}

/// Like [`load_styles`] but from an already-in-memory `ui_elements.json` string
/// (`include_str!`) — for a crate that embeds its layout rather than reading it
/// from disk (the front-end shell). Returns the token-resolved tree the component
/// walker resolves node `style` paths against. Empty object on a parse error.
pub fn load_styles_str(json: &str) -> serde_json::Value {
    let mut ui = match serde_json::from_str::<serde_json::Value>(json) {
        Ok(ui) => ui,
        Err(e) => {
            tracing::error!("ui_elements.json parse failed (styles str): {e}");
            serde_json::Value::Object(Default::default())
        }
    };
    resolve_tokens(&mut ui);
    ui
}

// (The dormant `load_arrangement_str` / `load_styles_merged` loaders died in S10
// — zero production callers. The surviving DATA entry points are
// `flicker_script::parse_ui_json` (the ONE arrangement reader) + the template
// registry (`builtin_templates` / `expand`) + the styles loaders above; a future
// CMS re-adds against those, not against a dormant convenience wrapper.)

/// Expand `"$name"` design-token references against the `theme.tokens` map, in
/// place, before the tree reaches Lua. A token (e.g. `"$sap_base"`) is replaced
/// by its literal value — an rgba array or a scalar — so every screen reads one
/// palette source (the Prism design language) and the per-file colour copies are
/// retired. This is the whole of the theme layer: colours live once in
/// `theme.tokens`; sections reference them by name.
///
/// Rules that keep it robust: tokens are **literal only** (no `$`-alias chains),
/// so one recursive pass is order-independent. A `$name` NOT in `theme.tokens`
/// is left as the string — since the S10 strings gate, display values in the
/// tree are STRINGTABLE refs (`$menu_title`, resolved at the draw boundary by
/// [`strings::resolve`]), so an unmatched name here is the normal case, not an
/// error; a genuinely missing token still fails visibly downstream (the
/// stringtable renders it RAW and warns once — the strings gate).
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
            serde_json::Value::String(s) if s.starts_with('$') => {
                if let Some(replacement) = tokens.get(&s[1..]) {
                    *value = replacement.clone();
                }
            }
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

    /// The canonical Prism palette (`theme.tokens` in `ui_elements.json`) as
    /// `(name, rgba)` — the one source of truth both fallback gates below check
    /// their copies against.
    fn theme_tokens() -> Vec<(String, [f64; 4])> {
        let elements: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../content/sensorium/resources/ui_elements.json"
        ))
        .expect("ui_elements.json parses");
        elements["theme"]["tokens"]
            .as_object()
            .expect("theme.tokens present")
            .iter()
            .map(|(k, v)| {
                let c: Vec<f64> = v.as_array().unwrap().iter().map(|n| n.as_f64().unwrap()).collect();
                (k.clone(), [c[0], c[1], c[2], c[3]])
            })
            .collect()
    }

    /// The name of the token `parts` matches exactly, else the nearest one — so a
    /// drifted fallback's failure message names what it probably meant.
    fn nearest_token(tokens: &[(String, [f64; 4])], parts: &[f64]) -> Option<String> {
        if tokens.iter().any(|(_, tv)| tv.iter().zip(parts).all(|(a, b)| (a - b).abs() < 1e-6)) {
            return None;
        }
        Some(
            tokens
                .iter()
                .min_by(|(_, a), (_, b)| {
                    let da: f64 = a.iter().zip(parts).map(|(x, y)| (x - y).abs()).sum();
                    let db: f64 = b.iter().zip(parts).map(|(x, y)| (x - y).abs()).sum();
                    da.partial_cmp(&db).unwrap()
                })
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| "?".to_string()),
        )
    }

    /// **Every neutral fallback in `component.rs` is a byte copy of a `theme.tokens`
    /// entry.** A control's missing-style floor (`const INK: [f32; 4] = […]`) is what
    /// it draws with when its style block omits a key, so a fallback that drifts from
    /// the palette is a control that looks subtly wrong only in the case nobody
    /// authored.
    ///
    /// This ran as a Lua-side gate until the container slice (2026-08-09) moved
    /// `STONE`/`SAP`/`INK`/`DIM`/`CLEAR` into the engine; it reads `component.rs`
    /// directly now, which is where every fallback lives.
    #[test]
    fn rust_fallback_consts_mirror_theme_tokens_exactly() {
        let tokens = theme_tokens();
        let mut checked = 0;
        let mut bad = Vec::new();
        for line in include_str!("component.rs").lines() {
            let Some(rest) = line.strip_prefix("const ") else { continue };
            let Some((name, value)) = rest.split_once(": [f32; 4] = [") else { continue };
            let Some((inner, _)) = value.split_once(']') else { continue };
            let parts: Vec<f64> =
                inner.split(',').filter_map(|p| p.trim().parse::<f64>().ok()).collect();
            if parts.len() != 4 {
                continue;
            }
            checked += 1;
            if let Some(near) = nearest_token(&tokens, &parts) {
                bad.push(format!("component.rs `{name}` = {parts:?} (nearest token: ${near})"));
            }
        }
        assert!(checked >= 9, "the gate must actually find the fallback consts ({checked})");
        assert!(bad.is_empty(), "engine fallback consts drifted from theme.tokens:\n{}", bad.join("\n"));
    }

    /// The NAMED half of the same law, and the replacement for the Lua fallback gate
    /// that died with the `ui/*.lua` component tier (2026-08-10). That gate walked
    /// `UI_COMPONENT_MODULES` checking each module's `local INK = {…}` against
    /// `theme.tokens`; with the list deleted it would have iterated NOTHING and passed
    /// — a gate certifying the very drift it exists to catch. This one carries the
    /// obligation over to the tier that now owns those colours.
    ///
    /// The gate above proves each fallback equals SOME token. That is not enough on a
    /// palette this dense: `$stone1` and `$stone2` are 0.02 apart, so a const could
    /// silently re-anchor to its neighbour and still pass. Here each const is pinned to
    /// the token it is NAMED for — `INK` is `$ink`, `STONE` is `$stone1` — so a retune
    /// that moves a token while its copy stands still fails the build instead of
    /// drifting the missing-style floor by one shade.
    ///
    /// The pairing is also a completeness ledger: a new `const X: [f32; 4]` must name
    /// its token here or be declared token-less, so no fallback escapes the discipline
    /// by being born after the gate.
    #[test]
    fn component_consts_mirror_their_named_theme_tokens() {
        // (the `const` in component.rs, the `theme.tokens` entry it copies)
        const PAIRS: &[(&str, &str)] = &[
            ("INK", "ink"),
            ("PANEL", "stone2"),
            ("RUNE", "rune_glow"),
            ("SAP", "sap_base"),
            ("CLEAR", "stage_void"),
            ("BRONZE", "bronze"),
            ("BRONZE_DIM", "bronze_dim"),
            ("FLASH_LIT", "rune_glow_hi"),
            ("DIM", "dim"),
            ("STONE", "stone1"),
            ("WELL", "well"),
            ("STONE_BTN", "stone_btn"),
            ("MARKER", "stam_hi"),
            ("SIG_BLUE", "sig_blue"),
        ];
        // Consts with no `$token` twin — the authored block carries the literal. Listed
        // so the completeness check below stays honest about them.
        const TOKENLESS: &[&str] = &["BAND"];

        let tokens = theme_tokens();
        // The top-level fallback consts as `component.rs` actually declares them (the
        // `#[cfg(test)]` locals inside functions are indented, so they are not picked up).
        let found: Vec<(String, Vec<f64>)> = include_str!("component.rs")
            .lines()
            .filter_map(|line| {
                let rest = line.strip_prefix("const ")?;
                let (name, value) = rest.split_once(": [f32; 4] = [")?;
                let (inner, _) = value.split_once(']')?;
                let parts: Vec<f64> =
                    inner.split(',').filter_map(|p| p.trim().parse::<f64>().ok()).collect();
                (parts.len() == 4).then(|| (name.to_string(), parts))
            })
            .collect();

        let mut bad = Vec::new();
        for (name, token) in PAIRS {
            let Some((_, parts)) = found.iter().find(|(n, _)| n == name) else {
                bad.push(format!("`{name}` is gone from component.rs — the pairing is stale"));
                continue;
            };
            let Some((_, want)) = tokens.iter().find(|(t, _)| t == token) else {
                bad.push(format!("`{name}` names `${token}`, which theme.tokens does not have"));
                continue;
            };
            if !want.iter().zip(parts).all(|(a, b)| (a - b).abs() < 1e-6) {
                bad.push(format!("`{name}` = {parts:?} but `${token}` = {want:?}"));
            }
        }
        assert!(bad.is_empty(), "engine fallback consts drifted from their tokens:\n{}", bad.join("\n"));

        let unpaired: Vec<&str> = found
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !PAIRS.iter().any(|(p, _)| p == n) && !TOKENLESS.contains(n))
            .collect();
        assert!(
            unpaired.is_empty(),
            "component.rs fallback consts with no named token: {unpaired:?} — pair each with \
             its `theme.tokens` entry above, or declare it TOKENLESS"
        );
    }

    /// The vocabulary gate has to be able to FAIL, or the screens it guards prove
    /// nothing. Also pins that `core` — the emitter library the deleted Lua tier
    /// exported, never a component — is still rejected, now because it is in neither
    /// roster rather than by a hardcoded `kind != "core"`.
    #[test]
    fn unknown_kinds_catches_a_typo_and_an_unexpanded_template() {
        let leaf = |kind: &str| UiNode { component: kind.to_string(), ..Default::default() };
        let mut screen = leaf("screen");
        screen.children = vec![leaf("cell"), leaf("button"), leaf("text")];
        assert!(unknown_kinds(&screen).is_empty(), "a well-formed tree is clean");

        screen.children.push(leaf("colunm")); // the typo a rename leaves behind
        assert_eq!(unknown_kinds(&screen), vec!["colunm".to_string()], "a stale kind is reported");

        let mut stale = leaf("screen");
        stale.children = vec![UiNode {
            component: String::new(),
            template: Some("window".into()),
            ..Default::default()
        }];
        assert_eq!(
            unknown_kinds(&stale),
            vec!["template:window".to_string()],
            "a template that never expanded is reported too"
        );

        assert!(!is_known_kind("core"), "`core` is the emitter library, never a component kind");
        for kind in rust_component_kinds() {
            assert!(is_known_kind(kind), "`{kind}` is an engine component but not a legal kind");
        }
    }

    /// The strings gate has to be able to FAIL — and its exemptions must hold, or
    /// every screen it guards would drown in false positives (glyphs, formats,
    /// bind-shadowed literals) or miss real copy.
    #[test]
    fn raw_display_literals_finds_copy_and_honours_exemptions() {
        let mut screen = UiNode { component: "screen".into(), ..Default::default() };
        let node = |props: &[(&str, &str)]| {
            let mut n = UiNode { component: "text".into(), ..Default::default() };
            for (k, v) in props {
                n.props.insert((*k).to_string(), flicker_script::Value::Text((*v).to_string()));
            }
            n
        };
        screen.children = vec![
            node(&[("text", "Hello World")]),            // raw copy → reported
            node(&[("label", "$menu_quit")]),            // token → exempt
            node(&[("text", "")]),                       // empty → exempt
            node(&[("label", "✕")]),                     // single glyph → exempt
            node(&[("text", "·")]),                      // no alphabetics → exempt
            node(&[("text", "%d")]),                     // pure format → exempt
            node(&[("text", "%.2f%%")]),                 // pure format chain → exempt
            node(&[("text", "dead"), ("text_bind", "live")]), // bind-shadowed → exempt
            node(&[("text", "Hello World")]),            // duplicate → deduped
        ];
        // A slot-authored literal is walked too.
        let mut holder = UiNode { component: "cell".into(), ..Default::default() };
        holder.slots.insert("items".into(), vec![node(&[("label", "Slot Copy")])]);
        screen.children.push(holder);

        assert_eq!(
            raw_display_literals(&screen),
            vec!["Hello World".to_string(), "Slot Copy".to_string()]
        );
    }

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
