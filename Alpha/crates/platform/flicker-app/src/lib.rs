//! flicker-app: winit event loop, frame orchestration, and the public entry point.
//!
//! Games implement the [`App`] trait and pass an instance to [`run`]. The
//! runner owns the [`Window`](winit::window::Window) and the
//! [`Renderer`](flicker_render::Renderer); each frame it accumulates input,
//! computes `dt`, calls [`App::update`], then [`App::render`].

mod runner;

pub use runner::{last_window_geometry, run, WindowGeometry};

// Convenience re-export so games can `use flicker::app::{App, run, InputState, Key};`
// alongside the runner entry points. The canonical home of every input type is
// `flicker-input-core`; this is a courtesy surface on the integration crate,
// re-exported straight from it under the canonical names.
pub use flicker_input_core::{
    AbstractControls, ActionSignal, AxisDirection, ContextBindings, ContextualBindings,
    DeadzoneShape, GamepadAxis, GamepadButton, GamepadConfig, GamepadState, InputBinding,
    InputContext, InputMap, InputProfile, InputState, Key, MouseButton, SignalBinding,
};
pub use flicker_input_core::rebind::RebindCapture;

use std::time::Duration;

use flicker_render::Renderer;

/// A custom hardware mouse cursor: straight RGBA8 pixels plus the hotspot (the
/// pixel that IS the pointer position). Returned by [`App::cursor`]; the runner
/// registers it with winit once, at window creation. Appearance only — cursor
/// visibility/capture belongs to the input-modality layer, never here.
#[derive(Clone)]
pub struct CursorImage {
    /// Tightly packed RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    pub width: u16,
    pub height: u16,
    /// (x, y) of the click point within the image, in pixels.
    pub hotspot: (u16, u16),
}

/// User-implemented application contract.
///
/// Lifecycle per frame:
/// 1. Driver collects platform events into an [`InputState`] snapshot.
/// 2. Driver computes `dt` since the previous frame.
/// 3. Driver calls [`App::update`] with `dt`, the input snapshot, and a
///    read-only renderer reference (for queries like `renderer.size()`).
/// 4. Driver polls [`App::should_quit`]; if it returns `true`, the event loop
///    exits cleanly.
/// 5. Otherwise driver calls [`App::render`] between `Renderer::begin_frame`
///    and `Renderer::end_frame`.
///
/// [`App::init`] runs once after the window and renderer are ready (use it to
/// upload textures and stash handles).
///
/// `update` and `should_quit` have defaults so apps that only draw a static
/// scene can omit them; override to advance simulation state or request exit.
pub trait App: 'static {
    fn init(&mut self, renderer: &mut Renderer);

    /// The custom mouse-cursor image, asked once when the window is created.
    /// Default `None` keeps the platform arrow.
    fn cursor(&self) -> Option<CursorImage> {
        None
    }

    fn update(&mut self, _dt: Duration, _input: &InputState, _renderer: &Renderer) {}

    fn should_quit(&self) -> bool {
        false
    }

    fn render(&mut self, renderer: &mut Renderer);
}
