//! macOS gamepad input via Apple's GameController framework (GCController).
//!
//! Polled once per frame from the runner — no callbacks and no run-loop dependency on our
//! side: the framework maintains the input snapshot that `isPressed()` / `value()` read,
//! so a per-frame poll of `GCController::controllers()` is enough. Controllers are keyed to
//! player indices by their position in that array.

use flicker_core::input::{GamepadAxis, GamepadButton, GamepadState, InputState};
use objc2_game_controller::{GCController, GCExtendedGamepad};

/// Reads connected controllers via the GameController framework each frame.
#[derive(Default)]
pub struct GamepadReader {
    /// Controllers mapped last poll, so slots can be freed when a pad goes away.
    prev_count: usize,
}

impl GamepadReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn poll(&mut self, input: &mut InputState) {
        // SAFETY: these getters are snapshot reads maintained by the framework; we only
        // call them on the main thread, from the runner's frame.
        let controllers = unsafe { GCController::controllers() };
        let count = controllers.count();

        for i in 0..count {
            let controller = controllers.objectAtIndex(i);
            // Only standard ("extended") gamepads are handled — the layout the whole engine
            // assumes. A controller without one (e.g. a bare remote) is skipped.
            if let Some(pad) = unsafe { controller.extendedGamepad() } {
                apply(&pad, input.gamepad_mut(i));
            }
        }

        // Free slots for controllers that disconnected since the last poll.
        for player in count..self.prev_count {
            input.remove_gamepad(player);
        }
        if count != self.prev_count {
            tracing::info!("GameController: {count} controller(s) connected");
        }
        self.prev_count = count;
    }
}

/// Copy one controller's live GameController-framework state into a gamepad slot.
fn apply(pad: &GCExtendedGamepad, gp: &mut GamepadState) {
    // SAFETY: every call is a snapshot read on the GameController profile; the `Retained`
    // temporaries are released at the end of each statement.
    unsafe {
        // Face buttons (A=South, B=East, X=West, Y=North).
        gp.set_button(GamepadButton::South, pad.buttonA().isPressed());
        gp.set_button(GamepadButton::East, pad.buttonB().isPressed());
        gp.set_button(GamepadButton::West, pad.buttonX().isPressed());
        gp.set_button(GamepadButton::North, pad.buttonY().isPressed());

        // Shoulders + triggers (triggers are also analog axes below).
        gp.set_button(GamepadButton::LeftBumper, pad.leftShoulder().isPressed());
        gp.set_button(GamepadButton::RightBumper, pad.rightShoulder().isPressed());
        gp.set_button(GamepadButton::LeftTrigger, pad.leftTrigger().isPressed());
        gp.set_button(GamepadButton::RightTrigger, pad.rightTrigger().isPressed());

        // Meta buttons (Options/Home are optional on some pads).
        gp.set_button(GamepadButton::Start, pad.buttonMenu().isPressed());
        if let Some(b) = pad.buttonOptions() {
            gp.set_button(GamepadButton::Select, b.isPressed());
        }
        if let Some(b) = pad.buttonHome() {
            gp.set_button(GamepadButton::Guide, b.isPressed());
        }

        // Stick presses (L3 / R3).
        if let Some(b) = pad.leftThumbstickButton() {
            gp.set_button(GamepadButton::LeftStick, b.isPressed());
        }
        if let Some(b) = pad.rightThumbstickButton() {
            gp.set_button(GamepadButton::RightStick, b.isPressed());
        }

        // D-pad.
        let dpad = pad.dpad();
        gp.set_button(GamepadButton::DPadUp, dpad.up().isPressed());
        gp.set_button(GamepadButton::DPadDown, dpad.down().isPressed());
        gp.set_button(GamepadButton::DPadLeft, dpad.left().isPressed());
        gp.set_button(GamepadButton::DPadRight, dpad.right().isPressed());

        // Sticks + trigger axes (GameController Y is +up, matching our convention).
        let ls = pad.leftThumbstick();
        gp.set_axis(GamepadAxis::LeftStickX, ls.xAxis().value());
        gp.set_axis(GamepadAxis::LeftStickY, ls.yAxis().value());
        let rs = pad.rightThumbstick();
        gp.set_axis(GamepadAxis::RightStickX, rs.xAxis().value());
        gp.set_axis(GamepadAxis::RightStickY, rs.yAxis().value());
        gp.set_axis(GamepadAxis::LeftTrigger, pad.leftTrigger().value());
        gp.set_axis(GamepadAxis::RightTrigger, pad.rightTrigger().value());
    }
}
