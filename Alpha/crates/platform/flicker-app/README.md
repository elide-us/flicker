# flicker-app

The **platform layer** — the one place the OS window, the winit event loop, and the
per-frame device→signal pump live. A game implements the [`App`](#the-app-trait-you-implement-this)
trait and hands an instance to [`run`](#entry-points) / [`run_with_input`](#entry-points);
from then on this crate owns the loop. Each frame it drains the input devices into one
snapshot, resolves that snapshot into the frame's **signal events** for the app's active
context, calls `App::update` with them, then `App::render`. It is the *only* crate that
touches winit; every layer above it works in signals and a `Renderer`, never in raw OS
events.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

---

## Contents

1. [Vocabulary](#vocabulary)
2. [Where it sits](#where-it-sits)
3. [The frame, end to end](#the-frame-end-to-end)
4. [Public API](#public-api)
5. [Interactions](#interactions)
6. [Gates](#gates)
7. [Sharp edges](#sharp-edges)

---

## Vocabulary

Terms this README leans on (flicker-specific; not general programming):

- **signal** — the semantic *what* of an input (`Confirm`, `Menu`, `NavDown`, `MoveForward`,
  `LookRight`…). The one input vocabulary, owned and named by
  [`flicker-input-core`](../../input/flicker-input-core/README.md#the-signal-catalog). A key
  or button is never wired to an action; it resolves to a signal first (rule 37722F91).
- **the pump** — this crate's central device→signal loop (`InputPump` in `runner.rs`), owned
  **once** for the whole app instead of every scene owning a copy. Present only under
  [`run_with_input`](#entry-points).
- **context** (`InputContext`) — the mode/vehicle a frame's input resolves under (`World`
  base, `Menu`, `TextEntry`, `FlightPath`…). The app declares the active one via
  [`App::active_context`](#the-app-trait-you-implement-this); the pump syncs its stack to it
  each frame. Owned by [`flicker-input-core`](../../input/flicker-input-core/README.md).
- **snapshot** (`InputState`) — the raw held-state + this-frame edges of every device,
  assembled once per frame. What `axis`/`held` query against; what an un-migrated scene may
  still poll directly.
- **resolved event** (`InputEvent`) — one fired signal + its kind + the active context + a
  borrow of the snapshot. The unit the pump produces and hands up; defined in
  [`flicker-input-router`](../../input/flicker-input-router/README.md).
- **the Model** — the per-frame key→value table the engine hands to Lua. flicker-app sits
  *below* it and touches it not at all (see [Interactions](#interactions)).
- **exclusive mode / pointer capture** — a full-screen play surface taking over the mouse for
  locked-cursor camera control. The app requests it per frame via
  [`App::pointer_captured`](#the-app-trait-you-implement-this); the runner grabs + hides the
  OS cursor.

---

## Where it sits

Bottom of the stack — nothing of ours sits below it except the input/render leaf crates it
drives.

**Builds on:**

| Crate | Takes from it |
|---|---|
| [`flicker-input-device`](../../input/flicker-input-device/README.md) | `WindowSource` (winit KBM → snapshot), `GamepadSource` (pad buttons + the 120 Hz analog hub), `note_frame` (last-used device family). The runner forwards `&WindowEvent`s in and drives the per-frame flush |
| [`flicker-input-core`](../../input/flicker-input-core/README.md) | `Resolver` (snapshot → `Fired`), `ContextualBindings` (the context stack), `GamepadConfig`, `InputState`, `InputMap`, `InputContext` — the pump's machinery |
| [`flicker-input-router`](../../input/flicker-input-router/README.md) | `InputEvent` (`from_fired`), `RouteCtx`, `apply_context_requests` — the bus the resolved events flow into and the reconciliation of a frame's context requests |
| [`flicker-render`](../../render/flicker-render/README.md) | `Renderer` — created here from the window, then **owns an `Arc<Window>` clone** and is the crate that mutates the window (resize / position / fullscreen). `begin_frame` / `end_frame` / `tick` |
| `winit` · `pollster` · `anyhow` · `tracing` | the event loop + window; `block_on` for the one async call (`Renderer::new`); error context; logging |

**Used by:**

| Crate | Takes |
|---|---|
| [`flicker-scene`](../../frontend/flicker-scene/README.md) | The **only** `App` implementor (`SceneManager`). Re-exports `FrameInput` as `SceneInput` and forwards `active_context()` / `pointer_captured()` / `cursor()` to its top scene |
| `flicker-shell` | Calls `run_with_input` (bindings from the player profile + a live-rebind closure); reads `last_window_geometry()` at exit to persist window placement |
| every `scenes/*` crate | Consumes the frame's input through the `SceneInput` (= `FrameInput`) alias — `events` + the continuous-query surface (`axis` / `held` / `pointer_delta`) |
| `prism-alpha` | Transitively, through `flicker_shell::run` |

**Content files it reaches into:** **none.** flicker-app reads no scene file, no `ui_theme.json`,
no stringtable. The window title and initial size are hardcoded (see
[Sharp edges](#sharp-edges) #5); the player profile → bindings is assembled by the shell and
injected via `run_with_input`.

---

## The frame, end to end

flicker-app *is* the loop the rest of the engine hangs off. The runner is a winit
`ApplicationHandler`; the interesting work happens once per `RedrawRequested`, under
`ControlFlow::Poll` (the loop runs continuously, it does not wait for events).

**Startup (lazy, on the first winit `resumed`):** the window cannot exist before `resumed`
fires (winit 0.30, required for macOS/iOS), so the runner creates the window + `Renderer`
there, registers the app's custom [`cursor`](#window-types) once, then calls `App::init`
once. Nothing GPU exists until the OS grants a surface.

**Each frame:**

1. **Build the snapshot.** Flush the buffered KBM events (`WindowSource`) and the pad
   held-buttons (`GamepadSource`) into the `InputState`; tick the 120 Hz analog cache once and
   latch a coherent sample; note the last-used device family (kbm ⇄ pad) — all in
   [`flicker-input-device`](../../input/flicker-input-device/README.md). The flush replays an
   **ordered** edge log, so a press that begins *and* ends inside one long frame still reaches
   `update` — late by at most one frame, never dropped.
2. **Compute `dt`** since the previous frame.
3. **Pump (only under `run_with_input`).** Adopt any live `World` rebind the caller's closure
   returns; sync the context stack to `App::active_context()` (unwind to the `World` base,
   then push the declared context); resolve the snapshot → `Vec<InputEvent>` for that context.
   Under plain `run` this step is skipped and the event set is empty.
4. **`App::update(dt, &input, &mut signals, renderer)`.** The app reads resolved events from
   `signals.events` and the continuous channels from `signals.axis/held/pointer_delta`, and
   the raw snapshot from `input`.
5. **Reconcile.** `pump.apply(&signals.route)` drains the scene's context requests into the
   shared stack (via `apply_context_requests`).
6. **Cursor.** `reconcile_pointer_capture(App::pointer_captured())` grabs + hides the OS cursor
   on the *change edge* (or ungrabs + shows on release) — see [Sharp edges](#sharp-edges) #7.
7. **Clear per-frame edges** (the ordered transition log, text entry, mouse deltas); held state
   survives.
8. **Quit check.** `App::should_quit()` → exit the loop cleanly.
9. **Draw.** `renderer.tick(dt)` (advances the per-surface clock once, before any offscreen
   pass re-enters `begin_frame`), then `begin_frame` → `App::render(renderer)` → `end_frame`,
   then request the next redraw.

**Shutdown:** on the winit `exiting` callback (window still alive) the runner captures the
window's final [`WindowGeometry`](#window-types) into a process-global that
[`last_window_geometry()`](#window-types) reads after the loop returns.

---

## Public API

### Entry points

Both block until the event loop exits.

| Item | What it is for | The one thing to know |
|---|---|---|
| `run<A: App>(app) -> Result<()>` | Run with **no** pump — the app gets an empty event set each frame | `signals.events` is always `&[]` and every continuous query reads zero. The minimal / raw-poll entry (a static-scene app, or one that polls `InputState` itself). **No shipped entry point uses it today** — the shell uses `run_with_input`. Returns `Ok(())` even if the window/renderer fail to create — see [Sharp edges](#sharp-edges) #1 |
| `run_with_input<A: App>(app, bindings, gamepad, rebind) -> Result<()>` | Run **with** the central pump | `bindings` + `gamepad` come from the player profile (the shell builds them); `rebind: impl FnMut() -> Option<InputMap>` is polled each frame for the current committed `World` map, so a settings-rebind reaches every scene live. The production entry point |

### The `App` trait (you implement this)

Lifecycle: `init` once after the window + renderer exist, then per frame `update` →
`should_quit` → `render`. `update` and `should_quit` have defaults, so a draw-only app can omit
them.

| Method | Default | What it does |
|---|---|---|
| `init(&mut self, renderer: &mut Renderer)` | *required* | One-time setup after the window + renderer are ready: upload textures, stash handles |
| `render(&mut self, renderer: &mut Renderer)` | *required* | Draw the frame; the runner brackets this with `begin_frame` / `end_frame` |
| `update(dt, input, signals, renderer)` | no-op | Advance state. `input: &InputState` is the raw snapshot; `signals: &mut FrameInput` is the resolved input (events + continuous queries + route scratch); `renderer: &Renderer` is read-only here (query `renderer.size()` etc.) |
| `active_context(&self) -> Option<InputContext>` | `None` (the `World` base) | The context the pump resolves this frame's events for. A `SceneManager` forwards its top scene's declaration. Declared at the **signal** level, never keys (DFE3E44E) |
| `pointer_captured(&self) -> bool` | `false` | Whether to **capture** (grab + hide) the OS cursor this frame for locked-cursor camera control. The runner reads it after `update` and grabs on the edge. No shipped scene returns `true` yet (exclusive-mode consumer deferred post-0.1.1) — the mechanism is complete, not dead |
| `cursor(&self) -> Option<CursorImage>` | `None` (platform arrow) | A custom hardware cursor, asked **once** at window creation. Appearance only — visibility/capture is `pointer_captured`'s job, not this |
| `should_quit(&self) -> bool` | `false` | The runner exits the loop cleanly when this returns `true` |

### `FrameInput` — the frame's resolved input

The runner is the only caller of `new`; a scene *receives* a `&mut FrameInput` and never builds
one. (`flicker-scene` re-exports it as `SceneInput`.)

| Item | What it is for | The one thing to know |
|---|---|---|
| `events: &[InputEvent]` (pub field) | This frame's resolved signal edges for the active context | Empty under plain `run`. Route these through your handler chain |
| `route: &mut RouteCtx` (pub field) | The scratch your handlers push context / focus requests into | The runner reconciles it into the shared stack **after** `update` returns |
| `axis(signal, input) -> f32` | Analog deflection of `signal`, `0..1` — stick rate / trigger travel (`1.0` while a bound key is down, so KBM + pad share one path) | Multiply by `dt` for a rate. Zero with no pump. Pass the **same** `input` you were handed |
| `pointer_delta(signal, input) -> f32` | Mouse-look delta (px **this frame**) for `signal` | **Add** it frame-absolute (no `dt`). Zero unless a `MouseMotion` is bound to `signal` and its gate held. Zero with no pump |
| `held(signal, input) -> bool` | Is any binding for `signal` held (deadzone-aware)? | False with no pump |
| `new(events, route, cont)` | Assemble one | Runner + tests only |

### Window types

| Item | What it is for | The one thing to know |
|---|---|---|
| `CursorImage { rgba, width, height, hotspot }` | A custom hardware cursor returned by `App::cursor` | Tightly packed RGBA8, `width * height * 4` bytes; `hotspot` is the `(x, y)` pixel that *is* the pointer position |
| `WindowGeometry { x, y, width, height, fullscreen }` | The window's outer position + inner size (physical px) at loop exit | `fullscreen == true` means `x/y` are not a meaningful windowed placement (the shell keeps the last windowed one) |
| `last_window_geometry() -> Option<WindowGeometry>` | Read the geometry captured at the last event-loop exit | Process-global; `None` until the loop has exited once; reflects the **last** exit across multiple runs. flicker-app's only window-geometry surface (read side, at exit) |

### Courtesy re-exports

So a game can write one `use` line, the whole input vocabulary is re-exported straight from
its canonical home: `ActionSignal`, `InputContext`, `ContextBindings`, `ContextualBindings`,
`GamepadConfig`/`GamepadState`/`GamepadAxis`/`GamepadButton`,
`InputState`, `InputMap`, `InputProfile`, `InputBinding`/`SignalBinding`, `Key`, `MouseButton`,
`AbstractControls`, `AxisDirection`, `DeadzoneShape`, `RebindCapture` (from
[`flicker-input-core`](../../input/flicker-input-core/README.md)); `InputEvent`, `RouteCtx`
(from [`flicker-input-router`](../../input/flicker-input-router/README.md)). Document and use
them from **those** crates' READMEs — this crate re-exports, it does not own them.

---

## Interactions

- **Signals it captures — none.** flicker-app is the **pump**, one level *below* any handler:
  it *produces* the resolved signal stream (`FrameInput.events`) for the app's active context
  and consumes nothing. The catalog of signals it can emit is
  [`flicker-input-core`](../../input/flicker-input-core/README.md#the-signal-catalog)'s; the
  runner never names a signal and never matches a key/button to an action (37722F91 /
  DFE3E44E) — resolution happens in the core, the runner only forwards raw `&WindowEvent`s down
  to [`flicker-input-device`](../../input/flicker-input-device/README.md) and hands resolved
  events up. The one signal in this crate's orbit — `ToggleMouseCapture`, the exclusive-mode
  toggle — is captured by a **consumer scene**, not here; flicker-app only offers the
  `App::pointer_captured()` seam a scene drives from it.
- **Results / intents it fires** — the resolved `events`; the reconciliation of the scene's
  `route.requests` into the shared context stack (`apply_context_requests`); and the event-loop
  **exit** (on winit `CloseRequested` or `App::should_quit`).
- **Model keys** — none. flicker-app sits below the Model; it neither publishes nor binds.
- **What it hands other crates** — a `&mut Renderer` to `App::init` / `App::render`; a
  `FrameInput` (events + continuous queries + route scratch) to `App::update`; a
  `WindowGeometry` via `last_window_geometry()`. The window is **created and owned here** but
  **mutated elsewhere**: the `Renderer` holds an `Arc<Window>` clone and exposes
  resize/position/fullscreen, so a scene changes the window through the `Renderer`, never
  through flicker-app.
- **Threads / workers / async** — no threads are spawned. `pollster::block_on` drives the one
  async call (`Renderer::new`) at startup. Input is drained on the frame loop (the ordered edge
  log, not a separate thread, is what stops presses being lost when a frame runs long); the
  120 Hz analog cache lives in `GamepadSource` and is ticked once per frame here.

---

## Gates

`cargo test -p flicker-app` — **2/0.**

| Test | What it locks |
|---|---|
| `continuous_queries_are_zero_without_a_pump` | Under plain `run` (no pump), `axis` / `pointer_delta` / `held` all read zero — the same "no signals" contract as the empty event set |
| `continuous_queries_delegate_to_the_active_context_bindings` | With a pump, the queries wire through to the active-context bindings (the delegation + the borrow the runner uses; the deflection math itself is gated in `flicker-input-core`) |

Everything else this crate does — window creation, the pump's `resolve` / context-stack sync,
the cursor-lock (`reconcile_pointer_capture`), geometry capture, the `should_quit` exit — is a
live-winit / GPU-window path that cannot be unit-tested and is **verified in-window by Aaron**
(rule 664B68A6). The signal *resolution* the pump drives is gated upstream in
[`flicker-input-core`](../../input/flicker-input-core/README.md) and
[`flicker-input-router`](../../input/flicker-input-router/README.md).

---

## Sharp edges

1. **`run` / `run_with_input` return `Ok(())` even when the window or renderer fails to
   create.** Those failures log via `tracing::error!` and exit the loop *cleanly*, so the
   `Result` only ever carries `EventLoop::new()` / `run_app()` construction errors — never GPU
   or window init failure. `run(...)?` in `main` therefore exits `0` on a headless / no-GPU
   box. Watch the logs, not the return value, to know the app actually rendered. *(Tracked:
   MCP incident — see the human-docs pass report.)*
2. **The public `run` has no rustdoc.** Its contract paragraph is orphaned onto the private
   `reconcile_pointer_capture`, so `cargo doc` shows `run` blank. Read `run_with_input`'s doc +
   this README for `run`'s behaviour. *(Tracked: MCP incident — see the report.)*
3. **Under plain `run`, all input is silently empty.** `events` is `&[]`, and
   `axis`/`held`/`pointer_delta` return `0`/`false`, with no warning. If input seems dead, you
   called `run` where you wanted `run_with_input`.
4. **The continuous queries take the `input` you were handed.** `signals.axis(sig, input)`
   wants the very `&InputState` `update` received — pass it straight through. (There is only one
   in scope, so this is ergonomic, not ambiguous.)
5. **Window title (`"flicker"`) and initial size (960×540) are hardcoded** in the runner with
   no `App` hook. The shell corrects size + position on the first frames via the `Renderer`
   (which holds the `Arc<Window>`), so a saved placement applies a beat *after* the window
   appears, not at creation.
6. **`last_window_geometry()` is process-global and `None` until the loop has exited once.**
   Across two runs in one process it reflects the last exit. It is the only window-geometry
   surface flicker-app exposes; all *live* window mutation is on `flicker-render`'s `Renderer`.
7. **Cursor capture prefers a `Locked` grab, falls back to `Confined`.** Under `Locked`
   (relative, cursor pinned — true mouse-look) the look delta comes from raw
   `DeviceEvent::MouseMotion`; under the `Confined` fallback (platform refused `Locked`) it
   comes from the ordinary `CursorMoved`, and the device event is skipped to avoid
   double-counting. On release the runner zeroes `mouse_delta` so the freed cursor does not jump
   the camera one last frame. No shipped scene requests capture yet.
8. **Nothing GPU exists until the first winit `resumed`.** The window + renderer + `App::init`
   are created lazily there (winit 0.30 requirement), not inside `run`.
