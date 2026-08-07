//! winit 0.30 `ApplicationHandler` adapter that drives an [`App`].
//!
//! The window cannot be created before `resumed` fires (this is the new
//! lifecycle in winit 0.30 — required for correctness on macOS and iOS), so
//! the runner holds `Option`s for the window/renderer and lazily creates them
//! the first time `resumed` is dispatched.
//!
//! Each frame the runner:
//! 1. Flushes the platform sources ([`WindowSource`] KBM + [`GamepadSource`]
//!    buttons) into the [`InputState`] snapshot and fills the 120 Hz analog cache
//!    — all in `flicker-input-device`; the runner only forwards `&WindowEvent`s
//!    and drives the per-frame flush. The flush records ordered key/mouse
//!    transitions, so a press that begins and ends while a frame runs long still
//!    reaches `update` — late by at most one frame, never dropped.
//! 2. Computes `dt` since the previous frame.
//! 3. Calls `App::update(dt, &input)`.
//! 4. Calls `App::render(renderer)` between `begin_frame` and `end_frame`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use flicker_input_core::InputState;
use flicker_input_device::{DiscreteSource, GamepadSource, WindowSource};
use flicker_render::Renderer;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::App;

struct Runner<A: App> {
    app: A,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    app_initialized: bool,
    input: InputState,
    last_update: Option<Instant>,
    /// Keyboard + mouse translation (winit events → snapshot). Moved out of the
    /// runner into `flicker-input-device`; fed via `ingest`, flushed via `drain_into`.
    window_source: WindowSource,
    /// The active pad + the 120 Hz analog hub — GameController on macOS, gilrs
    /// elsewhere. Held buttons via `drain_into`; the analog cache via `tick_analog`.
    gamepad: GamepadSource,
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

            // Register the app's custom hardware cursor, if it provides one.
            // Appearance only: nothing here shows, hides, or captures the
            // pointer — that stays with the input-modality wiring.
            if let Some(c) = self.app.cursor() {
                match winit::window::CustomCursor::from_rgba(
                    c.rgba, c.width, c.height, c.hotspot.0, c.hotspot.1,
                ) {
                    Ok(src) => window.set_cursor(event_loop.create_custom_cursor(src)),
                    Err(e) => {
                        tracing::warn!("custom cursor rejected — keeping the platform arrow: {e}");
                    }
                }
            }

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

        // Read any controllers already connected at startup.
        self.gamepad.drain_into(&mut self.input);

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
        match &event {
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
            // Keyboard + mouse translation now lives in `flicker-input-device`.
            // Buffer the event; it is flushed into the snapshot at frame build
            // (RedrawRequested) via `WindowSource::drain_into`.
            WindowEvent::CursorMoved { .. }
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. } => {
                self.window_source.ingest(&event);
            }
            WindowEvent::RedrawRequested => {
                // Build this frame's input snapshot (still under ControlFlow::Poll):
                // flush the accumulated KBM events, pump the gamepad held-buttons,
                // fill the 120 Hz analog cache once, and latch a coherent analog
                // sample onto the snapshot (spec §5 / §6.3).
                self.window_source.drain_into(&mut self.input);
                self.gamepad.drain_into(&mut self.input);
                self.gamepad.tick_analog();
                self.input.set_analog_latch(self.gamepad.analog().sample());

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
                // Reset every per-frame edge (ordered transition log, text entry,
                // mouse) in one call; held state survives.
                self.input.clear_frame_edges();

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

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Capture the window's final geometry so the shell can persist "where the
        // window was" after `run` returns. winit calls this as the loop exits, while
        // the window is still alive.
        if let Some(window) = self.window.as_ref() {
            let inner = window.inner_size();
            let (x, y) = window.outer_position().map(|p| (p.x, p.y)).unwrap_or((0, 0));
            let geom = WindowGeometry {
                x,
                y,
                width: inner.width,
                height: inner.height,
                fullscreen: window.fullscreen().is_some(),
            };
            if let Ok(mut last) = LAST_WINDOW_GEOMETRY.lock() {
                *last = Some(geom);
            }
        }
    }
}

/// A window's outer position + inner size (physical px) at event-loop exit.
#[derive(Copy, Clone, Debug)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// `true` if the window was fullscreen at exit — its position is then not a
    /// meaningful windowed placement, so the shell keeps the last windowed one.
    pub fullscreen: bool,
}

/// Window geometry captured when the event loop last exited (see [`run`]).
static LAST_WINDOW_GEOMETRY: Mutex<Option<WindowGeometry>> = Mutex::new(None);

/// The window's geometry captured at the last event-loop exit, if available — for
/// the shell to persist the windowed size + position across launches.
pub fn last_window_geometry() -> Option<WindowGeometry> {
    LAST_WINDOW_GEOMETRY.lock().ok().and_then(|g| *g)
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
        window_source: WindowSource::new(),
        gamepad: GamepadSource::new(),
    };

    event_loop
        .run_app(&mut runner)
        .context("event loop exited with error")?;

    Ok(())
}
