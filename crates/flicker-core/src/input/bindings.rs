//! Semantic input bindings: actions decoupled from physical keys.
//!
//! Consumer code asks "is the `MoveForward` action active?" — never
//! "is the W key down?". The mapping from physical [`super::Key`] to
//! [`Action`] lives in a [`Bindings`] table that can be swapped or
//! rebound at runtime without touching the consumer.
//!
//! [`ControlConfig`] sits on top: per-axis invert flags, look
//! sensitivity, and movement speed. Use [`ControlConfig::look_delta`]
//! to turn a raw cursor delta into a `(yaw, pitch)` increment with all
//! invert / sensitivity already applied — the consumer just adds the
//! result to its camera angles.

use std::collections::HashMap;

use glam::Vec2;

use super::Key;

/// Semantic input action. Game/example code reacts to these; the
/// physical-key→action mapping is in [`Bindings`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    MoveForward,
    MoveBackward,
    StrafeLeft,
    StrafeRight,
    MoveUp,
    MoveDown,
    Quit,
}

/// Maps physical keys to semantic actions.
///
/// Multiple keys may map to the same action (e.g. both `W` and `Up`
/// for `MoveForward`); one key maps to at most one action (last write
/// wins).
#[derive(Clone, Debug)]
pub struct Bindings {
    key_to_action: HashMap<Key, Action>,
}

impl Bindings {
    /// Empty bindings — nothing bound.
    pub fn empty() -> Self {
        Self {
            key_to_action: HashMap::new(),
        }
    }

    /// Default WASD layout: W/A/S/D move + strafe, R/F up/down, Esc quit.
    pub fn wasd() -> Self {
        let mut b = Self::empty();
        b.bind(Key::W, Action::MoveForward);
        b.bind(Key::S, Action::MoveBackward);
        b.bind(Key::A, Action::StrafeLeft);
        b.bind(Key::D, Action::StrafeRight);
        b.bind(Key::R, Action::MoveUp);
        b.bind(Key::F, Action::MoveDown);
        b.bind(Key::Escape, Action::Quit);
        b
    }

    /// ESDF layout: E/D/S/F move + strafe; R = up, W = down (the up
    /// and down keys bracket the move cluster vertically); Esc quit.
    pub fn esdf() -> Self {
        let mut b = Self::empty();
        b.bind(Key::E, Action::MoveForward);
        b.bind(Key::D, Action::MoveBackward);
        b.bind(Key::S, Action::StrafeLeft);
        b.bind(Key::F, Action::StrafeRight);
        b.bind(Key::R, Action::MoveUp);
        b.bind(Key::W, Action::MoveDown);
        b.bind(Key::Escape, Action::Quit);
        b
    }

    /// Bind a physical key to an action (overwrites any prior binding
    /// for that key).
    pub fn bind(&mut self, key: Key, action: Action) {
        self.key_to_action.insert(key, action);
    }

    /// Remove any binding for a key.
    pub fn unbind(&mut self, key: Key) {
        self.key_to_action.remove(&key);
    }

    /// The action bound to `key`, if any.
    pub fn action_for(&self, key: Key) -> Option<Action> {
        self.key_to_action.get(&key).copied()
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::wasd()
    }
}

/// Tunable control preferences applied on top of [`Bindings`]. These
/// shape how raw input becomes camera / movement intent.
#[derive(Clone, Copy, Debug)]
pub struct ControlConfig {
    /// Look sensitivity in radians per pixel of cursor movement.
    pub look_sensitivity: f32,
    /// Movement speed in world units per second.
    pub move_speed: f32,
    /// Invert vertical look (joystick / old-school flight style): when
    /// `true`, moving the cursor up pitches the view down.
    pub invert_pitch: bool,
    /// Invert horizontal look. Rare, but symmetric with `invert_pitch`.
    pub invert_yaw: bool,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            look_sensitivity: 0.005,
            move_speed: 120.0,
            invert_pitch: false,
            invert_yaw: false,
        }
    }
}

impl ControlConfig {
    /// Convert a raw cursor pixel delta into a `(yaw_delta,
    /// pitch_delta)` in radians, applying sensitivity and invert
    /// flags. The caller adds these to its camera angles.
    ///
    /// Pitch convention: positive pitch looks up. Screen Y grows
    /// downward, so a cursor moved up (negative `dy`) yields positive
    /// pitch in the non-inverted default; `invert_pitch` flips it.
    pub fn look_delta(&self, cursor_delta: Vec2) -> (f32, f32) {
        let yaw_sign = if self.invert_yaw { -1.0 } else { 1.0 };
        let pitch_sign = if self.invert_pitch { -1.0 } else { 1.0 };
        let yaw = cursor_delta.x * self.look_sensitivity * yaw_sign;
        let pitch = (-cursor_delta.y) * self.look_sensitivity * pitch_sign;
        (yaw, pitch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputState;

    #[test]
    fn wasd_binds_movement() {
        let b = Bindings::wasd();
        assert_eq!(b.action_for(Key::W), Some(Action::MoveForward));
        assert_eq!(b.action_for(Key::S), Some(Action::MoveBackward));
        assert_eq!(b.action_for(Key::A), Some(Action::StrafeLeft));
        assert_eq!(b.action_for(Key::D), Some(Action::StrafeRight));
        assert_eq!(b.action_for(Key::R), Some(Action::MoveUp));
        assert_eq!(b.action_for(Key::F), Some(Action::MoveDown));
        assert_eq!(b.action_for(Key::Escape), Some(Action::Quit));
    }

    #[test]
    fn esdf_binds_movement() {
        let b = Bindings::esdf();
        assert_eq!(b.action_for(Key::E), Some(Action::MoveForward));
        assert_eq!(b.action_for(Key::D), Some(Action::MoveBackward));
        assert_eq!(b.action_for(Key::S), Some(Action::StrafeLeft));
        assert_eq!(b.action_for(Key::F), Some(Action::StrafeRight));
        assert_eq!(b.action_for(Key::R), Some(Action::MoveUp));
        assert_eq!(b.action_for(Key::W), Some(Action::MoveDown));
    }

    #[test]
    fn empty_has_no_bindings() {
        let b = Bindings::empty();
        assert_eq!(b.action_for(Key::W), None);
    }

    #[test]
    fn rebinding_overwrites() {
        let mut b = Bindings::empty();
        b.bind(Key::E, Action::MoveForward);
        b.bind(Key::E, Action::MoveBackward); // overwrite
        assert_eq!(b.action_for(Key::E), Some(Action::MoveBackward));
    }

    #[test]
    fn unbind_removes() {
        let mut b = Bindings::wasd();
        b.unbind(Key::W);
        assert_eq!(b.action_for(Key::W), None);
    }

    #[test]
    fn default_is_wasd() {
        let b = Bindings::default();
        assert_eq!(b.action_for(Key::W), Some(Action::MoveForward));
    }

    #[test]
    fn look_delta_default_is_not_inverted() {
        let cfg = ControlConfig::default();
        let (yaw, pitch) = cfg.look_delta(Vec2::new(10.0, 10.0));
        // dx > 0 → positive yaw with sensitivity.
        assert!(yaw > 0.0);
        // dy > 0 (cursor moved down on screen) → negative pitch
        // (looking down) in the non-inverted convention.
        assert!(pitch < 0.0);
    }

    #[test]
    fn invert_pitch_flips_vertical_only() {
        let mut cfg = ControlConfig::default();
        let d = Vec2::new(10.0, 10.0);
        let (yaw0, pitch0) = cfg.look_delta(d);
        cfg.invert_pitch = true;
        let (yaw1, pitch1) = cfg.look_delta(d);
        assert_eq!(yaw0, yaw1, "yaw unaffected by invert_pitch");
        assert_eq!(pitch0, -pitch1, "pitch sign flipped");
    }

    #[test]
    fn invert_yaw_flips_horizontal_only() {
        let mut cfg = ControlConfig::default();
        let d = Vec2::new(10.0, 10.0);
        let (yaw0, pitch0) = cfg.look_delta(d);
        cfg.invert_yaw = true;
        let (yaw1, pitch1) = cfg.look_delta(d);
        assert_eq!(pitch0, pitch1);
        assert_eq!(yaw0, -yaw1);
    }

    #[test]
    fn action_active_reads_bindings() {
        let b = Bindings::wasd();
        let mut input = InputState::new();
        input.set_key(Key::W, true);
        assert!(input.action_active(&b, Action::MoveForward));
        assert!(!input.action_active(&b, Action::MoveBackward));
    }

    #[test]
    fn action_active_supports_multiple_keys_per_action() {
        let mut b = Bindings::wasd();
        b.bind(Key::Up, Action::MoveForward); // alias W
        let mut input = InputState::new();
        input.set_key(Key::Up, true);
        assert!(input.action_active(&b, Action::MoveForward));
    }
}
