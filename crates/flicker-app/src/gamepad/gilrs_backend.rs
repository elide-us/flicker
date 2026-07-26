//! gilrs gamepad input for every platform except macOS.
//!
//! gilrs is pumped straight from the main loop here — unlike on macOS, its backends on
//! Windows/Linux deliver input without any run-loop cooperation from us.

use std::collections::HashMap;

use flicker_core::input::{GamepadAxis, GamepadButton, InputState};
use gilrs::{self, Gilrs};

/// Pumps gilrs each frame and applies its events to [`InputState`].
pub struct GamepadReader {
    gilrs: Option<Gilrs>,
    /// gilrs gamepad id → our player index.
    id_map: HashMap<gilrs::GamepadId, usize>,
    next_player: usize,
}

impl GamepadReader {
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
        Self { gilrs, id_map: HashMap::new(), next_player: 0 }
    }

    pub fn poll(&mut self, input: &mut InputState) {
        // Drain events first so the gilrs borrow ends before we touch `self`/`input`.
        let events: Vec<gilrs::Event> = match self.gilrs.as_mut() {
            Some(gilrs) => std::iter::from_fn(|| gilrs.next_event()).collect(),
            None => return,
        };

        for event in events {
            let player = self.player_for(event.id);
            match event.event {
                gilrs::EventType::Connected => {
                    input.gamepad_mut(player); // ensure the slot exists
                }
                gilrs::EventType::Disconnected => {
                    input.remove_gamepad(player);
                }
                gilrs::EventType::ButtonPressed(b, _) => {
                    if let Some(btn) = translate_button(b) {
                        input.gamepad_mut(player).set_button(btn, true);
                    }
                }
                gilrs::EventType::ButtonReleased(b, _) => {
                    if let Some(btn) = translate_button(b) {
                        input.gamepad_mut(player).set_button(btn, false);
                    }
                }
                gilrs::EventType::AxisChanged(a, v, _) => {
                    if let Some(axis) = translate_axis(a) {
                        input.gamepad_mut(player).set_axis(axis, v);
                    }
                }
                _ => {}
            }
        }
    }

    /// The player index for a gilrs id, assigning the next free one on first sight.
    fn player_for(&mut self, id: gilrs::GamepadId) -> usize {
        let next = &mut self.next_player;
        *self.id_map.entry(id).or_insert_with(|| {
            let p = *next;
            *next += 1;
            p
        })
    }
}

impl Default for GamepadReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate a gilrs button into our [`GamepadButton`].
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

/// Translate a gilrs axis into our [`GamepadAxis`].
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
