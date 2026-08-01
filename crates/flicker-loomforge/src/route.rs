//! The scene's input-router handler chain (spec §9): the `InputHandler`s that
//! `LoomforgeBench::update` dispatches through, replacing the hand-rolled
//! `menu_prev` edge with the ONE event bus.
//!
//! Loomforge is a Group-A (trivial) scene: the only signal it routes is `Menu`
//! (Esc -> the shell pause overlay). Its node-graph canvas and TAE timeline are
//! bespoke, scene-drawn tools that pick / drag off the RAW pointer, so that logic
//! stays exactly as it was; only `Menu` and the walker's `hud_hit` pointer-consume
//! ride the bus. The chain is therefore just:
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler  hud_hit pointer-consume + the screen's declared intents
//!                       (S9: the Rust-built root's `on_menu = "pause_open"`)
//!                       (in flicker-widgets)
//! ```

use flicker_input_core::InputContext;
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context — nothing
/// more. Its hardcoded `Menu`-press arm died with S9: the pause-open binding is
/// DATA on the screen root now (`on_menu = "pause_open"`, set where `build_tree`
/// makes the root), consumed by the walker layer below and mapped onto the pause
/// push by the scene. The resolver still owns the press edge, so a held Esc
/// opens the overlay exactly once.
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

    use super::*;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_declares_world_and_consumes_nothing() {
        // S9: the root's Menu arm is DEAD — the pause binding is data on the
        // built root (`on_menu = "pause_open"`), consumed by the walker layer.
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut root = RootHandler;
        assert_eq!(root.declares_context(), Some(InputContext::World));
        assert_eq!(
            root.handle(&ev(ActionSignal::Menu, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
        assert_eq!(
            root.handle(&ev(ActionSignal::PrimaryAction, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
    }

    /// The declared pause intent (S9) through the editor's real 2-layer chain.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        use flicker::script::{UiNode, Value};
        use flicker::ui::{UiIntents, UiState, WalkerHandler};
        use flicker_input_router::Router;

        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        // The declaration exactly as `build_tree` stamps it on the root.
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
        assert_eq!(walker.take_fired(), vec!["pause_open".to_string()]);
    }
}
