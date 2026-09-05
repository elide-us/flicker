//! The scene's input-router handler chain (spec §9): the [`InputHandler`]s that
//! `GameScene::update` dispatches through, replacing the hand-rolled
//! `hud_hit` / `active()==World` gate ladder with ONE event bus.
//!
//! Chain order (highest input priority first — [`Router::dispatch`] runs a
//! top-down capture pass then a high→low handle pass):
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler   the walker: hud_hit pointer-consume + the screen's
//!                        DECLARED intents (S9 — `on_menu = "pause_open"` on the
//!                        screen root; the scene maps the fired name onto its
//!                        pause push)   [in flicker-widgets]
//! [2]    GameplayBase    world-pick + camera/move, run only on Pass
//! ```
//!
//! (The chat TextEntry owner this chain carried in the pocclusters lineage was
//! removed with the chat window itself — chat lives on in `flicker-pocclusters`,
//! whose own `route.rs` keeps that handler.)
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
/// the walker layer below.
pub struct RootHandler;

impl InputHandler for RootHandler {
    fn declares_context(&self) -> Option<InputContext> {
        Some(InputContext::World)
    }

    fn handle(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }
}

/// **[2]** The gameplay base — last in the chain, so it acts only on a `Pass` from
/// everything above (the typed form of the old `!hud_hit && World` gate). It
/// records the world-pick request — a `PrimaryAction` press that bubbled all the
/// way down — and the scene runs the ray-pick + polls camera/movement after
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

    /// The declared pause intent (S9) rides the REAL 3-layer chain: the screen
    /// root's `on_menu = "pause_open"` fires through the walker layer, and the
    /// root itself has no Menu arm.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        use flicker::script::{UiNode, Value};
        use flicker::ui::{UiIntents, UiState, WalkerHandler};

        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        // The screen declaration, exactly as the scene tree's root carries it.
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
        let mut gameplay = GameplayBase::default();
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
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
