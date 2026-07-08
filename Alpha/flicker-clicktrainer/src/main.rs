//! flicker-clicktrainer: an aim / click-training game — the `square-chase` POC
//! promoted to a proper engine client.
//!
//! `square-chase` was a bare `flicker::app::App` (init/update/render) that
//! auto-fire-scored while the button was held inside one box. This is the same
//! idea grown up into an engine client:
//! - a `flicker::scene::Scene` wired into the **`flicker-shell`** front-end
//!   (intro splash → menu → *this* → pause/settings), instead of owning the
//!   window itself;
//! - **discrete** clicks (press edge, not hold) scored as hits vs. misses, with
//!   live accuracy and reaction-time readouts;
//! - a difficulty ramp — the target shrinks as you land hits — and a per-target
//!   lifetime that counts as a miss if it times out.
//!
//! Controls: left-click a target to hit it; a misclick or a timed-out target
//! costs accuracy. Esc opens the pause menu (Resume / Settings / Quit).

use anyhow::Result;
use std::time::Duration;

use flicker::app::{AbstractControls, Action, GamepadConfig, InputMap, InputState};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
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

/// The click-trainer game scene. Everything a frame needs lives here; the shell
/// drives it through the [`Scene`] trait.
struct ClickTrainer {
    /// 1×1 white pixel — the sprite shader tints it, so one texture draws the
    /// target box and its lifetime bar in any colour.
    white: Option<TextureHandle>,

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
    /// Gothic theme for the pause overlay we push (built once in `enter`).
    ui_theme: Option<Theme>,
}

impl ClickTrainer {
    fn new() -> Self {
        Self {
            white: None,
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

    /// Place a fresh target at a random fully-on-screen position, reset its clock.
    fn spawn(&mut self, screen: Vec2) {
        self.target_size = self.size_for_difficulty();
        let max_x = ((screen.x - self.target_size) as u32).max(1);
        let max_y = ((screen.y - self.target_size) as u32).max(1);
        self.target_pos = Vec2::new(
            fastrand::u32(0..max_x) as f32,
            fastrand::u32(0..max_y) as f32,
        );
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
}

impl Scene for ClickTrainer {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Theme for the pause overlay we push on Esc (built once, reused).
        self.ui_theme = Some(Theme::build(renderer));
        self.spawn(renderer.size());
        renderer.window().set_title("Flicker Click Trainer");
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let dt_s = dt.as_secs_f32();
        let screen = renderer.size();

        // Pick up any input-settings change made in the pause→settings overlay.
        // Only the bindings matter here (no camera controls to carry).
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

        // Discrete click (press edge, not hold): inside the target → a hit that
        // scores + respawns; anywhere else → a misclick (accuracy penalty, the
        // target stays).
        if input.mouse_left_pressed {
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

        // HUD: title, hit/miss/accuracy, reaction times, hint.
        let text = [0.90, 0.93, 0.97, 1.0];
        let gold = [0.85, 0.66, 0.32, 1.0];
        let dim = [0.60, 0.64, 0.70, 1.0];
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
        renderer.draw_text(
            "CLICK TRAINER — click the targets",
            Vec2::new(16.0, 16.0),
            24.0,
            gold,
        );
        renderer.draw_text(
            &format!(
                "hits {}   misses {}   accuracy {:.0}%",
                self.hits,
                self.misses,
                self.accuracy()
            ),
            Vec2::new(16.0, 48.0),
            18.0,
            text,
        );
        renderer.draw_text(
            &format!(
                "reaction   last {}   best {}   avg {}",
                react(self.last_reaction),
                react(self.best_reaction),
                react(avg)
            ),
            Vec2::new(16.0, 70.0),
            18.0,
            text,
        );
        renderer.draw_text("Esc: menu", Vec2::new(16.0, 92.0), 16.0, dim);
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "flicker_clicktrainer=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    // The shell owns the whole front-end (splash → menu → settings/pause) and the
    // winit run loop; we hand it a factory for our click-trainer scene, which
    // START launches.
    flicker_shell::run(flicker_shell::ShellConfig {
        game_scene: Box::new(|| Box::new(ClickTrainer::new())),
    })
}
