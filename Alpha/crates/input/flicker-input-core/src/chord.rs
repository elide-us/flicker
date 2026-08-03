//! **The chord layer** — hold a modifier, and the controls under it mean
//! editor verbs.
//!
//! A bench has more verbs than a pad has buttons. The answer is a modifier:
//! hold [`ActionSignal::ChordBegin`] (Y by default) and L2 means `Cut`, R1
//! means `Redo`, and so on.
//!
//! # It is a CONTEXT, not a new event kind
//!
//! [`EventKind::Chord`](crate::EventKind::Chord) exists as scaffolding, but
//! emitting it would make a member fire with its OWN signal — a bench would
//! receive `(ModePrev, Chord)` and need a second table to turn that back into
//! `Cut`. Layering instead gives the right thing directly: while the modifier
//! is held, [`InputContext::Chord`] is on the binding stack, so its map IS the
//! active one and L2 resolves to `Cut` and to nothing else.
//!
//! That also satisfies the documented `ChordBegin` contract — *"members' normal
//! signals are suppressed while it is held"* — structurally, with no
//! suppression logic to get wrong. And because the layer is an ordinary
//! [`InputMap`], every chord is rebindable exactly like every other binding.
//!
//! # Ergonomics (Aaron's ruling, 2026-08-02)
//!
//! **Chord members must not be face buttons.** Holding Y commits that thumb, so
//! Y+X and Y+A cannot be pressed, and Y+B is awkward. Members are shoulders,
//! triggers, stick clicks and the d-pad — reachable by the other hand or by
//! fingers while the modifier is held. [`editor_chords`] follows this; a
//! rebind that breaks it is the user's call, not the default's.

use crate::binding::{InputBinding, InputMap};
use crate::context::{ContextualBindings, InputContext};
use crate::device::GamepadButton;
use crate::device::Key;
use crate::resolve::{EventKind, Fired};
use crate::snapshot::{GamepadConfig, InputState};
use crate::signal::ActionSignal;

/// Keeps the chord layer in step with the modifier that opened it.
///
/// # Why this is stateful
///
/// The obvious implementation — push on a `ChordBegin` press, pop on its
/// release — **strands the layer open forever**. Once pushed, the chord map is
/// the active one, and it binds editor verbs, not `ChordBegin`; so the
/// modifier's release resolves to nothing, the pop never runs, and the bench is
/// left in chord mode until it is restarted.
///
/// So the layer remembers the CONTROL that opened it and closes when that exact
/// control is no longer physically down. Reading the control's state rather than
/// waiting for an edge also makes it self-healing: a release swallowed by an
/// alt-tab, a focus loss, or a context switch closes the layer on the next frame
/// instead of wedging it.
#[derive(Clone, Debug, Default)]
pub struct ChordLayer {
    /// The control holding the layer open, if it is open.
    held_by: Option<InputBinding>,
}

impl ChordLayer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the layer is currently held — for a "chord open" hint bar.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.held_by.is_some()
    }

    /// The control holding the layer open.
    #[must_use]
    pub fn held_by(&self) -> Option<InputBinding> {
        self.held_by
    }

    /// Reconcile the layer with this frame. Call once per frame with the
    /// resolver's output, right after `resolve_frame`.
    ///
    /// Returns `true` when the layer opened or closed this frame.
    pub fn update(
        &mut self,
        bindings: &mut ContextualBindings,
        fired: &[Fired],
        curr: &InputState,
        cfg: &GamepadConfig,
    ) -> bool {
        // Closing wins: check the remembered control against physical state, so
        // a swallowed release cannot wedge the layer.
        if let Some(control) = self.held_by {
            if !control.is_down(curr, cfg) {
                if bindings.active() == InputContext::Chord {
                    bindings.pop();
                }
                self.held_by = None;
                return true;
            }
            return false; // already open and still held
        }
        for f in fired {
            if f.signal == ActionSignal::ChordBegin && f.kind == EventKind::Press {
                bindings.push(InputContext::Chord);
                self.held_by = Some(f.control);
                return true;
            }
        }
        false
    }
}

/// The default editor chord layer: the verbs a bench exposes while the modifier
/// is held.
///
/// Pad members are all NON-FACE (see the module note). Keyboard equivalents ride
/// the same map so a key and a chord produce one signal — `Ctrl` is the
/// keyboard's modifier by convention, but the keys are listed here bare because
/// the layer is only active while the modifier is held, which is exactly what
/// `Ctrl` means.
#[must_use]
pub fn editor_chords() -> InputMap {
    let mut m = InputMap::empty();

    // Shoulders: history, because a shoulder reads as "back / forward in time".
    m.bind(ActionSignal::Undo, InputBinding::GamepadButton(GamepadButton::LeftBumper));
    m.bind(ActionSignal::Undo, InputBinding::Key(Key::Z));
    m.bind(ActionSignal::Redo, InputBinding::GamepadButton(GamepadButton::RightBumper));
    m.bind(ActionSignal::Redo, InputBinding::Key(Key::Y));

    // Triggers: move the thing — pick it up, put it down.
    m.bind(ActionSignal::Cut, InputBinding::GamepadButton(GamepadButton::LeftTrigger));
    m.bind(ActionSignal::Cut, InputBinding::Key(Key::X));
    m.bind(ActionSignal::Paste, InputBinding::GamepadButton(GamepadButton::RightTrigger));
    m.bind(ActionSignal::Paste, InputBinding::Key(Key::V));

    // D-pad: the naming verbs.
    m.bind(ActionSignal::CreateFolder, InputBinding::GamepadButton(GamepadButton::DPadUp));
    m.bind(ActionSignal::CreateFolder, InputBinding::Key(Key::N));
    m.bind(ActionSignal::Rename, InputBinding::GamepadButton(GamepadButton::DPadDown));
    m.bind(ActionSignal::Rename, InputBinding::Key(Key::R));

    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Resolver;

    fn bindings_with_chord() -> ContextualBindings {
        let mut world = InputMap::empty();
        // Y opens the layer; L2 normally means ModePrev.
        world.bind(ActionSignal::ChordBegin, InputBinding::GamepadButton(GamepadButton::North));
        world.bind(ActionSignal::ModePrev, InputBinding::GamepadButton(GamepadButton::LeftTrigger));
        ContextualBindings::new(world).with(InputContext::Chord, editor_chords())
    }

    fn pad_down(buttons: &[GamepadButton]) -> InputState {
        let mut s = InputState::new();
        let gp = s.gamepad_mut(0);
        for b in buttons {
            gp.set_button(*b, true);
        }
        s
    }

    /// THE contract: while the modifier is held, a member resolves to its EDITOR
    /// VERB and its normal signal is suppressed — structurally, because the
    /// chord map is the active one.
    #[test]
    fn holding_the_modifier_turns_a_member_into_its_editor_verb() {
        let cfg = GamepadConfig::default();
        let b = bindings_with_chord();
        let mut r = Resolver::new();
        let mut out = Vec::new();

        // Baseline: L2 alone is ModePrev.
        r.resolve_frame(&b, &cfg, &InputState::new(), 0, &mut out);
        out.clear();
        r.resolve_frame(&b, &cfg, &pad_down(&[GamepadButton::LeftTrigger]), 1, &mut out);
        assert!(
            out.iter().any(|f| f.signal == ActionSignal::ModePrev),
            "L2 alone is ModePrev: {out:?}"
        );

        // Hold Y: the layer opens.
        let mut b = bindings_with_chord();
        let mut layer = ChordLayer::new();
        let mut r = Resolver::new();
        out.clear();
        r.resolve_frame(&b, &cfg, &InputState::new(), 0, &mut out);

        let held = pad_down(&[GamepadButton::North]);
        out.clear();
        r.resolve_frame(&b, &cfg, &held, 1, &mut out);
        assert!(layer.update(&mut b, &out, &held, &cfg), "the layer opened");
        assert!(layer.is_open());

        // Now L2 is Cut, and ModePrev never fires.
        let both = pad_down(&[GamepadButton::North, GamepadButton::LeftTrigger]);
        out.clear();
        r.resolve_frame(&b, &cfg, &both, 2, &mut out);
        layer.update(&mut b, &out, &both, &cfg);
        assert!(out.iter().any(|f| f.signal == ActionSignal::Cut), "L2 is Cut in the layer: {out:?}");
        assert!(
            !out.iter().any(|f| f.signal == ActionSignal::ModePrev),
            "the member's normal signal is SUPPRESSED: {out:?}"
        );
    }

    /// REGRESSION: the chord map does not bind `ChordBegin`, so the modifier's
    /// RELEASE resolves to nothing while the layer is active. An edge-driven
    /// implementation strands the layer open forever; this closes on the
    /// control's physical state instead.
    #[test]
    fn releasing_the_modifier_closes_the_layer() {
        let cfg = GamepadConfig::default();
        let mut b = bindings_with_chord();
        let mut layer = ChordLayer::new();
        let mut r = Resolver::new();
        let mut out = Vec::new();
        r.resolve_frame(&b, &cfg, &InputState::new(), 0, &mut out);

        let held = pad_down(&[GamepadButton::North]);
        out.clear();
        r.resolve_frame(&b, &cfg, &held, 1, &mut out);
        layer.update(&mut b, &out, &held, &cfg);
        assert!(layer.is_open());

        // Release: the resolver emits NOTHING for it (the chord map has no
        // ChordBegin binding) — the layer must still close.
        let idle = InputState::new();
        out.clear();
        r.resolve_frame(&b, &cfg, &idle, 2, &mut out);
        assert!(
            !out.iter().any(|f| f.signal == ActionSignal::ChordBegin),
            "no release edge is available while the layer is active: {out:?}"
        );
        assert!(layer.update(&mut b, &out, &idle, &cfg), "the layer closed anyway");
        assert!(!layer.is_open(), "back to the base map");
        assert_eq!(b.active(), InputContext::World);
    }

    /// A swallowed release (alt-tab mid-hold) must not wedge the bench in chord
    /// mode, and repeated frames must not stack duplicate layers.
    #[test]
    fn a_swallowed_release_cannot_strand_the_layer() {
        let cfg = GamepadConfig::default();
        let mut b = bindings_with_chord();
        let mut layer = ChordLayer::new();
        let press = Fired {
            signal: ActionSignal::ChordBegin,
            kind: EventKind::Press,
            control: InputBinding::GamepadButton(GamepadButton::North),
        };
        let held = pad_down(&[GamepadButton::North]);

        assert!(layer.update(&mut b, &[press], &held, &cfg));
        // Same press seen again while still held — no second push.
        assert!(!layer.update(&mut b, &[press], &held, &cfg), "no duplicate layer");
        assert!(layer.is_open());

        // The pad simply stops reporting the button; no edge is delivered.
        assert!(layer.update(&mut b, &[], &InputState::new(), &cfg), "closed on physical state");
        assert!(!layer.is_open());
        assert_eq!(b.active(), InputContext::World);
    }

    /// The ergonomic ruling, pinned: a held face button commits the thumb, so no
    /// chord member may be one.
    #[test]
    fn no_chord_member_is_a_face_button() {
        let m = editor_chords();
        const FACE: [GamepadButton; 4] = [
            GamepadButton::North,
            GamepadButton::South,
            GamepadButton::East,
            GamepadButton::West,
        ];
        for signal in m.bound_actions() {
            for b in m.bindings_for(signal) {
                if let InputBinding::GamepadButton(g) = b {
                    assert!(
                        !FACE.contains(g),
                        "{signal:?} is bound to the face button {g:?} — unreachable while the \
                         modifier is held (Aaron's ergonomic ruling)"
                    );
                }
            }
        }
    }
}
