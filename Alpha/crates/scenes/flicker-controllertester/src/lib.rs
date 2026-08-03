//! flicker-controllertester: a live **input-BUS inspector**.
//!
//! The left half is the physical side — the controller diagram (face buttons,
//! d-pad, bumpers, triggers, sticks light up / move as you press them) plus the
//! keyboard readout, drawn straight from the current [`InputState`] snapshot —
//! and, alongside it, the **analog channel**: the current [`AnalogFrame`] latched
//! from the device's volatile cache (`seq`, staleness, current-vs-previous delta).
//!
//! The right half is the logical side — this frame's input driven through a REAL
//! [`Router`] (spec §10). A stateful core [`Resolver`] turns the snapshot into
//! `Fired` edges against the SELECTED context's [`ContextualBindings`], those are
//! wrapped into `InputEvent`s and `dispatch`ed through a small demo handler chain
//! (system ▸ scene-root ▸ modal ▸ gameplay-base), and three panels report:
//!   1. the active `ContextualBindings` **stack** (pushed contexts over base World),
//!   2. the **focus chain** — which layer owns input (declares the active context),
//!   3. which handler **consumed** each fired signal this frame (from the
//!      `DispatchReport`).
//!
//! The context TAB BAR (World / Menu / Radial / TextEntry) selects which real
//! `InputContext` sits on top of the stack, so you can watch the SAME control
//! resolve to a different signal — and get consumed by a different layer — per
//! context. Cycle with Tab (keyboard) or Back/View (pad).
//!
//! A STANDALONE diagnostic (`34BE2610`) built on the click-trainer scene toolkit
//! (`draw_ui_panel` + `draw_text`, no Lua, no `ui_elements.json`); `prism-alpha`
//! hosts it. Esc / Start open the pause menu.

use std::time::{Duration, Instant};

use flicker::render::{Renderer, Vec2};
use flicker::scene::{Scene, Transition};
use flicker_input_core::{
    AbstractControls, ActionSignal, AnalogFrame, ContextualBindings, EventKind, Fired, GamepadAxis,
    GamepadButton, GamepadConfig, InputBinding, InputContext, InputMap, InputState, Key, Resolver,
};
use flicker_input_router::{DispatchReport, Flow, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

// ── palette ─────────────────────────────────────────────────────────
const BG: [f32; 4] = [0.055, 0.065, 0.085, 1.0];
const PANEL_BG: [f32; 4] = [0.088, 0.10, 0.128, 1.0];
const PANEL_BORDER: [f32; 4] = [0.22, 0.25, 0.31, 1.0];
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
const GREENV: [f32; 4] = [0.34, 0.94, 0.56, 1.0];
// Context tab bar — the Prism tab pattern, colours matched to the settings-panel tabs.
const TAB_ACTIVE_BG: [f32; 4] = [0.141, 0.247, 0.471, 0.30];
const TAB_HOVER_BG: [f32; 4] = [0.141, 0.247, 0.471, 0.15];
const BRONZE: [f32; 4] = [0.722, 0.592, 0.353, 1.0];
const TAB_LABEL_ON: [f32; 4] = [0.906, 0.882, 0.824, 1.0];
const TAB_LABEL_OFF: [f32; 4] = [0.561, 0.541, 0.490, 1.0];
const TAB_Y: f32 = 86.0;
const TAB_H: f32 = 28.0;

/// The real `InputContext`s the tab bar cycles (spec §10). World is the immutable
/// base; selecting any other tab PUSHES it on top of World.
const CONTEXTS: [InputContext; 4] = [
    InputContext::World,
    InputContext::Menu,
    InputContext::Radial,
    InputContext::TextEntry,
];

/// The demo handler chain, highest input priority first (spec §4.2 bus, minus the
/// walker layer the tester has no Lua HUD for). `SCENE_ROOT` is the layer whose
/// consumed `Menu` press opens the pause overlay.
const LAYER_NAMES: [&str; 4] = ["System", "Scene root", "Modal", "Gameplay"];
const SCENE_ROOT: usize = 1;

/// Keyboard keys shown in the readout row (each lights while held).
const KEYS: [Key; 20] = [
    Key::W, Key::A, Key::S, Key::D, Key::Q, Key::E, Key::R, Key::F, Key::C, Key::Tab,
    Key::Space, Key::LeftShift, Key::LeftControl, Key::LeftAlt, Key::Escape,
    Key::Up, Key::Down, Key::Left, Key::Right, Key::Enter,
];

// ── the demo maps (physical → signal, per context) ──────────────────

fn gb(b: GamepadButton) -> InputBinding {
    InputBinding::GamepadButton(b)
}
fn kb(k: Key) -> InputBinding {
    InputBinding::Key(k)
}

/// World gameplay: gamepad souls-combat + keyboard movement. Both device families
/// are bound so the inspector is useful with a pad OR a keyboard.
fn world_map() -> InputMap {
    use ActionSignal as A;
    use GamepadButton as B;
    let mut m = InputMap::empty();
    // Gamepad combat (souls layout).
    m.bind(A::AttackLight, gb(B::RightBumper));
    m.bind(A::AttackHeavy, gb(B::RightTrigger));
    m.bind(A::Defend, gb(B::LeftBumper));
    m.bind(A::Special, gb(B::LeftTrigger));
    m.bind(A::Dodge, gb(B::East));
    m.bind(A::Jump, gb(B::North));
    m.bind(A::Interact, gb(B::South));
    m.bind(A::Interact, gb(B::West));
    m.bind(A::LockOn, gb(B::RightStick));
    m.bind(A::Crouch, gb(B::LeftStick));
    m.bind(A::Menu, gb(B::Start));
    m.bind(A::Quit, gb(B::Guide)); // demo: the System layer captures Quit
    // Gamepad d-pad → digital movement (keyboard/d-pad path).
    m.bind(A::MoveForward, gb(B::DPadUp));
    m.bind(A::MoveBackward, gb(B::DPadDown));
    m.bind(A::StrafeLeft, gb(B::DPadLeft));
    m.bind(A::StrafeRight, gb(B::DPadRight));
    // Keyboard.
    m.bind(A::MoveForward, kb(Key::W));
    m.bind(A::MoveBackward, kb(Key::S));
    m.bind(A::StrafeLeft, kb(Key::A));
    m.bind(A::StrafeRight, kb(Key::D));
    m.bind(A::Jump, kb(Key::Space));
    m.bind(A::Sprint, kb(Key::LeftShift));
    m.bind(A::Crouch, kb(Key::LeftControl));
    m.bind(A::Interact, kb(Key::E));
    m.bind(A::Reload, kb(Key::R));
    m.bind(A::UseItem, kb(Key::F));
    m.bind(A::Menu, kb(Key::Escape));
    m.bind(A::Quit, kb(Key::Q)); // demo: System-captured
    m
}

/// Menu navigation: Confirm / Cancel / Nav / Tab. Menus fire on the same edges as
/// the promoted mockup; here the Modal layer consumes them.
fn menu_map() -> InputMap {
    use ActionSignal as A;
    use GamepadButton as B;
    let mut m = InputMap::empty();
    m.bind(A::Confirm, gb(B::South));
    m.bind(A::Cancel, gb(B::East));
    m.bind(A::TabPrev, gb(B::LeftBumper));
    m.bind(A::TabNext, gb(B::RightBumper));
    m.bind(A::NavUp, gb(B::DPadUp));
    m.bind(A::NavDown, gb(B::DPadDown));
    m.bind(A::NavLeft, gb(B::DPadLeft));
    m.bind(A::NavRight, gb(B::DPadRight));
    m.bind(A::Confirm, kb(Key::Enter));
    m.bind(A::Cancel, kb(Key::Escape));
    m.bind(A::NavUp, kb(Key::Up));
    m.bind(A::NavDown, kb(Key::Down));
    m.bind(A::NavLeft, kb(Key::Left));
    m.bind(A::NavRight, kb(Key::Right));
    m
}

/// Radial selector: the SAME South button that Confirms in Menu here selects an
/// item — the point of the per-context inspector.
fn radial_map() -> InputMap {
    use ActionSignal as A;
    use GamepadButton as B;
    let mut m = InputMap::empty();
    m.bind(A::ItemSelect, gb(B::South));
    m.bind(A::Cancel, gb(B::East));
    m.bind(A::NavUp, gb(B::DPadUp));
    m.bind(A::NavDown, gb(B::DPadDown));
    m.bind(A::NavLeft, gb(B::DPadLeft));
    m.bind(A::NavRight, gb(B::DPadRight));
    m.bind(A::ItemSelect, kb(Key::Enter));
    m.bind(A::Cancel, kb(Key::Escape));
    m.bind(A::NavUp, kb(Key::Up));
    m.bind(A::NavDown, kb(Key::Down));
    m.bind(A::NavLeft, kb(Key::Left));
    m.bind(A::NavRight, kb(Key::Right));
    m
}

/// A real `TextEntry` map is EMPTY (typed chars flow to the focused field, gameplay
/// suppressed). The tester instead binds Enter/Esc so the terminal's dedicated
/// `SubmitText` / `CancelText` signals (spec R2) are visible — swallowed by the
/// exclusive-owner (Modal) layer's capture pass.
fn text_map() -> InputMap {
    use ActionSignal as A;
    let mut m = InputMap::empty();
    m.bind(A::SubmitText, kb(Key::Enter));
    m.bind(A::CancelText, kb(Key::Escape));
    m
}

/// The context service the inspector resolves against: World base + Menu / Radial /
/// TextEntry maps, each on its own stack slot when its tab is selected.
fn demo_bindings() -> ContextualBindings {
    ContextualBindings::new(world_map())
        .with(InputContext::Menu, menu_map())
        .with(InputContext::Radial, radial_map())
        .with(InputContext::TextEntry, text_map())
}

// ── the demo handler chain (a real Router bus) ──────────────────────

/// UI / menu / dialog / text signals — the Modal layer's remit when a modal
/// context is active.
fn is_ui_signal(s: ActionSignal) -> bool {
    use ActionSignal::*;
    matches!(
        s,
        Confirm | Cancel | NavUp | NavDown | NavLeft | NavRight | TabNext | TabPrev | ItemSelect
            | Yes | No | Activate | SubmitText | CancelText
    )
}

/// Gameplay signals — the base handler's remit (movement / combat / interaction).
fn is_gameplay_signal(s: ActionSignal) -> bool {
    use ActionSignal::*;
    matches!(
        s,
        MoveForward | MoveBackward | StrafeLeft | StrafeRight | MoveUp | MoveDown | LookUp
            | LookDown | LookLeft | LookRight | PrimaryAction | SecondaryAction | Jump | Sprint
            | Crouch | Interact | Reload | AttackLight | AttackHeavy | Defend | Special | Dodge
            | LockOn | UseItem
    )
}

/// **[0]** System / global — a capture-only layer. Claims `Quit` in the top-down
/// capture pass before any handler can see it (the spec's screenshot / quit /
/// debug-console tier).
struct SystemLayer;
impl InputHandler for SystemLayer {
    fn capture(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if ev.signal == ActionSignal::Quit {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
    fn handle(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }
}

/// **[1]** Scene / mode root — declares the base `World` context and consumes the
/// `Menu` press, which the scene turns into a pause `Transition`.
struct SceneRoot;
impl InputHandler for SceneRoot {
    fn declares_context(&self) -> Option<InputContext> {
        Some(InputContext::World)
    }
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if ev.signal == ActionSignal::Menu && ev.kind == EventKind::Press {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

/// **[2]** Context / modal — the pushed modal layer. It declares (owns) the active
/// non-World context. For `TextEntry` it is an EXCLUSIVE owner: its capture pass
/// swallows everything top-down (nothing leaks to gameplay). For `Menu` / `Radial`
/// it consumes the UI signals in the handle pass.
struct ModalLayer {
    active: InputContext,
}
impl InputHandler for ModalLayer {
    fn declares_context(&self) -> Option<InputContext> {
        (self.active != InputContext::World).then_some(self.active)
    }
    fn capture(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if self.active == InputContext::TextEntry {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        let modal = self.active != InputContext::World && self.active != InputContext::TextEntry;
        if modal && is_ui_signal(ev.signal) {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

/// **[3]** Gameplay at base — last in the chain, so it acts only on a `Pass` from
/// everything above. Consumes the gameplay signals; anything else falls through as
/// "passed".
struct GameplayBase;
impl InputHandler for GameplayBase {
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if is_gameplay_signal(ev.signal) {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

// ── the per-frame bus view model (computed in update, drawn in render) ──

/// One focus-chain row: layer name, the context it declares (if any), a short
/// note, and whether it currently owns input.
struct LayerRow {
    name: &'static str,
    declares: Option<InputContext>,
    note: &'static str,
    owns: bool,
}

/// One consumed-by row: the fired signal, its edge, and the layer that consumed it
/// (`None` = it passed the whole chain unconsumed).
struct ConsumedRow {
    label: &'static str,
    kind: EventKind,
    consumer: Option<&'static str>,
}

/// This frame's routing outcome, distilled from the `DispatchReport` for render.
#[derive(Default)]
struct BusFrame {
    layers: Vec<LayerRow>,
    consumed: Vec<ConsumedRow>,
}

/// A short note for the Modal layer given the active context.
fn modal_note(active: InputContext) -> &'static str {
    match active.id() {
        3 => "exclusive owner (capture)", // TextEntry
        1 | 2 => "consumes UI signals",   // Menu / Radial
        _ => "(inactive)",
    }
}

/// Distil the chain's declared contexts + the dispatch report into a `BusFrame`.
/// The owner is the topmost layer whose `declares_context()` equals the active
/// context (spec §5, "active context = top of focus chain").
fn build_bus_frame(active: InputContext, fired: &[Fired], report: &DispatchReport) -> BusFrame {
    let modal_ctx = (active != InputContext::World).then_some(active);
    let meta: [(Option<InputContext>, &'static str); 4] = [
        (None, "capture: Quit"),
        (Some(InputContext::World), "consumes Menu (press)"),
        (modal_ctx, modal_note(active)),
        (None, "base: gameplay signals"),
    ];
    let owner = meta.iter().position(|(d, _)| *d == Some(active));
    let layers = meta
        .iter()
        .enumerate()
        .map(|(i, (declares, note))| LayerRow {
            name: LAYER_NAMES[i],
            declares: *declares,
            note,
            owns: owner == Some(i),
        })
        .collect();
    let consumed = fired
        .iter()
        .map(|f| {
            let consumer = (0..LAYER_NAMES.len())
                .find(|&i| report.consumed_by(i, f.signal))
                .map(|i| LAYER_NAMES[i]);
            ConsumedRow { label: f.signal.label(), kind: f.kind, consumer }
        })
        .collect();
    BusFrame { layers, consumed }
}

/// Human label for a built-in `InputContext` (the tab bar + panels read this).
fn ctx_label(c: InputContext) -> &'static str {
    match c.id() {
        0 => "World",
        1 => "Menu",
        2 => "Radial",
        3 => "TextEntry",
        4 => "Mounted",
        5 => "Flying",
        6 => "Vehicle",
        _ => "Custom",
    }
}

/// The `(x, width)` of context tab `i` in a bar across the top of a `screen_x`-wide window.
fn tab_geom(screen_x: f32, i: usize) -> (f32, f32) {
    let margin = 40.0;
    let avail = (screen_x - 2.0 * margin).max(240.0);
    let w = (avail / CONTEXTS.len() as f32).min(170.0);
    (margin + i as f32 * w, w)
}

pub struct ControllerTester {
    /// This frame's input, captured in `update` for `render` to draw the diagram from.
    snap: Option<InputState>,
    /// This frame's analog latch (device's volatile cache copy) + last frame's, so
    /// `render` can show the current sample and the current-vs-previous delta.
    latch: Option<AnalogFrame>,
    prev_latch: Option<AnalogFrame>,

    /// The context service the inspector resolves against; its active-context stack
    /// is driven by the tab bar. `stack` mirrors the (private) stack for panel (1).
    bindings: ContextualBindings,
    stack: Vec<InputContext>,

    /// The new-input-model seam (spec §5/§9): a stateful edge [`Resolver`] (owns the
    /// prev snapshot + press-times), a REUSED `Fired` scratch buffer (no per-frame
    /// alloc — RT-7), a monotonic `tick` = the resolver's `TickTime`, and the
    /// distilled `bus` view model for `render`.
    resolver: Resolver,
    ev: Vec<Fired>,
    tick: u64,
    bus: BusFrame,

    /// Edge state for the context-cycle control (Tab / pad Back).
    cycle_prev: bool,

    gamepad_config: GamepadConfig,
    controls: AbstractControls,
    ui_theme: Option<Theme>,
}

impl ControllerTester {
    pub fn new() -> Self {
        Self {
            snap: None,
            latch: None,
            prev_latch: None,
            bindings: demo_bindings(),
            stack: vec![InputContext::World],
            resolver: Resolver::new(),
            ev: Vec::new(),
            tick: 0,
            bus: BusFrame::default(),
            cycle_prev: false,
            gamepad_config: GamepadConfig::default(),
            controls: AbstractControls::default(),
            ui_theme: None,
        }
    }

    /// Select the context on top of the stack (base World, plus the picked context
    /// if it is not World). Drives the real `ContextualBindings` push/pop and the
    /// `stack` mirror together so they can never diverge.
    fn select_context(&mut self, ctx: InputContext) {
        while self.bindings.pop().is_some() {} // reset to [World]
        self.stack.clear();
        self.stack.push(InputContext::World);
        if ctx != InputContext::World {
            self.bindings.push(ctx);
            self.stack.push(ctx);
        }
    }
}

// ── draw helpers ─────────────────────────────────────────────────────

fn text_centered(r: &mut Renderer, cx: f32, cy: f32, s: &str, size: f32, color: [f32; 4]) {
    let m = r.measure_text(s, size);
    r.draw_text(s, Vec2::new(cx - m.x * 0.5, cy - m.y * 0.5), size, color);
}

/// A square button indicator centred on `center`, lit when `on`. Text scales with
/// the cell so the diagram stays legible at any window size.
fn cell(r: &mut Renderer, center: Vec2, side: f32, label: &str, on: bool) {
    let tl = center - Vec2::splat(side * 0.5);
    let (fill, border, txt) = if on {
        (ON_FILL, ON_BORDER, ON_CELL_TXT)
    } else {
        (IDLE_FILL, IDLE_BORDER, TXT)
    };
    r.draw_ui_panel(tl, Vec2::splat(side), fill, fill, 0.0, side * 0.18, if on { 2.0 } else { 1.5 }, border, 0.0);
    text_centered(r, center.x, center.y, label, (side * 0.34).max(9.0), txt);
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
    text_centered(r, center.x, tl.y - h * 0.18, label, (w * 0.3).clamp(9.0, 12.0), DIM);
    text_centered(r, center.x, center.y, &format!("{:.0}%", frac * 100.0), (w * 0.28).clamp(8.0, 11.0), if frac > 0.02 { TXT } else { DIM });
}

/// A stick box with a live dot at `(x, y)` (Y inverted), border lit when pressed (L3/R3).
fn stick(r: &mut Renderer, center: Vec2, half: f32, x: f32, y: f32, pressed: bool, label: &str) {
    let tl = center - Vec2::splat(half);
    let border = if pressed { ON_BORDER } else { IDLE_BORDER };
    r.draw_ui_panel(tl, Vec2::splat(half * 2.0), IDLE_FILL, IDLE_FILL, 0.0, half * 0.24, if pressed { 2.5 } else { 1.5 }, border, 0.0);
    r.draw_ui_panel(center - Vec2::splat(2.0), Vec2::splat(4.0), IDLE_BORDER, IDLE_BORDER, 0.0, 2.0, 0.0, IDLE_BORDER, 0.0);
    let dot = half * 0.16;
    let range = half - dot - 3.0;
    let p = Vec2::new(center.x + x.clamp(-1.0, 1.0) * range, center.y - y.clamp(-1.0, 1.0) * range);
    r.draw_ui_panel(p - Vec2::splat(dot), Vec2::splat(dot * 2.0), CYAN, CYAN, 0.0, dot, 0.0, CYAN, 0.0);
    text_centered(r, center.x, tl.y - half * 0.22, label, (half * 0.24).clamp(9.0, 12.0), DIM);
    text_centered(r, center.x, center.y + half + half * 0.22, &format!("{x:+.2}, {y:+.2}"), (half * 0.22).clamp(8.0, 11.0), DIM);
}

/// Draw a titled panel box and return the y at which content rows start.
fn panel(r: &mut Renderer, x: f32, y: f32, w: f32, h: f32, title: &str) -> f32 {
    r.draw_ui_panel(Vec2::new(x, y), Vec2::new(w, h), PANEL_BG, PANEL_BG, 0.0, 8.0, 1.5, PANEL_BORDER, 0.0);
    r.draw_text(title, Vec2::new(x + 12.0, y + 8.0), 12.0, ACCENT);
    y + 30.0
}

impl Scene for ControllerTester {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.ui_theme = Some(Theme::build(renderer));
        renderer.window().set_title("Flicker Controller Tester \u{00b7} Bus Inspector");
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        self.tick = self.tick.wrapping_add(1);
        self.snap = Some(input.clone());
        // Remember last frame's latch so render can show the current-vs-previous delta.
        self.prev_latch = self.latch;
        self.latch = input.analog_latch();

        // Select context: click a tab, or cycle with Tab (keyboard) / Back-View (pad).
        if input.mouse_left_pressed {
            let sx = renderer.size().x;
            let c = input.mouse_position;
            for (i, ctx) in CONTEXTS.iter().enumerate() {
                let (x, w) = tab_geom(sx, i);
                if c.x >= x && c.x <= x + w && c.y >= TAB_Y && c.y <= TAB_Y + TAB_H {
                    self.select_context(*ctx);
                }
            }
        }
        let cycle = input.key_down(Key::Tab)
            || input.gamepad(0).is_some_and(|g| g.button_down(GamepadButton::Select));
        if cycle && !self.cycle_prev {
            let cur = self.bindings.active();
            let i = CONTEXTS.iter().position(|c| *c == cur).unwrap_or(0);
            self.select_context(CONTEXTS[(i + 1) % CONTEXTS.len()]);
        }
        self.cycle_prev = cycle;

        // ── The seam (spec §5/§10): ONE resolve + ONE dispatch. The core Resolver
        // turns the snapshot into Fired edges over the SELECTED context's map; each
        // is wrapped into an InputEvent and dispatched through the demo chain. ──
        self.ev.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad_config,
            input,
            self.tick,
            &mut self.ev,
        );
        let active = self.bindings.active();
        let events: Vec<InputEvent> = self
            .ev
            .iter()
            .map(|f| InputEvent::from_fired(f, active, input))
            .collect();

        let mut system = SystemLayer;
        let mut root = SceneRoot;
        let mut modal = ModalLayer { active };
        let mut gameplay = GameplayBase;
        let mut route = RouteCtx::new();
        let report: DispatchReport = {
            let mut chain: [&mut dyn InputHandler; 4] =
                [&mut system, &mut root, &mut modal, &mut gameplay];
            Router::dispatch(&events, &mut chain, &mut route)
        };

        // Distil the routing outcome for render (panels 2 + 3).
        self.bus = build_bus_frame(active, &self.ev, &report);

        // Scene-root consumed the Menu press → open the pause overlay. Under a modal
        // context Menu never resolves, so pausing is a World-context action (Tab back
        // to World, or press Start / Esc there).
        if report.consumed_by(SCENE_ROOT, ActionSignal::Menu) {
            let theme = self.ui_theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                self.bindings.active_map(),
                &self.controls,
                &self.gamepad_config,
            )));
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let size = renderer.size();
        let margin = 18.0;
        renderer.draw_ui_panel(Vec2::ZERO, size, BG, BG, 0.0, 0.0, 0.0, BG, 0.0);

        let snap = self.snap.as_ref();

        // ── header ──
        renderer.draw_text("CONTROLLER TESTER \u{00b7} LIVE BUS INSPECTOR", Vec2::new(margin, 16.0), 20.0, ACCENT);
        renderer.draw_text(
            "click a tab \u{00b7} Tab / Back cycles context \u{00b7} Esc / Start = menu \u{00b7} Q / Guide = quit",
            Vec2::new(margin, 44.0),
            12.0,
            DIM,
        );
        let connected = snap.is_some_and(|s| s.gamepad_connected(0));
        let slots = snap.map_or(0, |s| s.gamepads().count());
        renderer.draw_text(
            &format!(
                "gamepad 0: {}   \u{00b7}   {slots} slot(s)   \u{00b7}   tick {}",
                if connected { "CONNECTED" } else { "not detected" },
                self.tick
            ),
            Vec2::new(margin, 64.0),
            12.0,
            if connected { ON_BORDER } else { DIM },
        );
        let (mpos, ml, mr, mm) = snap
            .map(|s| (s.mouse_position, s.mouse_left, s.mouse_right, s.mouse_middle))
            .unwrap_or((Vec2::ZERO, false, false, false));
        renderer.draw_text(
            &format!("mouse {:.0},{:.0}  L{} R{} M{}", mpos.x, mpos.y, ml as u8, mr as u8, mm as u8),
            Vec2::new(size.x - 230.0, 20.0),
            12.0,
            DIM,
        );

        // ── context tab bar (Prism tab pattern: active tint + bronze underline) ──
        let cursor = snap.map(|s| s.mouse_position).unwrap_or(Vec2::ZERO);
        let active_ctx = self.bindings.active();
        for (i, ctx) in CONTEXTS.iter().enumerate() {
            let (x, w) = tab_geom(size.x, i);
            let is_active = *ctx == active_ctx;
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
            text_centered(renderer, x + w * 0.5, TAB_Y + TAB_H * 0.5, ctx_label(*ctx), 14.0, col);
        }

        // ── layout: left column = physical (diagram + analog), right = the bus ──
        let panel_w = (size.x * 0.34).clamp(240.0, 420.0);
        let panel_x = size.x - panel_w - margin;
        let content_top = TAB_Y + TAB_H + 12.0;
        let content_bottom = size.y - 34.0;
        let content_h = (content_bottom - content_top).max(140.0);
        let left_x0 = margin;
        let left_w = (panel_x - margin - left_x0).max(220.0);

        // Diagram occupies the top ~58% of the left column; the analog panel the rest.
        // Scale so it always fits both the left width and its vertical share.
        const DIAG_HALF_W: f32 = 320.0;
        const DIAG_HALF_H: f32 = 170.0;
        let diagram_h = content_h * 0.58;
        let s = (left_w / (2.0 * DIAG_HALF_W))
            .min(diagram_h / (2.0 * DIAG_HALF_H))
            .clamp(0.5, 1.2);
        let dcx = left_x0 + left_w * 0.5;
        let dcy = content_top + diagram_h * 0.5;

        // ── the controller diagram (discrete snapshot) ──
        let bd = |b: GamepadButton| snap.and_then(|s| s.gamepad(0)).is_some_and(|gp| gp.button_down(b));
        let av = |a: GamepadAxis| snap.and_then(|s| s.gamepad(0)).map_or(0.0, |gp| gp.axis_value(a));
        let at = |dx: f32, dy: f32| Vec2::new(dcx + dx * s, dcy + dy * s);
        let u = 44.0 * s;
        let meta_side = 32.0 * s;

        // shoulders + triggers
        trigger(renderer, at(-300.0, -112.0), 38.0 * s, 60.0 * s, av(GamepadAxis::LeftTrigger), "LT");
        cell(renderer, at(-220.0, -104.0), u, "LB", bd(GamepadButton::LeftBumper));
        cell(renderer, at(220.0, -104.0), u, "RB", bd(GamepadButton::RightBumper));
        trigger(renderer, at(300.0, -112.0), 38.0 * s, 60.0 * s, av(GamepadAxis::RightTrigger), "RT");
        // d-pad (centred at -250, 6; spacing 56)
        cell(renderer, at(-250.0, -50.0), u, "Up", bd(GamepadButton::DPadUp));
        cell(renderer, at(-250.0, 62.0), u, "Dn", bd(GamepadButton::DPadDown));
        cell(renderer, at(-306.0, 6.0), u, "Lt", bd(GamepadButton::DPadLeft));
        cell(renderer, at(-194.0, 6.0), u, "Rt", bd(GamepadButton::DPadRight));
        // face buttons (A=South, B=East, X=West, Y=North)
        cell(renderer, at(250.0, -50.0), u, "Y", bd(GamepadButton::North));
        cell(renderer, at(250.0, 62.0), u, "A", bd(GamepadButton::South));
        cell(renderer, at(194.0, 6.0), u, "X", bd(GamepadButton::West));
        cell(renderer, at(306.0, 6.0), u, "B", bd(GamepadButton::East));
        // meta
        cell(renderer, at(-44.0, -10.0), meta_side, "Bk", bd(GamepadButton::Select));
        cell(renderer, at(0.0, -10.0), meta_side, "Xb", bd(GamepadButton::Guide));
        cell(renderer, at(44.0, -10.0), meta_side, "St", bd(GamepadButton::Start));
        // sticks
        stick(
            renderer, at(-108.0, 104.0), 50.0 * s,
            av(GamepadAxis::LeftStickX), av(GamepadAxis::LeftStickY),
            bd(GamepadButton::LeftStick), "L3",
        );
        stick(
            renderer, at(108.0, 104.0), 50.0 * s,
            av(GamepadAxis::RightStickX), av(GamepadAxis::RightStickY),
            bd(GamepadButton::RightStick), "R3",
        );

        // ── analog channel panel (the volatile latch, alongside the discrete bus) ──
        let analog_top = content_top + diagram_h + 8.0;
        let analog_h = content_bottom - analog_top;
        let mut ay = panel(renderer, left_x0, analog_top, left_w, analog_h, "ANALOG LATCH  \u{00b7}  input.analog_latch() (volatile cache)");
        match self.latch {
            Some(f) => {
                let age_ms = Instant::now().saturating_duration_since(f.captured).as_secs_f32() * 1000.0;
                let stale = age_ms > 100.0;
                let dseq = self.prev_latch.map_or(0, |p| f.seq.saturating_sub(p.seq));
                renderer.draw_text(
                    &format!("seq {}  (\u{0394}+{})    age {:.1} ms    {}", f.seq, dseq, age_ms, if stale { "STALE" } else { "LIVE" }),
                    Vec2::new(left_x0 + 14.0, ay),
                    12.0,
                    if stale { AMBER } else { GREENV },
                );
                ay += 20.0;
                // Current sample + current-vs-previous-frame delta (sticks + triggers).
                let p = self.prev_latch.unwrap_or(f);
                let dl = f.left_stick - p.left_stick;
                let dr = f.right_stick - p.right_stick;
                renderer.draw_text(&format!("L stick  ({:+.2}, {:+.2})    \u{0394}({:+.2}, {:+.2})", f.left_stick.x, f.left_stick.y, dl.x, dl.y), Vec2::new(left_x0 + 14.0, ay), 12.0, TXT);
                ay += 18.0;
                renderer.draw_text(&format!("R stick  ({:+.2}, {:+.2})    \u{0394}({:+.2}, {:+.2})", f.right_stick.x, f.right_stick.y, dr.x, dr.y), Vec2::new(left_x0 + 14.0, ay), 12.0, TXT);
                ay += 18.0;
                renderer.draw_text(&format!("L trig   {:.2}         \u{0394}{:+.2}", f.left_trigger, f.left_trigger - p.left_trigger), Vec2::new(left_x0 + 14.0, ay), 12.0, TXT);
                ay += 18.0;
                renderer.draw_text(&format!("R trig   {:.2}         \u{0394}{:+.2}", f.right_trigger, f.right_trigger - p.right_trigger), Vec2::new(left_x0 + 14.0, ay), 12.0, TXT);
                ay += 18.0;
                renderer.draw_text("120 Hz cache sampled off the discrete bus; seq climbs while live", Vec2::new(left_x0 + 14.0, ay), 10.0, DIM);
            }
            None => {
                renderer.draw_text("no analog latch this frame (device not sampling)", Vec2::new(left_x0 + 14.0, ay), 12.0, DIM);
            }
        }

        // ── right column: the three bus panels ──
        let gap = 8.0;
        let ph = (content_h - 2.0 * gap) / 3.0;
        let p1_y = content_top;
        let p2_y = content_top + ph + gap;
        let p3_y = content_top + 2.0 * (ph + gap);

        // (1) the active ContextualBindings stack (top of stack first, base = World).
        let mut cy = panel(renderer, panel_x, p1_y, panel_w, ph, "CONTEXT STACK");
        let depth = self.stack.len();
        for (i, ctx) in self.stack.iter().enumerate().rev() {
            let is_top = i == depth - 1;
            let (marker, col) = if is_top { ("\u{25b8} ", ACCENT) } else { ("  ", TXT) };
            renderer.draw_text(&format!("{marker}{}", ctx_label(*ctx)), Vec2::new(panel_x + 14.0, cy), 14.0, col);
            let tag = if is_top { "active" } else if i == 0 { "base" } else { "" };
            if !tag.is_empty() {
                let tw = renderer.measure_text(tag, 11.0).x;
                renderer.draw_text(tag, Vec2::new(panel_x + panel_w - 14.0 - tw, cy + 2.0), 11.0, DIM);
            }
            cy += 22.0;
        }

        // (2) the focus chain — which layer owns input (declares the active context).
        let mut cy = panel(renderer, panel_x, p2_y, panel_w, ph, "FOCUS CHAIN  (owner = top of chain)");
        for (i, layer) in self.bus.layers.iter().enumerate() {
            let col = if layer.owns { GREENV } else { TXT };
            renderer.draw_text(&format!("{i} {}", layer.name), Vec2::new(panel_x + 14.0, cy), 13.0, col);
            let mid = match layer.declares {
                Some(c) => format!("ctx {}", ctx_label(c)),
                None => layer.note.to_string(),
            };
            renderer.draw_text(&mid, Vec2::new(panel_x + panel_w * 0.40, cy), 11.0, if layer.owns { GREENV } else { DIM });
            if layer.owns {
                let tag = "\u{25c0} OWNS";
                let tw = renderer.measure_text(tag, 11.0).x;
                renderer.draw_text(tag, Vec2::new(panel_x + panel_w - 12.0 - tw, cy), 11.0, GREENV);
            }
            cy += 20.0;
        }

        // (3) which handler consumed each fired signal this frame (DispatchReport).
        let mut cy = panel(renderer, panel_x, p3_y, panel_w, ph, "CONSUMED THIS FRAME");
        if self.bus.consumed.is_empty() {
            renderer.draw_text("\u{2014} no signals this frame \u{2014}", Vec2::new(panel_x + 14.0, cy), 12.0, DIM);
        } else {
            let max_rows = (((ph - 34.0) / 20.0).max(1.0)) as usize;
            for row in self.bus.consumed.iter().take(max_rows) {
                let kindc = match row.kind {
                    EventKind::Press => "press",
                    EventKind::Release => "rel",
                    EventKind::Hold => "hold",
                    EventKind::Chord => "chord",
                };
                renderer.draw_text(&format!("{kindc:<5} {}", row.label), Vec2::new(panel_x + 14.0, cy), 12.0, TXT);
                let (dest, col) = match row.consumer {
                    Some(name) => (format!("\u{2192} {name}"), GREENV),
                    None => ("\u{2192} passed".to_string(), AMBER),
                };
                let tw = renderer.measure_text(&dest, 12.0).x;
                renderer.draw_text(&dest, Vec2::new(panel_x + panel_w - 12.0 - tw, cy), 12.0, col);
                cy += 20.0;
            }
        }

        // ── keyboard pills (bottom strip, below both columns) ──
        let ky = size.y - 24.0;
        renderer.draw_text("KEYS", Vec2::new(margin, ky), 11.0, DIM);
        let mut x = 84.0;
        for k in KEYS {
            let on = snap.is_some_and(|s| s.key_down(k));
            let lbl = format!("{k}");
            let w = renderer.measure_text(&lbl, 11.0).x + 12.0;
            let (fill, border, txt) = if on { (ON_FILL, ON_BORDER, ON_CELL_TXT) } else { (IDLE_FILL, IDLE_BORDER, DIM) };
            renderer.draw_ui_panel(Vec2::new(x, ky - 3.0), Vec2::new(w, 18.0), fill, fill, 0.0, 5.0, 1.0, border, 0.0);
            renderer.draw_text(&lbl, Vec2::new(x + 6.0, ky), 11.0, txt);
            x += w + 5.0;
            if x > size.x - 56.0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(active: InputContext, events: &[InputEvent]) -> DispatchReport {
        let mut system = SystemLayer;
        let mut root = SceneRoot;
        let mut modal = ModalLayer { active };
        let mut gameplay = GameplayBase;
        let mut route = RouteCtx::new();
        let mut chain: [&mut dyn InputHandler; 4] =
            [&mut system, &mut root, &mut modal, &mut gameplay];
        Router::dispatch(events, &mut chain, &mut route)
    }

    #[test]
    fn world_routes_signals_to_expected_layers() {
        let raw = InputState::new();
        let active = InputContext::World;
        let mk = |sig, kind| InputEvent::new(sig, kind, active, &raw);
        let events = [
            mk(ActionSignal::Quit, EventKind::Press),
            mk(ActionSignal::Menu, EventKind::Press),
            mk(ActionSignal::Dodge, EventKind::Press),
        ];
        let report = run(active, &events);
        assert!(report.consumed_by(0, ActionSignal::Quit), "System captures Quit");
        assert!(report.consumed_by(SCENE_ROOT, ActionSignal::Menu), "Scene root consumes Menu");
        assert!(report.consumed_by(3, ActionSignal::Dodge), "Gameplay base consumes Dodge");
    }

    #[test]
    fn menu_context_routes_ui_to_modal() {
        let raw = InputState::new();
        let active = InputContext::Menu;
        let events = [InputEvent::new(ActionSignal::Confirm, EventKind::Press, active, &raw)];
        let report = run(active, &events);
        assert!(report.consumed_by(2, ActionSignal::Confirm), "Modal consumes UI signals when active");
    }

    #[test]
    fn textentry_owner_captures_everything() {
        let raw = InputState::new();
        let active = InputContext::TextEntry;
        let events = [InputEvent::new(ActionSignal::SubmitText, EventKind::Press, active, &raw)];
        let report = run(active, &events);
        assert!(report.consumed_by(2, ActionSignal::SubmitText), "exclusive owner captures in TextEntry");
    }

    #[test]
    fn owner_follows_active_context() {
        let report = DispatchReport::default();
        assert!(build_bus_frame(InputContext::World, &[], &report).layers[SCENE_ROOT].owns);
        assert!(build_bus_frame(InputContext::Menu, &[], &report).layers[2].owns);
        assert!(build_bus_frame(InputContext::TextEntry, &[], &report).layers[2].owns);
    }

    #[test]
    fn demo_maps_resolve_context_sensitively() {
        // The same physical button resolves to a different signal per context.
        let cb = demo_bindings();
        assert_eq!(
            cb.active_map().action_for(InputBinding::GamepadButton(GamepadButton::East)),
            Some(ActionSignal::Dodge),
        );
        let mut cb = demo_bindings();
        cb.push(InputContext::Menu);
        assert_eq!(
            cb.active_map().action_for(InputBinding::GamepadButton(GamepadButton::East)),
            Some(ActionSignal::Cancel),
        );
    }
}
