//! The jiggle scene's input-router handler chain (spec §9) — the same shared event
//! bus clicktrainer/pocclusters route through. NOTHING is wired to a raw device
//! button (ALL INPUT EVENTS ARE SIGNALS, 37722F91): the rail-drop is the
//! `PrimaryAction` SIGNAL (press grabs the ball on the rail; release — queried via
//! `SceneInput::held` in the scene — drops it straight down), keyboard/pad aim is
//! `StrafeLeft`/`StrafeRight`, and a `Confirm` press is the no-pointer drop. The aim
//! X is the pointer POSITION sample (allowed by the surface contract), never a button.
//!
//! Chain order (highest priority first — [`Router::dispatch`] runs a top-down capture
//! pass then a high→low handle pass):
//!
//! ```text
//! [ROOT] RootHandler    declares the World base context (no consuming arms)
//! [1]    WalkerHandler   the HUD panel — hud_hit pointer-consume + the screen's
//!                        DECLARED intents (on_menu = "pause_open")   [flicker-widgets]
//! [2]    GameplayBase    grab / aim-nudge / drop signals, run only on Pass
//! ```

use flicker_input_core::{ActionSignal, EventKind, InputContext};
use flicker_input_router::{Flow, InputEvent, InputHandler, RouteCtx};

/// **[ROOT]** The scene-mode root: declares the base `World` context, nothing more.
/// The pause-open binding is DATA on the screen root (`on_menu = "pause_open"`),
/// consumed by the walker layer and mapped onto the pause push by the scene.
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
/// everything above (a press over the HUD panel is swallowed by the walker layer, so
/// it never grabs a ball). It records this frame's gameplay SIGNAL edges; the scene
/// reads them after dispatch and drives the rail from them + the pointer position.
#[derive(Default)]
pub struct GameplayBase {
    /// `PrimaryAction` pressed on the play field → grab the rail ball (start a drag).
    pub grab: bool,
    /// Net `StrafeLeft`/`StrafeRight` this frame → nudge the rail aim (keyboard/pad).
    pub nudge: f32,
    /// `Confirm` pressed → the no-pointer drop (keyboard/pad), gated on not dragging.
    pub confirm: bool,
}

impl InputHandler for GameplayBase {
    fn handle(&mut self, ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        if ev.kind != EventKind::Press {
            return Flow::Pass;
        }
        match ev.signal {
            ActionSignal::PrimaryAction => {
                self.grab = true;
                Flow::Consumed
            }
            ActionSignal::StrafeLeft => {
                self.nudge -= 1.0;
                Flow::Consumed
            }
            ActionSignal::StrafeRight => {
                self.nudge += 1.0;
                Flow::Consumed
            }
            ActionSignal::Confirm => {
                self.confirm = true;
                Flow::Consumed
            }
            _ => Flow::Pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_input_core::InputState;
    use flicker_input_router::Router;

    fn ev<'a>(signal: ActionSignal, kind: EventKind, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, kind, InputContext::World, raw)
    }

    #[test]
    fn root_declares_world_and_consumes_nothing() {
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut root = RootHandler;
        assert_eq!(root.declares_context(), Some(InputContext::World));
        assert_eq!(
            root.handle(
                &ev(ActionSignal::PrimaryAction, EventKind::Press, &raw),
                &mut rc
            ),
            Flow::Pass
        );
    }

    #[test]
    fn gameplay_records_grab_and_aim_signals_on_press_only() {
        let raw = InputState::new();
        let mut rc = RouteCtx::new();
        let mut g = GameplayBase::default();
        // A release edge is not a grab.
        assert_eq!(
            g.handle(
                &ev(ActionSignal::PrimaryAction, EventKind::Release, &raw),
                &mut rc
            ),
            Flow::Pass
        );
        assert!(!g.grab);
        assert_eq!(
            g.handle(
                &ev(ActionSignal::PrimaryAction, EventKind::Press, &raw),
                &mut rc
            ),
            Flow::Consumed
        );
        assert!(g.grab, "a PrimaryAction press grabs the rail ball");
        g.handle(
            &ev(ActionSignal::StrafeRight, EventKind::Press, &raw),
            &mut rc,
        );
        g.handle(
            &ev(ActionSignal::StrafeLeft, EventKind::Press, &raw),
            &mut rc,
        );
        g.handle(
            &ev(ActionSignal::StrafeLeft, EventKind::Press, &raw),
            &mut rc,
        );
        assert!(
            (g.nudge + 1.0).abs() < 1e-6,
            "one right, two left → net −1 aim nudge"
        );
    }

    /// A grab press over the HUD is swallowed by the walker layer, so the gameplay
    /// base never starts a drag there — the shared bus's version of the click gate.
    #[test]
    fn hud_hit_swallows_the_grab_before_the_base() {
        use flicker::ui::{UiState, WalkerHandler};

        let raw = InputState::new();
        let events = [ev(ActionSignal::PrimaryAction, EventKind::Press, &raw)];
        let mut root = RootHandler;
        let mut ui = UiState::new();
        let mut walker = WalkerHandler::hud(&mut ui, true); // over the HUD
        let mut gameplay = GameplayBase::default();
        let mut rc = RouteCtx::new();
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
            Router::dispatch(&events, &mut chain, &mut rc);
        }
        assert!(!gameplay.grab, "a HUD press never grabs a ball");
    }
}
