//! The [`InputHandler`] trait — one layer of the bus (spec §4.2).

use flicker_input_core::{ActionSignal, InputContext};

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
    /// The signals this handler SUBSCRIBES to — the ones it is willing to consume.
    /// The dispatcher offers a signal to [`capture`](Self::capture) /
    /// [`handle`](Self::handle) ONLY when the handler subscribes to it; an
    /// unsubscribed signal passes straight through this layer to the next, so a
    /// handler can never eat input it does not own (Aaron 2026-08-11: a context
    /// WATCHES the stream and takes only what is relevant — it never consumes ALL
    /// input; MCP `67DEE93A`).
    ///
    /// Defaults to subscribing to EVERYTHING, so a handler that already gates
    /// imperatively inside `handle` (returning [`Flow::Pass`] for signals it does
    /// not want) is unchanged. A handler adopting the subscription discipline —
    /// e.g. a nav-only focus layer, or a scene's orchestration layer — OVERRIDES
    /// this to declare its own set, and the dispatcher then guarantees it is only
    /// ever asked about those signals.
    fn subscribes(&self, _signal: ActionSignal) -> bool {
        true
    }

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
