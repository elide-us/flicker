//! flicker-controllertester: a live controller diagram that lights up as you press, PLUS
//! the action-SIGNAL each input fires shown as a large message under the map, for the
//! selected input CONTEXT.
//!
//! The diagram is the display; the signal firing is layered ON it (Aaron: keep the
//! controller map, show the fired signal underneath). Event-kind is case-by-case (combat
//! on press, menus on release), edges via the standard last-frame snapshot — the mapping
//! lives in [`signals`]. Cycle context with Tab (keyboard) or Back/View (pad).
//!
//! Built on the click-trainer scene toolkit (`draw_ui_panel` + `draw_text`, no Lua, no
//! `ui_elements.json`); `prism-alpha` hosts it. Esc opens the pause menu.

mod signals;

use std::collections::VecDeque;
use std::time::Duration;

use flicker::app::{
    AbstractControls, Action, GamepadAxis, GamepadButton, GamepadConfig, InputMap, InputState, Key,
};
use flicker::render::{Renderer, Vec2};
use flicker::scene::{Scene, Transition};
use flicker_shell::{PauseScene, Theme};

use signals::Ctx;

// ── palette ─────────────────────────────────────────────────────────
const BG: [f32; 4] = [0.055, 0.065, 0.085, 1.0];
const IDLE_FILL: [f32; 4] = [0.13, 0.15, 0.19, 1.0];
const IDLE_BORDER: [f32; 4] = [0.24, 0.27, 0.33, 1.0];
const ON_FILL: [f32; 4] = [0.15, 0.72, 0.42, 1.0];
const ON_BORDER: [f32; 4] = [0.45, 0.98, 0.62, 1.0];
const ON_CELL_TXT: [f32; 4] = [0.03, 0.10, 0.06, 1.0];
const TXT: [f32; 4] = [0.88, 0.90, 0.94, 1.0];
const DIM: [f32; 4] = [0.52, 0.56, 0.62, 1.0];
const ACCENT: [f32; 4] = [0.45, 0.68, 1.0, 1.0];
const AMBER: [f32; 4] = [0.92, 0.72, 0.28, 1.0];
const CYAN: [f32; 4] = [0.34, 0.82, 0.86, 1.0];
const GREEN: [f32; 3] = [0.34, 0.94, 0.56];
// Context tab bar — the Prism tab pattern, colours matched to the settings-panel tabs.
const TAB_ACTIVE_BG: [f32; 4] = [0.141, 0.247, 0.471, 0.30];
const TAB_HOVER_BG: [f32; 4] = [0.141, 0.247, 0.471, 0.15];
const BRONZE: [f32; 4] = [0.722, 0.592, 0.353, 1.0];
const TAB_LABEL_ON: [f32; 4] = [0.906, 0.882, 0.824, 1.0];
const TAB_LABEL_OFF: [f32; 4] = [0.561, 0.541, 0.490, 1.0];
const TAB_Y: f32 = 92.0;
const TAB_H: f32 = 30.0;

/// Frames a logged signal takes to fade out (~1.3 s at 60 Hz).
const FADE_FRAMES: f32 = 80.0;

/// The `(x, width)` of context tab `i` in a bar across the top of a `screen_x`-wide window.
fn tab_geom(screen_x: f32, i: usize) -> (f32, f32) {
    let margin = 40.0;
    let avail = (screen_x - 2.0 * margin).max(240.0);
    let w = (avail / Ctx::ALL.len() as f32).min(170.0);
    (margin + i as f32 * w, w)
}

/// Keyboard keys shown in the readout row (each lights while held).
const KEYS: [Key; 20] = [
    Key::W, Key::A, Key::S, Key::D, Key::Q, Key::E, Key::R, Key::F, Key::C, Key::Tab,
    Key::Space, Key::LeftShift, Key::LeftControl, Key::LeftAlt, Key::Escape,
    Key::Up, Key::Down, Key::Left, Key::Right, Key::Enter,
];

pub struct ControllerTester {
    /// This frame's input, captured in `update` for `render` to draw the diagram from.
    snap: Option<InputState>,
    /// Last frame's input — the other half of edge detection for signals.
    prev: Option<InputState>,
    ctx: Ctx,
    frame: u64,
    /// Recently fired signals `(frame_fired, label)`, newest first.
    log: VecDeque<(u64, &'static str)>,
    /// Edge state for the context-cycle control.
    cycle_prev: bool,

    bindings: InputMap,
    menu_prev: bool,
    ui_theme: Option<Theme>,
}

impl ControllerTester {
    pub fn new() -> Self {
        Self {
            snap: None,
            prev: None,
            ctx: Ctx::World,
            frame: 0,
            log: VecDeque::new(),
            cycle_prev: false,
            bindings: InputMap::wasd_and_mouse(),
            menu_prev: false,
            ui_theme: None,
        }
    }
}

// ── draw helpers ─────────────────────────────────────────────────────

fn text_centered(r: &mut Renderer, cx: f32, cy: f32, s: &str, size: f32, color: [f32; 4]) {
    let m = r.measure_text(s, size);
    r.draw_text(s, Vec2::new(cx - m.x * 0.5, cy - m.y * 0.5), size, color);
}

/// A square button indicator centred on `center`, lit when `on`.
fn cell(r: &mut Renderer, center: Vec2, side: f32, label: &str, on: bool) {
    let tl = center - Vec2::splat(side * 0.5);
    let (fill, border, txt) = if on {
        (ON_FILL, ON_BORDER, ON_CELL_TXT)
    } else {
        (IDLE_FILL, IDLE_BORDER, TXT)
    };
    r.draw_ui_panel(tl, Vec2::splat(side), fill, fill, 0.0, 8.0, if on { 2.0 } else { 1.5 }, border, 0.0);
    text_centered(r, center.x, center.y, label, 15.0, txt);
}

/// A vertical trigger fill bar (analog 0..1), captioned with `label` + the %.
fn trigger(r: &mut Renderer, center: Vec2, w: f32, h: f32, frac: f32, label: &str) {
    let frac = frac.clamp(0.0, 1.0);
    let tl = center - Vec2::new(w * 0.5, h * 0.5);
    r.draw_ui_panel(tl, Vec2::new(w, h), IDLE_FILL, IDLE_FILL, 0.0, 5.0, 1.5, IDLE_BORDER, 0.0);
    let fh = h * frac;
    if fh > 0.5 {
        r.draw_ui_panel(Vec2::new(tl.x, tl.y + (h - fh)), Vec2::new(w, fh), AMBER, AMBER, 0.0, 5.0, 0.0, AMBER, 0.0);
    }
    text_centered(r, center.x, tl.y - 11.0, label, 12.0, DIM);
    text_centered(r, center.x, center.y, &format!("{:.0}%", frac * 100.0), 11.0, if frac > 0.02 { TXT } else { DIM });
}

/// A stick box with a live dot at `(x, y)` (Y inverted), border lit when pressed (L3/R3).
fn stick(r: &mut Renderer, center: Vec2, half: f32, x: f32, y: f32, pressed: bool, label: &str) {
    let tl = center - Vec2::splat(half);
    let border = if pressed { ON_BORDER } else { IDLE_BORDER };
    r.draw_ui_panel(tl, Vec2::splat(half * 2.0), IDLE_FILL, IDLE_FILL, 0.0, 12.0, if pressed { 2.5 } else { 1.5 }, border, 0.0);
    r.draw_ui_panel(center - Vec2::splat(2.0), Vec2::splat(4.0), IDLE_BORDER, IDLE_BORDER, 0.0, 2.0, 0.0, IDLE_BORDER, 0.0);
    let dot = 8.0;
    let range = half - dot - 3.0;
    let p = Vec2::new(center.x + x.clamp(-1.0, 1.0) * range, center.y - y.clamp(-1.0, 1.0) * range);
    r.draw_ui_panel(p - Vec2::splat(dot), Vec2::splat(dot * 2.0), CYAN, CYAN, 0.0, dot, 0.0, CYAN, 0.0);
    text_centered(r, center.x, tl.y - 11.0, label, 12.0, DIM);
    text_centered(r, center.x, center.y + half + 12.0, &format!("{x:+.2}, {y:+.2}"), 11.0, DIM);
}

impl Scene for ControllerTester {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.ui_theme = Some(Theme::build(renderer));
        renderer.window().set_title("Flicker Controller Tester");
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        self.frame += 1;
        self.prev = self.snap.take();
        self.snap = Some(input.clone());

        if let Some((map, _, _)) = flicker_shell::take_pending_input() {
            self.bindings = map;
        }

        // Select context: click a tab, or cycle with Tab (keyboard) / Back-View (pad).
        if input.mouse_left_pressed {
            let sx = renderer.size().x;
            let c = input.mouse_position;
            for (i, ctx) in Ctx::ALL.iter().enumerate() {
                let (x, w) = tab_geom(sx, i);
                if c.x >= x && c.x <= x + w && c.y >= TAB_Y && c.y <= TAB_Y + TAB_H {
                    self.ctx = *ctx;
                }
            }
        }
        let cycle = input.key_down(Key::Tab)
            || input.gamepad(0).is_some_and(|g| g.button_down(GamepadButton::Select));
        if cycle && !self.cycle_prev {
            self.ctx = self.ctx.next();
        }
        self.cycle_prev = cycle;

        // Fire signals for this frame (last-frame edge detection over the context map).
        let prev = self.prev.as_ref();
        for bind in signals::binds(self.ctx) {
            if signals::fired(&bind, prev, input) {
                self.log.push_front((self.frame, bind.signal.label()));
            }
        }
        while self.log.len() > 8 {
            self.log.pop_back();
        }

        // Esc / Menu → pause overlay (edge-detected).
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
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let size = renderer.size();
        let cx = size.x * 0.5;
        let dcy = size.y * 0.50; // diagram centre; tab bar above, fired-signal message below

        renderer.draw_ui_panel(Vec2::ZERO, size, BG, BG, 0.0, 0.0, 0.0, BG, 0.0);

        // ── header ──
        renderer.draw_text("CONTROLLER TESTER", Vec2::new(40.0, 22.0), 22.0, ACCENT);
        renderer.draw_text(
            "click a tab · Tab / Back cycles context     Esc = menu",
            Vec2::new(40.0, 52.0),
            13.0,
            DIM,
        );

        let snap = self.snap.as_ref();
        let connected = snap.is_some_and(|s| s.gamepad_connected(0));
        let slots = snap.map_or(0, |s| s.gamepads().count());
        renderer.draw_text(
            &format!(
                "gamepad 0: {}   ·   {slots} slot(s)",
                if connected { "CONNECTED" } else { "not detected" }
            ),
            Vec2::new(40.0, 72.0),
            13.0,
            if connected { ON_BORDER } else { DIM },
        );

        // mouse readout (top-right)
        let (mpos, ml, mr, mm) = snap
            .map(|s| (s.mouse_position, s.mouse_left, s.mouse_right, s.mouse_middle))
            .unwrap_or((Vec2::ZERO, false, false, false));
        renderer.draw_text(
            &format!("mouse {:.0},{:.0}   L{} R{} M{}", mpos.x, mpos.y, ml as u8, mr as u8, mm as u8),
            Vec2::new(size.x - 250.0, 24.0),
            13.0,
            DIM,
        );

        // ── context tab bar (Prism tab pattern: active tint + bronze underline) ──
        let cursor = snap.map(|s| s.mouse_position).unwrap_or(Vec2::ZERO);
        for (i, ctx) in Ctx::ALL.iter().enumerate() {
            let (x, w) = tab_geom(size.x, i);
            let is_active = *ctx == self.ctx;
            let hovered =
                cursor.x >= x && cursor.x <= x + w && cursor.y >= TAB_Y && cursor.y <= TAB_Y + TAB_H;
            let bg = if is_active {
                TAB_ACTIVE_BG
            } else if hovered {
                TAB_HOVER_BG
            } else {
                [0.0; 4]
            };
            renderer.draw_ui_panel(Vec2::new(x, TAB_Y), Vec2::new(w, TAB_H), bg, bg, 0.0, 4.0, 0.0, bg, 0.0);
            if is_active {
                renderer.draw_ui_panel(
                    Vec2::new(x, TAB_Y + TAB_H - 3.0),
                    Vec2::new(w, 3.0),
                    BRONZE, BRONZE, 0.0, 0.0, 0.0, BRONZE, 0.0,
                );
            }
            let col = if is_active { TAB_LABEL_ON } else { TAB_LABEL_OFF };
            text_centered(renderer, x + w * 0.5, TAB_Y + TAB_H * 0.5, ctx.short(), 15.0, col);
        }

        // ── the controller diagram ──
        let bd = |b: GamepadButton| snap.and_then(|s| s.gamepad(0)).is_some_and(|gp| gp.button_down(b));
        let av = |a: GamepadAxis| snap.and_then(|s| s.gamepad(0)).map_or(0.0, |gp| gp.axis_value(a));

        let u = 44.0;
        let d = u + 12.0;

        // shoulders + triggers
        let ysh = dcy - 112.0;
        trigger(renderer, Vec2::new(cx - 300.0, ysh), 38.0, 60.0, av(GamepadAxis::LeftTrigger), "LT");
        cell(renderer, Vec2::new(cx - 220.0, ysh + 8.0), u, "LB", bd(GamepadButton::LeftBumper));
        cell(renderer, Vec2::new(cx + 220.0, ysh + 8.0), u, "RB", bd(GamepadButton::RightBumper));
        trigger(renderer, Vec2::new(cx + 300.0, ysh), 38.0, 60.0, av(GamepadAxis::RightTrigger), "RT");

        // d-pad
        let dc = Vec2::new(cx - 250.0, dcy + 6.0);
        cell(renderer, dc + Vec2::new(0.0, -d), u, "Up", bd(GamepadButton::DPadUp));
        cell(renderer, dc + Vec2::new(0.0, d), u, "Dn", bd(GamepadButton::DPadDown));
        cell(renderer, dc + Vec2::new(-d, 0.0), u, "Lt", bd(GamepadButton::DPadLeft));
        cell(renderer, dc + Vec2::new(d, 0.0), u, "Rt", bd(GamepadButton::DPadRight));

        // face buttons (A=South, B=East, X=West, Y=North)
        let fc = Vec2::new(cx + 250.0, dcy + 6.0);
        cell(renderer, fc + Vec2::new(0.0, -d), u, "Y", bd(GamepadButton::North));
        cell(renderer, fc + Vec2::new(0.0, d), u, "A", bd(GamepadButton::South));
        cell(renderer, fc + Vec2::new(-d, 0.0), u, "X", bd(GamepadButton::West));
        cell(renderer, fc + Vec2::new(d, 0.0), u, "B", bd(GamepadButton::East));

        // meta
        let m = 32.0;
        cell(renderer, Vec2::new(cx - 44.0, dcy - 10.0), m, "Bk", bd(GamepadButton::Select));
        cell(renderer, Vec2::new(cx, dcy - 10.0), m, "Xb", bd(GamepadButton::Guide));
        cell(renderer, Vec2::new(cx + 44.0, dcy - 10.0), m, "St", bd(GamepadButton::Start));

        // sticks
        let yst = dcy + 104.0;
        stick(
            renderer, Vec2::new(cx - 108.0, yst), 50.0,
            av(GamepadAxis::LeftStickX), av(GamepadAxis::LeftStickY),
            bd(GamepadButton::LeftStick), "L3",
        );
        stick(
            renderer, Vec2::new(cx + 108.0, yst), 50.0,
            av(GamepadAxis::RightStickX), av(GamepadAxis::RightStickY),
            bd(GamepadButton::RightStick), "R3",
        );

        // ── the fired SIGNAL, as a large message under the map ──
        let msg_y = size.y - 90.0;
        if let Some((fired_frame, label)) = self.log.front() {
            let age = self.frame.saturating_sub(*fired_frame) as f32;
            let alpha = (1.0 - age / FADE_FRAMES).clamp(0.25, 1.0);
            text_centered(renderer, cx, msg_y, label, 34.0, [GREEN[0], GREEN[1], GREEN[2], alpha]);
        } else {
            text_centered(renderer, cx, msg_y, "press a control to see its signal", 17.0, DIM);
        }
        // a thin recent-history line beneath the big message
        let recent: Vec<&str> = self.log.iter().skip(1).take(5).map(|(_, l)| *l).collect();
        if !recent.is_empty() {
            text_centered(renderer, cx, msg_y + 27.0, &recent.join("     "), 12.0, DIM);
        }

        // ── keyboard pills (bottom) ──
        let ky = size.y - 28.0;
        renderer.draw_text("KEYS", Vec2::new(40.0, ky), 12.0, DIM);
        let mut x = 88.0;
        for k in KEYS {
            let on = snap.is_some_and(|s| s.key_down(k));
            let lbl = format!("{k}");
            let w = renderer.measure_text(&lbl, 12.0).x + 12.0;
            let (fill, border, txt) = if on { (ON_FILL, ON_BORDER, ON_CELL_TXT) } else { (IDLE_FILL, IDLE_BORDER, DIM) };
            renderer.draw_ui_panel(Vec2::new(x, ky - 3.0), Vec2::new(w, 19.0), fill, fill, 0.0, 5.0, 1.0, border, 0.0);
            renderer.draw_text(&lbl, Vec2::new(x + 6.0, ky), 12.0, txt);
            x += w + 5.0;
            if x > size.x - 60.0 {
                break;
            }
        }
    }
}

impl Default for ControllerTester {
    fn default() -> Self {
        Self::new()
    }
}
