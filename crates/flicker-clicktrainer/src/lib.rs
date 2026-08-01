//! flicker-clicktrainer: an aim / click-training game — the reference **2D**
//! client, and the demonstration of blending the engine's two UI modes on one
//! screen: **2D sprite gameplay** (the target box + lifetime bar, drawn with
//! `draw_sprite`) UNDER a **declarative vector HUD** (a carved-stone stats panel
//! and a RESET button — a `hud_clicktrainer.lua` component tree walked by the
//! Rust component walker), with **clicks routed correctly** between them.
//!
//! The walker reports `hud_hit` when the cursor is over its panel; the scene only
//! scores a click as a game hit/miss when it did NOT land on the HUD — so the
//! RESET button (and the panel it sits on) never costs you accuracy. A minigame
//! PACKAGE (library only); the `prism-alpha` launcher lists this scene as "CLICK TRAINER".
//!
//! - a `flicker::scene::Scene` wired into the **`flicker-shell`** front-end
//!   (intro splash → menu → *this* → pause/settings);
//! - **discrete** clicks (press edge, not hold) scored as hits vs. misses, with
//!   live accuracy and reaction-time readouts in the HUD;
//! - a difficulty ramp — the target shrinks as you land hits — and a per-target
//!   lifetime that counts as a miss if it times out.
//!
//! Controls: left-click a target to hit it; a misclick or a timed-out target
//! costs accuracy. RESET zeroes the stats. Esc opens the pause menu (the screen
//! root's DECLARED `on_menu = "pause_open"` intent — S9/S10).

use std::time::Duration;

use flicker_input_core::{AbstractControls, ContextualBindings, GamepadConfig, InputMap, InputState};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{ComponentLibrary, HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    load_styles, load_ui_json, render_hud, run_ui_with, UiInput, UiIntents, UiState, WalkerHandler,
    UI_COMPONENT_MODULES,
};
use flicker_input_core::{Fired, Resolver};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

mod route;
use route::{GameplayBase, RootHandler};

// ── Tuning ───────────────────────────────────────────────────────────
/// Target edge (px) for the first hit; shrinks toward `TARGET_MIN_SIZE`.
const TARGET_START_SIZE: f32 = 90.0;
/// Smallest a target ever gets, however high the score climbs.
const TARGET_MIN_SIZE: f32 = 34.0;
/// Edge shrink per hit — the difficulty ramp.
const TARGET_SHRINK_PER_HIT: f32 = 1.5;
/// Seconds a target survives before it times out (counts as a miss).
const TARGET_LIFETIME: f32 = 1.15;

/// Fresh-target colour; lerps toward [`URGENT`] as the lifetime drains.
const CALM: [f32; 3] = [0.30, 0.80, 0.85];
const URGENT: [f32; 3] = [0.95, 0.30, 0.25];

/// The declarative HUD tree (`hud_clicktrainer.lua`) + the shared UI-element layout.
const HUD_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Alpha/content/sensorium/scripts/hud_clicktrainer.lua"
);
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/sensorium/resources/ui_elements.json");
/// Top-left region (px) reserved for the HUD panel, so every target stays
/// clickable (the HUD absorbs clicks in its own area). A touch larger than the
/// panel; targets that would land inside it are re-rolled.
const HUD_RESERVE_W: f32 = 320.0;
const HUD_RESERVE_H: f32 = 340.0;

/// The click-trainer game scene. Everything a frame needs lives here; the shell
/// drives it through the [`Scene`] trait. Public so a host binary (this crate's own
/// `main`, or the paperdoll launcher) can register it as a launchable scene.
pub struct ClickTrainer {
    /// 1×1 white pixel — the sprite shader tints it, so one texture draws the
    /// target box and its lifetime bar in any colour.
    white: Option<TextureHandle>,
    /// The HUD script host, RETAINED past tree-build as the Lua component library
    /// (`ui.*` modules) the walker dispatches per-node DRAW to. `None` if it
    /// failed to load — the game still runs, just without the panel.
    script: Option<ScriptHost>,
    /// The HUD's component tree, parsed ONCE from the script's `tree()` at load;
    /// the walker redraws this cached tree every frame with fresh Model bindings.
    ui_tree: Option<UiNode>,
    /// The screen's declarative bindings (S9), read off the cached tree's root
    /// (`on_menu = "pause_open"`).
    ui_intents: UiIntents,
    /// Token-resolved `ui_elements.json` styles the walker resolves node `style`
    /// paths against.
    ui_styles: serde_json::Value,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,

    // ── current target ──
    target_pos: Vec2,
    target_size: f32,
    /// Seconds left before the target times out.
    time_remaining: f32,
    /// Seconds the target has been alive → the reaction time on a hit.
    spawn_age: f32,
    /// `false` until the first target is placed.
    spawned: bool,

    // ── stats ──
    hits: u32,
    misses: u32,
    /// Reaction times (seconds); `INFINITY` reads as "—" until the first hit.
    last_reaction: f32,
    best_reaction: f32,
    total_reaction: f32,

    // ── shell / pause plumbing ──
    /// Per-context action maps (World base only — the click trainer has no chat /
    /// text-entry context). Drives the pause hotkey (`Menu` = Esc) through the
    /// resolver and round-trips through the settings overlay; each `PauseScene` we
    /// push reads its `active_map()`.
    bindings: ContextualBindings,
    /// Gamepad calibration (defaults — the click trainer has no camera). Read by the
    /// resolver each frame and handed to each `PauseScene` we push.
    gamepad_config: GamepadConfig,
    /// Theme for the pause overlay we push (built once in `enter`).
    ui_theme: Option<Theme>,

    /// The new-input-model per-frame seam (spec §5/§9): a stateful edge [`Resolver`]
    /// (replaces the `menu_prev` bool), a REUSED `Fired` scratch buffer (no per-frame
    /// alloc — RT-7), the router's request queue, a monotonic frame `tick` (the
    /// resolver's `TickTime`, NOT wall-clock — spec §3.2a), and the retained walker
    /// [`UiState`] the HUD layer writes focus through.
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    tick: u64,
    ui_state: UiState,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
}

impl ClickTrainer {
    /// A fresh click-trainer scene (no target placed yet — `enter` spawns the first).
    pub fn new() -> Self {
        Self {
            white: None,
            script: None,
            ui_tree: None,
            ui_intents: UiIntents::default(),
            ui_styles: serde_json::Value::Object(Default::default()),
            hud_commands: Vec::new(),
            target_pos: Vec2::ZERO,
            target_size: TARGET_START_SIZE,
            time_remaining: 0.0,
            spawn_age: 0.0,
            spawned: false,
            hits: 0,
            misses: 0,
            last_reaction: f32::INFINITY,
            best_reaction: f32::INFINITY,
            total_reaction: 0.0,
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            ui_theme: None,
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            ui_state: UiState::new(),
            fired_sigs: Vec::new(),
        }
    }

    /// The current target edge, shrunk by the hit count (difficulty ramp).
    fn size_for_difficulty(&self) -> f32 {
        (TARGET_START_SIZE - self.hits as f32 * TARGET_SHRINK_PER_HIT).max(TARGET_MIN_SIZE)
    }

    /// Place a fresh target at a random position clear of the HUD panel, reset
    /// its clock.
    fn spawn(&mut self, screen: Vec2) {
        self.target_size = self.size_for_difficulty();
        let max_x = ((screen.x - self.target_size) as u32).max(1);
        let max_y = ((screen.y - self.target_size) as u32).max(1);
        let roll =
            || Vec2::new(fastrand::u32(0..max_x) as f32, fastrand::u32(0..max_y) as f32);
        let mut pos = roll();
        // Re-roll off the top-left HUD region (bounded, so it always terminates).
        for _ in 0..12 {
            if pos.x >= HUD_RESERVE_W || pos.y >= HUD_RESERVE_H {
                break;
            }
            pos = roll();
        }
        self.target_pos = pos;
        self.time_remaining = TARGET_LIFETIME;
        self.spawn_age = 0.0;
        self.spawned = true;
    }

    fn target_contains(&self, p: Vec2) -> bool {
        p.x >= self.target_pos.x
            && p.x < self.target_pos.x + self.target_size
            && p.y >= self.target_pos.y
            && p.y < self.target_pos.y + self.target_size
    }

    fn accuracy(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            100.0
        } else {
            self.hits as f32 / total as f32 * 100.0
        }
    }

    /// Reset all stats + the difficulty ramp (the HUD's RESET button).
    fn reset(&mut self) {
        self.hits = 0;
        self.misses = 0;
        self.last_reaction = f32::INFINITY;
        self.best_reaction = f32::INFINITY;
        self.total_reaction = 0.0;
        self.target_size = TARGET_START_SIZE;
        self.spawned = false; // a fresh target is placed next frame
    }

    /// The per-frame HUD model — every stat pre-formatted to the string the HUD
    /// tree's `text_bind`s display verbatim (`"—"` until there's data), plus the
    /// transient `sig_<name>` mirror of last frame's fired intents.
    fn hud_model(&self) -> ValueMap {
        let react = |t: f32| {
            if t.is_finite() {
                format!("{:.0} ms", t * 1000.0)
            } else {
                "—".to_string()
            }
        };
        let avg = if self.hits > 0 {
            self.total_reaction / self.hits as f32
        } else {
            f32::INFINITY
        };
        let mut m = ValueMap::new()
            .with("hits", self.hits.to_string())
            .with("misses", self.misses.to_string())
            .with("accuracy", format!("{:.0}%", self.accuracy()))
            .with("react_last", react(self.last_reaction))
            .with("react_best", react(self.best_reaction))
            .with("react_avg", react(avg));
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }
}

impl Scene for ClickTrainer {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Theme for the pause overlay we push on Esc (built once, reused).
        self.ui_theme = Some(Theme::build(renderer));
        // The declarative HUD: a component tree walked by the engine. Degrades to
        // no HUD (the game still plays) if the script can't load. The styles are
        // the token-resolved layout JSON (the same tree Lua reads via `UI`).
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        match ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES) {
            Ok(script) => {
                load_ui_json(&script, HUD_UI_ELEMENTS); // layout (`UI.clicktrainer`)
                match script.ui_tree() {
                    Ok(Some(tree)) => {
                        // The screen's declarative bindings (S9), read off the
                        // root once — cached exactly like the tree.
                        self.ui_intents = UiIntents::of(&tree);
                        self.ui_tree = Some(tree);
                    }
                    Ok(None) => tracing::error!("HUD script exposes no `tree()` — no HUD"),
                    Err(e) => tracing::error!("HUD tree build failed ({e}); no HUD"),
                }
                self.script = Some(script);
            }
            Err(e) => tracing::warn!("HUD script load failed ({HUD_SCRIPT}): {e} — no HUD"),
        }
        self.spawn(renderer.size());
        renderer.window().set_title("Flicker Click Trainer");
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let dt_s = dt.as_secs_f32();
        let screen = renderer.size();

        // Pick up any input-settings change made in the pause→settings overlay.
        if let Some((map, _, _)) = flicker_shell::take_pending_input() {
            self.bindings = ContextualBindings::new(map);
        }

        if !self.spawned {
            self.spawn(screen);
        }

        // Walk the cached HUD tree: layout + hit-test + draw in one pass. The
        // results carry `hud_hit` (cursor over the panel) and `reset` (the RESET
        // button's action). `over_hud` feeds the walker layer below as this
        // frame's pointer-consume, so a click on the panel / RESET is swallowed
        // before it can score — the click-routing that lets the 2D game and the
        // vector HUD share one screen.
        let mut over_hud = false;
        if let Some(tree) = self.ui_tree.as_ref() {
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                screen,
                typed: String::new(),
                backspace: false,
                wheel: input.mouse_wheel_delta,
            };
            let lib = self.script.as_ref().map(|h| h as &dyn ComponentLibrary);
            let frame = run_ui_with(tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib);
            over_hud = frame.results.is_on("hud_hit");
            if frame.results.is_on("reset") {
                self.reset();
            }
            self.hud_commands = frame.commands;
        }

        // ── The input seam (spec §5/§9): ONE resolve + ONE dispatch. The resolver
        // owns the press edges; the chain arbitrates them:
        //   [ROOT]  RootHandler   — declares World (no consuming arms — S10)
        //   [1]     WalkerHandler — consumes the click while `over_hud`, and the
        //                           screen's DECLARED `on_menu` intent
        //   [2]     GameplayBase  — a click that bubbled past the HUD scores a hit/miss
        // `ev` is the REUSED `Fired` buffer; the `InputEvent` list is a short-lived local
        // (it borrows this frame's snapshot, so it cannot be a field — RT-7 holds because
        // steady-state frames resolve zero edges and allocate nothing).
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver
            .resolve_frame(&self.bindings, &self.gamepad_config, input, self.tick, &mut self.ev);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, input))
            .collect();
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done

        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        let mut gameplay = GameplayBase::default();
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // Reconcile any context/focus intents. None arise in this chain today (no
        // context-pushing handler), but this is the standard post-dispatch seam,
        // applied through the walker so a future navigable HUD shares one focus id
        // for mouse + gamepad (spec §4.2a).
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        // The screen's fired intents (S9), drained once: acted on below and queued
        // for the one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // The screen DECLARED `on_menu = "pause_open"` (S9/S10): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto the
        // shell pause push — the root's hardcoded Menu arm is gone. The scene
        // manager freezes us while the overlay is up, so the target clock stops too.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.ui_theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                self.bindings.active_map(),
                &AbstractControls::default(),
                &self.gamepad_config,
            )));
        }

        // A left-click that bubbled past the HUD to the gameplay base is a game click
        // (the typed form of the old `!over_hud` gate): inside the target → a hit that
        // scores + respawns; elsewhere on the play-field → a misclick (accuracy
        // penalty, the target stays).
        if gameplay.click {
            if self.target_contains(input.mouse_position) {
                self.hits += 1;
                self.last_reaction = self.spawn_age;
                self.best_reaction = self.best_reaction.min(self.spawn_age);
                self.total_reaction += self.spawn_age;
                self.spawn(screen);
            } else {
                self.misses += 1;
            }
        }

        // Lifetime: run the clock down; a timeout (no hit) is a miss + respawn.
        self.time_remaining -= dt_s;
        self.spawn_age += dt_s;
        if self.time_remaining <= 0.0 {
            self.misses += 1;
            self.spawn(screen);
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(white) = self.white else {
            return;
        };

        // ── 2D game elements (sprite engine), drawn at the scene base layer ──
        // Target box, tinted calm → urgent as its lifetime drains.
        let urgency = 1.0 - (self.time_remaining / TARGET_LIFETIME).clamp(0.0, 1.0);
        let mix = |a: f32, b: f32| a + (b - a) * urgency;
        let color = [
            mix(CALM[0], URGENT[0]),
            mix(CALM[1], URGENT[1]),
            mix(CALM[2], URGENT[2]),
            1.0,
        ];
        renderer.draw_sprite(white, self.target_pos, Vec2::splat(self.target_size), color);

        // Thin lifetime bar beneath the target (width tracks time remaining).
        let frac = (self.time_remaining / TARGET_LIFETIME).clamp(0.0, 1.0);
        renderer.draw_sprite(
            white,
            Vec2::new(self.target_pos.x, self.target_pos.y + self.target_size + 4.0),
            Vec2::new(self.target_size * frac, 4.0),
            [0.85, 0.85, 0.90, 0.9],
        );

        // ── Vector UI (walker commands stashed by `update`), above the game ──
        render_hud(renderer, &self.hud_commands, white, &[]);
    }
}

impl Default for ClickTrainer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! Load the real `hud_clicktrainer.lua` + the shared `ui_elements.json` and walk
    //! a frame, so a Lua syntax/runtime error — or a broken click-routing contract —
    //! fails the build instead of only surfacing in the running app.
    use super::*;
    use flicker::ui::run_ui;
    use flicker_input_core::ActionSignal;

    fn tree_and_styles() -> (UiNode, serde_json::Value) {
        let h = ScriptHost::from_file_with_modules(HUD_SCRIPT, UI_COMPONENT_MODULES)
            .expect("load hud_clicktrainer.lua");
        load_ui_json(&h, HUD_UI_ELEMENTS);
        let tree = h.ui_tree().expect("tree builds").expect("script exposes tree()");
        (tree, load_styles(HUD_UI_ELEMENTS))
    }

    fn model() -> ValueMap {
        ValueMap::new()
            .with("hits", "5")
            .with("misses", "1")
            .with("accuracy", "83%")
            .with("react_last", "210 ms")
            .with("react_best", "180 ms")
            .with("react_avg", "205 ms")
    }

    fn snap_at(x: f32, y: f32, clicked: bool) -> UiInput {
        UiInput {
            mouse: Vec2::new(x, y),
            clicked,
            down: clicked,
            screen: Vec2::new(1280.0, 720.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        }
    }

    /// The vocabulary gate + the screen's S9 declaration: every kind in the real
    /// tree is one the engine knows, and the root declares the pause intent.
    #[test]
    fn tree_is_well_formed_and_declares_the_pause_intent() {
        let (tree, _) = tree_and_styles();
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "hud_clicktrainer.lua names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "hud_clicktrainer.lua ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
    }

    /// The click-routing contract through the REAL tree: a pointer on the panel
    /// sets `hud_hit`; one on the play-field does not; a click on RESET fires the
    /// `reset` action; and the whole panel draws.
    #[test]
    fn hud_routes_clicks_and_draws() {
        let (tree, styles) = tree_and_styles();
        let model = model();

        // A pointer over the top-left panel is the HUD's (hud_hit), so the
        // scene must not score a click there as a game click.
        let frame = run_ui(&tree, &model, &styles, &snap_at(30.0, 30.0, false), &mut UiState::new());
        assert!(frame.results.is_on("hud_hit"), "a pointer on the panel must set hud_hit");
        assert!(!frame.commands.is_empty(), "the HUD draws the panel + stats");

        // A pointer out on the play-field is the GAME's, not the HUD's.
        let frame =
            run_ui(&tree, &model, &styles, &snap_at(900.0, 500.0, false), &mut UiState::new());
        assert!(!frame.results.is_on("hud_hit"), "a play-field pointer is not hud_hit");

        // A click on the RESET button fires its action (panel: margin 16 + pad 16,
        // below title/subtitle/divider/6 stat rows → the button row sits ~y 252..288;
        // click its centre).
        let frame =
            run_ui(&tree, &model, &styles, &snap_at(160.0, 270.0, true), &mut UiState::new());
        assert!(frame.results.is_on("reset"), "a click on RESET fires the reset action");
        assert!(frame.results.is_on("hud_hit"), "…and it is a HUD click, never a game miss");
    }
}
