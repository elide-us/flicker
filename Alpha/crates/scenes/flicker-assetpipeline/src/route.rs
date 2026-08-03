//! The scene's input-router handler chain (spec section 9 port): the
//! `InputHandler`s that `AssetPipeline::update` dispatches through, replacing the
//! hand-rolled `Esc`-edge menu handling with the shared, already-proven event bus.
//!
//! This is a **Group A / trivial** scene (spec section 11.1): a walker HUD plus a
//! 2x2 quad viewport with per-panel orbit cameras. Only two arbitration facts move
//! onto the bus here:
//!
//!   * the pause edge (`ActionSignal::Menu`) — DECLARED on the screen root now
//!     (S9: `on_menu = "pause_open"`, declared by `build_tree`) and consumed by
//!     the walker layer, which fires the name the scene maps onto its pause
//!     push — and
//!   * the HUD's pointer-consume (`hud_hit`), routed through the
//!     `WalkerHandler` layer built in `update` (from `flicker-widgets`).
//!
//! The viewport's per-panel orbit / pan / zoom stays **bespoke and polled** after
//! dispatch: the `editor_quad` holder is a *styled* `stage` node, so the walker's
//! `hud_hit` is already true over the viewport itself — the controls therefore
//! gate on the `QuadGrid`'s own `cell_at` hit-test, not on the bus (spec section 9
//! "kept polled"). That leaves this chain with just the scene root and the HUD:
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler   the HUD walker: hud_hit pointer-consume + the screen's
//!                        declared intents (S9)   [flicker-widgets]
//! ```

use flicker_input_core::InputContext;
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context — nothing
/// more. Its hardcoded `Menu`-press arm died with S9: the pause-open binding is
/// DATA on the screen root now (`on_menu = "pause_open"`), consumed by the
/// walker layer below and mapped onto the pause push by the scene.
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
    use flicker_input_core::{ActionSignal, EventKind, InputState};
    use flicker_input_router::Router;

    use super::*;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_declares_world_and_consumes_nothing() {
        // S9: the root's Menu arm is DEAD — the pause binding is data on the
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
            root.handle(
                &ev(ActionSignal::PrimaryAction, EventKind::Press, &raw),
                &mut rc
            ),
            Flow::Pass
        );
    }

    /// The declared pause intent (S9) through the scene's real 2-layer chain.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        use flicker::script::{UiNode, Value};
        use flicker::ui::{UiIntents, UiState, WalkerHandler};

        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        let mut tree = UiNode { component: "screen".into(), ..Default::default() };
        tree.props.insert("on_menu".into(), Value::Text("pause_open".into()));
        let intents = UiIntents::of(&tree);

        let mut root = RootHandler;
        let mut ui = UiState::new();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut root, &mut walker];
            Router::dispatch(&events, &mut chain, &mut rc)
        };
        assert!(report.consumed_by(1, ActionSignal::Menu), "the walker layer consumed it");
        assert!(!report.consumed_by(ROOT, ActionSignal::Menu), "the root has no Menu arm");
        assert_eq!(
            walker.take_fired(),
            vec!["pause_open".to_string()],
            "the fired name is the pause-open edge the scene reads after dispatch"
        );
    }
}
