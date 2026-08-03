//! Physical device identifiers: keys, mouse buttons, gamepad buttons/axes,
//! and the two small enums that describe how an analog axis or a deadzone
//! behaves.
//!
//! These are pure, platform-agnostic symbols. The `flicker-input-device`
//! crate maps `winit`/`gilrs`/GameController values onto them; nothing here
//! knows about a platform. Moved verbatim from `flicker-core::input`
//! (`Key`, `MouseButton`, `GamepadButton`, `GamepadAxis`, `DeadzoneShape`)
//! plus `AxisDirection` (previously in `bindings`).

use std::fmt;

use serde::{Deserialize, Serialize};

// ───────────────────────────────────────────────────────────────────
// Keyboard
// ───────────────────────────────────────────────────────────────────

/// Symbolic key identifier covering the full standard keyboard.
///
/// Variants map 1:1 from winit `KeyCode` values in the device crate.
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

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            // Letters
            Key::A => "A", Key::B => "B", Key::C => "C", Key::D => "D",
            Key::E => "E", Key::F => "F", Key::G => "G", Key::H => "H",
            Key::I => "I", Key::J => "J", Key::K => "K", Key::L => "L",
            Key::M => "M", Key::N => "N", Key::O => "O", Key::P => "P",
            Key::Q => "Q", Key::R => "R", Key::S => "S", Key::T => "T",
            Key::U => "U", Key::V => "V", Key::W => "W", Key::X => "X",
            Key::Y => "Y", Key::Z => "Z",
            // Digits
            Key::Digit0 => "0", Key::Digit1 => "1", Key::Digit2 => "2",
            Key::Digit3 => "3", Key::Digit4 => "4", Key::Digit5 => "5",
            Key::Digit6 => "6", Key::Digit7 => "7", Key::Digit8 => "8",
            Key::Digit9 => "9",
            // Function keys
            Key::F1 => "F1", Key::F2 => "F2", Key::F3 => "F3", Key::F4 => "F4",
            Key::F5 => "F5", Key::F6 => "F6", Key::F7 => "F7", Key::F8 => "F8",
            Key::F9 => "F9", Key::F10 => "F10", Key::F11 => "F11", Key::F12 => "F12",
            // Arrows
            Key::Up => "Up", Key::Down => "Down", Key::Left => "Left", Key::Right => "Right",
            // Modifiers
            Key::LeftShift => "L-Shift", Key::RightShift => "R-Shift",
            Key::LeftControl => "L-Ctrl", Key::RightControl => "R-Ctrl",
            Key::LeftAlt => "L-Alt", Key::RightAlt => "R-Alt",
            Key::LeftSuper => "L-Super", Key::RightSuper => "R-Super",
            // Navigation
            Key::Home => "Home", Key::End => "End", Key::PageUp => "PageUp",
            Key::PageDown => "PageDown", Key::Insert => "Insert", Key::Delete => "Del",
            Key::Backspace => "Backspace", Key::Tab => "Tab", Key::Enter => "Enter",
            Key::Escape => "Esc", Key::Space => "Space",
            Key::PrintScreen => "PrtSc", Key::ScrollLock => "ScrLk", Key::Pause => "Pause",
            // Punctuation
            Key::Minus => "-", Key::Equal => "=",
            Key::LeftBracket => "[", Key::RightBracket => "]",
            Key::Backslash => "\\", Key::Semicolon => ";", Key::Apostrophe => "'",
            Key::Comma => ",", Key::Period => ".", Key::Slash => "/", Key::Grave => "`",
            // Numpad
            Key::Numpad0 => "KP0", Key::Numpad1 => "KP1", Key::Numpad2 => "KP2",
            Key::Numpad3 => "KP3", Key::Numpad4 => "KP4", Key::Numpad5 => "KP5",
            Key::Numpad6 => "KP6", Key::Numpad7 => "KP7", Key::Numpad8 => "KP8",
            Key::Numpad9 => "KP9",
            Key::NumpadAdd => "KP+", Key::NumpadSubtract => "KP-",
            Key::NumpadMultiply => "KP*", Key::NumpadDivide => "KP/",
            Key::NumpadDecimal => "KP.", Key::NumpadEnter => "KPEnter",
            Key::NumpadEqual => "KP=", Key::NumLock => "NumLk",
        };
        write!(f, "{name}")
    }
}

// ───────────────────────────────────────────────────────────────────
// Mouse
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

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Left => write!(f, "Left Mouse"),
            Self::Right => write!(f, "Right Mouse"),
            Self::Middle => write!(f, "Middle Mouse"),
            Self::Back => write!(f, "Mouse Back"),
            Self::Forward => write!(f, "Mouse Forward"),
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Gamepad
// ───────────────────────────────────────────────────────────────────

/// Gamepad button identifier (Xbox-style naming).
///
/// Maps from gilrs `Button` / GameController values in the device crate.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadButton {
    // ── Face buttons (Xbox layout names) ──
    South,   // A / Cross
    East,    // B / Circle
    North,   // Y / Triangle
    West,    // X / Square

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

impl fmt::Display for GamepadButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::South => write!(f, "A / Cross"),
            Self::East => write!(f, "B / Circle"),
            Self::North => write!(f, "Y / Triangle"),
            Self::West => write!(f, "X / Square"),
            Self::LeftBumper => write!(f, "LB"),
            Self::RightBumper => write!(f, "RB"),
            Self::LeftTrigger => write!(f, "LT"),
            Self::RightTrigger => write!(f, "RT"),
            Self::Select => write!(f, "Select"),
            Self::Start => write!(f, "Start"),
            Self::Guide => write!(f, "Guide"),
            Self::Mode => write!(f, "Mode"),
            Self::LeftStick => write!(f, "L-Stick Press"),
            Self::RightStick => write!(f, "R-Stick Press"),
            Self::DPadUp => write!(f, "D-Pad Up"),
            Self::DPadDown => write!(f, "D-Pad Down"),
            Self::DPadLeft => write!(f, "D-Pad Left"),
            Self::DPadRight => write!(f, "D-Pad Right"),
            Self::Touchpad => write!(f, "Touchpad"),
            Self::C => write!(f, "C"),
            Self::Z => write!(f, "Z"),
        }
    }
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

impl fmt::Display for GamepadAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LeftStickX => write!(f, "L-Stick X"),
            Self::LeftStickY => write!(f, "L-Stick Y"),
            Self::RightStickX => write!(f, "R-Stick X"),
            Self::RightStickY => write!(f, "R-Stick Y"),
            Self::LeftTrigger => write!(f, "Left Trigger"),
            Self::RightTrigger => write!(f, "Right Trigger"),
        }
    }
}

/// Which half of an analog axis triggers a binding.
///
/// Previously lived in `bindings`; homed with the other device symbols per
/// spec §3.4.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AxisDirection {
    /// Positive half (> 0.5) — right / down on most sticks.
    Positive,
    /// Negative half (< -0.5) — left / up on most sticks.
    Negative,
}

impl fmt::Display for AxisDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Positive => write!(f, "+"),
            Self::Negative => write!(f, "-"),
        }
    }
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

impl fmt::Display for DeadzoneShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Circular => write!(f, "Circular"),
            Self::PerAxis => write!(f, "Per-Axis"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_button_labels_match_physical_position() {
        // North is the TOP face button (Y / Triangle), West is the LEFT (X / Square) —
        // gilrs' convention, preserved by the straight ingestion in the device crate.
        // A prior bug had these two labels swapped in the settings UI.
        assert_eq!(GamepadButton::North.to_string(), "Y / Triangle");
        assert_eq!(GamepadButton::West.to_string(), "X / Square");
        assert_eq!(GamepadButton::South.to_string(), "A / Cross");
        assert_eq!(GamepadButton::East.to_string(), "B / Circle");
    }
}
