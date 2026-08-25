# flicker-solarbirth

The **Solar Birth** intro cinematic — "the birth of the Prism system." A camera flies in
from far below a dissipating dust cloud; the cloud clears inside-out to reveal the fixed
Prism roster (the sun, eight planets, and Home's moon) slowly orbiting. It is a *graphical
scene*: a full-window 3D **surface** (a rectangle the UI reserves and this crate draws into)
under a small readout panel. This crate is a **library only** — no binary — that the
`prism-alpha` launcher registers as one entry in its **roster** (the list of playable
scenes) and runs like any other scene.

It is a **cinematic, not a simulation**: the camera choreography is the deliverable, the
planets are cosmetic. Nothing here accretes or collides — the system already exists.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

Cluster: `Alpha/crates/scenes` — a leaf scene crate. It composes engine services; nothing
builds on it except the launcher.

- **Builds on:**
  - [`flicker-flight`](../../animation/flicker-flight/README.md) — the camera-cinematic
    service. `Flight::load` + `FlightPlayer` play the authored `.flight` that drives the
    camera pose *and* the dust-clearing clock (`progress()`).
  - [`flicker-orrery`](../../world/flicker-orrery/README.md) — the GPU-free planet-layout
    model (roster, orbits, `BodyKind`, `SYSTEM_INNER`/`SYSTEM_OUTER`). Re-exported through
    this crate's private `system` module, so the layout is shared, never copied.
  - `flicker` — the engine core: `render` (FrameGraph, StageDef, meshes), `scene`
    (`Scene`/`Transition`), `script` (the Lua host + `Model`), `ui` (the walker, `stage_def`).
  - `flicker-core` — the content-roots service (`roots()`), which resolves the bundled flight.
  - `flicker-shell` — `PauseScene` + `Theme` for the pause overlay, and
    `input_profile()` / `publish_signal_bindings()` for device-adaptive control hints.
  - `flicker-input-core` / `flicker-input-router` — `ActionSignal`, `InputContext`, and the
    `Router`/`InputHandler` event bus. (Signal vocabulary lives in `flicker-input-core`.)
- **Used by:** `prism-alpha` — `main.rs:115` registers the roster factory:
  `SceneEntry::new("solarbirth", "Solar Birth", "primary", flicker_solarbirth::scene)`.

### Content it reads (the human-editable half)

Author these with the guides linked; this crate never re-teaches their formats. A *pair
script* is the `SceneName.lua` half of a scene; the *scene file* is its `.scene.json` tree.

| Path (under `Alpha/content/sensorium/`) | When read | If missing / broken |
|---|---|---|
| `scenes/solarbirth.scene.json` | Parsed by the kernel; the `SceneDef` is handed to `scene()` | Kernel-level failure (the roster can't build the scene) |
| `scripts/solarbirth.lua` | **Compile-time** `include_str!` — baked into the binary | Load error is logged; the phase line falls back to raw numbers (gated) |
| `flights/intro.flight` | **Runtime**, at scene construction (retune without recompiling) | **Panics** at construction (gated at test time) |
| `resources/ui_theme.json` + `resources/ui_stages.json` | At `enter`, merged with the scene file's own `styles`/`stages` | The stage `source` fails to compile → no 3D is drawn (logged) |

Authoring guides: the scene file + pair script →
[`content/sensorium/README.md`](../../../content/sensorium/README.md); the stage recipe (the
sky + dust passes) → [`content/sensorium/STAGES.md`](../../../content/sensorium/STAGES.md)
(its *Worked example: Solar Birth* is this scene's stage).

## Public API

The externally reachable surface is deliberately tiny — this crate is invoked through the
roster, not called item-by-item.

| Item | What it is for | The one thing to know |
|---|---|---|
| `scene(def: &SceneDef) -> Box<dyn Scene>` | The **roster factory** — the client behaviour the launcher dispatches. | The only intended entry point. It just boxes `Sim::new(def)`. |
| `Sim` (re-export of `scene::Sim`) | The scene type `scene()` builds and the runner drives (`enter`/`update`/`render`/`exit`). | Named `Sim` for historical reasons — it is a **cinematic, not a simulation** (see Sharp edges). |

Everything else — the orbit camera (`OrbitCam`), the mesh builders (`uv_sphere`,
`ring_mesh`, `pack_rgb`), and the input-root handler (`RootHandler`) — is `pub` only *within*
private modules (`camera`, `system`, `route`). None of it is reachable from outside the
crate; treat it as implementation, not API.

## Interactions

### Input contexts (which control scheme is live)

The scene is a **flight-camera vehicle** with two modes, declared frame-by-frame via
`Scene::input_context()`; the central input **pump** resolves the matching device→signal map,
so this crate owns **no** resolver or bindings of its own.

- **`FlightPath`** — on the rail. The flight drives the camera; the left stick / `MoveForward`
  is **throttle**.
- **`Flying`** — off the rail (free camera). The left stick / `ZoomIn` is **dolly**; look pans.

A look gesture on the open sky flips the mode to `Flying`; `Interact` (replay) returns to
`FlightPath`. The read happens *before* `update`, so a mode flip inside `update` takes effect
next frame (a 1-frame skew the scene owns).

### Signals it captures (by name — never keys)

Specs and docs name **signals**, never the keys/buttons a profile binds to them
([DFE3E44E](../../../content/sensorium/README.md)). All of these are resolved by the pump;
this scene reads signals, never a device.

| Signal(s) | Effect | Channel |
|---|---|---|
| `Menu` | Open the pause overlay | Declared `on_menu = "pause_open"` on the scene-file root; consumed by the walker layer |
| `Interact` (press) | **Replay** the fly-in (restart, re-enter the rail) | The pump's `signals.events` |
| `LookUp` / `LookDown` / `LookLeft` / `LookRight` | Orbit the camera; on the rail, the first look **drops out** to the free camera | Continuous: `signals.axis` (stick, a rate) + `signals.pointer_delta` (mouse **right**-drag; left-click stays free) |
| `MoveForward` / `MoveBackward` | **Throttle** the fly-in (0.25×–5×) | `signals.axis`; active only on the rail (`FlightPath`) |
| `ZoomIn` / `ZoomOut` | **Dolly** the free camera | `signals.axis`; active only off the rail (`Flying`) — binds to nothing on the rail |
| `Confirm` | Nothing — deliberately (a cinematic must not steal the menu's activation signal) | — |

The signal *is* the intent — there is no separate intent router. An `on_<signal>` in the
scene file is a capture declaration, not a mapping into a second vocabulary.

### Results it fires

| Result | Routed to |
|---|---|
| `pause_open` | The only declared intent (screen root `on_menu`, and the footer MENU button's `action`). The scene maps the fired name onto `Transition::Push(PauseScene)`. |

Fired result names are also mirrored once into the next Model as the transient `sig_<name>`.

### Model keys

The **Model** is the per-frame key→value table the engine hands to the pair script and the
walker binds against. This scene **publishes** (from `hud_model`) and the scene file / emitted
rows **bind**:

| Key(s) | Published by | Bound by |
|---|---|---|
| `segment`, `progress_pct`, `sys`, `approaching`, `settled` | this scene (raw flight vars + resolved copy tokens) | the pair script's `derive()`, which composes `phase` |
| `phase` | the pair script (`solarbirth.lua`) | scene file `text_bind: phase` |
| `roster_1` … `roster_N` | this scene (one pre-formatted legend row per planet) | the emitted legend rows (`text_bind: roster_<i>`) in the `roster_legend` container |
| `bind_Interact`, `glyph_Interact`, `bind_Menu`, `glyph_Menu`, `input_device` | `flicker_shell::publish_signal_bindings` for `[Interact, Menu]` | the footer/tooltip nodes carrying `signal: Interact` (device-adaptive keycap/glyph). **`bind_Menu`/`glyph_Menu` are currently unconsumed** — see Sharp edges. |
| `sig_<name>` | this scene (transient mirror of last frame's fired results) | — |

### Stage inputs (the per-frame channel into the recipe)

The 3D **surface** the scene file names (`solarbirth_view` → stage source `solarbirth_sky`)
is a **recipe**: an ordered set of passes the one stage compiler builds. Draw order is
*derived from what each pass reads/writes*, never authored as a number; for this scene it is
**sky → scene → volumetric_disk → tonemap_grade**. This crate contributes the bodies and
orbit rings inside the `scene` pass and **binds** the two numbers only the cinematic knows
(via `dust_inputs`):

| Bind key | Value | Recipe field |
|---|---|---|
| `dust_formation` | the fly-in's `progress()` (inside-out dissipation) | `formation_bind` |
| `dust_time` | `progress() * 10` (swirl clock) | `time_bind` |
| annular gaps | carved at each giant's orbit | the typed `gaps` channel (no file authors it) |

### What it hands the frame graph

An offscreen `surface` pass rendering the recipe, composited into the `solarbirth_view` rect,
then a HUD `overlay` (the readout panel + nav footer) drawn over it.

## Gates

`cargo test -p flicker-solarbirth` — 7 tests, all green:

| Test | What it locks |
|---|---|
| `the_view_names_a_stage_whose_recipe_draws_the_sky_and_the_dust` | The shipped scene file parses; its `source` compiles clean; the derived pass order is exactly sky → scene → volumetric_disk → tonemap_grade; the disk radii track the orrery (`inner == SYSTEM_INNER`, `outer == SYSTEM_OUTER × 1.4`); the grade is static (no binds); and every `*_bind` the recipe names is a key `dust_inputs` publishes. |
| `bundled_intro_flight_loads` | `flights/intro.flight` parses (2 segments — glide + coast — and the coast tail loops), so a typo fails at test time, not as a runtime panic. |
| `the_pair_script_derives_the_phase_line` | `solarbirth.lua` loads and its `derive()` composes `phase` from the raw flight vars (guards against silent raw-number breakage). |
| `hud_tree_is_well_formed_and_draws_the_roster` | The scene tree names no unknown kinds; ships no raw display literals (tree **and** Rust-published Model copy); declares `on_menu → pause_open`; renders one row per planet; the readout panel claims the pointer; and `solarbirth_view` reserves a real-extent viewport slot. |
| `root_declares_flightpath_and_consumes_nothing` | The input root declares `FlightPath` and consumes no signal arms (pause is data, not a hardcoded arm). |
| `dispatch_fires_the_declared_pause_intent` | A `Menu` press through the real 2-layer chain fires `pause_open` at the walker layer, not the root. |
| `direct_rgb_escape_round_trips` | The `pack_rgb` direct-colour packing (bit 31 + RGB888) round-trips. |

## Sharp edges

- **`Sim` is a misnomer.** The public scene type is named `Sim`, but the design of record is
  emphatic that this is a cinematic, **not** a simulation. Read it as "the Solar Birth
  scene." (Flagged as a docs finding — a rename is Aaron's call.)
- **Node-id coupling to the scene file, silent if broken.** The behaviour finds the
  `solarbirth_view` surface and the `roster_legend` container by their **exact ids** in
  `solarbirth.scene.json`. Rename either in the content file and the 3D viewport / planet
  legend silently vanish at runtime (only a bad stage `source` logs an error). The *shipped*
  file is gated, so this bites only someone editing the scene file.
- **The dust radii are derived and gated.** The recipe's `inner` / `outer`
  (`0.4` / `21.7` in the scene file) must equal `SYSTEM_INNER` and `SYSTEM_OUTER × 1.4`.
  Retune the orrery's extent and you must retune these, or
  `the_view_names_a_stage_whose_recipe_draws_the_sky_and_the_dust` fails (the derivation is
  manual — see the scene file's `_comment`).
- **Failure modes vary by asset.** A missing `intro.flight` **panics** at construction; a
  broken `solarbirth.lua` degrades to raw numbers (logged); a stage `source` that doesn't
  compile draws no 3D (logged). The flight is the loud one.
- **`bind_Menu` / `glyph_Menu` are published but read by nothing** — the footer MENU button
  uses `action` + a text label, not `signal: Menu`. Harmless, mildly wasteful. (Docs finding.)
- **The camera is the maintainer's, not the stage's.** The recipe deliberately authors no
  framing; the scene's `OrbitCam` owns the view in both modes.

## Related scenes

[`flicker-clicktrainer`](../flicker-clicktrainer/README.md) is the sibling input-P3 scene —
the simpler case (a `World`-context bench with discrete edges and no continuous camera).
