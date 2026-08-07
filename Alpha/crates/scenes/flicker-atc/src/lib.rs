//! flicker-atc — **Air Traffic Control**: sequence 26 aircraft through a radar
//! scope without ever letting two of them get close. A minigame PACKAGE (library
//! only) hosted by `prism-alpha` on the Adventurer page. Like the Click Trainer it
//! demonstrates ENGINE technique and carries no Prism fiction.
//!
//! What it demonstrates, and why it is worth having beside the Click Trainer:
//!
//! * a **2D scope painted by the scene** (grid, corridors, approach vectors, blips,
//!   data tags — sprites, triangles and text at the base layer) under a **fully
//!   data-composed Prism surface**: the scene emits ONE `atc_console` template
//!   instance, declares its intents on the screen root, and `expand`s once;
//! * a game driven entirely by a **command panel** — three dropdowns and a
//!   DISPATCH button — rather than by direct manipulation, so every action is one
//!   focusable control away and the whole game is playable from a pad;
//! * the click-routing contract the two need to share a screen: the walker reports
//!   `hud_hit` over the rail, so a click on a dropdown can never also be read as a
//!   pick on the radar (`route::ScopeBase` sees only what the console let past).
//!
//! The rules, the board and the clock live in [`sim`] — pure state and one
//! `tick()` per radar sweep, so every rule is tested without a window.
//!
//! Controls: pick a FLIGHT, a COMMAND and a VALUE, then DISPATCH (`Confirm` on the
//! focused control does the same). The bumpers step through the live flights; a
//! click on a blip selects it. Esc opens the pause menu — the screen root's
//! DECLARED `on_menu = "pause_open"` intent.

use std::time::Duration;

use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{ComponentLibrary, HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    builtin_templates, expand, load_styles, render_hud, run_ui_with, strings, TemplateRegistry,
    UiInput, UiIntents, UiState, WalkerHandler, UI_COMPONENT_MODULES,
};
use flicker_input_core::{
    AbstractControls, ContextualBindings, Fired, GamepadConfig, InputMap, InputState, Resolver,
};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

pub mod sim;
mod route;
#[cfg(test)]
mod tests;

use route::{RootHandler, ScopeBase};
use sim::{Aircraft, Cmd, Event, Game, Phase, Reject, Verdict, AIRPORTS, CORRIDORS, HOLDS};

const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_elements.json");

/// The ONE composed surface: the console proto in `ui_templates.json`.
const CONSOLE_TEMPLATE: &str = "atc_console";
/// Strip-bay slots the proto ships. More traffic than this and the oldest strips
/// simply do not show — the scope still does.
const STRIPS: usize = 12;
/// Transmission-log rows the proto ships.
const LOG_ROWS: usize = 6;
/// How near the cursor must be to a blip, in sectors, to pick it.
const PICK_GRID: f32 = 0.55;

// ── style reading ────────────────────────────────────────────────────────────

/// Read a number out of the token-resolved style JSON by dotted path.
fn style_num(styles: &serde_json::Value, path: &str) -> Option<f32> {
    path.split('.')
        .try_fold(styles, |node, key| node.get(key))
        .and_then(serde_json::Value::as_f64)
        .map(|n| n as f32)
}

/// Read an rgba colour out of the token-resolved style JSON by dotted path. A path
/// that names nothing comes back MAGENTA rather than something plausible, so a
/// missing key is visible on the scope instead of quietly reading as a colour
/// choice.
fn style_color(styles: &serde_json::Value, path: &str) -> [f32; 4] {
    const MISSING: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
    let Some(v) = path.split('.').try_fold(styles, |node, key| node.get(key)) else {
        return MISSING;
    };
    match v.as_array() {
        Some(a) if a.len() >= 4 => std::array::from_fn(|i| a[i].as_f64().unwrap_or(0.0) as f32),
        _ => MISSING,
    }
}

/// The console geometry, read ONCE from `atc.layout`. The scene places the radar
/// from these numbers and hands the SAME ones to the template as instance params,
/// so the bezel the walker draws and the picture painted inside it cannot drift.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub margin: f32,
    pub gap: f32,
    pub pad: f32,
    pub rail_w: f32,
    pub cmd_h: f32,
    pub strip_h: f32,
    pub ctrl_h: f32,
    pub log_h: f32,
}

impl Layout {
    /// What a console looks like when `atc.layout` is missing: a cramped, obviously
    /// unshipped geometry. Never the real numbers — a fallback that equals its
    /// canonical value is a second source of truth that drifts in silence.
    const LOUD: Self = Self {
        margin: 0.0,
        gap: 0.0,
        pad: 0.0,
        rail_w: 120.0,
        cmd_h: 120.0,
        strip_h: 10.0,
        ctrl_h: 12.0,
        log_h: 8.0,
    };

    fn from_styles(styles: &serde_json::Value) -> Self {
        let read = |key: &str| style_num(styles, &format!("atc.layout.{key}"));
        let Some(margin) = read("margin") else {
            tracing::error!("ui_elements.json has no `atc.layout` — the console will look wrong");
            return Self::LOUD;
        };
        Self {
            margin,
            gap: read("gap").unwrap_or(Self::LOUD.gap),
            pad: read("pad").unwrap_or(Self::LOUD.pad),
            rail_w: read("rail_w").unwrap_or(Self::LOUD.rail_w),
            cmd_h: read("cmd_h").unwrap_or(Self::LOUD.cmd_h),
            strip_h: read("strip_h").unwrap_or(Self::LOUD.strip_h),
            ctrl_h: read("ctrl_h").unwrap_or(Self::LOUD.ctrl_h),
            log_h: read("log_h").unwrap_or(Self::LOUD.log_h),
        }
    }
}

/// Where the radar picture lands, in screen pixels: the bezel rect, and the square
/// sector grid centred inside it.
#[derive(Clone, Copy, Debug)]
pub struct Scope {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Pixels per sector — square, so a heading looks like the heading it is.
    pub cell: f32,
    /// Screen position of sector (0, 0).
    pub ox: f32,
    pub oy: f32,
}

impl Scope {
    /// The bezel for this window: sized to the largest SQUARE sector grid that fits
    /// the space the rail leaves — square because a heading has to look like the
    /// heading it is — and centred in it, so the console reads as a scope rather
    /// than as a grid adrift on a wide dark face.
    pub fn new(screen: Vec2, l: &Layout) -> Self {
        let room_w = (screen.x - l.rail_w - l.gap - l.margin * 2.0).max(160.0);
        let room_h = (screen.y - l.margin * 2.0).max(160.0);
        let inset = l.pad;
        let cell = ((room_w - inset * 2.0) / sim::COLS).min((room_h - inset * 2.0) / sim::ROWS);
        let (w, h) = (cell * sim::COLS + inset * 2.0, cell * sim::ROWS + inset * 2.0);
        let x = l.margin + (room_w - w) * 0.5;
        let y = l.margin + (room_h - h) * 0.5;
        Self { x, y, w, h, cell, ox: x + inset, oy: y + inset }
    }

    /// Sector coordinates → screen pixels.
    pub fn px(&self, gx: f32, gy: f32) -> Vec2 {
        Vec2::new(self.ox + gx * self.cell, self.oy + gy * self.cell)
    }

    /// Screen pixels → sector coordinates.
    pub fn sector(&self, p: Vec2) -> (f32, f32) {
        ((p.x - self.ox) / self.cell, (p.y - self.oy) / self.cell)
    }
}

// ── the command vocabulary, as the panel offers it ───────────────────────────

/// One row of the COMMAND dropdown. The panel's whole verb set, and the only
/// place a picked verb turns into a [`Cmd`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    TurnLeft,
    TurnRight,
    Descend,
    Ascend,
    HoldA,
    HoldB,
    Takeoff,
}

impl Verb {
    /// In the order the proto lists them — the `value` of each `option`.
    pub const ALL: [Verb; 7] = [
        Verb::TurnLeft,
        Verb::TurnRight,
        Verb::Descend,
        Verb::Ascend,
        Verb::HoldA,
        Verb::HoldB,
        Verb::Takeoff,
    ];

    /// The option value the dropdown binds — data, matching `ui_templates.json`.
    pub fn id(self) -> &'static str {
        match self {
            Verb::TurnLeft => "tl",
            Verb::TurnRight => "tr",
            Verb::Descend => "dh",
            Verb::Ascend => "ah",
            Verb::HoldA => "ha",
            Verb::HoldB => "hb",
            Verb::Takeoff => "ct",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.id() == id)
    }

    /// Does this verb need a heading from the VALUE dropdown?
    pub fn needs_heading(self) -> bool {
        matches!(self, Verb::TurnLeft | Verb::TurnRight)
    }

    /// Does this verb need an altitude from the VALUE dropdown?
    pub fn needs_altitude(self) -> bool {
        matches!(self, Verb::Descend | Verb::Ascend)
    }

    /// The transmission this verb makes with `value`, or `None` while the VALUE
    /// dropdown still owes one.
    pub fn cmd(self, value: Option<i32>) -> Option<Cmd> {
        Some(match self {
            Verb::TurnLeft => Cmd::TurnLeft(value?),
            Verb::TurnRight => Cmd::TurnRight(value?),
            Verb::Descend => Cmd::DescendHold(value?),
            Verb::Ascend => Cmd::AscendHold(value?),
            Verb::HoldA => Cmd::HoldAt(0),
            Verb::HoldB => Cmd::HoldAt(1),
            Verb::Takeoff => Cmd::ClearedTakeoff,
        })
    }
}

/// The reason a refused transmission gives back, as a stringtable token.
fn reject_token(r: Reject) -> &'static str {
    match r {
        Reject::NoSuchFlight => "$atc_rej_flight",
        Reject::OnTheGround => "$atc_rej_ground",
        Reject::Airborne => "$atc_rej_air",
        Reject::WrongDirection => "$atc_rej_dir",
        Reject::OutOfRange => "$atc_rej_range",
    }
}

/// The pilot's call, as a stringtable token.
fn event_token(e: Event) -> &'static str {
    match e {
        Event::Entered(_) => "$atc_ev_entered",
        Event::Ready(_) => "$atc_ev_ready",
        Event::Landed(_) => "$atc_ev_landed",
        Event::Departed(_) => "$atc_ev_departed",
        Event::LowFuel(_) => "$atc_ev_lowfuel",
        Event::WentAround(_) => "$atc_ev_around",
    }
}

/// The two halves of the end-of-session read-out, as stringtable tokens.
fn verdict_tokens(v: Verdict) -> (&'static str, &'static str) {
    match v {
        Verdict::Conflict(..) => ("$atc_end_conflict", "$atc_line_conflict"),
        Verdict::OutOfFuel(_) => ("$atc_end_fuel", "$atc_line_fuel"),
        Verdict::WrongExit(_) => ("$atc_end_exit", "$atc_line_exit"),
        Verdict::WrongAltitude(_) => ("$atc_end_altitude", "$atc_line_altitude"),
        Verdict::Cleared => ("$atc_end_cleared", "$atc_line_cleared"),
    }
}

// ── the scene ────────────────────────────────────────────────────────────────

/// The Air Traffic Control scene: the session, the command panel's pending
/// transmission, and the engine plumbing every Prism screen carries.
pub struct Atc {
    game: Game,
    /// Seeds the next session too, bumped per restart so NEW SESSION deals a fresh
    /// hour rather than the same one.
    seed: u64,
    /// Real seconds accumulated toward the next radar sweep.
    clock: f32,

    /// Who the pending transmission is addressed to.
    sel_flight: Option<char>,
    /// What it says.
    sel_verb: Option<Verb>,
    /// The heading (degrees) or altitude (feet) it carries.
    sel_value: Option<i32>,
    /// The transmission log, newest FIRST, capped at [`LOG_ROWS`].
    log: Vec<String>,

    // ── engine plumbing ──
    layout: Layout,
    templates: TemplateRegistry,
    ui_intents: UiIntents,
    ui_state: UiState,
    ui_styles: serde_json::Value,
    script: Option<ScriptHost>,
    hud_commands: Vec<HudCommand>,
    ui_theme: Option<Theme>,
    white: Option<TextureHandle>,
    /// Where the radar landed last frame — the click-pick reads it after dispatch.
    scope: Scope,

    bindings: ContextualBindings,
    gamepad_config: GamepadConfig,
    resolver: Resolver,
    ev: Vec<Fired>,
    route: RouteCtx,
    tick: u64,
    fired_sigs: Vec<String>,
}

impl Default for Atc {
    fn default() -> Self {
        Self::new()
    }
}

impl Atc {
    /// A session on the wall-clock seed — a different hour every launch.
    pub fn new() -> Self {
        Self::with_seed(fastrand::u64(..))
    }

    /// A session on an exact seed — the seam the sim tests drive.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            game: Game::new(seed),
            seed,
            clock: 0.0,
            sel_flight: None,
            sel_verb: None,
            sel_value: None,
            log: Vec::new(),
            layout: Layout::LOUD,
            templates: builtin_templates(),
            ui_intents: UiIntents::default(),
            ui_state: UiState::default(),
            ui_styles: serde_json::Value::Null,
            script: None,
            hud_commands: Vec::new(),
            ui_theme: None,
            white: None,
            scope: Scope::new(Vec2::new(1280.0, 720.0), &Layout::LOUD),
            bindings: ContextualBindings::new(InputMap::wasd_and_mouse()),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            route: RouteCtx::new(),
            tick: 0,
            fired_sigs: Vec::new(),
        }
    }

    /// The live session, for tests and for the scope draw.
    pub fn game(&self) -> &Game {
        &self.game
    }

    /// The flight the command panel is addressed to.
    pub fn selected(&self) -> Option<char> {
        self.sel_flight
    }

    /// The transmission log, newest first.
    pub fn log(&self) -> &[String] {
        &self.log
    }

    /// Deal a fresh session on the next seed.
    fn restart(&mut self) {
        self.seed = self.seed.wrapping_add(1);
        self.game = Game::new(self.seed);
        self.clock = 0.0;
        self.sel_flight = None;
        self.sel_verb = None;
        self.sel_value = None;
        self.log.clear();
    }

    /// Push a line onto the transmission log, newest first.
    fn say(&mut self, line: String) {
        self.log.insert(0, line);
        self.log.truncate(LOG_ROWS);
    }

    /// The flights the panel may address, in strip order.
    fn roster(&self) -> Vec<char> {
        self.game.aircraft.iter().map(|a| a.id).collect()
    }

    /// Step the addressed flight through the live roster (the bumpers).
    fn step_flight(&mut self, forward: bool) {
        let roster = self.roster();
        if roster.is_empty() {
            self.sel_flight = None;
            return;
        }
        let at = self.sel_flight.and_then(|c| roster.iter().position(|r| *r == c));
        let next = match (at, forward) {
            (Some(i), true) => (i + 1) % roster.len(),
            (Some(i), false) => (i + roster.len() - 1) % roster.len(),
            (None, _) => 0,
        };
        self.sel_flight = Some(roster[next]);
    }

    /// The VALUE dropdown's domain for the picked verb: headings in tens, or
    /// altitudes in thousands. Empty when the verb carries no value.
    fn value_domain(&self) -> Vec<i32> {
        match self.sel_verb {
            Some(v) if v.needs_heading() => (1..=36).map(|i| i * 10).collect(),
            Some(v) if v.needs_altitude() => (1..=10).map(|i| i * 1000).collect(),
            _ => Vec::new(),
        }
    }

    /// The transmission DISPATCH would send right now, if the panel is complete.
    fn pending(&self) -> Option<(char, Cmd)> {
        Some((self.sel_flight?, self.sel_verb?.cmd(self.sel_value)?))
    }

    /// Send the pending transmission. Logs the canonical code on acceptance and
    /// the refusal reason otherwise; a clean send clears the value so the panel is
    /// never one careless Confirm away from repeating itself.
    fn dispatch(&mut self) {
        let Some((id, cmd)) = self.pending() else {
            return;
        };
        match self.game.command(id, cmd) {
            Ok(code) => {
                self.say(code);
                self.sel_value = None;
                self.sel_verb = None;
            }
            Err(r) => {
                let why = strings::resolve(reject_token(r)).into_owned();
                self.say(format!("{id} · {why}"));
            }
        }
    }

    /// Advance the session by `dt`, running a sweep whenever the radar comes round.
    fn advance(&mut self, dt: f32) {
        if self.game.verdict.is_some() {
            return;
        }
        self.clock += dt;
        while self.clock >= sim::SWEEP_REAL_SECONDS {
            self.clock -= sim::SWEEP_REAL_SECONDS;
            for e in self.game.tick() {
                let what = strings::resolve(event_token(e)).into_owned();
                self.say(format!("{} · {what}", e.flight()));
            }
            // A flight that left the board can no longer be addressed.
            if self.sel_flight.is_some_and(|c| self.game.find(c).is_none()) {
                self.sel_flight = None;
            }
            if self.game.verdict.is_some() {
                break;
            }
        }
    }

    /// Pick the blip under `cursor`, if the cursor is on one.
    fn pick_at(&mut self, cursor: Vec2) {
        let (gx, gy) = self.scope.sector(cursor);
        let hit = self
            .game
            .aircraft
            .iter()
            .map(|a| (a.id, (a.x - gx).hypot(a.y - gy)))
            .filter(|(_, d)| *d <= PICK_GRID)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = hit {
            self.sel_flight = Some(id);
        }
    }

    // ── the Model ────────────────────────────────────────────────────────────

    /// Every Model key the `atc_console` proto binds, published fresh each frame.
    /// Values here are DATA — designations, levels, headings, counts — or text
    /// already resolved through the stringtable; no display copy is minted here.
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::new();

        // The strip bay: a fixed bank, one row per aircraft, the rest switched off.
        for i in 0..STRIPS {
            let on = self.game.aircraft.get(i);
            m.set(format!("strip_{i}_on"), on.is_some());
            let Some(a) = on else { continue };
            m.set(format!("strip_{i}_id"), a.id.to_string());
            m.set(format!("strip_{i}_kind"), a.kind.mark().to_string());
            m.set(format!("strip_{i}_alt"), self.alt_cell(a));
            m.set(format!("strip_{i}_hdg"), self.hdg_cell(a));
            m.set(format!("strip_{i}_route"), format!("{} {} {}", a.origin(), '\u{2192}', a.destination()));
            m.set(format!("strip_{i}_fuel"), a.fuel.to_string());
            m.set(format!("strip_{i}_style"), self.strip_style(a));
        }

        m.set("stat_sweep", self.game.sweep.to_string());
        m.set("stat_landed", self.game.landed.to_string());
        m.set("stat_departed", self.game.departed.to_string());
        m.set("stat_pending", self.game.pending().to_string());

        // The command panel's three dropdowns read their own selection back.
        if let Some(c) = self.sel_flight {
            m.set("cmd_flight", c.to_string());
        }
        if let Some(v) = self.sel_verb {
            m.set("cmd_verb", v.id().to_string());
        }
        if let Some(v) = self.sel_value {
            m.set("cmd_value", value_id(v));
        }
        m.set(
            "cmd_preview",
            match self.pending() {
                Some((id, cmd)) => format!("{id}{}", cmd.code()),
                None => strings::resolve("$atc_hint_pick").into_owned(),
            },
        );

        for i in 0..LOG_ROWS {
            m.set(format!("log_{i}"), self.log.get(i).cloned().unwrap_or_default());
        }

        m.set("has_verdict", self.game.verdict.is_some());
        if let Some(v) = self.game.verdict {
            let (kind, line) = verdict_tokens(v);
            m.set("verdict_kind", strings::resolve(kind).into_owned());
            m.set("verdict_line", self.verdict_line(v, line));
            m.set("verdict_score", self.score_line());
        }

        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    /// The strip's altitude cell: thousands of feet, or a dash on the ground.
    fn alt_cell(&self, a: &Aircraft) -> String {
        if a.phase == Phase::Ready {
            '\u{2014}'.to_string()
        } else {
            a.flight_level().to_string()
        }
    }

    /// The strip's heading cell: three digits, or a dash on the ground.
    fn hdg_cell(&self, a: &Aircraft) -> String {
        if a.phase == Phase::Ready {
            '\u{2014}'.to_string()
        } else {
            format!("{:03}", a.heading)
        }
    }

    /// Which row skin a strip wears: addressed, low on fuel, or plain.
    fn strip_style(&self, a: &Aircraft) -> String {
        if self.sel_flight == Some(a.id) {
            "atc.row_sel".to_string()
        } else if a.low_fuel() {
            "atc.row_warn".to_string()
        } else {
            "atc.row".to_string()
        }
    }

    /// The end-of-session sentence: who it was about, then the resolved reason.
    fn verdict_line(&self, v: Verdict, token: &str) -> String {
        let why = strings::resolve(token).into_owned();
        match v {
            Verdict::Conflict(a, b) => format!("{a} · {b} — {why}"),
            Verdict::OutOfFuel(c)
            | Verdict::WrongExit(c)
            | Verdict::WrongAltitude(c) => format!("{c} — {why}"),
            Verdict::Cleared => why,
        }
    }

    /// The tally under it, every word of which comes from the stringtable.
    fn score_line(&self) -> String {
        format!(
            "{} {} · {} {} · {} {}",
            self.game.landed,
            strings::resolve("$atc_score_landed"),
            self.game.departed,
            strings::resolve("$atc_score_departed"),
            self.game.sweep,
            strings::resolve("$atc_score_sweeps"),
        )
    }

    // ── the tree ─────────────────────────────────────────────────────────────

    /// This frame's surface: ONE `atc_console` instance, configured with the
    /// geometry and fed the two dynamic option lists through its slots. Rebuilt
    /// every frame because re-expansion is what keeps a template param live.
    pub fn build_tree(&self, screen: Vec2) -> UiNode {
        let mut page = node("screen");
        // The screen's input DECLARATION: everything the console reacts to is named
        // here as data, so a pad press, a key and a click are the same event by the
        // time the dispatcher sees it. `Confirm` is deliberately NOT declared — a
        // declaration would displace the walker's own "activate the focused control",
        // which is how the pad presses DISPATCH.
        for (signal, result) in [
            ("on_menu", "pause_open"),
            ("on_cancel", "clear_cmd"),
            ("on_tab_next", "flight_next"),
            ("on_tab_prev", "flight_prev"),
        ] {
            page.props.insert(signal.into(), Value::Text(result.into()));
        }

        let scope = Scope::new(screen, &self.layout);
        let l = &self.layout;
        let mut console = UiNode { template: Some(CONSOLE_TEMPLATE.into()), ..Default::default() };
        console.id = "atc_console".into();
        for (key, v) in [
            ("margin", l.margin),
            ("margin_neg", -l.margin),
            ("gap", l.gap),
            ("pad", l.pad),
            ("rail_w", l.rail_w),
            ("cmd_h", l.cmd_h),
            ("strip_h", l.strip_h),
            ("ctrl_h", l.ctrl_h),
            ("log_h", l.log_h),
            // The bezel, verbatim from the same `Scope` the radar is painted into —
            // the frame and the picture cannot land in different places.
            ("scope_x", scope.x),
            ("scope_y", scope.y),
            ("scope_w", scope.w),
            ("scope_h", scope.h),
            ("body_h", screen.y - l.margin * 2.0),
        ] {
            console = prop(console, key, Value::Number(f64::from(v)));
        }
        console.slots.insert("flights".into(), self.flight_options());
        console.slots.insert("values".into(), self.value_options());
        page.children = vec![console];

        // Expanded HERE, not at the call sites, so the scene and every gate walk the
        // SAME tree — an unresolved proto would otherwise draw a bare box in the app
        // while the tests inspected a `template` node they never opened.
        expand(page, &self.templates)
    }

    /// The FLIGHT dropdown's rows — one per aircraft on the scope, labelled with
    /// the designation and nothing else. That is what the pilot says, and the strip
    /// bay beside the dropdown already carries the type, the route and the fuel, so
    /// repeating them in the row would be noise (and unlocalizable noise at that:
    /// a designation is a single glyph, which is data in any language).
    fn flight_options(&self) -> Vec<UiNode> {
        self.game.aircraft.iter().map(|a| option(&a.id.to_string(), &a.id.to_string())).collect()
    }

    /// The VALUE dropdown's rows for the picked verb — headings or altitudes, each
    /// labelled exactly as the transmission will read.
    fn value_options(&self) -> Vec<UiNode> {
        self.value_domain()
            .into_iter()
            .map(|v| {
                let id = value_id(v);
                let label = if v >= 1000 { format!("{}00", v / 100) } else { format!("{v:03}") };
                option(&id, &label)
            })
            .collect()
    }

    // ── results ──────────────────────────────────────────────────────────────

    /// Fold one frame's results — dropdown picks, button actions and fired intent
    /// names alike — into the session. The ONE dispatcher: a click, a key and a pad
    /// press all arrive here as the same names.
    fn apply_results(&mut self, results: &ValueMap) {
        if let Some(v) = results.text("cmd_flight") {
            self.sel_flight = v.chars().next();
        }
        if let Some(v) = results.text("cmd_verb").and_then(Verb::from_id) {
            // A new verb may change what the VALUE dropdown even means, so the old
            // value never survives into a transmission it does not belong to.
            if self.sel_verb != Some(v) {
                self.sel_value = None;
            }
            self.sel_verb = Some(v);
        }
        if let Some(v) = results.text("cmd_value").and_then(|s| s.parse::<i32>().ok()) {
            if self.value_domain().contains(&v) {
                self.sel_value = Some(v);
            }
        }
        if results.is_on("flight_next") {
            self.step_flight(true);
        }
        if results.is_on("flight_prev") {
            self.step_flight(false);
        }
        if results.is_on("clear_cmd") {
            self.sel_verb = None;
            self.sel_value = None;
        }
        if results.is_on("dispatch") {
            self.dispatch();
        }
        if results.is_on("restart") {
            self.restart();
        }
    }

    // ── the radar picture ────────────────────────────────────────────────────

    /// Paint the scope: the face, the sector grid, the corridors, the two fields
    /// with their approach vectors and outer markers, the holding racetracks, and
    /// every blip with its data tag. Drawn at the BASE layer, so the walker's bezel
    /// (a transparent frame with a bronze edge) rings it a moment later.
    fn draw_scope(&self, r: &mut Renderer, white: TextureHandle) {
        let s = &self.scope;
        let ink = |path: &str| style_color(&self.ui_styles, path);
        r.draw_sprite(white, Vec2::new(s.x, s.y), Vec2::new(s.w, s.h), ink("atc.scope.face"));

        // The sector grid — the edges brighter, so the boundary reads as the edge of
        // the controller's area rather than as one more line.
        let (grid, edge) = (ink("atc.scope.grid"), ink("atc.scope.grid_edge"));
        for i in 0..=(sim::COLS as i32) {
            let c = if i == 0 || i == sim::COLS as i32 { edge } else { grid };
            let p = s.px(i as f32, 0.0);
            r.draw_sprite(white, p, Vec2::new(1.0, sim::ROWS * s.cell), c);
        }
        for i in 0..=(sim::ROWS as i32) {
            let c = if i == 0 || i == sim::ROWS as i32 { edge } else { grid };
            let p = s.px(0.0, i as f32);
            r.draw_sprite(white, p, Vec2::new(sim::COLS * s.cell, 1.0), c);
        }

        self.draw_corridors(r, ink("atc.scope.corridor"));
        self.draw_fields(r, white);
        self.draw_holds(r, white, ink("atc.scope.hold"));
        self.draw_traffic(r, white);
    }

    /// The six corridors: a chevron pointing the way OUT, and the centre's name.
    fn draw_corridors(&self, r: &mut Renderer, ink: [f32; 4]) {
        let s = &self.scope;
        for c in &CORRIDORS {
            let at = s.px(c.x, c.y);
            let (dx, dy) = sim::track(c.exit_heading);
            let (out, side) = (Vec2::new(dx, dy), Vec2::new(-dy, dx));
            let n = s.cell * 0.3;
            r.draw_triangle(at + out * n, at + side * n * 0.7, at - side * n * 0.7, ink);
            // The label sits INSIDE the area, clear of the chevron.
            let text = at - out * (n * 2.6) - side * (n * 0.9);
            r.draw_text(c.id, text, s.cell * 0.24, ink);
        }
    }

    /// Both fields: the runway axis as a dashed approach vector, an outer marker on
    /// each side (the live one lit, its opposite inert — landings run either way,
    /// and the wind picked one for this session), the field mark, and the landing
    /// heading printed beside it exactly as the paper spec asks.
    fn draw_fields(&self, r: &mut Renderer, white: TextureHandle) {
        let s = &self.scope;
        let ink = |path: &str| style_color(&self.ui_styles, path);
        for (i, ap) in AIRPORTS.iter().enumerate() {
            let hdg = self.game.landing_heading(i);
            let (dx, dy) = sim::track(hdg);
            let reach = sim::MARKER_DIST + 0.7;
            let a = s.px(ap.x - dx * reach, ap.y - dy * reach);
            let b = s.px(ap.x + dx * reach, ap.y + dy * reach);
            dashes(r, white, a, b, s.cell * 0.12, ink("atc.scope.approach"));

            for (mx, my, lit) in [
                (self.game.outer_marker(i).0, self.game.outer_marker(i).1, true),
                (self.game.far_marker(i).0, self.game.far_marker(i).1, false),
            ] {
                let c = ink(if lit { "atc.scope.marker" } else { "atc.scope.marker_off" });
                dot(r, white, s.px(mx, my), s.cell * 0.14, c);
            }

            let field = ink("atc.scope.airport");
            let at = s.px(ap.x, ap.y);
            let e = s.cell * 0.34;
            r.draw_sprite(white, at - Vec2::splat(e * 0.5), Vec2::splat(e), field);
            let size = s.cell * 0.24;
            r.draw_text(ap.id, at + Vec2::new(e * 0.8, -size), size, field);
            r.draw_text(&format!("{hdg:03}"), at + Vec2::new(e * 0.8, 2.0), size, field);
        }
    }

    /// The two holding fixes, drawn as the racetrack an aircraft flies on them.
    fn draw_holds(&self, r: &mut Renderer, white: TextureHandle, ink: [f32; 4]) {
        let s = &self.scope;
        for h in &HOLDS {
            let at = s.px(h.x, h.y);
            let (w, t) = (s.cell * 0.42, s.cell * 0.62);
            for (o, size) in [
                (Vec2::new(-w, -t), Vec2::new(w * 2.0, 1.0)),
                (Vec2::new(-w, t), Vec2::new(w * 2.0, 1.0)),
                (Vec2::new(-w, -t), Vec2::new(1.0, t * 2.0)),
                (Vec2::new(w, -t), Vec2::new(1.0, t * 2.0)),
            ] {
                r.draw_sprite(white, at + o, size, ink);
            }
            r.draw_text(h.id, at - Vec2::new(w * 0.25, s.cell * 0.13), s.cell * 0.26, ink);
        }
    }

    /// Every aircraft: a blip (a filled diamond for a jet, a hollow box for a prop),
    /// a leader showing where the next sweep puts it, and the data tag a controller
    /// actually reads — designation, level, heading.
    fn draw_traffic(&self, r: &mut Renderer, white: TextureHandle) {
        let s = &self.scope;
        let ink = |path: &str| style_color(&self.ui_styles, path);
        let flagged = match self.game.verdict {
            Some(Verdict::Conflict(a, b)) => [Some(a), Some(b)],
            _ => [None, None],
        };
        for a in &self.game.aircraft {
            let at = s.px(a.x, a.y);
            let color = if flagged.contains(&Some(a.id)) {
                ink("atc.scope.conflict")
            } else if self.sel_flight == Some(a.id) {
                ink("atc.scope.blip_sel")
            } else if a.phase == Phase::Ready {
                ink("atc.scope.blip_ground")
            } else if a.low_fuel() {
                ink("atc.scope.blip_low")
            } else {
                ink("atc.scope.blip")
            };

            if a.phase != Phase::Ready {
                let (dx, dy) = sim::track(a.heading);
                let lead = at + Vec2::new(dx, dy) * (a.kind.speed() * s.cell);
                dashes(r, white, at, lead, s.cell * 0.08, ink("atc.scope.leader"));
            }

            let e = s.cell * 0.16;
            match a.kind {
                sim::Kind::Jet => {
                    r.draw_triangle(at + Vec2::new(0.0, -e), at + Vec2::new(e, 0.0), at + Vec2::new(0.0, e), color);
                    r.draw_triangle(at + Vec2::new(0.0, -e), at + Vec2::new(-e, 0.0), at + Vec2::new(0.0, e), color);
                }
                sim::Kind::Prop => {
                    for (o, size) in [
                        (Vec2::new(-e, -e), Vec2::new(e * 2.0, 1.0)),
                        (Vec2::new(-e, e), Vec2::new(e * 2.0, 1.0)),
                        (Vec2::new(-e, -e), Vec2::new(1.0, e * 2.0)),
                        (Vec2::new(e, -e), Vec2::new(1.0, e * 2.0)),
                    ] {
                        r.draw_sprite(white, at + o, size, color);
                    }
                }
            }

            // The data tag, two lines off the blip's shoulder. Pure data — the
            // designation, the level, and where it is pointed.
            let size = s.cell * 0.2;
            let tag = at + Vec2::new(e * 1.6, -e - size);
            r.draw_text(&format!("{}{}", a.id, a.kind.mark()), tag, size, color);
            let second = if a.phase == Phase::Ready {
                a.destination().to_string()
            } else {
                format!("{:02} {:03}", a.flight_level(), a.heading)
            };
            r.draw_text(&second, tag + Vec2::new(0.0, size * 1.1), size, ink("atc.scope.tag"));
        }
    }
}

// ── small helpers (the Rust-tree idiom) ──────────────────────────────────────

fn node(component: &str) -> UiNode {
    UiNode { component: component.to_string(), ..Default::default() }
}

fn prop(mut n: UiNode, key: &str, value: Value) -> UiNode {
    n.props.insert(key.to_string(), value);
    n
}

/// A dropdown row. `value` is what the bind carries; `label` is what it reads as —
/// both DATA here (designations, headings, altitudes), which is why neither is a
/// stringtable token: the fixed VERB vocabulary that does need one lives in the
/// proto.
fn option(value: &str, label: &str) -> UiNode {
    let mut n = node("option");
    n = prop(n, "value", Value::Text(value.to_string()));
    prop(n, "label", Value::Text(label.to_string()))
}

/// The VALUE dropdown's bind text for a heading or an altitude — the number itself,
/// so the pick round-trips through `parse`.
fn value_id(v: i32) -> String {
    v.to_string()
}

/// A dotted line from `a` to `b` — the scope's only diagonal, and the reason the
/// approach vectors and leaders read as vectors rather than as boxes.
fn dashes(r: &mut Renderer, white: TextureHandle, a: Vec2, b: Vec2, step: f32, color: [f32; 4]) {
    let span = b - a;
    let len = span.length();
    if len <= f32::EPSILON || step <= 0.5 {
        return;
    }
    let n = (len / step.max(2.0)).round().clamp(1.0, 96.0) as i32;
    for i in 0..=n {
        let p = a + span * (i as f32 / n as f32);
        r.draw_sprite(white, p - Vec2::splat(0.75), Vec2::splat(1.5), color);
    }
}

/// A small filled square, centred — the scope's marker dot.
fn dot(r: &mut Renderer, white: TextureHandle, at: Vec2, radius: f32, color: [f32; 4]) {
    r.draw_sprite(white, at - Vec2::splat(radius), Vec2::splat(radius * 2.0), color);
}

// ── the Scene ────────────────────────────────────────────────────────────────

impl Scene for Atc {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.ui_theme = Some(Theme::build(renderer));
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        self.layout = Layout::from_styles(&self.ui_styles);
        // The tree is built in Rust from ONE data proto, so the script host is here
        // purely as the COMPONENT LIBRARY (the Lua draw/hit modules the walker calls).
        match ScriptHost::library(UI_COMPONENT_MODULES) {
            Ok(host) => self.script = Some(host),
            Err(e) => tracing::error!("component library load failed: {e} — no console"),
        }
        renderer.window().set_title("Flicker Air Traffic Control");
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let screen = renderer.size();
        self.scope = Scope::new(screen, &self.layout);

        if let Some((map, _, _)) = flicker_shell::take_pending_input() {
            self.bindings = ContextualBindings::new(map);
        }

        // The radar runs on its own clock; a pushed pause overlay freezes this scene,
        // so the sweep stops with it.
        self.advance(dt.as_secs_f32());

        let tree = self.build_tree(screen);
        self.ui_intents = UiIntents::of(&tree);
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
        let frame = run_ui_with(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib);
        let over_console = frame.results.is_on("hud_hit");
        self.hud_commands = frame.commands;

        // ONE resolve, ONE dispatch. The console layer consumes the declared intents
        // and every pointer press it owns; only what it lets through is a pick on the
        // radar (`route::ScopeBase`).
        self.tick = self.tick.wrapping_add(1);
        self.ev.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad_config,
            input,
            self.tick,
            &mut self.ev,
        );
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> =
            self.ev.iter().map(|f| InputEvent::from_fired(f, ctx, input)).collect();
        self.fired_sigs.clear();

        let mut root = RootHandler;
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_console)
            .with_nav(&tree, &model)
            .with_intents(&self.ui_intents);
        let mut scope = ScopeBase::default();
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut scope];
            Router::dispatch(&events, &mut chain, &mut self.route);
        }
        // The standard post-dispatch seam: reconcile context/focus intents so the
        // pointer and the pad share ONE focus id.
        let focus_change = apply_context_requests(&mut self.bindings, &self.route.requests);
        walker.apply_focus(focus_change);
        let activated = walker.activated().map(str::to_string);
        self.fired_sigs = walker.take_fired();
        self.route.requests.clear();

        // Fold every channel into ONE result set: this frame's clicks, the pad's
        // activation of the focused control, and the declared intent names.
        let mut results = frame.results.clone();
        if let Some(action) = activated {
            results.set(action, true);
        }
        for name in &self.fired_sigs {
            results.set(name.clone(), true);
        }
        self.apply_results(&results);

        // A press the console did not want landed on the radar.
        if scope.click {
            self.pick_at(input.mouse_position);
        }

        if results.is_on("pause_open") {
            if let Some(theme) = self.ui_theme {
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    self.bindings.active_map(),
                    &AbstractControls::default(),
                    &self.gamepad_config,
                )));
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(white) = self.white else {
            return;
        };
        // The radar first, at the base layer; the console's vector chrome — bezel,
        // rail, popups — paints over it exactly as the click routing implies.
        self.draw_scope(renderer, white);
        render_hud(renderer, &self.hud_commands, white, &[]);
    }
}

/// The scene factory the launcher roster registers.
pub fn scene() -> Box<dyn Scene> {
    Box::new(Atc::new())
}
