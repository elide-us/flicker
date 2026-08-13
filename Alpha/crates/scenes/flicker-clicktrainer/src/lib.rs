//! flicker-clicktrainer: an aim / click-training game — the reference **2D**
//! client, and the demonstration of blending the engine's two UI modes on one
//! screen: **2D sprite gameplay** (the target box + lifetime bar, drawn with
//! `draw_sprite`) UNDER a **declarative vector HUD** (a carved-stone stats panel
//! and a RESET button — a `clicktrainer.scene.json` component tree walked by the
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

use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_router::{InputHandler, Router};
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

/// The scene's PAIR SCRIPT (`SceneName.lua` — the scene's component logic).
const CLICKTRAINER_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/clicktrainer.lua");
const HUD_UI_THEME: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_theme.json");
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
    /// The AUTHORED tree off the manifest's def (the kernel parsed the scene file;
    /// this bench is the behaviour that plays it). `enter` installs it as the HUD.
    authored: Option<UiNode>,
    /// The scene file's own `styles` blocks (five-line split) — merged over the
    /// shared theme root when `enter` loads styles.
    scene_styles: Option<serde_json::Value>,
    /// The PAIR SCRIPT host (`clicktrainer.lua`): `derive()` owns the HUD's display
    /// derivations; the engine publishes only raw runtime variables.
    script: Option<ScriptHost>,
    /// The HUD's component tree, parsed ONCE from the script's `tree()` at load;
    /// the walker redraws this cached tree every frame with fresh Model bindings.
    ui_tree: Option<UiNode>,
    /// The screen's declarative bindings (S9), read off the cached tree's root
    /// (`on_menu = "pause_open"`).
    ui_intents: UiIntents,
    /// Token-resolved `ui_theme.json` styles the walker resolves node `style`
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
    /// Theme for the pause overlay we push (built once in `enter`).
    ui_theme: Option<Theme>,

    /// The retained walker [`UiState`] the HUD layer writes focus through. Input-P3
    /// (0569DA9B): the scene owns NO resolver/bindings — the PUMP resolves this frame's
    /// events (World context) and hands them in via `SceneInput`.
    ui_state: UiState,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
}

impl ClickTrainer {
    /// A fresh click-trainer scene (no target placed yet — `enter` spawns the first),
    /// carrying its authored HUD tree off the manifest's def.
    pub fn new(def: &SceneDef) -> Self {
        Self {
            white: None,
            authored: def.tree.clone(),
            scene_styles: def.styles.clone(),
            script: match ScriptHost::new(CLICKTRAINER_SCRIPT, "clicktrainer.lua") {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("clicktrainer.lua failed to load — raw HUD values only: {e}");
                    None
                }
            },
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
            ui_theme: None,
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
        // The ENGINE publishes raw runtime variables; the PAIR SCRIPT derives the
        // display values (five-line split — logic lives in clicktrainer.lua).
        let secs = |t: f32| if t.is_finite() { f64::from(t) } else { -1.0 };
        let avg = if self.hits > 0 {
            self.total_reaction / self.hits as f32
        } else {
            f32::INFINITY
        };
        let raw = ValueMap::new()
            .with("hits", f64::from(self.hits))
            .with("misses", f64::from(self.misses))
            .with("accuracy_pct", f64::from(self.accuracy()))
            .with("react_last_s", secs(self.last_reaction))
            .with("react_best_s", secs(self.best_reaction))
            .with("react_avg_s", secs(avg));
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("clicktrainer: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("clicktrainer.lua derive() failed: {e}"),
            }
        }
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
        self.ui_styles = flicker::ui::load_styles_for(HUD_UI_THEME, self.scene_styles.as_ref());
        // The HUD is DATA (201F4F51): the authored tree came off the manifest's def at
        // construction; installing it here keeps re-entry (pause pop) behaviour
        // identical. Degrades to no HUD (the game still plays) if the file had none.
        match self.authored.clone() {
            Some(tree) => {
                // The screen's declarative bindings (S9), read off the root once.
                self.ui_intents = UiIntents::of(&tree);
                self.ui_tree = Some(tree);
            }
            None => tracing::error!("ClickTrainer's scene file has no `tree` — no HUD"),
        }
        self.spawn(renderer.size());
        renderer.window().set_title("Flicker Click Trainer");
    }

    fn update(&mut self, dt: Duration, input: &InputState, signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        let dt_s = dt.as_secs_f32();
        let screen = renderer.size();

        // (Settings rebinds are the PUMP's job now — S1c — not the scene's: it owns no
        // bindings to re-seed, and the pump adopts the committed World map each frame.)

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
            let frame = run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
            over_hud = frame.results.is_on("hud_hit");
            if frame.results.is_on("reset") {
                self.reset();
            }
            self.hud_commands = frame.commands;
        }

        // ── The input seam (input-P3, 0569DA9B): the PUMP resolved this frame's events
        // (World context) — the scene owns no Resolver. Dispatch `signals.events`:
        //   [ROOT]  RootHandler   — declares World (no consuming arms — S10)
        //   [1]     WalkerHandler — consumes the click while `over_hud`, + the screen's
        //                           DECLARED `on_menu` intent
        //   [2]     GameplayBase  — a click that bubbled past the HUD scores a hit/miss
        // No focusable tree + no context-pushing handler, so nothing to reconcile — the
        // runner applies the route's (empty) context requests after `update`. ──
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        let mut gameplay = GameplayBase::default();
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        self.fired_sigs = walker.take_fired();

        // The screen DECLARED `on_menu = "pause_open"` (S9/S10): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto the
        // shell pause push — the root's hardcoded Menu arm is gone. The scene
        // manager freezes us while the overlay is up, so the target clock stops too.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.ui_theme.expect("theme built in enter");
            // The scene owns no bindings; take the World map from the shared profile for
            // the pause overlay (it binds Menu, so Esc resumes).
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

/// The bench's launchable-scene factory — the CLIENT BEHAVIOUR the roster registers:
/// a menu LOAD (`Goto { id: "clicktrainer" }`) resolves the manifest's def and hands
/// it here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(ClickTrainer::new(def))
}

#[cfg(test)]
mod tests {
    //! Load the real `clicktrainer.scene.json` + the shared `ui_theme.json` and walk
    //! a frame, so a Lua syntax/runtime error — or a broken click-routing contract —
    //! fails the build instead of only surfacing in the running app.
    use super::*;
    use flicker::ui::run_ui;
    use flicker_input_core::ActionSignal;

    /// The shipped scene file, read by the gates exactly as the manifest reads it.
    const CLICKTRAINER_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/clicktrainer.scene.json");

    fn tree_and_styles() -> (UiNode, serde_json::Value) {
        let def = SceneDef::parse("clicktrainer", CLICKTRAINER_SCENE)
            .expect("clicktrainer.scene.json loads");
        let tree = def.tree.expect("scene defines a tree");
        // The scene's OWN style blocks ride its file (five-line split) and merge
        // over the shared root — the exact path `enter` runs.
        let styles = flicker::ui::load_styles_for(HUD_UI_THEME, def.styles.as_ref());
        (tree, styles)
    }

    /// THE PAIR-SCRIPT REGRESSION GATE: build the bench exactly as the resolver
    /// does (real def, real clicktrainer.lua) and run the REAL hud_model path —
    /// the raw variables must come back as the DERIVED display strings the tree
    /// binds. This is the coverage whose absence let a silent Lua failure ship:
    /// a derive() that throws leaves numbers (or nothing) under the display keys.
    #[test]
    fn the_pair_script_derives_the_display_strings() {
        let def = SceneDef::parse("clicktrainer", CLICKTRAINER_SCENE)
            .expect("clicktrainer.scene.json loads");
        if let Err(e) = ScriptHost::new(CLICKTRAINER_SCRIPT, "clicktrainer.lua") {
            panic!("clicktrainer.lua failed to load: {e}");
        }
        let ct = ClickTrainer::new(&def);
        assert!(ct.script.is_some(), "clicktrainer.lua loads (the pair script)");
        let m = ct.hud_model();
        for key in ["hits", "misses", "accuracy", "react_last", "react_best", "react_avg"] {
            assert!(
                m.text(key).is_some(),
                "derive() must yield display TEXT for '{key}' — got {:?}",
                m.number(key).map(|n| format!("Number({n})"))
            );
        }
        assert_eq!(m.text("hits"), Some("0"));
        assert_eq!(m.text("accuracy"), Some("100%"), "no shots yet = a perfect record");
        assert_eq!(m.text("react_last"), Some("—"), "no hit yet reads as an em dash");
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
            "clicktrainer.scene.json names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "clicktrainer.scene.json ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The MODEL-CHANNEL strings gate (S10's blind side): display copy published
        // from Rust into the Model bypasses the tree gate above, so the crate
        // self-gates its OWN source — every `.set`/`.with` value must be a resolved
        // `$token`, a data shape, or carry an explicit `strings-gate-exempt` reason.
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
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
