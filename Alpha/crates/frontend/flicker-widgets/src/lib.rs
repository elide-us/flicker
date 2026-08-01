//! flicker-widgets — the engine's **UI toolkit**: the Rust component walker
//! (layout / draw / hit-test over a Lua-declared [`UiNode`] tree), the
//! `ui/<kind>.lua` component library, the template tier, Surfaces, intents,
//! the stringtable, and the [`render_hud`] draw bridge. (`flicker` re-exports
//! this crate as `flicker::ui`.)
//!
//! The UI pattern: **Lua declares** (a screen's `tree()` returns component
//! instances; per-control behaviour lives in `ui/<kind>.lua`), **data rides the
//! Model** (binds), **styles live in `ui_elements.json`** (one palette,
//! `theme.tokens`), and **Rust owns the walk** — [`run_ui_with`] lays the cached
//! tree out, dispatches component draw/hit through the embedded
//! [`UI_COMPONENT_MODULES`], and returns [`HudCommand`]s for [`render_hud`].
//! This crate depends on both `flicker-script` (the Lua↔Rust seam) and
//! `flicker-render` (the draw calls), which is exactly why it is its own crate:
//! the boundary contract forbids `flicker-script` from ever touching a renderer
//! handle.
//!
//! A walker consumer wires the seam like this:
//! ```ignore
//! // setup (once):
//! let styles = flicker_widgets::load_styles("ui_elements.json"); // token-resolved
//! let host = ScriptHost::from_file_with_modules(path, UI_COMPONENT_MODULES)?;
//! flicker_widgets::load_ui_json(&host, "ui_elements.json");   // → the `UI` global
//! let tree = host.ui_tree()?.expect("tree()");                 // cached once
//! let intents = UiIntents::of(&tree);                          // S9 declaration
//! // each frame:
//! let frame = run_ui_with(&tree, &model, &styles, &input, &mut state, Some(&host));
//! flicker_widgets::render_hud(renderer, &frame.commands, white, &textures);
//! ```
//!
//! ([`WIDGETS_LUA`] / [`load_widgets`] are the legacy immediate-mode residue —
//! one flagged consumer left; see their docs.)

use std::path::Path;

use flicker_render::{Renderer, TextureHandle, Vec2};
use flicker_script::{HudCommand, ScriptHost, TextAlign, UiNode};

/// The Rust **component walker** — the target UI path (Lua declares a [`UiNode`]
/// tree; Rust owns draw / layout / hit-test). Runs alongside the legacy
/// immediate [`render_hud`] path during the migration.
///
/// [`UiNode`]: flicker_script::UiNode
/// The one shared prop-reader surface (`config::text` / `num` / `flag`) the walker and
/// the template builders both read their inputs through — no duplicate reader impls.
mod config;

pub mod component;
pub use component::{run_ui, run_ui_with, DragPayload, RttSlot, UiFrame, UiInput, UiState};

/// The **router adapter** — [`WalkerHandler`] makes the walker one layer of the
/// `flicker-input-router` event bus (`hud_hit` → consume-pointer, focus writes
/// through [`UiState`]). See [`walker`].
pub mod walker;
pub use walker::{focusables_of, WalkerHandler};

/// The **declarative intents** (S9) — a screen ROOT's `on_<signal>: "result"`
/// props collected as [`UiIntents`], consumed by the walker layer
/// ([`WalkerHandler::with_intents`]) and folded into results + the `sig_<name>`
/// Model mirror by the scene. See [`intents`].
pub mod intents;
pub use intents::UiIntents;

/// The **template tier** — named Rust builders that compose walker pieces into a
/// [`UiNode`] subtree, invoked by name from per-scene arrangement DATA. See
/// [`template`].
///
/// [`UiNode`]: flicker_script::UiNode
pub mod template;
pub use template::{
    builtin_templates, expand, BuildCtx, Slots, TemplateDef, TemplateFn, TemplateRegistry,
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
pub use surfaces::{Surface, SurfaceChange, Surfaces};

pub mod strings;

/// The embedded LEGACY immediate-mode widget toolkit (slider / stepper /
/// dropdown / button), exposed to a script as the `Widgets` global by
/// [`load_widgets`]. S10 residue: its ONE remaining consumer is
/// `flicker-world`'s `world_ui.lua` control HUD (flagged as the last
/// immediate-mode control surface); every other screen is a declarative
/// component tree. Deleted together with that conversion.
pub const WIDGETS_LUA: &str = include_str!("widgets.lua");

/// The Lua UI **component library** modules — `(require-name, source)` pairs to pass to
/// `ScriptHost::new_with_modules` / `from_file_with_modules` / `library`, so a screen's
/// VM can `require("ui.<kind>")` and the walker can dispatch that kind's DRAW to it (see
/// [`run_ui_with`]). ONE canonical list: every consumer registers the SAME set — add a
/// control here when its `ui/<kind>.lua` lands and drops the Rust twin.
pub const UI_COMPONENT_MODULES: &[(&str, &str)] = &[
    ("ui.core", include_str!("../../../../content/sensorium/scripts/ui/core.lua")),
    ("ui.button", include_str!("../../../../content/sensorium/scripts/ui/button.lua")),
    ("ui.checkbox", include_str!("../../../../content/sensorium/scripts/ui/checkbox.lua")),
    ("ui.toggle", include_str!("../../../../content/sensorium/scripts/ui/toggle.lua")),
    ("ui.radio", include_str!("../../../../content/sensorium/scripts/ui/radio.lua")),
    ("ui.tile", include_str!("../../../../content/sensorium/scripts/ui/tile.lua")),
    ("ui.pill_toggle", include_str!("../../../../content/sensorium/scripts/ui/pill_toggle.lua")),
    ("ui.tabs", include_str!("../../../../content/sensorium/scripts/ui/tabs.lua")),
    ("ui.select", include_str!("../../../../content/sensorium/scripts/ui/select.lua")),
    ("ui.context_menu", include_str!("../../../../content/sensorium/scripts/ui/context_menu.lua")),
    ("ui.slider", include_str!("../../../../content/sensorium/scripts/ui/slider.lua")),
    ("ui.stepper", include_str!("../../../../content/sensorium/scripts/ui/stepper.lua")),
    ("ui.list", include_str!("../../../../content/sensorium/scripts/ui/list.lua")),
    ("ui.text_field", include_str!("../../../../content/sensorium/scripts/ui/text_field.lua")),
    ("ui.sprite", include_str!("../../../../content/sensorium/scripts/ui/sprite.lua")),
    ("ui.gauge", include_str!("../../../../content/sensorium/scripts/ui/gauge.lua")),
    ("ui.badge", include_str!("../../../../content/sensorium/scripts/ui/badge.lua")),
    ("ui.tooltip", include_str!("../../../../content/sensorium/scripts/ui/tooltip.lua")),
    ("ui.rune_corners", include_str!("../../../../content/sensorium/scripts/ui/rune_corners.lua")),
];

/// The **structural** component kinds — the ones the walker itself lays out and
/// draws. Every other legal kind is an interactive Component, and its name is the
/// `ui.<kind>` module that owns it in [`UI_COMPONENT_MODULES`]; `option` is neither —
/// it is pure data a segmented control reads out of its own children. (`list` is a
/// Component — `ui/list.lua` owns its draw + hit — even though its column LAYOUT
/// and viewport clip remain walker primitives.)
///
/// Together these are the complete vocabulary. A kind outside it is a typo: the walker
/// would anchor-overlay its children and draw nothing, silently, so
/// [`unknown_kinds`] exists to make that a test failure instead of a blank panel.
const STRUCTURAL_KINDS: &[&str] =
    &["screen", "cell", "row", "stack", "grid", "rtt", "text", "option"];

/// Every component kind a tree may legally name — [`STRUCTURAL_KINDS`] plus one per
/// Lua component module.
pub fn is_known_kind(kind: &str) -> bool {
    STRUCTURAL_KINDS.contains(&kind)
        || UI_COMPONENT_MODULES
            .iter()
            .any(|(name, _)| name.strip_prefix("ui.") == Some(kind))
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

    /// The vocabulary gate has to be able to FAIL, or the screens it guards prove
    /// nothing. Also pins that every Lua component module is a legal kind — the two
    /// lists are maintained separately and would otherwise drift apart silently.
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

        for (name, _) in UI_COMPONENT_MODULES {
            if let Some(kind) = name.strip_prefix("ui.") {
                // `core` is the primitive emitter set, not a component.
                if kind != "core" {
                    assert!(is_known_kind(kind), "`{kind}` has a Lua module but is not a legal kind");
                }
            }
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
