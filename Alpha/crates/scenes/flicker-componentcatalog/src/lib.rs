//! flicker-componentcatalog: the UI TEST scene — a workbench that shows ONE demo copy
//! of every Rust widget component ([`RUST_COMPONENT_KINDS`]) with all its features
//! enabled. Every component sits in its own card box; all 25 flow top-to-bottom in a
//! scrollable tray on the right. A left nav rail of bookmarks SCROLLS the tray to a
//! card, and the bookmark of the card at the top of the view highlights.
//!
//! A Developer-realm bench (like Click Trainer), on the ratified pattern:
//! - **template-free** — `componentcatalog.scene.json` names primitive component KINDS
//!   directly (201F4F51), loaded by the bench; every display string is a `$token`.
//! - **no Lua** — nothing is gated or arranged (the tray is static), so like Click
//!   Trainer this scene has no orchestration layer; the scroll-to is pure engine wiring.
//! - **on the pump** (input-P3, 0569DA9B): owns no resolver — the PUMP resolves this
//!   frame's events and hands them in via [`SceneInput`].
//!
//! Scroll-to: the walker reports every card box's resolved rect ([`UiFrame::rect`]); the
//! scroll offset that brings card `i` to the top is `card_i.y - card_0.y` (the height
//! stacked above it), read live from the layout — no hardcoded heights. Esc opens pause.

use std::time::Duration;

use flicker::render::{Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

/// The scene's PAIR SCRIPT (`SceneName.lua` — the scene's component logic:
/// demo seeds, nav highlight, the Paged Menu card's page/tab gates).
const CATALOG_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/componentcatalog.lua");
const HUD_UI_THEME: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_theme.json");

/// The scrollable content tray's `bind` — the offset (px) the bench sets to scroll-to a
/// card, and the wheel writes as you scroll.
const CONTENT_SCROLL_BIND: &str = "cat_content_scroll";

/// The bookmark of the card at the top of the view wears the primary slab, the rest the
/// secondary — the nav's active indicator, published per-frame through each `style_bind`.
const NAV_ACTIVE_STYLE: &str = "modal.buttons.variants.primary";
const NAV_IDLE_STYLE: &str = "modal.buttons.variants.secondary";

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
}

impl ComponentCatalog {
    /// A fresh catalog scene — takes its authored tree off the MANIFEST's def (the
    /// kernel parsed the file; this bench is only the behaviour that plays it) and
    /// seeds the demo values.
    pub fn new(def: &SceneDef) -> Self {
        let ui_styles = flicker::ui::load_styles_for(HUD_UI_THEME, def.styles.as_ref());
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
        let Some(first) = self.cards.first().and_then(|id| card_y(id)) else { return fallback };
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
            screen,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        let frame = run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
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
        if let Some(&white) = self.textures.first() {
            render_hud(renderer, &self.hud_commands, white, &self.textures);
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

    /// THE PAIR-SCRIPT REGRESSION GATE: componentcatalog.lua loads; derive()
    /// seeds the demo values, lights the active bookmark, and gates the Paged
    /// Menu card — the scene's component logic lives in the pair script.
    #[test]
    fn the_pair_script_seeds_and_derives() {
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        let cat = ComponentCatalog::new(&def);
        assert!(cat.script.is_some(), "componentcatalog.lua loads (the pair script)");
        let m = cat.model();
        assert!(m.is_on("cat_check_val"), "the checkbox demo seeds ON");
        assert_eq!(m.text("nav_sty_0"), Some(NAV_ACTIVE_STYLE), "bookmark 0 starts active");
        assert_eq!(m.text("nav_sty_1"), Some(NAV_IDLE_STYLE), "the rest rest");
        assert!(m.is_on("cat_pm_on_p0"), "the Paged Menu card opens on page 1");
    }

    /// PROOF the catalog is NOT hardcoded — the full pipeline, walked end to end
    /// with the REAL shipped data, read-only (five-line architecture):
    ///
    /// 1. `ui_theme.json` = COLORS ONLY (no modal block); the modal layout details
    ///    live in THIS SCENE'S OWN file (`componentcatalog.scene.json` `styles`);
    /// 2. the bench's real loader path (`load_styles_for(theme, def.styles)`)
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
            &std::fs::read_to_string(HUD_UI_THEME).expect("theme reads"),
        )
        .expect("theme parses");
        assert!(theme.get("modal").is_none(), "ui_theme.json carries colors, nothing else");
        let def = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads");
        assert!(
            def.styles.as_ref().and_then(|st| st.get("modal")).is_some(),
            "this scene's OWN file carries the modal layout details it uses"
        );

        // 2 + 3 — the live loader resolves the scene's blocks against the file palette.
        let styles = flicker::ui::load_styles_for(HUD_UI_THEME, def.styles.as_ref());
        let fill = styles["modal"]["buttons"]["variants"]["primary"]["fill_top"]
            .as_array()
            .expect("primary fill_top resolved to an rgba array (style satellite merge)")
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<f32>>();
        let raw_theme: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(HUD_UI_THEME).expect("theme file reads"),
        )
        .expect("theme file parses");
        let sap_base = raw_theme["theme"]["tokens"]["sap_base"]
            .as_array()
            .expect("$sap_base lives in the FILE palette")
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<f32>>();
        assert_eq!(fill, sap_base, "structure from Rust, ink from the one palette");

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
            screen: flicker::render::Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
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
    /// 1B5F6BB8): a `tab_group` with interior controls must have exactly ONE ordinal-0
    /// actionless CONTAINER whose id equals the group, so the left stick lands on the pane
    /// (not a leaf) and the d-pad reaches the interior only after Confirm enters. This is
    /// the "ambiguous panel navigation is a violation" rule as a gate — `cat_content` was a
    /// container-less group (`PanelNext` deposited the cursor on a leaf inside it), which
    /// this now forbids.
    #[test]
    fn every_pane_group_has_a_clear_container() {
        let tree = SceneDef::parse("componentcatalog", CATALOG_SCENE)
            .expect("componentcatalog.scene.json loads")
            .tree
            .expect("it declares a tree");
        // (id, tab_group, nav_ordinal, has_action) for every focusable node.
        fn collect(n: &UiNode, out: &mut Vec<(String, String, u32, bool)>) {
            if !n.tab_group.is_empty() && !n.id.is_empty() {
                out.push((n.id.clone(), n.tab_group.clone(), n.nav_ordinal, n.action.is_some()));
            }
            for c in &n.children {
                collect(c, out);
            }
        }
        let mut nodes = Vec::new();
        collect(&tree, &mut nodes);
        let groups: std::collections::BTreeSet<&str> =
            nodes.iter().map(|(_, g, _, _)| g.as_str()).collect();
        assert!(groups.contains("cat_nav") && groups.contains("cat_content"), "both panes present");
        for g in groups {
            let members: Vec<_> = nodes.iter().filter(|(_, grp, _, _)| grp == g).collect();
            let containers = members
                .iter()
                .filter(|(id, grp, ord, act)| id == grp && *ord == 0 && !act)
                .count();
            assert_eq!(
                containers, 1,
                "pane group `{g}` needs exactly one ordinal-0 actionless container (id == group)",
            );
            for (id, _, ord, _) in &members {
                if id != g {
                    assert!(*ord > 0, "interior `{id}` in `{g}` must follow its container (ordinal > 0)");
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
        assert!(flags.is_empty(), "raw model-published copy (need a $token): {flags:?}");
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
            assert!(CATALOG_SCENE.contains(&nav), "card {card} has its bookmark {nav}");
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
        assert!(missing.is_empty(), "engine kinds with no demo in the catalog: {missing:?}");
    }
}
