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
use flicker_input_core::{
    ContextualBindings, Fired, GamepadConfig, InputContext, InputMap, InputState, Resolver,
    TextStream,
};
use flicker_input_device::{DiscreteSource, GamepadSource, WindowSource};
use flicker_input_router::{apply_context_requests, InputEvent, RouteCtx};
use flicker_render::Renderer;
use flicker_render::Vec2;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::{App, FrameInput};

/// The central event pump — the device→signal MECHANISM the runner owns ONCE for the
/// whole app (Input Standardization, 2026-08-10), instead of every scene owning a copy.
///
/// Built from the player's profile and injected via [`run_with_input`]; the plain
/// [`run`] leaves it absent (the app then gets empty events, the pre-pump behaviour).
struct InputPump {
    resolver: Resolver,
    /// The full context registry + active-context stack. `World` is the immovable
    /// base; the pump syncs the top to the app's declared [`active_context`] each frame.
    ///
    /// [`active_context`]: crate::App::active_context
    bindings: ContextualBindings,
    gamepad: GamepadConfig,
    /// Monotonic resolver tick (edge timing); wraps.
    tick: u64,
    /// Reused fired-signal buffer — no per-frame alloc.
    fired: Vec<Fired>,
    /// Settings-rebind seam (input-P3 / S1c): pulls the current committed `World` map
    /// each frame (the shell reads it from the player's profile), which the pump writes
    /// into `bindings` so a live rebind reaches every scene consuming the pump — no
    /// scene owns a resolver. `None` = nothing to adopt this frame.
    rebind: Box<dyn FnMut() -> Option<InputMap>>,
}

impl InputPump {
    /// This frame's TEXT for the route: the keyboard's stream while the active context is
    /// `TextEntry` — the one state in which the input system READS keys instead of
    /// resolving them (Aaron 2026-09-03) — and nothing in any other context. Call after
    /// [`resolve`](Self::resolve) so the stack is synced to the app's declaration.
    fn text(&self, input: &InputState) -> TextStream {
        text_for(self.bindings.active(), input)
    }

    /// Resolve this frame's snapshot into signal events for `ctx` (the active surface's
    /// context, `None` = the base). The returned events borrow `input`.
    fn resolve<'a>(
        &mut self,
        input: &'a InputState,
        ctx: Option<InputContext>,
    ) -> Vec<InputEvent<'a>> {
        // Settings-rebind (S1c): adopt the latest committed World map before resolving,
        // so a key rebound in the pause→settings overlay takes effect live. Non-draining
        // (it reads the profile, never the scene-facing `take_pending_input`), so a scene
        // still polling its own rebind is unaffected during the migration.
        if let Some(world) = (self.rebind)() {
            self.bindings.set_map(InputContext::World, world);
        }
        self.tick = self.tick.wrapping_add(1);
        // Sync the active context to the app's declaration: unwind to the World base,
        // then push the declared context. `pop` stops at the base, so this terminates.
        while self.bindings.pop().is_some() {}
        if let Some(c) = ctx {
            if c != self.bindings.active() {
                self.bindings.push(c);
            }
        }
        self.fired.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad,
            input,
            self.tick,
            &mut self.fired,
        );
        let active = self.bindings.active();
        self.fired
            .iter()
            .map(|f| InputEvent::from_fired(f, active, input))
            .collect()
    }

    /// Reconcile the handler chain's context requests into the shared stack (focus is
    /// the scene's, applied by its walker during dispatch).
    fn apply(&mut self, route: &RouteCtx) {
        let _ = apply_context_requests(&mut self.bindings, &route.requests);
    }
}

/// The pump's text rule, as a pure function: the keyboard's stream only under
/// `TextEntry`, empty under every other context.
fn text_for(active: InputContext, input: &InputState) -> TextStream {
    if active == InputContext::TextEntry {
        input.text_stream()
    } else {
        TextStream::default()
    }
}

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
    /// The central signal pump — present when the app is run via [`run_with_input`].
    pump: Option<InputPump>,
    /// Whether the mouse is currently CAPTURED — the OS cursor is grabbed + hidden and
    /// relative motion feeds `mouse_delta` (the live-scene container's exclusive mode,
    /// barrier §4e). Tracked so the grab is applied only on the edge, and so the raw
    /// `DeviceEvent::MouseMotion` is ingested only while it holds. Driven each frame by
    /// [`App::pointer_captured`].
    pointer_captured: bool,
    /// Whether the grab that IS held is `Locked` (relative, cursor pinned) rather than
    /// the `Confined` fallback (the platform refused `Locked`): under `Locked` the
    /// cursor stops moving so `DeviceEvent::MouseMotion` is the motion source; under
    /// `Confined` the ordinary `CursorMoved` path still carries it, so the device event
    /// is skipped to avoid double-counting.
    pointer_locked: bool,
    /// Whether the window currently allows IME — held exactly while the pump's active
    /// context is `TextEntry` (a text field owns the keyboard), flipped on the edge
    /// only. With IME allowed the OS text path delivers composed / dead-key / any-layout
    /// text through `Ime` events (Aaron 2026-09-03: the keyboard yields ALL input);
    /// without it a game gets plain key events. Both channels feed the same text
    /// stream — winit sends a key's text OR an IME commit for it, never both.
    ime_allowed: bool,
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
                    c.rgba,
                    c.width,
                    c.height,
                    c.hotspot.0,
                    c.hotspot.1,
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
            | WindowEvent::KeyboardInput { .. }
            | WindowEvent::Ime(_) => {
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
                // Under a `Locked` grab the OS cursor is pinned, so `CursorMoved` no
                // longer carries motion — the raw `DeviceEvent::MouseMotion` accumulated
                // below into `mouse_delta` is the pointer-look delta this frame.
                // Latch the last-used device family (kbm ⇄ pad, tagged with the pad's
                // detected vendor) from the fully drained snapshot, BEFORE
                // `clear_frame_edges` below wipes the KBM edges. Governs which bindings
                // the UI shows (keycap vs pad glyph, and which vendor's glyph atlas).
                flicker_input_device::note_frame(&self.input, self.gamepad.vendor());

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

                // Central event pump: resolve this frame's snapshot into signal events
                // for the app's active context, hand them to `update` alongside the
                // route scratch, then reconcile the route's context requests. With no
                // pump (plain `run`) the app gets an empty set — the pre-pump path.
                let ctx = self.app.active_context();
                let mut route = RouteCtx::new();
                let events = match self.pump.as_mut() {
                    Some(pump) => pump.resolve(&self.input, ctx),
                    None => Vec::new(),
                };
                // The text channel rides the route beside the events: the pump hands the
                // keyboard's stream to the bus only while the synced context is TextEntry.
                route.text = self
                    .pump
                    .as_ref()
                    .map_or_else(TextStream::default, |p| p.text(&self.input));
                // The continuous-query surface (input-P3): a scene consuming the pump
                // reads analog axes / pointer-delta from the pump's active-context
                // bindings (synced by `resolve` above), not a private resolver. `None`
                // with no pump — queries read zero, like the empty event set.
                let cont = self.pump.as_ref().map(|p| (&p.bindings, &p.gamepad));
                let mut signals = FrameInput::new(&events, &mut route, cont);
                self.app.update(dt, &self.input, &mut signals, renderer);
                if let Some(pump) = self.pump.as_mut() {
                    pump.apply(&route);
                }
                // IME follows the TextEntry context (see the field): flip the window and
                // the text source together, on the edge only.
                let text_entry = self
                    .pump
                    .as_ref()
                    .is_some_and(|p| p.bindings.active() == InputContext::TextEntry);
                if text_entry != self.ime_allowed {
                    window.set_ime_allowed(text_entry);
                    self.ime_allowed = text_entry;
                }
                // Reconcile the OS cursor with the app's exclusive-mode request. Only the
                // edge touches the window; while captured the cursor is grabbed (Locked
                // if the platform allows, else Confined) and hidden — the mouse IS the
                // camera (barrier §4e). Split-borrows the fields (the render arm holds a
                // live `&mut self.renderer`), so this is a free fn, not a `&mut self` method.
                reconcile_pointer_capture(
                    self.app.pointer_captured(),
                    window,
                    &mut self.pointer_captured,
                    &mut self.pointer_locked,
                    &mut self.input.mouse_delta,
                );
                // Reset every per-frame edge (ordered transition log, text entry,
                // mouse) in one call; held state survives.
                self.input.clear_frame_edges();

                if self.app.should_quit() {
                    tracing::info!("app requested quit");
                    event_loop.exit();
                    return;
                }

                // Advance the per-surface clock once per frame — before `begin_frame`, which
                // `render_to_texture` re-enters per offscreen pass. Poster / `hz` surfaces
                // measure their liveness against it.
                renderer.tick(dt);
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

    /// Raw pointer motion — the source of the look delta while the cursor is `Locked`
    /// (pinned), where `WindowEvent::CursorMoved` stops firing. Accumulated into
    /// `mouse_delta` exactly as `CursorMoved` would, and ONLY while a `Locked` grab
    /// holds, so free-mouse frames (and the `Confined` fallback) keep their single
    /// `CursorMoved` source and never double-count.
    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.pointer_captured && self.pointer_locked {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.input.mouse_delta += Vec2::new(delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Capture the window's final geometry so the shell can persist "where the
        // window was" after `run` returns. winit calls this as the loop exits, while
        // the window is still alive.
        if let Some(window) = self.window.as_ref() {
            let inner = window.inner_size();
            let (x, y) = window
                .outer_position()
                .map(|p| (p.x, p.y))
                .unwrap_or((0, 0));
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

/// Run the application with NO central input pump — the app receives an empty event
/// set each frame (the pre-Input-Standardization path, for apps that still poll the
/// raw [`InputState`]). Blocks until the event loop exits.
/// Apply the app's exclusive-mode request to the OS cursor on the change edge: grab +
/// hide while captured, ungrab + show while free. Prefers `Locked` (relative, cursor
/// pinned — true mouse-look); falls back to `Confined` when the platform refuses it
/// (e.g. some X11/Wayland setups), which keeps the pointer in the window but leaves
/// `CursorMoved` as the motion source. Idempotent per frame. A free fn (not a `&mut
/// self` method) so the render arm can hold a live `&mut self.renderer` alongside.
fn reconcile_pointer_capture(
    want: bool,
    window: &Window,
    captured: &mut bool,
    locked: &mut bool,
    mouse_delta: &mut Vec2,
) {
    if want == *captured {
        return;
    }
    *captured = want;
    if want {
        *locked = window.set_cursor_grab(CursorGrabMode::Locked).is_ok();
        if !*locked && window.set_cursor_grab(CursorGrabMode::Confined).is_err() {
            tracing::warn!(
                "cursor grab refused (Locked and Confined) — exclusive mode is degraded"
            );
        }
        window.set_cursor_visible(false);
    } else {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        *locked = false;
        window.set_cursor_visible(true);
        // Drop any relative motion that arrived on the release edge so the freed cursor
        // does not jump the camera one last frame.
        *mouse_delta = Vec2::ZERO;
    }
}

pub fn run<A: App>(app: A) -> Result<()> {
    run_inner(app, None)
}

/// Run the application with the central event pump wired to `bindings` + `gamepad`
/// (built by the caller from the player's profile). Each frame the runner resolves the
/// device snapshot into signal events for the app's [`active_context`](App::active_context)
/// and hands them to [`App::update`] via [`FrameInput`]. Blocks until the loop exits.
///
/// `rebind` is polled each frame for the current committed `World` map (the caller reads
/// it from the player's profile); when it returns `Some`, the pump adopts it in place so
/// a live settings-rebind reaches every scene consuming the pump (input-P3 / S1c).
pub fn run_with_input<A: App>(
    app: A,
    bindings: ContextualBindings,
    gamepad: GamepadConfig,
    rebind: impl FnMut() -> Option<InputMap> + 'static,
) -> Result<()> {
    run_inner(
        app,
        Some(InputPump {
            resolver: Resolver::new(),
            bindings,
            gamepad,
            tick: 0,
            fired: Vec::new(),
            rebind: Box::new(rebind),
        }),
    )
}

fn run_inner<A: App>(app: A, pump: Option<InputPump>) -> Result<()> {
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
        pump,
        pointer_captured: false,
        pointer_locked: false,
        ime_allowed: false,
    };

    event_loop
        .run_app(&mut runner)
        .context("event loop exited with error")?;

    Ok(())
}

#[cfg(test)]
mod text_route_tests {
    use super::*;

    /// The pump reads the keyboard ONLY in TextEntry: the same typed snapshot yields the
    /// stream under TextEntry and nothing under World or Menu.
    #[test]
    fn the_pump_hands_text_to_the_route_only_under_text_entry() {
        let mut input = InputState::new();
        input.push_typed("8");
        input.flag_backspace();
        let under_text = text_for(InputContext::TextEntry, &input);
        assert_eq!(under_text.typed, "8");
        assert!(under_text.backspace);
        for ctx in [InputContext::World, InputContext::Menu] {
            assert!(text_for(ctx, &input).is_empty(), "{ctx:?} reads no text");
        }
    }

    /// GREP GATE: the keyboard's text is read by the input system alone. No scene, widget
    /// or shell crate touches the snapshot's text channel — the route delivers it.
    #[test]
    fn no_crate_outside_the_input_system_reads_the_text_channel() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut offenders = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if p.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    // Separator-agnostic: the Windows runner walks `Alpha\crates\...`, and
                    // a `/`-only test let the input system's own readers through as
                    // offenders there (CI 2026-09-05, windows-latest).
                    let s = p.to_string_lossy().replace('\\', "/");
                    if s.contains("/crates/input/") || s.contains("/crates/platform/") {
                        continue;
                    }
                    let Ok(src) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    for needle in ["text_stream(", ".typed()", ".backspace()", ".preedit()"] {
                        if src.contains(needle) {
                            out.push(format!("{s}: {needle}"));
                        }
                    }
                }
            }
        }
        walk(&crates, &mut offenders);
        assert!(
            offenders.is_empty(),
            "the text channel is read outside the input system: {offenders:?}"
        );
    }
}
