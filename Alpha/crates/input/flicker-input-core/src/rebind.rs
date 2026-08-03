//! Rebind capture: watch the input snapshot for a fresh key/button/axis press
//! and turn it into an [`InputBinding`].
//!
//! Relocated from `flicker-core::input::settings_gui` (spec R6 / §3.4): the
//! capture logic is pure and needs the `ALL_*` enumerations, so it belongs in
//! core beside the device symbols. [`RebindCapture`] is the lightweight,
//! standalone driver the Lua settings screen uses; [`capture_input`] is the
//! shared edge-detector (previously the `InputSettingsPanel::capture_input`
//! method, which the panel now calls through here).

use std::collections::{HashMap, HashSet};

use crate::binding::{InputBinding, InputMap};
use crate::device::{AxisDirection, GamepadAxis, GamepadButton, Key, MouseButton};
use crate::signal::ActionSignal;
use crate::snapshot::InputState;

// ───────────────────────────────────────────────────────────────────
// Input lists for rebind capture
// ───────────────────────────────────────────────────────────────────

pub const ALL_GAMEPAD_BUTTONS: [GamepadButton; 21] = [
    GamepadButton::South,
    GamepadButton::East,
    GamepadButton::North,
    GamepadButton::West,
    GamepadButton::LeftBumper,
    GamepadButton::RightBumper,
    GamepadButton::LeftTrigger,
    GamepadButton::RightTrigger,
    GamepadButton::Select,
    GamepadButton::Start,
    GamepadButton::Guide,
    GamepadButton::Mode,
    GamepadButton::LeftStick,
    GamepadButton::RightStick,
    GamepadButton::DPadUp,
    GamepadButton::DPadDown,
    GamepadButton::DPadLeft,
    GamepadButton::DPadRight,
    GamepadButton::Touchpad,
    GamepadButton::C,
    GamepadButton::Z,
];

pub const ALL_GAMEPAD_AXES: [GamepadAxis; 6] = [
    GamepadAxis::LeftStickX,
    GamepadAxis::LeftStickY,
    GamepadAxis::RightStickX,
    GamepadAxis::RightStickY,
    GamepadAxis::LeftTrigger,
    GamepadAxis::RightTrigger,
];

pub const ALL_KEYS: [Key; 103] = [
    Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H,
    Key::I, Key::J, Key::K, Key::L, Key::M, Key::N, Key::O, Key::P,
    Key::Q, Key::R, Key::S, Key::T, Key::U, Key::V, Key::W, Key::X,
    Key::Y, Key::Z,
    Key::Digit0, Key::Digit1, Key::Digit2, Key::Digit3, Key::Digit4,
    Key::Digit5, Key::Digit6, Key::Digit7, Key::Digit8, Key::Digit9,
    Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
    Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
    Key::Up, Key::Down, Key::Left, Key::Right,
    Key::LeftShift, Key::RightShift, Key::LeftControl, Key::RightControl,
    Key::LeftAlt, Key::RightAlt, Key::LeftSuper, Key::RightSuper,
    Key::Space, Key::Enter, Key::Escape, Key::Tab, Key::Backspace,
    Key::Delete, Key::Insert, Key::Home, Key::End, Key::PageUp, Key::PageDown,
    Key::PrintScreen, Key::ScrollLock, Key::Pause,
    Key::Minus, Key::Equal, Key::LeftBracket, Key::RightBracket,
    Key::Backslash, Key::Semicolon, Key::Apostrophe,
    Key::Comma, Key::Period, Key::Slash, Key::Grave,
    Key::Numpad0, Key::Numpad1, Key::Numpad2, Key::Numpad3, Key::Numpad4,
    Key::Numpad5, Key::Numpad6, Key::Numpad7, Key::Numpad8, Key::Numpad9,
    Key::NumpadAdd, Key::NumpadSubtract, Key::NumpadMultiply, Key::NumpadDivide,
    Key::NumpadDecimal, Key::NumpadEnter, Key::NumpadEqual, Key::NumLock,
];

// ───────────────────────────────────────────────────────────────────
// Shared edge-detecting capture
// ───────────────────────────────────────────────────────────────────

/// Scan the input snapshot for the first freshly-pressed key/mouse/gamepad
/// control (edge vs the supplied previous-frame state) and return it as an
/// [`InputBinding`], or `None` while still waiting. Previously
/// `InputSettingsPanel::capture_input`.
#[allow(clippy::too_many_arguments)] // faithful relocation of the panel's edge-capture signature
pub fn capture_input(
    input: &InputState,
    prev_keys: &HashSet<Key>,
    prev_mouse_left: &bool,
    prev_mouse_right: &bool,
    prev_mouse_middle: &bool,
    prev_mouse_back: &bool,
    prev_mouse_forward: &bool,
    prev_gp_buttons: &HashSet<GamepadButton>,
    prev_gp_axes: &HashMap<GamepadAxis, f32>,
    for_gamepad: bool,
) -> Option<InputBinding> {
    if for_gamepad {
        if let Some(gp) = input.gamepad(0) {
            for &btn in &ALL_GAMEPAD_BUTTONS {
                if gp.button_down(btn) && !prev_gp_buttons.contains(&btn) {
                    return Some(InputBinding::GamepadButton(btn));
                }
            }
            for &axis in &ALL_GAMEPAD_AXES {
                let val = gp.axis_value(axis);
                let prev = prev_gp_axes.get(&axis).copied().unwrap_or(0.0);
                if prev <= 0.7 && val > 0.7 {
                    return Some(InputBinding::GamepadAxis {
                        axis,
                        direction: AxisDirection::Positive,
                    });
                } else if prev >= -0.7 && val < -0.7 {
                    return Some(InputBinding::GamepadAxis {
                        axis,
                        direction: AxisDirection::Negative,
                    });
                }
            }
        }
    } else {
        for &key in &ALL_KEYS {
            if input.key_down(key) && !prev_keys.contains(&key) {
                return Some(InputBinding::Key(key));
            }
        }
        if input.mouse_left && !prev_mouse_left {
            return Some(InputBinding::MouseButton(MouseButton::Left));
        }
        if input.mouse_right && !prev_mouse_right {
            return Some(InputBinding::MouseButton(MouseButton::Right));
        }
        if input.mouse_middle && !prev_mouse_middle {
            return Some(InputBinding::MouseButton(MouseButton::Middle));
        }
        if input.mouse_back && !prev_mouse_back {
            return Some(InputBinding::MouseButton(MouseButton::Back));
        }
        if input.mouse_forward && !prev_mouse_forward {
            return Some(InputBinding::MouseButton(MouseButton::Forward));
        }
    }
    None
}

// ───────────────────────────────────────────────────────────────────
// Standalone rebind capture (usable from Lua-driven settings)
// ───────────────────────────────────────────────────────────────────

/// Lightweight rebind capture state, usable independently from any settings
/// panel. The Lua settings screen drives this via the Rust host to perform
/// key/gamepad rebinding.
pub struct RebindCapture {
    /// The signal being rebound, if any.
    action: Option<ActionSignal>,
    /// Whether this rebind targets a gamepad binding.
    for_gamepad: bool,
    /// Previous-frame input snapshots for edge detection.
    prev_keys: HashSet<Key>,
    prev_mouse_left: bool,
    prev_mouse_right: bool,
    prev_mouse_middle: bool,
    prev_mouse_back: bool,
    prev_mouse_forward: bool,
    prev_gamepad_buttons: HashSet<GamepadButton>,
    prev_gamepad_axes: HashMap<GamepadAxis, f32>,
}

impl RebindCapture {
    pub fn new() -> Self {
        Self {
            action: None,
            for_gamepad: false,
            prev_keys: HashSet::new(),
            prev_mouse_left: false,
            prev_mouse_right: false,
            prev_mouse_middle: false,
            prev_mouse_back: false,
            prev_mouse_forward: false,
            prev_gamepad_buttons: HashSet::new(),
            prev_gamepad_axes: HashMap::new(),
        }
    }

    /// Start rebinding `action`. Set `for_gamepad` to capture gamepad input.
    pub fn start(&mut self, action: ActionSignal, for_gamepad: bool) {
        self.action = Some(action);
        self.for_gamepad = for_gamepad;
    }

    /// Cancel the current rebind.
    pub fn cancel(&mut self) {
        self.action = None;
    }

    /// Whether a rebind is in progress.
    pub fn is_active(&self) -> bool {
        self.action.is_some()
    }

    /// The signal being rebound, if any.
    pub fn current_action(&self) -> Option<ActionSignal> {
        self.action
    }

    /// Whether the current rebind targets gamepad.
    pub fn is_gamepad(&self) -> bool {
        self.for_gamepad
    }

    /// Poll for a captured input. Returns `Some((action, binding))` when the
    /// user presses a button/key, or `None` if still waiting. Resolves
    /// conflicts by unbinding the input from any other signal first.
    pub fn poll(
        &mut self,
        input: &InputState,
        input_map: &mut InputMap,
    ) -> Option<(ActionSignal, InputBinding)> {
        let action = self.action?;

        let captured = capture_input(
            input,
            &self.prev_keys,
            &self.prev_mouse_left,
            &self.prev_mouse_right,
            &self.prev_mouse_middle,
            &self.prev_mouse_back,
            &self.prev_mouse_forward,
            &self.prev_gamepad_buttons,
            &self.prev_gamepad_axes,
            self.for_gamepad,
        );

        self.update_prev(input);

        let binding = captured?;

        // Conflict detection: unbind from other signals
        if let Some(conflict_action) = input_map.action_for(binding) {
            if conflict_action != action {
                input_map.unbind(conflict_action, binding);
            }
        }

        // Remove old binding at slot 0 if it exists
        let old_bindings: Vec<InputBinding> = input_map.bindings_for(action).to_vec();
        if !old_bindings.is_empty() {
            input_map.unbind(action, old_bindings[0]);
        }

        input_map.bind(action, binding);
        self.action = None;

        Some((action, binding))
    }

    fn update_prev(&mut self, input: &InputState) {
        self.prev_keys.clear();
        for &key in &ALL_KEYS {
            if input.key_down(key) {
                self.prev_keys.insert(key);
            }
        }
        self.prev_mouse_left = input.mouse_left;
        self.prev_mouse_right = input.mouse_right;
        self.prev_mouse_middle = input.mouse_middle;
        self.prev_mouse_back = input.mouse_back;
        self.prev_mouse_forward = input.mouse_forward;
        self.prev_gamepad_buttons.clear();
        self.prev_gamepad_axes.clear();
        if let Some(gp) = input.gamepad(0) {
            for &btn in &ALL_GAMEPAD_BUTTONS {
                if gp.button_down(btn) {
                    self.prev_gamepad_buttons.insert(btn);
                }
            }
            for &axis in &ALL_GAMEPAD_AXES {
                self.prev_gamepad_axes.insert(axis, gp.axis_value(axis));
            }
        }
    }
}

impl Default for RebindCapture {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_fresh_key_press_as_binding() {
        let mut rc = RebindCapture::new();
        let mut map = InputMap::empty();
        rc.start(ActionSignal::Jump, false);
        assert!(rc.is_active());
        assert_eq!(rc.current_action(), Some(ActionSignal::Jump));

        // A down key with no prior-frame state is a fresh edge → captured on the first
        // poll (the pre-move behavior). No capture until a key is actually down.
        assert_eq!(rc.poll(&InputState::new(), &mut map), None);
        let mut down = InputState::new();
        down.set_key(Key::J, true);
        let got = rc.poll(&down, &mut map);
        assert_eq!(got, Some((ActionSignal::Jump, InputBinding::Key(Key::J))));
        assert_eq!(map.action_for(InputBinding::Key(Key::J)), Some(ActionSignal::Jump));
        assert!(!rc.is_active());
    }

    #[test]
    fn all_lists_are_the_expected_size() {
        assert_eq!(ALL_GAMEPAD_BUTTONS.len(), 21);
        assert_eq!(ALL_GAMEPAD_AXES.len(), 6);
        assert_eq!(ALL_KEYS.len(), 103);
    }
}
