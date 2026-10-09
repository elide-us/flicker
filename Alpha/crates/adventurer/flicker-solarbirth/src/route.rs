//! The Solar-Birth scene's input-router handler chain (spec §9), replacing the
//! hand-rolled `menu_prev` edge with the shared event bus.
//!
//! The scene is a Group-A cinematic (spec §11.1) — the only arbitrated input is the
//! pause toggle, now DECLARED on the HUD screen root (S10):
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler   the HUD readout — carries the screen's DECLARED intents
//!                        (`on_menu = "pause_open"` on the solarbirth.scene.json
//!                        root)   [flicker-widgets]
//! ```
//!
//! The `WalkerHandler` layer is constructed in `update` (it borrows the retained
//! `UiState`), so it is not defined here. Camera look/zoom/throttle and the flight
//! REPLAY are all MAPPED signals now (input-P3): replay is `Interact`, and the
//! continuous channels come from the pump's `signals.axis` / `signals.pointer_delta`.
//! Only the orbit camera's RMB-drag + wheel stay polled off the raw snapshot — the
//! sanctioned analog-pointer channel, which needs no handler.
//!
//! [`InputHandler`]: flicker_input_router::InputHandler

use flicker_input_core::InputContext;
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context — nothing
/// more. Its hardcoded `Menu`-press arm died with S10: the pause-open binding is
/// DATA on the screen root now (`on_menu = "pause_open"` in
/// `solarbirth.scene.json`), consumed by the walker layer below and mapped onto the
/// pause push by the scene.
pub struct RootHandler;

impl InputHandler for RootHandler {
    fn declares_context(&self) -> Option<InputContext> {
        // The scene is a flight-camera vehicle (MCP 3B4DB4C2) with two modes: it STARTS
        // on the rail (`FlightPath`), dropping out to `Flying` on a look gesture. The
        // root declares the entry context; the resolved events carry the live active one.
        Some(InputContext::FlightPath)
    }

    fn handle(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }
}

#[cfg(test)]
mod tests {
    use flicker::script::{UiNode, Value};
    use flicker::ui::{UiIntents, UiState, WalkerHandler};
    use flicker_input_core::{ActionSignal, EventKind, InputState};
    use flicker_input_router::Router;

    use super::*;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::FlightPath, raw)
    }

    #[test]
    fn root_declares_flightpath_and_consumes_nothing() {
        // S10: the root's Menu arm is DEAD — the pause binding is data on the
        // screen root (`on_menu = "pause_open"`), consumed by the walker layer. The
        // scene enters on the rail (`FlightPath` context, MCP 3B4DB4C2).
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut root = RootHandler;
        assert_eq!(root.declares_context(), Some(InputContext::FlightPath));
        assert_eq!(
            root.handle(&ev(ActionSignal::Menu, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            root.handle(&ev(ActionSignal::Jump, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
    }

    /// The declared pause intent (S9/S10) through the scene's real 2-layer chain:
    /// the screen root's `on_menu = "pause_open"` fires at the walker layer and
    /// the scene maps the name onto its pause push — the root has no Menu arm.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        let mut tree = UiNode {
            component: "surface".into(),
            ..Default::default()
        };
        tree.props
            .insert("on_menu".into(), Value::Text("pause_open".into()));
        let intents = UiIntents::of(&tree);

        let mut root = RootHandler;
        let mut ui = UiState::new();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(&events, &mut chain, &mut rc)
        };
        assert!(
            report.consumed_by(1, ActionSignal::Menu),
            "the walker layer consumed it"
        );
        assert!(
            !report.consumed_by(ROOT, ActionSignal::Menu),
            "the root has no Menu arm"
        );
        assert_eq!(walker.take_fired(), vec!["pause_open".to_string()]);
    }
}
