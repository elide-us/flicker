//! [`Router::dispatch`] (the two-phase bus), the router-owned request queue
//! ([`RouteCtx`] / [`RouterRequest`]), the post-dispatch reconciliation
//! ([`apply_context_requests`]), and the per-frame [`DispatchReport`].

use flicker_input_core::{ActionSignal, ContextualBindings, InputContext};

use crate::event::{Flow, InputEvent};
use crate::handler::InputHandler;

/// The event bus. A stateless dispatcher: all per-frame state lives in the
/// handlers, the [`RouteCtx`], and the returned [`DispatchReport`].
pub struct Router;

impl Router {
    /// Route a frame's `events` through the `chain`, two phases per event
    /// (spec §4.2 "Phases"):
    ///
    /// 1. **capture**, top-down (`chain[0]` → `chain[N]`): the first
    ///    [`Flow::Consumed`] fully consumes that event (its `handle` phase is
    ///    skipped). This is where the exclusive keyboard owner claims text +
    ///    Enter/Esc so `Menu` never reaches the scene root.
    /// 2. **target + bubble**, high→low (`chain[0]` → `chain[N]`): the first
    ///    [`Flow::Consumed`] stops propagation; a [`Flow::Pass`] falls through to
    ///    the next handler; the gameplay-at-base handler (last in chain) runs
    ///    only when everything above it passed.
    ///
    /// Handlers push router-owned intents into `rc`; the caller reconciles them
    /// after dispatch with [`apply_context_requests`] and reads the returned
    /// [`DispatchReport`] (e.g. the scene turns `report.consumed_by(ROOT, Menu)`
    /// into a `Transition`).
    pub fn dispatch(
        events: &[InputEvent],
        chain: &mut [&mut dyn InputHandler],
        rc: &mut RouteCtx,
    ) -> DispatchReport {
        let mut report = DispatchReport::default();

        for ev in events {
            // ── Phase 1: capture (top-down, first Consumed stops the event) ──
            let mut captured = false;
            for (layer, handler) in chain.iter_mut().enumerate() {
                if handler.capture(ev, rc) == Flow::Consumed {
                    report.consumed.push((layer, ev.signal));
                    captured = true;
                    break;
                }
            }
            if captured {
                continue;
            }

            // ── Phase 2: target + bubble (high→low, first Consumed stops) ──
            let mut consumed = false;
            for (layer, handler) in chain.iter_mut().enumerate() {
                if handler.handle(ev, rc) == Flow::Consumed {
                    report.consumed.push((layer, ev.signal));
                    consumed = true;
                    break;
                }
            }
            if !consumed {
                report.passed.push(ev.signal);
            }
        }

        report
    }
}

/// A router-owned intent emitted by a handler during dispatch. These carry ONLY
/// context + focus changes — no `flicker-scene` dependency (structural scene
/// transitions stay with the scene, which reads the [`DispatchReport`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouterRequest {
    /// Push a context onto the active-context stack.
    PushContext(InputContext),
    /// Pop the top context (never below the `World` base).
    PopContext,
    /// Give keyboard/nav focus to the node with this id.
    SetFocus(String),
    /// Drop focus entirely.
    ClearFocus,
}

/// The router's scratch state for one dispatch: the queue handlers push intents
/// into. Carries router-owned intents only (spec §4.2 — no `flicker-scene` dep).
#[derive(Clone, Debug, Default)]
pub struct RouteCtx {
    /// Intents emitted this frame, in emission order.
    pub requests: Vec<RouterRequest>,
}

impl RouteCtx {
    /// A fresh, empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a [`RouterRequest::PushContext`].
    pub fn push_context(&mut self, context: InputContext) {
        self.requests.push(RouterRequest::PushContext(context));
    }

    /// Queue a [`RouterRequest::PopContext`].
    pub fn pop_context(&mut self) {
        self.requests.push(RouterRequest::PopContext);
    }

    /// Queue a [`RouterRequest::SetFocus`].
    pub fn set_focus(&mut self, id: impl Into<String>) {
        self.requests.push(RouterRequest::SetFocus(id.into()));
    }

    /// Queue a [`RouterRequest::ClearFocus`].
    pub fn clear_focus(&mut self) {
        self.requests.push(RouterRequest::ClearFocus);
    }
}

/// The resolved focus intent after a frame's requests are reconciled.
///
/// The router core is **window-free**, so it does not own the focus store
/// (`UiState.focus` lives in `flicker-widgets`, spec §4.3). [`apply_context_requests`]
/// therefore applies the context push/pop it owns to the [`ContextualBindings`]
/// and *returns* the focus decision for the caller to write through the walker
/// adapter — one focus identity for mouse + gamepad.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusChange {
    /// Focus the node with this id.
    Set(String),
    /// Clear focus.
    Clear,
}

/// Reconcile a frame's `requests` after [`Router::dispatch`] (spec §4.2a).
///
/// `PushContext` / `PopContext` are applied to `bindings` in **emission order**.
/// `SetFocus` / `ClearFocus` **collapse to the last one** (last-wins) and are
/// returned as an [`Option<FocusChange>`] rather than applied here, because the
/// focus store lives in `flicker-widgets` and the router core cannot depend on
/// it. Per §4.2a the caller emits requests so the winning (higher-priority)
/// handler's focus request is the last one in the queue; in the reference flow a
/// single handler owns focus per frame, so this is a stable convention rather
/// than a contended path.
pub fn apply_context_requests(
    bindings: &mut ContextualBindings,
    requests: &[RouterRequest],
) -> Option<FocusChange> {
    let mut focus: Option<FocusChange> = None;
    for req in requests {
        match req {
            RouterRequest::PushContext(context) => bindings.push(*context),
            RouterRequest::PopContext => {
                bindings.pop();
            }
            RouterRequest::SetFocus(id) => focus = Some(FocusChange::Set(id.clone())),
            RouterRequest::ClearFocus => focus = Some(FocusChange::Clear),
        }
    }
    focus
}

/// What each signal's routing did this frame. The scene reads this after
/// dispatch — e.g. turns `consumed_by(ROOT, Menu)` into a scene `Transition`, or
/// checks `passed(signal)` to know a signal fell through to gameplay.
///
/// Both queries are existential over the frame's events: `consumed_by(layer, s)`
/// is true if any event carrying `s` was consumed at that layer, and `passed(s)`
/// is true if any event carrying `s` reached the end of the chain unconsumed.
#[derive(Clone, Debug, Default)]
pub struct DispatchReport {
    /// `(layer, signal)` pairs consumed this frame (capture or handle phase).
    consumed: Vec<(usize, ActionSignal)>,
    /// Signals that passed all the way through the chain unconsumed.
    passed: Vec<ActionSignal>,
}

impl DispatchReport {
    /// Did `layer` consume `signal` this frame (in either phase)?
    pub fn consumed_by(&self, layer: usize, signal: ActionSignal) -> bool {
        self.consumed.iter().any(|&(l, s)| l == layer && s == signal)
    }

    /// Did `signal` fall through the whole chain unconsumed this frame?
    pub fn passed(&self, signal: ActionSignal) -> bool {
        self.passed.contains(&signal)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use flicker_input_core::{ActionSignal, ContextualBindings, EventKind, InputMap, InputState};

    use super::*;

    // ── A small mock handler that logs its calls in order and returns a
    //    configured verdict, optionally emitting a request during `handle`. ──
    type Log = Rc<RefCell<Vec<String>>>;

    struct Mock {
        name: &'static str,
        cap: Flow,
        hnd: Flow,
        log: Log,
        emit: Option<RouterRequest>,
    }

    impl Mock {
        fn new(name: &'static str, log: &Log) -> Self {
            Self { name, cap: Flow::Pass, hnd: Flow::Pass, log: log.clone(), emit: None }
        }
        fn capturing(mut self) -> Self {
            self.cap = Flow::Consumed;
            self
        }
        fn consuming(mut self) -> Self {
            self.hnd = Flow::Consumed;
            self
        }
        fn emitting(mut self, req: RouterRequest) -> Self {
            self.emit = Some(req);
            self
        }
    }

    impl InputHandler for Mock {
        fn capture(&mut self, _ev: &InputEvent, _rc: &mut RouteCtx) -> Flow {
            self.log.borrow_mut().push(format!("{}:capture", self.name));
            self.cap
        }
        fn handle(&mut self, _ev: &InputEvent, rc: &mut RouteCtx) -> Flow {
            self.log.borrow_mut().push(format!("{}:handle", self.name));
            if let Some(req) = &self.emit {
                rc.requests.push(req.clone());
            }
            self.hnd
        }
    }

    fn event<'a>(signal: ActionSignal, raw: &'a InputState) -> InputEvent<'a> {
        InputEvent::new(signal, EventKind::Press, InputContext::World, raw)
    }

    #[test]
    fn capture_stops_on_first_consumed() {
        let raw = InputState::new();
        let log: Log = Rc::default();
        let mut a = Mock::new("a", &log).capturing(); // consumes in capture
        let mut b = Mock::new("b", &log).consuming(); // would consume in handle
        let mut rc = RouteCtx::new();
        let events = [event(ActionSignal::Menu, &raw)];
        let mut chain: [&mut dyn InputHandler; 2] = [&mut a, &mut b];

        let report = Router::dispatch(&events, &mut chain, &mut rc);

        // Layer 0 captured; layer 1 never saw capture, no handle phase ran.
        assert!(report.consumed_by(0, ActionSignal::Menu));
        assert!(!report.passed(ActionSignal::Menu));
        assert_eq!(*log.borrow(), vec!["a:capture"]);
    }

    #[test]
    fn handle_runs_high_to_low_and_pass_falls_through() {
        let raw = InputState::new();
        let log: Log = Rc::default();
        let mut a = Mock::new("a", &log); // capture pass, handle pass
        let mut b = Mock::new("b", &log).consuming(); // handle consumes
        let mut c = Mock::new("c", &log).consuming(); // must never run
        let mut rc = RouteCtx::new();
        let events = [event(ActionSignal::Confirm, &raw)];
        let mut chain: [&mut dyn InputHandler; 3] = [&mut a, &mut b, &mut c];

        let report = Router::dispatch(&events, &mut chain, &mut rc);

        assert!(report.consumed_by(1, ActionSignal::Confirm));
        assert!(!report.consumed_by(2, ActionSignal::Confirm));
        assert!(!report.passed(ActionSignal::Confirm));
        // All captures first (0→2), then handle a (pass), handle b (consume);
        // c.handle is never reached.
        assert_eq!(
            *log.borrow(),
            vec!["a:capture", "b:capture", "c:capture", "a:handle", "b:handle"]
        );
    }

    #[test]
    fn base_handler_runs_only_on_all_pass() {
        let raw = InputState::new();

        // Case 1: everything above passes → the base handler runs and consumes.
        {
            let log: Log = Rc::default();
            let mut a = Mock::new("a", &log);
            let mut b = Mock::new("b", &log);
            let mut base = Mock::new("base", &log).consuming();
            let mut rc = RouteCtx::new();
            let events = [event(ActionSignal::PrimaryAction, &raw)];
            let mut chain: [&mut dyn InputHandler; 3] = [&mut a, &mut b, &mut base];

            let report = Router::dispatch(&events, &mut chain, &mut rc);

            assert!(report.consumed_by(2, ActionSignal::PrimaryAction));
            assert!(!report.passed(ActionSignal::PrimaryAction));
            assert_eq!(
                *log.borrow(),
                vec![
                    "a:capture",
                    "b:capture",
                    "base:capture",
                    "a:handle",
                    "b:handle",
                    "base:handle",
                ]
            );
        }

        // Case 2: a higher layer consumes → the base handler never runs.
        {
            let log: Log = Rc::default();
            let mut a = Mock::new("a", &log).consuming();
            let mut base = Mock::new("base", &log).consuming();
            let mut rc = RouteCtx::new();
            let events = [event(ActionSignal::PrimaryAction, &raw)];
            let mut chain: [&mut dyn InputHandler; 2] = [&mut a, &mut base];

            Router::dispatch(&events, &mut chain, &mut rc);

            assert!(!log.borrow().iter().any(|e| e == "base:handle"));
        }

        // Case 3: nothing consumes (base passes too) → the signal passed.
        {
            let log: Log = Rc::default();
            let mut a = Mock::new("a", &log);
            let mut base = Mock::new("base", &log);
            let mut rc = RouteCtx::new();
            let events = [event(ActionSignal::PrimaryAction, &raw)];
            let mut chain: [&mut dyn InputHandler; 2] = [&mut a, &mut base];

            let report = Router::dispatch(&events, &mut chain, &mut rc);

            assert!(report.passed(ActionSignal::PrimaryAction));
            assert!(!report.consumed_by(0, ActionSignal::PrimaryAction));
            assert!(!report.consumed_by(1, ActionSignal::PrimaryAction));
        }
    }

    #[test]
    fn requests_push_pop_in_emission_order_and_focus_last_wins() {
        let mut cb = ContextualBindings::new(InputMap::empty());
        let requests = vec![
            RouterRequest::SetFocus("a".into()),
            RouterRequest::PushContext(InputContext::Menu),
            RouterRequest::PushContext(InputContext::Radial),
            RouterRequest::PopContext, // pops Radial
            RouterRequest::SetFocus("b".into()),
        ];

        let focus = apply_context_requests(&mut cb, &requests);

        // push Menu, push Radial, pop Radial → Menu on top (emission order).
        assert_eq!(cb.active(), InputContext::Menu);
        // Later SetFocus wins over the earlier one.
        assert_eq!(focus, Some(FocusChange::Set("b".into())));
    }

    #[test]
    fn clear_focus_wins_when_emitted_last() {
        let mut cb = ContextualBindings::new(InputMap::empty());
        let requests = vec![RouterRequest::SetFocus("a".into()), RouterRequest::ClearFocus];
        assert_eq!(apply_context_requests(&mut cb, &requests), Some(FocusChange::Clear));
    }

    #[test]
    fn set_focus_wins_when_emitted_last() {
        let mut cb = ContextualBindings::new(InputMap::empty());
        let requests = vec![RouterRequest::ClearFocus, RouterRequest::SetFocus("z".into())];
        assert_eq!(apply_context_requests(&mut cb, &requests), Some(FocusChange::Set("z".into())));
    }

    #[test]
    fn no_focus_requests_returns_none() {
        let mut cb = ContextualBindings::new(InputMap::empty());
        let requests = vec![RouterRequest::PushContext(InputContext::Menu)];
        assert_eq!(apply_context_requests(&mut cb, &requests), None);
        assert_eq!(cb.active(), InputContext::Menu);
    }

    #[test]
    fn dispatch_collects_requests_then_apply_reconciles_them() {
        // The full seam: a handler emits an intent during dispatch; after
        // dispatch the caller drains it into the ContextualBindings.
        let raw = InputState::new();
        let log: Log = Rc::default();
        let mut owner = Mock::new("owner", &log)
            .consuming()
            .emitting(RouterRequest::PushContext(InputContext::TextEntry));
        let mut rc = RouteCtx::new();
        let events = [event(ActionSignal::SubmitText, &raw)];
        let mut chain: [&mut dyn InputHandler; 1] = [&mut owner];

        Router::dispatch(&events, &mut chain, &mut rc);
        assert_eq!(rc.requests.len(), 1);

        let mut cb = ContextualBindings::new(InputMap::empty());
        apply_context_requests(&mut cb, &rc.requests);
        rc.requests.clear();

        assert_eq!(cb.active(), InputContext::TextEntry);
    }
}
