//! [`InputEvent`] — a resolved [`Fired`] wrapped with its context + raw snapshot,
//! ready to route — and [`Flow`], a handler's consume-or-pass verdict.

use flicker_input_core::{ActionSignal, EventKind, Fired, InputContext, InputState};

/// One routable input event: the semantic signal, how it fired, the context it
/// resolved under, and a borrow of the raw snapshot (spec §4.2).
///
/// The router does NOT resolve — the caller builds these from the core
/// [`Resolver`](flicker_input_core::Resolver)'s [`Fired`] output plus the active
/// context (spec R3 / §5), typically via [`InputEvent::from_fired`]. `raw` is the
/// pointer / analog-latch source a UI handler hit-tests against or a camera
/// handler reads.
#[derive(Clone, Copy)]
pub struct InputEvent<'a> {
    /// The semantic WHAT.
    pub signal: ActionSignal,
    /// How the control fired (`Press` / `Release` / …).
    pub kind: EventKind,
    /// The context this event resolved under.
    pub context: InputContext,
    /// The raw held-state snapshot (pointer / analog latch) for hit-test / camera.
    pub raw: &'a InputState,
}

impl<'a> InputEvent<'a> {
    /// Assemble an event from its parts.
    pub fn new(
        signal: ActionSignal,
        kind: EventKind,
        context: InputContext,
        raw: &'a InputState,
    ) -> Self {
        Self { signal, kind, context, raw }
    }

    /// Wrap a core [`Fired`] with the active `context` and a borrow of the raw
    /// snapshot — the router's half of the resolve ▸ context ▸ route seam
    /// (spec R3 / §5). Copies `signal` + `kind` from the `Fired`; the physical
    /// `control` stays in the resolver's domain and is not carried onto the bus.
    pub fn from_fired(fired: &Fired, context: InputContext, raw: &'a InputState) -> Self {
        Self { signal: fired.signal, kind: fired.kind, context, raw }
    }
}

/// A handler's verdict on an event. `Consumed` = today's `hud_hit` / `chat_hit`
/// "true" (stop propagation); `Pass` lets the next handler act.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Flow {
    /// The event was handled here; stop propagating it.
    Consumed,
    /// Not handled here; let the next handler act.
    Pass,
}
