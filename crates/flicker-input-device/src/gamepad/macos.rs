//! macOS gamepad reader via Apple's GameController framework (GCController).
//!
//! Read once per [`refresh`](super::GamepadBackend::refresh) — no callbacks and no
//! run-loop dependency on our side: the framework maintains the input snapshot
//! that `isPressed()` / `value()` read, so a poll of `GCController::controllers()`
//! is enough. Single local player: slot 0 = the first controller exposing an
//! extended profile; the rest are ignored (spec §6.4). Moved from
//! `flicker-app/src/gamepad/macos.rs`, refactored to fill a [`PadSnapshot`].

use flicker_input_core::{GamepadAxis, GamepadButton};
use objc2_game_controller::{GCController, GCExtendedGamepad};

use super::{DeviceCaps, PadSnapshot};

/// Reads the active controller via the GameController framework each refresh.
#[derive(Default)]
pub struct PlatformGamepad {
    /// Whether slot 0 was connected last refresh — for connect/disconnect logs.
    was_connected: bool,
}

impl PlatformGamepad {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fill `snap` from slot 0 (the first extended pad). Rebuilds the held-button
    /// set each call — the framework snapshot is authoritative.
    pub fn read_into(&mut self, snap: &mut PadSnapshot) {
        // SAFETY: these getters are snapshot reads maintained by the framework; we
        // only call them from the runner's main thread, once per frame.
        let controllers = unsafe { GCController::controllers() };
        let count = controllers.count();

        let mut connected = false;
        for i in 0..count {
            let controller = controllers.objectAtIndex(i);
            // Only standard ("extended") gamepads are handled — the layout the
            // whole engine assumes. A controller without one (e.g. a bare remote)
            // is skipped.
            if let Some(pad) = unsafe { controller.extendedGamepad() } {
                snap.reset_neutral();
                read_pad(&pad, snap);
                snap.caps = DeviceCaps::all();
                connected = true;
                break; // single player — slot 0 is the first extended pad
            }
        }

        if !connected {
            snap.reset_neutral();
            snap.caps = DeviceCaps::default();
        }
        snap.connected = connected;

        if connected != self.was_connected {
            tracing::info!(
                "GameController: slot-0 pad {}",
                if connected { "connected" } else { "disconnected" }
            );
        }
        self.was_connected = connected;
    }
}

/// Copy one controller's live GameController-framework state into the snapshot.
fn read_pad(pad: &GCExtendedGamepad, snap: &mut PadSnapshot) {
    // SAFETY: every call is a snapshot read on the GameController profile; the
    // `Retained` temporaries are released at the end of each statement.
    unsafe {
        // Face buttons (A=South, B=East, X=West, Y=North).
        set_button(snap, GamepadButton::South, pad.buttonA().isPressed());
        set_button(snap, GamepadButton::East, pad.buttonB().isPressed());
        set_button(snap, GamepadButton::West, pad.buttonX().isPressed());
        set_button(snap, GamepadButton::North, pad.buttonY().isPressed());

        // Shoulders + triggers (triggers are also analog axes below).
        set_button(snap, GamepadButton::LeftBumper, pad.leftShoulder().isPressed());
        set_button(snap, GamepadButton::RightBumper, pad.rightShoulder().isPressed());
        set_button(snap, GamepadButton::LeftTrigger, pad.leftTrigger().isPressed());
        set_button(snap, GamepadButton::RightTrigger, pad.rightTrigger().isPressed());

        // Meta buttons (Options/Home are optional on some pads).
        set_button(snap, GamepadButton::Start, pad.buttonMenu().isPressed());
        if let Some(b) = pad.buttonOptions() {
            set_button(snap, GamepadButton::Select, b.isPressed());
        }
        if let Some(b) = pad.buttonHome() {
            set_button(snap, GamepadButton::Guide, b.isPressed());
        }

        // Stick presses (L3 / R3).
        if let Some(b) = pad.leftThumbstickButton() {
            set_button(snap, GamepadButton::LeftStick, b.isPressed());
        }
        if let Some(b) = pad.rightThumbstickButton() {
            set_button(snap, GamepadButton::RightStick, b.isPressed());
        }

        // D-pad.
        let dpad = pad.dpad();
        set_button(snap, GamepadButton::DPadUp, dpad.up().isPressed());
        set_button(snap, GamepadButton::DPadDown, dpad.down().isPressed());
        set_button(snap, GamepadButton::DPadLeft, dpad.left().isPressed());
        set_button(snap, GamepadButton::DPadRight, dpad.right().isPressed());

        // Sticks + trigger axes (GameController Y is +up, matching our
        // convention — the device only normalizes sign/range, spec §6.3).
        let ls = pad.leftThumbstick();
        snap.axes.insert(GamepadAxis::LeftStickX, ls.xAxis().value());
        snap.axes.insert(GamepadAxis::LeftStickY, ls.yAxis().value());
        let rs = pad.rightThumbstick();
        snap.axes.insert(GamepadAxis::RightStickX, rs.xAxis().value());
        snap.axes.insert(GamepadAxis::RightStickY, rs.yAxis().value());
        snap.axes.insert(GamepadAxis::LeftTrigger, pad.leftTrigger().value());
        snap.axes.insert(GamepadAxis::RightTrigger, pad.rightTrigger().value());
    }
}

/// Insert/remove a held button in the snapshot's bitset.
fn set_button(snap: &mut PadSnapshot, button: GamepadButton, pressed: bool) {
    if pressed {
        snap.buttons.insert(button);
    } else {
        snap.buttons.remove(&button);
    }
}
