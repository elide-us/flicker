//! Engine-side input snapshot + semantic bindings layer.
//!
//! [`InputState`] is a per-frame, polled snapshot — the same model
//! DXTK and XNA use. The driver (typically `flicker-app`) accumulates
//! platform events into the snapshot; game code reads it from
//! `App::update`.
//!
//! Game code should usually NOT read raw [`Key`] values. Instead, hold
//! a [`bindings::Bindings`] map and query semantic
//! [`bindings::Action`]s via [`InputState::action_active`]. That keeps
//! the physical-key→action mapping rebindable without touching
//! consumer code. See the [`bindings`] module docs for the design.
//!
//! The types here are intentionally platform-agnostic — `flicker-core`
//! does not depend on `winit`. Translation from `winit` events happens
//! in `flicker-app`.

pub mod bindings;

use std::collections::HashSet;

use glam::Vec2;

use self::bindings::{Action, Bindings};

/// Symbolic key identifier.
///
/// Covers the keys most commonly bound in 3D-camera examples — letters
/// used by WASD/ESDF/QE/RF movement clusters, arrow keys, and the
/// usual modifiers. Add variants here (and the matching mapping in the
/// driver) as games need them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Escape,
    // Letters used by default + common alternate movement bindings.
    A,
    B,
    C,
    D,
    E,
    F,
    Q,
    R,
    S,
    W,
    X,
    Z,
    // Arrow keys — sometimes bound to look or movement.
    Up,
    Down,
    Left,
    Right,
    // Modifiers / extras likely to be bound.
    Space,
    LeftShift,
    LeftControl,
    // Number-row digits — useful for debug toggles in examples.
    Digit1,
    Digit2,
    // Backslash — `\` key, used as another debug toggle.
    Backslash,
}

/// Per-frame input snapshot. Held-state plus a small set of "just
/// happened this frame" edge fields reset by the driver after
/// `App::update`.
#[derive(Default, Clone, Debug)]
pub struct InputState {
    /// Mouse position in pixels, origin top-left, matching the renderer's
    /// coordinate system. `(0, 0)` until the first cursor event.
    pub mouse_position: Vec2,
    pub mouse_left: bool,
    pub mouse_right: bool,
    pub mouse_middle: bool,
    /// `true` only on the first frame after the left button transitions
    /// up→down. Driver-set, reset after each `App::update`. Use for
    /// click-to-toggle UI; use [`Self::mouse_left`] for held / drag
    /// behavior.
    pub mouse_left_pressed: bool,
    /// Accumulated scroll delta since the previous frame consumed it.
    /// Positive = scroll up (wheel toward the user / two-finger swipe up).
    /// The driver resets this to `0.0` immediately after `App::update`
    /// returns so each frame sees only that frame's scroll events.
    pub mouse_wheel_delta: f32,
    keys_held: HashSet<Key>,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Is this key currently held down?
    pub fn key_down(&self, key: Key) -> bool {
        self.keys_held.contains(&key)
    }

    /// Is any key bound to `action` currently held?
    ///
    /// Consumers should prefer this over [`Self::key_down`] so the
    /// physical-key→action mapping can be rebound without touching
    /// consumer code.
    pub fn action_active(&self, bindings: &Bindings, action: Action) -> bool {
        self.keys_held
            .iter()
            .any(|k| bindings.action_for(*k) == Some(action))
    }

    /// Driver hook: update the held-state for a key. Not part of the polling
    /// API — game code should use [`InputState::key_down`] or
    /// [`InputState::action_active`] instead.
    pub fn set_key(&mut self, key: Key, down: bool) {
        if down {
            self.keys_held.insert(key);
        } else {
            self.keys_held.remove(&key);
        }
    }
}
