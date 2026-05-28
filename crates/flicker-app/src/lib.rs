//! flicker-app: winit event loop, frame orchestration, and the public entry point.
//!
//! Games implement the [`App`] trait and pass an instance to [`run`]. The
//! runner owns the [`Window`](winit::window::Window) and the
//! [`Renderer`](flicker_render::Renderer); each frame it accumulates input,
//! computes `dt`, calls [`App::update`], then [`App::render`].

mod runner;

pub use runner::run;

// Re-export so games can `use flicker::app::{App, run, InputState, Key};`
// (plus the bindings layer) without reaching into
// `flicker::core::input::*` themselves.
pub use flicker_core::input::bindings::{Action, Bindings, ControlConfig};
pub use flicker_core::input::{InputState, Key};

use std::time::Duration;

use flicker_render::Renderer;

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

    fn update(&mut self, _dt: Duration, _input: &InputState, _renderer: &Renderer) {}

    fn should_quit(&self) -> bool {
        false
    }

    fn render(&mut self, renderer: &mut Renderer);
}
