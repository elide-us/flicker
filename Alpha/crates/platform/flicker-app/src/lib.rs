//! flicker-app: winit event loop, frame orchestration, and the public entry point.
//!
//! Games implement the [`App`] trait and pass an instance to [`run`]. The
//! runner owns the [`Window`](winit::window::Window) and the
//! [`Renderer`](flicker_render::Renderer); each frame it accumulates input,
//! computes `dt`, calls [`App::update`], then [`App::render`].

mod runner;

pub use runner::{last_window_geometry, run, run_with_input, WindowGeometry};

// Convenience re-export so games can `use flicker::app::{App, run, InputState, Key};`
// alongside the runner entry points. The canonical home of every input type is
// `flicker-input-core`; this is a courtesy surface on the integration crate,
// re-exported straight from it under the canonical names.
pub use flicker_input_core::rebind::RebindCapture;
pub use flicker_input_core::{
    AbstractControls, ActionSignal, AxisDirection, ContextBindings, ContextualBindings,
    DeadzoneShape, GamepadAxis, GamepadButton, GamepadConfig, GamepadState, InputBinding,
    InputContext, InputMap, InputProfile, InputState, Key, MouseButton, SignalBinding,
};
pub use flicker_input_router::{InputEvent, RouteCtx};

use std::time::Duration;

use flicker_render::Renderer;

/// The resolved discrete input for one frame, handed to [`App::update`] by the runner.
///
/// This is the seam of the central event pump (Input Standardization, 2026-08-10):
/// the device→signal MECHANISM lives in the RUNNER (one Resolver + the active-context
/// stack), not in every scene. The app receives the already-resolved signal
/// [`events`](Self::events) — pointer clicks included, once pointer-as-signal lands —
/// and the [`route`](Self::route) scratch its handler chain routes them into. The
/// runner reconciles the route's context requests against the shared stack after
/// `update` returns. An app that declares no [`active_context`](App::active_context)
/// and is run via the plain [`run`] receives an empty event set.
pub struct FrameInput<'a> {
    /// This frame's resolved signal events, for the active context.
    pub events: &'a [InputEvent<'a>],
    /// The router scratch the app routes `events` into; the runner reconciles its
    /// context requests against the shared stack after the app returns.
    pub route: &'a mut RouteCtx,
    /// The pump's active-context bindings + gamepad, for the CONTINUOUS queries a
    /// scene can't get from the edge [`events`](Self::events): analog axes (stick
    /// throttle/zoom, `signal_axis`) and pointer-look deltas (`signal_pointer_delta`).
    /// A scene that has handed device→signal resolution to the pump (input-P3,
    /// 0569DA9B) reads these through [`axis`](Self::axis) / [`pointer_delta`](Self::pointer_delta)
    /// / [`held`](Self::held) instead of owning a resolver. `None` under the plain
    /// [`run`] (no pump) — the queries then read as zero, matching the empty event set.
    cont: Option<(&'a ContextualBindings, &'a GamepadConfig)>,
}

impl<'a> FrameInput<'a> {
    /// Assemble the frame's input. `cont` is the pump's active-context bindings +
    /// gamepad for continuous queries (`None` = no pump). The runner is the only
    /// caller; a scene receives this by reference and never builds one.
    pub fn new(
        events: &'a [InputEvent<'a>],
        route: &'a mut RouteCtx,
        cont: Option<(&'a ContextualBindings, &'a GamepadConfig)>,
    ) -> Self {
        Self {
            events,
            route,
            cont,
        }
    }

    /// Analog deflection of `signal` in the active context, 0..1 — the stick-rate /
    /// trigger-travel channel (`1.0` while a bound key/button is down, so KBM and pad
    /// drive one path). A camera multiplies this by `dt`. Zero with no pump. Delegates
    /// to the pump's [`ContextualBindings::signal_axis`].
    pub fn axis(&self, signal: ActionSignal, input: &InputState) -> f32 {
        self.cont
            .map_or(0.0, |(b, g)| b.signal_axis(signal, input, g))
    }

    /// Pointer-look delta (pixels THIS frame) for `signal` — the mouse channel beside
    /// [`axis`](Self::axis)'s stick rate; a camera ADDS it frame-absolute (no `dt`).
    /// Zero unless a `MouseMotion` is bound to `signal` (and its gate held). Zero with
    /// no pump. Delegates to [`ContextualBindings::signal_pointer_delta`].
    pub fn pointer_delta(&self, signal: ActionSignal, input: &InputState) -> f32 {
        self.cont
            .map_or(0.0, |(b, _)| b.signal_pointer_delta(signal, input))
    }

    /// Is any binding for `signal` held in the active context (deadzone-aware)? The
    /// stance-level query (`signal_held`). False with no pump.
    pub fn held(&self, signal: ActionSignal, input: &InputState) -> bool {
        self.cont
            .is_some_and(|(b, g)| b.signal_held(signal, input, g))
    }
}

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

    fn update(
        &mut self,
        _dt: Duration,
        _input: &InputState,
        _signals: &mut FrameInput,
        _renderer: &Renderer,
    ) {
    }

    /// The [`InputContext`] the app's active surface owns this frame — the runner
    /// resolves the pump's events for it (syncing the top of the shared context
    /// stack). `None` (default) = the `World` base. A `SceneManager` forwards its top
    /// scene's declaration.
    fn active_context(&self) -> Option<InputContext> {
        None
    }

    fn should_quit(&self) -> bool {
        false
    }

    /// Whether the app's active surface wants the mouse CAPTURED this frame — the
    /// player has toggled the top scene into exclusive, locked-cursor camera control
    /// (the live-scene container's barrier §4e). The runner reads this after `update`
    /// and grabs/hides the OS cursor accordingly, feeding relative motion into
    /// `mouse_delta`. `false` (default) = ordinary free-mouse play. A `SceneManager`
    /// forwards its top scene's declaration.
    fn pointer_captured(&self) -> bool {
        false
    }

    fn render(&mut self, renderer: &mut Renderer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_input_core::InputProfile;

    /// With no pump (the plain `run` path), every continuous query reads zero —
    /// the same "no signals" contract as the empty event set.
    #[test]
    fn continuous_queries_are_zero_without_a_pump() {
        let input = InputState::new();
        let mut route = RouteCtx::new();
        let f = FrameInput::new(&[], &mut route, None);
        assert_eq!(f.axis(ActionSignal::MoveForward, &input), 0.0);
        assert_eq!(f.pointer_delta(ActionSignal::LookRight, &input), 0.0);
        assert!(!f.held(ActionSignal::MoveForward, &input));
    }

    /// With a pump, the queries wire through to the active-context bindings (the
    /// deflection math itself is covered in flicker-input-core). Idle input → no
    /// deflection; this exercises the delegation + the borrow wiring the runner uses.
    #[test]
    fn continuous_queries_delegate_to_the_active_context_bindings() {
        let bindings = ContextualBindings::from_profile(&InputProfile::default_profile());
        let gamepad = GamepadConfig::default();
        let input = InputState::new();
        let mut route = RouteCtx::new();
        let f = FrameInput::new(&[], &mut route, Some((&bindings, &gamepad)));
        assert_eq!(
            f.axis(ActionSignal::MoveForward, &input),
            0.0,
            "idle keys/stick = no deflection"
        );
        assert!(!f.held(ActionSignal::MoveForward, &input));
    }
}
