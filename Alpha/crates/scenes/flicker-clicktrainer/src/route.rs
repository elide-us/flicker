//! The click-trainer's input-router handler chain (spec §9): the tiny set of
//! [`InputHandler`]s that `ClickTrainer::update` dispatches through, replacing the
//! hand-rolled `menu_prev` edge + the `mouse_left_pressed && !over_hud` click gate
//! with the shared event bus (the same one `flicker-pocclusters` is ported onto).
//!
//! Chain order (highest input priority first — [`Router::dispatch`] runs a top-down
//! capture pass then a high→low handle pass):
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler   the HUD panel — hud_hit pointer-consume + the screen's
//!                        DECLARED intents (S9: `on_menu = "pause_open"` on the
//!                        hud_clicktrainer.lua root)   [flicker-widgets]
//! [2]    GameplayBase    the target click, run only on Pass
//! ```
//!
//! The `WalkerHandler` layer is constructed in `update` (it borrows the retained
//! `UiState`), so it is not defined here; this module owns the two scene-local
//! layers.
//!
//! [`Router::dispatch`]: flicker_input_router::Router::dispatch

use flicker_input_core::{ActionSignal, EventKind, InputContext};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// The scene-root layer index (the chain-order tests assert against it).
#[cfg(test)]
pub const ROOT: usize = 0;

/// **[ROOT]** The scene-mode root. Declares the base `World` context — nothing
/// more. Its hardcoded `Menu`-press arm died with S10: the pause-open binding is
/// DATA on the screen root now (`on_menu = "pause_open"` in
/// `hud_clicktrainer.lua`), consumed by the walker layer below and mapped onto
/// the pause push by the scene.
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
/// everything above (the typed form of the old `!over_hud` click gate: while the
/// cursor is over the HUD panel the walker layer above consumes the click, so it
/// never reaches here). It records that a `PrimaryAction` press bubbled all the way
/// down; the scene then scores this frame's cursor as a hit/miss after dispatch,
/// since the hit-test is per-frame, not per-event.
#[derive(Default)]
pub struct GameplayBase {
    /// A `PrimaryAction` press reached the base this frame (it was not swallowed by
    /// the HUD) → the scene scores this frame's cursor position.
    pub click: bool,
}

impl InputHandler for GameplayBase {
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
    use flicker::ui::{UiState, WalkerHandler};
    use flicker_input_core::InputState;
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
            root.handle(&ev(ActionSignal::PrimaryAction, EventKind::Press, &raw), &mut rc),
            Flow::Pass
        );
    }

    #[test]
    fn gameplay_base_records_the_click() {
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut g = GameplayBase::default();
        // A release edge, or a non-pointer signal, is not a game click.
        assert_eq!(
            g.handle(&ev(ActionSignal::PrimaryAction, EventKind::Release, &raw), &mut rc),
            Flow::Pass
        );
        assert!(!g.click);
        assert_eq!(
            g.handle(&ev(ActionSignal::PrimaryAction, EventKind::Press, &raw), &mut rc),
            Flow::Consumed
        );
        assert!(g.click, "a PrimaryAction press that reached the base is a game click");
    }

    /// The scene's defining behaviour: a click over the HUD is swallowed by the
    /// walker layer, so the gameplay base never scores it — the structural form of
    /// the old `mouse_left_pressed && !over_hud` gate.
    #[test]
    fn hud_hit_swallows_the_click_before_the_base() {
        let raw = InputState::new();
        let events = [ev(ActionSignal::PrimaryAction, EventKind::Press, &raw)];

        // Not over the HUD: the click bubbles past the walker to the base → scored.
        {
            let mut root = RootHandler;
            let mut ui = UiState::new();
            let mut walker = WalkerHandler::hud(&mut ui, false);
            let mut gameplay = GameplayBase::default();
            let mut rc = RouteCtx::new();
            let report = {
                let mut chain: [&mut dyn InputHandler; 3] =
                    [&mut root, &mut walker, &mut gameplay];
                Router::dispatch(&events, &mut chain, &mut rc)
            };
            assert!(gameplay.click, "a play-field click reaches the gameplay base");
            assert!(report.consumed_by(2, ActionSignal::PrimaryAction));
        }

        // Over the HUD (hud_hit): the walker consumes the click first → no score.
        {
            let mut root = RootHandler;
            let mut ui = UiState::new();
            let mut walker = WalkerHandler::hud(&mut ui, true);
            let mut gameplay = GameplayBase::default();
            let mut rc = RouteCtx::new();
            let report = {
                let mut chain: [&mut dyn InputHandler; 3] =
                    [&mut root, &mut walker, &mut gameplay];
                Router::dispatch(&events, &mut chain, &mut rc)
            };
            assert!(!gameplay.click, "a HUD click never reaches the gameplay base");
            assert!(report.consumed_by(1, ActionSignal::PrimaryAction), "the walker consumed it");
        }
    }

    /// The declared pause intent (S9/S10) through the real 3-layer chain: the
    /// screen root's `on_menu = "pause_open"` fires at the walker layer and the
    /// scene maps the name onto its pause push — the root has no Menu arm.
    #[test]
    fn dispatch_fires_the_declared_pause_intent() {
        use flicker::script::{UiNode, Value};
        use flicker::ui::UiIntents;

        let raw = InputState::new();
        let events = [ev(ActionSignal::Menu, EventKind::Press, &raw)];
        let mut tree = UiNode { component: "screen".into(), ..Default::default() };
        tree.props.insert("on_menu".into(), Value::Text("pause_open".into()));
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
        assert!(report.consumed_by(1, ActionSignal::Menu), "the walker layer consumed it");
        assert!(!report.consumed_by(ROOT, ActionSignal::Menu), "the root has no Menu arm");
        assert_eq!(walker.take_fired(), vec!["pause_open".to_string()]);
        assert!(!gameplay.click);
    }
}
