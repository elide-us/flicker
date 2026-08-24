//! flicker-jiggle: **Jiggle Bucket** — a soft-body merge-dropper (Suika-style) and
//! the reference **2D pressurized soft-body** client. Balls are pressurized soft
//! bodies ([`sim`]) that wobble, squish, and jostle; two of the SAME tier that touch
//! and settle merge into the next size. You drag the current ball along a rail at the
//! top and release to drop it straight down; equal balls merged in one cascade score a
//! rising COMBO multiplier; unlocking a never-seen tier scales the whole field down a
//! notch for headroom. Push as high as you dare, then CASH OUT to bank the run — let a
//! ball overflow the rim first and the run's points are gone.
//!
//! Architecture (five-line, 491BD9BB): the sim + all game logic + drawing are compiled
//! Rust (security law 69E82FE7 — the client is in the enemy's hands); `jiggle.scene.json`
//! is the HUD tree + anchors; `jiggle.lua` derives the HUD display strings; colours ride
//! `ui_theme.json` tokens. Input is signals only (37722F91): the rail-drop is the
//! `PrimaryAction` signal (press grabs, release drops); aim is `StrafeLeft`/`StrafeRight`
//! plus the pointer POSITION sample, never a raw button and never a pointer delta.
//! Modelled on clicktrainer (a 2D game under a vector HUD, with clicks routed between).

use std::collections::HashSet;
use std::time::Duration;

use flicker::render::{FontRole, FrameGraph, Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_input_core::{AbstractControls, ActionSignal, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

mod route;
use route::{GameplayBase, RootHandler};
mod sim;
use sim::{find_merges, simulate, Ball, Bucket, Tier};

// ── Tuning ───────────────────────────────────────────────────────────
const TAPER: f32 = 0.62; // bucket bottom width / top width (\___/)
const BALL_FULL_FRAC: f32 = 0.22; // "100%" ball radius as a fraction of bucket top width
const SIZE_BASE: f32 = 0.70; // tier-0 ball radius = 70% of full
const SIZE_STEP: f32 = 0.05; // +5% of full per tier — a gentle ladder (may exceed 100%)
const START_UNLOCKED: usize = 2; // tiers 1–3 (indices 0,1,2) are unlocked from the start
const RNG_CAP: usize = 5; // the RNG never hands you a ball above this tier (first SIX tiers)
const RNG_DECAY: f32 = 0.5; // each higher unlocked tier is this× as likely (log-ish falloff)
const ZOOM_STEP: f32 = 0.95; // field shrink per new-tier unlock — small, so higher tiers stay TIGHT
const DROP_COOL: f32 = 0.34; // seconds after a drop before the next ball loads
const COMBO_WINDOW: f32 = 0.7; // a merge within this of the last keeps the combo alive
const MERGE_COOLDOWN: f32 = 0.3; // sim-seconds a merged ball waits before it can merge again
const CHAIN_TTL: f32 = 0.9; // sim-seconds a merge product stays "hot" to continue a combo
const NUDGE: f32 = 16.0; // px the keyboard/pad aim moves per Strafe press
const SIM_DT: f32 = 1.0 / 120.0; // fixed simulation step
const TIME_SCALE: f32 = 0.8; // sim time / real time — the ANIMATION SPEED dial (<1 = slow-mo)
const POPUP_TTL: f32 = 1.4; // seconds a combo/score popup lives
const POPUP_RISE: f32 = 110.0; // px/s a popup floats upward as it fades
const POPUP_SIZE: f32 = 60.0; // base score-popup font size (display face)
const POPUP_COMBO_SIZE: f32 = 120.0; // combo (×2+) popup font size — big and loud

/// The scene's PAIR SCRIPT (`jiggle.lua`) — HUD display derivations.
const JIGGLE_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/jiggle.lua");

/// The tier ladder — colour + physical personality + payout per size. This is the 2D
/// game's render/gameplay data, held in Rust like clicktrainer's `CALM`/`URGENT` target
/// colours (drawing code, not UI-chrome styling). Personalities cycle
/// bouncy→slippery→rolly→sticky; the top two are also heavy (dense), so they squish
/// smaller balls when dropped on them.
const TIERS: [Tier; 10] = [
    Tier {
        color: [1.00, 0.18, 0.18, 1.0],
        rest: 0.62,
        fric: 0.05,
        stick: 0.00,
        dens: 1.0,
        press: 1.10,
        score: 100,
    }, // red · bouncy
    Tier {
        color: [1.00, 0.55, 0.10, 1.0],
        rest: 0.12,
        fric: 0.00,
        stick: 0.00,
        dens: 1.0,
        press: 1.05,
        score: 300,
    }, // orange · slippery
    Tier {
        color: [1.00, 0.82, 0.10, 1.0],
        rest: 0.28,
        fric: 0.10,
        stick: 0.00,
        dens: 1.05,
        press: 1.08,
        score: 600,
    }, // yellow · rolly
    Tier {
        color: [0.22, 0.88, 0.29, 1.0],
        rest: 0.02,
        fric: 0.70,
        stick: 0.90,
        dens: 1.15,
        press: 1.00,
        score: 1000,
    }, // green · sticky
    Tier {
        color: [0.08, 0.85, 0.77, 1.0],
        rest: 0.62,
        fric: 0.05,
        stick: 0.00,
        dens: 1.0,
        press: 1.10,
        score: 1500,
    }, // teal · bouncy
    Tier {
        color: [0.18, 0.48, 1.00, 1.0],
        rest: 0.12,
        fric: 0.00,
        stick: 0.00,
        dens: 1.0,
        press: 1.05,
        score: 2100,
    }, // blue · slippery
    Tier {
        color: [0.55, 0.23, 1.00, 1.0],
        rest: 0.28,
        fric: 0.10,
        stick: 0.00,
        dens: 1.6,
        press: 1.08,
        score: 2800,
    }, // purple · rolly · heavy
    Tier {
        color: [1.00, 0.23, 0.82, 1.0],
        rest: 0.02,
        fric: 0.70,
        stick: 0.90,
        dens: 1.7,
        press: 1.00,
        score: 3600,
    }, // magenta · sticky · heavy
    Tier {
        color: [1.00, 0.40, 0.60, 1.0],
        rest: 0.55,
        fric: 0.05,
        stick: 0.00,
        dens: 1.75,
        press: 1.10,
        score: 4500,
    }, // rose · bouncy · heavy
    Tier {
        color: [0.95, 0.96, 1.00, 1.0],
        rest: 0.10,
        fric: 0.02,
        stick: 0.00,
        dens: 1.85,
        press: 1.05,
        score: 5500,
    }, // platinum · top (stays)
];

// Play-field chrome colours (drawing code — the arcade felt, not UI theme tokens).
const INTERIOR: [f32; 4] = [0.06, 0.07, 0.10, 1.0];
const WALL: [f32; 4] = [0.24, 0.27, 0.36, 1.0];
const RAIL: [f32; 4] = [0.28, 0.31, 0.42, 1.0];
const GUIDE: [f32; 4] = [1.0, 0.71, 0.33, 0.30];

/// A weighted random tier in `0..=cap`, higher tiers exponentially less likely
/// ([`RNG_DECAY`] per step — the log-ish falloff). `cap` is the current unlock ceiling.
fn rand_tier(cap: usize) -> usize {
    let cap = cap.min(TIERS.len() - 1);
    let mut total = 0.0;
    for t in 0..=cap {
        total += RNG_DECAY.powi(t as i32);
    }
    let mut r = fastrand::f32() * total;
    for t in 0..=cap {
        let w = RNG_DECAY.powi(t as i32);
        if r < w {
            return t;
        }
        r -= w;
    }
    0
}

/// A transient floating score number spawned at a merge (combos get a bigger gold one).
struct Popup {
    pos: Vec2,
    gained: u32,
    mult: u32,
    age: f32,
}

/// A drifting background bubble — pure decoration behind the play field.
struct Bubble {
    pos: Vec2,
    r: f32,
    speed: f32,
}

/// A static colorful ball in the ball-pit background (like the playable ones).
struct PitBall {
    pos: Vec2,
    r: f32,
    color: [f32; 4],
    seed: u32,
}

/// One persisted high-score entry (top-five, in `content/data/buckets_scores.json`).
struct HighScore {
    name: String,
    score: u32,
}

/// The five seeded default records.
fn default_scores() -> Vec<HighScore> {
    [500_000u32, 400_000, 300_000, 200_000, 100_000]
        .into_iter()
        .map(|score| HighScore {
            name: "Record".into(),
            score,
        })
        .collect()
}

/// The on-disk scores file — the content DATA dir (mutable user data, DATA PLACEMENT LAW).
fn scores_path() -> std::path::PathBuf {
    flicker::core::roots::roots()
        .data()
        .join("buckets_scores.json")
}

/// Load the top-five high scores, falling back to the seeded defaults if absent/broken.
fn load_scores() -> Vec<HighScore> {
    let mut list = std::fs::read_to_string(scores_path())
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|e| {
                        Some(HighScore {
                            name: e.get("name")?.as_str()?.to_string(),
                            score: e.get("score")?.as_u64()? as u32,
                        })
                    })
                    .collect::<Vec<_>>()
            })
        })
        .filter(|l| !l.is_empty())
        .unwrap_or_else(default_scores);
    list.sort_by_key(|h| std::cmp::Reverse(h.score));
    list.truncate(5);
    list
}

/// Jiggle Bucket — the soft-body merge game. The shell drives it through [`Scene`].
pub struct Jiggle {
    // ── HUD plumbing (mirrors clicktrainer) ──
    white: Option<TextureHandle>,
    authored: Option<UiNode>,
    scene_styles: Option<serde_json::Value>,
    script: Option<ScriptHost>,
    ui_tree: Option<UiNode>,
    ui_intents: UiIntents,
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    ui_theme: Option<Theme>,
    ui_state: UiState,
    fired_sigs: Vec<String>,

    // ── game ──
    bucket: Option<Bucket>,
    last_screen: Vec2,
    balls: Vec<Ball>,
    next_id: u32,
    current: usize,
    next: usize,
    rail_x: f32,
    grabbing: bool,
    drop_cool: f32,
    /// Id of the last ball dropped — the drop gate holds until it LANDS (touches a
    /// surface or another ball), so you can't machine-gun balls onto the rail.
    last_drop_id: Option<u32>,
    /// True from a drop until that ball lands; the rail stays empty and current/next do
    /// not advance until it clears.
    awaiting_land: bool,
    size_scale: f32,
    highest_unlocked: usize,
    acc: f32,
    score: u32,
    best: u32,
    combo: u32,
    combo_timer: f32,
    running: bool,
    /// Conservation ledger: Σ 2^tier over every ball that SHOULD be present. A drop adds
    /// 2^(dropped tier); a merge conserves it (2·2^k = 2^(k+1)). If the live total ever
    /// drifts from this while running, a ball was silently dropped — we log it loudly.
    units_expected: u64,
    /// Floating "+score ×combo" numbers, spawned at each merge, that rise and fade.
    popups: Vec<Popup>,
    /// Drifting background bubbles (decoration).
    bubbles: Vec<Bubble>,
    /// The ball-pit background — a screen-filling field of colorful shiny balls.
    pit: Vec<PitBall>,
    /// Persisted top-five high scores.
    high_scores: Vec<HighScore>,
}

impl Jiggle {
    pub fn new(def: &SceneDef) -> Self {
        Self {
            white: None,
            authored: def.tree.clone(),
            scene_styles: def.styles.clone(),
            script: match ScriptHost::new(JIGGLE_SCRIPT, "jiggle.lua") {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::error!("jiggle.lua failed to load — raw HUD values only: {e}");
                    None
                }
            },
            ui_tree: None,
            ui_intents: UiIntents::default(),
            ui_styles: serde_json::Value::Object(Default::default()),
            hud_commands: Vec::new(),
            ui_theme: None,
            ui_state: UiState::new(),
            fired_sigs: Vec::new(),
            bucket: None,
            last_screen: Vec2::ZERO,
            balls: Vec::new(),
            next_id: 1,
            current: 0,
            next: 0,
            rail_x: 0.0,
            grabbing: false,
            drop_cool: 0.0,
            last_drop_id: None,
            awaiting_land: false,
            size_scale: 1.0,
            highest_unlocked: START_UNLOCKED,
            acc: 0.0,
            score: 0,
            best: 0,
            combo: 0,
            combo_timer: 0.0,
            running: true,
            units_expected: 0,
            popups: Vec::new(),
            bubbles: Vec::new(),
            pit: Vec::new(),
            high_scores: Vec::new(),
        }
    }

    /// Seed the drifting background bubbles across the screen.
    fn spawn_bubbles(&mut self, screen: Vec2) {
        self.bubbles.clear();
        for _ in 0..24 {
            self.bubbles.push(Bubble {
                pos: Vec2::new(fastrand::f32() * screen.x, fastrand::f32() * screen.y),
                r: 6.0 + fastrand::f32() * 26.0,
                speed: 12.0 + fastrand::f32() * 26.0,
            });
        }
    }

    /// Fill the background with a hex-packed pit of colorful shiny balls.
    fn spawn_pit(&mut self, screen: Vec2) {
        self.pit.clear();
        let step = (screen.x * 0.10).clamp(104.0, 160.0);
        let mut y = -step * 0.5;
        let mut row = 0u32;
        while y < screen.y + step {
            let xoff = if row.is_multiple_of(2) {
                0.0
            } else {
                step * 0.5
            };
            let mut x = -step * 0.5 + xoff;
            while x < screen.x + step {
                let tier = (fastrand::f32() * TIERS.len() as f32) as usize % TIERS.len();
                self.pit.push(PitBall {
                    pos: Vec2::new(
                        x + (fastrand::f32() - 0.5) * step * 0.2,
                        y + (fastrand::f32() - 0.5) * step * 0.2,
                    ),
                    r: step * (0.42 + fastrand::f32() * 0.12),
                    color: TIERS[tier].color,
                    seed: fastrand::u32(..),
                });
                x += step;
            }
            y += step * 0.86;
            row += 1;
        }
    }

    fn tier_radius(&self, t: usize) -> f32 {
        // Gentle ladder tied to the bucket width, so the play ratio holds at any size.
        let full = self
            .bucket
            .as_ref()
            .map_or(40.0, |b| b.top_width() * BALL_FULL_FRAC);
        full * (SIZE_BASE + SIZE_STEP * t as f32) * self.size_scale
    }

    fn rail_y(&self) -> f32 {
        // Well ABOVE the bucket — the ball starts high and drops a long way in.
        (self.last_screen.y * 0.11).max(56.0)
    }

    /// The RNG spawn ceiling: only tiers you've UNLOCKED (produced by a merge), capped.
    fn spawn_cap(&self) -> usize {
        self.highest_unlocked.min(RNG_CAP)
    }

    /// Start a fresh run — clears the field and resets the score/zoom.
    fn new_game(&mut self) {
        self.balls.clear();
        self.popups.clear();
        self.next_id = 1;
        self.score = 0;
        self.combo = 0;
        self.combo_timer = 0.0;
        self.size_scale = 1.0;
        self.highest_unlocked = START_UNLOCKED;
        self.units_expected = 0;
        self.drop_cool = 0.0;
        self.grabbing = false;
        self.last_drop_id = None;
        self.awaiting_land = false;
        self.acc = 0.0;
        self.running = true;
        self.current = rand_tier(self.spawn_cap());
        self.next = rand_tier(self.spawn_cap());
        if let Some(b) = &self.bucket {
            let (l, r) = b.rail_span();
            self.rail_x = (l + r) * 0.5;
        }
    }

    /// Drop the current ball straight down from the rail, then load the next.
    fn drop_ball(&mut self) {
        let r = self.tier_radius(self.current);
        let pos = Vec2::new(self.rail_x, self.rail_y());
        let id = self.next_id;
        let ball = Ball::new(id, self.current, &TIERS[self.current], pos, r);
        self.next_id += 1;
        self.balls.push(ball);
        self.last_drop_id = Some(id);
        self.awaiting_land = true;
        self.units_expected += 1u64 << self.current; // ledger: a tier-k drop adds 2^k units
        self.drop_cool = DROP_COOL;
        // current/next advance only when the dropped ball LANDS (see update()).
    }

    /// Bank the run into the persisted high scores (as "Player") and end it safely.
    fn cash_out(&mut self) {
        if self.running && self.score > 0 {
            self.best = self.best.max(self.score); // your own best cashed run this session
            self.record_score(self.score);
            self.running = false;
        }
    }

    /// Insert a cashed score into the top-five (named "Player"), refresh best, persist.
    fn record_score(&mut self, score: u32) {
        self.high_scores.push(HighScore {
            name: "Player".into(),
            score,
        });
        self.high_scores.sort_by_key(|h| std::cmp::Reverse(h.score));
        self.high_scores.truncate(5);
        self.save_scores();
    }

    /// Write the top-five to `content/data/buckets_scores.json` (best-effort).
    fn save_scores(&self) {
        let arr: Vec<serde_json::Value> = self
            .high_scores
            .iter()
            .map(|h| serde_json::json!({ "name": h.name, "score": h.score }))
            .collect();
        match serde_json::to_string_pretty(&serde_json::Value::Array(arr)) {
            Ok(s) => {
                if let Err(e) = std::fs::write(scores_path(), s) {
                    tracing::warn!("bucket-of-suds high-score save failed: {e}");
                }
            }
            Err(e) => tracing::warn!("bucket-of-suds high-score encode failed: {e}"),
        }
    }

    /// End the run in failure — the rim overflowed, so the run's points are forfeit.
    fn lose(&mut self) {
        self.running = false;
        self.score = 0;
        self.grabbing = false;
    }

    /// Resolve every same-tier contact into a merge: pop both, spawn the next size at
    /// the midpoint, score it × the live combo, and zoom the field on a new unlock.
    fn resolve_merges(&mut self) {
        let pairs = find_merges(&self.balls);
        if pairs.is_empty() {
            return;
        }
        let mut consumed: HashSet<u32> = HashSet::new();
        let mut spawns: Vec<Ball> = Vec::new();
        let mut unlock = false;
        for (a, b) in pairs {
            if consumed.contains(&a) || consumed.contains(&b) {
                continue; // one of the pair already merged this frame
            }
            let (Some(ia), Some(ib)) = (
                self.balls.iter().position(|x| x.id == a),
                self.balls.iter().position(|x| x.id == b),
            ) else {
                continue;
            };
            let tier = self.balls[ia].tier;
            // The top tier is the MAX MERGE — two of them do NOT merge, they just stay.
            if tier + 1 >= TIERS.len() {
                continue;
            }
            consumed.insert(a);
            consumed.insert(b);
            let mid = (self.balls[ia].centroid() + self.balls[ib].centroid()) * 0.5;
            // Combo = CASCADE DEPTH (lineage + hot-window), not a time window and not the
            // tier: the product's chain is one deeper than its deepest STILL-HOT parent, so
            // only a real chain reaction (products merging before they cool) climbs the
            // multiplier — two settled balls merging is always ×1.
            let chain = self.balls[ia].chain.max(self.balls[ib].chain) + 1;
            let mult = chain;
            self.combo = mult; // HUD readout (fades via combo_timer)
            self.combo_timer = COMBO_WINDOW;
            let gained = TIERS[tier].score * mult;
            self.score += gained;
            self.popups.push(Popup {
                pos: mid,
                gained,
                mult,
                age: 0.0,
            });
            let r = self.tier_radius(tier + 1);
            let mut nb = Ball::new(self.next_id, tier + 1, &TIERS[tier + 1], mid, r);
            nb.compress(0.72); // gentle merge morph-in (less spring on nearby smaller balls)
            nb.merge_cd = MERGE_COOLDOWN; // …and can't merge again for a beat (staged cascade)
            nb.chain = chain; // carry the cascade depth forward…
            nb.chain_ttl = CHAIN_TTL; // …but only while it stays hot
            spawns.push(nb);
            self.next_id += 1;
            if tier + 1 > self.highest_unlocked {
                self.highest_unlocked = tier + 1;
                unlock = true;
            }
        }
        self.balls.retain(|x| !consumed.contains(&x.id));
        self.balls.extend(spawns);
        if unlock {
            self.size_scale *= ZOOM_STEP;
            for b in &mut self.balls {
                b.shrink(ZOOM_STEP);
            }
        }
    }

    /// Lose when a settled ball rests above the rim for a grace period, or instantly if
    /// one escapes the top entirely.
    /// End the run only when a ball has fallen OUT of the bucket and off the screen (or
    /// the sim blew up). Rising above the rim is NOT a failure — balls pile above the
    /// open mouth; you lose only if one spills over a lip and drops away.
    fn check_overflow(&mut self, _dt: f32) {
        let screen = self.last_screen;
        for b in &self.balls {
            let c = b.centroid();
            if !c.is_finite() || c.y > screen.y + 60.0 || c.x < -80.0 || c.x > screen.x + 80.0 {
                self.lose();
                return;
            }
        }
    }

    /// The per-frame HUD model — raw numbers only; `jiggle.lua` derives the display
    /// strings (five-line split), so no display copy is published from Rust.
    fn hud_model(&self) -> ValueMap {
        let raw = ValueMap::new()
            .with("score", f64::from(self.score))
            .with("best", f64::from(self.best))
            .with("combo", f64::from(self.combo));
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("jiggle: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("jiggle.lua derive() failed: {e}"),
            }
        }
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }
}

/// Multiply an RGBA colour's RGB by `f` (alpha kept), clamped to 1.0 — the cheap
/// shading term that turns a flat fill into a lit jelly (bright top, dark rim).
fn mul(c: [f32; 4], f: f32) -> [f32; 4] {
    [
        (c[0] * f).min(1.0),
        (c[1] * f).min(1.0),
        (c[2] * f).min(1.0),
        c[3],
    ]
}

/// Push a colored thick line segment (as two triangles) between `a` and `b`.
fn push_stroke(tris: &mut Vec<([Vec2; 3], [f32; 4])>, a: Vec2, b: Vec2, w: f32, col: [f32; 4]) {
    let dir = (b - a).normalize_or_zero();
    let n = Vec2::new(-dir.y, dir.x) * (w * 0.5);
    let (p0, p1, p2, p3) = (a + n, b + n, b - n, a - n);
    tris.push(([p0, p1, p2], col));
    tris.push(([p0, p2, p3], col));
}

impl Scene for Jiggle {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.ui_theme = Some(Theme::build(renderer));
        self.ui_styles = flicker::ui::load_shared_styles(self.scene_styles.as_ref());
        match self.authored.clone() {
            Some(tree) => {
                self.ui_intents = UiIntents::of(&tree);
                self.ui_tree = Some(tree);
            }
            None => tracing::error!("Jiggle's scene file has no `tree` — no HUD"),
        }
        let screen = renderer.size();
        self.last_screen = screen;
        self.bucket = Some(Bucket::new(screen, TAPER));
        self.spawn_bubbles(screen);
        self.spawn_pit(screen);
        self.new_game();
        self.high_scores = load_scores();
        renderer.window().set_title("Flicker · Bucket o' Suds");
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        let dt_s = dt.as_secs_f32();
        let screen = renderer.size();
        if screen != self.last_screen || self.bucket.is_none() {
            self.bucket = Some(Bucket::new(screen, TAPER));
            self.last_screen = screen;
            self.spawn_bubbles(screen);
            self.spawn_pit(screen);
        }

        // ── HUD walk (layout + hit-test + draw); routes clicks off the game field ──
        let mut over_hud = false;
        if let Some(tree) = self.ui_tree.as_ref() {
            let model = self.hud_model();
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
            over_hud = frame.results.is_on("hud_hit");
            if frame.results.is_on("cashout") {
                self.cash_out();
            }
            if frame.results.is_on("newgame") {
                self.new_game();
            }
            self.hud_commands = frame.commands;
        }

        // ── Input seam: the pump resolved this frame's events; route them (signals) ──
        self.fired_sigs.clear();
        let mut root = RootHandler;
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        let mut gameplay = GameplayBase::default();
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        self.fired_sigs = walker.take_fired();
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.ui_theme.expect("theme built in enter");
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

        // ── Rail control — SIGNALS only (PrimaryAction grab/release + Strafe/Confirm),
        // pointer POSITION sample for the aim x. No raw button, no delta (37722F91). ──
        let primary_held = signals.held(ActionSignal::PrimaryAction, input);
        if let Some(bucket) = self.bucket.as_ref() {
            let (lo, hi) = bucket.rail_span(); // ball CENTER can reach the mouth edges
                                               // Drop gate: hold until the last dropped ball has LANDED (see the advance below).
            let ready = self.running && self.drop_cool <= 0.0 && !self.awaiting_land;
            if gameplay.grab && ready && !self.grabbing {
                self.grabbing = true;
            }
            if self.grabbing {
                self.rail_x = input.mouse_position.x.clamp(lo, hi);
                if !primary_held {
                    self.grabbing = false;
                    if ready {
                        self.drop_ball();
                    }
                }
            }
            if gameplay.nudge != 0.0 {
                self.rail_x = (self.rail_x + gameplay.nudge * NUDGE).clamp(lo, hi);
            }
            if gameplay.confirm && ready && !self.grabbing {
                self.drop_ball();
            }
        }

        // ── Simulate (fixed step) ──
        self.drop_cool = (self.drop_cool - dt_s).max(0.0);
        if self.combo_timer > 0.0 {
            self.combo_timer = (self.combo_timer - dt_s).max(0.0);
            if self.combo_timer <= 0.0 {
                self.combo = 0;
            }
        }
        if self.running {
            self.acc += (dt_s * TIME_SCALE).min(0.1); // slow-mo: fewer sim steps per real second
            let mut steps = 0;
            while self.acc >= SIM_DT && steps < 6 {
                {
                    let bucket = self.bucket.as_ref().expect("bucket built in enter");
                    simulate(&mut self.balls, bucket, SIM_DT, TIERS.len() - 1);
                }
                self.resolve_merges();
                self.check_overflow(SIM_DT);
                self.acc -= SIM_DT;
                steps += 1;
            }
            if self.acc > SIM_DT * 6.0 {
                self.acc = 0.0;
            }
            // Conservation guard — nothing should silently vanish. Escaped balls stay in
            // the list (counted) until they leave the screen and end the run, so a real
            // drift here means a genuine drop bug, not an escape. Fail loud (resync so it
            // warns once).
            if self.running {
                let actual: u64 = self.balls.iter().map(|b| 1u64 << b.tier).sum();
                if actual != self.units_expected {
                    tracing::error!(
                        "jiggle conservation drift: {actual} live tier-units vs {} expected \
                         — a ball was dropped",
                        self.units_expected
                    );
                    self.units_expected = actual;
                }
            }
        }

        // Advance the rail only once the dropped ball has LANDED (touched a surface or
        // another ball) — or vanished (merged). Until then the rail is empty and the
        // "next" preview does not roll forward.
        if self.awaiting_land {
            let landed = match self
                .last_drop_id
                .and_then(|id| self.balls.iter().find(|b| b.id == id))
            {
                Some(b) => b.contacted,
                None => true,
            };
            if landed {
                self.current = self.next;
                self.next = rand_tier(self.spawn_cap());
                self.awaiting_land = false;
            }
        }

        // Combo/score popups rise and fade (real time, so they finish even after a loss).
        for p in &mut self.popups {
            p.age += dt_s;
            p.pos.y -= POPUP_RISE * dt_s;
        }
        self.popups.retain(|p| p.age < POPUP_TTL);

        // Background bubbles drift upward (ambient, real time) and wrap around.
        let sb = self.last_screen;
        for b in &mut self.bubbles {
            b.pos.y -= b.speed * dt_s;
            if b.pos.y < -b.r {
                b.pos.y = sb.y + b.r;
                b.pos.x = fastrand::f32() * sb.x;
            }
        }

        Transition::None
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        let Some(white) = self.white else {
            return;
        };
        let Some(bucket) = self.bucket.as_ref() else {
            return;
        };
        let screen = renderer.size();

        let mut tris: Vec<([Vec2; 3], [f32; 4])> = Vec::new();
        // Bucket interior wash (trapezoid) + walls + floor.
        tris.push(([bucket.tl, bucket.tr, bucket.br], INTERIOR));
        tris.push(([bucket.tl, bucket.br, bucket.bl], INTERIOR));
        push_stroke(&mut tris, bucket.tl, bucket.bl, 7.0, WALL);
        push_stroke(&mut tris, bucket.tr, bucket.br, 7.0, WALL);
        push_stroke(&mut tris, bucket.bl, bucket.br, 7.0, WALL);

        // Each ball is drawn ENTIRELY from its deformed membrane outline, so the squish is
        // always visible: a top-lit gradient fill over the real silhouette, a smaller copy
        // of that same silhouette as the glossy core (nudged up, brighter), a dark rim that
        // follows the squish, then a specular sparkle (added later, on the layer above).
        let mut bodies: Vec<(Vec2, f32, u32)> = Vec::new(); // (centroid, reff, id)
        for b in &self.balls {
            let c = b.centroid();
            let col = TIERS[b.tier].color;
            let n = b.n();
            // Vertical extent → a cheap top-lit gradient that RIDES the deformed shape.
            let (mut y0, mut y1) = (f32::MAX, f32::MIN);
            for nd in &b.nodes {
                y0 = y0.min(nd.pos.y);
                y1 = y1.max(nd.pos.y);
            }
            let inv_h = 1.0 / (y1 - y0).max(1.0);
            for i in 0..n {
                let (p, q) = (b.nodes[i].pos, b.nodes[(i + 1) % n].pos);
                let t = ((p.y + q.y) * 0.5 - y0) * inv_h; // 0 at top → 1 at bottom
                tris.push(([c, p, q], mul(col, 1.28 - 0.72 * t)));
            }
            // Glossy core: the SAME outline at 0.55×, nudged up, bright — a highlight that
            // deforms with the bubble instead of a round disc hiding the squish.
            let hi = c - Vec2::new(0.0, b.reff * 0.14);
            let bright = mul(col, 1.5);
            for i in 0..n {
                let p = hi + (b.nodes[i].pos - c) * 0.55;
                let q = hi + (b.nodes[(i + 1) % n].pos - c) * 0.55;
                tris.push(([hi, p, q], bright));
            }
            let rim = (b.reff * 0.06).max(2.0);
            for i in 0..n {
                push_stroke(
                    &mut tris,
                    b.nodes[i].pos,
                    b.nodes[(i + 1) % n].pos,
                    rim,
                    mul(col, 0.5),
                );
            }
            bodies.push((c, b.reff, b.id));
        }

        // Rail + aim guide (the point is to aim: show where it will fall).
        let cur_r = self.tier_radius(self.current);
        let rail_y = self.rail_y();
        let rail_x = self.rail_x.clamp(bucket.tl.x, bucket.tr.x); // center reaches the mouth edges
        push_stroke(
            &mut tris,
            Vec2::new(bucket.tl.x, rail_y),
            Vec2::new(bucket.tr.x, rail_y),
            4.0,
            RAIL,
        );
        if self.running && !self.awaiting_land {
            // Aim guide: rail down to the MOUTH only — it doesn't run into the bucket.
            push_stroke(
                &mut tris,
                Vec2::new(rail_x, rail_y),
                Vec2::new(rail_x, bucket.rim_y),
                2.0,
                GUIDE,
            );
        }

        // Captured, owned draw data for the root-surface pass (no borrow of self).
        let cur_col = TIERS[self.current].color;
        let next_col = TIERS[self.next].color;
        // Show the rail ball only when a drop is allowed (drop gate).
        let show_current = self.running && self.drop_cool <= 0.0 && !self.awaiting_land;
        // "Next Ball" box, top-right: a labeled panel with the upcoming ball inside.
        let nb_w = (screen.x * 0.10).clamp(96.0, 140.0);
        let nb_h = nb_w * 0.92;
        let nb_x = screen.x * 0.75 - nb_w * 0.5; // anchored at 3/4 width, clear of the tier column
        let nb_y = 20.0;
        let nb_label = flicker::ui::strings::resolve("$jiggle_next_ball").into_owned();
        let nb_label_size = (nb_w * 0.15).clamp(13.0, 18.0);
        let next_c = Vec2::new(nb_x + nb_w * 0.5, nb_y + nb_h * 0.58);
        let next_r = self.tier_radius(self.next).min(nb_w * 0.30);

        // Progression column (right side): a disc per tier — coloured once UNLOCKED (a
        // merge has produced it), else a black disc with a "?". Tier 0 starts unlocked;
        // this is also the RNG pool (capped at RNG_CAP).
        let col_r = (screen.x * 0.014).clamp(14.0, 22.0);
        let col_x = screen.x - col_r - 22.0;
        let col_gap = col_r * 2.5;
        let mut counts = [0u32; TIERS.len()];
        for b in &self.balls {
            counts[b.tier] += 1;
        }
        let mut column: Vec<(Vec2, [f32; 4], u32)> = Vec::new();
        let mut locked: Vec<Vec2> = Vec::new();
        for (t, tier) in TIERS.iter().enumerate() {
            let cc = Vec2::new(col_x, 150.0 + t as f32 * col_gap);
            if t <= self.highest_unlocked {
                column.push((cc, tier.color, counts[t]));
            } else {
                locked.push(cc);
            }
        }
        let q_size = col_r * 1.15;
        let q_w = renderer.measure_text("?", q_size).x;

        // Combo/score popups — bigger + gold when it's a real combo (×2+).
        let popups: Vec<(Vec2, String, f32, [f32; 4])> = self
            .popups
            .iter()
            .map(|p| {
                let a = (1.0 - p.age / POPUP_TTL).clamp(0.0, 1.0);
                if p.mult >= 2 {
                    (
                        p.pos,
                        format!("+{} ×{}", p.gained, p.mult),
                        POPUP_COMBO_SIZE,
                        [1.0, 0.86, 0.30, a],
                    )
                } else {
                    (
                        p.pos,
                        format!("+{}", p.gained),
                        POPUP_SIZE,
                        [0.95, 0.97, 1.0, a],
                    )
                }
            })
            .collect();
        let bubbles: Vec<(Vec2, f32)> = self.bubbles.iter().map(|b| (b.pos, b.r)).collect();
        let pit: Vec<(Vec2, f32, [f32; 4], u32)> = self
            .pit
            .iter()
            .map(|p| (p.pos, p.r, p.color, p.seed))
            .collect();
        let hs_label = flicker::ui::strings::resolve("$jiggle_high_scores").into_owned();
        let hs: Vec<(String, String)> = self
            .high_scores
            .iter()
            .map(|h| (h.name.clone(), h.score.to_string()))
            .collect();

        // Translucent frames behind the tier list + the dropper rack — same look as the
        // Next Bubble panel (list matches its WIDTH, rack matches its HEIGHT) for feng shui.
        let list_rect = (
            Vec2::new(col_x + col_r + 14.0 - nb_w, 150.0 - col_r - 14.0),
            Vec2::new(nb_w, 9.0 * col_gap + col_r * 2.0 + 28.0),
        );
        let rack_pad = 18.0;
        let rack_rect = (
            Vec2::new(bucket.tl.x - rack_pad, rail_y - nb_h * 0.5),
            Vec2::new((bucket.tr.x - bucket.tl.x) + 2.0 * rack_pad, nb_h),
        );

        fg.root(move |r| {
            let base = r.layer();
            let dot = |r: &mut Renderer, c: Vec2, rad: f32, alpha: f32| {
                r.draw_ui_panel(
                    Vec2::new(c.x - rad, c.y - rad),
                    Vec2::splat(rad * 2.0),
                    [1.0, 1.0, 1.0, alpha],
                    [1.0, 1.0, 1.0, alpha],
                    0.0,
                    rad,
                    0.0,
                    [1.0; 4],
                    2.0,
                );
            };
            // Per-ball VARIED sheen: main highlight (upper hemisphere) + 0–2 sparkles, all
            // deterministic from a seed — every ball looks different, stable frame to frame.
            let specular = |r: &mut Renderer, c: Vec2, rad: f32, seed: u32| {
                let h = seed.wrapping_mul(2_654_435_761);
                let f = |shift: u32| ((h >> shift) & 0xff) as f32 / 255.0;
                let ang = -std::f32::consts::FRAC_PI_2 + (f(0) - 0.5) * 1.7;
                let off = rad * (0.30 + f(8) * 0.16);
                let mc = c + Vec2::new(ang.cos(), ang.sin()) * off;
                dot(r, mc, rad * (0.28 + f(16) * 0.16), 0.6);
                for k in 0..((h >> 5) % 3) {
                    let sa = f(24) * std::f32::consts::TAU + k as f32 * 2.4;
                    let sd = rad * (0.34 + 0.16 * k as f32);
                    let sc = c + Vec2::new(sa.cos(), sa.sin()) * sd;
                    dot(r, sc, rad * (0.09 + 0.05 * f(2)), 0.32);
                }
            };
            // Ball-pit background: colorful shiny balls filling the screen, very back.
            for (pos, pr, col, seed) in &pit {
                r.draw_ui_panel(
                    Vec2::new(pos.x - pr, pos.y - pr),
                    Vec2::splat(pr * 2.0),
                    mul(*col, 1.05),
                    mul(*col, 0.6),
                    1.0,
                    *pr,
                    0.0,
                    [1.0; 4],
                    1.0,
                );
                specular(r, *pos, *pr, *seed);
            }
            // Background bubbles (faint, drifting) — over the pit.
            for (pos, br) in &bubbles {
                r.draw_ui_panel(
                    Vec2::new(pos.x - br, pos.y - br),
                    Vec2::splat(br * 2.0),
                    [0.40, 0.62, 0.95, 0.06],
                    [0.30, 0.50, 0.85, 0.03],
                    1.0,
                    *br,
                    1.5,
                    [0.60, 0.80, 1.0, 0.22],
                    1.0,
                );
            }
            // 70% mask over the busy background so the game pieces read clearly.
            r.draw_ui_panel(
                Vec2::ZERO,
                screen,
                [0.14, 0.15, 0.19, 0.7],
                [0.14, 0.15, 0.19, 0.7],
                0.0,
                0.0,
                0.0,
                [1.0; 4],
                0.0,
            );
            // Feng-shui frames behind the tier list + dropper rack (over the mask, under
            // the rail line and the discs).
            for (pos, size) in [list_rect, rack_rect] {
                r.draw_ui_panel(
                    pos,
                    size,
                    [0.10, 0.11, 0.15, 0.5],
                    [0.05, 0.05, 0.08, 0.5],
                    1.0,
                    8.0,
                    1.5,
                    [0.36, 0.38, 0.46, 0.7],
                    1.0,
                );
            }
            for (p, c) in &tris {
                r.draw_triangle(p[0], p[1], p[2], *c);
            }
            // Specular sparkles (UI batch) go a layer ABOVE the deformed fans: the renderer
            // draws the UI batch UNDER triangles within a layer, so without this the fill
            // would hide the sheen. The gradient fill + glossy core (both deformed) already
            // carry the lit-jelly look; only the tiny sparkle dots live up here now.
            r.set_layer(base + 1.0);
            for (c, reff, id) in &bodies {
                specular(r, *c, *reff, *id);
            }
            // Rail + next preview: a full glossy jelly (own gradient + rim + specular).
            let jelly = |r: &mut Renderer, c: Vec2, rad: f32, col: [f32; 4], seed: u32| {
                r.draw_ui_panel(
                    Vec2::new(c.x - rad, c.y - rad),
                    Vec2::splat(rad * 2.0),
                    mul(col, 1.2),
                    mul(col, 0.7),
                    1.0,
                    rad,
                    2.0,
                    mul(col, 0.45),
                    1.0,
                );
                specular(r, c, rad, seed);
            };
            if show_current {
                jelly(r, Vec2::new(rail_x, rail_y), cur_r, cur_col, 0x511D);
            }
            // "Next Ball" box: panel + label + the upcoming ball.
            r.draw_ui_panel(
                Vec2::new(nb_x, nb_y),
                Vec2::new(nb_w, nb_h),
                [0.10, 0.11, 0.15, 0.92],
                [0.05, 0.05, 0.08, 0.92],
                1.0,
                8.0,
                1.5,
                [0.34, 0.36, 0.44, 1.0],
                1.0,
            );
            let lw = r.measure_text(&nb_label, nb_label_size).x;
            r.draw_text(
                &nb_label,
                Vec2::new(nb_x + (nb_w - lw) * 0.5, nb_y + 8.0),
                nb_label_size,
                [0.82, 0.85, 0.92, 1.0],
            );
            jelly(r, next_c, next_r, next_col, 0xB0BB1E);
            // Progression column: locked (black + "?") then unlocked (coloured).
            for cc in &locked {
                r.draw_ui_panel(
                    Vec2::new(cc.x - col_r, cc.y - col_r),
                    Vec2::splat(col_r * 2.0),
                    [0.10, 0.10, 0.13, 1.0],
                    [0.04, 0.04, 0.06, 1.0],
                    1.0,
                    col_r,
                    2.0,
                    [0.34, 0.34, 0.40, 1.0],
                    1.0,
                );
                r.draw_text(
                    "?",
                    Vec2::new(cc.x - q_w * 0.5, cc.y - q_size * 0.5),
                    q_size,
                    [0.62, 0.62, 0.70, 1.0],
                );
            }
            for (ci, (cc, col, cnt)) in column.iter().enumerate() {
                jelly(r, *cc, col_r, *col, ci as u32 + 3);
                let s = format!("{cnt}"); // live count of this tier on the field
                let w = r.measure_text(&s, q_size).x;
                r.draw_text(
                    &s,
                    Vec2::new(cc.x - col_r - 10.0 - w, cc.y - q_size * 0.5),
                    q_size,
                    [0.82, 0.85, 0.92, 1.0],
                );
            }
            // Floating combo/score popups — big display face, on top of everything.
            for (pos, txt, size, col) in &popups {
                let w = r
                    .measure_text_role(txt, *size, FontRole::Display, false, true, -1.0)
                    .x;
                r.draw_text_role(
                    txt,
                    Vec2::new(pos.x - w * 0.5, pos.y - size * 0.5),
                    *size,
                    *col,
                    FontRole::Display,
                    false,
                    true,
                    -1.0,
                    None,
                );
            }
            // High-scores box (lower-left, below the stats panel).
            let (hs_x, hs_y, hs_w) = (16.0, 330.0, 264.0);
            let hs_h = 34.0 + hs.len() as f32 * 26.0;
            r.draw_ui_panel(
                Vec2::new(hs_x, hs_y),
                Vec2::new(hs_w, hs_h),
                [0.10, 0.11, 0.15, 0.92],
                [0.05, 0.05, 0.08, 0.92],
                1.0,
                8.0,
                1.5,
                [0.34, 0.36, 0.44, 1.0],
                1.0,
            );
            r.draw_text(
                &hs_label,
                Vec2::new(hs_x + 14.0, hs_y + 8.0),
                16.0,
                [0.82, 0.85, 0.92, 1.0],
            );
            for (i, (name, sc)) in hs.iter().enumerate() {
                let ry = hs_y + 34.0 + i as f32 * 26.0;
                r.draw_text(
                    name,
                    Vec2::new(hs_x + 14.0, ry),
                    15.0,
                    [0.75, 0.78, 0.86, 1.0],
                );
                let w = r.measure_text(sc, 15.0).x;
                r.draw_text(
                    sc,
                    Vec2::new(hs_x + hs_w - 14.0 - w, ry),
                    15.0,
                    [0.90, 0.92, 1.0, 1.0],
                );
            }
        });

        // The screen surface's final 2D — the walker-HUD replay — as one overlay, run
        // after the composites, exactly where the post-`execute` `render_hud` used to land.
        let hud_commands = &self.hud_commands;
        fg.overlay(move |r| render_hud(r, hud_commands, white, &[]));
    }
}

/// The bench's launchable-scene factory — the CLIENT BEHAVIOUR the roster registers.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(Jiggle::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JIGGLE_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/jiggle.scene.json");

    #[test]
    fn tree_is_well_formed_and_declares_the_pause_intent() {
        let def = SceneDef::parse("jiggle", JIGGLE_SCENE).expect("jiggle.scene.json loads");
        let tree = def.tree.expect("scene defines a tree");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "jiggle.scene.json names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "jiggle.scene.json ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The model-channel strings gate: this crate publishes only numbers into the
        // Model (formatting lives in jiggle.lua), so no raw display copy escapes here.
        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(
            flags.is_empty(),
            "raw display copy published into the Model: {flags:?}"
        );
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
    }

    #[test]
    fn the_pair_script_derives_the_display_strings() {
        let def = SceneDef::parse("jiggle", JIGGLE_SCENE).expect("jiggle.scene.json loads");
        let j = Jiggle::new(&def);
        assert!(j.script.is_some(), "jiggle.lua loads (the pair script)");
        let m = j.hud_model();
        for key in ["score", "best", "combo"] {
            assert!(
                m.text(key).is_some(),
                "derive() must yield display TEXT for '{key}'"
            );
        }
        assert_eq!(m.text("score"), Some("0"));
    }

    #[test]
    fn rng_only_spawns_unlocked_capped_tiers() {
        for cap in 0..=RNG_CAP {
            for _ in 0..200 {
                assert!(rand_tier(cap) <= cap, "never above the unlock cap");
            }
        }
        assert!(rand_tier(99) < TIERS.len(), "cap clamps to the ladder");
    }

    #[test]
    fn merging_two_same_tier_pops_both_and_spawns_the_next() {
        let def = SceneDef::parse("jiggle", JIGGLE_SCENE).expect("scene loads");
        let mut j = Jiggle::new(&def);
        let r = j.tier_radius(0);
        j.balls
            .push(Ball::new(1, 0, &TIERS[0], Vec2::new(276.0, 300.0), r));
        j.balls
            .push(Ball::new(2, 0, &TIERS[0], Vec2::new(300.0, 300.0), r));
        j.next_id = 3;
        let before: u64 = j.balls.iter().map(|b| 1u64 << b.tier).sum();
        j.resolve_merges();
        let after: u64 = j.balls.iter().map(|b| 1u64 << b.tier).sum();
        assert_eq!(
            before, after,
            "a merge conserves tier-units — no ball is dropped"
        );
        assert_eq!(j.balls.len(), 1, "two tier-0 balls merged into one");
        assert_eq!(j.balls[0].tier, 1, "…which is the next tier up");
        assert_eq!(j.score, TIERS[0].score, "the merge scored one payout at ×1");
        assert_eq!(
            j.combo, 1,
            "two settled (cold) balls merging is not a combo"
        );
    }

    #[test]
    fn combo_climbs_only_through_a_live_cascade() {
        let def = SceneDef::parse("jiggle", JIGGLE_SCENE).expect("scene loads");
        let mut j = Jiggle::new(&def);
        let r = j.tier_radius(1);
        let mut a = Ball::new(1, 1, &TIERS[1], Vec2::new(276.0, 300.0), r);
        let mut b = Ball::new(2, 1, &TIERS[1], Vec2::new(300.0, 300.0), r);
        a.chain = 1; // still-hot cascade products (would-be chain reaction)
        a.chain_ttl = 1.0;
        b.chain = 1;
        b.chain_ttl = 1.0;
        j.balls.push(a);
        j.balls.push(b);
        j.next_id = 3;
        j.resolve_merges();
        assert_eq!(j.combo, 2, "merging two hot chain-1 products is a ×2 combo");
        assert_eq!(j.score, TIERS[1].score * 2, "…scored at ×2");
        assert_eq!(
            j.balls[0].chain, 2,
            "the product carries the deeper cascade depth"
        );
    }
}
