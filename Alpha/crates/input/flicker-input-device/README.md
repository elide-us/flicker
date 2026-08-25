# flicker-input-device

The platform input **sources**: the one input crate that touches the OS and the
hardware. It reads `winit` window events and the connected gamepad, and writes
their raw state into the pure model owned by
[`flicker-input-core`](../flicker-input-core/README.md) — the per-frame
**snapshot** (`InputState`: held keys/buttons, an ordered edge log, mouse motion,
typed text) and the volatile **analog cache** (stick/trigger values). Its whole
job is *device → snapshot*. It does **not** turn any of that into gameplay
*signals*: edge classification (Press/Release/Hold/Chord) and the binding of a
physical input to an `ActionSignal` are the core `Resolver`'s job, one layer up.
This is the crate where a physical key or button is still a real, named thing;
everywhere above it, only signals exist.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:**
  - [`flicker-input-core`](../flicker-input-core/README.md) — every model type it
    fills: `InputState`, `AnalogCache` / `AnalogFrame`, `Key`, `MouseButton`,
    `GamepadButton`, `GamepadAxis`, `InputEdge`. This crate adds no new input
    *vocabulary*; it only populates the core's.
  - `winit` — the source of keyboard/mouse `WindowEvent`s (translation only; the
    event *loop* stays in `flicker-app`).
  - `glam` (`Vec2`), `tracing` (connect/disconnect + init-failure logs).
  - **Gamepads, platform-split (load-bearing):** `gilrs` on every platform
    **except** macOS; Apple's **GameController** framework (`objc2-game-controller`)
    on macOS. This is the only crate in the workspace that declares these
    platform/hardware deps.
- **Used by:**
  - `flicker-app` — the runner owns all three sources and drives them every frame
    (see *How the runner drives it*). This is the sole driver.
  - core `flicker`, which re-exports this crate as **`flicker::input_device`**
    (`Alpha/crates/core/flicker/src/lib.rs:7`). Through that alias,
    `flicker-shell` and `flicker-componentcatalog` read `last_input_context()`,
    and `flicker-controllertester` reads the analog frame this crate latches.
- **Reads from the content tree:** nothing. This crate reads hardware, not files.
- **Layering rule (the reason this crate exists):** it must **never** depend on
  `flicker-app` — `flicker-app` depends on *it*. Device reading sits strictly
  below the application so the OS/hardware deps stay contained here and the app
  layer stays portable. The crate has no `serde` dep and serializes nothing.

## The three sources

Two implement the drain contract; the gamepad additionally owns the analog hub.

| Item | What it is | The one thing to know |
|---|---|---|
| `DiscreteSource` (trait) | The "flush my accumulated **held-state** into the snapshot" contract: `fn drain_into(&mut self, out: &mut InputState)`. | Sources write **held-state and ordered edges only** — never signals, never edge *classification*. Both sources below implement it. |
| `WindowSource` | Keyboard + mouse. `new()`, `ingest(&WindowEvent)`, `impl DiscreteSource`. | `ingest` translates+buffers **one** winit event; `drain_into` replays the buffer into the snapshot at frame build. Feed it every keyboard/mouse `WindowEvent`; it ignores the rest. |
| `GamepadSource` | The active pad (single local player, **slot 0**) **and** the analog hub. `new()` / `Default`, `impl DiscreteSource` (held buttons → slot 0), plus the accessors below. | One backend read (`refresh`) per drain fills both the held-button set and the latest axes, so buttons and axes stay coherent within a frame. |

### `GamepadSource` accessors

| Method | Returns | Notes |
|---|---|---|
| `tick_analog()` | — | Reads the pad once and pushes one `AnalogFrame` into the analog cache. See *The analog channel*. |
| `analog()` | `&AnalogCache` | The cache (current + previous frame). The runner calls `.analog().sample()` and latches the copy onto the snapshot each frame. **Live.** |
| `vendor()` | `PadVendor` | The connected pad's button-face family, classified from OS metadata on connect. `Generic` while disconnected. **Live** (fed to `note_frame`). |
| `caps()` | `DeviceCaps` | Which controls the pad exposes. *Not yet wired* — built for the settings/tester surface to gray out absent controls; no consumer reads it yet. |
| `connected()` | `bool` | Is slot 0 connected? *Not yet wired* — the same settings/tester surface; no external caller yet. |

### `DeviceCaps`

Plain struct, `pub` bool fields `has_left_stick` / `has_right_stick` /
`has_triggers` / `has_dpad`. All-false when no pad is connected; **all-true**
whenever any standard extended pad is connected (per-feature detection is a future
refinement — the type is the seam for it). *Not yet referenced outside this crate*
— a tool built toward the settings/tester surface, not dead code.

### Last-used-device monitor (process-global)

Answers one question for the UI: is the player on keyboard/mouse or a gamepad
*right now*? — so a hint can show a keycap or a controller glyph for the same
authored signal.

| Item | What it is | The one thing to know |
|---|---|---|
| `note_frame(&InputState, PadVendor)` | Latch the last-used device family from this frame's snapshot. | The runner calls it **once per frame, after the sources drain and before `clear_frame_edges`** — so this frame's KBM edges are still visible. A frame with input on neither family leaves the latch unchanged. |
| `last_input_context() -> InputDeviceKind` | The family the player last touched. | `Kbm` until the first input (and if the lock is ever poisoned — the display then just shows keycaps). Process-global, shared by every scene. |
| `InputDeviceKind` (enum) | `Kbm` \| `Pad(PadVendor)`; `token() -> &'static str`, `is_pad() -> bool`. | `token()` is the stable string a scene publishes as the `input_device` Model value: `"kbm"` / `"xbox"` / `"playstation"` / `"generic"`. |
| `PadVendor` (enum) | `Xbox` \| `PlayStation` \| `Generic` (default); `from_metadata(name, vendor_id)`. | Classifies on connect: a known USB vendor id wins (Sony `0x054C`, Microsoft `0x045E`), else the product/name string, else `Generic`. |

## Interactions

- **Signals it captures — none.** This crate is *below* the signal layer. It
  writes raw held-state and axes into the snapshot; the core `Resolver` turns
  those into `ActionSignal`s. It matches no keys/buttons to actions (rule
  `37722F91`), because it produces no actions at all.
- **Results / intents it fires — none.**
- **Model keys — one, and this crate is only the value *source*:** `input_device`.
  This crate provides the value (`last_input_context().token()`); the **scene/shell
  publishes it** — `flicker-shell` (`shell.rs:2827`) and `flicker-componentcatalog`
  (`lib.rs:211`) call `model.set("input_device", …)`. Downstream, `flicker-widgets`
  reads `model.text("input_device")` to choose keycaps vs. pad glyphs. This crate
  never touches the Model itself.
- **What it hands other crates:** it fills the shared `InputState` (held keys /
  mouse buttons / position, the ordered `InputEdge` log, `mouse_delta`,
  `mouse_wheel_delta`, typed text, the backspace flag, and slot-0 gamepad buttons +
  discrete axes) and the `AnalogCache` (via `tick_analog`). It also provides
  `PadVendor` and `DeviceCaps` for the glyph/settings layers.
- **Threads / workers / async — none.** Everything runs on the runner's main
  thread, once per frame. The macOS GameController getters are **main-thread-only**
  (documented `SAFETY`); calling them off the main thread is undefined behaviour.
  `last_input_context` is a process-global `Mutex`.

## How the runner drives it (the frame contract)

`flicker-app`'s runner performs this exact sequence; a consumer building its own
runner must keep the **order**, because nothing enforces it at compile time:

1. Per keyboard/mouse `WindowEvent`: `window_source.ingest(&event)`.
2. At frame build (redraw):
   `window_source.drain_into(&input)` → `gamepad.drain_into(&input)` →
   `gamepad.tick_analog()` → `input.set_analog_latch(gamepad.analog().sample())` →
   `note_frame(&input, gamepad.vendor())` → *then* `input.clear_frame_edges()`.

`note_frame` **must** run before `clear_frame_edges`, or KBM activity is wiped
before it is counted and the last-used device silently sticks on "pad". (Evidence:
`Alpha/crates/platform/flicker-app/src/runner.rs:222-240`.)

## The analog channel (and the RT-12 discrete-axis bridge)

Stick and trigger axes reach consumers through **two** live representations today —
both carry identical values:

1. **The analog cache** — `tick_analog()` pushes an `AnalogFrame` (raw axes,
   sign/range-normalized only; deadzone/sensitivity stay in core); the runner
   samples it and latches the copy onto the snapshot (`InputState::analog_latch`).
   This is the ratified home for **Move / Look** and is read by the souls
   controller Move/Look path and by `flicker-controllertester`.
2. **The discrete axis bridge (RT-12)** — `drain_into` *also* copies the same axes
   into the discrete `GamepadState` (`gamepad.rs:112-122`), because some camera
   presets still bind discrete stick axes rather than the analog latch.

The bridge is a **labeled, tracked transitional path** (risk RT-12): its own
comment says to delete that block only once the discrete-axis camera presets move
onto the analog latch. Until then, do not bind a signal to a discrete stick axis
*and* let the analog path drive the same signal — that double-drives it (see
`flicker-input-core/src/binding.rs:875-878`).

**Sampling rate:** despite the "120 Hz" in the crate's name, `tick_analog()` is
called **once per frame** (frame-rate) today; true 120 Hz pacing is deferred. The
`AnalogCache` is *built* for high-rate sampling (double-buffered, each frame
`seq`-numbered and timestamped), so treat 120 Hz as the channel's design target,
not its current cadence.

## Platform split

- **Non-macOS** (`gilrs_backend.rs`): gilrs is pumped straight from the main loop.
- **macOS** (`macos.rs`): Apple's GameController framework — `GCController::controllers()`
  is polled once per refresh (no callbacks, no run-loop cooperation needed).

The split is load-bearing: gilrs's raw-HID path *enumerates* Xbox pads on macOS but
delivers no input from them, so macOS uses the framework the platform expects.
Everywhere, it is **single local player** — the first pad seen owns slot 0; any
others are ignored. On disconnect, slot 0 is removed and the analog frame goes
neutral (zero axes, stamped stale) so a camera holds instead of snapping.

## Gates

The tests that pin the contracts (`cargo test -p flicker-input-device`):

- **Keyboard/mouse (`window.rs`)** — `press_and_release_in_one_drain_survives_as_edges`
  (the long-frame case: a press+release in one drain survives as ordered edges even
  though held-state looks untouched); `auto_repeat_does_not_push_duplicate_edges`
  (a held key re-delivered by the OS is not a fresh press);
  `repeated_taps_in_one_drain_all_survive`; `mouse_click_in_one_drain_survives_as_edges`;
  `mouse_left_press_edge_fires_once`; `key_and_backspace_and_typed_replay`;
  `drain_replays_then_clears_buffer` (wheel accumulates, motion is a delta off the
  retained position, buffer empties); `translate_key_maps_letters_and_specials`
  (CapsLock intentionally unmapped); `translate_mouse_button_maps_all_five`.
- **Gamepad (`gamepad.rs`)** — `discrete_writes_held_buttons_and_axis_bridge_to_slot0`;
  `single_player_touches_slot0_only`; `disconnect_removes_slot0`;
  `axes_map_to_analog_frame`; `neutral_on_disconnect_yields_zero_frame`;
  `reset_neutral_clears_held_state`; `caps_all_vs_default`.
- **Last-used monitor (`monitor.rs`)** — `kbm_wins_a_tie_with_a_held_stick`;
  `a_stick_reads_as_pad_only_past_the_threshold`;
  `a_resting_trigger_axis_is_not_pad_activity` (resting triggers never pin "pad");
  `a_held_pad_button_carries_the_detected_vendor`; `wheel_or_motion_reads_as_kbm`;
  `a_resting_frame_reports_no_activity`; `from_metadata_classifies_vendors`;
  `tokens_are_stable`.

There is **no** test over the runner's drain/`note_frame`/`clear_frame_edges`
ordering — that contract lives in `flicker-app` and is enforced only by reading.

## Sharp edges

- **It is the *device* layer.** No signals, no edge classification, no keys→actions
  here. If you want "is `Jump` active?", you are one crate too low — use
  [`flicker-input-core`](../flicker-input-core/README.md).
- **Held-state alone is lossy; edges are not.** `drain_into` records an ordered
  edge for every state change, so a press+release inside one long frame still
  fires. Read `InputState::pressed/released`, not just `key_down`.
- **Two axis representations exist at once** (analog latch + RT-12 discrete
  bridge). Pick one channel per signal; binding both double-drives it.
- **"120 Hz" is aspirational** — sampled at frame rate today.
- **`caps()` / `connected()` / `DeviceCaps` are not wired yet** — tools for the
  settings/tester surface, not dead code; `DeviceCaps` is all-or-nothing today.
- **Single local player only** — slot 0; extra pads ignored.
- **`last_input_context()` is one process-global** shared across all scenes; a
  poisoned lock falls back to `Kbm` silently.
- **Vendor art gap** — only Xbox glyphs exist today, so a PlayStation pad resolves
  to Xbox glyphs at the display layer even though `vendor()` reports it correctly.
  That is an atlas concern, not this crate's.
