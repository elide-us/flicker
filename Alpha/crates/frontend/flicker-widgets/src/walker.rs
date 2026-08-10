//! [`WalkerHandler`] — the adapter that makes the component walker a **layer
//! of the input event bus** (spec §4.1 / task A).
//!
//! The walker already computes the one fact the router needs: `hud_hit`
//! ("UI consumed the pointer this frame", `component.rs`'s [`run_ui`](crate::run_ui)
//! result). This adapter wraps that fact as a
//! [`flicker_input_router::InputHandler`]: its [`handle`](InputHandler::handle)
//! returns [`Flow::Consumed`] for pointer-driven signals when the walker owns the
//! pointer, so the gameplay base handler below it never world-picks *through* a
//! panel — exactly the old `!hud_hit && !chat_hit` gate, now structural.
//!
//! Focus is the walker's other job on the bus (spec §4.3): a single
//! [`UiState::focus`](crate::UiState) id is the source of truth for **both** mouse
//! and gamepad, so the router writes focus *through* the walker via
//! [`apply_focus`](WalkerHandler::apply_focus) rather than owning a second store.
//!
//! # Directional nav (spec §8)
//!
//! When the walker layer is given the current UI tree ([`WalkerHandler::with_nav`]),
//! it also **consumes** the directional-nav signals while it owns a focusable tree.
//! **One meaning per signal, and the walker owns all of them:**
//!
//! * `PanelNext`/`PanelPrev` (the LEFT STICK) pick the PANEL — cycling `tab_group`
//!   ([`tab`]) and landing on the group's lowest `nav_ordinal`, which is the panel
//!   node itself. That is the whole of "which pane has the cursor": a scene owns
//!   no pane enum, no rim style and no enter/exit mode.
//! * `NavUp/Down/Left/Right` move inside the focused group by `nav_ordinal`
//!   ([`nav`]), with the slider nudge folded in.
//! * `Confirm` fires the focused node's `action` the SAME way a click does
//!   (`results.set(action, true)`), delivered on the ONE drain,
//!   [`take_fired`](WalkerHandler::take_fired).
//! * `Cancel` requests a context pop / back-out ([`cancelled`](WalkerHandler::cancelled)).
//!
//! `TabNext`/`TabPrev` (the bumpers) are deliberately NOT here: they belong to the
//! page/tab control's tab rail, which steps itself.
//!
//! Every focus write lands in the one `UiState.focus` id, so d-pad and pointer
//! share a single highlight. All are consumed so they never leak to gameplay.
//!
//! # No cycle
//!
//! `flicker-widgets` depends on `flicker-input-router` (and `-core`); the router
//! depends on neither. The edge is one-way — frontend → router → core (spec §2 /
//! Risk RT-16) — so hosting the adapter here keeps the router frontend-free.

use flicker_input_core::{ActionSignal, EventKind};
use flicker_input_router::{nav, tab, Flow, Focusable, FocusChange, InputEvent, InputHandler, NavDir, RouteCtx};
use flicker_script::{UiNode, ValueMap};

use crate::component::{visible, UiState};
use crate::intents::UiIntents;

/// The walker as one layer of the event bus. Borrows the retained
/// [`UiState`] (so it can write focus) and carries this frame's `hud_hit`
/// verdict (the canonical `run_ui` "UI consumed pointer").
pub struct WalkerHandler<'a> {
    ui: &'a mut UiState,
    /// This frame's `hud_hit` — the walker consumed the pointer over some UI
    /// region (a panel, a slider drag, …). OR-folded across a scene's walker
    /// passes (e.g. a HUD pass and a floating-chat pass) before construction.
    consumed_pointer: bool,
    /// The focusable nodes of the current UI tree, flattened for directional nav
    /// (spec §8). Empty = this walker layer is **not** navigable, so nav signals
    /// pass through instead of being consumed. Filled by [`Self::with_nav`].
    focusables: Vec<Focusable>,
    /// `(node id → action name)` for the focusables that carry an `action`, so
    /// `Confirm` can fire the focused node's action. Parallel to `focusables`.
    actions: Vec<(String, String)>,
    /// `(node id, vertical)` for every focusable SLIDER — nav on the slider's
    /// own axis nudges it (the component-level pad contract every slider gets)
    /// instead of moving focus; the cross axis still navigates away. Filled by
    /// [`Self::with_nav`].
    sliders: Vec<(String, bool)>,
    /// The `action` of the node `Confirm` activated this frame, if any. Drained
    /// into `fired` by [`Self::take_fired`] — the ONE channel a scene reads.
    activated: Option<String>,
    /// `Cancel` was consumed this frame (the scene pops its modal / backs out).
    cancelled: bool,
    /// The screen's declarative signal bindings (S9): a mapped signal's Press
    /// records its result name in `fired` and the event is consumed. `None` =
    /// the screen declares nothing on this layer. Filled by [`Self::with_intents`].
    intents: Option<&'a UiIntents>,
    /// Result names fired by declared intents this frame, in firing order.
    /// Drained by the scene via [`take_fired`](Self::take_fired).
    fired: Vec<String>,
}

impl<'a> WalkerHandler<'a> {
    /// Build the handler from the retained walker `ui` and this frame's
    /// `consumed_pointer` (the canonical `run_ui` `hud_hit`, `component.rs`; the
    /// scene OR-folds its walker passes into this bool).
    ///
    /// The bool — not a re-walk of the tree — is the input, because `run_ui`
    /// already ran the authoritative layout + hit-test this frame; re-testing the
    /// tree here would duplicate it and could disagree.
    ///
    /// Not navigable by default — call [`with_nav`](Self::with_nav) to enable
    /// directional nav for a screen whose nodes carry `tab_group`/`nav_ordinal`.
    pub fn hud(ui: &'a mut UiState, consumed_pointer: bool) -> Self {
        Self {
            ui,
            consumed_pointer,
            focusables: Vec::new(),
            actions: Vec::new(),
            sliders: Vec::new(),
            activated: None,
            cancelled: false,
            intents: None,
            fired: Vec::new(),
        }
    }

    /// Make this walker layer **navigable**: flatten `tree`'s focusable nodes
    /// (those carrying a non-empty `tab_group`) so it consumes and routes the
    /// directional-nav signals (spec §8). Pass the SAME tree AND model this
    /// frame's `run_ui` walked, so nav focus and pointer focus address the same
    /// nodes — and so a hidden overlay contributes nothing (see
    /// [`focusables_of`]).
    pub fn with_nav(mut self, tree: &UiNode, model: &ValueMap) -> Self {
        collect_nav(tree, model, &mut self.focusables, &mut self.actions, &mut self.sliders);
        self
    }

    /// Bind the screen's declarative intents (S9): a Press of a signal `intents`
    /// maps records its declared result name (drained by
    /// [`take_fired`](Self::take_fired)) and BOTH edges of the signal are
    /// consumed, so a declared signal never leaks below this layer.
    ///
    /// **A declared binding is not a licence to name a WALKER-OWNED signal.** The
    /// declaration channel exists for the signals a screen genuinely owns — Menu,
    /// the page/tab rails, the chord verbs. Confirm, Cancel, `Nav*` and `Panel*`
    /// mean one thing on every screen in Prism (activate / back out / move the
    /// cursor / pick the panel), and this layer answers all of them; a screen that
    /// named one would statically kill it on itself, because the intent branch in
    /// [`handle`](InputHandler::handle) consumes and returns BEFORE [`Self::act`]
    /// (violation F1, 2026-08-09 — every button on a bench went dead exactly this
    /// way). The pointer-consume gate still runs first: a click the HUD owns is
    /// swallowed, never re-fired as an intent.
    pub fn with_intents(mut self, intents: &'a UiIntents) -> Self {
        self.intents = Some(intents);
        self
    }

    /// **The ONE activation drain.** Every result name this walker layer produced
    /// this frame, in firing order: the names of declared intents that fired (S9)
    /// **and** the `action` of the node a pad `Confirm` activated. Both are the
    /// same thing to the scene — a name to fold into its results exactly like a
    /// click (`results.set(name, true)`) and republish once as the transient
    /// `sig_<name>` Model mirror ([`UiIntents::mirror_into`]).
    ///
    /// Confirm rides HERE rather than on a second accessor because a second
    /// accessor is a second thing to remember: nine benches drained the intents
    /// and never read the activation, so a focused button could be reached with
    /// the d-pad and never pressed. One channel, and every scene that already
    /// drains gets pad activation with no new line.
    ///
    /// Also the press-flash clock: every call decays the retained flashes one
    /// step, then lights this dispatch's fired result names — so any BUTTON
    /// whose `action` is that name glows exactly as if it had been clicked
    /// (the flash is the button's activate acknowledgement, one mechanism for
    /// every input route). Riding here — the one call every intent-using scene
    /// already makes exactly once per frame — means no scene adds a tick line,
    /// and a scene without intents (nothing to flash) never pays for one.
    pub fn take_fired(&mut self) -> Vec<String> {
        // A pad Confirm's action joins the drain — `act` already lit its flash, so
        // pushing it here (before the tick's relight loop skips it) keeps ONE list.
        if let Some(action) = self.activated.take() {
            self.fired.push(action);
        }
        self.ui.flash_tick();
        for name in &self.fired {
            self.ui.flash(name);
        }
        // The strip-stepping channel (the same shape as `push_nudge`): the names
        // that fired this frame are handed to the next `run_ui` pass, where a
        // `tabs` / `pill_toggle` naming one as its `next_action`/`prev_action`
        // advances its OWN bind. The control owns its stepping; the scene sees
        // only the resulting index.
        for name in &self.fired {
            self.ui.push_step(name);
        }
        std::mem::take(&mut self.fired)
    }

    /// Whether `Cancel` was consumed this frame — the scene pops its modal / backs
    /// out (the router-owned `PopContext` is also queued during dispatch).
    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Apply the focus decision reconciled by
    /// [`apply_context_requests`](flicker_input_router::apply_context_requests) to
    /// the walker's retained focus — one id for mouse **and** gamepad (spec §4.3).
    /// `None` leaves focus untouched.
    pub fn apply_focus(&mut self, change: Option<FocusChange>) {
        match change {
            Some(FocusChange::Set(id)) => self.ui.request_focus(id),
            Some(FocusChange::Clear) => self.ui.clear_focus(),
            None => {}
        }
    }

    /// Route one directional-nav signal (on its press edge) into a focus write /
    /// activation / cancel. `current` is the retained focus id.
    fn act(&mut self, signal: ActionSignal, rc: &mut RouteCtx) {
        // Every routed nav-family signal restores nav modality: the focused node
        // lights again after the pointer had taken the highlight over.
        self.ui.note_nav_input();
        let current = self.ui.focused().map(str::to_string);
        match signal {
            ActionSignal::NavUp => self.nudge_or_move(current.as_deref(), NavDir::Up),
            ActionSignal::NavDown => self.nudge_or_move(current.as_deref(), NavDir::Down),
            ActionSignal::NavLeft => self.nudge_or_move(current.as_deref(), NavDir::Left),
            ActionSignal::NavRight => self.nudge_or_move(current.as_deref(), NavDir::Right),
            // The LEFT STICK is the panel tier: it cycles `tab_group` and lands on
            // the group's lowest ordinal — the panel node itself. (The bumpers are
            // NOT here: they belong to the page/tab control's own rail.)
            ActionSignal::PanelNext => self.cycle_group(current.as_deref(), true),
            ActionSignal::PanelPrev => self.cycle_group(current.as_deref(), false),
            ActionSignal::Confirm => {
                // Same path a click uses: hand the focused node's action to the
                // scene, which sets `results.set(action, true)` (component.rs ~:896).
                let action = current
                    .as_deref()
                    .and_then(|id| self.action_for(id))
                    .map(str::to_string);
                if let Some(a) = action.as_deref() {
                    // A pad Confirm is an activation like any click — the focused
                    // button lights the same flash (one acknowledgement, every route).
                    self.ui.flash(a);
                }
                self.activated = action;
            }
            ActionSignal::Cancel => {
                self.cancelled = true;
                rc.pop_context();
            }
            _ => {}
        }
    }

    /// Directional nav with the slider contract folded in: when the FOCUSED
    /// node is a slider and `dir` runs along its own axis, the press NUDGES it
    /// (recorded in [`UiState`], stepped/clamped/written by the next `run_ui`
    /// pass — chord held scales to the coarse step); every other case moves
    /// focus exactly as before. The cross axis still navigates away, so a
    /// slider is never a focus trap.
    fn nudge_or_move(&mut self, current: Option<&str>, dir: NavDir) {
        if let Some(id) = current {
            if let Some((_, vertical)) = self.sliders.iter().find(|(sid, _)| sid == id) {
                let step = match (vertical, dir) {
                    (true, NavDir::Up) => 1,               // up grows — top is max
                    (true, NavDir::Down) => -1,
                    (false, NavDir::Right) => 1,
                    (false, NavDir::Left) => -1,
                    _ => 0, // the cross axis — fall through to focus movement
                };
                if step != 0 {
                    let coarse = self.ui.chord;
                    self.ui.push_nudge(id, step, coarse);
                    return;
                }
            }
        }
        self.move_focus(current, dir);
    }

    fn move_focus(&mut self, current: Option<&str>, dir: NavDir) {
        if let Some(id) = nav(&self.focusables, current, dir) {
            self.ui.request_focus(id);
        }
    }

    fn cycle_group(&mut self, current: Option<&str>, forward: bool) {
        if let Some(id) = tab(&self.focusables, current, forward) {
            self.ui.request_focus(id);
        }
    }

    fn action_for(&self, id: &str) -> Option<&str> {
        self.actions
            .iter()
            .find(|(fid, _)| fid == id)
            .map(|(_, a)| a.as_str())
    }
}

/// Flatten a UI `tree` into the router's [`Focusable`] list (spec §8): every node
/// with a non-empty `tab_group` **and** a non-empty `id` becomes one focusable,
/// carrying its Lua-authored `nav_ordinal` + `tab_group`. `rect` is a placeholder —
/// this slice's nav is ordinal-primary and does not consult it (see [`nav`]).
///
/// **A HIDDEN SUBTREE IS NOT NAVIGABLE.** `model` is consulted for every node's
/// `visible_bind`, and an invisible node prunes itself AND its descendants: a
/// closed modal, a gated overlay or an off-tab page must not put anything in the
/// nav ring, or the pad walks into controls nobody can see. The model must be the
/// SAME one this frame's `run_ui` walked, so nav and draw agree on what is on
/// screen.
pub fn focusables_of(tree: &UiNode, model: &ValueMap) -> Vec<Focusable> {
    let mut focusables = Vec::new();
    let mut actions = Vec::new();
    let mut sliders = Vec::new();
    collect_nav(tree, model, &mut focusables, &mut actions, &mut sliders);
    focusables
}

/// Recursively collect the focusables (+ their `action`s, + which of them are
/// SLIDERS and their orientation) of a tree — walking `children` and template
/// `slots` so a slot-authored button is navigable too. Invisible nodes prune
/// their whole subtree (see [`focusables_of`]).
fn collect_nav(
    node: &UiNode,
    model: &ValueMap,
    focusables: &mut Vec<Focusable>,
    actions: &mut Vec<(String, String)>,
    sliders: &mut Vec<(String, bool)>,
) {
    if !visible(node, model) {
        return;
    }
    if !node.tab_group.is_empty() && !node.id.is_empty() {
        focusables.push(Focusable {
            id: node.id.clone(),
            group: node.tab_group.clone(),
            ordinal: node.nav_ordinal,
            rect: [0.0; 4],
        });
        if let Some(action) = &node.action {
            actions.push((node.id.clone(), action.clone()));
        }
        if node.component == "slider" {
            sliders.push((node.id.clone(), crate::config::flag(&node.props, "vertical")));
        }
    }
    for child in &node.children {
        collect_nav(child, model, focusables, actions, sliders);
    }
    for group in node.slots.values() {
        for child in group {
            collect_nav(child, model, focusables, actions, sliders);
        }
    }
}

/// The signals THIS LAYER owns while it holds a focusable tree — the ones whose
/// meaning is the same on every screen in Prism, so no screen may name them
/// (see [`WalkerHandler::with_intents`]). The bumpers (`TabNext`/`TabPrev`) are
/// deliberately absent: they belong to the page/tab control's own rail.
pub fn walker_owned(signal: ActionSignal) -> bool {
    is_nav_signal(signal) || signal == ActionSignal::ChordBegin
}

/// The directional-nav signals the walker consumes while it owns a focusable tree.
fn is_nav_signal(signal: ActionSignal) -> bool {
    matches!(
        signal,
        ActionSignal::NavUp
            | ActionSignal::NavDown
            | ActionSignal::NavLeft
            | ActionSignal::NavRight
            | ActionSignal::PanelNext
            | ActionSignal::PanelPrev
            | ActionSignal::Confirm
            | ActionSignal::Cancel
    )
}

/// Signals driven by the pointer — the ones a `hud_hit` should swallow so the
/// scene behind a panel does not also act on the click.
fn is_pointer_signal(signal: ActionSignal) -> bool {
    matches!(
        signal,
        ActionSignal::PrimaryAction | ActionSignal::SecondaryAction
    )
}

impl InputHandler for WalkerHandler<'_> {
    fn handle(&mut self, ev: &InputEvent, rc: &mut RouteCtx) -> Flow {
        // The chord modifier is OBSERVED, never consumed: the walker tracks
        // held-ness (it scales a slider nudge to the coarse step) and passes
        // the event on, so the chord layer below keeps every verb it owns.
        if ev.signal == ActionSignal::ChordBegin {
            match ev.kind {
                EventKind::Press => self.ui.chord = true,
                EventKind::Release => self.ui.chord = false,
                _ => {}
            }
            return Flow::Pass;
        }
        // The typed form of the old `hud_hit` / `chat_hit` gate: when the pointer
        // is over UI, the walker consumes the click so the gameplay base handler
        // (last in the chain) never world-picks through the panel.
        if self.consumed_pointer && is_pointer_signal(ev.signal) {
            return Flow::Consumed;
        }
        // Declarative intents (S9): a signal the screen ROOT bound (`on_<signal>`)
        // fires its declared result name on the Press and is consumed on BOTH
        // edges (like nav — neither edge leaks below). Declared bindings beat the
        // nav defaults below: the screen owns what its bound signal means. The
        // fire is also the press-feedback cue: `take_fired` lights the result
        // name's flash, and any button whose `action` is that name glows — even
        // when the action itself wrapped to nowhere (Aaron, 2026-08-08: the
        // visible response is the point).
        if let Some(name) = self.intents.and_then(|i| i.result_for(ev.signal)) {
            if ev.kind == EventKind::Press {
                self.fired.push(name.to_string());
            }
            return Flow::Consumed;
        }
        // Directional nav — only while this walker layer is navigable (owns a
        // focusable tree). When it is not, these signals pass through so the
        // gameplay base handler still sees them.
        if is_nav_signal(ev.signal) && !self.focusables.is_empty() {
            // The resolver emits BOTH a press and a release for every edge; act on
            // the press so one d-pad tap is one step, but consume both so neither
            // edge leaks to gameplay (spec §8).
            if ev.kind == EventKind::Press {
                self.act(ev.signal, rc);
            }
            return Flow::Consumed;
        }
        Flow::Pass
    }
}

#[cfg(test)]
mod tests {
    use flicker_input_core::{EventKind, InputContext, InputState};
    use flicker_input_router::RouteCtx;

    use super::*;

    /// An empty model: nothing is gated, so every node is visible. Nav filters on
    /// visibility now, so a test tree must be collected against some model.
    fn shown() -> ValueMap {
        ValueMap::new()
    }

    fn press<'a>(signal: ActionSignal, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, EventKind::Press, InputContext::World, raw)
    }

    fn release<'a>(signal: ActionSignal, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, EventKind::Release, InputContext::World, raw)
    }

    fn button(id: &str, group: &str, ordinal: u32, action: &str) -> UiNode {
        UiNode {
            id: id.into(),
            component: "button".into(),
            action: Some(action.into()),
            tab_group: group.into(),
            nav_ordinal: ordinal,
            ..Default::default()
        }
    }

    /// A menu tree: a column of buttons, some in group "menu", some in "scenes".
    fn menu_tree() -> UiNode {
        UiNode {
            id: "root".into(),
            component: "column".into(),
            children: vec![
                button("start", "menu", 0, "start"),
                button("settings", "menu", 1, "settings"),
                button("quit", "menu", 2, "quit"),
                button("load_a", "scenes", 0, "scene_a"),
                button("load_b", "scenes", 1, "scene_b"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn consumes_pointer_only_when_hud_hit() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();

        // hud_hit → the click is consumed; a non-pointer signal still passes.
        let mut h = WalkerHandler::hud(&mut ui, true);
        assert_eq!(h.handle(&press(ActionSignal::PrimaryAction, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.handle(&press(ActionSignal::Jump, &raw), &mut rc), Flow::Pass);
    }

    #[test]
    fn passes_pointer_when_no_hud_hit() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false);
        assert_eq!(h.handle(&press(ActionSignal::PrimaryAction, &raw), &mut rc), Flow::Pass);
    }

    /// **A fired intent lights its RESULT name's flash; the fade runs on the
    /// take_fired clock.** The press fires the result and `take_fired` lights
    /// that name at full intensity — so a BUTTON whose `action` is the same
    /// name glows exactly as if it had been clicked; the release edge is merely
    /// consumed. Each subsequent `take_fired` decays the flash until it
    /// expires. This is the whole press-feedback contract, keyed by the ONE
    /// name every activation route shares.
    #[test]
    fn a_fired_intent_lights_its_results_flash_and_fades() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut intents_tree =
            UiNode { component: "screen".into(), id: "root".into(), ..Default::default() };
        intents_tree
            .props
            .insert("on_page_next".into(), flicker_script::Value::Text("page_flip".into()));
        let intents = UiIntents::of(&intents_tree);

        let mut h = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        assert_eq!(h.handle(&press(ActionSignal::PageNext, &raw), &mut rc), Flow::Consumed);
        let fired = h.take_fired();
        assert_eq!(fired, vec!["page_flip".to_string()], "the press fires the result");
        assert_eq!(
            h.ui.flash_intensity("page_flip"),
            1.0,
            "the fire lights the RESULT name's flash — the button's action key"
        );

        assert_eq!(
            h.handle(&release(ActionSignal::PageNext, &raw), &mut rc),
            Flow::Consumed,
            "the release edge is consumed so it never leaks below"
        );

        let mut last = 1.0;
        let mut frames = 0;
        while h.ui.flash_intensity("page_flip") > 0.0 {
            h.take_fired();
            let now = h.ui.flash_intensity("page_flip");
            assert!(now < last, "the flash fades monotonically");
            last = now;
            frames += 1;
            assert!(frames < 120, "the flash must expire, not linger forever");
        }
        assert!(frames >= 5, "\"briefly\" is a fade, not a single-frame blink");
        // An undeclared signal fires nothing and lights nothing.
        assert_eq!(h.handle(&press(ActionSignal::TabNext, &raw), &mut rc), Flow::Pass);
        assert!(h.take_fired().is_empty());
        assert_eq!(h.ui.flash_intensity("tab_next"), 0.0);
    }

    /// **Every slider owns the pad channel** — the component-level contract,
    /// asserted generically: nav along the FOCUSED slider's own axis nudges
    /// its bound value by the NODE's `step` (`step_coarse` while the chord
    /// modifier is held — observed, never consumed), the write clamps to the
    /// node's range, and the cross axis still moves focus so a slider is
    /// never a trap. No scene declares anything for any of this.
    #[test]
    fn a_focused_slider_steps_on_its_axis_and_chord_scales_the_step() {
        use flicker_render::Vec2;
        use flicker_script::Value;

        let raw = InputState::new();
        let mut vdial =
            UiNode { component: "slider".into(), id: "vdial".into(), ..Default::default() };
        vdial.bind = Some("v".into());
        vdial.tab_group = "g".into();
        vdial.nav_ordinal = 0;
        vdial.size = Some(120.0);
        vdial.props.insert("vertical".into(), Value::Bool(true));
        vdial.props.insert("min".into(), Value::Number(0.0));
        vdial.props.insert("max".into(), Value::Number(100.0));
        let mut hdial =
            UiNode { component: "slider".into(), id: "hdial".into(), ..Default::default() };
        hdial.bind = Some("h".into());
        hdial.tab_group = "g".into();
        hdial.nav_ordinal = 1;
        hdial.size = Some(24.0);
        hdial.props.insert("min".into(), Value::Number(0.0));
        hdial.props.insert("max".into(), Value::Number(10.0));
        let mut col = UiNode { component: "cell".into(), ..Default::default() };
        col.anchor = Some(flicker_script::UiAnchor::TopLeft);
        col.width = Some(200.0);
        col.children = vec![vdial, hdial];
        let mut tree =
            UiNode { component: "screen".into(), id: "root".into(), ..Default::default() };
        tree.children.push(col);

        let mut model = ValueMap::new();
        model.set("v", 50.0);
        model.set("h", 5.0);
        let styles = serde_json::json!({});
        let idle = crate::UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            screen: Vec2::new(800.0, 600.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let mut ui = UiState::new();
        ui.request_focus("vdial");
        let mut rc = RouteCtx::new();

        // Vertical: Up steps toward max by the default step of 1.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(h.handle(&press(ActionSignal::NavUp, &raw), &mut rc), Flow::Consumed);
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(frame.results.number("v"), Some(51.0), "NavUp steps the vertical dial");

        // Chord held scales to the coarse step — and the modifier itself is
        // OBSERVED (Flow::Pass), so the chord layer below still sees it.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(h.handle(&press(ActionSignal::ChordBegin, &raw), &mut rc), Flow::Pass);
            assert_eq!(h.handle(&press(ActionSignal::NavDown, &raw), &mut rc), Flow::Consumed);
            assert_eq!(h.handle(&release(ActionSignal::ChordBegin, &raw), &mut rc), Flow::Pass);
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(
            frame.results.number("v"),
            Some(40.0),
            "chord + NavDown steps by the coarse default (step × 10)"
        );

        // The cross axis is never captured: Left moves focus off the dial.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            assert_eq!(h.handle(&press(ActionSignal::NavLeft, &raw), &mut rc), Flow::Consumed);
        }
        assert_eq!(ui.focused(), Some("hdial"), "the cross axis navigates away");

        // Horizontal: Right steps toward max; the write clamps to the range.
        {
            let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &model);
            for _ in 0..8 {
                assert_eq!(
                    h.handle(&press(ActionSignal::NavRight, &raw), &mut rc),
                    Flow::Consumed
                );
            }
        }
        let frame = crate::run_ui(&tree, &model, &styles, &idle, &mut ui);
        assert_eq!(
            frame.results.number("h"),
            Some(10.0),
            "eight steps from 5 clamp at the horizontal dial's max"
        );
    }

    #[test]
    fn apply_focus_writes_through_the_walker() {
        let mut ui = UiState::new();
        {
            let mut h = WalkerHandler::hud(&mut ui, false);
            h.apply_focus(Some(FocusChange::Set("chat_input".into())));
        }
        assert_eq!(ui.focused(), Some("chat_input"));
        {
            let mut h = WalkerHandler::hud(&mut ui, false);
            h.apply_focus(Some(FocusChange::Clear));
        }
        assert_eq!(ui.focused(), None);
    }

    /// A HIDDEN SUBTREE IS NOT NAVIGABLE. Before this, focusables were collected
    /// with no model at all, so a closed modal's buttons sat in the nav ring and
    /// the pad could walk into controls nobody could see — the reason a bench
    /// could not put `tab_group` on an overlay at all.
    #[test]
    fn a_hidden_subtree_contributes_no_focusables() {
        // A visible screen, plus an overlay gated by `dialog_open`.
        let mut screen = UiNode { component: "screen".into(), ..Default::default() };
        let mut bench = UiNode { component: "button".into(), ..Default::default() };
        bench.id = "bench_btn".into();
        bench.tab_group = "bench".into();

        let mut overlay = UiNode { component: "cell".into(), ..Default::default() };
        overlay.visible_bind = Some("dialog_open".into());
        let mut confirm = UiNode { component: "button".into(), ..Default::default() };
        confirm.id = "dialog_confirm".into();
        confirm.tab_group = "dialog".into();
        overlay.children = vec![confirm];
        screen.children = vec![bench, overlay];

        // Closed: the overlay's button is NOT navigable.
        let closed = focusables_of(&screen, &shown());
        let ids: Vec<_> = closed.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["bench_btn"], "a closed overlay is out of the ring: {ids:?}");

        // Open: it joins the ring, so a modal CAN carry focusable controls.
        let mut open = ValueMap::new();
        open.set("dialog_open", true);
        let shown_ids: Vec<_> =
            focusables_of(&screen, &open).iter().map(|f| f.id.clone()).collect();
        assert_eq!(shown_ids, vec!["bench_btn", "dialog_confirm"], "{shown_ids:?}");
    }

    #[test]
    fn focusables_of_flattens_only_tab_group_nodes() {
        let items = focusables_of(&menu_tree(), &shown());
        // 5 buttons carry a tab_group; the root column (no tab_group) is skipped.
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|f| f.group == "menu" || f.group == "scenes"));
        assert!(items.iter().any(|f| f.id == "start" && f.ordinal == 0));
    }

    #[test]
    fn nav_signal_restores_nav_modality() {
        // The pointer had taken the modality over; the first routed nav signal
        // hands it back, so the focused node lights again.
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.pointer_mode = true;
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert!(h.ui.nav_mode(), "a routed nav press restores nav modality");
    }

    #[test]
    fn nav_moves_focus_within_group_and_is_consumed() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        // No current focus: NavDown enters the list at the lowest ordinal.
        assert_eq!(h.handle(&press(ActionSignal::NavDown, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.ui.focused(), Some("start"));
        // NavDown again steps by ordinal within the group.
        h.handle(&press(ActionSignal::NavDown, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("settings"));
        // The matching RELEASE is consumed too, but does NOT move focus again.
        assert_eq!(h.handle(&release(ActionSignal::NavDown, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.ui.focused(), Some("settings"));
        // NavUp steps back.
        h.handle(&press(ActionSignal::NavUp, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("start"));
    }

    /// **The LEFT STICK cycles PANELS, and it wraps.** Three groups, and
    /// `PanelNext` four times returns to where it started — landing each time on
    /// the group's LOWEST `nav_ordinal`, which is the panel node itself. This is
    /// the whole of "which pane has the cursor": no scene enum, no rim style, no
    /// enter/exit mode. (Named replacement for the Populous bench's
    /// `pane_focus_walks_wraps_enters_and_rims`, which asserted the same
    /// behaviour about a copy of it living inside the scene.)
    #[test]
    fn the_left_stick_cycles_panels_and_wraps() {
        let raw = InputState::new();
        // Three PANELS, each with its own interior control — the panel node sits
        // at ordinal 0 so cycling into a group lands on the pane, not its guts.
        let mut tree = UiNode { id: "root".into(), component: "cell".into(), ..Default::default() };
        for (i, group) in ["pop_left", "pop_view", "pop_right"].iter().enumerate() {
            let mut pane = UiNode {
                id: (*group).into(),
                component: "panel".into(),
                tab_group: (*group).into(),
                nav_ordinal: 0,
                ..Default::default()
            };
            pane.children.push(button(&format!("ctl_{i}"), group, 1, "act"));
            tree.children.push(pane);
        }
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        for want in ["pop_left", "pop_view", "pop_right", "pop_left"] {
            assert_eq!(
                h.handle(&press(ActionSignal::PanelNext, &raw), &mut rc),
                Flow::Consumed,
                "the panel tier is the walker's"
            );
            assert_eq!(h.ui.focused(), Some(want), "lands on the group's lowest ordinal");
        }
        // …and backwards, wrapping the other way.
        h.handle(&press(ActionSignal::PanelPrev, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("pop_right"));
    }

    /// **The bumpers no longer move focus.** `TabNext`/`TabPrev` belong to the
    /// page/tab control's own rail (which steps itself now), so the walker passes
    /// them straight through — one meaning per signal.
    #[test]
    fn the_bumpers_no_longer_move_focus() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.request_focus("settings"); // in group "menu"
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        assert_eq!(h.handle(&press(ActionSignal::TabNext, &raw), &mut rc), Flow::Pass);
        assert_eq!(h.handle(&press(ActionSignal::TabPrev, &raw), &mut rc), Flow::Pass);
        assert_eq!(h.ui.focused(), Some("settings"), "the bumpers moved nothing");
    }

    /// **Confirm on a focused button arrives through `take_fired`** — the ONE
    /// activation drain. A scene that already drains its declared intents gets
    /// pad activation with no extra line, and the name it folds is byte-identical
    /// to the one a click writes.
    #[test]
    fn confirm_on_a_focused_button_arrives_through_take_fired() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.request_focus("quit");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        assert_eq!(h.handle(&press(ActionSignal::Confirm, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.take_fired(), vec!["quit".to_string()], "the drain carries the activation");
        assert!(h.take_fired().is_empty(), "and it drains");

        // The POINTER twin: a click on the same button writes the same key.
        let mut pad = ValueMap::new();
        {
            let mut ui2 = UiState::new();
            ui2.request_focus("quit");
            let mut rc2 = RouteCtx::new();
            let mut h2 = WalkerHandler::hud(&mut ui2, false).with_nav(&tree, &shown());
            h2.handle(&press(ActionSignal::Confirm, &raw), &mut rc2);
            for name in h2.take_fired() {
                pad.set(name, true);
            }
        }
        let mut click = ValueMap::new();
        click.set("quit", true); // what the hit pass writes for a clicked button
        assert!(pad.is_on("quit") && click.is_on("quit"));
        assert_eq!(pad.get("quit"), click.get("quit"), "same key, same value, same channel");
    }

    #[test]
    fn cancel_backs_out_and_queues_pop_context() {
        use flicker_input_router::RouterRequest;
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown());

        assert_eq!(h.handle(&press(ActionSignal::Cancel, &raw), &mut rc), Flow::Consumed);
        assert!(h.cancelled());
        assert!(rc.requests.contains(&RouterRequest::PopContext));
    }

    // ── Declarative intents (S9) ─────────────────────────────────────────────

    /// A tree whose ROOT binds signals declaratively (`on_<signal>` props).
    fn declaring_tree() -> UiNode {
        let mut tree = menu_tree();
        tree.props.insert(
            "on_menu".into(),
            flicker_script::Value::Text("pause_open".into()),
        );
        tree.props.insert(
            "on_cancel".into(),
            flicker_script::Value::Text("settings_close".into()),
        );
        tree
    }

    #[test]
    fn declared_intent_fires_on_press_and_consumes_both_edges() {
        let raw = InputState::new();
        let tree = declaring_tree();
        let intents = UiIntents::of(&tree);
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_intents(&intents);

        // Press records the declared result name and consumes.
        assert_eq!(h.handle(&press(ActionSignal::Menu, &raw), &mut rc), Flow::Consumed);
        // The matching Release is consumed too but fires nothing again.
        assert_eq!(h.handle(&release(ActionSignal::Menu, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.take_fired(), vec!["pause_open".to_string()]);
        assert!(h.take_fired().is_empty(), "take_fired drains");
        // An undeclared signal still passes (the layer is not a black hole).
        assert_eq!(h.handle(&press(ActionSignal::Jump, &raw), &mut rc), Flow::Pass);
    }

    #[test]
    fn declared_binding_beats_the_nav_default() {
        // The tree is navigable AND declares `on_cancel`: the declaration owns
        // Cancel — the intent fires and the built-in back-out (cancelled() +
        // PopContext) does NOT also run. The screen said what Cancel means.
        let raw = InputState::new();
        let tree = declaring_tree();
        let intents = UiIntents::of(&tree);
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree, &shown()).with_intents(&intents);

        assert_eq!(h.handle(&press(ActionSignal::Cancel, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.take_fired(), vec!["settings_close".to_string()]);
        assert!(!h.cancelled(), "declared Cancel does not also run the nav back-out");
        assert!(rc.requests.is_empty(), "…and queues no PopContext");
        // Undeclared nav signals keep their defaults on the same handler.
        assert_eq!(h.handle(&press(ActionSignal::NavDown, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.ui.focused(), Some("start"), "nav still walks the focusables");
    }

    #[test]
    fn empty_intents_change_nothing() {
        let raw = InputState::new();
        let intents = UiIntents::default();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        assert_eq!(h.handle(&press(ActionSignal::Menu, &raw), &mut rc), Flow::Pass);
        assert!(h.take_fired().is_empty());
    }

    #[test]
    fn nav_passes_through_when_not_navigable() {
        let raw = InputState::new();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        // Plain hud() (no with_nav) → nav signals fall through to gameplay.
        let mut h = WalkerHandler::hud(&mut ui, false);
        assert_eq!(h.handle(&press(ActionSignal::NavDown, &raw), &mut rc), Flow::Pass);
        assert_eq!(h.handle(&press(ActionSignal::Confirm, &raw), &mut rc), Flow::Pass);
        assert_eq!(h.ui.focused(), None);
    }
}
