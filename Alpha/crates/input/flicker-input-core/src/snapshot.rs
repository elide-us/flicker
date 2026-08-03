//! The per-frame polled input snapshot + gamepad state and config.
//!
//! [`InputState`] is a per-frame held-state snapshot (the same model DXTK/XNA
//! use). The `flicker-input-device` crate accumulates platform events into it;
//! game code reads it. Moved from `flicker-core::input`, **minus** the legacy
//! `action_active` method (spec §3.4 / checklist 2) and **plus** an
//! `analog_latch` placeholder the device crate will fill from the 120 Hz cache
//! (spec §6.3).

use std::collections::{HashMap, HashSet};

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::analog::AnalogFrame;
use crate::binding::{InputBinding, InputMap};
use crate::device::{AxisDirection, GamepadAxis, GamepadButton, Key, MouseButton};
use crate::signal::ActionSignal;

// ───────────────────────────────────────────────────────────────────
// Gamepad config + state
// ───────────────────────────────────────────────────────────────────

/// Per-gamepad configuration (deadzones, thresholds).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GamepadConfig {
    /// Deadzone for the left analog stick (0.0–1.0).
    pub left_stick_deadzone: f32,
    /// Deadzone for the right analog stick (0.0–1.0).
    pub right_stick_deadzone: f32,
    /// Deadzone shape applied to sticks.
    pub deadzone_shape: crate::device::DeadzoneShape,
    /// Threshold above which a trigger is considered "pressed" (0.0–1.0).
    pub trigger_threshold: f32,
}

impl Default for GamepadConfig {
    fn default() -> Self {
        Self {
            left_stick_deadzone: 0.15,
            right_stick_deadzone: 0.15,
            deadzone_shape: crate::device::DeadzoneShape::Circular,
            trigger_threshold: 0.5,
        }
    }
}

/// Snapshot state for a single connected gamepad.
///
/// The device crate updates this each frame. Game code polls it via
/// [`InputState::gamepad()`].
#[derive(Clone, Debug)]
pub struct GamepadState {
    buttons_held: HashSet<GamepadButton>,
    axes: HashMap<GamepadAxis, f32>,
    config: GamepadConfig,
}

impl GamepadState {
    pub fn new(config: GamepadConfig) -> Self {
        Self {
            buttons_held: HashSet::new(),
            axes: HashMap::new(),
            config,
        }
    }

    /// Is this button currently held?
    pub fn button_down(&self, button: GamepadButton) -> bool {
        self.buttons_held.contains(&button)
    }

    /// Raw axis value (–1.0 to 1.0 for sticks, 0.0 to 1.0 for triggers).
    pub fn axis_value(&self, axis: GamepadAxis) -> f32 {
        self.axes.get(&axis).copied().unwrap_or(0.0)
    }

    /// Left stick position with deadzone applied.
    pub fn left_stick(&self) -> Vec2 {
        let raw = Vec2::new(
            self.axis_value(GamepadAxis::LeftStickX),
            self.axis_value(GamepadAxis::LeftStickY),
        );
        apply_deadzone(raw, self.config.left_stick_deadzone, self.config.deadzone_shape)
    }

    /// Right stick position with deadzone applied.
    pub fn right_stick(&self) -> Vec2 {
        let raw = Vec2::new(
            self.axis_value(GamepadAxis::RightStickX),
            self.axis_value(GamepadAxis::RightStickY),
        );
        apply_deadzone(raw, self.config.right_stick_deadzone, self.config.deadzone_shape)
    }

    /// Left trigger value (0.0–1.0).
    pub fn left_trigger(&self) -> f32 {
        self.axis_value(GamepadAxis::LeftTrigger)
    }

    /// Right trigger value (0.0–1.0).
    pub fn right_trigger(&self) -> f32 {
        self.axis_value(GamepadAxis::RightTrigger)
    }

    /// Is the left trigger past the configured threshold?
    pub fn left_trigger_down(&self) -> bool {
        self.left_trigger() >= self.config.trigger_threshold
    }

    /// Is the right trigger past the configured threshold?
    pub fn right_trigger_down(&self) -> bool {
        self.right_trigger() >= self.config.trigger_threshold
    }

    // ── Driver hooks (not part of the polling API) ──

    pub fn set_button(&mut self, button: GamepadButton, down: bool) {
        if down {
            self.buttons_held.insert(button);
        } else {
            self.buttons_held.remove(&button);
        }
    }

    pub fn set_axis(&mut self, axis: GamepadAxis, value: f32) {
        self.axes.insert(axis, value);
    }

    pub fn set_config(&mut self, config: GamepadConfig) {
        self.config = config;
    }

    pub fn config(&self) -> &GamepadConfig {
        &self.config
    }
}

/// Apply a deadzone to a 2D stick vector, returning the rescaled output.
///
/// Values inside the deadzone return `Vec2::ZERO`. Values outside are
/// rescaled so the deadzone edge maps to 0.0 and the stick extreme
/// maps to 1.0.
pub fn apply_deadzone(raw: Vec2, deadzone: f32, shape: crate::device::DeadzoneShape) -> Vec2 {
    use crate::device::DeadzoneShape;
    match shape {
        DeadzoneShape::Circular => {
            let mag = raw.length();
            if mag < deadzone {
                Vec2::ZERO
            } else {
                let rescaled = (mag - deadzone) / (1.0 - deadzone);
                raw.normalize_or_zero() * rescaled
            }
        }
        DeadzoneShape::PerAxis => Vec2::new(
            apply_single_axis_deadzone(raw.x, deadzone),
            apply_single_axis_deadzone(raw.y, deadzone),
        ),
    }
}

fn apply_single_axis_deadzone(value: f32, deadzone: f32) -> f32 {
    if value.abs() < deadzone {
        0.0
    } else {
        let sign = value.signum();
        let magnitude = (value.abs() - deadzone) / (1.0 - deadzone);
        sign * magnitude.clamp(0.0, 1.0)
    }
}

// ───────────────────────────────────────────────────────────────────
// Input State (polled snapshot)
// ───────────────────────────────────────────────────────────────────

/// Per-frame input snapshot. Held-state plus a small set of "just
/// happened this frame" edge fields reset by the device crate after
/// `App::update`.
///
/// Keyboard and mouse state is always present. Gamepad state is
/// keyed by player index (0-based); a missing entry means no
/// gamepad connected for that slot.
#[derive(Clone, Debug)]
pub struct InputState {
    // ── Mouse ──
    /// Mouse position in pixels, origin top-left, matching the
    /// renderer's coordinate system. `(0, 0)` until the first cursor
    /// event.
    pub mouse_position: Vec2,
    pub mouse_left: bool,
    pub mouse_right: bool,
    pub mouse_middle: bool,
    pub mouse_back: bool,
    pub mouse_forward: bool,
    /// `true` only on the first frame after the left button transitions
    /// up→down. Driver-set, reset after each `App::update`.
    pub mouse_left_pressed: bool,
    /// Accumulated scroll delta since the previous frame consumed it.
    /// Positive = scroll up (wheel toward the user / two-finger swipe
    /// up). The driver resets this to `0.0` immediately after
    /// `App::update` returns.
    pub mouse_wheel_delta: f32,

    // ── Keyboard ──
    keys_held: HashSet<Key>,
    /// OS-committed text for this frame (post-IME / post-layout, from winit
    /// `KeyEvent.text`), for a focused text field to append. Empty except on
    /// frames with text entry. Driver-set via [`push_typed`](Self::push_typed);
    /// reset after each `App::update`, like [`mouse_left_pressed`].
    typed_text: String,
    /// `true` only on the frame Backspace transitions up→down. Edge (no
    /// auto-repeat yet). Driver-set; reset after each `App::update`.
    backspace_edge: bool,

    // ── Gamepad ──
    gamepads: HashMap<usize, GamepadState>,

    // ── Analog latch (device §6.3) ──
    /// Immutable per-frame copy of the 120 Hz analog cache, latched when the
    /// device crate builds the frame, so frame-critical semi-chords (e.g. dodge
    /// direction) read a coherent, wake-jitter-immune sample. `None` until a
    /// device fills it; a placeholder this slice (spec §3.4 / §6.3).
    analog_latch: Option<AnalogFrame>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mouse_position: Vec2::ZERO,
            mouse_left: false,
            mouse_right: false,
            mouse_middle: false,
            mouse_back: false,
            mouse_forward: false,
            mouse_left_pressed: false,
            mouse_wheel_delta: 0.0,
            keys_held: HashSet::new(),
            typed_text: String::new(),
            backspace_edge: false,
            gamepads: HashMap::new(),
            analog_latch: None,
        }
    }
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Keyboard queries ──

    /// Is this key currently held down?
    pub fn key_down(&self, key: Key) -> bool {
        self.keys_held.contains(&key)
    }

    /// OS-committed text entered this frame (post-IME), for a focused text
    /// field to append. Empty on frames with no text entry.
    pub fn typed(&self) -> &str {
        &self.typed_text
    }

    /// Did Backspace transition up→down this frame? An edge, reset each frame.
    pub fn backspace(&self) -> bool {
        self.backspace_edge
    }

    /// Is any input (keyboard, mouse, or gamepad) bound to `action`
    /// currently active?
    ///
    /// (The newer, deadzone/threshold-aware equivalent is
    /// [`InputBinding::is_down`](crate::InputBinding::is_down) /
    /// [`ContextualBindings::signal_held`](crate::ContextualBindings::signal_held);
    /// this is kept while consumers migrate off it.)
    pub fn input_active(&self, map: &InputMap, action: ActionSignal) -> bool {
        for binding in map.bindings_for(action) {
            match binding {
                InputBinding::Key(k) => {
                    if self.key_down(*k) {
                        return true;
                    }
                }
                InputBinding::MouseButton(mb) => {
                    if self.mouse_button_down(*mb) {
                        return true;
                    }
                }
                InputBinding::GamepadButton(gb) => {
                    if self.any_gamepad_button_down(*gb) {
                        return true;
                    }
                }
                InputBinding::GamepadAxis { axis, direction } => {
                    if self.any_gamepad_axis_active(*axis, *direction) {
                        return true;
                    }
                }
            }
        }
        false
    }

    // ── Mouse queries ──

    /// Is this mouse button currently held?
    pub fn mouse_button_down(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.mouse_left,
            MouseButton::Right => self.mouse_right,
            MouseButton::Middle => self.mouse_middle,
            MouseButton::Back => self.mouse_back,
            MouseButton::Forward => self.mouse_forward,
        }
    }

    // ── Gamepad queries ──

    /// Get the gamepad state for a specific player index.
    pub fn gamepad(&self, player: usize) -> Option<&GamepadState> {
        self.gamepads.get(&player)
    }

    /// Is a gamepad connected for the given player index?
    pub fn gamepad_connected(&self, player: usize) -> bool {
        self.gamepads.contains_key(&player)
    }

    /// Iterator over (player_index, gamepad_state) for all connected
    /// gamepads.
    pub fn gamepads(&self) -> impl Iterator<Item = (usize, &GamepadState)> {
        self.gamepads.iter().map(|(&k, v)| (k, v))
    }

    /// Is the given button held on any connected gamepad?
    fn any_gamepad_button_down(&self, button: GamepadButton) -> bool {
        self.gamepads.values().any(|gp| gp.button_down(button))
    }

    /// Is the given axis past threshold on any connected gamepad?
    ///
    /// Stick axes use the per-gamepad deadzone; trigger axes use the
    /// per-gamepad `trigger_threshold`.
    fn any_gamepad_axis_active(&self, axis: GamepadAxis, direction: AxisDirection) -> bool {
        self.gamepads.values().any(|gp| {
            let val = gp.axis_value(axis);
            let threshold = match axis {
                GamepadAxis::LeftStickX | GamepadAxis::LeftStickY => {
                    gp.config().left_stick_deadzone
                }
                GamepadAxis::RightStickX | GamepadAxis::RightStickY => {
                    gp.config().right_stick_deadzone
                }
                GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger => {
                    gp.config().trigger_threshold
                }
            };
            match direction {
                AxisDirection::Positive => val > threshold,
                AxisDirection::Negative => val < -threshold,
            }
        })
    }

    // ── Analog latch ──

    /// The latched analog sample for this frame, if a device filled it.
    pub fn analog_latch(&self) -> Option<AnalogFrame> {
        self.analog_latch
    }

    /// Latch the frame's analog sample (device hook; spec §6.3).
    pub fn set_analog_latch(&mut self, frame: AnalogFrame) {
        self.analog_latch = Some(frame);
    }

    // ── Driver hooks (not part of the polling API) ──

    /// Update keyboard held-state.
    pub fn set_key(&mut self, key: Key, down: bool) {
        if down {
            self.keys_held.insert(key);
        } else {
            self.keys_held.remove(&key);
        }
    }

    /// Append OS-committed text for this frame (driver hook; from winit
    /// `KeyEvent.text`). Callers should strip control characters first.
    pub fn push_typed(&mut self, text: &str) {
        self.typed_text.push_str(text);
    }

    /// Flag a Backspace edge for this frame (driver hook).
    pub fn flag_backspace(&mut self) {
        self.backspace_edge = true;
    }

    /// Clear this frame's text-entry edges. The driver calls this after
    /// `App::update`, alongside the mouse-edge resets.
    pub fn clear_frame_text(&mut self) {
        self.typed_text.clear();
        self.backspace_edge = false;
    }

    /// Update a mouse button's held state.
    pub fn set_mouse_button(&mut self, button: MouseButton, down: bool) {
        match button {
            MouseButton::Left => self.mouse_left = down,
            MouseButton::Right => self.mouse_right = down,
            MouseButton::Middle => self.mouse_middle = down,
            MouseButton::Back => self.mouse_back = down,
            MouseButton::Forward => self.mouse_forward = down,
        }
    }

    /// Get or create the gamepad state for a player index.
    pub fn gamepad_mut(&mut self, player: usize) -> &mut GamepadState {
        self.gamepads
            .entry(player)
            .or_insert_with(|| GamepadState::new(GamepadConfig::default()))
    }

    /// Remove a gamepad (disconnected).
    pub fn remove_gamepad(&mut self, player: usize) {
        self.gamepads.remove(&player);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::InputBinding;
    use crate::device::DeadzoneShape;

    #[test]
    fn key_down_basic() {
        let mut input = InputState::new();
        assert!(!input.key_down(Key::W));
        input.set_key(Key::W, true);
        assert!(input.key_down(Key::W));
        input.set_key(Key::W, false);
        assert!(!input.key_down(Key::W));
    }

    #[test]
    fn mouse_button_roundtrip() {
        let mut input = InputState::new();
        assert!(!input.mouse_button_down(MouseButton::Left));
        input.set_mouse_button(MouseButton::Left, true);
        assert!(input.mouse_button_down(MouseButton::Left));
        assert!(!input.mouse_button_down(MouseButton::Right));
    }

    #[test]
    fn gamepad_connect_disconnect() {
        let mut input = InputState::new();
        assert!(!input.gamepad_connected(0));
        input.gamepad_mut(0);
        assert!(input.gamepad_connected(0));
        input.remove_gamepad(0);
        assert!(!input.gamepad_connected(0));
    }

    #[test]
    fn gamepad_button_and_axis() {
        let mut input = InputState::new();
        input.gamepad_mut(0).set_button(GamepadButton::South, true);
        input.gamepad_mut(0).set_axis(GamepadAxis::LeftStickX, 0.8);
        let gp = input.gamepad(0).unwrap();
        assert!(gp.button_down(GamepadButton::South));
        assert!(!gp.button_down(GamepadButton::East));
        assert!((gp.axis_value(GamepadAxis::LeftStickX) - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn circular_deadzone_clips_small_values() {
        let raw = Vec2::new(0.05, 0.05);
        let result = apply_deadzone(raw, 0.15, DeadzoneShape::Circular);
        assert_eq!(result, Vec2::ZERO);
    }

    #[test]
    fn circular_deadzone_rescales_large_values() {
        let raw = Vec2::new(1.0, 0.0);
        let result = apply_deadzone(raw, 0.15, DeadzoneShape::Circular);
        // After rescaling: (1.0 - 0.15) / (1.0 - 0.15) = 1.0
        assert!((result.x - 1.0).abs() < 0.01);
    }

    #[test]
    fn per_axis_deadzone_independent() {
        let raw = Vec2::new(0.05, 0.8);
        let result = apply_deadzone(raw, 0.15, DeadzoneShape::PerAxis);
        assert_eq!(result.x, 0.0);
        assert!(result.y > 0.0);
    }

    #[test]
    fn gamepad_stick_applies_deadzone() {
        let mut input = InputState::new();
        // Small stick deflection inside deadzone
        input.gamepad_mut(0).set_axis(GamepadAxis::LeftStickX, 0.05);
        input.gamepad_mut(0).set_axis(GamepadAxis::LeftStickY, 0.05);
        let gp = input.gamepad(0).unwrap();
        let stick = gp.left_stick();
        assert_eq!(stick, Vec2::ZERO);
    }

    #[test]
    fn trigger_threshold() {
        let mut input = InputState::new();
        input.gamepad_mut(0).set_axis(GamepadAxis::LeftTrigger, 0.3);
        let gp = input.gamepad(0).unwrap();
        assert!(!gp.left_trigger_down());
        input.gamepad_mut(0).set_axis(GamepadAxis::LeftTrigger, 0.7);
        let gp = input.gamepad(0).unwrap();
        assert!(gp.left_trigger_down());
    }

    #[test]
    fn input_active_checks_all_devices() {
        let mut map = InputMap::empty();
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
        map.bind(
            ActionSignal::MoveForward,
            InputBinding::GamepadButton(GamepadButton::LeftStick),
        );

        // Keyboard only
        let mut input = InputState::new();
        input.set_key(Key::W, true);
        assert!(input.input_active(&map, ActionSignal::MoveForward));

        // Gamepad only
        let mut input = InputState::new();
        input.gamepad_mut(0).set_button(GamepadButton::LeftStick, true);
        assert!(input.input_active(&map, ActionSignal::MoveForward));

        // Neither
        let input = InputState::new();
        assert!(!input.input_active(&map, ActionSignal::MoveForward));
    }

    #[test]
    fn analog_latch_defaults_none_and_sets() {
        let mut input = InputState::new();
        assert!(input.analog_latch().is_none());
        input.set_analog_latch(AnalogFrame::neutral(std::time::Instant::now()));
        assert!(input.analog_latch().is_some());
    }
}
