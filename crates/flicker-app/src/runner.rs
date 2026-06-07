//! winit 0.30 `ApplicationHandler` adapter that drives an [`App`].
//!
//! The window cannot be created before `resumed` fires (this is the new
//! lifecycle in winit 0.30 — required for correctness on macOS and iOS), so
//! the runner holds `Option`s for the window/renderer and lazily creates them
//! the first time `resumed` is dispatched.
//!
//! Each frame the runner:
//! 1. Pumps the `gilrs` gamepad event queue.
//! 2. Updates the [`InputState`] snapshot from accumulated events.
//! 3. Computes `dt` since the previous frame.
//! 4. Calls `App::update(dt, &input)`.
//! 5. Calls `App::render(renderer)` between `begin_frame` and `end_frame`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use flicker_core::input::{
    GamepadAxis, GamepadButton, InputState, Key, MouseButton,
};
use flicker_render::Renderer;
use glam::Vec2;
use gilrs::{self, Gilrs};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::App;

struct Runner<A: App> {
    app: A,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    app_initialized: bool,
    input: InputState,
    last_update: Option<Instant>,
    gilrs: Gilrs,
    /// Maps gilrs gamepad ID → our player index.
    gamepad_id_map: std::collections::HashMap<gilrs::GamepadId, usize>,
    next_player: usize,
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title("flicker")
                .with_inner_size(winit::dpi::LogicalSize::new(960, 540));
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    tracing::error!("failed to create window: {e}");
                    event_loop.exit();
                    return;
                }
            };

            let renderer = match pollster::block_on(Renderer::new(window.clone())) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("failed to create renderer: {e:?}");
                    event_loop.exit();
                    return;
                }
            };

            self.window = Some(window);
            self.renderer = Some(renderer);
        }

        // Connect any gamepads that were present at startup.
        while let Some(event) = self.gilrs.next_event() {
            self.handle_gilrs_event(event);
        }

        if !self.app_initialized {
            if let Some(renderer) = self.renderer.as_mut() {
                self.app.init(renderer);
                self.app_initialized = true;
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("close requested");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = position;
                self.input.mouse_position = Vec2::new(x as f32, y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = matches!(state, ElementState::Pressed);
                if let Some(mb) = translate_mouse_button(button) {
                    if mb == MouseButton::Left && down && !self.input.mouse_left {
                        self.input.mouse_left_pressed = true;
                    }
                    self.input.set_mouse_button(mb, down);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 120.0) as f32,
                };
                self.input.mouse_wheel_delta += scroll;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = matches!(event.state, ElementState::Pressed);
                if let Some(key) = translate_key(event.physical_key) {
                    self.input.set_key(key, down);
                }
            }
            WindowEvent::RedrawRequested => {
                // Pump gamepad events first (before we borrow renderer).
                while let Some(event) = self.gilrs.next_event() {
                    self.handle_gilrs_event(event);
                }

                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                let Some(window) = self.window.as_ref() else {
                    return;
                };

                let now = Instant::now();
                let dt = self
                    .last_update
                    .map(|t| now.saturating_duration_since(t))
                    .unwrap_or(Duration::ZERO);
                self.last_update = Some(now);

                self.app.update(dt, &self.input, renderer);
                // Reset per-frame edges.
                self.input.mouse_wheel_delta = 0.0;
                self.input.mouse_left_pressed = false;

                if self.app.should_quit() {
                    tracing::info!("app requested quit");
                    event_loop.exit();
                    return;
                }

                renderer.begin_frame();
                self.app.render(renderer);
                if let Err(e) = renderer.end_frame() {
                    tracing::error!("frame error: {e:?}");
                }
                window.request_redraw();
            }
            _ => {}
        }
    }
}

impl<A: App> Runner<A> {
    fn handle_gilrs_event(&mut self, event: gilrs::Event) {
        match event.event {
            gilrs::EventType::Connected => {
                let player = self.next_player;
                self.next_player += 1;
                self.gamepad_id_map.insert(event.id, player);
                self.input.gamepad_mut(player);
                tracing::info!("gamepad {} connected as player {}", event.id, player);
            }
            gilrs::EventType::Disconnected => {
                if let Some(player) = self.gamepad_id_map.remove(&event.id) {
                    self.input.remove_gamepad(player);
                    tracing::info!("gamepad {} (player {}) disconnected", event.id, player);
                }
            }
            gilrs::EventType::ButtonPressed(button, _) => {
                if let (Some(&player), Some(btn)) = (
                    self.gamepad_id_map.get(&event.id),
                    translate_gamepad_button(button),
                ) {
                    self.input.gamepad_mut(player).set_button(btn, true);
                }
            }
            gilrs::EventType::ButtonReleased(button, _) => {
                if let (Some(&player), Some(btn)) = (
                    self.gamepad_id_map.get(&event.id),
                    translate_gamepad_button(button),
                ) {
                    self.input.gamepad_mut(player).set_button(btn, false);
                }
            }
            gilrs::EventType::AxisChanged(axis, value, _) => {
                if let (Some(&player), Some(ax)) = (
                    self.gamepad_id_map.get(&event.id),
                    translate_gamepad_axis(axis),
                ) {
                    self.input.gamepad_mut(player).set_axis(ax, value);
                }
            }
            _ => {}
        }
    }
}

/// Translate a gilrs button into our [`GamepadButton`] enum.
fn translate_gamepad_button(button: gilrs::Button) -> Option<GamepadButton> {
    match button {
        gilrs::Button::South => Some(GamepadButton::South),
        gilrs::Button::East => Some(GamepadButton::East),
        gilrs::Button::North => Some(GamepadButton::North),
        gilrs::Button::West => Some(GamepadButton::West),
        gilrs::Button::DPadUp => Some(GamepadButton::DPadUp),
        gilrs::Button::DPadDown => Some(GamepadButton::DPadDown),
        gilrs::Button::DPadLeft => Some(GamepadButton::DPadLeft),
        gilrs::Button::DPadRight => Some(GamepadButton::DPadRight),
        gilrs::Button::LeftTrigger => Some(GamepadButton::LeftBumper),
        gilrs::Button::LeftTrigger2 => Some(GamepadButton::LeftTrigger),
        gilrs::Button::RightTrigger => Some(GamepadButton::RightBumper),
        gilrs::Button::RightTrigger2 => Some(GamepadButton::RightTrigger),
        gilrs::Button::Select => Some(GamepadButton::Select),
        gilrs::Button::Start => Some(GamepadButton::Start),
        gilrs::Button::Mode => Some(GamepadButton::Guide),
        gilrs::Button::LeftThumb => Some(GamepadButton::LeftStick),
        gilrs::Button::RightThumb => Some(GamepadButton::RightStick),
        gilrs::Button::C => Some(GamepadButton::C),
        gilrs::Button::Z => Some(GamepadButton::Z),
        _ => None,
    }
}

/// Translate a gilrs axis into our [`GamepadAxis`] enum.
fn translate_gamepad_axis(axis: gilrs::Axis) -> Option<GamepadAxis> {
    match axis {
        gilrs::Axis::LeftStickX => Some(GamepadAxis::LeftStickX),
        gilrs::Axis::LeftStickY => Some(GamepadAxis::LeftStickY),
        gilrs::Axis::RightStickX => Some(GamepadAxis::RightStickX),
        gilrs::Axis::RightStickY => Some(GamepadAxis::RightStickY),
        gilrs::Axis::LeftZ => Some(GamepadAxis::LeftTrigger),
        gilrs::Axis::RightZ => Some(GamepadAxis::RightTrigger),
        _ => None,
    }
}

/// Translate a winit mouse button into our [`MouseButton`] enum.
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

/// Translate a winit physical key into our [`Key`] enum.
///
/// Covers the full standard keyboard to match the expanded [`Key`]
/// enum. Returns `None` only for truly unknown codes.
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

/// Run the application. Blocks until the event loop exits.
pub fn run<A: App>(app: A) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create winit event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(gilrs::Error::NotImplemented(g)) => {
            tracing::warn!("gamepads unavailable on this platform");
            g
        }
        Err(e) => return Err(anyhow::anyhow!("gilrs init failed: {e}")),
    };

    let mut runner = Runner {
        app,
        window: None,
        renderer: None,
        app_initialized: false,
        input: InputState::new(),
        last_update: None,
        gilrs,
        gamepad_id_map: std::collections::HashMap::new(),
        next_player: 0,
    };

    event_loop
        .run_app(&mut runner)
        .context("event loop exited with error")?;

    Ok(())
}
