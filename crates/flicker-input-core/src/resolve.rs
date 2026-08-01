//! Edge resolution: `(prev, curr) × active context → Vec<Fired>`.
//!
//! Promotes the tester mockup's `When` / `fired()` / `binds()` (`signals.rs`) into
//! the engine. The [`Resolver`] is the single home of previous-frame state and
//! per-binding press-times, replacing every scene's hand-rolled `*_prev` bools
//! (spec R4 / §3.2). Timing is driven by a monotonic [`TickTime`] tick, **not**
//! wall-clock `Instant`, so discrete resolution is deterministic and replayable
//! (spec §3.2a / Risk RT-1).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::binding::InputBinding;
use crate::context::ContextualBindings;
use crate::signal::ActionSignal;
use crate::snapshot::{GamepadConfig, InputState};

/// A monotonic tick / frame counter (NOT `std::time::Instant`). Supplied by the
/// caller; `flicker-input-core` stays platform-free.
pub type TickTime = u64;

/// When a control fires its signal. Promotes the tester's `When` and adds
/// `Chord` (spec §3.2).
///
/// Event-kind is **data**, case-by-case per (control, context): combat = `Press`,
/// menus = `Release`, nav = press-with-repeat, split cases like Y (press →
/// `ChordBegin`, release → `Jump`). Never hardcoded into a control.
///
/// Serde-stable: it rides inside the persisted profile's per-context reshape
/// (`ContextBindings::default_event` / `SignalBinding::event`, spec §7.1), so JSON
/// carries the variant name. **ADD variants only, never rename** (`C60AE43C §2`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    /// Edge down — combat responsiveness.
    Press,
    /// Edge up — menu UX (escape a mis-press before releasing).
    Release,
    /// Held past a threshold (scaffolded this slice; not yet emitted).
    Hold,
    /// A modifier chord opened (scaffolded this slice; not yet emitted).
    Chord,
}

/// One resolved event: the signal, how it fired, and the physical control that
/// produced it. The router wraps this into an `InputEvent` (adding context +
/// raw state) downstream (spec R3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Fired {
    pub signal: ActionSignal,
    pub kind: EventKind,
    pub control: InputBinding,
}

/// Owns the previous discrete snapshot + per-binding press-times, so it — not the
/// scene, not the device — is the single home of edge + Hold/Chord timing.
///
/// First slice implements `Press`/`Release` fully (all pocclusters needs);
/// `Hold`/`Chord` are scaffolded — the API carries `now` and the resolver keeps
/// press-times — but not yet emitted, pending the combat scenes (spec §3.2).
#[derive(Clone, Debug, Default)]
pub struct Resolver {
    prev: Option<InputState>,
    press_at: HashMap<InputBinding, TickTime>,
}

impl Resolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve `(prev, curr)` over the ACTIVE context's map into `Fired`s, written
    /// into the caller-owned `out` buffer (no per-frame heap alloc — the caller
    /// reuses a cleared `Vec`, spec §9 / Risk RT-7). Promotes `signals::fired()`
    /// / `binds()`.
    ///
    /// `Press` fires on the up→down edge of a bound control; `Release` on down→up.
    /// Per-binding press-times are recorded on press and cleared on release, so a
    /// later step can emit `Hold`/`Chord` from `now - press_at` without adding
    /// state.
    pub fn resolve_frame(
        &mut self,
        bindings: &ContextualBindings,
        cfg: &GamepadConfig,
        curr: &InputState,
        now: TickTime,
        out: &mut Vec<Fired>,
    ) {
        let map = bindings.active_map();
        for signal in map.bound_actions() {
            for &control in map.bindings_for(signal) {
                let is_down = control.is_down(curr, cfg);
                let was_down = self
                    .prev
                    .as_ref()
                    .is_some_and(|p| control.is_down(p, cfg));
                if is_down && !was_down {
                    self.press_at.insert(control, now);
                    out.push(Fired { signal, kind: EventKind::Press, control });
                } else if !is_down && was_down {
                    self.press_at.remove(&control);
                    out.push(Fired { signal, kind: EventKind::Release, control });
                }
            }
        }
        self.prev = Some(curr.clone());
    }

    /// Ticks a control has been held since its press edge, or `None` if it is not
    /// currently pressed. Scaffolding for Hold/Chord (spec §3.2a); not yet emitted
    /// as `Fired` events.
    pub fn held_ticks(&self, control: InputBinding, now: TickTime) -> Option<TickTime> {
        self.press_at.get(&control).map(|&t| now.saturating_sub(t))
    }

    /// Drop the remembered previous frame + press-times (e.g. on a context reset).
    pub fn reset(&mut self) {
        self.prev = None;
        self.press_at.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{InputBinding, InputMap};
    use crate::device::Key;

    fn jump_on_space() -> ContextualBindings {
        let mut m = InputMap::empty();
        m.bind(ActionSignal::Jump, InputBinding::Key(Key::Space));
        ContextualBindings::new(m)
    }

    #[test]
    fn emits_press_then_release() {
        let cb = jump_on_space();
        let cfg = GamepadConfig::default();
        let mut r = Resolver::new();
        let mut out = Vec::new();

        // Frame 0: idle → nothing, seeds prev.
        let idle = InputState::new();
        r.resolve_frame(&cb, &cfg, &idle, 0, &mut out);
        assert!(out.is_empty());

        // Frame 1: Space down → Press(Jump), press-time recorded.
        out.clear();
        let mut down = InputState::new();
        down.set_key(Key::Space, true);
        r.resolve_frame(&cb, &cfg, &down, 1, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].signal, ActionSignal::Jump);
        assert_eq!(out[0].kind, EventKind::Press);
        assert_eq!(out[0].control, InputBinding::Key(Key::Space));
        assert_eq!(r.held_ticks(InputBinding::Key(Key::Space), 5), Some(4));

        // Frame 2: still held → no new edge.
        out.clear();
        r.resolve_frame(&cb, &cfg, &down, 2, &mut out);
        assert!(out.is_empty());

        // Frame 3: released → Release(Jump), press-time cleared.
        out.clear();
        r.resolve_frame(&cb, &cfg, &idle, 3, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, EventKind::Release);
        assert_eq!(r.held_ticks(InputBinding::Key(Key::Space), 5), None);
    }

    #[test]
    fn reset_clears_history() {
        let cb = jump_on_space();
        let cfg = GamepadConfig::default();
        let mut r = Resolver::new();
        let mut out = Vec::new();

        let mut down = InputState::new();
        down.set_key(Key::Space, true);
        r.resolve_frame(&cb, &cfg, &down, 0, &mut out);
        out.clear();
        r.reset();
        // After reset, a still-held control reads as a fresh press again.
        r.resolve_frame(&cb, &cfg, &down, 1, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, EventKind::Press);
    }
}
