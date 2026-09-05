//! flicker-widgets — the engine's **UI toolkit**: the Rust component walker
//! (layout / draw / hit-test over a Lua-declared [`UiNode`] tree), the
//! [`component`] tier that owns every control's draw + hit,
//! Sections, intents, the stringtable, and the [`render_hud`] draw bridge.
//! (`flicker` re-exports this crate as `flicker::ui`.)
//!
//! The UI pattern: **Lua declares** (a screen's `tree()` returns component
//! instances), **data rides the Model** (binds), **styles live in
//! `ui_theme.json`** (one palette, `theme.tokens`), and **Rust owns
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
//! let styles = flicker_widgets::load_styles("ui_theme.json"); // token-resolved
//! let host = ScriptHost::from_file(path)?;
//! flicker_widgets::load_ui_json(&host, "ui_theme.json");   // → the `UI` global
//! let tree = host.ui_tree()?.expect("tree()");                 // cached once
//! let intents = UiIntents::of(&tree);                          // S9 declaration
//! // each frame:
//! let frame = run_ui(&tree, &model, &styles, &input, &mut state);
//! flicker_widgets::render_hud(renderer, &frame.commands, white, &textures);
//! ```

use std::path::{Path, PathBuf};

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
pub use component::{
    popup_dismissable, run_ui, DragPayload, SurfacePointer, SurfaceSlot, UiFrame, UiInput, UiState,
};

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
pub mod chat_modal;
pub mod chat_panel;
pub use chat_modal::{ChatFrame, ChatModal};
pub use chat_panel::{chat_panel, ChatLineKind, ChatLineView, ChatView, RosterEntry};

/// The **2D readout filler** — a bounded series drawn into a `surface` node's
/// reserved rect as a sparkline, a histogram or a filled curve, over
/// [`HudCommand::Line`]. The [`ChatModal`] shape: a component STRUCT the scene
/// hosts, seated on a rect the walker reserved. See [`plot`].
pub mod plot;
pub use plot::{Plot, PlotKind, PlotSeries, PlotStyle};

/// The **Screen declaration** (S8) — a scene declares its screen's SECTIONS (the
/// `visible_bind`-gated subtrees: settings sections, dialogs, inspector panes) as
/// data and drives their keys through one [`Sections`] helper instead of hand-rolled
/// show/hide chains. See [`sections`]. (Named `Surfaces` until 2026-08-21, when
/// `surface` became the drawing-surface KIND — one word, one meaning.)
pub mod sections;

/// The spine's REVERSIBLE half — undo/redo over commands a bench can apply and
/// take back. Domain-free: the commands live with the data they mutate.
pub mod history;
pub use history::{Command, CommandHistory, DEFAULT_DEPTH};
pub use sections::{Section, SectionChange, Sections};

pub mod strings;

/// The **one stage compiler** — `stages.<source>` JSON → the typed
/// [`StageDef`](flicker_render::StageDef) every surface filler consumes, with every
/// authoring problem reported as data (and gated on the shipped content). See [`stages`].
pub mod stages;
/// Data-driven rows: a `list` with `rows_from` expands its ONE prototype child into a
/// clone per row the scene publishes — see [`rows`].
pub mod rows;
pub use rows::{instantiate_rows, Row};
pub use stages::{
    compile_rate, compile_stage, is_source_key, lighting_preset, stage_def, stage_defs,
};

/// The **structural** component kinds — the ones the walker itself lays out and
/// draws. Every other legal kind is an interactive Component, owned by the engine
/// ([`RUST_COMPONENT_KINDS`]); `option` is neither — it is pure data an option strip
/// reads out of its own children. (`list` is a Component — the engine owns its draw
/// + hit — even though its column LAYOUT and viewport clip remain walker primitives.)
///
/// Together these are the complete vocabulary. A kind outside it is a typo: the walker
/// would anchor-overlay its children and draw nothing, silently, so
/// [`unknown_kinds`] exists to make that a test failure instead of a blank panel.
const STRUCTURAL_KINDS: &[&str] = &["surface", "cell", "row", "stack", "grid", "text", "option"];

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
    "button",
    "panel",
    "sprite",
    "tooltip",
    "checkbox",
    "toggle",
    "radio",
    "tile",
    "pill_toggle",
    "tabs",
    "select",
    "slider",
    "stepper",
    "text_field",
    "list",
    "context_menu",
    "gauge",
    "resource_gauge",
    "stat_dot",
    "action_slot",
    "medallion",
    "badge",
    // Composites the engine draws at walk time — the carved modal slab and the two-rail
    // page/tab control (PTT). Formalised from the retired `popup_panel` / `paged_menu`
    // template builders (201F4F51 P1): now first-class kinds the scene names via
    // `component:` and the walker lays out / draws / hit-tests, never a template pass.
    "popup_panel",
    "paged_menu",
    // The bench-standard footer band: a left LEGEND of controller-glyph + help-label
    // pairs and the scene's authored button cluster right-aligned (`[ MENU ]` always,
    // `[ BACK ]`/`[ NEXT ]` as relevant). Stateless — its buttons fire the same result
    // names the screen's declared Next/Prev/Menu intents fire, one activation channel.
    "nav_footer",
    // The tooltip BINDING ICON (Aaron 2026-09-02): give it an ActionSignal and it shows
    // the CURRENT DEVICE's face of that signal's binding — the atlas glyph on a pad, the
    // house keycap (bold black on a solid cap) on kbm — with an optional help label
    // (`{Signal}` placeholders). Read-only chrome: never focusable, never a target.
    "binding_icon",
];

/// Whether `kind` is an interactive Component — i.e. one the engine draws and hit-tests
/// rather than merely laying out.
///
/// The walker asks this at each decision the old Lua library used to gate: the draw
/// cache's `hot_matters` and the hit dispatch.
pub(crate) fn is_rust_component(kind: &str) -> bool {
    RUST_COMPONENT_KINDS.contains(&kind)
}

/// Every engine-tier component kind — the roster, THE single source of truth for
/// what the engine draws. The walker itself asks [`is_rust_component`] about ONE
/// kind at a time and never needs the list; the consumers are the gates: this
/// crate's roster test, and the Component Catalog's coverage gate, which derives
/// its required card set from HERE so the catalog can never silently lag the
/// engine (a new kind fails the catalog build until its demo card is authored).
pub fn rust_component_kinds() -> &'static [&'static str] {
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
/// an invisible hole.
pub fn unknown_kinds(tree: &UiNode) -> Vec<String> {
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        if !is_known_kind(&n.component) && !out.iter().any(|k| k == &n.component) {
            out.push(n.component.clone());
        }
        for c in &n.children {
            walk(c, out);
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
/// hint / name / meta / prefix), over children. EXEMPT — not
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
                        x - renderer
                            .measure_text_role(text, *size, role, *italic, *bold, *tracking)
                            .x
                            * 0.5
                    }
                    TextAlign::Right => {
                        x - renderer
                            .measure_text_role(text, *size, role, *italic, *bold, *tracking)
                            .x
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
            HudCommand::TextCaret {
                x,
                y,
                w,
                h,
                prefix,
                size,
                color,
                layer,
                font,
                max_x,
            } => {
                // The caret sits after the SHAPED width of the text before it — real
                // glyph measurement, here at the render bridge where the glyphs live,
                // never a char-count estimate (text ruling 2026-07-31).
                renderer.set_layer(base + layer);
                let role = font_role(*font);
                let cx = (x + renderer
                    .measure_text_role(prefix, *size, role, false, false, -1.0)
                    .x)
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
            HudCommand::Line {
                from,
                to,
                width,
                color,
                layer,
            } => {
                // One ROTATED quad in the SAME sprite batch a `Rect` uses (the 1×1
                // white tinted by `color`) — no line pipeline, and the current clip
                // and layer apply exactly as they do to every other 2D draw.
                renderer.set_layer(base + layer);
                renderer.draw_line(
                    white,
                    Vec2::new(from[0], from[1]),
                    Vec2::new(to[0], to[1]),
                    *width,
                    *color,
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

/// Parse the `ui_theme.json` at `path` and expose it to `script` as the `UI`
/// global, so a screen reads its layout from named elements (`UI.hud.controls`)
/// instead of hardcoded constants. Logs and continues on failure (scripts guard
/// `if not UI`). Calling again hot-reloads the layout after an edit.
pub fn load_ui_json(script: &ScriptHost, path: impl AsRef<Path>) {
    load_ui_json_for(script, path, None);
}

/// [`load_ui_json`] plus the SCENE's own style blocks (five-line split): the full
/// on-disk pipeline — satellites merged, scene blocks over them, `$token`s
/// resolved against the one palette — handed to Lua as one `UI` global, so a
/// dormant bench's `UI.<bench>.*` reads survive its bucket leaving the theme file.
pub fn load_ui_json_for(
    script: &ScriptHost,
    path: impl AsRef<Path>,
    scene: Option<&serde_json::Value>,
) {
    let ui = load_styles_for(path, scene);
    if let Err(e) = script.set_global_json("UI", &ui) {
        tracing::error!("UI elements exposure failed: {e}");
    }
}

/// Expose an **already-in-memory** `ui_theme.json` string to `script` as the
/// `UI` global — the same contract as [`load_ui_json`], for layouts embedded in
/// a crate (`include_str!`) rather than read from disk. Logs and continues on a
/// parse error (scripts guard `if not UI`).
pub fn load_ui_json_str(script: &ScriptHost, json: &str) {
    load_ui_json_strs(script, &[json]);
}

/// [`load_ui_json_str`] over the embedded theme TRIO — root + satellites merged
/// by [`load_styles_strs`]'s rules, then handed to Lua as one `UI` global, so a
/// script's `UI.settings.*` / `UI.menu.*` reads survive the file split unchanged.
pub fn load_ui_json_strs(script: &ScriptHost, parts: &[&str]) {
    load_ui_json_strs_for(script, parts, None);
}

/// [`load_ui_json_strs`] plus the SCENE's own style blocks — the embedded Lua
/// exposure for a scene pair: the scene file's `styles` land in the `UI` global
/// beside the shared defaults, tokens resolved against the one palette.
pub fn load_ui_json_strs_for(
    script: &ScriptHost,
    parts: &[&str],
    scene: Option<&serde_json::Value>,
) {
    let ui = load_styles_strs_for(parts, scene);
    if let Err(e) = script.set_global_json("UI", &ui) {
        tracing::error!("UI elements exposure failed: {e}");
    }
}

/// The one shared `ui_theme.json` path, resolved through the content-roots
/// service — `<content_root>/sensorium/resources/ui_theme.json`. Every scene
/// crate used to spell its own `CARGO_MANIFEST_DIR` climb to this file, which
/// baked the repo layout into ~11 call sites and broke the moment the app was
/// installed anywhere; the roots service is the one knob that relocates them all.
pub fn shared_theme_path() -> PathBuf {
    flicker_core::roots::roots()
        .sensorium()
        .join("resources/ui_theme.json")
}

/// [`load_styles_for`] over [`shared_theme_path`] — the styles input for a scene
/// crate's Rust walker, satellites merged, scene blocks over them, tokens
/// resolved. Call this instead of spelling a path to the theme file.
pub fn load_shared_styles(scene: Option<&serde_json::Value>) -> serde_json::Value {
    load_styles_for(shared_theme_path(), scene)
}

/// [`load_ui_json_for`] over [`shared_theme_path`] — the `UI` global exposure
/// for a scene pair's Lua, from the one shared theme location.
pub fn load_shared_ui_json(script: &ScriptHost, scene: Option<&serde_json::Value>) {
    load_ui_json_for(script, shared_theme_path(), scene);
}

/// The theme's SATELLITE files, merged into the loaded root by [`load_styles`]:
/// the RTT stage sources (`ui_stages.json`) and the shell chrome (`ui_style.json`)
/// — split out of the one big file by Aaron 2026-08-12. The PALETTE never leaves
/// `ui_theme.json`: a satellite carrying a `theme` key is a palette fork
/// (rule 8D8A4215) and is refused loudly.
const THEME_SATELLITES: &[&str] = &["ui_stages.json", "ui_style.json"];

/// Merge one satellite file's top-level entries into the theme root. The theme
/// file WINS a key collision (loud error — a satellite must own its keys), and a
/// `theme` key is refused outright (the palette-fork guard).
fn merge_satellite(root: &mut serde_json::Value, sat: serde_json::Value, name: &str) {
    let (Some(obj), serde_json::Value::Object(sat)) = (root.as_object_mut(), sat) else {
        tracing::error!("{name}: not a JSON object — ignored");
        return;
    };
    for (k, v) in sat {
        if k == "theme" {
            tracing::error!(
                "{name} carries a `theme` key — the palette lives ONLY in ui_theme.json \
                 (one-palette law); refusing the fork"
            );
            continue;
        }
        if obj.contains_key(&k) {
            tracing::error!("{name}: key `{k}` collides with ui_theme.json — the theme file wins");
            continue;
        }
        obj.insert(k, v);
    }
}

/// Load `ui_theme.json` at `path`, merge its sibling SATELLITE files
/// ([`THEME_SATELLITES`] in the same folder, when present), expand `$token`
/// design-token references, and return the resolved tree — the **styles** input
/// for the Rust component walker ([`run_ui`]), which resolves a node's dotted
/// `style` path against it (so colours stay single-sourced in `theme.tokens`,
/// exactly like the `UI` global [`load_ui_json`] hands Lua). One merged root
/// means a `stage` node's source and a chrome style resolve exactly as they did
/// when everything lived in one file — call sites carry only the theme path.
/// Returns an empty object when the theme file can't be read or parsed (the
/// walker then falls back to its neutral defaults).
pub fn load_styles(path: impl AsRef<Path>) -> serde_json::Value {
    load_styles_for(path, None)
}

/// [`load_styles`] plus the SCENE's own style blocks ([`SceneDef::styles`] — the
/// five-line split: a scene's values live in its scene file). Merged AFTER the
/// satellites and BEFORE token resolution, so a scene's `$token` refs resolve
/// against the one palette exactly like the shared defaults. A scene may
/// deliberately override a shared bucket for itself — its own file wins — but a
/// `theme` key is refused (the palette-fork guard, also enforced at parse).
pub fn load_styles_for(
    path: impl AsRef<Path>,
    scene: Option<&serde_json::Value>,
) -> serde_json::Value {
    let path = path.as_ref();
    let mut ui = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(ui) => ui,
            Err(e) => {
                tracing::error!("ui_theme.json parse failed (styles): {e}");
                serde_json::Value::Object(Default::default())
            }
        },
        Err(e) => {
            tracing::error!(
                "ui_theme.json read failed (styles) ({}): {e}",
                path.display()
            );
            serde_json::Value::Object(Default::default())
        }
    };
    if let Some(dir) = path.parent() {
        for name in THEME_SATELLITES {
            let sp = dir.join(name);
            let Ok(text) = std::fs::read_to_string(&sp) else {
                continue;
            };
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(sat) => merge_satellite(&mut ui, sat, name),
                Err(e) => tracing::error!("{name} parse failed ({}): {e}", sp.display()),
            }
        }
    }
    if let (Some(obj), Some(serde_json::Value::Object(scene))) = (ui.as_object_mut(), scene) {
        merge_scene_blocks(obj, scene);
    }
    resolve_tokens(&mut ui);
    ui
}

/// Merge a scene file's own blocks ([`SceneDef::styles`]) over the shared root. A
/// style block is the scene's to override wholesale (its own file wins); `theme` is
/// refused (the palette-fork guard); and `stages` merges INTO the shared stage block —
/// a scene's stages land beside the library's `lighting` presets, and a scene stage
/// that collides with a library source is refused loudly (the library wins), because
/// two definitions of one name is exactly the drift the one compiler exists to end.
fn merge_scene_blocks(
    root: &mut serde_json::Map<String, serde_json::Value>,
    scene: &serde_json::Map<String, serde_json::Value>,
) {
    for (k, v) in scene {
        match k.as_str() {
            "theme" => tracing::error!(
                "scene styles carry a `theme` key — the palette lives ONLY in \
                 ui_theme.json (one-palette law); refusing the fork"
            ),
            "stages" => {
                let Some(scene_stages) = v.as_object() else {
                    tracing::error!("scene `stages` is not an object — ignored");
                    continue;
                };
                let shared = root
                    .entry("stages")
                    .or_insert_with(|| serde_json::Value::Object(Default::default()));
                let Some(shared) = shared.as_object_mut() else {
                    tracing::error!(
                        "the shared `stages` block is not an object — scene stages dropped"
                    );
                    continue;
                };
                for (name, stage) in scene_stages {
                    if shared.contains_key(name) {
                        tracing::error!(
                            "scene stage `{name}` collides with the shared stage library \
                             (ui_stages.json) — the library wins; rename the scene's stage"
                        );
                        continue;
                    }
                    shared.insert(name.clone(), stage.clone());
                }
            }
            _ => {
                root.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Like [`load_styles`] but from an already-in-memory `ui_theme.json` string
/// (`include_str!`) — for a crate that embeds its layout rather than reading it
/// from disk (the front-end shell). Returns the token-resolved tree the component
/// walker resolves node `style` paths against. Empty object on a parse error.
pub fn load_styles_str(json: &str) -> serde_json::Value {
    load_styles_strs(&[json])
}

/// The EMBEDDED counterpart of [`load_styles`]'s satellite merge: the first
/// string is the theme root (it must carry `theme.tokens`), each further string
/// is an embedded satellite (`ui_style.json`, `ui_stages.json`, …) merged under
/// the same rules — the root wins collisions, a satellite `theme` key is refused
/// (the palette-fork guard). One merged root, one token resolution, exactly like
/// the on-disk trio.
pub fn load_styles_strs(parts: &[&str]) -> serde_json::Value {
    load_styles_strs_for(parts, None)
}

/// [`load_styles_strs`] plus the SCENE's own style blocks (five-line split) — the
/// embedded counterpart of [`load_styles_for`].
pub fn load_styles_strs_for(
    parts: &[&str],
    scene: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut ui = match parts
        .first()
        .map(|p| serde_json::from_str::<serde_json::Value>(p))
    {
        Some(Ok(ui)) => ui,
        Some(Err(e)) => {
            tracing::error!("ui_theme.json parse failed (styles str): {e}");
            serde_json::Value::Object(Default::default())
        }
        None => serde_json::Value::Object(Default::default()),
    };
    for (i, part) in parts.iter().enumerate().skip(1) {
        match serde_json::from_str::<serde_json::Value>(part) {
            Ok(sat) => merge_satellite(&mut ui, sat, &format!("embedded satellite #{i}")),
            Err(e) => tracing::error!("embedded satellite #{i} parse failed: {e}"),
        }
    }
    if let (Some(obj), Some(serde_json::Value::Object(scene))) = (ui.as_object_mut(), scene) {
        merge_scene_blocks(obj, scene);
    }
    resolve_tokens(&mut ui);
    ui
}

// (The dormant `load_arrangement_str` / `load_styles_merged` loaders died in S10
// — zero production callers. The surviving DATA entry points are
// `flicker_script::parse_ui_json` (the ONE arrangement reader) + the styles
// loaders above; a future CMS re-adds against those, not against a dormant
// convenience wrapper.)

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

    /// The canonical Prism palette (`theme.tokens` in `ui_theme.json`) as
    /// `(name, rgba)` — the one source of truth both fallback gates below check
    /// their copies against.
    fn theme_tokens() -> Vec<(String, [f64; 4])> {
        let elements: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../content/sensorium/resources/ui_theme.json"
        ))
        .expect("ui_theme.json parses");
        elements["theme"]["tokens"]
            .as_object()
            .expect("theme.tokens present")
            .iter()
            .map(|(k, v)| {
                let c: Vec<f64> = v
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_f64().unwrap())
                    .collect();
                (k.clone(), [c[0], c[1], c[2], c[3]])
            })
            .collect()
    }

    /// The name of the token `parts` matches exactly, else the nearest one — so a
    /// drifted fallback's failure message names what it probably meant.
    fn nearest_token(tokens: &[(String, [f64; 4])], parts: &[f64]) -> Option<String> {
        if tokens
            .iter()
            .any(|(_, tv)| tv.iter().zip(parts).all(|(a, b)| (a - b).abs() < 1e-6))
        {
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
            let Some(rest) = line.strip_prefix("const ") else {
                continue;
            };
            let Some((name, value)) = rest.split_once(": [f32; 4] = [") else {
                continue;
            };
            let Some((inner, _)) = value.split_once(']') else {
                continue;
            };
            let parts: Vec<f64> = inner
                .split(',')
                .filter_map(|p| p.trim().parse::<f64>().ok())
                .collect();
            if parts.len() != 4 {
                continue;
            }
            checked += 1;
            if let Some(near) = nearest_token(&tokens, &parts) {
                bad.push(format!(
                    "component.rs `{name}` = {parts:?} (nearest token: ${near})"
                ));
            }
        }
        assert!(
            checked >= 9,
            "the gate must actually find the fallback consts ({checked})"
        );
        assert!(
            bad.is_empty(),
            "engine fallback consts drifted from theme.tokens:\n{}",
            bad.join("\n")
        );
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
            ("KEYCAP_FACE", "ink"),
            ("KEYCAP_EDGE", "dim"),
            ("KEYCAP_INK", "stage_black"),
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
                let parts: Vec<f64> = inner
                    .split(',')
                    .filter_map(|p| p.trim().parse::<f64>().ok())
                    .collect();
                (parts.len() == 4).then(|| (name.to_string(), parts))
            })
            .collect();

        let mut bad = Vec::new();
        for (name, token) in PAIRS {
            let Some((_, parts)) = found.iter().find(|(n, _)| n == name) else {
                bad.push(format!(
                    "`{name}` is gone from component.rs — the pairing is stale"
                ));
                continue;
            };
            let Some((_, want)) = tokens.iter().find(|(t, _)| t == token) else {
                bad.push(format!(
                    "`{name}` names `${token}`, which theme.tokens does not have"
                ));
                continue;
            };
            if !want.iter().zip(parts).all(|(a, b)| (a - b).abs() < 1e-6) {
                bad.push(format!("`{name}` = {parts:?} but `${token}` = {want:?}"));
            }
        }
        assert!(
            bad.is_empty(),
            "engine fallback consts drifted from their tokens:\n{}",
            bad.join("\n")
        );

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
    fn unknown_kinds_catches_a_typo() {
        let leaf = |kind: &str| UiNode {
            component: kind.to_string(),
            ..Default::default()
        };
        let mut screen = leaf("surface");
        screen.children = vec![leaf("cell"), leaf("button"), leaf("text")];
        assert!(
            unknown_kinds(&screen).is_empty(),
            "a well-formed tree is clean"
        );

        screen.children.push(leaf("colunm")); // the typo a rename leaves behind
        assert_eq!(
            unknown_kinds(&screen),
            vec!["colunm".to_string()],
            "a stale kind is reported"
        );

        assert!(
            !is_known_kind("core"),
            "`core` is the emitter library, never a component kind"
        );
        for kind in rust_component_kinds() {
            assert!(
                is_known_kind(kind),
                "`{kind}` is an engine component but not a legal kind"
            );
        }
    }

    /// The strings gate has to be able to FAIL — and its exemptions must hold, or
    /// every screen it guards would drown in false positives (glyphs, formats,
    /// bind-shadowed literals) or miss real copy.
    #[test]
    fn raw_display_literals_finds_copy_and_honours_exemptions() {
        let mut screen = UiNode {
            component: "surface".into(),
            ..Default::default()
        };
        let node = |props: &[(&str, &str)]| {
            let mut n = UiNode {
                component: "text".into(),
                ..Default::default()
            };
            for (k, v) in props {
                n.props.insert(
                    (*k).to_string(),
                    flicker_script::Value::Text((*v).to_string()),
                );
            }
            n
        };
        screen.children = vec![
            node(&[("text", "Hello World")]), // raw copy → reported
            node(&[("label", "$menu_quit")]), // token → exempt
            node(&[("text", "")]),            // empty → exempt
            node(&[("label", "✕")]),          // single glyph → exempt
            node(&[("text", "·")]),           // no alphabetics → exempt
            node(&[("text", "%d")]),          // pure format → exempt
            node(&[("text", "%.2f%%")]),      // pure format chain → exempt
            node(&[("text", "dead"), ("text_bind", "live")]), // bind-shadowed → exempt
            node(&[("text", "Hello World")]), // duplicate → deduped
        ];
        // A nested child's literal is walked too.
        let mut holder = UiNode {
            component: "cell".into(),
            ..Default::default()
        };
        holder.children = vec![node(&[("label", "Nested Copy")])];
        screen.children.push(holder);

        assert_eq!(
            raw_display_literals(&screen),
            vec!["Hello World".to_string(), "Nested Copy".to_string()]
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
        assert_eq!(
            ui["modal"]["title"]["color"],
            serde_json::json!([0.9, 0.9, 0.8, 1.0])
        );
        assert_eq!(
            ui["modal"]["buttons"]["fill"],
            serde_json::json!([0.1, 0.2, 0.3, 1.0])
        );
        assert_eq!(
            ui["screens"]["menu"]["overlay"],
            serde_json::json!([0.1, 0.2, 0.3, 1.0])
        );
        // literal strings (labels) are untouched; an unknown token is left as-is.
        assert_eq!(ui["screens"]["menu"]["title"], serde_json::json!("START"));
        assert_eq!(ui["oops"], serde_json::json!("$missing"));
    }

    /// The theme trio: `load_styles` merges the sibling satellite files into ONE
    /// root (so stage sources + chrome resolve exactly as when everything lived in
    /// one file), the theme file WINS a key collision, and a satellite carrying a
    /// `theme` key is REFUSED — the palette-fork guard (one-palette law, 8D8A4215).
    #[test]
    fn load_styles_merges_satellites_and_refuses_a_palette_fork() {
        let dir = std::env::temp_dir().join("flicker_theme_trio_merge");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(
            dir.join("ui_theme.json"),
            r#"{ "theme": { "tokens": { "ink": [0.9, 0.9, 0.8, 1.0] } },
                 "panel": { "fill": "$ink" }, "mine": { "kept": true } }"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("ui_stages.json"),
            r#"{ "stages": { "test_stage": { "tint": "$ink" } },
                 "theme": { "tokens": { "ink": [1.0, 0.0, 0.0, 1.0] } },
                 "mine": { "kept": false } }"#,
        )
        .unwrap();

        let ui = load_styles(dir.join("ui_theme.json"));
        // Satellite keys merged in — and their $tokens resolved against the ONE palette.
        assert_eq!(
            ui["stages"]["test_stage"]["tint"],
            serde_json::json!([0.9, 0.9, 0.8, 1.0])
        );
        // The fork was refused: the satellite's `theme` never replaced the palette.
        assert_eq!(ui["panel"]["fill"], serde_json::json!([0.9, 0.9, 0.8, 1.0]));
        // A colliding key keeps the theme file's value.
        assert_eq!(ui["mine"]["kept"], serde_json::json!(true));

        // The REAL shipped pair: the split stages land in the merged root, and the
        // shipped satellite carries no `theme` key (the guard has nothing to refuse).
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/resources/ui_theme.json");
        let shipped = load_styles(&root);
        assert!(
            shipped
                .get("stages")
                .and_then(|s| s.as_object())
                .is_some_and(|s| !s.is_empty()),
            "the shipped ui_stages.json merges into the theme root"
        );
    }

    /// THE FIVE-LINE SPLIT, file half (Aaron 2026-08-12): `ui_theme.json` carries
    /// the `theme` node and NOTHING else — the default UI theme colors. Scene
    /// values live in scene files, weights/effects in ui_style.json, structure in
    /// Rust drawing code. Any other key here is the violation returning.
    #[test]
    fn ui_theme_json_is_the_theme_and_nothing_else() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/resources/ui_theme.json");
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&root).expect("theme file reads"))
                .expect("theme file parses");
        let obj = raw.as_object().expect("theme file is an object");
        for key in obj.keys() {
            assert!(
                key == "theme" || key.starts_with('_'),
                "`{key}` does not belong in ui_theme.json — THE ONLY DATA IN THIS \
                 FILE IS THE THEME (five-line architecture): scene values go in the \
                 scene's own file, weights/effects in ui_style.json, component \
                 structure in Rust"
            );
        }
        assert!(
            obj.get("theme").and_then(|t| t.get("tokens")).is_some(),
            "the theme node carries the one palette (theme.tokens)"
        );
    }

    /// THE FIVE-LINE SPLIT, shared-file half (Aaron 2026-08-12): NO component
    /// block lives in ANY shared file. `ui_theme.json` = the theme (colors) only;
    /// `ui_style.json` = truly-global weight/effect defaults (currently none) and
    /// never a per-component or per-scene block. A component's layout details live
    /// in the scene files that use it; its structure is Rust drawing code.
    #[test]
    fn no_component_block_lives_in_a_shared_file() {
        const COMPONENT_BLOCKS: &[&str] = &[
            "modal",
            "badge",
            "tooltip",
            "pad_glyphs",
            "paged_menu",
            "resource_gauge",
            "action_slot",
            "medallion",
            "rtt_holder",
            "slider",
            "panel",
            "stat_dot",
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/resources");
        for file in ["ui_theme.json", "ui_style.json", "ui_stages.json"] {
            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join(file)).expect("shared file reads"),
            )
            .expect("shared file parses");
            for key in COMPONENT_BLOCKS {
                assert!(
                    raw.get(key).is_none(),
                    "`{key}` is in {file} — component layout details live in the \
                     SCENE FILES that use them, never a shared file"
                );
            }
        }
        // ui_style.json also never carries a palette (the fork guard's target).
        let style: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("ui_style.json")).expect("style reads"),
        )
        .expect("style parses");
        assert!(
            style.get("theme").is_none(),
            "ui_style.json must never carry a `theme` key"
        );
    }

    /// **NO SCENE READS A DEVICE OR NAMES A PANE STYLE.** A source-level sweep of
    /// every scene crate — the channel three separate defects travelled, closed
    /// in one gate so none of them can grow back quietly. (Moved here verbatim
    /// when the template tier was deleted — 201F4F51; it is a scene-crate source
    /// sweep, never template-specific.)
    ///
    /// * `input.gamepad(` / `.gamepad(0)` — a scene reaching past the input map
    ///   for a stick. The camera reads BOUND SIGNALS (`signal_axis`); a raw read
    ///   re-applies the deadzone a second time, which is a bug you can only find
    ///   by measuring. `flicker-controllertester` is exempt and only it: that
    ///   bench IS the device visualizer, so reading the device is its subject.
    /// * `tri_pane.` — the retired per-bench pane palette. There is ONE pane
    ///   palette now (`panel.resting` / `panel.focused`) and the PANEL draws
    ///   itself from the focus the walker holds; a scene naming a pane skin is a
    ///   scene deciding what focus looks like.
    /// * a walker-owned `on_*` declaration — Confirm, Cancel, `Nav*`, `Panel*`
    ///   and `ChordBegin` mean one thing on every screen, so no scene may name
    ///   them in its own props. (The template/proto channel that once needed its
    ///   own gate is GONE with the template tier — 201F4F51 — so a scene's props
    ///   are now the ONLY channel a declaration travels.) The allow-list below is
    ///   EMPTY: the gate fails the moment any scene falls in (rule 98232A50).
    /// * a private globe: `fn build_shell` / `struct OrbitCam` in a scene. There
    ///   is ONE globe in Prism (`flicker-globe`) and it was three copies twice.
    #[test]
    fn no_scene_reads_a_device_or_names_a_pane_style() {
        use flicker_input_core::ActionSignal;
        use std::path::{Path, PathBuf};

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scenes");
        let root = root.canonicalize().expect("Alpha/crates/scenes resolves");

        fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).expect("scene dir reads").flatten() {
                let p = e.path();
                if p.is_dir() {
                    rust_files(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        files.sort();
        assert!(
            files.len() > 20,
            "the sweep found the scene crates: {}",
            files.len()
        );
        let crate_of = |p: &Path| -> String {
            p.strip_prefix(&root)
                .ok()
                .and_then(|r| {
                    r.components()
                        .next()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                })
                .unwrap_or_default()
        };

        // The walker's OWN answer to "whose signal is this", folded into the two
        // shapes a DECLARATION takes in Rust — the `(prop, result)` pair a bench
        // folds into its root, and a direct `props.insert`. Never a second list
        // here that could drift from what the layer actually consumes; and
        // naming the SHAPE rather than the bare string is what lets a migrated
        // bench's own absence gate mention the signal it is disowning.
        let owned: Vec<String> = ActionSignal::ALL
            .iter()
            .copied()
            .filter(|s| crate::walker_owned(*s))
            .flat_map(|s| {
                let mut key = String::from("on");
                for c in s.name().chars() {
                    if c.is_uppercase() {
                        key.push('_');
                    }
                    key.extend(c.to_lowercase());
                }
                [format!("(\"{key}\", \""), format!("insert(\"{key}\"")]
            })
            .collect();
        assert!(
            owned.iter().any(|k| k == "(\"on_confirm\", \"")
                && owned.iter().any(|k| k == "insert(\"on_panel_next\""),
            "the fold produced the declaration shapes a scene writes: {owned:?}"
        );

        // A scene camera that is NOT the globe's: Solar Birth's `OrbitCam` is a
        // cinematic POSE HOLDER — the `.flight` player drives it and hands over to
        // the pointer mid-shot — and it frames a solar system, not a planet at
        // `flicker_globe::RADIUS`. Folding it in is a real change to the shared
        // camera's contract, so it is named here rather than waved through.
        const CAMERAS_NOT_THE_GLOBES: [&str; 1] = ["flicker-solarbirth"];

        // The scenes that still declare a walker-owned signal. EMPTY as of the
        // template-tier removal (2026-08-12): quartermaster / godmode / assetpipeline
        // each folded one in from their template-built UI; stubbing those benches off
        // templates removed the declarations, so the gate now enforces ZERO. A NEW name
        // here means a scene stole a walker signal (violation F1) — migrate it, don't
        // allow-list it. (Rule 98232A50: the backlog shrinks as benches migrate.)
        const NOT_YET_MIGRATED: [&str; 0] = [];

        let (mut devices, mut panes, mut globes) = (Vec::new(), Vec::new(), Vec::new());
        let mut declarers: Vec<String> = Vec::new();
        for f in &files {
            let krate = crate_of(f);
            let src = std::fs::read_to_string(f).expect("scene source reads");
            for (n, line) in src.lines().enumerate() {
                let at = format!("{}:{}", f.strip_prefix(&root).unwrap().display(), n + 1);
                if krate != "flicker-controllertester"
                    && (line.contains("input.gamepad(") || line.contains(".gamepad(0)"))
                {
                    devices.push(at.clone());
                }
                if line.contains("tri_pane.") {
                    panes.push(at.clone());
                }
                if line.contains("fn build_shell")
                    || (line.contains("struct OrbitCam")
                        && !CAMERAS_NOT_THE_GLOBES.contains(&krate.as_str()))
                {
                    globes.push(at.clone());
                }
                if owned.iter().any(|o| line.contains(o.as_str())) && !declarers.contains(&krate) {
                    declarers.push(krate.clone());
                }
            }
        }
        assert!(
            devices.is_empty(),
            "a scene reached past the input map for a device: {devices:?}"
        );
        assert!(
            panes.is_empty(),
            "a scene named the retired pane palette: {panes:?}"
        );
        assert!(
            globes.is_empty(),
            "a scene grew its own globe again: {globes:?}"
        );
        declarers.sort();
        let mut expected: Vec<String> = NOT_YET_MIGRATED.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(
            declarers, expected,
            "the walker-owned backlog moved. A NEW name means a scene stole a walker signal; a \
             MISSING name means a bench migrated and this list must shrink with it"
        );
    }

    /// **ABSENCE GATE: no shipped scene names the removed template tier.** The
    /// `template` / `slots` keys are gone (201F4F51) and both readers reject them,
    /// but a scene file could still be authored with one and only fail when it
    /// loads. This scans every `content/sensorium/scenes/*.scene.json` for those
    /// keys at ANY depth, so the tier cannot quietly regrow in shipped content — a
    /// build failure, not a runtime hole.
    #[test]
    fn no_shipped_scene_names_the_template_tier() {
        use std::path::PathBuf;
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content/sensorium/scenes");
        let dir = dir
            .canonicalize()
            .expect("content/sensorium/scenes resolves");

        fn has_key(v: &serde_json::Value, key: &str) -> bool {
            match v {
                serde_json::Value::Object(m) => {
                    m.contains_key(key) || m.values().any(|c| has_key(c, key))
                }
                serde_json::Value::Array(a) => a.iter().any(|c| has_key(c, key)),
                _ => false,
            }
        }

        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("scenes dir reads").flatten() {
            let path = entry.path();
            if !path.to_string_lossy().ends_with(".scene.json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("scene file reads");
            let json: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
            for key in ["template", "slots"] {
                assert!(
                    !has_key(&json, key),
                    "{} names the removed template tier (a `{key}` key) — the tier is gone \
                     (201F4F51); author component KINDS with nested `children`",
                    path.display()
                );
            }
            checked += 1;
        }
        assert!(
            checked > 0,
            "the gate found the shipped scene files in {}",
            dir.display()
        );
    }

    /// **ABSENCE GATE: no shipped scene authors a retired surface kind.** `screen`,
    /// `rtt` and `viewport` all became ONE kind, `surface` (Aaron 2026-08-21: "surface
    /// is the correct unified term"; the root screen is a surface too). The kind rosters
    /// no longer know the old names, so a scene using one would fail its own roster gate
    /// at load — this names the retirement directly and scans `shared/` as well.
    #[test]
    fn no_shipped_scene_authors_a_retired_surface_kind() {
        use std::path::{Path, PathBuf};
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content/sensorium/scenes");
        let dir = dir
            .canonicalize()
            .expect("content/sensorium/scenes resolves");

        fn kinds(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    if let Some(serde_json::Value::String(k)) = m.get("component") {
                        out.push(k.clone());
                    }
                    m.values().for_each(|c| kinds(c, out));
                }
                serde_json::Value::Array(a) => a.iter().for_each(|c| kinds(c, out)),
                _ => {}
            }
        }
        fn walk(dir: &Path, checked: &mut usize) {
            for entry in std::fs::read_dir(dir).expect("scenes dir reads").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, checked);
                    continue;
                }
                if !path.to_string_lossy().ends_with(".scene.json") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("scene file reads");
                let json: serde_json::Value = serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
                let mut found = Vec::new();
                kinds(&json, &mut found);
                for retired in ["screen", "rtt", "viewport"] {
                    assert!(
                        !found.iter().any(|k| k == retired),
                        "{} authors the retired kind `{retired}` — the one kind is `surface` \
                         (root and nested alike; 2026-08-21)",
                        path.display()
                    );
                }
                *checked += 1;
            }
        }
        let mut checked = 0;
        walk(&dir, &mut checked);
        assert!(
            checked > 0,
            "the gate found the shipped scene files in {}",
            dir.display()
        );
    }
}
