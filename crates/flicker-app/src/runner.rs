//! winit 0.30 `ApplicationHandler` adapter that drives an [`App`].
//!
//! The window cannot be created before `resumed` fires (this is the new
//! lifecycle in winit 0.30 — required for correctness on macOS and iOS), so
//! the runner holds `Option`s for the window/renderer and lazily creates them
//! the first time `resumed` is dispatched.
//!
//! Each frame the runner:
//! 1. Updates the [`InputState`] snapshot from accumulated events.
//! 2. Computes `dt` since the previous frame.
//! 3. Calls `App::update(dt, &input)`.
//! 4. Calls `App::render(renderer)` between `begin_frame` and `end_frame`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use flicker_core::input::{InputState, Key};
use flicker_render::Renderer;
use glam::Vec2;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
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
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("close requested");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                renderer.resize(size.width, size.height);
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let PhysicalPosition { x, y } = position;
                self.input.mouse_position = Vec2::new(x as f32, y as f32);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = matches!(state, ElementState::Pressed);
                match button {
                    MouseButton::Left => {
                        // Record an up→down edge for click-to-toggle UI.
                        if down && !self.input.mouse_left {
                            self.input.mouse_left_pressed = true;
                        }
                        self.input.mouse_left = down;
                    }
                    MouseButton::Right => self.input.mouse_right = down,
                    MouseButton::Middle => self.input.mouse_middle = down,
                    // Back / Forward / Other — not modeled yet; ignore.
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Accumulate into the per-frame delta; reset after
                // `App::update` consumes it. Line vs pixel deltas are
                // normalized so a "one-notch" mouse wheel and a
                // trackpad swipe both land in roughly the same range.
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
                let now = Instant::now();
                let dt = self
                    .last_update
                    .map(|t| now.saturating_duration_since(t))
                    .unwrap_or(Duration::ZERO);
                self.last_update = Some(now);

                self.app.update(dt, &self.input, renderer);
                // Per-frame scroll delta and "just-pressed" edges are
                // "what arrived this frame"; reset them now so the
                // next `update` sees only the next frame's events.
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
                // Continuous redraw — the next pass adds a fixed-step loop.
                window.request_redraw();
            }
            _ => {}
        }
    }
}

/// Translate a winit physical key into our [`Key`] enum. Keys we
/// haven't named yet are returned as `None`; add a variant in
/// `flicker-core::input::Key` and a mapping arm here as games need them.
fn translate_key(key: PhysicalKey) -> Option<Key> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        KeyCode::Escape => Key::Escape,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyZ => Key::Z,
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        KeyCode::Space => Key::Space,
        KeyCode::ShiftLeft => Key::LeftShift,
        KeyCode::ControlLeft => Key::LeftControl,
        KeyCode::Digit1 => Key::Digit1,
        KeyCode::Digit2 => Key::Digit2,
        KeyCode::Backslash => Key::Backslash,
        _ => return None,
    })
}

/// Run the application. Blocks until the event loop exits.
pub fn run<A: App>(app: A) -> Result<()> {
    let event_loop = EventLoop::new().context("failed to create winit event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut runner = Runner {
        app,
        window: None,
        renderer: None,
        app_initialized: false,
        input: InputState::new(),
        last_update: None,
    };

    event_loop
        .run_app(&mut runner)
        .context("event loop exited with error")?;

    Ok(())
}
