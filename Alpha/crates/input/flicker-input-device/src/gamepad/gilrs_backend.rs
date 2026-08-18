//! gilrs gamepad reader for every platform except macOS.
//!
//! gilrs is pumped straight from the main loop — unlike on macOS, its backends on
//! Windows/Linux deliver input without any run-loop cooperation from us. Single
//! local player: the first pad we see owns slot 0; all others are ignored (spec
//! §6.4). Moved from `flicker-app/src/gamepad/gilrs_backend.rs`, with the
//! `id_map`/`next_player` multi-player bookkeeping deleted and the event drain
//! refactored to fill a [`PadSnapshot`].

use flicker_input_core::{GamepadAxis, GamepadButton};
use gilrs::{self, Gilrs};

use super::{DeviceCaps, PadSnapshot};

/// Pumps gilrs each refresh and maintains the slot-0 snapshot.
pub struct PlatformGamepad {
    gilrs: Option<Gilrs>,
    /// The gilrs id that owns slot 0, if any (single player — spec §6.4).
    active: Option<gilrs::GamepadId>,
}

impl PlatformGamepad {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(gilrs::Error::NotImplemented(g)) => {
                tracing::warn!("gamepads unavailable on this platform");
                Some(g)
            }
            Err(e) => {
                tracing::error!("gilrs init failed: {e}");
                None
            }
        };
        Self {
            gilrs,
            active: None,
        }
    }

    /// Drain gilrs's queue once into the maintained slot-0 snapshot.
    pub fn read_into(&mut self, snap: &mut PadSnapshot) {
        // Drain events first so the gilrs borrow ends before we touch `self`.
        let events: Vec<gilrs::Event> = match self.gilrs.as_mut() {
            Some(gilrs) => std::iter::from_fn(|| gilrs.next_event()).collect(),
            None => {
                snap.reset_neutral();
                snap.connected = false;
                snap.caps = DeviceCaps::default();
                return;
            }
        };

        for event in events {
            let id = event.id;
            match event.event {
                gilrs::EventType::Connected => {
                    // Adopt the first pad as slot 0; ignore later connections.
                    if self.active.is_none() {
                        self.active = Some(id);
                        snap.reset_neutral();
                        snap.connected = true;
                        snap.caps = DeviceCaps::all();
                    }
                }
                gilrs::EventType::Disconnected => {
                    if self.active == Some(id) {
                        self.active = None;
                        snap.reset_neutral();
                        snap.connected = false;
                        snap.caps = DeviceCaps::default();
                    }
                }
                // Guards keep slot-0 filtering in the arm head; a pad that fails
                // the guard falls through to `_` exactly as the nested-if did.
                // (`adopt` mutating in a guard is deliberate: adoption must
                // happen even when the translate below yields nothing.)
                gilrs::EventType::ButtonPressed(b, _) if self.adopt(id, snap) => {
                    if let Some(btn) = translate_button(b) {
                        snap.buttons.insert(btn);
                    }
                }
                gilrs::EventType::ButtonReleased(b, _) if self.active == Some(id) => {
                    if let Some(btn) = translate_button(b) {
                        snap.buttons.remove(&btn);
                    }
                }
                gilrs::EventType::AxisChanged(a, v, _) if self.adopt(id, snap) => {
                    if let Some(axis) = translate_axis(a) {
                        snap.axes.insert(axis, v);
                    }
                }
                _ => {}
            }
        }
    }

    /// Adopt `id` as slot 0 if we have none, then report whether `id` owns slot 0.
    /// Covers pads already connected before the first event (no `Connected` seen).
    fn adopt(&mut self, id: gilrs::GamepadId, snap: &mut PadSnapshot) -> bool {
        match self.active {
            Some(active) => active == id,
            None => {
                self.active = Some(id);
                snap.connected = true;
                snap.caps = DeviceCaps::all();
                true
            }
        }
    }
}

impl Default for PlatformGamepad {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate a gilrs button into the model's [`GamepadButton`]. Moved verbatim.
fn translate_button(button: gilrs::Button) -> Option<GamepadButton> {
    use gilrs::Button as G;
    Some(match button {
        G::South => GamepadButton::South,
        G::East => GamepadButton::East,
        G::North => GamepadButton::North,
        G::West => GamepadButton::West,
        G::DPadUp => GamepadButton::DPadUp,
        G::DPadDown => GamepadButton::DPadDown,
        G::DPadLeft => GamepadButton::DPadLeft,
        G::DPadRight => GamepadButton::DPadRight,
        G::LeftTrigger => GamepadButton::LeftBumper,
        G::LeftTrigger2 => GamepadButton::LeftTrigger,
        G::RightTrigger => GamepadButton::RightBumper,
        G::RightTrigger2 => GamepadButton::RightTrigger,
        G::Select => GamepadButton::Select,
        G::Start => GamepadButton::Start,
        G::Mode => GamepadButton::Guide,
        G::LeftThumb => GamepadButton::LeftStick,
        G::RightThumb => GamepadButton::RightStick,
        G::C => GamepadButton::C,
        G::Z => GamepadButton::Z,
        _ => return None,
    })
}

/// Translate a gilrs axis into the model's [`GamepadAxis`]. Moved verbatim.
fn translate_axis(axis: gilrs::Axis) -> Option<GamepadAxis> {
    use gilrs::Axis as A;
    Some(match axis {
        A::LeftStickX => GamepadAxis::LeftStickX,
        A::LeftStickY => GamepadAxis::LeftStickY,
        A::RightStickX => GamepadAxis::RightStickX,
        A::RightStickY => GamepadAxis::RightStickY,
        A::LeftZ => GamepadAxis::LeftTrigger,
        A::RightZ => GamepadAxis::RightTrigger,
        _ => return None,
    })
}
