//! The scene's input-router handler chain. Two scene-local layers bracket the
//! shared walker layer, in the order [`Router::dispatch`] runs them (highest
//! priority first):
//!
//! ```text
//! [ROOT] RootHandler   declares the World base context; consumes nothing
//! [1]    WalkerHandler the console — pointer-consume over the rail, the screen's
//!                      DECLARED intents, focus nav, Confirm/Cancel  [flicker-widgets]
//! [2]    ScopeBase     a click that got past the console is a click on the SCOPE
//! ```
//!
//! The walker layer is built in `update` (it borrows the retained `UiState`), so
//! only the two ends live here.
//!
//! [`Router::dispatch`]: flicker_input_router::Router::dispatch

use flicker_input_core::{ActionSignal, EventKind, InputContext};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **\[ROOT\]** Declares the base `World` context and nothing else — every key the
/// console reacts to is DATA on the screen root, consumed by the walker below.
pub struct RootHandler;

impl InputHandler for RootHandler {
    fn declares_context(&self) -> Option<InputContext> {
        Some(InputContext::World)
    }

    fn handle(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }
}

/// **\[2\]** The scope, last in the chain — so it acts only on what the console
/// above let through. A pointer press that reaches here landed on the radar, and
/// the scene picks the aircraft under the cursor after dispatch (the hit test is
/// per-frame geometry, not per-event).
#[derive(Default)]
pub struct ScopeBase {
    /// A primary press reached the scope this frame.
    pub click: bool,
}

impl InputHandler for ScopeBase {
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if ev.signal == ActionSignal::PrimaryAction && ev.kind == EventKind::Press {
            self.click = true;
            Flow::Consumed
        } else {
            Flow::Pass
        }
    }
}

#[cfg(test)]
mod tests {
    use flicker::script::{UiNode, Value};
    use flicker::ui::{UiIntents, UiState, WalkerHandler};
    use flicker_input_core::InputState;
    use flicker_input_router::Router;

    use super::*;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_declares_world_and_consumes_nothing() {
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut root = RootHandler;
        assert_eq!(root.declares_context(), Some(InputContext::World));
        for signal in [ActionSignal::Menu, ActionSignal::PrimaryAction, ActionSignal::Confirm] {
            assert_eq!(root.handle(&ev(signal, EventKind::Press, &raw), &mut rc), Flow::Pass);
        }
    }

    /// The defining routing contract: a click on the command rail is swallowed by
    /// the console, so it can never also be read as a pick on the radar.
    #[test]
    fn console_clicks_never_reach_the_scope() {
        let raw = InputState::new();
        let events = [ev(ActionSignal::PrimaryAction, EventKind::Press, &raw)];

        for (over_console, expect_pick) in [(false, true), (true, false)] {
            let mut root = RootHandler;
            let mut ui = UiState::new();
            let mut walker = WalkerHandler::hud(&mut ui, over_console);
            let mut scope = ScopeBase::default();
            let mut rc = RouteCtx::new();
            let report = {
                let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut scope];
                Router::dispatch(&events, &mut chain, &mut rc)
            };
            assert_eq!(scope.click, expect_pick, "over_console = {over_console}");
            let layer = if expect_pick { 2 } else { 1 };
            assert!(report.consumed_by(layer, ActionSignal::PrimaryAction));
        }
    }

    /// The declared pause intent through the real chain: the root has no Menu arm,
    /// the walker fires the declared name, and the scope never sees it.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        let mut tree = UiNode { component: "screen".into(), ..Default::default() };
        tree.props.insert("on_menu".into(), Value::Text("pause_open".into()));
        let intents = UiIntents::of(&tree);

        let mut root = RootHandler;
        let mut ui = UiState::new();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut scope = ScopeBase::default();
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut scope];
            Router::dispatch(&events, &mut chain, &mut rc)
        };
        assert!(report.consumed_by(1, ActionSignal::Menu), "the walker layer consumed it");
        assert!(!report.consumed_by(ROOT, ActionSignal::Menu), "the root has no Menu arm");
        assert_eq!(walker.take_fired(), vec!["pause_open".to_string()]);
        assert!(!scope.click);
    }
}
