//! Engine-side input snapshot + semantic bindings layer.
//!
//! [`InputState`] is a per-frame, polled snapshot — the same model
//! DXTK and XNA use. The driver (typically `flicker-app`) accumulates
//! platform events into the snapshot; game code reads it from
//! `App::update`.
//!
//! Game code should usually NOT read raw [`Key`] values. Instead, hold
//! an [`InputMap`] and query semantic [`bindings::Action`]s via
//! [`InputState::action_active`]. That keeps the physical-key→action
//! mapping rebindable without touching consumer code. See the
//! [`bindings`] module docs for the design.
//!
//! The types here are intentionally platform-agnostic — `flicker-core`
//! does not depend on `winit` or `gilrs`. Translation from platform
//! events happens in `flicker-app`.
//!
//! # Sections
//!
//! The input system is organized into vertical sections:
//!
//! - **Keyboard** — [`Key`] enum covering all standard keys, polled via
//!   [`InputState::key_down`].
//! - **Mouse** — [`MouseButton`] enum, position/buttons on
//!   [`InputState`], scroll delta.
//! - **Gamepad** — [`GamepadButton`], [`GamepadAxis`], per-gamepad
//!   [`GamepadState`] held in [`InputState`].
//! - **Abstract Controls** — [`bindings::AbstractControls`] for
//!   per-device sensitivity, inversion, and deadzone tuning.

pub mod bindings;

use std::collections::{HashMap, HashSet};

use glam::Vec2;
use serde::{Deserialize, Serialize};

use self::bindings::{Action, InputMap};

// ───────────────────────────────────────────────────────────────────
// Section: Keyboard
// ───────────────────────────────────────────────────────────────────

/// Symbolic key identifier covering the full standard keyboard.
///
/// Variants map 1:1 from winit `KeyCode` values in `flicker-app`.
/// Add new variants here and in the driver's `translate_key()` as
/// games need them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Key {
    // ── Letters ──
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,

    // ── Digits ──
    Digit0, Digit1, Digit2, Digit3, Digit4,
    Digit5, Digit6, Digit7, Digit8, Digit9,

    // ── Function keys ──
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,

    // ── Arrow keys ──
    Up, Down, Left, Right,

    // ── Modifiers ──
    LeftShift, RightShift,
    LeftControl, RightControl,
    LeftAlt, RightAlt,
    LeftSuper, RightSuper,

    // ── Navigation / editing ──
    Home, End, PageUp, PageDown,
    Insert, Delete,
    Backspace, Tab, Enter, Escape, Space,
    PrintScreen, ScrollLock, Pause,

    // ── Punctuation ──
    Minus, Equal,
    LeftBracket, RightBracket,
    Backslash,
    Semicolon, Apostrophe,
    Comma, Period, Slash,
    Grave,

    // ── Numpad ──
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4,
    Numpad5, Numpad6, Numpad7, Numpad8, Numpad9,
    NumpadAdd, NumpadSubtract, NumpadMultiply, NumpadDivide,
    NumpadDecimal, NumpadEnter, NumpadEqual,
    NumLock,
}

// ───────────────────────────────────────────────────────────────────
// Section: Mouse
// ───────────────────────────────────────────────────────────────────

/// Mouse button identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

// ───────────────────────────────────────────────────────────────────
// Section: Gamepad
// ───────────────────────────────────────────────────────────────────

/// Gamepad button identifier (Xbox-style naming).
///
/// Maps from gilrs `Button` values in `flicker-app`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadButton {
    // ── Face buttons (Xbox layout names) ──
    South,   // A / Cross
    East,    // B / Circle
    North,   // X / Square
    West,    // Y / Triangle

    // ── Shoulder / trigger ──
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,

    // ── Meta ──
    Select,  // Back / Share
    Start,   // Menu / Options
    Guide,   // Xbox / PS button
    Mode,    // Platform-specific home

    // ── Stick clicks ──
    LeftStick,
    RightStick,

    // ── D-Pad ──
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,

    // ── Misc (Switch-style, etc.) ──
    Touchpad,
    C,
    Z,
}

/// Gamepad axis identifier.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

/// Deadzone application strategy for analog sticks.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeadzoneShape {
    /// Circular deadzone — magnitude-based. Eliminates diagonal
    /// stick drift better than per-axis.
    Circular,
    /// Per-axis deadzone — each axis is clamped independently.
    /// Simpler, but allows slight diagonal drift.
    PerAxis,
}

/// Per-gamepad configuration (deadzones, thresholds).
#[derive(Clone, Debug)]
pub struct GamepadConfig {
    /// Deadzone for the left analog stick (0.0–1.0).
    pub left_stick_deadzone: f32,
    /// Deadzone for the right analog stick (0.0–1.0).
    pub right_stick_deadzone: f32,
    /// Deadzone shape applied to sticks.
    pub deadzone_shape: DeadzoneShape,
    /// Threshold above which a trigger is considered "pressed" (0.0–1.0).
    pub trigger_threshold: f32,
}

impl Default for GamepadConfig {
    fn default() -> Self {
        Self {
            left_stick_deadzone: 0.15,
            right_stick_deadzone: 0.15,
            deadzone_shape: DeadzoneShape::Circular,
            trigger_threshold: 0.5,
        }
    }
}

/// Snapshot state for a single connected gamepad.
///
/// The driver updates this each frame from gilrs events. Game code
/// polls it via [`InputState::gamepad()`].
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
fn apply_deadzone(raw: Vec2, deadzone: f32, shape: DeadzoneShape) -> Vec2 {
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
// Section: Input State (polled snapshot)
// ───────────────────────────────────────────────────────────────────

/// Per-frame input snapshot. Held-state plus a small set of "just
/// happened this frame" edge fields reset by the driver after
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

    // ── Gamepad ──
    gamepads: HashMap<usize, GamepadState>,
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
            gamepads: HashMap::new(),
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

    /// Is any key bound to `action` currently held?
    ///
    /// Works with the legacy [`bindings::Bindings`] map. For the
    /// newer [`InputMap`], use [`Self::input_active`].
    pub fn action_active(&self, bindings: &bindings::Bindings, action: Action) -> bool {
        self.keys_held
            .iter()
            .any(|k| bindings.action_for(*k) == Some(action))
    }

    /// Is any input (keyboard, mouse, or gamepad) bound to `action`
    /// currently active?
    pub fn input_active(&self, map: &InputMap, action: Action) -> bool {
        for binding in map.bindings_for(action) {
            match binding {
                bindings::InputBinding::Key(k) => {
                    if self.key_down(*k) {
                        return true;
                    }
                }
                bindings::InputBinding::MouseButton(mb) => {
                    if self.mouse_button_down(*mb) {
                        return true;
                    }
                }
                bindings::InputBinding::GamepadButton(gb) => {
                    if self.any_gamepad_button_down(*gb) {
                        return true;
                    }
                }
                bindings::InputBinding::GamepadAxis { axis, direction } => {
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
    fn any_gamepad_axis_active(
        &self,
        axis: GamepadAxis,
        direction: bindings::AxisDirection,
    ) -> bool {
        self.gamepads.values().any(|gp| {
            let val = gp.axis_value(axis);
            match direction {
                bindings::AxisDirection::Positive => val > 0.5,
                bindings::AxisDirection::Negative => val < -0.5,
            }
        })
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
        use bindings::{InputBinding, InputMap};
        let mut map = InputMap::empty();
        map.bind(Action::MoveForward, InputBinding::Key(Key::W));
        map.bind(
            Action::MoveForward,
            InputBinding::GamepadButton(GamepadButton::LeftStick),
        );

        // Keyboard only
        let mut input = InputState::new();
        input.set_key(Key::W, true);
        assert!(input.input_active(&map, Action::MoveForward));

        // Gamepad only
        let mut input = InputState::new();
        input.gamepad_mut(0).set_button(GamepadButton::LeftStick, true);
        assert!(input.input_active(&map, Action::MoveForward));

        // Neither
        let input = InputState::new();
        assert!(!input.input_active(&map, Action::MoveForward));
    }
}
