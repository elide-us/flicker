//! The [`InputHandler`] trait — one layer of the bus (spec §4.2).

use flicker_input_core::InputContext;

use crate::event::{Flow, InputEvent};
use crate::router::RouteCtx;

/// One layer in the dispatch chain. A scene builds a chain of these (system →
/// scene-root → modal → UI-panel → gameplay-base) and hands it to
/// [`Router::dispatch`](crate::Router::dispatch).
///
/// The two phases mirror the bus (spec §4.2): [`capture`](Self::capture) is the
/// top-down "first refusal" pass (the exclusive keyboard owner claims text +
/// Enter/Esc here), and [`handle`](Self::handle) is the high→low target+bubble
/// pass. Both default-implemented methods let a handler opt into only what it
/// needs — most handlers implement `handle` alone.
pub trait InputHandler {
    /// Top-down first-refusal pass. Return [`Flow::Consumed`] to claim the event
    /// before any lower handler's [`handle`](Self::handle) can run. Defaults to
    /// [`Flow::Pass`] (capture-only layers like system/global override it).
    fn capture(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
        Flow::Pass
    }

    /// Target + bubble pass. Return [`Flow::Consumed`] to stop propagation, or
    /// [`Flow::Pass`] to let the next (lower-priority) handler act.
    fn handle(&mut self, ev: &InputEvent, rc: &mut RouteCtx) -> Flow;

    /// The [`InputContext`] this handler owns while it holds focus / is modal —
    /// read by the caller to derive the active context (spec §5, "active context
    /// = top of focus chain"). Defaults to `None` (the handler declares no
    /// context of its own; the scene-root base applies).
    fn declares_context(&self) -> Option<InputContext> {
        None
    }
}
