# flicker-flight

The engine's **camera-cinematic** service. A *camera cinematic* is a scripted
camera move authored as data (a `.flight` file) and replayed by a runtime — the
same idea as a `.pack`, but its own file type because a camera path is neither a
skeleton/skin (`.rig`) nor an animation state graph (`.pack`). This crate parses
a `.flight` and plays it back, handing the caller a camera **pose** each frame. It
is **render-agnostic**: it emits poses (yaw/pitch/distance about a target point),
never a `Camera`, so the client maps them onto its own camera. That keeps the
crate usable from tools and headless tests, with no GPU dependency.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Cluster:** `animation` (it animates the camera), parallel to `flicker-skeletal`.
- **Builds on:** `flicker-core` — only for the gz-at-rest content read seam
  (`flicker_core::compression::read_text`). Otherwise pure serde/anyhow.
- **Used by:** `flicker-solarbirth` (the intro "Sim" scene) embeds a
  [`FlightPlayer`](#playback----flightplayer) and feeds its pose to the scene's
  orbit camera every frame; it also reads [`progress()`](#playback----flightplayer)
  to drive the dust/reveal clock so the fly-in and the scene stay in step.
- **Sibling file type:** `flicker-worldengine`'s `.epoch` deliberately mirrors this
  crate's load/parse/validate shape (a `format`/`version` header + segments), so
  the two file types feel alike; it is a *different concept* (a captured world,
  not a camera path) and does not depend on this crate.

### Reads from the content tree

`Flight::load(path)` reads whatever **logical path** you hand it, gz-transparently
(the `<path>.gz` at-rest twin is tried first, the raw path is the dev fallback —
see `flicker_core::compression`). Camera cinematics ship under the package root:

| Path | When read | If missing |
|---|---|---|
| `Alpha/content/package/flights/<name>.flight` (at-rest `…/<name>.flight.gz`) | at scene construction, by logical path via `flicker_core::roots::roots().package()` | `Flight::load` returns `Err` ("reading .flight …"); `flicker-solarbirth` turns that into a panic at scene start (loud, by design) |

The crate never hardcodes a path — the location above is the shipped **convention**,
resolved by the consumer, not by this crate.

## The `.flight` format (JSON)

A flight is a `target` (the orbit centre / look-at point) plus an ordered list of
**segments**. Each segment eases from one pose to another over a `duration` in
seconds. The **last** segment may `loop` — an endless idle "coast" tail (e.g. a
slow orbit) so a cinematic never dead-stops.

```json
{
  "format": "flicker.flight", "version": 1,
  "target": [0.0, 0.0, 0.0],
  "segments": [
    { "name": "glide", "duration": 36.0, "ease": "smooth_step",
      "from": { "yaw": 0.10, "pitch": -0.24, "distance": 45.0 },
      "to":   { "yaw": 1.00, "pitch": 0.52,  "distance": 6.24 } },
    { "name": "coast", "duration": 251.33, "ease": "linear", "loop": true,
      "from": { "yaw": 1.00,     "pitch": 0.52, "distance": 6.24 },
      "to":   { "yaw": 7.283185, "pitch": 0.52, "distance": 6.24 } }
  ]
}
```

### Fields

| Field | Type | Meaning | Notes |
|---|---|---|---|
| `format` | string | Format tag | **Parsed but never checked** — advisory only (see Sharp edges). Optional; defaults to `""`. |
| `version` | number | Format version | **Parsed but never checked** — advisory only. Optional; defaults to `0`. |
| `target` | `[f32; 3]` | Orbit centre / look-at, world space | Optional; defaults to `[0,0,0]`. |
| `segments` | array | Ordered legs of the move | **Required, non-empty.** |
| `segments[].name` | string | Label for HUD/debug | Optional; defaults to `""` (empty label — see Sharp edges). |
| `segments[].duration` | f32 seconds | Length of this leg | **Required, must be > 0.** |
| `segments[].ease` | enum | Time-warp curve (see below) | Optional; defaults to `linear`. |
| `segments[].loop` | bool | This leg wraps forever | Optional; defaults `false`. **Only the final segment may be `true`.** |
| `segments[].from` / `.to` | pose | Start / end pose of this leg | **Required.** A pose is `{ "yaw", "pitch", "distance" }` — yaw & pitch in radians, distance from `target`. |

### Ease curves

Written as the `ease` string; interpolation runs on the eased, normalised time.

| `ease` value | Curve | Feel |
|---|---|---|
| `linear` (default) | `t` | constant speed |
| `smooth_step` | `3t²−2t³` | gentle start *and* arrival |
| `ease_in` | `t²` | slow start, fast finish |
| `ease_out` | `t·(2−t)` | fast start, gentle finish |

An unknown `ease` (or a non-final `loop`, an empty `segments`, or a `duration ≤ 0`)
is a **loud parse/validation error** — `Flight::load` / `from_json` return `Err`.

## Playing it

```rust
let flight = flicker_flight::Flight::load("flights/intro.flight")?;
let mut player = flicker_flight::FlightPlayer::new(flight);
// each frame, while the cinematic is driving the camera:
let pose = player.advance(dt_seconds);
// map the pose onto YOUR camera (orbit convention lives in flicker-render):
camera.set_pose(pose.yaw, pose.pitch, pose.distance);
```

## Public API

### `Flight` — parse & query (immutable)

| Item | For | The one thing to know |
|---|---|---|
| `Flight::from_json(&str) -> Result<Flight>` | Parse + validate from JSON text | Validation runs here (non-empty, positive durations, only-last-loops). |
| `Flight::load(path) -> Result<Flight>` | Read a `.flight` file, then `from_json` | Gz-transparent (`.gz` twin first, raw fallback). Missing file → `Err`. |
| `Flight::target() -> [f32; 3]` | The orbit centre / look-at | Same value as the public `target` field (a convenience accessor). |
| `Flight::loops() -> bool` | Does it end in a looping tail? | `true` ⇒ plays forever. |
| `Flight::total() -> Option<f32>` | Total run length (seconds) | `None` if it loops forever. |
| `Flight::lead_in() -> f32` | The finite one-shot run: sum of every segment **before** a looping tail | This is the clock a scene's reveal/dust effect tracks. |
| `Flight::pose_at(time) -> OrbitPose` | The interpolated pose at `time` seconds | Past a non-looping end holds the final pose; a looping tail wraps within its own duration. |
| `Flight::segment_at(time) -> &str` | Name of the segment active at `time` | Returns `""` for an unnamed segment. |
| `Flight::segments` (field) | The parsed segments | Public for inspection. |
| `Flight::target` (field) | Orbit centre / look-at | Public; equal to `target()`. |

### Playback — `FlightPlayer`

Stateful wrapper: it holds the elapsed clock, you `advance` it each frame.

| Item | For | The one thing to know |
|---|---|---|
| `FlightPlayer::new(flight) -> Self` | Wrap a `Flight`, clock at 0 | — |
| `advance(dt) -> OrbitPose` | Step the clock by `dt` seconds, return the pose | Negative `dt` is ignored (no rewind); a non-looping flight clamps at its end. |
| `pose() -> OrbitPose` | Current pose without advancing | — |
| `restart()` | Reset the clock to the opening pose | Clears the finished flag. |
| `is_finished() -> bool` | `true` once a **non-looping** flight reaches its end | Never `true` while looping. |
| `elapsed() -> f32` | Seconds since start / last restart | — |
| `progress() -> f32` | Lead-in progress `0..1` (1 once the finite lead-in completes) | Drives a scene's dust/reveal clock. `1.0` immediately if there is no finite lead-in. |
| `segment_name() -> &str` | Name of the currently-active segment | e.g. `"glide"` / `"coast"`; `""` if unnamed. |
| `flight() -> &Flight` | The underlying flight (e.g. for `target()`) | — |

### Value types

| Item | For | The one thing to know |
|---|---|---|
| `OrbitPose { yaw, pitch, distance }` | The pose the runtime interpolates and hands back | `yaw`/`pitch` radians; `distance` from `target`. All `pub f32`. |
| `Ease` | The per-segment easing curve | `apply(t) -> f32` maps normalised time through the curve (clamps `t` to `[0,1]`). |
| `Segment { name, duration, ease, looping, from, to }` | One leg of a flight | The JSON key is `loop`; the Rust field is `looping` (`loop` is a reserved word). |

## Interactions

- **Signals / results / Model keys:** **None.** This crate captures no input
  signals, fires no results, and publishes nothing into the Model. It hands the
  caller `OrbitPose` values (and `Flight::target()`); wiring those to a camera and
  publishing any HUD keys (`segment`, `progress_pct`, …) is the **consumer's** job
  — `flicker-solarbirth` does exactly that in its scene.
- **Threads / async:** none — synchronous, allocation-free per frame.

## Gates

The crate's tests (`cargo test -p flicker-flight`) are the drift gates:

| Test | Enforces |
|---|---|
| `parses_and_bounds_the_endpoints` | A sample parses; `loops`/`total`/`lead_in` agree; `pose_at` hits the authored endpoints. |
| `progress_and_segment_track_the_fly_in` | `progress()` climbs 0→1 across the lead-in; `segment_name()` follows; a looping flight never `is_finished`. |
| `coast_loops_seamlessly` | The looping tail wraps: pose one full period into the loop matches the loop start. |
| `rejects_a_non_final_loop` | A `loop` on a non-final segment fails validation. |
| `rejects_empty_and_bad_duration` | Empty `segments`, and `duration ≤ 0`, both fail validation. |

The bundled asset itself is guarded by the **consumer**: `flicker-solarbirth`'s
`bundled_intro_flight_loads` test loads `flights/intro.flight` so a typo surfaces
at test time, not only as a runtime panic.

## Sharp edges

- **`format` and `version` are advisory — parsed but never validated.** A `.flight`
  with a wrong `format` tag, or a `version` you bump expecting new parsing, loads
  silently and is interpreted as-is. There is no version gate.
- **An omitted `name` gives an empty HUD label.** `segment_name()` / `segment_at()`
  return `""` for an unnamed segment, not a fallback like the index — a blank shows
  up in the HUD with no error.
- **Only the *final* segment may loop**, and it wraps within its own `duration`.
  Author the loop's `from`/`to` so the endpoints meet (e.g. a full `2π` yaw sweep)
  or the wrap will visibly jump.
- **`total()` is `None` for a looping flight** — guard for it before treating the
  return as a finite length.
- **`progress()` needs a finite lead-in.** A flight that is *only* a looping tail
  has `lead_in() == 0`, so `progress()` reads `1.0` from `t = 0`.
- **Poses are handed off render-agnostic.** The orbit convention (how yaw/pitch/
  distance become a view matrix) is single-sourced in `flicker-render`
  (`Camera::orbit`), not here — feed the pose through it, don't reinvent the math.
