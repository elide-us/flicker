//! Engine-side input snapshot.
//!
//! [`InputState`] is a per-frame, polled snapshot — the same model DXTK and
//! XNA use. The driver (typically `flicker-app`) accumulates platform events
//! into the snapshot; game code reads it from `App::update`.
//!
//! The types here are intentionally platform-agnostic — `flicker-core` does
//! not depend on `winit`. Translation from `winit` events happens in
//! `flicker-app`.

use std::collections::HashSet;

use glam::Vec2;

/// Symbolic key identifier.
///
/// Intentionally minimal: variants are added as engine consumers need them,
/// rather than enumerating every key on every keyboard layout up front. If a
/// key you need is missing, add a variant and a mapping in the driver.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Escape,
}

/// Per-frame input snapshot. Held-state only — edge detection (just-pressed /
/// just-released) will be added when a game needs it.
#[derive(Default, Clone, Debug)]
pub struct InputState {
    /// Mouse position in pixels, origin top-left, matching the renderer's
    /// coordinate system. `(0, 0)` until the first cursor event.
    pub mouse_position: Vec2,
    pub mouse_left: bool,
    pub mouse_right: bool,
    pub mouse_middle: bool,
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

    /// Driver hook: update the held-state for a key. Not part of the polling
    /// API — game code should use [`InputState::key_down`] instead.
    pub fn set_key(&mut self, key: Key, down: bool) {
        if down {
            self.keys_held.insert(key);
        } else {
            self.keys_held.remove(&key);
        }
    }
}
