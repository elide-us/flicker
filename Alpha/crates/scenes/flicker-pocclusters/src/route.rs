//! The scene's input-router handler chain (spec §9): the [`InputHandler`]s that
//! `GameScene::update` dispatches through, replacing the hand-rolled
//! `hud_hit` / `chat_hit` / `active()==World` gate ladder with ONE event bus.
//!
//! Chain order (highest input priority first — [`Router::dispatch`] runs a
//! top-down capture pass then a high→low handle pass):
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    CommandHandler  the exclusive TextEntry keyboard owner (chat hand-off)
//! [2]    WalkerHandler   the walker: hud_hit pointer-consume + the screen's
//!                        DECLARED intents (S9 — `on_menu = "pause_open"` on the
//!                        hud_pocclusters.lua root; the scene maps the fired
//!                        name onto its pause push)   [in flicker-widgets]
//! [3]    GameplayBase    world-pick + camera/move, run only on Pass
//! ```
//!
//! [`Router::dispatch`]: flicker_input_router::Router::dispatch

use flicker_input_core::{ActionSignal, EventKind, InputContext, InputState, Key};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context — nothing
/// more. Its old hardcoded `Menu`-press arm died with S9: the pause-open binding
/// now lives in DATA on the screen root (`on_menu = "pause_open"`), consumed by
/// the walker layer below. The TextEntry guard is unchanged and structural: while
/// chat owns the keyboard, [`CommandHandler`]'s `capture` swallows every routed
/// signal above the walker, so the declared intent never fires — and in practice
/// the TextEntry map is empty, so no `Menu` event even resolves.
pub struct RootHandler;

impl InputHandler for RootHandler {
    fn declares_context(&self) -> Option<InputContext> {
        Some(InputContext::World)
    }

    fn handle(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }
}

/// **[1]** The exclusive **TextEntry** keyboard owner (the in-world chat line).
///
/// Homes the text hand-off the scene used to hand-roll — `chat_focus`,
/// `chat_key_guard`, and the `t_prev`/`esc_prev`/`enter_prev` edge bools — while
/// preserving the `4B15929B` contract exactly:
///
///  * **T hands off** from gameplay: a ONE-WAY enter, never a toggle;
///  * the **trigger key** (and its auto-repeats) is guarded out of the buffer for a
///    frame — the extra frame is load-bearing, since macOS commits key text a frame
///    later than the raw key-down;
///  * **Esc cancels** (the draft is kept), **Enter submits**; both are one-way exits.
///
/// The trigger (T), Enter, and Esc are not action-map bindings, and while TextEntry
/// is active the map is empty so the resolver emits no events at all — therefore
/// this state machine is driven once per frame by [`drive`](Self::drive) from the
/// raw snapshot, not from inside the per-event dispatch. The handler still sits in
/// the chain as the exclusive-owner layer: while it owns the keyboard its `capture`
/// swallows every routed signal so nothing leaks to gameplay.
#[derive(Default)]
pub struct CommandHandler {
    /// While set, the scene must swallow this frame's committed text (so the
    /// trigger key never types into the field). The promoted `chat_key_guard`.
    guard: bool,
    t_prev: bool,
    esc_prev: bool,
    enter_prev: bool,
    /// Set by [`drive`](Self::drive); read by `capture` so the exclusive owner
    /// swallows this frame's events even on the very frame it claims the keyboard.
    owns_keyboard: bool,
}

/// What one [`CommandHandler::drive`] resolved this frame.
#[derive(Default)]
pub struct TextOutcome {
    /// Enter — post the line and leave text entry.
    pub submit: bool,
    /// Esc — leave text entry, keeping the draft.
    pub cancel: bool,
}

impl CommandHandler {
    /// Is the trigger-key guard armed this frame? The scene gates the chat pass's
    /// `typed()` / `backspace()` on `!guard` so the hand-off keystroke never types.
    pub fn guard(&self) -> bool {
        self.guard
    }

    /// Run the text-entry state machine for one frame (see the type docs).
    ///
    /// `focused` is the context-derived truth (`active() == TextEntry`);
    /// `click_focus` is a panel click that should enter. The one-way hand-off pushes
    /// its `TextEntry` context + `SetFocus("chat_input")` intents into `rc` (applied
    /// after dispatch); the scene owns the *exit* (pop/clear) so it can fold the
    /// panel's send button into `submit`.
    pub fn drive(
        &mut self,
        input: &InputState,
        focused: bool,
        click_focus: bool,
        rc: &mut RouteCtx,
    ) -> TextOutcome {
        let t_was_down = self.t_prev;
        let t_down = input.key_down(Key::T);
        let t_edge = t_down && !t_was_down;
        self.t_prev = t_down;

        // One-way hand-off from gameplay: T (key) or a panel click enters TextEntry.
        let mut entered = false;
        if !focused && (t_edge || click_focus) {
            rc.set_focus("chat_input");
            rc.push_context(InputContext::TextEntry);
            entered = true;
            // Only the KEY trigger needs the guard — a click never types.
            if t_edge {
                self.guard = true;
            }
        }
        // Clear the guard once T has been UP for a full frame (that extra frame keeps
        // macOS's late-committed 't' out of the buffer — `4B15929B`).
        if self.guard && !t_down && !t_was_down {
            self.guard = false;
        }

        // Esc cancels (draft kept), Enter submits — both only while focused.
        let esc_down = input.key_down(Key::Escape);
        let esc_edge = esc_down && !self.esc_prev;
        self.esc_prev = esc_down;

        let enter_down = input.key_down(Key::Enter);
        let enter_edge = enter_down && !self.enter_prev;
        self.enter_prev = enter_down;

        let (mut submit, mut cancel) = (false, false);
        if focused {
            if esc_edge {
                cancel = true;
            } else if enter_edge {
                submit = true;
            }
        }

        self.owns_keyboard = focused || entered;
        TextOutcome { submit, cancel }
    }
}

impl InputHandler for CommandHandler {
    fn declares_context(&self) -> Option<InputContext> {
        self.owns_keyboard.then_some(InputContext::TextEntry)
    }

    fn capture(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        // Exclusive owner: while it holds the keyboard nothing routes to gameplay
        // (Menu → no pause, a click → no world-pick). In practice the TextEntry map
        // is empty so the resolver emits nothing — this is the belt-and-braces path,
        // and it also covers the frame a hand-off is claimed (`owns_keyboard` set by
        // `drive` before dispatch).
        if self.owns_keyboard {
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }

    fn handle(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }
}

/// **[3]** The gameplay base — last in the chain, so it acts only on a `Pass` from
/// everything above (the typed form of the old `!hud_hit && !chat_hit && World`
/// gate). It records the world-pick request — a `PrimaryAction` press that bubbled
/// all the way down — and the scene runs the ray-pick + polls camera/movement after
/// dispatch, since those are per-frame, not per-event.
#[derive(Default)]
pub struct GameplayBase {
    /// A `PrimaryAction` press reached the base this frame → run the world-pick.
    pub pick: bool,
}

impl InputHandler for GameplayBase {
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if ev.signal == ActionSignal::PrimaryAction && ev.kind == EventKind::Press {
            self.pick = true;
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use flicker_input_router::{Router, RouterRequest};

    use super::*;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_declares_world_and_consumes_nothing() {
        // S9: the root's Menu arm is DEAD — the pause binding is data on the
        // screen root now (the walker layer consumes it). The root only anchors
        // the World base context.
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut root = RootHandler;
        assert_eq!(root.declares_context(), Some(InputContext::World));
        assert_eq!(
            root.handle(&ev(ActionSignal::Menu, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            root.handle(&ev(ActionSignal::Jump, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
    }

    #[test]
    fn gameplay_base_records_the_world_pick() {
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut g = GameplayBase::default();
        assert_eq!(
            g.handle(
                &ev(ActionSignal::SecondaryAction, EventKind::Press, &raw),
                &mut rc
            ),
            Flow::Pass
        );
        assert!(!g.pick);
        assert_eq!(
            g.handle(
                &ev(ActionSignal::PrimaryAction, EventKind::Press, &raw),
                &mut rc
            ),
            Flow::Consumed
        );
        assert!(
            g.pick,
            "a PrimaryAction press that reached the base requests a pick"
        );
    }

    #[test]
    fn t_hands_off_once_and_guards_the_trigger_key() {
        let mut cmd = CommandHandler::default();
        let mut rc = RouteCtx::new();

        // Frame 0: T pressed in World (not focused) → claim focus + push TextEntry + arm
        // the guard so the trigger 't' never types.
        let mut down = InputState::new();
        down.set_key(Key::T, true);
        let out = cmd.drive(&down, false, false, &mut rc);
        assert!(!out.submit && !out.cancel);
        assert!(cmd.guard(), "the trigger key is guarded out of the buffer");
        assert!(rc
            .requests
            .contains(&RouterRequest::PushContext(InputContext::TextEntry)));
        assert!(rc
            .requests
            .contains(&RouterRequest::SetFocus("chat_input".into())));

        // Frame 1: still held, now focused → NO second hand-off (one-way), guard held.
        rc.requests.clear();
        let _ = cmd.drive(&down, true, false, &mut rc);
        assert!(cmd.guard());
        assert!(
            rc.requests.is_empty(),
            "one-way: no re-entry while already focused"
        );

        // Frame 2: T released THIS frame → guard persists one more frame (macOS commits
        // key text a frame late).
        let idle = InputState::new();
        let _ = cmd.drive(&idle, true, false, &mut rc);
        assert!(cmd.guard(), "guard persists the frame T is released");

        // Frame 3: T up a full frame → guard clears.
        let _ = cmd.drive(&idle, true, false, &mut rc);
        assert!(!cmd.guard(), "guard clears once T has been up a full frame");
    }

    #[test]
    fn click_enters_without_arming_the_guard() {
        // A panel click enters TextEntry but never types, so it must NOT arm the guard.
        let mut cmd = CommandHandler::default();
        let mut rc = RouteCtx::new();
        let idle = InputState::new();
        let _ = cmd.drive(&idle, false, true, &mut rc);
        assert!(!cmd.guard());
        assert!(rc
            .requests
            .contains(&RouterRequest::PushContext(InputContext::TextEntry)));
        assert!(rc
            .requests
            .contains(&RouterRequest::SetFocus("chat_input".into())));
    }

    #[test]
    fn esc_cancels_and_enter_submits_only_while_focused() {
        let mut rc = RouteCtx::new();

        // Esc while NOT focused → nothing (Esc→Menu is the scene root's job).
        let mut esc = InputState::new();
        esc.set_key(Key::Escape, true);
        assert!(
            !CommandHandler::default()
                .drive(&esc, false, false, &mut rc)
                .cancel
        );

        // Esc while focused → cancel (draft kept).
        let out = CommandHandler::default().drive(&esc, true, false, &mut rc);
        assert!(out.cancel && !out.submit);

        // Enter while focused → submit.
        let mut enter = InputState::new();
        enter.set_key(Key::Enter, true);
        let out = CommandHandler::default().drive(&enter, true, false, &mut rc);
        assert!(out.submit && !out.cancel);
    }

    /// The declared pause intent (S9) rides the REAL 4-layer chain: the screen
    /// root's `on_menu = "pause_open"` fires through the walker layer — unless
    /// the TextEntry owner holds the keyboard, in which case its capture starves
    /// the intent exactly like it starved the old root arm (the `4B15929B`
    /// contract, structural as ever).
    #[test]
    fn dispatch_fires_the_declared_pause_intent_unless_command_owns_the_keyboard() {
        use flicker::script::{UiNode, Value};
        use flicker::ui::{UiIntents, UiState, WalkerHandler};

        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        // The screen declaration, exactly as hud_pocclusters.lua's root carries it.
        let mut tree = UiNode {
            component: "screen".into(),
            ..Default::default()
        };
        tree.props
            .insert("on_menu".into(), Value::Text("pause_open".into()));
        let intents = UiIntents::of(&tree);

        // Not focused: the Menu press reaches the walker layer → the declared name fires.
        {
            let mut root = RootHandler;
            let mut cmd = CommandHandler::default(); // never drove → owns_keyboard == false
            let mut ui = UiState::new();
            let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
            let mut gameplay = GameplayBase::default();
            let mut rc = RouteCtx::new();
            let report = {
                let mut chain: [&mut dyn InputHandler; 4] =
                    [&mut root, &mut cmd, &mut walker, &mut gameplay];
                Router::dispatch(&events, &mut chain, &mut rc)
            };
            assert!(
                report.consumed_by(2, ActionSignal::Menu),
                "the walker layer consumed it"
            );
            assert!(
                !report.consumed_by(ROOT, ActionSignal::Menu),
                "the root has no Menu arm"
            );
            assert_eq!(walker.take_fired(), vec!["pause_open".to_string()]);
        }

        // Command owns the keyboard: its `capture` swallows Menu above the walker,
        // so the intent never fires (the old `!chat_was_focused` guard, structural).
        {
            let mut root = RootHandler;
            let mut cmd = CommandHandler::default();
            let mut rc = RouteCtx::new();
            let mut down = InputState::new();
            down.set_key(Key::T, true);
            let _ = cmd.drive(&down, false, false, &mut rc); // claim → owns_keyboard = true
            rc.requests.clear();
            let mut ui = UiState::new();
            let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
            let mut gameplay = GameplayBase::default();
            let report = {
                let mut chain: [&mut dyn InputHandler; 4] =
                    [&mut root, &mut cmd, &mut walker, &mut gameplay];
                Router::dispatch(&events, &mut chain, &mut rc)
            };
            assert!(
                report.consumed_by(1, ActionSignal::Menu),
                "command captured it first"
            );
            assert!(
                walker.take_fired().is_empty(),
                "the declared intent never fired"
            );
        }
    }
}
