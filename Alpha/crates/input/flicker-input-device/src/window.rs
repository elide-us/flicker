//! Keyboard + mouse source.
//!
//! [`WindowSource`] owns the `winit` → model translation that used to live inline
//! in `flicker-app`'s runner (`runner.rs:104-152`, `translate_key`,
//! `translate_mouse_button`), plus the typed-text / backspace seam preserved
//! verbatim (ruling `4B15929B`).
//!
//! winit delivers window events one at a time, while the runner only builds the
//! per-frame [`InputState`] at redraw. So [`WindowSource::ingest`] translates each
//! event into a small [`KbmOp`] and buffers it; [`DiscreteSource::drain_into`]
//! then replays the buffer into the snapshot at frame build. Held state (keys,
//! mouse buttons, position) lives in the persistent snapshot and survives across
//! frames; the per-frame edges (`mouse_left_pressed`, `mouse_wheel_delta`,
//! typed-text, backspace) are accumulated here and reset by the runner after
//! `App::update`.
//!
//! The drain also records each state change as an ordered
//! [`InputEdge`](flicker_input_core::InputEdge). Replaying into held-state alone
//! is lossy: a key pressed *and* released while a frame ran long applies both
//! writes to the same snapshot, and the press disappears instead of arriving late.
//! Keeping the transitions in arrival order is what lets the core `Resolver` fire
//! that press no matter how long the frame took.

use flicker_input_core::{InputEdge, InputState, Key, MouseButton};
use glam::Vec2;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::DiscreteSource;

/// One translated keyboard/mouse mutation, buffered between [`WindowSource::ingest`]
/// and [`DiscreteSource::drain_into`].
#[derive(Clone, Debug, PartialEq)]
enum KbmOp {
    CursorMoved(Vec2),
    MouseButton { button: MouseButton, down: bool },
    Wheel(f32),
    Key { key: Key, down: bool },
    Typed(String),
}

/// Keyboard + mouse source. Buffers translated events, then replays them into the
/// snapshot at frame build.
#[derive(Default)]
pub struct WindowSource {
    buffer: Vec<KbmOp>,
}

impl WindowSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Translate a single winit window event and buffer it. Non-input events
    /// (resize, close, redraw) are ignored — the runner keeps handling those.
    /// Replaces the inline arms at `runner.rs:104-152`.
    pub fn ingest(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = *position;
                self.buffer
                    .push(KbmOp::CursorMoved(Vec2::new(x as f32, y as f32)));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = matches!(state, ElementState::Pressed);
                if let Some(mb) = translate_mouse_button(*button) {
                    self.buffer.push(KbmOp::MouseButton { button: mb, down });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 120.0) as f32,
                };
                self.buffer.push(KbmOp::Wheel(scroll));
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = matches!(event.state, ElementState::Pressed);
                let key = translate_key(event.physical_key);
                if let Some(k) = key {
                    self.buffer.push(KbmOp::Key { key: k, down });
                }
                // Committed text for focused UI text fields (post-IME / post-layout).
                // Press only; drop control chars (Enter/Tab/Backspace/Escape are
                // consumed as keycodes above, so they never reach a text field).
                // Preserves the `4B15929B` text seam verbatim.
                if down {
                    let mut pushed = false;
                    if let Some(text) = event.text.as_ref() {
                        let printable: String =
                            text.chars().filter(|c| !c.is_control()).collect();
                        if !printable.is_empty() {
                            self.buffer.push(KbmOp::Typed(printable));
                            pushed = true;
                        }
                    }
                    // Fallback: some platforms/layouts don't populate `text` for the
                    // space bar — commit a space explicitly so text fields still get it.
                    if !pushed && key == Some(Key::Space) {
                        self.buffer.push(KbmOp::Typed(" ".to_string()));
                    }
                }
            }
            _ => {}
        }
    }
}

impl DiscreteSource for WindowSource {
    fn drain_into(&mut self, out: &mut InputState) {
        for op in self.buffer.drain(..) {
            apply_op(&op, out);
        }
    }
}

/// Apply one buffered op to the snapshot. The `mouse_left_pressed` edge is
/// computed against the snapshot's retained held-state, exactly as the old inline
/// arm did (`runner.rs:111`).
fn apply_op(op: &KbmOp, out: &mut InputState) {
    match op {
        KbmOp::CursorMoved(pos) => out.mouse_position = *pos,
        KbmOp::MouseButton { button, down } => {
            if out.mouse_button_down(*button) != *down {
                out.push_edge(InputEdge::Mouse { button: *button, down: *down });
            }
            if *button == MouseButton::Left && *down && !out.mouse_left {
                out.mouse_left_pressed = true;
            }
            out.set_mouse_button(*button, *down);
        }
        KbmOp::Wheel(scroll) => out.mouse_wheel_delta += *scroll,
        KbmOp::Key { key, down } => {
            // Only an actual state change is an edge: winit re-delivers a held key
            // as auto-repeat, which must not read as a fresh press.
            if out.key_down(*key) != *down {
                out.push_edge(InputEdge::Key { key: *key, down: *down });
            }
            if *down && *key == Key::Backspace {
                out.flag_backspace();
            }
            out.set_key(*key, *down);
        }
        KbmOp::Typed(text) => out.push_typed(text),
    }
}

/// Translate a winit mouse button into the model's [`MouseButton`].
/// Moved verbatim from `runner.rs:216`.
fn translate_mouse_button(button: winit::event::MouseButton) -> Option<MouseButton> {
    match button {
        winit::event::MouseButton::Left => Some(MouseButton::Left),
        winit::event::MouseButton::Right => Some(MouseButton::Right),
        winit::event::MouseButton::Middle => Some(MouseButton::Middle),
        winit::event::MouseButton::Back => Some(MouseButton::Back),
        winit::event::MouseButton::Forward => Some(MouseButton::Forward),
        _ => None,
    }
}

/// Translate a winit physical key into the model's [`Key`].
///
/// Covers the full standard keyboard. Returns `None` only for truly unknown
/// codes. Moved verbatim from `runner.rs:231`.
fn translate_key(key: PhysicalKey) -> Option<Key> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        // Letters
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,
        // Digits
        KeyCode::Digit0 => Key::Digit0,
        KeyCode::Digit1 => Key::Digit1,
        KeyCode::Digit2 => Key::Digit2,
        KeyCode::Digit3 => Key::Digit3,
        KeyCode::Digit4 => Key::Digit4,
        KeyCode::Digit5 => Key::Digit5,
        KeyCode::Digit6 => Key::Digit6,
        KeyCode::Digit7 => Key::Digit7,
        KeyCode::Digit8 => Key::Digit8,
        KeyCode::Digit9 => Key::Digit9,
        // Function keys
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        // Arrows
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        // Modifiers
        KeyCode::ShiftLeft => Key::LeftShift,
        KeyCode::ShiftRight => Key::RightShift,
        KeyCode::ControlLeft => Key::LeftControl,
        KeyCode::ControlRight => Key::RightControl,
        KeyCode::AltLeft => Key::LeftAlt,
        KeyCode::AltRight => Key::RightAlt,
        KeyCode::SuperLeft => Key::LeftSuper,
        KeyCode::SuperRight => Key::RightSuper,
        // Navigation / editing
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Insert => Key::Insert,
        KeyCode::Delete => Key::Delete,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::Enter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Space => Key::Space,
        KeyCode::PrintScreen => Key::PrintScreen,
        KeyCode::ScrollLock => Key::ScrollLock,
        KeyCode::Pause => Key::Pause,
        // Punctuation
        KeyCode::Minus => Key::Minus,
        KeyCode::Equal => Key::Equal,
        KeyCode::BracketLeft => Key::LeftBracket,
        KeyCode::BracketRight => Key::RightBracket,
        KeyCode::Backslash => Key::Backslash,
        KeyCode::Semicolon => Key::Semicolon,
        KeyCode::Quote => Key::Apostrophe,
        KeyCode::Comma => Key::Comma,
        KeyCode::Period => Key::Period,
        KeyCode::Slash => Key::Slash,
        KeyCode::Backquote => Key::Grave,
        // Numpad
        KeyCode::Numpad0 => Key::Numpad0,
        KeyCode::Numpad1 => Key::Numpad1,
        KeyCode::Numpad2 => Key::Numpad2,
        KeyCode::Numpad3 => Key::Numpad3,
        KeyCode::Numpad4 => Key::Numpad4,
        KeyCode::Numpad5 => Key::Numpad5,
        KeyCode::Numpad6 => Key::Numpad6,
        KeyCode::Numpad7 => Key::Numpad7,
        KeyCode::Numpad8 => Key::Numpad8,
        KeyCode::Numpad9 => Key::Numpad9,
        KeyCode::NumpadAdd => Key::NumpadAdd,
        KeyCode::NumpadSubtract => Key::NumpadSubtract,
        KeyCode::NumpadMultiply => Key::NumpadMultiply,
        KeyCode::NumpadDivide => Key::NumpadDivide,
        KeyCode::NumpadDecimal => Key::NumpadDecimal,
        KeyCode::NumpadEnter => Key::NumpadEnter,
        KeyCode::NumpadEqual => Key::NumpadEqual,
        KeyCode::NumLock => Key::NumLock,
        // CapsLock / ContextMenu — not modeled; skip
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_mouse_button_maps_all_five() {
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Left),
            Some(MouseButton::Left)
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Right),
            Some(MouseButton::Right)
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Middle),
            Some(MouseButton::Middle)
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Back),
            Some(MouseButton::Back)
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Forward),
            Some(MouseButton::Forward)
        );
        assert_eq!(
            translate_mouse_button(winit::event::MouseButton::Other(9)),
            None
        );
    }

    #[test]
    fn translate_key_maps_letters_and_specials() {
        assert_eq!(
            translate_key(PhysicalKey::Code(KeyCode::KeyW)),
            Some(Key::W)
        );
        assert_eq!(
            translate_key(PhysicalKey::Code(KeyCode::Space)),
            Some(Key::Space)
        );
        assert_eq!(
            translate_key(PhysicalKey::Code(KeyCode::Backspace)),
            Some(Key::Backspace)
        );
        // CapsLock is intentionally unmodeled.
        assert_eq!(translate_key(PhysicalKey::Code(KeyCode::CapsLock)), None);
    }

    #[test]
    fn mouse_left_press_edge_fires_once() {
        let mut input = InputState::new();
        // First down: edge fires and held-state set.
        apply_op(
            &KbmOp::MouseButton {
                button: MouseButton::Left,
                down: true,
            },
            &mut input,
        );
        assert!(input.mouse_left);
        assert!(input.mouse_left_pressed);

        // Runner resets the edge after update; a second down while still held
        // must NOT re-fire the edge.
        input.mouse_left_pressed = false;
        apply_op(
            &KbmOp::MouseButton {
                button: MouseButton::Left,
                down: true,
            },
            &mut input,
        );
        assert!(!input.mouse_left_pressed);
    }

    #[test]
    fn key_and_backspace_and_typed_replay() {
        let mut input = InputState::new();
        apply_op(
            &KbmOp::Key {
                key: Key::Backspace,
                down: true,
            },
            &mut input,
        );
        assert!(input.key_down(Key::Backspace));
        assert!(input.backspace());

        apply_op(&KbmOp::Typed("ab".to_string()), &mut input);
        apply_op(&KbmOp::Typed("c".to_string()), &mut input);
        assert_eq!(input.typed(), "abc");
    }

    /// THE long-frame case: a key pressed AND released while one frame ran long
    /// arrives as two ordered edges, even though the held-state it leaves behind is
    /// identical to never having touched the key.
    #[test]
    fn press_and_release_in_one_drain_survives_as_edges() {
        let mut src = WindowSource::new();
        src.buffer.push(KbmOp::Key { key: Key::Space, down: true });
        src.buffer.push(KbmOp::Key { key: Key::Space, down: false });

        let mut input = InputState::new();
        src.drain_into(&mut input);

        // Held-state alone says nothing happened — this is the lossy view.
        assert!(!input.key_down(Key::Space));
        // The ordered log still carries the press, in order.
        assert_eq!(
            input.edges(),
            &[
                InputEdge::Key { key: Key::Space, down: true },
                InputEdge::Key { key: Key::Space, down: false },
            ]
        );
        assert!(input.pressed(Key::Space));
        assert!(input.released(Key::Space));
    }

    /// Auto-repeat re-delivers a held key as further `Pressed` events. Those are not
    /// transitions and must not read as fresh presses.
    #[test]
    fn auto_repeat_does_not_push_duplicate_edges() {
        let mut src = WindowSource::new();
        src.buffer.push(KbmOp::Key { key: Key::A, down: true });
        src.buffer.push(KbmOp::Key { key: Key::A, down: true });
        src.buffer.push(KbmOp::Key { key: Key::A, down: true });

        let mut input = InputState::new();
        src.drain_into(&mut input);

        assert!(input.key_down(Key::A));
        assert_eq!(input.edges(), &[InputEdge::Key { key: Key::A, down: true }]);
    }

    /// Two full taps inside one stalled frame stay two presses, in order.
    #[test]
    fn repeated_taps_in_one_drain_all_survive() {
        let mut src = WindowSource::new();
        for _ in 0..2 {
            src.buffer.push(KbmOp::Key { key: Key::V, down: true });
            src.buffer.push(KbmOp::Key { key: Key::V, down: false });
        }

        let mut input = InputState::new();
        src.drain_into(&mut input);

        let presses = input
            .edges()
            .iter()
            .filter(|e| matches!(e, InputEdge::Key { key: Key::V, down: true }))
            .count();
        assert_eq!(presses, 2);
        assert_eq!(input.edges().len(), 4);
    }

    /// Mouse buttons take the same path as keys.
    #[test]
    fn mouse_click_in_one_drain_survives_as_edges() {
        let mut src = WindowSource::new();
        src.buffer.push(KbmOp::MouseButton { button: MouseButton::Left, down: true });
        src.buffer.push(KbmOp::MouseButton { button: MouseButton::Left, down: false });

        let mut input = InputState::new();
        src.drain_into(&mut input);

        assert!(!input.mouse_left);
        assert!(input.mouse_pressed(MouseButton::Left));
        // The pre-existing one-shot edge flag keeps its old meaning.
        assert!(input.mouse_left_pressed);
    }

    #[test]
    fn drain_replays_then_clears_buffer() {
        let mut src = WindowSource::new();
        src.buffer.push(KbmOp::CursorMoved(Vec2::new(3.0, 4.0)));
        src.buffer.push(KbmOp::Wheel(1.5));
        src.buffer.push(KbmOp::Wheel(0.5));

        let mut input = InputState::new();
        src.drain_into(&mut input);
        assert_eq!(input.mouse_position, Vec2::new(3.0, 4.0));
        // Wheel accumulates across events in a frame.
        assert!((input.mouse_wheel_delta - 2.0).abs() < f32::EPSILON);
        // Buffer is emptied by the drain.
        assert!(src.buffer.is_empty());

        // A second drain with no new events is a no-op (held state persists).
        src.drain_into(&mut input);
        assert_eq!(input.mouse_position, Vec2::new(3.0, 4.0));
    }
}
