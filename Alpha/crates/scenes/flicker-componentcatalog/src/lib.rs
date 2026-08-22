//! flicker-componentcatalog: the UI TEST scene — a workbench that shows ONE demo copy
//! of every Rust widget component ([`RUST_COMPONENT_KINDS`]) with all its features
//! enabled. Every component sits in its own card box; all 27 flow top-to-bottom in a
//! scrollable tray on the right. A left nav rail of bookmarks SCROLLS the tray to a
//! card, and the bookmark of the card at the top of the view highlights.
//!
//! A Developer-realm bench (like Click Trainer), on the ratified pattern:
//! - **template-free** — `componentcatalog.scene.json` names primitive component KINDS
//!   directly (201F4F51), loaded by the bench; every display string is a `$token`.
//! - **pair-script Lua** (five-line split, 491BD9BB) — `componentcatalog.lua` owns the
//!   scene's component logic (demo seeds, nav highlight, the Paged Menu gates); the
//!   tray itself is static and the scroll-to stays pure engine wiring.
//! - **on the pump** (input-P3, 0569DA9B): owns no resolver — the PUMP resolves this
//!   frame's events and hands them in via [`SceneInput`].
//!
//! Scroll-to: the walker reports every card box's resolved rect ([`UiFrame::rect`]); the
//! scroll offset that brings card `i` to the top is `card_i.y - card_0.y` (the height
//! stacked above it), read live from the layout — no hardcoded heights. Esc opens pause.

use std::time::Duration;

use flicker::render::{
    grid_segments_xy, Rect, Renderer, TextureHandle, Vec3, ViewportFiller, ViewportLayout,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, SceneDef, SurfacePointer, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

/// The scene's PAIR SCRIPT (`SceneName.lua` — the scene's component logic:
/// demo seeds, nav highlight, the Paged Menu card's page/tab gates).
const CATALOG_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/componentcatalog.lua");

/// The scrollable content tray's `bind` — the offset (px) the bench sets to scroll-to a
/// card, and the wheel writes as you scroll.
const CONTENT_SCROLL_BIND: &str = "cat_content_scroll";

/// Every `card_<i>` id in the authored tray, in tree order — DERIVED from the tree
/// at load, never counted by hand. The card list, the nav loop, and the section
/// tracker all walk this, so adding a card is one authored box + its bookmark and
/// zero bookkeeping (the roster-coverage gate below tells you when one is owed).
fn card_ids(tree: &UiNode) -> Vec<String> {
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        if n.id.starts_with("card_") {
            out.push(n.id.clone());
        }
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(tree, &mut out);
    out
}

/// Every two-way `bind` the authored tree carries — the fold-back set (rule
/// 3A04B4CE), DERIVED from the tree: the tree IS the list, so a new bound demo
/// control round-trips without touching Rust.
fn tree_binds(tree: &UiNode) -> Vec<String> {
    fn walk(n: &UiNode, out: &mut Vec<String>) {
        if let Some(b) = n.bind.as_deref() {
            if !b.is_empty() && !out.iter().any(|k| k == b) {
                out.push(b.to_string());
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

/// The viewport-card demo's framing radius and wireframe colours.
const DEMO_RADIUS: f32 = 2.2;
const DEMO_GROUND: [f32; 4] = [0.25, 0.28, 0.34, 1.0];
const DEMO_CUBE: [f32; 4] = [0.55, 0.75, 0.95, 1.0];

/// The viewport demo's content — a unit cube standing on the grid floor, as the 12
/// wireframe edges `Renderer::draw_lines` takes. One card, so rebuilding it a frame is free.
fn demo_cube() -> Vec<(Vec3, Vec3)> {
    let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let p = [
        c(-1.0, -1.0, -1.0),
        c(1.0, -1.0, -1.0),
        c(1.0, 1.0, -1.0),
        c(-1.0, 1.0, -1.0),
        c(-1.0, -1.0, 1.0),
        c(1.0, -1.0, 1.0),
        c(1.0, 1.0, 1.0),
        c(-1.0, 1.0, 1.0),
    ];
    vec![
        (p[0], p[1]),
        (p[1], p[2]),
        (p[2], p[3]),
        (p[3], p[0]),
        (p[4], p[5]),
        (p[5], p[6]),
        (p[6], p[7]),
        (p[7], p[4]),
        (p[0], p[4]),
        (p[1], p[5]),
        (p[2], p[6]),
        (p[3], p[7]),
    ]
}

/// The UI test scene. Everything a frame needs lives here; the shell drives it through
/// the [`Scene`] trait.
pub struct ComponentCatalog {
    /// The component tree, parsed ONCE from `componentcatalog.scene.json`.
    tree: Option<UiNode>,
    /// The screen's declarative intents (S9) — `on_menu = "pause_open"`.
    ui_intents: UiIntents,
    /// Token-resolved `ui_theme.json` styles.
    ui_styles: serde_json::Value,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// The theme's engine textures in ID ORDER — for `render_hud` + the sprite card.
    textures: Vec<TextureHandle>,
    /// Theme for the pause overlay we push (built once in `enter`).
    theme: Option<Theme>,
    /// The retained walker [`UiState`] the nav writes its ONE focus id through.
    ui_state: UiState,
    /// The content tray's scroll offset (px). Set on a bookmark click (scroll-to), moved
    /// by the wheel, always adopted back clamped from the list's echo.
    content_scroll: f32,
    /// The card currently at the top of the tray — which bookmark highlights.
    section: usize,
    /// The tray's `card_<i>` ids in tree order — derived from the authored tree.
    cards: Vec<String>,
    /// Every two-way bind the tree carries — the fold-back set, derived likewise.
    binds: Vec<String>,
    /// Live values every bound control round-trips through (rule 3A04B4CE).
    /// Starts EMPTY — the PAIR SCRIPT seeds the demo values; a committed value
    /// echoed back here wins over the seed from then on.
    demo: ValueMap,
    /// The PAIR SCRIPT host (`componentcatalog.lua`).
    script: Option<ScriptHost>,
    /// The viewport card's shared filler — built LAZILY on first render (it needs
    /// `&mut Renderer`), then seated in the reserved rect each frame.
    viewport: Option<ViewportFiller>,
    /// The rect + layout the walker reserved for the `cat_surface` node this frame
    /// (captured in `update`; consumed in the input-less `render`). `None` when off screen.
    surface_seat: Option<(Rect, ViewportLayout)>,
    /// The walker's pointer SAMPLE for the card's surface this frame (the barrier) —
    /// `render` replays it to orbit the panel under the cursor; `render` gets no input
    /// of its own, and the bench never reads the device for it.
    pointer: Option<SurfacePointer>,
}

impl ComponentCatalog {
    /// A fresh catalog scene — takes its authored tree off the MANIFEST's def (the
    /// kernel parsed the file; this bench is only the behaviour that plays it) and
    /// seeds the demo values.
    pub fn new(def: &SceneDef) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let tree = def.tree.clone();
        if tree.is_none() {
            tracing::error!("scene '{}' declares no `tree` — no UI", def.id);
        }
        let ui_intents = tree.as_ref().map(UiIntents::of).unwrap_or_default();
        let cards = tree.as_ref().map(card_ids).unwrap_or_default();
        let binds = tree.as_ref().map(tree_binds).unwrap_or_default();
        Self {
            tree,
            ui_intents,
            ui_styles,
            hud_commands: Vec::new(),
            textures: Vec::new(),
            theme: None,
            ui_state: UiState::default(),
            content_scroll: 0.0,
            section: 0,
            cards,
            binds,
            demo: ValueMap::new(),
            script: match ScriptHost::new(CATALOG_SCRIPT, "componentcatalog.lua") {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("componentcatalog.lua failed to load — raw values only: {e}");
                    None
                }
            },
            viewport: None,
            surface_seat: None,
            pointer: None,
        }
    }

    /// **What the components read.** The engine publishes the RAW runtime
    /// variables (committed demo echoes, the tray section, the scroll offset);
    /// the PAIR SCRIPT derives everything else — seeds, the nav highlight, the
    /// Paged Menu gates (five-line split: logic lives in componentcatalog.lua).
    fn model(&self) -> ValueMap {
        let mut raw = self.demo.clone();
        raw.set(CONTENT_SCROLL_BIND, f64::from(self.content_scroll));
        raw.set("section", self.section as f64);
        raw.set("card_count", self.cards.len() as f64);
        // Q2 (ruling 7AB130A7): publish the live display device so the Paged Menu drops
        // its pad shoulder-glyph hints on kbm and shows them on a controller.
        raw.set(
            "input_device",
            flicker::input_device::last_input_context().token(),
        );
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("componentcatalog: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("componentcatalog.lua derive() failed: {e}"),
            }
        }
        m
    }

    /// Fold each committed demo value so its control shows it next frame (rule 3A04B4CE);
    /// a control that did not move re-echoes its resting value, a no-op.
    fn apply_results(&mut self, results: &ValueMap) {
        for k in &self.binds {
            if let Some(v) = results.get(k) {
                self.demo.set(k.clone(), v.clone());
            }
        }
    }

    /// The card whose top is nearest the tray's current scroll offset — the one at the top
    /// of the view, so its bookmark highlights. `card_i.y - card_0.y` is the height stacked
    /// above card `i`; the active card is the one whose stack ≈ the offset.
    fn active_card(&self, rects: &[(String, [f32; 4])], scroll: f32, fallback: usize) -> usize {
        let card_y = |id: &str| rects.iter().find(|(n, _)| n == id).map(|(_, r)| r[1]);
        let Some(first) = self.cards.first().and_then(|id| card_y(id)) else {
            return fallback;
        };
        let mut best = (f32::MAX, fallback);
        for (i, id) in self.cards.iter().enumerate() {
            if let Some(ci) = card_y(id) {
                let d = ((ci - first) - scroll).abs();
                if d < best.0 {
                    best = (d, i);
                }
            }
        }
        best.1
    }
}

impl Scene for ComponentCatalog {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
        let theme = Theme::build(renderer);
        let entries = theme.lua_textures();
        self.textures = entries.iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
    }

    fn update(
        &mut self,
        _dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        let Some(tree) = self.tree.as_ref() else {
            return Transition::None;
        };
        let screen = renderer.size();
        let model = self.model();

        // ONE walker pass: layout + hit-test + draw the tray.
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        // The viewport card: capture the rect+layout the walker reserved and this frame's
        // pointer sample, for the input-less `render` to seat + orbit (a left-drag over a
        // panel orbits it; the filler self-gates to the cursor-over-viewport).
        self.surface_seat = frame.surface_slot("cat_surface");
        // …and the walker's pointer SAMPLE for the surface (the barrier, A8C9F02B §4b):
        // present only while the cursor is over the card's well with no UI over it, or
        // while a press that began there is still held — never a device read.
        self.pointer = frame.surface_pointer("cat_surface").cloned();
        let hud_hit = frame.results.is_on("hud_hit");
        // Take the frame apart: the draw commands to blit, the result values to read, and
        // the per-card RECTS the scroll-to needs (each card box reports its resolved Y).
        self.hud_commands = frame.commands;
        let mut results = frame.results;
        let rects = frame.rects;

        // ── The input seam (input-P3): dispatch the pump's events through the walker (with
        // nav); a fired nav/action folds into `results` exactly like a click. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, hud_hit)
            .with_nav(tree, &model)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        for name in walker.take_fired() {
            results.set(name.as_str(), true);
        }

        // The wheel wrote (and the list echoed, clamped) the tray offset — adopt it, so
        // the scene's copy tracks the list's clamped truth.
        if let Some(v) = results.number(CONTENT_SCROLL_BIND) {
            self.content_scroll = v as f32;
        }
        // A bookmark fired — scroll the tray so that card sits at the top. The offset is
        // the height stacked above it, read live from the layout (`card_i.y - card_0.y`),
        // so it is correct at any current scroll and for any card height. Overrides the
        // wheel adopt above (an explicit jump outranks the resting echo — rule 3A04B4CE).
        let card_y = |id: &str| rects.iter().find(|(n, _)| n == id).map(|(_, r)| r[1]);
        let jump = self
            .cards
            .iter()
            .find(|card| results.is_on(&format!("nav_{}", &card["card_".len()..])))
            .cloned();
        if let (Some(c0), Some(card)) = (self.cards.first().and_then(|id| card_y(id)), jump) {
            if let Some(ci) = card_y(&card) {
                self.content_scroll = (ci - c0).max(0.0);
            }
        }
        self.apply_results(&results);
        // Which card is at the top now → which bookmark highlights next frame.
        self.section = self.active_card(&rects, self.content_scroll, self.section);

        // Menu opens the shell's pause overlay (the screen's DECLARED `on_menu`).
        if results.is_on("pause_open") {
            if let Some(theme) = self.theme {
                let pause_map = flicker_shell::input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse);
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    &pause_map,
                    &AbstractControls::default(),
                    &GamepadConfig::default(),
                )));
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let base_layer = renderer.layer();
        // The viewport card FIRST. Its frame graph's offscreen passes RESET the per-frame
        // draw queues (the "render RTTs before the main view" rule), so anything queued
        // before it is discarded — the HUD MUST come after. Seat the shared filler in the
        // rect the walker reserved, orbit the panel under the cursor on a left-drag, and
        // render a wireframe stage (ground grid + a cube) into it — the kind's LIVE catalog
        // exerciser. Wheel is left out so it never fights the tray scroll. Composites at
        // `base+2`, ABOVE the HUD's `base+1`, so the views land inside the card's well.
        if let Some((rect, layout)) = self.surface_seat {
            if self.viewport.is_none() {
                self.viewport = Some(ViewportFiller::new(renderer, layout));
            }
            let vf = self.viewport.as_mut().unwrap();
            vf.set_rect(rect);
            if let Some(p) = &self.pointer {
                let orbit = p.captured && p.left;
                vf.apply_pointer(
                    p.cursor,
                    renderer.size(),
                    p.delta,
                    orbit,
                    false,
                    0.0,
                    DEMO_RADIUS,
                );
            }
            let ground = grid_segments_xy(0.5, 2.5, -1.0);
            let cube = demo_cube();
            vf.render(renderer, base_layer + 2.0, DEMO_RADIUS, |r, _view| {
                r.draw_lines(&ground, DEMO_GROUND);
                r.draw_lines(&cube, DEMO_CUBE);
            });
        }
        if let Some(&white) = self.textures.first() {
            renderer.set_layer(base_layer + 1.0);
            render_hud(renderer, &self.hud_commands, white, &self.textures);
            renderer.set_layer(base_layer);
        }
    }
}

/// The bench's launchable-scene factory — the CLIENT BEHAVIOUR the roster registers:
/// the manifest resolves `componentcatalog.scene.json` by id and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(ComponentCatalog::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped scene file, read by the gates below exactly as the manifest reads it.
    const CATALOG_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/componentcatalog.scene.json");

    /// The nav's active/idle bookmark styles — the TEST-side mirror of the paths
    /// componentcatalog.lua publishes through each `style_bind` (the Lua owns the
    /// runtime choice; these pin it).
    const NAV_ACTIVE_STYLE: &str = "modal.buttons.variants.primary";
    const NAV_IDLE_STYLE: &str = "modal.buttons.variants.secondary";

    /// THE PAIR-SCRIPT REGRESSION GATE: componentcatalog.lua loads; derive()
    /// seeds the demo values, lights the active bookmark, and gates the Paged
    /// Menu card — the scene's component logic lives in the pair script.
    #[test]
    fn the_pair_script_seeds_and_derives() {
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        let cat = ComponentCatalog::new(&def);
        assert!(
            cat.script.is_some(),
            "componentcatalog.lua loads (the pair script)"
        );
        let m = cat.model();
        assert!(m.is_on("cat_check_val"), "the checkbox demo seeds ON");
        assert_eq!(
            m.text("nav_sty_0"),
            Some(NAV_ACTIVE_STYLE),
            "bookmark 0 starts active"
        );
        assert_eq!(m.text("nav_sty_1"), Some(NAV_IDLE_STYLE), "the rest rest");
        assert!(
            m.is_on("cat_pm_on_p0"),
            "the Paged Menu card opens on page 1"
        );
    }

    /// THE AUTHORED-STYLE-PATH GATE (S1 of the styling pass): every style path any
    /// shipped scene tree names must resolve to a style BLOCK in that scene's
    /// merged styles. A path that resolves to nothing draws compiled defaults
    /// SILENTLY — the walker cannot warn (the lookup simply misses), so a card
    /// that claims to demo styling through a dead path is a lie nobody sees. The
    /// day it was written this gate caught five dead paths in the catalog and the
    /// sablework root slab.
    #[test]
    fn every_authored_style_path_resolves_to_a_block() {
        const BLOCK_PROPS: [&str; 9] = [
            "style",
            "style_off",
            "panel_style",
            "divider_style",
            "rule_style",
            "glyph_style",
            "tab_active",
            "tab_idle",
            "runes_style",
        ];
        fn jwalk<'v>(root: &'v serde_json::Value, path: &str) -> Option<&'v serde_json::Value> {
            path.split('.').try_fold(root, |v, seg| v.get(seg))
        }
        // Raw-JSON walk, not SceneDef: the shared modal trees (scenes/shared/)
        // carry no `behaviour` — the manifest skips that folder and a host scene
        // merges them — but their authored paths must resolve all the same.
        fn collect(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
            let kind = node
                .get("component")
                .and_then(|c| c.as_str())
                .unwrap_or("?");
            for prop in BLOCK_PROPS {
                if let Some(path) = node.get(prop).and_then(|p| p.as_str()) {
                    out.push((kind.to_string(), path.to_string()));
                }
            }
            if let Some(kids) = node.get("children").and_then(|c| c.as_array()) {
                for kid in kids {
                    collect(kid, out);
                }
            }
        }

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/scenes");
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for folder in [dir.clone(), dir.join("shared")] {
            for entry in std::fs::read_dir(&folder).expect("scenes folder reads") {
                let p = entry.expect("dir entry").path();
                if p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".scene.json"))
                {
                    files.push(p);
                }
            }
        }
        assert!(!files.is_empty(), "the scenes folder holds scene files");

        let mut broken = Vec::new();
        for path in files {
            let id = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .trim_end_matches(".scene.json")
                .to_string();
            let text = std::fs::read_to_string(&path).expect("scene file reads");
            let doc: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{id}.scene.json parses: {e}"));
            let Some(tree) = doc.get("tree") else {
                continue;
            };
            // A shared modal tree renders under its HOST's merge (today: Main
            // merges scenes/shared/* back via main_scene_styles), so its paths
            // resolve against host styles ⊕ its own — that hosting contract is
            // part of what this gate pins.
            let in_shared = path.parent().is_some_and(|p| p.ends_with("shared"));
            let own = doc
                .get("styles")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let scene_styles = if in_shared {
                let main_text = std::fs::read_to_string(dir.join("Main.scene.json"))
                    .expect("the host scene file reads");
                let main_doc: serde_json::Value =
                    serde_json::from_str(&main_text).expect("Main.scene.json parses");
                let mut merged = main_doc.get("styles").cloned().unwrap_or_default();
                if let (Some(m), Some(o)) = (merged.as_object_mut(), own.as_object()) {
                    for (k, v) in o {
                        m.insert(k.clone(), v.clone());
                    }
                }
                merged
            } else {
                own
            };
            let styles = flicker::ui::load_shared_styles(Some(&scene_styles));
            let mut named = Vec::new();
            collect(tree, &mut named);
            for (kind, p) in named {
                match jwalk(&styles, &p) {
                    Some(v) if v.is_object() => {}
                    Some(_) => broken.push(format!(
                        "{id}: {kind} style '{p}' resolves to a non-block value"
                    )),
                    None => broken.push(format!("{id}: {kind} names style '{p}' → NOTHING")),
                }
            }
        }
        assert!(
            broken.is_empty(),
            "authored style paths must resolve to blocks:\n{}",
            broken.join("\n")
        );
    }

    /// THE LADDER-PAIRING GATE (styling S2): a `size_class` button inside a ROW
    /// flows horizontally, so its measured rung height only lands when the row
    /// aligns non-stretch — a stretch row overrides every child's cross extent
    /// and the rung would be a silent no-op. Vertical flows (cell/panel/list/
    /// stack/popup_panel) consume the height as the MAIN extent and need nothing.
    #[test]
    fn every_ladder_button_in_a_row_sits_in_an_aligned_flow() {
        fn walk(
            node: &serde_json::Value,
            parent: Option<&serde_json::Value>,
            id: &str,
            broken: &mut Vec<String>,
        ) {
            let is_ladder_button = node.get("component").and_then(|c| c.as_str()) == Some("button")
                && node.get("size_class").is_some()
                && node.get("height").is_none();
            if is_ladder_button {
                if let Some(p) = parent {
                    let p_kind = p.get("component").and_then(|c| c.as_str()).unwrap_or("");
                    let p_align = p.get("align").and_then(|a| a.as_str()).unwrap_or("stretch");
                    if p_kind == "row" && p_align == "stretch" {
                        let bid = node.get("id").and_then(|i| i.as_str()).unwrap_or("<anon>");
                        broken.push(format!(
                            "{id}: button '{bid}' has a size_class inside a STRETCH row — the rung is a no-op; align the row (center/start/end)"
                        ));
                    }
                }
            }
            if let Some(kids) = node.get("children").and_then(|c| c.as_array()) {
                for kid in kids {
                    walk(kid, Some(node), id, broken);
                }
            }
        }

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/sensorium/scenes");
        let mut broken = Vec::new();
        for folder in [dir.clone(), dir.join("shared")] {
            for entry in std::fs::read_dir(&folder).expect("scenes folder reads") {
                let p = entry.expect("dir entry").path();
                if !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".scene.json"))
                {
                    continue;
                }
                let id = p.file_name().expect("name").to_string_lossy().to_string();
                let doc: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&p).expect("scene reads"))
                        .unwrap_or_else(|e| panic!("{id} parses: {e}"));
                if let Some(tree) = doc.get("tree") {
                    walk(tree, None, &id, &mut broken);
                }
            }
        }
        assert!(
            broken.is_empty(),
            "ladder buttons need an aligned row:\n{}",
            broken.join("\n")
        );
    }

    /// PROOF the catalog is NOT hardcoded — the full pipeline, walked end to end
    /// with the REAL shipped data, read-only (five-line architecture):
    ///
    /// 1. `ui_theme.json` = COLORS ONLY (no modal block); the modal layout details
    ///    live in THIS SCENE'S OWN file (`componentcatalog.scene.json` `styles`);
    /// 2. the bench's real loader path (`load_shared_styles(def.styles)`)
    ///    resolves `modal.buttons.variants.primary` from the scene's blocks;
    /// 3. the resolved fill equals the theme file's `theme.tokens.sap_base` — the
    ///    COLOUR comes from the one palette (the scene points, the theme defines);
    /// 4. `run_ui` over the REAL catalog tree emits a Panel draw command carrying
    ///    exactly that fill on the active nav bookmark — resolved live, per frame,
    ///    never baked into the bench.
    #[test]
    fn the_nav_rail_draws_rust_owned_modal_chrome_not_hardcoded_bytes() {
        // 1 — provenance: theme = colors only; the scene file owns its layout blocks.
        let theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(flicker::ui::shared_theme_path()).expect("theme reads"),
        )
        .expect("theme parses");
        assert!(
            theme.get("modal").is_none(),
            "ui_theme.json carries colors, nothing else"
        );
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        assert!(
            def.styles.as_ref().and_then(|st| st.get("modal")).is_some(),
            "this scene's OWN file carries the modal layout details it uses"
        );

        // 2 + 3 — the live loader resolves the scene's blocks against the file palette.
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let fill = styles["modal"]["buttons"]["variants"]["primary"]["fill_top"]
            .as_array()
            .expect("primary fill_top resolved to an rgba array (style satellite merge)")
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<f32>>();
        let raw_theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(flicker::ui::shared_theme_path()).expect("theme file reads"),
        )
        .expect("theme file parses");
        let sap_base = raw_theme["theme"]["tokens"]["sap_base"]
            .as_array()
            .expect("$sap_base lives in the FILE palette")
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<f32>>();
        assert_eq!(
            fill, sap_base,
            "structure from Rust, ink from the one palette"
        );

        // 4 — the REAL tree draws it: the active bookmark's slab carries the fill.
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        let mut model = ValueMap::new();
        model.set("nav_sty_0", NAV_ACTIVE_STYLE.to_string());
        let snap = UiInput {
            mouse: flicker::render::Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: flicker::render::Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut UiState::default());
        let want = [fill[0], fill[1], fill[2], fill[3]];
        assert!(
            frame.commands.iter().any(|c| matches!(
                c,
                HudCommand::Panel { color, .. } if color.iter().zip(want.iter()).all(|(a, b)| (a - b).abs() < 1e-6)
            )),
            "the walked catalog tree draws the Rust-owned primary slab fill {want:?}"
        );
    }

    /// **Every pane group has a clear panel-navigation layer** (nav-tier contract
    /// 1B5F6BB8, flattened per 1A292918): a `tab_group` with interior controls must have
    /// exactly ONE actionless CONTAINER (with a non-zero authored ordinal) whose id equals
    /// the group, so a panel stop is a real container and never a bare leaf. The flatten
    /// lets BOTH the stick and the d-pad move between those stops; `Confirm` still scopes
    /// into a container's interior. This is the "ambiguous panel navigation is a violation"
    /// rule as a gate — `cat_content` was a container-less group (nav deposited the cursor
    /// on a leaf inside it), which this forbids.
    #[test]
    fn every_pane_group_has_a_clear_container() {
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        // Every node's (id, tab_group, nav_ordinal, has_action) — membership AND
        // the container candidates the ownership rule derives (nested panes,
        // Aaron 2026-08-15: a container is a node other nodes CLAIM as their
        // `tab_group`; it carries NO self-membership marker).
        fn collect(n: &UiNode, out: &mut Vec<(String, String, u32, bool)>) {
            if !n.id.is_empty() {
                out.push((
                    n.id.clone(),
                    n.tab_group.clone(),
                    n.nav_ordinal,
                    n.action.is_some(),
                ));
            }
            for c in &n.children {
                collect(c, out);
            }
        }
        let mut nodes = Vec::new();
        collect(&tree, &mut nodes);
        let groups: std::collections::BTreeSet<&str> = nodes
            .iter()
            .filter(|(_, g, _, _)| !g.is_empty())
            .map(|(_, g, _, _)| g.as_str())
            .collect();
        assert!(
            groups.contains("cat_nav") && groups.contains("cat_content"),
            "both panes present"
        );
        for g in groups {
            // OWNERSHIP: exactly one actionless node whose id IS the group — the
            // container Confirm enters. An unclaimed group would strand its
            // members off the pad (the fail-to-nothing class).
            let containers: Vec<_> = nodes
                .iter()
                .filter(|(id, _, _, act)| id == g && !act)
                .collect();
            assert_eq!(
                containers.len(),
                1,
                "pane group `{g}` needs exactly one actionless container node with that id",
            );
            // The stick-stop order is AUTHORED: every container carries an
            // explicit non-zero ordinal (Aaron 2026-08-15 — never tree-implicit).
            assert!(
                containers[0].2 > 0,
                "container `{g}` must author its stick-stop `nav_ordinal`",
            );
            for (id, grp, ord, _) in &nodes {
                if grp == g {
                    assert!(
                        *ord > 0,
                        "member `{id}` of `{g}` must author its ring ordinal"
                    );
                }
            }
        }
    }

    /// The shipped tree names only kinds the engine knows (S10 vocabulary gate) and ships
    /// NO raw display literal — every label is a `$token`, in the tree or a bound value
    /// this source publishes (the strings gate + its model-publish twin).
    #[test]
    fn the_scene_ships_clean_kinds_and_no_raw_literals() {
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "unknown component kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "raw display literals in the tree (need a $token): {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(
            flags.is_empty(),
            "raw model-published copy (need a $token): {flags:?}"
        );
    }

    /// One bookmark + one card box per component, counts agreeing: [`CARD_COUNT`] and the
    /// tree's `nav_<i>` bookmarks + `card_<i>` boxes.
    #[test]
    fn one_bookmark_and_card_box_per_component() {
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE).expect("scene parses");
        let tree = def.tree.expect("the scene ships a tree");

        // Structure: every card has its bookmark and the ordinals chain 1..n in
        // authoring order (the drifted-ordinal class of bug fails here).
        let cards = card_ids(&tree);
        assert!(!cards.is_empty(), "the tray carries cards");
        for card in &cards {
            let nav = format!("\"nav_{}\"", &card["card_".len()..]);
            assert!(
                CATALOG_SCENE.contains(&nav),
                "card {card} has its bookmark {nav}"
            );
        }

        // COVERAGE — derived from the ROSTER, the single source of truth: every
        // engine component kind must appear somewhere in the tray (its own card,
        // or inside a composite's demo, e.g. `tabs` as the PTT's page rail). A
        // newly promoted kind fails HERE, by name, until its demo is authored —
        // the catalog can never silently lag the engine again.
        fn kinds_in(n: &UiNode, out: &mut std::collections::HashSet<String>) {
            out.insert(n.component.clone());
            for c in &n.children {
                kinds_in(c, out);
            }
        }
        let mut present = std::collections::HashSet::new();
        kinds_in(&tree, &mut present);
        let missing: Vec<&str> = flicker::ui::rust_component_kinds()
            .iter()
            .copied()
            .filter(|k| !present.contains(*k))
            .collect();
        assert!(
            missing.is_empty(),
            "engine kinds with no demo in the catalog: {missing:?}"
        );
    }

    /// **PRESENCE GATE for the `surface` card.** `surface` is a STRUCTURAL kind (the one
    /// kind at two depths — the root screen and every nested live surface), so the
    /// roster-coverage gate above, which derives its demand from the Rust component
    /// roster, no longer reaches it. The live-scene container's exerciser (card_26: a
    /// quad-layout nested surface the `ViewportFiller` fills, orbit on left-drag) is
    /// ratified catalog content (BDE5BFD0 / A8C9F02B), so its absence is a failure
    /// here, by name — a drift gate must cover the channel the drift travels (8634C200).
    #[test]
    fn the_surface_kind_keeps_its_live_catalog_card() {
        let json: serde_json::Value =
            serde_json::from_str(CATALOG_SCENE).expect("the catalog scene is JSON");
        fn find<'a>(v: &'a serde_json::Value, id: &str) -> Option<&'a serde_json::Value> {
            match v {
                serde_json::Value::Object(m) => {
                    if m.get("id").and_then(|i| i.as_str()) == Some(id) {
                        return Some(v);
                    }
                    m.values().find_map(|c| find(c, id))
                }
                serde_json::Value::Array(a) => a.iter().find_map(|c| find(c, id)),
                _ => None,
            }
        }
        let node =
            find(&json, "cat_surface").expect("the catalog carries the `cat_surface` card node");
        assert_eq!(
            node.get("component").and_then(|c| c.as_str()),
            Some("surface"),
            "the card's node is a `surface`"
        );
        assert!(
            node.get("layout").and_then(|l| l.as_str()).is_some(),
            "the card authors a `layout` so the filler tiles more than one pane"
        );
        assert!(
            CATALOG_SCENE.contains("\"nav_26\"") && CATALOG_SCENE.contains("\"card_26\""),
            "the surface card keeps its bookmark and its card box"
        );
    }
}
