//! The scene's input-router handler chain (spec §9): the [`InputHandler`]s that
//! `WorldScene::update` dispatches through, replacing the hand-rolled `Esc` edge
//! (`prev_menu`) with the shared event bus.
//!
//! This is a **data viewer** (Group B): the HUD is a display-only walker tree
//! (readout text + the life-supporting gauge panel) and there is no discrete
//! world-pick, so the chain is two layers. The orbit camera (raw pointer drag +
//! wheel) and the bespoke viewer keys are polled directly after dispatch,
//! exactly as the reference polls its raw mouse-look:
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler   the HUD — carries the screen's DECLARED intents (S10:
//!                        `on_menu = "pause_open"` on the hud_pocepochs.lua
//!                        root)   [flicker-widgets]
//! ```
//!
//! The `WalkerHandler` layer is constructed in `update` (it borrows the retained
//! `UiState`), so it is not defined here.
//!
//! [`Router::dispatch`]: flicker_input_router::Router::dispatch

use flicker_input_core::InputContext;
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context — nothing
/// more. Its hardcoded `Menu`-press arm died with S10: the pause-open binding is
/// DATA on the screen root now (`on_menu = "pause_open"` in `hud_pocepochs.lua`),
/// consumed by the walker layer below and mapped onto the pause push by the scene.
pub struct RootHandler;

impl InputHandler for RootHandler {
    fn declares_context(&self) -> Option<InputContext> {
        Some(InputContext::World)
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
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_declares_world_and_consumes_nothing() {
        // S10: the root's Menu arm is DEAD — the pause binding is data on the
        // screen root (`on_menu = "pause_open"`), consumed by the walker layer.
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

    /// The declared pause intent (S9/S10) through the scene's real 2-layer chain:
    /// the screen root's `on_menu = "pause_open"` fires at the walker layer and
    /// the scene maps the name onto its pause push — the root has no Menu arm.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        let mut tree = UiNode {
            component: "screen".into(),
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
