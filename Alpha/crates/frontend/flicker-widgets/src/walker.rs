//! [`WalkerHandler`] — the adapter that makes the immediate-mode walker a **layer
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
//! it also **consumes** the directional-nav signals while it owns a focusable tree:
//! `NavUp/Down/Left/Right` move focus by `nav_ordinal` inside a `tab_group`
//! ([`nav`]), `TabNext/TabPrev` (the bumpers) cycle groups ([`tab`]), `Confirm`
//! fires the focused node's `action` the SAME way a click does
//! (`results.set(action, true)` — surfaced via [`activated`](WalkerHandler::activated)),
//! and `Cancel` requests a context pop / back-out ([`cancelled`](WalkerHandler::cancelled)).
//! Every focus write lands in the one `UiState.focus` id, so d-pad and pointer
//! share a single highlight. All four are consumed so they never leak to gameplay.
//!
//! # No cycle
//!
//! `flicker-widgets` depends on `flicker-input-router` (and `-core`); the router
//! depends on neither. The edge is one-way — frontend → router → core (spec §2 /
//! Risk RT-16) — so hosting the adapter here keeps the router frontend-free.

use flicker_input_core::{ActionSignal, EventKind};
use flicker_input_router::{nav, tab, Flow, Focusable, FocusChange, InputEvent, InputHandler, NavDir, RouteCtx};
use flicker_script::UiNode;

use crate::component::UiState;
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
    /// The `action` of the node `Confirm` activated this frame, if any (read by
    /// the scene after dispatch, which fires it like a click — `results.set`).
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
            activated: None,
            cancelled: false,
            intents: None,
            fired: Vec::new(),
        }
    }

    /// Make this walker layer **navigable**: flatten `tree`'s focusable nodes
    /// (those carrying a non-empty `tab_group`) so it consumes and routes the
    /// directional-nav signals (spec §8). Pass the SAME tree this frame's `run_ui`
    /// walked, so nav focus and pointer focus address the same nodes.
    pub fn with_nav(mut self, tree: &UiNode) -> Self {
        collect_nav(tree, &mut self.focusables, &mut self.actions);
        self
    }

    /// Bind the screen's declarative intents (S9): a Press of a signal `intents`
    /// maps records its declared result name (drained by
    /// [`take_fired`](Self::take_fired)) and BOTH edges of the signal are
    /// consumed, so a declared signal never leaks below this layer. A declared
    /// binding takes precedence over the walker's own nav defaults (a screen
    /// mapping `on_cancel` owns what Cancel means — the built-in back-out /
    /// `PopContext` does not also run). The pointer-consume gate still runs
    /// first: a click the HUD owns is swallowed, never re-fired as an intent.
    pub fn with_intents(mut self, intents: &'a UiIntents) -> Self {
        self.intents = Some(intents);
        self
    }

    /// Drain the result names fired by declared intents this frame (S9). The
    /// scene folds each into its results exactly like a click
    /// (`results.set(name, true)`) and republishes them once as the transient
    /// `sig_<name>` Model mirror ([`UiIntents::mirror_into`]).
    pub fn take_fired(&mut self) -> Vec<String> {
        std::mem::take(&mut self.fired)
    }

    /// The `action` of the node `Confirm` activated this frame, if any. The scene
    /// fires it the SAME way a click does — `results.set(action, true)` (spec §8).
    pub fn activated(&self) -> Option<&str> {
        self.activated.as_deref()
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
        let current = self.ui.focused().map(str::to_string);
        match signal {
            ActionSignal::NavUp => self.move_focus(current.as_deref(), NavDir::Up),
            ActionSignal::NavDown => self.move_focus(current.as_deref(), NavDir::Down),
            ActionSignal::NavLeft => self.move_focus(current.as_deref(), NavDir::Left),
            ActionSignal::NavRight => self.move_focus(current.as_deref(), NavDir::Right),
            ActionSignal::TabNext => self.cycle_group(current.as_deref(), true),
            ActionSignal::TabPrev => self.cycle_group(current.as_deref(), false),
            ActionSignal::Confirm => {
                // Same path a click uses: hand the focused node's action to the
                // scene, which sets `results.set(action, true)` (component.rs ~:896).
                let action = current
                    .as_deref()
                    .and_then(|id| self.action_for(id))
                    .map(str::to_string);
                self.activated = action;
            }
            ActionSignal::Cancel => {
                self.cancelled = true;
                rc.pop_context();
            }
            _ => {}
        }
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
pub fn focusables_of(tree: &UiNode) -> Vec<Focusable> {
    let mut focusables = Vec::new();
    let mut actions = Vec::new();
    collect_nav(tree, &mut focusables, &mut actions);
    focusables
}

/// Recursively collect the focusables (+ their `action`s) of a tree — walking
/// `children` and template `slots` so a slot-authored button is navigable too.
fn collect_nav(node: &UiNode, focusables: &mut Vec<Focusable>, actions: &mut Vec<(String, String)>) {
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
    }
    for child in &node.children {
        collect_nav(child, focusables, actions);
    }
    for group in node.slots.values() {
        for child in group {
            collect_nav(child, focusables, actions);
        }
    }
}

/// The directional-nav signals the walker consumes while it owns a focusable tree.
fn is_nav_signal(signal: ActionSignal) -> bool {
    matches!(
        signal,
        ActionSignal::NavUp
            | ActionSignal::NavDown
            | ActionSignal::NavLeft
            | ActionSignal::NavRight
            | ActionSignal::TabNext
            | ActionSignal::TabPrev
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
        // The typed form of the old `hud_hit` / `chat_hit` gate: when the pointer
        // is over UI, the walker consumes the click so the gameplay base handler
        // (last in the chain) never world-picks through the panel.
        if self.consumed_pointer && is_pointer_signal(ev.signal) {
            return Flow::Consumed;
        }
        // Declarative intents (S9): a signal the screen ROOT bound (`on_<signal>`)
        // fires its declared result name on the Press and is consumed on BOTH
        // edges (like nav — neither edge leaks below). Declared bindings beat the
        // nav defaults below: the screen owns what its bound signal means.
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

    #[test]
    fn focusables_of_flattens_only_tab_group_nodes() {
        let items = focusables_of(&menu_tree());
        // 5 buttons carry a tab_group; the root column (no tab_group) is skipped.
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(|f| f.group == "menu" || f.group == "scenes"));
        assert!(items.iter().any(|f| f.id == "start" && f.ordinal == 0));
    }

    #[test]
    fn nav_moves_focus_within_group_and_is_consumed() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree);

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

    #[test]
    fn tab_cycles_between_groups() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.request_focus("settings"); // in group "menu"
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree);

        // TabNext (RightBumper) cycles to the next group's lowest ordinal.
        assert_eq!(h.handle(&press(ActionSignal::TabNext, &raw), &mut rc), Flow::Consumed);
        assert_eq!(h.ui.focused(), Some("load_a"));
        // TabPrev cycles back.
        h.handle(&press(ActionSignal::TabPrev, &raw), &mut rc);
        assert_eq!(h.ui.focused(), Some("start"));
    }

    #[test]
    fn confirm_surfaces_the_focused_action() {
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        ui.request_focus("quit");
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree);

        assert_eq!(h.handle(&press(ActionSignal::Confirm, &raw), &mut rc), Flow::Consumed);
        // Same path a click uses: the scene fires this via results.set(action, true).
        assert_eq!(h.activated(), Some("quit"));
    }

    #[test]
    fn cancel_backs_out_and_queues_pop_context() {
        use flicker_input_router::RouterRequest;
        let raw = InputState::new();
        let tree = menu_tree();
        let mut ui = UiState::new();
        let mut rc = RouteCtx::new();
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree);

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
        let mut h = WalkerHandler::hud(&mut ui, false).with_nav(&tree).with_intents(&intents);

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
