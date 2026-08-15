//! Rebind capture: watch the input snapshot for a fresh key/button/axis press
//! and turn it into an [`InputBinding`].
//!
//! Relocated from `flicker-core::input::settings_gui` (spec R6 / §3.4): the
//! capture logic is pure and rides the device catalogs (`Key::ALL`,
//! `MouseButton::ALL`, `GamepadButton::ALL`, `GamepadAxis::ALL` — canonical on
//! the enums since S2), so it belongs in core beside the device symbols.
//! [`RebindCapture`] is the lightweight, standalone driver the settings screen
//! uses; [`capture_input`] is the shared edge-detector (previously the
//! settings panel's `capture_input` method, which now lives here).

use std::collections::{HashMap, HashSet};

use crate::binding::{InputBinding, InputMap};
use crate::device::{AxisDirection, GamepadAxis, GamepadButton, Key, MouseButton};
use crate::signal::ActionSignal;
use crate::snapshot::InputState;

// ───────────────────────────────────────────────────────────────────
// Shared edge-detecting capture
// ───────────────────────────────────────────────────────────────────

/// Scan the input snapshot for the first freshly-pressed key/mouse/gamepad
/// control (edge vs the supplied previous-frame state) and return it as an
/// [`InputBinding`], or `None` while still waiting. Iterates the device
/// catalogs, so a control added to an enum is capturable the moment it exists.
pub fn capture_input(
    input: &InputState,
    prev_keys: &HashSet<Key>,
    prev_mouse: &HashSet<MouseButton>,
    prev_gp_buttons: &HashSet<GamepadButton>,
    prev_gp_axes: &HashMap<GamepadAxis, f32>,
    for_gamepad: bool,
) -> Option<InputBinding> {
    if for_gamepad {
        if let Some(gp) = input.gamepad(0) {
            for &btn in GamepadButton::ALL {
                if gp.button_down(btn) && !prev_gp_buttons.contains(&btn) {
                    return Some(InputBinding::GamepadButton(btn));
                }
            }
            for &axis in GamepadAxis::ALL {
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
        for &key in Key::ALL {
            if input.key_down(key) && !prev_keys.contains(&key) {
                return Some(InputBinding::Key(key));
            }
        }
        for &mb in MouseButton::ALL {
            if input.mouse_button_down(mb) && !prev_mouse.contains(&mb) {
                return Some(InputBinding::MouseButton(mb));
            }
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
    prev_mouse: HashSet<MouseButton>,
    prev_gamepad_buttons: HashSet<GamepadButton>,
    prev_gamepad_axes: HashMap<GamepadAxis, f32>,
}

impl RebindCapture {
    pub fn new() -> Self {
        Self {
            action: None,
            for_gamepad: false,
            prev_keys: HashSet::new(),
            prev_mouse: HashSet::new(),
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
            &self.prev_mouse,
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

    /// UNBIND the current action's slot-0 binding and end capture — the sibling of
    /// [`poll`](Self::poll) (which binds slot 0) and the "Backspace to unbind" the settings
    /// rebind banner advertises. Returns `(action, removed binding)`, or `None` if nothing
    /// was being rebound or the action had no binding. Capture always ends. The CALLER
    /// detects the Backspace edge — Backspace is otherwise a bindable key, so it must be
    /// intercepted before `poll` would capture it.
    pub fn unbind_current(&mut self, input_map: &mut InputMap) -> Option<(ActionSignal, InputBinding)> {
        let action = self.action?;
        let removed = input_map.bindings_for(action).first().copied();
        if let Some(binding) = removed {
            input_map.unbind(action, binding);
        }
        self.action = None;
        removed.map(|binding| (action, binding))
    }

    fn update_prev(&mut self, input: &InputState) {
        self.prev_keys.clear();
        for &key in Key::ALL {
            if input.key_down(key) {
                self.prev_keys.insert(key);
            }
        }
        self.prev_mouse.clear();
        for &mb in MouseButton::ALL {
            if input.mouse_button_down(mb) {
                self.prev_mouse.insert(mb);
            }
        }
        self.prev_gamepad_buttons.clear();
        self.prev_gamepad_axes.clear();
        if let Some(gp) = input.gamepad(0) {
            for &btn in GamepadButton::ALL {
                if gp.button_down(btn) {
                    self.prev_gamepad_buttons.insert(btn);
                }
            }
            for &axis in GamepadAxis::ALL {
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

    // The catalog size/uniqueness pins moved to device.rs beside the `ALL`
    // consts (`all_catalogs_are_complete_and_unique`).

    #[test]
    fn captures_fresh_mouse_press_as_binding() {
        // The MouseButton::ALL loop must behave exactly like the old five
        // per-button if-blocks: edge on a held-down button, none on repeat.
        let mut rc = RebindCapture::new();
        let mut map = InputMap::empty();
        rc.start(ActionSignal::Interact, false);
        let mut down = InputState::new();
        down.mouse_middle = true;
        let got = rc.poll(&down, &mut map);
        assert_eq!(
            got,
            Some((
                ActionSignal::Interact,
                InputBinding::MouseButton(MouseButton::Middle)
            ))
        );
        // Held across frames after the capture consumed it: no re-capture.
        rc.start(ActionSignal::Interact, false);
        assert_eq!(rc.poll(&down, &mut map), None);
    }

    #[test]
    fn unbind_current_drops_slot_zero_and_ends_capture() {
        let mut rc = RebindCapture::new();
        let mut map = InputMap::empty();
        map.bind(ActionSignal::Jump, InputBinding::Key(Key::Space));
        rc.start(ActionSignal::Jump, false);
        assert_eq!(
            rc.unbind_current(&mut map),
            Some((ActionSignal::Jump, InputBinding::Key(Key::Space))),
        );
        assert!(map.bindings_for(ActionSignal::Jump).is_empty(), "slot 0 removed");
        assert!(!rc.is_active(), "capture ends after an unbind");
        // Backspace on an already-unbound action: capture still ends, nothing removed.
        rc.start(ActionSignal::Reload, false);
        assert_eq!(rc.unbind_current(&mut map), None);
        assert!(!rc.is_active());
        // No action being rebound → None, no panic.
        assert_eq!(rc.unbind_current(&mut map), None);
    }
}
