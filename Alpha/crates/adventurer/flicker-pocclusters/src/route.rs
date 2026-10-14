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

use flicker_input_core::{ActionSignal, EventKind, InputContext};
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
/// The session itself is the WALKER's (Aaron 2026-09-03): `EnterText` — a click into
/// the chat field, a pad Confirm on it, or the bound key (T on kbm) — opens it, and
/// `SubmitText` / `CancelText` (the only signals the `TextEntry` map binds) close it;
/// the trigger key's own character is guarded out by the field's fold. What stays
/// here is the CHAIN role: while chat owns the keyboard this layer's `capture`
/// swallows every routed signal so nothing leaks to gameplay, and it declares the
/// `TextEntry` context. Driven once per frame by [`drive`](Self::drive) from the
/// modal's session truth.
#[derive(Default)]
pub struct CommandHandler {
    /// Set by [`drive`](Self::drive); read by `capture` so the exclusive owner
    /// swallows this frame's events while the session is open.
    owns_keyboard: bool,
}

impl CommandHandler {
    /// Mirror the chat line's session for this frame's dispatch (`focused` =
    /// `ChatModal::text_entry`).
    pub fn drive(&mut self, focused: bool) {
        self.owns_keyboard = focused;
    }
}

impl InputHandler for CommandHandler {
    fn declares_context(&self) -> Option<InputContext> {
        self.owns_keyboard.then_some(InputContext::TextEntry)
    }

    fn capture(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        // Exclusive owner: while it holds the keyboard nothing routes to gameplay
        // (Menu → no pause, a click → no world-pick). In practice the TextEntry map
        // binds only the two text exits — which must reach the walker below to close
        // the session — so this is the belt-and-braces path for everything else.
        if self.owns_keyboard
            && !matches!(
                ev.signal,
                ActionSignal::SubmitText | ActionSignal::CancelText
            )
        {
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
    use flicker_input_core::InputState;
    use flicker_input_router::Router;

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
            component: "surface".into(),
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
            cmd.drive(true); // the chat line's session is open → owns_keyboard = true
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
