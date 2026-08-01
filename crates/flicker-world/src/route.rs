//! The world viewer's input-router handler chain (spec §9): the single
//! [`InputHandler`] `World::update` dispatches through, replacing the hand-rolled
//! `esc_prev` Menu edge with the shared event bus.
//!
//! The viewer is a data-viewer (spec §11.1 Group B): its only routed arbitration
//! is the `Menu` press (Esc -> pause). The orbit camera's drag/zoom are continuous
//! controls polled off the bus (`signal_held`), and the view-mode / epoch / freq
//! shortcut keys are bespoke *unmapped* keys read raw — so the chain is just the
//! scene-root layer.
//!
//! [`Router::dispatch`]: flicker_input_router::Router::dispatch

use flicker_input_core::{ActionSignal, EventKind, InputContext};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index. `report.consumed_by(ROOT, ActionSignal::Menu)` is
/// the pause-open edge (spec §9).
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context and consumes
/// the `Menu` press so `update` turns it into `Transition::Push(PauseScene)` — the
/// promoted form of the old raw `esc_prev` edge. It consumes in `handle` (not
/// `capture`); with no exclusive keyboard owner above it that is simply where the
/// scene root acts.
pub struct RootHandler;

impl InputHandler for RootHandler {
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

#[cfg(test)]
mod tests {
    use flicker_input_core::InputState;
    use flicker_input_router::Router;

    use super::*;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_consumes_only_the_menu_press() {
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut root = RootHandler;
        assert_eq!(root.declares_context(), Some(InputContext::World));
        assert_eq!(
            root.handle(&ev(ActionSignal::Menu, EventKind::Press, &raw), &mut rc),
            Flow::Consumed
        );
        // Not the release edge, not a different signal (Quit also rides Escape).
        assert_eq!(
            root.handle(&ev(ActionSignal::Menu, EventKind::Release, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            root.handle(&ev(ActionSignal::Quit, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
    }

    #[test]
    fn dispatch_routes_the_menu_press_to_root() {
        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        let mut root = RootHandler;
        let mut rc = RouteCtx::new();
        let mut chain: [&mut dyn InputHandler; 1] = [&mut root];
        let report = Router::dispatch(&events, &mut chain, &mut rc);
        assert!(report.consumed_by(ROOT, ActionSignal::Menu));
        // A movement signal (e.g. R -> MoveUp in the default map) falls through the
        // single-layer chain unconsumed — the scene never acts on it.
        assert!(!report.consumed_by(ROOT, ActionSignal::MoveUp));
    }
}
