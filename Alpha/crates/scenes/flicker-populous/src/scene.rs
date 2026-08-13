//! The bench: the DEFAULT PAGE — an empty frame, and the pause-menu signal.
//!
//! # The UI is DATA
//!
//! Following the Quartermaster (rule E5AFBBAB): this scene owns no HUD Lua and
//! composes nothing. [`crate::ui`] instances the `default_page` proto and calls
//! `expand` at its end — the single seam, so the scene and every gate walk the
//! same tree.
//!
//! # Five intents, one channel — and NOTHING the walker owns
//!
//! The screen's whole input declaration rides in data: `on_menu → pause_open`
//! from the `default_page` proto itself, plus the four rail intents
//! (`page_next` / `page_prev` / `tab_next` / `tab_prev`) through its optional
//! params. That is the entire list. Confirm, Cancel, `Nav*` and `Panel*` are
//! the WALKER's on every screen in Prism — it activates the focused control,
//! backs out, moves the cursor and cycles the panels — and this bench once
//! declared four of them, which statically killed activation on itself
//! (violation F1, 2026-08-09).
//!
//! A pad press, the bound key (`[` `]` pages, `,` `.` tabs) and a click are the
//! same event by the time the walker fires the result name; `update` folds them
//! into ONE dispatch. The rails then step THEMSELVES — each strip carries its
//! own `next_action`/`prev_action` — so `apply_results` only ever reads the
//! resulting numeric index, and the pause arm pushes the shell overlay.
//!
//! # Two tabs of one world
//!
//! The centre pane hands the bench's data core ([`crate::map::HexMap`], the hex
//! ledger's tiling) to ONE component — [`flicker_globe::GlobeWorld`] — which
//! owns the meshes, the offscreen target, the authored stage and the camera. The
//! scene names no colour, no radius and no inset: what the world LOOKS like is
//! authored in `stages.populous_globe` (the near-black under-shell, the inset
//! tile shell whose seams show it as outlines, and the shared reference frame
//! over both). No simulation, no layers, no data meaning: the sphere itself,
//! under the stage's authored light. The MAP tab flanks it with
//! the size dial and the world's numbers; the SEAMS tab (its view arrives
//! next) shows the SAME world between panes resting on their default text.
//! One viewport id, one render path — a tab swaps only its panes, and each
//! interior arrives as a layer that docks in, never into the chrome.
//!
//! **The world itself is SHARED state (Aaron's ruling).** One [`HexMap`], one
//! size, held once on this scene — a control that writes it changes it for
//! every tab, even though the control is shown on only one. The size dial is
//! currently the ONLY such control. A tab owns its VIEW — panes, captions,
//! someday its own overlays — never a copy of the world; the cross-tab gate
//! below is what keeps a future refactor from quietly forking it per tab.

use std::time::Duration;

use flicker::render::{FrameGraph, Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    load_styles, render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState,
    WalkerHandler,
};
use flicker_globe::GlobeWorld;
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

use crate::map::{HexMap, DEFAULT_FREQ, MAX_FREQ, MIN_FREQ};
use crate::ui;

/// The scene's `$token` styles live in the shared `ui_theme.json` — the ONE
/// global UI-element definition + Prism palette every prism-alpha scene reads.
/// NOT a per-scene copy: a second file would need its own `theme.tokens`, forking
/// the palette.
const HUD_UI_THEME: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_theme.json");

/// The bench's Lua ORCHESTRATION script — `arrange()` decides which tab-specific
/// components are shown from the two-way-bound page/tab selection. Embedded (the
/// same `include_str!` pattern the shell scenes use), so the bench ships with it.
const POPULOUS_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/populous.lua");

/// **Populous Bench** — the DEFAULT PAGE, and the surface the world will be
/// authored from.
pub struct PopulousBench {
    // ── navigation (indices into the roster, never a hardcoded count) ──
    sel_page: usize,
    sel_tab: usize,

    // ── the surface (the scene as DATA) ──
    /// The static component tree, parsed ONCE from `populous.scene.json`:
    /// every component declared, each tab's slice gated on a `shown_p0_t*` bind.
    /// Never rebuilt — the walker redraws it each frame against a fresh Model.
    tree: UiNode,
    /// The Lua ORCHESTRATION host (`populous.lua`): `arrange()` reads the bound
    /// page/tab and returns which slice is lit. Held for the bench's whole life.
    script: ScriptHost,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    /// The screen's declared signal->result intents, read off the static tree ONCE.
    ui_intents: UiIntents,
    hud_commands: Vec<HudCommand>,
    theme: Option<Theme>,
    /// The theme's engine textures in ID ORDER — white 0, muse 1, pad_glyphs 2.
    /// Registered whole so a component added to the page later (text, sprites,
    /// pad glyphs) draws without re-plumbing the atlas hand-off.
    textures: Vec<TextureHandle>,

    // ── input (input-P3, 0569DA9B): the scene owns NO resolver/bindings. The central
    //    PUMP resolves this frame's World-context events for the scene's `input_context()`
    //    (the World default); the walker consumes the edges and the globe reads continuous
    //    look/zoom from `signals.axis`. ──

    // ── the globe as an instrument ──
    map: HexMap,
    /// **The hex world, whole.** Meshes, offscreen target, authored stage and
    /// camera all live in the one component; this bench hands it the tiling and
    /// the rect the walker reserved, and nothing else. No colour, no radius, no
    /// mesh, no camera state, no device read.
    world: GlobeWorld,
}

impl PopulousBench {
    pub fn new(def: &SceneDef) -> Self {
        let map = HexMap::new(DEFAULT_FREQ);
        tracing::info!("populous: {} tiles at freq {}", map.len(), map.freq());
        let ui_styles = flicker::ui::load_styles_for(HUD_UI_THEME, def.styles.as_ref());
        // Open with the world FILLING the square viewport (Aaron: ~85%), not a
        // distant marble; the camera belongs to the viewport PANE, which is what
        // hands it the look signals.
        let world = GlobeWorld::new(ui::STAGE_SOURCE, &ui_styles, Some(0.85)).in_panel(ui::VIEW_PANE);

        // The scene is DATA: the kernel's manifest parsed the authored scene-def and
        // handed it here (this bench is the BEHAVIOUR that plays it). Its tree names
        // component KINDS directly (the template tier is gone — 201F4F51).
        let tree = def
            .tree
            .clone()
            .expect("populous.scene.json declares a tree");
        // The declared signal->result intents live in the static tree — read ONCE.
        let ui_intents = UiIntents::of(&tree);
        // The Lua ORCHESTRATION host, held for the bench's life so `arrange()` runs
        // each frame the selection may have moved.
        let script = ScriptHost::new(POPULOUS_SCRIPT, "populous.lua")
            .expect("populous.lua loads (it ships with the crate)");

        let mut bench = Self {
            sel_page: 0,
            sel_tab: 0,
            tree,
            script,
            ui_styles,
            ui_state: UiState::default(),
            ui_intents,
            hud_commands: Vec::new(),
            theme: None,
            textures: Vec::new(),
            map,
            world,
        };
        bench.publish_world();
        bench
    }

    /// Hand the world this map's tiling. WHAT it looks like — the two static
    /// shells' radii, insets and colours — is authored in `stages.populous_globe`
    /// and read by the world itself; the bench supplies only the geometry.
    fn publish_world(&mut self) {
        let Self { map, world, .. } = self;
        let shells = world.authored_shells(&map.grid().dirs, map.outlines());
        world.set_shells(shells);
    }

    /// The map in the viewport.
    pub fn map(&self) -> &HexMap {
        &self.map
    }

    /// The scene's static component tree — a clone for a gate or a test to walk. It is
    /// authored DATA now (`populous.scene.json`), parsed ONCE in `new`, so
    /// this is no longer per-(page, tab): which slice shows is decided by `arrange()`'s
    /// visibility binds, not by rebuilding structure.
    pub fn build_tree(&self) -> UiNode {
        self.tree.clone()
    }

    /// Which page and tab the rails are on — the roster indices, for tests.
    pub fn selection(&self) -> (usize, usize) {
        (self.sel_page, self.sel_tab)
    }

    /// Which node holds the walker's ONE focus — for tests. The panel cursor,
    /// the d-pad and the pointer all write this same id; the scene never does.
    pub fn focused(&self) -> Option<&str> {
        self.ui_state.focused()
    }

    /// Stand the walker's cursor on `id` — the test-side stand-in for a left-stick
    /// press or a click, so a gate can exercise the focus-gated channels without
    /// building a whole input frame. **Not a scene path**: nothing in `update`
    /// writes focus, which is the point.
    #[cfg(test)]
    fn focus_for_test(&mut self, id: &str) {
        self.ui_state.request_focus(id);
    }

    /// **What the components read.** The rail selections as NUMBERS (an index is
    /// a number end to end — rule 1B64FF03), whether the selected page has tabs
    /// at all (a condition on the ROSTER, so a future page with none collapses
    /// the tab rail by itself), the world's COMMITTED size, and the three stat
    /// readouts already formatted.
    ///
    /// Formatting happens HERE and rides a bind, never in the tree: a node
    /// carries a `$token` caption or a bind name, never a composed string. The
    /// dial publishes the map's own frequency — the control owns its display
    /// while a gesture is in flight (rule 3A04B4CE), so the scene has nothing
    /// live to publish.
    fn model(&self) -> ValueMap {
        let mut m = ValueMap::default();
        m.set(ui::PAGE_BIND, self.sel_page as f64);
        m.set(ui::TAB_BIND, self.sel_tab as f64);
        m.set(ui::TABS_SHOWN, !ui::page(self.sel_page).tabs.is_empty());
        m.set(ui::FREQ_BIND, f64::from(self.map.freq()));
        m.set(ui::HEXES_BIND, group_thousands(self.map.len() as u64));
        m.set(
            ui::DIAMETER_BIND,
            group_thousands(crate::map::diameter_mi(self.map.freq()).round() as u64),
        );
        m.set(ui::TILE_BIND, format!("{:.2}", crate::map::TILE_MI));
        m
    }

    /// Rebuild the ONE shared world at a new size, clamped by the offered range.
    /// A no-op at the current size.
    fn resize(&mut self, freq: u32) {
        let freq = freq.clamp(MIN_FREQ, MAX_FREQ);
        if freq == self.map.freq() {
            return;
        }
        self.map = HexMap::new(freq);
        self.publish_world();
        tracing::info!("populous: {} tiles at freq {}", self.map.len(), self.map.freq());
    }

    /// **The one dispatcher.** A click and a pad press arrive here identically,
    /// because both were folded into `results` before it was called.
    ///
    /// Four arms. Three of them read a value a CONTROL wrote — two rail indices
    /// and the dial's committed size — and the fourth is an action name a button
    /// fired. The rails step themselves (each strip owns its
    /// `next_action`/`prev_action`), the dial commits on release and owns its
    /// own display in between (rules B694F6B1 + 3A04B4CE), the panel cursor is
    /// the walker's, and activation arrives on the walker's one drain — so there
    /// is no stepper, no pane cursor, no enter/exit state and no live size copy
    /// left in this bench.
    pub fn apply_results(&mut self, results: &ValueMap) {
        let pages = ui::PAGES.len();
        // The strip writes its bind with the selected entry's roster index —
        // from a click on a cell, or from its own ±1 step. The bind also ECHOES
        // the resting value every frame (the echo contract), so only a CHANGED
        // value moves anything; in particular the page echo must not re-fire the
        // page arm, or its tab reset would pin the tab rail to 0 forever.
        if let Some(v) = results.number(ui::PAGE_BIND) {
            let want = (v.round().max(0.0) as usize).min(pages.saturating_sub(1));
            if want != self.sel_page {
                self.sel_page = want;
                self.sel_tab = 0; // a new page's tabs are its own
            }
        }
        if let Some(v) = results.number(ui::TAB_BIND) {
            let tabs = ui::page(self.sel_page).tabs.len();
            let want = (v.round().max(0.0) as usize).min(tabs.saturating_sub(1));
            if want != self.sel_tab {
                self.sel_tab = want;
            }
        }
        // The dial's ONE write: the number it committed on release (or on a pad
        // step). `resize` is a no-op at the current size, so the every-frame
        // echo of the resting value rebuilds nothing.
        if let Some(v) = results.number(ui::FREQ_BIND) {
            self.resize(v.round() as u32);
        }
        // The seams button's action resolves to an ARM — a loud one. The
        // feature is not built; an authored name that failed to NOTHING is the
        // difference between authorable and not (rule 4BB12A75).
        if results.is_on(ui::SEAMS_ACTION) {
            tracing::warn!("seams randomize is not built yet");
        }
    }
}

/// `92162` → `92,162` — thousands separators for the stat readouts. Formatting
/// lives in the scene and rides a Model bind (docs/ui-authoring: pre-format in
/// Rust, publish on a bind); a node never carries a composed number.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Scene for PopulousBench {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
        let theme = Theme::build(renderer);
        // The same texture list feeds the script host (names → ids) and
        // `render_hud` (ids → handles); registering one without the other is how
        // a sprite resolves to a name that draws nothing.
        let entries = theme.lua_textures();
        self.textures = entries.iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.world.free(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        if let Some((_map, look, _gp)) = flicker_shell::take_pending_input() {
            // The pump owns the key rebind now (S1c, non-draining); this scene keeps only
            // the player's LOOK controls (sensitivity + invert) for its globe camera — they
            // used to be discarded here while the globe turned at a private rate. The pad
            // deadzone rides the pump's `signals.axis`, so the gamepad is no longer ours.
            self.world.set_controls(look);
        }

        let screen = renderer.size();
        // The scene is DATA: walk the STATIC tree (built once in `new`). `arrange()`
        // reads the two-way-bound page/tab from the model and returns which slice is
        // lit; fold those visibility binds in so the walker draws the right one.
        // `ui_intents` was read off the static tree once, in `new`.
        let mut model = self.model();
        if let Err(e) = self.script.set_model(&model) {
            tracing::error!("populous: publishing the model to the script failed: {e}");
        }
        match self.script.arrange() {
            Ok(Some(arrangement)) => model.extend(arrangement.to_model()),
            Ok(None) => {}
            Err(e) => tracing::error!("populous: arrange() failed: {e}"),
        }
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen,
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
        };
        let frame = run_ui(&self.tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        let over_hud = frame.results.is_on("hud_hit");
        // The walker RESERVES the viewport's rect and never fills it (it runs
        // late; offscreen passes must run first) — hand it to the world here,
        // which draws into it at the top of `render`.
        self.world.place(frame.rtt_rect(ui::VIEW_SLOT));
        self.hud_commands = frame.commands;

        // ── The input seam (input-P3, 0569DA9B): the PUMP resolved this frame's
        // World-context events — the scene owns no Resolver. Dispatch `signals.events`
        // through the [walker, world] chain via `signals.route`; the walker layer consumes
        // the screen's declared `on_menu` intent (so the pause never reads a raw key), and
        // the globe below it takes what is left of the look/zoom edges while its panel
        // holds the cursor. The runner reconciles the route's context requests after
        // `update`; the walker writes focus directly during dispatch, so there is nothing
        // to apply here. ──

        // UNCONDITIONAL: the walker owns the focus graph on every frame, so the
        // left stick can reach every panel and the d-pad can reach every control
        // inside the focused one. A scene that switched navigability on and off
        // would be deciding what a signal means, which is the walker's job.
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&self.tree, &model)
            .with_intents(&self.ui_intents);
        {
            // The world sits BELOW the walker: navigation is decided first, and
            // what is left of the look/zoom signals belongs to the globe while
            // its panel holds the cursor.
            let mut chain: [&mut dyn InputHandler; 2] = [&mut walker, &mut self.world];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }

        // Fold the fired intent names in beside the click results, so both
        // channels reach the ONE dispatcher identically.
        let mut results = frame.results;
        for name in walker.take_fired() {
            results.set(name, true);
        }
        self.apply_results(&results);

        // One camera line, and it is the WORLD's: the look and zoom come from the PUMP's
        // continuous queries (`signals.axis`, never a device), and the globe answers them
        // only while its own panel holds the walker's cursor, latching the pointer drag
        // inside its own rect. The six Look/Zoom signals stay the camera's (`look_from`).
        let dtf = dt.as_secs_f32();
        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        // The globe answers look/zoom only while its pane is ENTERED (0EFF5464): merely
        // highlighting the viewport pane no longer feeds the camera — Confirm locks into
        // it, Cancel backs out. `entered` is the walker's; the scene reads it, never a
        // second focus system (F2).
        let look_gate = if self.ui_state.entered() { self.ui_state.focused() } else { None };
        self.world.update(dtf, input, look, look_gate);

        // Menu opens the shell's pause overlay — quit, settings, back to the
        // menu. The screen DECLARED `on_menu`; the arm lives here rather than in
        // `apply_results` because it returns a Transition.
        if results.is_on("pause_open") {
            if let Some(theme) = self.theme {
                // The scene owns no bindings; take the World map from the shared profile
                // for the pause overlay (it binds Menu, so Esc resumes).
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
        // The globe goes into the rect the walker reserved — FIRST.
        // `FrameGraph::execute` resets the shared per-frame draw queues, so an
        // offscreen pass declared after a main-frame draw would throw that
        // draw away.
        {
            let mut fg = FrameGraph::new();
            let layer = renderer.layer();
            self.world.render(renderer, &mut fg, layer);
            fg.execute(renderer);
        }

        if let Some(&white) = self.textures.first() {
            render_hud(renderer, &self.hud_commands, white, &self.textures);
        }
    }
}

/// Build the bench as a boxed `Scene` — the CLIENT BEHAVIOUR the roster registers;
/// the manifest resolves `populous.scene.json` and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(PopulousBench::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::script::{UiNode, Value};
    // Test-only now: the tests synthesize events with a real resolver to drive the walker
    // chain end-to-end; the scene itself owns none of this any more (input-P3).
    use flicker_input_core::{
        ActionSignal, ContextualBindings, EventKind, Fired, InputContext, Resolver,
    };
    use flicker_input_router::{InputEvent, RouteCtx};

    fn flatten(n: &UiNode) -> Vec<&UiNode> {
        let mut out = vec![n];
        let mut i = 0;
        while i < out.len() {
            let node = out[i];
            for c in &node.children {
                out.push(c);
            }
            i += 1;
        }
        out
    }

    /// **The whole surface resolves.** Every component kind is registered and no raw
    /// English reaches a draw — the two drift gates every migrated bench carries (the
    /// template tier that once needed a third gate is gone — 201F4F51).
    #[test]
    fn the_screen_names_known_kinds_with_no_raw_strings() {
        let tree = test_bench().build_tree();
        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(unknown.is_empty(), "{unknown:?}");
        let raw = flicker::ui::raw_display_literals(&tree);
        assert!(raw.is_empty(), "{raw:?}");
    }

    /// **THE BENCH IS EXACTLY THE CATALOG, and nothing else.** Aaron's list —
    /// Frame, PTT, Slider, Button, Multi-View, RTT Panel, UI Panel, localized
    /// strings — is the whole vocabulary this screen may draw, and this gate
    /// asserts the inverse too: every component kind in the expanded tree is on
    /// the list, on BOTH tabs. A stray `cell` column, a hand-built readout or a
    /// borrowed bench widget fails here, which is what makes "the entire
    /// Populous Bench should be that" a build result rather than a wish.
    ///
    /// The list, and why each name is on it: `screen` is the root the
    /// `default_page` proto emits; `grid` / `cell` / `row` / `stack` /
    /// `rune_corners` are what the FRAME (Aaron's own "Frame (9-grid)" — the
    /// frame builder emits a three-column `grid` of cells) and the PTT are MADE
    /// of, structure rather than content; `tabs` + `pill_toggle` + `button` are
    /// the PTT's two rails and its four glyph hints; `panel` is the UI Panel and
    /// the RTT Panel (one component, two protos); `rtt` is the viewport;
    /// `slider` is the size dial; `text` and `option` carry the localized
    /// strings.
    #[test]
    fn the_bench_is_exactly_the_catalog_and_nothing_else() {
        /// Aaron's catalog, expanded to the component kinds it is built from.
        const CATALOG: &[&str] = &[
            "screen", "cell", "row", "stack",
            "rune_corners", // the page frame's corner runes (frame inlined as stack + overlay)
            "paged_menu",   // the PTT — a native Component (rails/hints/rule drawn by Rust)
            "tabs", "pill_toggle", // the PTT's authored page + tab rails
            "panel",  // UI Panel and RTT Panel
            "rtt",    // the hex world's viewport
            "slider", // the size dial
            "button", // the seams action
            "text", "option", // localized strings
        ];
        for tab in [0.0, 1.0] {
            let mut bench = test_bench();
            let mut r = ValueMap::default();
            r.set(ui::TAB_BIND, tab);
            bench.apply_results(&r);
            let tree = bench.build_tree();
            let mut kinds: Vec<&str> =
                flatten(&tree).iter().map(|n| n.component.as_str()).collect();
            kinds.sort_unstable();
            kinds.dedup();
            for k in &kinds {
                assert!(CATALOG.contains(k), "tab {tab}: `{k}` is not in the catalog");
            }
        }
        // …and the catalog is not aspirational: on the MAP tab every interactive
        // member of it is actually PRESENT.
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        let count = |kind: &str| all.iter().filter(|n| n.component == kind).count();
        assert_eq!(count("paged_menu"), 1, "the PTT is ONE Component");
        assert_eq!(count("tabs"), 1, "the PTT's page rail");
        assert_eq!(count("pill_toggle"), 1, "the PTT's tab rail");
        assert_eq!(count("panel"), 3, "two UI Panels and one RTT Panel");
        assert_eq!(count("rtt"), 1, "one viewport");
        assert_eq!(count("slider"), 1, "the size dial");
        // The rail hints (lt/rt/lb/rb) and the rule are now drawn BY the `paged_menu`
        // Component, not authored tree nodes — so NOTHING on the surface wears a glyph.
        // The hint→step behaviour is gated in flicker-widgets (`draw_paged_menu` /
        // `hit_paged_menu`); here we assert only the stray-node inverse.
        assert!(
            !all.iter().any(|n| n.props.contains_key("glyph")),
            "the PTT hints are component-internal, not authored glyph buttons"
        );
        // The three panes, each the ONE panel component, and the viewport wired
        // to its authored stage (the default light source).
        for id in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE] {
            let p = all.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{id} is placed"));
            assert_eq!(p.component, "panel", "{id} is the ONE panel component");
        }
        let view = all.iter().find(|n| n.id == ui::VIEW_SLOT).expect("the viewport is placed");
        assert_eq!(view.component, "rtt", "the centre pane is the viewport");
        assert_eq!(
            view.props.get("source"),
            Some(&Value::Text(ui::STAGE_SOURCE.to_string())),
            "the viewport names the stage that lights it"
        );
        // EVERY word on the surface is a `$token`, and NOT ONE number is
        // composed into a node: the three stat values arrive on Model binds
        // (`text_bind`), which is why there is no literal left to check.
        let texts: Vec<&&UiNode> = all.iter().filter(|n| n.component == "text").collect();
        for t in &texts {
            if let Some(Value::Text(s)) = t.props.get("text") {
                assert!(s.starts_with('$'), "`{s}` must be a stringtable token");
            }
        }
        let mut bound: Vec<&str> = texts
            .iter()
            .filter_map(|n| match n.props.get("text_bind") {
                Some(Value::Text(t)) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        bound.sort_unstable();
        assert_eq!(
            bound,
            [ui::DIAMETER_BIND, ui::HEXES_BIND, ui::TILE_BIND],
            "every readout is a bind the scene pre-formats, never a node's own string"
        );
        // The size dial: bound over the offered range, focusable (the pad
        // channel's gate), and wearing the label the proto carries.
        let dial = all.iter().find(|n| n.component == "slider").expect("the dial");
        assert_eq!(dial.props.get("vertical"), Some(&Value::Bool(true)), "upright");
        assert_eq!(dial.bind.as_deref(), Some(ui::FREQ_BIND));
        assert_eq!(dial.props.get("min"), Some(&Value::Number(48.0)));
        assert_eq!(dial.props.get("max"), Some(&Value::Number(120.0)));
        assert_eq!(dial.props.get("label"), Some(&Value::Text("$pop_size".into())));
        assert_eq!(dial.props.get("step"), Some(&Value::Number(1.0)));
        assert_eq!(dial.props.get("step_coarse"), Some(&Value::Number(10.0)));
        assert!(!dial.tab_group.is_empty(), "walker-focusable — the pad channel's gate");
        // NOT ONE style string reaches a node from this crate: every skin the
        // surface wears is named by a proto, in data.
        for n in &all {
            if let Some(Value::Text(s)) = n.props.get("style") {
                assert!(
                    !s.contains("sablework") && !s.contains("assetpipeline"),
                    "`{s}` is another bench's palette"
                );
            }
        }
        // The right pane displays and never interacts: its whole subtree is the
        // pane plus the readout rows — focusing it changes no control, because
        // there are none to change.
        fn subtree<'a>(n: &'a UiNode, out: &mut Vec<&'a UiNode>) {
            out.push(n);
            for c in &n.children {
                subtree(c, out);
            }
        }
        let right = all.iter().find(|n| n.id == ui::RIGHT_PANE).expect("right pane");
        let mut nodes = Vec::new();
        subtree(right, &mut nodes);
        assert!(
            nodes.iter().all(|n| matches!(n.component.as_str(), "panel" | "cell" | "row" | "text")),
            "the stats pane is display-only (its slice gate is a plain `cell`)"
        );
        // ONE set of corner runes: the page chrome's. The content frame turned
        // its own off (`runes = false`) — the corners never stack.
        assert_eq!(count("rune_corners"), 1, "the inner frame's runes are off");
    }

    /// **The panes are PANELS, and the walker owns which one has the cursor.**
    /// Three `panel` nodes, each its own `tab_group`, each at the group's lowest
    /// ordinal — that is the entire pane model this bench carries. The wrapping,
    /// the rim and the enter/exit that used to live here are the walker's and the
    /// `panel` component's (walker gate: `the_left_stick_cycles_panels_and_wraps`;
    /// component gate: `a_focused_panel_draws_its_own_rim`).
    #[test]
    fn the_panes_are_panels_the_walker_can_cycle() {
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        let panes: Vec<&&UiNode> = all.iter().filter(|n| n.component == "panel").collect();
        assert_eq!(panes.len(), 3, "three panes, all the ONE panel component");
        for (i, id) in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE].iter().enumerate() {
            let p = panes[i];
            assert_eq!(&p.id, id, "panes keep authoring order");
            assert_eq!(&p.tab_group, id, "each pane is its own focus group");
            assert_eq!(p.nav_ordinal, 0, "the pane is the group's entry point");
            assert!(
                !p.props.keys().any(|k| k == "focused" || k.ends_with("_style")),
                "{id}: the scene passes no rim and no focus flag"
            );
        }
        // …and the ONE interactive control inside a pane sits ABOVE the pane in
        // its own group, so the panel cursor lands on the pane and the d-pad
        // reaches the control. This asserts the COMPOSED result only — what the
        // dial itself is (its range, its two pad step sizes, its skin, its own
        // default ordinal) is the proto's contract and is gated beside the proto:
        // flicker-widgets `the_size_dial_proto_carries_the_whole_control_contract`.
        let dial = all.iter().find(|n| n.component == "slider").expect("the dial");
        assert_eq!(dial.tab_group, ui::LEFT_PANE);
        assert!(dial.nav_ordinal > 0, "the control follows its panel in the group");
    }

    /// **Both rails carry the roster.** One entry per page and one per tab of
    /// the selected page — read from [`ui::PAGES`], never a written-down count.
    #[test]
    fn the_rails_are_built_from_the_roster() {
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        let rail = |id: &str| {
            all.iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("{id} is placed"))
                .children
                .len()
        };
        assert_eq!(rail("paged_pages"), ui::PAGES.len(), "one entry per page");
        assert_eq!(rail("paged_tabs"), ui::PAGES[0].tabs.len(), "one entry per tab");
    }

    /// **The rails step THEMSELVES, and the scene only reads the result.** Each
    /// strip authors the `next_action` / `prev_action` naming the very result the
    /// neighbouring hint button fires, so a click, a shoulder press and a pad
    /// Confirm all converge on one numeric index write — which is the only thing
    /// this bench dispatches. (The stepping itself, over the real `paged_menu`,
    /// is gated in flicker-widgets: `a_rail_hint_press_steps_the_strip_by_one_and_clamps`.)
    #[test]
    fn the_rails_own_their_stepping_and_the_scene_reads_the_index() {
        let mut bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        for (id, next, prev) in [
            ("paged_pages", "page_next", "page_prev"),
            ("paged_tabs", "tab_next", "tab_prev"),
        ] {
            let rail = all.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{id} is placed"));
            assert_eq!(rail.props.get("next_action"), Some(&Value::Text(next.into())));
            assert_eq!(rail.props.get("prev_action"), Some(&Value::Text(prev.into())));
        }
        // The scene has NO stepper: a fired step name it happens to see moves
        // nothing — only the index the strip wrote does.
        let mut named = ValueMap::default();
        named.set("tab_next", true);
        bench.apply_results(&named);
        assert_eq!(bench.selection(), (0, 0), "the scene owns no stepping");
        let mut wrote = ValueMap::default();
        wrote.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&wrote);
        assert_eq!(bench.selection(), (0, 1), "the strip's index is what moves the tab");
    }

    /// **Both tab slices live in ONE static tree, over the SAME panes, gated apart.**
    /// A tab no longer rebuilds structure — `build_tree` is identical whatever the
    /// selection; the MAP slice (the size dial + stats) and the SEAMS slice (the
    /// randomize button + the pane's `$ui_pane_empty` placeholder) coexist, each gated
    /// on its `shown_p0_t*` key over the SAME viewport + three panes. WHICH slice shows
    /// is `arrange()`'s job (`arrange_lights_the_selected_tabs_slice`), never the
    /// tree's — the point of the shared-panes arrangement.
    #[test]
    fn both_tab_slices_share_the_panes_and_are_gated_apart() {
        let bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        assert!(flicker::ui::unknown_kinds(&tree).is_empty());
        assert!(flicker::ui::raw_display_literals(&tree).is_empty());

        // The one viewport, wired to the one stage — shared by both tabs, ungated.
        let view = all.iter().find(|n| n.id == ui::VIEW_SLOT).expect("the viewport is placed");
        assert_eq!(view.component, "rtt", "the centre pane is the viewport");
        assert_eq!(
            view.props.get("source"),
            Some(&Value::Text(ui::STAGE_SOURCE.to_string())),
            "the one authored stage lights it"
        );

        // The one three-pane arrangement, all the `panel` component, left/view/right.
        let panes: Vec<&&UiNode> = all.iter().filter(|n| n.component == "panel").collect();
        assert_eq!(panes.len(), 3, "one three-pane arrangement, shared by both tabs");
        for (i, id) in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE].iter().enumerate() {
            assert_eq!(&panes[i].id, id, "the panes keep left/view/right order");
        }

        // BOTH slices are DECLARED in the one tree — the tab lights one, never adds or
        // removes structure. The map slice's dial and the seams slice's button coexist.
        assert!(
            all.iter().any(|n| n.component == "slider" && n.bind.as_deref() == Some(ui::FREQ_BIND)),
            "the map slice's size dial is declared"
        );
        assert!(
            all.iter().any(|n| n.action.as_deref() == Some(ui::SEAMS_ACTION)),
            "the seams slice's randomize button is declared"
        );
        let placeholders = all
            .iter()
            .filter(|n| matches!(n.props.get("text"), Some(Value::Text(t)) if t == "$ui_pane_empty"))
            .count();
        assert_eq!(placeholders, 1, "the seams pane declares one empty placeholder");

        // Both slices are gated, on DIFFERENT keys, so a selection lights exactly one.
        let gates: std::collections::HashSet<&str> =
            all.iter().filter_map(|n| n.visible_bind.as_deref()).collect();
        assert!(
            gates.contains("shown_p0_t0") && gates.contains("shown_p0_t1"),
            "the map and seams slices are gated apart"
        );
    }

    /// **The planet size is ONE world, shared by every tab** (Aaron's ruling).
    /// The dial is shown on the map tab alone, but its committed number writes
    /// the scene's one `HexMap` — so a size set there IS the size the seams tab
    /// renders, and the value the dial reads back on return. A tab owns its
    /// view, never a copy of the world; this gate is what makes forking it per
    /// tab a failing build instead of a quiet refactor.
    ///
    /// The dial's own GESTURE is not tested here and no longer can be: the
    /// bench holds no live size and no stick reader. Mid-gesture behaviour —
    /// the knob tracking the hand while `results` still report the resting
    /// value, and the single write on release — belongs to the walker and is
    /// gated there (rules B694F6B1 commit-on-release + 3A04B4CE local display
    /// ownership; `slider_drag_captures_and_commits_on_release`). This scene
    /// sees one number, once, and rebuilds.
    #[test]
    fn the_planet_size_is_one_world_shared_by_every_tab() {
        let mut bench = test_bench();
        let tab = |i: f64| {
            let mut r = ValueMap::default();
            r.set(ui::TAB_BIND, i);
            r
        };

        // The dial's COMMITTED number on the MAP tab rebuilds the one world...
        let mut write = ValueMap::default();
        write.set(ui::FREQ_BIND, 72.0);
        bench.apply_results(&write);
        assert_eq!(bench.map().freq(), 72, "the dial's committed number lands");

        // ...and the SEAMS tab renders that same world — same map, same size;
        // there is no per-tab copy to drift.
        bench.apply_results(&tab(1.0));
        assert_eq!(bench.selection(), (0, 1), "on the seams tab");
        assert_eq!(bench.map().freq(), 72, "the seams tab shows the resized world");

        // Returning, the dial reads the SHARED value back out of the model —
        // the size did not reset with the tab.
        bench.apply_results(&tab(0.0));
        assert_eq!(bench.selection(), (0, 0));
        assert_eq!(
            bench.model().number(ui::FREQ_BIND),
            Some(72.0),
            "the dial shows the world's one size, not a per-tab memory"
        );

        // The bind ECHOES its resting value every frame; `resize` is a no-op at
        // the current size, so the echo rebuilds nothing. An out-of-range number
        // clamps into the offered range rather than reaching `HexMap`.
        bench.apply_results(&write);
        assert_eq!(bench.map().freq(), 72, "the resting echo is not a rebuild");
        let mut wild = ValueMap::default();
        wild.set(ui::FREQ_BIND, 9_000.0);
        bench.apply_results(&wild);
        assert_eq!(bench.map().freq(), MAX_FREQ, "a wild number clamps into the range");
    }

    /// **A rail click jumps the selection directly** — the strip writes its
    /// bind with the clicked entry's index and the dispatcher consumes it. The
    /// every-frame echo is inert: only a CHANGED value moves anything, so the
    /// page echo can never re-fire the arm whose side effect resets the tab.
    #[test]
    fn a_rail_click_write_jumps_the_selection_and_echoes_are_inert() {
        let mut bench = test_bench();
        // A wild write clamps into the roster and lands — one click, no steps.
        let mut wild = ValueMap::default();
        wild.set(ui::TAB_BIND, 9.0);
        bench.apply_results(&wild);
        assert_eq!(bench.selection(), (0, 1), "clamped into the roster and applied");
        // The resting echo (both binds, current values) changes nothing.
        let mut echo = ValueMap::default();
        echo.set(ui::PAGE_BIND, 0.0);
        echo.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&echo);
        assert_eq!(bench.selection(), (0, 1), "echoes are inert");
        // A clamp-equal page write is NOT a page change — the tab survives.
        let mut pw = ValueMap::default();
        pw.set(ui::PAGE_BIND, 3.0);
        bench.apply_results(&pw);
        assert_eq!(bench.selection(), (0, 1), "no page change, no tab reset");
        // A real click on the first pill jumps back.
        let mut back = ValueMap::default();
        back.set(ui::TAB_BIND, 0.0);
        bench.apply_results(&back);
        assert_eq!(bench.selection(), (0, 0));
    }

    /// **A mouse click on the tab rail's second pill switches to the seams
    /// tab** — the full pointer contract through the REAL component: the strip
    /// under the real styles, the Lua hit verdict carrying the entry's numeric
    /// value, the bind write in `results`, and the dispatcher's jump. The
    /// numeric click path was dead once (string-only gate); this drives it end
    /// to end so it cannot go quiet again.
    #[test]
    fn a_click_on_the_tab_rail_switches_to_the_seams_tab() {
        use flicker::render::Vec2;

        let mut bench = test_bench();
        let tree = bench.build_tree();
        let model = bench.model();
        let styles = load_styles(HUD_UI_THEME);
        let mut state = UiState::default();
        let snap = |mouse: Vec2, clicked: bool, down: bool| UiInput {
            mouse,
            clicked,
            down,
            screen: Vec2::new(1600.0, 900.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };

        // Resolve the rail's rect, then click the centre of its SECOND pill.
        let idle =
            run_ui(&tree, &model, &styles, &snap(Vec2::ZERO, false, false), &mut state);
        let rail = idle.rect("paged_tabs").expect("the tab rail resolves");
        let click = Vec2::new(rail.pos.x + rail.size.x * 0.75, rail.pos.y + rail.size.y * 0.5);
        let f = run_ui(&tree, &model, &styles, &snap(click, true, true), &mut state);
        assert_eq!(
            f.results.number(ui::TAB_BIND),
            Some(1.0),
            "the pill's numeric value rides the bind write"
        );
        bench.apply_results(&f.results);
        assert_eq!(bench.selection(), (0, 1), "the click switched to the seams tab");
    }

    /// **Five intents declared, every one with an arm — and NOT ONE the walker
    /// owns.** `on_menu` rides in from the `default_page` proto; the four rail
    /// intents through its optional params. The inverse is the load-bearing half:
    /// Confirm, Cancel, `Nav*` and `Panel*` are the walker's on every screen in
    /// Prism, and this bench declared four of them until 2026-08-09 — which,
    /// because the walker consumes a declared intent and returns BEFORE the
    /// activation path, statically killed every button on the screen (F1).
    #[test]
    fn the_screen_declares_only_what_it_owns_and_every_one_has_an_arm() {
        let tree = test_bench().build_tree();
        let mut declared: Vec<(String, String)> = tree
            .props
            .iter()
            .filter(|(k, _)| k.starts_with("on_"))
            .filter_map(|(k, v)| match v {
                Value::Text(t) => Some((k.clone(), t.clone())),
                _ => None,
            })
            .collect();
        declared.sort();
        assert_eq!(
            declared,
            [
                ("on_menu", "pause_open"),
                ("on_page_next", "page_next"),
                ("on_page_prev", "page_prev"),
                ("on_tab_next", "tab_next"),
                ("on_tab_prev", "tab_prev"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string())),
            "the declaration is exactly the set the scene owns"
        );
        // THE INVERSE — the channel the F1 defect travelled. Not one walker-owned
        // signal may be named, at the root or anywhere beneath it.
        for signal in [
            ActionSignal::Confirm,
            ActionSignal::Cancel,
            ActionSignal::NavUp,
            ActionSignal::NavDown,
            ActionSignal::NavLeft,
            ActionSignal::NavRight,
            ActionSignal::PanelNext,
            ActionSignal::PanelPrev,
            ActionSignal::ChordBegin,
        ] {
            assert_eq!(
                UiIntents::of(&tree).result_for(signal),
                None,
                "{signal:?} is the walker's — declaring it kills it on this screen"
            );
        }
        assert!(
            !flatten(&tree).iter().any(|n| n
                .props
                .keys()
                .any(|k| matches!(
                    k.as_str(),
                    "on_confirm"
                        | "on_cancel"
                        | "on_nav_up"
                        | "on_nav_down"
                        | "on_nav_left"
                        | "on_nav_right"
                        | "on_panel_next"
                        | "on_panel_prev"
                        | "on_chord_begin"
                ))),
            "no node anywhere in the tree names a walker-owned signal"
        );
        assert_eq!(
            UiIntents::of(&tree).result_for(ActionSignal::Menu),
            Some("pause_open"),
            "the menu declaration names the signal the resolver actually fires"
        );
        for (_, name) in declared {
            if name == "pause_open" {
                continue; // handled in `update`, which returns a Transition
            }
            // Every rail intent is a STRIP's own step name — the control that
            // dispatches it, so a fired name can never fail to nothing.
            assert!(
                flatten(&tree).iter().any(|n| {
                    n.props.get("next_action") == Some(&Value::Text(name.clone()))
                        || n.props.get("prev_action") == Some(&Value::Text(name.clone()))
                }),
                "`{name}` is declared but no strip steps on it"
            );
        }
    }

    /// **The seams button's authored name reaches an ARM.** The feature is not
    /// built; what IS built is that the name resolves — the button fires it, the
    /// dispatcher answers it with a loud warn, and the size the world is at does
    /// not move underneath it. An authored name that failed to NOTHING is the
    /// difference between authorable and not (rule 4BB12A75).
    #[test]
    fn the_seams_button_fires_a_name_the_dispatcher_answers() {
        let mut bench = test_bench();
        let mut seams = ValueMap::default();
        seams.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&seams);

        // The button is in the tree, on the seams tab, firing exactly that name.
        let tree = bench.build_tree();
        let button = flatten(&tree)
            .into_iter()
            .find(|n| n.action.as_deref() == Some(ui::SEAMS_ACTION))
            .expect("the seams button fires the authored action");
        assert_eq!(button.component, "button", "a plain button, not a bench widget");
        assert_eq!(button.tab_group, ui::LEFT_PANE, "it lives in the left pane's focus group");

        // …and the dispatcher has an arm for it: it consumes the name without
        // disturbing the world (there is nothing behind it yet).
        let before = bench.map().freq();
        let mut fired = ValueMap::default();
        fired.set(ui::SEAMS_ACTION, true);
        bench.apply_results(&fired);
        assert_eq!(bench.map().freq(), before, "not built yet — and it changes nothing");
        assert_eq!(bench.selection(), (0, 1), "…and it is not a navigation either");
    }

    /// **The PTT rails fire results this screen dispatches.** The glyph hints are now
    /// drawn BY the `paged_menu` Component and their click→step is gated in flicker-widgets
    /// (`hit_paged_menu` fires the neighbouring rail's step name). At the SCENE level the
    /// load-bearing property survives: each rail's `next_action`/`prev_action` is one of the
    /// screen's declared intents — so a hint click, a pad Confirm and the shoulder signal all
    /// converge on the ONE result the rail steps on. A rail stepping a result nothing
    /// declares would be a dead control.
    #[test]
    fn the_ptt_rails_fire_results_this_screen_dispatches() {
        let tree = test_bench().build_tree();
        let declared: Vec<String> = tree
            .props
            .iter()
            .filter(|(k, _)| k.starts_with("on_"))
            .filter_map(|(_, v)| match v {
                Value::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let all = flatten(&tree);
        let mut steps: Vec<&str> = Vec::new();
        for n in &all {
            for k in ["next_action", "prev_action"] {
                if let Some(Value::Text(t)) = n.props.get(k) {
                    steps.push(t.as_str());
                }
            }
        }
        steps.sort_unstable();
        steps.dedup();
        assert_eq!(
            steps,
            ["page_next", "page_prev", "tab_next", "tab_prev"],
            "both rails carry their four step names"
        );
        for want in steps {
            assert!(
                declared.iter().any(|d| d == want),
                "`{want}` is a declared intent, so pad / click / shoulder converge on the rail's step"
            );
        }
    }

    /// **The viewport actually gets its pixels.** The real tree through the
    /// real resolver at a bench-sized screen: the reserved rect must exist, be
    /// SQUARE (the aspect lock), and fill a substantial share of the height —
    /// the assertion that catches a collapsed layout chain. Both halves of the
    /// squashed-band bug lived below this line: the inner frame flowing at its
    /// measured ~100px instead of growing, and an anchored square inside a
    /// flow cell measuring to nothing and vanishing. Tree gates saw both trees
    /// as fine; only the resolved RECT tells the truth.
    #[test]
    fn the_viewport_resolves_to_a_substantial_square() {
        use flicker::render::Vec2;

        let bench = test_bench();
        let tree = bench.build_tree();
        // Light the MAP slice (what arrange() publishes on tab 0) so the gated size
        // dial is placed — it is dark until the selection lights `shown_p0_t0`.
        let mut model = ValueMap::default();
        model.set("shown_p0_t0", true);
        let styles = load_styles(HUD_UI_THEME);
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            screen: Vec2::new(1600.0, 900.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let mut state = UiState::default();
        let frame = run_ui(&tree, &model, &styles, &snap, &mut state);
        let rect = frame
            .rtt_rect(ui::VIEW_SLOT)
            .expect("the viewport's rect is reserved by the walker");
        assert!(
            (rect.size.x - rect.size.y).abs() < 1.5,
            "the viewport is square: {}×{}",
            rect.size.x,
            rect.size.y
        );
        assert!(
            rect.size.y > 300.0,
            "the square fills a substantial share of a 900px screen, got {}",
            rect.size.y
        );
        // EVERY control a user must see and hit gets the same treatment — a
        // node can be perfect in the tree and still resolve to zero pixels
        // (the size dial shipped exactly that way once: present, bound,
        // unclickable). Presence AND extent, from the layout's own answer.
        for id in [ui::LEFT_PANE, ui::RIGHT_PANE] {
            let r = frame.rect(id).unwrap_or_else(|| panic!("{id} resolves"));
            assert!(r.size.x > 100.0 && r.size.y > 300.0, "{id} has real pixels");
        }
        let dial = frame.rect(ui::FREQ_BIND).expect("the size dial resolves");
        assert!(
            dial.size.y > 150.0,
            "the dial's track is tall enough to grab, got {}",
            dial.size.y
        );
        let left = frame.rect(ui::LEFT_PANE).unwrap();
        assert!(
            dial.pos.x >= left.pos.x && dial.pos.x + dial.size.x <= left.pos.x + left.size.x,
            "the dial sits inside the left pane"
        );
    }

    /// **A mouse click on the dial's track writes the bound value** — the full
    /// pointer contract through the REAL component: press inside the vertical
    /// track captures (top is max), and the release edge lands the one real
    /// write on `pop_freq`. This is the test for "I could not click on that":
    /// it drives the actual Lua slider, so a dial that draws but cannot be hit
    /// — or hits but writes nothing — fails here, not in Aaron's hands.
    #[test]
    fn a_click_on_the_dials_track_writes_the_bound_value() {
        use flicker::render::Vec2;

        let bench = test_bench();
        let tree = bench.build_tree();
        // Light the MAP slice so the gated dial is placed + hittable (tab 0).
        let mut model = bench.model();
        model.set("shown_p0_t0", true);
        let styles = load_styles(HUD_UI_THEME);
        let mut state = UiState::default();
        let snap = |mouse: Vec2, clicked: bool, down: bool| UiInput {
            mouse,
            clicked,
            down,
            screen: Vec2::new(1600.0, 900.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };

        // Find the dial, then press at the very BOTTOM of it — below-track
        // presses clamp to t = 0, so the landing value is EXACTLY the minimum,
        // independent of the label band's height above the track.
        let count_48 = |cmds: &[flicker::script::HudCommand]| {
            cmds.iter()
                .filter(|c| matches!(c, flicker::script::HudCommand::Text { text, .. } if text == "48"))
                .count()
        };
        let frame = run_ui(&tree, &model, &styles, &snap(Vec2::ZERO, false, false), &mut state);
        let dial = frame.rect(ui::FREQ_BIND).expect("the dial resolves");
        assert_eq!(count_48(&frame.commands), 1, "at rest: the min range mark alone reads 48");
        let grab = Vec2::new(dial.pos.x + dial.size.x * 0.5, dial.pos.y + dial.size.y - 2.0);

        let held = run_ui(&tree, &model, &styles, &snap(grab, true, true), &mut state);
        // The drag INDICATOR: while the hand holds the knob, the LIVE value
        // joins the range mark — two "48"s on screen, the number you are
        // setting riding the handle at the point of motion.
        assert_eq!(
            count_48(&held.commands),
            2,
            "mid-drag: the live readout joins the min mark beside the handle"
        );
        let released =
            run_ui(&tree, &model, &styles, &snap(grab, false, false), &mut state);
        let v = released
            .results
            .number(ui::FREQ_BIND)
            .expect("the release edge writes the bind");
        assert!(
            (v - 48.0).abs() < 1.0,
            "a press at the track's bottom lands the minimum, got {v}"
        );
    }

    /// **The map is the standard 96.** Frequency 96 — 92,162 tiles, each the
    /// standard ~49.65 mi across on an Earth-sized body. The icosphere's own
    /// law (`10·f² + 2`) is the cross-check, so a drifted constant cannot pass
    /// as a rounding quirk.
    #[test]
    fn the_map_is_the_standard_ninety_six() {
        let bench = test_bench();
        assert_eq!(bench.map().freq(), 96);
        assert_eq!(bench.map().len(), (10 * 96 * 96 + 2) as usize, "92,162 tiles");
    }

    /// **The whole look of this world is AUTHORED.** The bench names a stage
    /// and hands over a tiling; the stage says what is drawn — the near-black
    /// under-shell, the inset tile shell over it, and the shared reference
    /// frame above both. Nothing in this crate names a colour, a radius or an
    /// inset any more, and this gate is what keeps one from creeping back:
    /// the appearance must arrive through `stages.populous_globe`.
    ///
    /// (The layer parsing and the graticule's own shape are gated where they
    /// live, in flicker-globe: `the_stage_block_drives_the_shells_and_the_clear`
    /// and `the_world_draws_the_shared_graticule`.)
    #[test]
    fn the_worlds_appearance_comes_from_the_authored_stage() {
        use flicker_globe::StageLayer;

        let bench = test_bench();
        let layers = &bench.world.stage().layers;
        let shells: Vec<&StageLayer> =
            layers.iter().filter(|l| matches!(l, StageLayer::Shell { .. })).collect();
        assert_eq!(shells.len(), 2, "the under-shell and the tile shell, both authored");
        assert!(
            layers.iter().any(|l| matches!(l, StageLayer::Graticule { .. })),
            "the reference frame is a layer of the stage, not scene code"
        );
        assert!(
            !layers.iter().any(|l| matches!(l, StageLayer::Shells)),
            "this bench publishes no simulated shells — its world is the authored one"
        );
        // The tiling the world is drawn over is the map's, at every size.
        let shells = bench.world.authored_shells(&bench.map.grid().dirs, bench.map.outlines());
        assert_eq!(shells.len(), 2, "one shell per authored layer, over the map's own tiling");
        assert_eq!(shells[0].dirs.len(), bench.map.len(), "the world IS the map");
    }

    /// **A key press steps the dial through the WHOLE component chain** — the
    /// real bindings → the resolver's edge → the walker's slider-axis nudge on
    /// the FOCUSED dial → `run_ui` stepping the bind by the node's own `step`.
    /// No scene wiring anywhere in the loop: this is the pad channel every
    /// slider now owns, driven end to end with the actual key the map binds.
    /// The dial once sat dead in-window while results-level gates were green —
    /// this chain is the one that must never go quiet again.
    #[test]
    fn a_key_press_steps_the_dial_through_the_component_chain() {
        use flicker::render::Vec2;
        use flicker_input_core::device::Key;
        use flicker_input_core::EventKind;

        let bench = test_bench();
        let tree = bench.build_tree();
        // Light the MAP slice so the gated dial is placed + focusable (tab 0).
        let mut model = bench.model();
        model.set("shown_p0_t0", true);
        let styles = load_styles(HUD_UI_THEME);
        let intents = UiIntents::of(&tree);
        let bindings = ContextualBindings::new(InputMap::wasd_and_mouse());
        let cfg = GamepadConfig::default();
        let mut resolver = Resolver::default();
        let mut input = InputState::new();
        input.set_key(Key::Up, true);

        let mut ev: Vec<Fired> = Vec::new();
        resolver.resolve_frame(&bindings, &cfg, &input, 1, &mut ev);
        assert!(
            ev.iter().any(|f| f.signal == ActionSignal::NavUp && f.kind == EventKind::Press),
            "the resolver edges the bound key into NavUp"
        );

        // The entered-left context: the dial holds the walker focus and the
        // pane's focus graph is live — exactly what the scene sets up.
        let ctx = bindings.active();
        let events: Vec<InputEvent> =
            ev.iter().map(|f| InputEvent::from_fired(f, ctx, &input)).collect();
        let mut ui = UiState::default();
        ui.request_focus(ui::FREQ_BIND);
        let mut walker = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        assert!(walker.take_fired().is_empty(), "a nudge is not a declared intent");

        // The next `run_ui` pass applies the nudge with the NODE's own step and
        // writes the bind — the component's committed write, one step up.
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            screen: Vec2::new(1600.0, 900.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut ui);
        assert_eq!(
            frame.results.number(ui::FREQ_BIND),
            Some(97.0),
            "NavUp stepped the focused vertical dial by its own step"
        );
    }

    /// **The camera is the WORLD's, and the pane it lives in is its gate.** The
    /// bench keeps no orbit code and no device read: the world names the panel
    /// that hands it the look signals, and the walker's one focus id decides
    /// when that is. (The motion itself is gated in flicker-globe:
    /// `the_camera_moves_only_while_the_world_panel_holds_focus`.)
    #[test]
    fn the_camera_belongs_to_the_world_in_the_viewport_pane() {
        let mut bench = test_bench();
        let input = InputState::new();

        // A non-zero look tuple (the pump resolves this from `signals.axis` in-scene); the
        // world applies it ONLY while the viewport pane holds the walker's focus — the gate
        // this asserts. (The motion itself is gated in flicker-globe:
        // `the_camera_moves_only_while_the_world_panel_holds_focus`.)
        let turn = |b: &mut PopulousBench, input: &InputState| {
            let before = b.world.camera().position;
            let focus = b.ui_state.focused().map(str::to_string);
            b.world.update(0.5, input, (1.0, 0.0, 0.0), focus.as_deref());
            (b.world.camera().position - before).length()
        };
        assert_eq!(turn(&mut bench, &input), 0.0, "panel navigation owns the sticks");
        bench.focus_for_test(ui::RIGHT_PANE);
        assert_eq!(turn(&mut bench, &input), 0.0, "another pane owns its interior, never the camera");
        bench.focus_for_test(ui::VIEW_PANE);
        assert!(turn(&mut bench, &input) > 0.0, "the focused viewport pane flies the planet");
    }

    /// **A Menu press fires the pause intent** through the real chain: the
    /// screen's declared `on_menu` is consumed at the walker layer and comes
    /// back as the `pause_open` result `update` answers with the pause push.
    #[test]
    fn a_menu_press_fires_the_declared_pause_intent() {
        let bench = test_bench();
        let tree = bench.build_tree();
        let intents = UiIntents::of(&tree);

        let raw = InputState::new();
        let events =
            [InputEvent::new(ActionSignal::Menu, EventKind::Press, InputContext::World, &raw)];
        let mut ui = UiState::default();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        assert_eq!(
            walker.take_fired(),
            vec!["pause_open".to_string()],
            "the declared intent reaches the scene as its result name"
        );
    }

    /// **`arrange()` lights exactly the selected tab's slice.** The Lua
    /// orchestration reads the two-way-bound page/tab out of the Model and returns
    /// which selection-keyed slice is shown — the MAP tab lights `shown_p0_t0`, the
    /// SEAMS tab lights `shown_p0_t1`, and neither lights the other's. Gating is by
    /// SELECTION, not content; this is the whole of what Lua decides for this bench,
    /// and the values (the dial's number, the readouts) stay engine-side.
    #[test]
    fn arrange_lights_the_selected_tabs_slice() {
        use flicker::script::ScriptHost;

        let host = ScriptHost::new(POPULOUS_SCRIPT, "populous.lua").expect("populous.lua loads");
        let arrange_at = |page: f64, tab: f64| {
            let mut m = ValueMap::default();
            m.set(ui::PAGE_BIND, page);
            m.set(ui::TAB_BIND, tab);
            host.set_model(&m).expect("model publishes");
            host.arrange().expect("arrange runs").expect("arrange is present").to_model()
        };

        // MAP tab (page 0, tab 0): its slice is lit, the seams slice is dark.
        let map = arrange_at(0.0, 0.0);
        assert!(map.is_on("shown_p0_t0"), "the map tab lights its slice");
        assert!(!map.is_on("shown_p0_t1"), "the map tab darkens the seams slice");

        // SEAMS tab (page 0, tab 1): its slice is lit, the map slice is dark.
        let seams = arrange_at(0.0, 1.0);
        assert!(seams.is_on("shown_p0_t1"), "the seams tab lights its slice");
        assert!(!seams.is_on("shown_p0_t0"), "the seams tab darkens the map slice");
    }

    /// **The static tree loads and declares BOTH tabs' slices gated.** The authored
    /// JSON parses to a `UiNode` tree of component KINDS (the template tier is gone —
    /// 201F4F51), every component kind is known, the three panes + the viewport + the
    /// size dial are all present at once, and each tab's slice is gated on its
    /// `shown_p0_t*` key — the data half of the Lua-arrange pattern.
    /// The shipped scene file, read exactly as the manifest reads it.
    const POPULOUS_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/populous.scene.json");

    /// A bench built the way the resolver builds it: from the shipped file's def.
    fn test_bench() -> PopulousBench {
        let def = SceneDef::parse("populous", POPULOUS_SCENE).expect("populous.scene.json loads");
        PopulousBench::new(&def)
    }

    #[test]
    fn the_static_tree_loads_and_gates_both_tabs() {
        let tree = SceneDef::parse("populous", POPULOUS_SCENE)
            .expect("populous.scene.json loads")
            .tree
            .expect("populous.scene.json declares a tree");
        let all = flatten(&tree);

        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(unknown.is_empty(), "unknown component kinds: {unknown:?}");

        // The whole layout is present at once — both tabs, gated (not a per-tab rebuild).
        for id in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE] {
            assert!(all.iter().any(|n| n.id == id), "the `{id}` pane is in the static tree");
        }
        let view = all.iter().find(|n| n.id == ui::VIEW_SLOT).expect("the viewport is placed");
        assert_eq!(view.component, "rtt", "the centre pane is the viewport");
        assert!(
            all.iter().any(|n| n.bind.as_deref() == Some(ui::FREQ_BIND)),
            "the size dial (bound to pop_freq) is present"
        );

        // Both tabs' slices are declared and gated on the keys `arrange()` lights.
        let gates: std::collections::HashSet<&str> =
            all.iter().filter_map(|n| n.visible_bind.as_deref()).collect();
        assert!(gates.contains("shown_p0_t0"), "the map tab's slice is gated");
        assert!(gates.contains("shown_p0_t1"), "the seams tab's slice is gated");
    }
}
