//! flicker-clicktrainer: an aim / click-training game — the reference **2D**
//! client, and the demonstration of blending the engine's two UI modes on one
//! screen: **2D sprite gameplay** (the target box + lifetime bar, drawn with
//! `draw_sprite`) UNDER a **vector Lua HUD** (a carved-stone stats panel + a
//! RESET button, `hud_clicktrainer.lua` + `UI.clicktrainer`), with **clicks routed
//! correctly** between them.
//!
//! The HUD reports `hud_hit` when the cursor is over its panel; the scene only
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
//! costs accuracy. RESET zeroes the stats. Esc opens the pause menu.

use std::time::Duration;

use flicker::app::{AbstractControls, Action, GamepadConfig, InputMap, InputState};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};
use flicker_shell::{PauseScene, Theme};

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

/// The vector HUD panel (`hud_clicktrainer.lua`) + the shared UI-element layout.
const HUD_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../Alpha/content/scripts/hud_clicktrainer.lua"
);
const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/resources/ui_elements.json");
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
    /// The vector HUD script (`hud_clicktrainer.lua` + `UI.clicktrainer`). `None` if
    /// it failed to load — the game still runs, just without the panel.
    script: Option<ScriptHost>,

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
    /// Bindings drive the pause hotkey (Menu = Esc) and round-trip through the
    /// settings overlay. The click trainer has no camera, so it doesn't keep an
    /// `AbstractControls`/`GamepadConfig` — the pause scene takes defaults.
    bindings: InputMap,
    menu_prev: bool,
    /// Theme for the pause overlay we push (built once in `enter`).
    ui_theme: Option<Theme>,
}

impl ClickTrainer {
    /// A fresh click-trainer scene (no target placed yet — `enter` spawns the first).
    pub fn new() -> Self {
        Self {
            white: None,
            script: None,
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
            bindings: InputMap::wasd_and_mouse(),
            menu_prev: false,
            ui_theme: None,
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

    /// The per-frame HUD model — every stat pre-formatted to the string the Lua
    /// panel displays verbatim (`"—"` until there's data).
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
        ValueMap::new()
            .with("hits", self.hits.to_string())
            .with("misses", self.misses.to_string())
            .with("accuracy", format!("{:.0}%", self.accuracy()))
            .with("react_last", react(self.last_reaction))
            .with("react_best", react(self.best_reaction))
            .with("react_avg", react(avg))
    }
}

impl Scene for ClickTrainer {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Theme for the pause overlay we push on Esc (built once, reused).
        self.ui_theme = Some(Theme::build(renderer));
        // The vector HUD: a Lua stats panel over the 2D game. Degrades to no HUD
        // (the game still plays) if the script can't load.
        match ScriptHost::from_file(HUD_SCRIPT) {
            Ok(script) => {
                load_ui_json(&script, HUD_UI_ELEMENTS); // layout (`UI.clicktrainer`)
                load_widgets(&script); // the shared immediate-mode toolkit
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
            self.bindings = map;
        }

        // Esc / Menu → push the shell pause overlay (edge-detected). The scene
        // manager freezes us while it's up, so the target clock stops too.
        let menu_down = self.bindings.action_pressed(Action::Menu, input);
        let menu_pressed = menu_down && !self.menu_prev;
        self.menu_prev = menu_down;
        if menu_pressed {
            let theme = self.ui_theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &self.bindings,
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }

        if !self.spawned {
            self.spawn(screen);
        }

        // Run the vector HUD. It reports whether the cursor is over the panel (a
        // UI click, not a game click) and whether RESET fired — this is the
        // click-routing that lets the two modes share the screen.
        let mut over_hud = false;
        if let Some(script) = self.script.as_ref() {
            let _ = script.set_model(&self.hud_model());
            match script.update(input, screen.x, screen.y) {
                Ok(res) => {
                    over_hud = res.is_on("hud_hit");
                    if res.is_on("reset") {
                        self.reset();
                    }
                }
                Err(e) => tracing::warn!("HUD update failed: {e}"),
            }
        }

        // Discrete click (press edge, not hold), scored only when it did NOT land
        // on the HUD: inside the target → a hit that scores + respawns; elsewhere
        // on the play-field → a misclick (accuracy penalty, the target stays).
        if input.mouse_left_pressed && !over_hud {
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

        // ── Vector UI (Lua panel), drawn above the game via its own layer ──
        if let Some(script) = self.script.as_ref() {
            let _ = script.set_model(&self.hud_model());
            let size = renderer.size();
            match script.draw(size.x, size.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::warn!("HUD draw failed: {e}"),
            }
        }
    }
}

impl Default for ClickTrainer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! Load the real `hud_clicktrainer.lua` + the shared `ui_elements.json` and run a
    //! frame, so a Lua syntax/runtime error — or a broken click-routing contract —
    //! fails the build instead of only surfacing in the running app.
    use super::*;

    fn host() -> ScriptHost {
        let h = ScriptHost::from_file(HUD_SCRIPT).expect("load hud.lua");
        load_ui_json(&h, HUD_UI_ELEMENTS);
        load_widgets(&h);
        h
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

    fn click_at(x: f32, y: f32) -> InputState {
        let mut i = InputState::new();
        i.mouse_position = Vec2::new(x, y);
        i.mouse_left_pressed = true;
        i
    }

    #[test]
    fn hud_routes_clicks_and_draws() {
        let h = host();

        // A click over the top-left panel is CONSUMED by the HUD (hud_hit), so the
        // scene must not score it as a game click.
        h.set_model(&model()).unwrap();
        let res = h.update(&click_at(30.0, 30.0), 1280.0, 720.0).expect("update");
        assert!(res.is_on("hud_hit"), "a click on the panel must set hud_hit");

        // A click out on the play-field is the GAME's, not the HUD's.
        h.set_model(&model()).unwrap();
        let res = h.update(&click_at(900.0, 500.0), 1280.0, 720.0).expect("update");
        assert!(!res.is_on("hud_hit"), "a play-field click is not hud_hit");

        // Draw emits the panel + stats (proves the whole draw path runs).
        h.set_model(&model()).unwrap();
        let cmds = h.draw(1280.0, 720.0).expect("draw");
        assert!(!cmds.is_empty(), "the HUD draws the panel + stats");
    }
}
