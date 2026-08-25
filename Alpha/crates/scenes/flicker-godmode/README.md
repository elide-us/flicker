# flicker-godmode

**God Mode** — the world-simulation console, and the reference **developer/planet-inspector
bench**. It watches one planet evolve from a molten ball toward something that could hold
life, and lets the maintainer steer that formation without ever writing a result into it:
orbit a layer-shell globe, recolour it by any of ten field views, play/step/reseed, hold or
release the chemistry's own process gates, dial a rack of physics levers, read the five-axis
life-supporting verdict, and — once the world is alive — drop into one hex cell and rain
pixel-scale erosion on it. The simulation itself is GPU-free and runs on its **own thread**
([`flicker-poc-chemistry`](../../world/flicker-poc-chemistry/README.md)); this crate is only
its window and its controls. It is a scene **package** — a library that supplies one `Scene`
behaviour, launched by name from the `prism-alpha` roster; there is no binary.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Vocabulary used below

Each is a flicker word, not a general one; defined once here.

- **bench** — a scene/tool app in the roster (God Mode is one). Also called the *scene*.
- **scene pair** — the two authored files that define a bench under the five-line
  architecture: a **scene file** (`*.scene.json`, the component **tree** + anchors + this
  bench's style/stage blocks) and its same-named **pair script** (`*.lua`, the logic that
  turns raw numbers into display words). See
  [`content/sensorium/README.md`](../../../content/sensorium/README.md) for the format — this
  file does not re-teach it.
- **Model** — the per-frame key→value table the engine hands to the pair script and to the
  tree's `*_bind`s. This bench publishes a *raw* Model each frame; the pair script derives a
  *display* Model over it; the tree binds the merge.
- **walker** — the Rust pass ([`flicker-widgets`](../../frontend/flicker-widgets/README.md))
  that lays out the tree, hit-tests the pointer, owns the focus/nav graph, and fires declared
  intents.
- **signal** — a device-independent input verb (`Menu`, `Confirm`, `Cancel`, `LookUp`, …;
  catalog in [`flicker-input-core`](../../input/flicker-input-core/README.md)). Nothing in
  this bench is wired to a key or a button — only to signals (rules 37722F91 / DFE3E44E).
- **intent** — a signal binding declared as data on the scene tree (`"on_menu":
  "pause_open"`); the walker fires the named **result** when that signal arrives.
- **result** — a name in the frame's results map, fired by an intent or by a component's own
  `action`/`bind`. God Mode's dispatcher reads results and turns them into scene state changes
  or sim commands.
- **surface / pane** — a live-drawing region the walker reserves (here the globe's viewport);
  a **pane** (`"pane": true`) is one the maintainer can *enter* to give it focus.
- **stage** — the authored look a surface is drawn with (clear colour, lights, draw layers),
  compiled from the scene file's `stages` block. God Mode's is `stages.godmode_globe`.
- **shell** — one layer of the globe: a sphere of hex patches at a radius, coloured per cell.
  A world is a *stack* of shells (core · mantle · crust beds · a veil per gas).
- **lever** — a live condition on the running world (a rate multiplier or a water budget).
  Moving one changes how the world evolves **from here**; it never edits what already is.
- **process gate** — a stage in the chemistry pipeline is *gated* on the world's own state
  (`ready`); the maintainer can additionally **hold** it shut. A gate *moving* is the world
  crossing a threshold — the sim pauses itself there.
- **forge** — birth a brand-new planet from the Starter's pending size + endowment knobs. The
  transport's RESEED and the Starter's FORGE button are the same act.
- **Snapshot** — one published frame of sim state the render thread reads (see the seam below).

## Where it sits

**Cluster:** `Alpha/crates/scenes` (the roster benches). It is the largest of them.

- **Builds on:**
  - [`flicker-poc-chemistry`](../../world/flicker-poc-chemistry/README.md) — the GPU-free
    world simulation this bench drives and reads: `World`, `Scheduler`, `Levers`,
    `PlanetState`, `Habitability`/`BANDS`, `ProcessState`/`ProcessDef`, the observers
    (`PlateObserver`, `Weather`, `air_shells`, `enrichment`, `observe_habitability`), the
    formation pipeline (`formation_stages`, `load_processes`), and the size/seed constants
    (`PLANET_FREQ`, `NOMINAL_DT_MYR`, `Budget`).
  - [`flicker-worldtile`](../../world/flicker-worldtile/README.md) — the **pixel tier**: the
    tile inspector calls `materialize` (with `radius_for_freq`), and the erosion worker drives
    an `Eroder`/`ErosionParams` over the resulting `Tile`, banking a `PassReport`.
  - [`flicker-globe`](../../frontend/flicker-globe/README.md) — the **one shared globe**:
    `GlobeWorld` (the mesh stack + offscreen target + orbit camera + the pane-focus gate),
    `ShellSpec`, `look_from`, `graticule`, `in_wedge`, `RADIUS`. God Mode publishes what the
    planet is *made of*; the globe owns the *picture* of it. **The camera is the globe's, not
    this bench's** — read that crate for how a signal (never a device) flies the planet.
  - [`flicker-worldgrid`](../../world/) — `icosphere_with_outlines` (the topology + per-cell
    boundary polygons the sim thread keeps).
  - [`flicker-materials`](../../content/) — `Tables` (the periodic table + compound catalog),
    loaded once on the sim thread.
  - `flicker` (core umbrella) — `Scene`/`Transition`/`SceneInput`, `Renderer`/`FrameGraph`,
    `ScriptHost`/`ValueMap`, and the walker entry points (`run_ui`, `render_hud`, `SceneDef`,
    `UiIntents`, `UiState`, `stage_def`, `strings`).
  - [`flicker-shell`](../../) — `PauseScene`, `Theme`, `input_profile` (the Menu-key pause
    overlay).
  - [`flicker-input-core`](../../input/flicker-input-core/README.md) /
    [`flicker-input-router`](../../input/flicker-input-router/README.md) — the signal bus and
    the `InputHandler`/`Router` seam the walker sits on.
  - `glam`, `serde_json`, `tracing`.
- **Used by:** `prism-alpha` only, through [`scene`](#public-api). Its roster entry is in
  [`../../../prism-alpha/src/main.rs`](../../../prism-alpha/src/main.rs) — id `"godmode"`,
  title `"God Mode"`, realm `REALM_GAMEMASTER` (grouped with Populous and the Epoch
  Simulation; realm placement is a maintainer call and may be re-ruled).
- **Reads from the content tree:**

| Path | When | If missing |
|---|---|---|
| [`content/sensorium/scenes/godmode.scene.json`](../../../content/sensorium/scenes/godmode.scene.json) | at launch, by the kernel — the parsed `SceneDef` is handed to `scene()` | no `tree` ⇒ `tracing::error!` and no HUD draws (the tree is gated behind `loading`/`loaded` binds) |
| [`content/sensorium/scripts/godmode.lua`](../../../content/sensorium/scripts/godmode.lua) | compiled in via `include_str!` (`src/scene.rs`), loaded in `GodModeScene::new` | load error ⇒ `tracing::error!` and the tree binds raw state (no derived words/glyphs/style paths — the console reads blank) |
| [`content/sensorium/resources/ui_theme.json`](../../../content/sensorium/resources/) | in `new`, via `load_shared_styles` — merged under the scene file's own `styles`/`stages` | `$token` colours fall back to compiled defaults, silently |
| [`content/data/stringtable.json`](../../../content/data/) | per frame — every `$chem_*` token this bench resolves | the raw token text draws (loaded globally by the app, not by this crate) |
| `content/data/*` (accretion, processes, elements, compounds) | on the **sim thread**, via `flicker-poc-chemistry` (`content_data_dir()`) | that crate's failure modes — this bench reads `processes.json`'s `view`/`summary`/`watch` for the tab strip and gate cards (see [the view roster](#the-ten-field-views)) |

## Public API

An external caller (the roster) sees **two** items — everything else is `pub` inside
**private modules** (`sim_thread`, `globe_view`), so it is the crate's internal contract, not
its external surface. It is documented under [Internal architecture](#internal-architecture-the-render--sim-seam)
because that is where the bench's real behaviour lives.

| Item | For | The one thing to know |
|---|---|---|
| `pub fn scene(def: &SceneDef) -> Box<dyn Scene>` | the roster factory — the only intended entry point | The `SceneDef` is the *parsed scene file*; the kernel resolves it from the manifest when the menu row fires. Spawns the sim thread immediately (a loading banner shows until the topology arrives). |
| `pub struct GodModeScene` (+ `::new(def)`) | the `Scene` implementation; all frame state lives here | Construct via `scene`/`new(def)`. `Default`/`shipped()` **panic outside `cfg(test)`** (a def-less bench is a blank screen, so it fails loud) — never build one that way in production. |

The `Scene` methods it implements: `enter` (GPU + the generated legend styles), `update`
(walk → dispatch → drive the sim + camera), `render` (rebuild shells → draw the globe → 2D
overlay), `exit` (free the globe; the sim thread stops when the handle drops).

## Internal architecture: the render ↔ sim seam

The simulation never runs inside a frame — the per-cell stages plus the every-tick
conservation audit froze the app at ~92k cells. So the world lives on its own thread and the
two sides talk over one command channel, one static-data channel, and two mutex-guarded slots
(the latest `Snapshot`, and the latest tile preview). These types are `pub` but crate-internal
(private modules); they are the contract between `scene.rs` (render thread) and `sim_thread.rs`
(sim thread), not an API another crate calls.

| Type | Role | The one thing to know |
|---|---|---|
| `SimHandle` | the render thread's handle: `spawn(seed)`, `send(cmd)`, `take_static()`, `take_tile()`, `latest_if_newer(gen)` | `latest_if_newer` clones the snapshot only when the `gen` counter advanced — no 92k-cell clone on a still frame. Dropping the handle shuts the thread down. |
| `SimCommand` | what the render thread asks the sim to do | `TogglePlay` · `Reset` (same planet, t=0) · `Reseed(SeedSpec)` (a new planet) · `SetLevers(Levers)` (rebuilds the pipeline) · `SetRate(f32)` · `Hold{stage,held}` · `Inspect(u32)` (materialise one cell) · `ErodeToggle` (rain on the tile) · `Shutdown`. |
| `Snapshot` | one published frame of world state | Carries a monotonic `gen` (survives a reset-to-t0, unlike `tick`), the `PlanetState`, per-cell `CellView`s, `processes`, `levers`, `habitability`, the merged plate + gate event log, and the classified air. The dial/label fields (`playing`, `rate_hz`) are **echoes** — what the sim is *actually* doing, so a clamped request springs back instead of lying. |
| `StaticData` | topology + immutable readouts, sent at spawn and re-sent after every forge | `dirs`/`outlines` (a new size is a new mesh), the bulk-seed element distribution, the gas-name catalog, the loaded `ProcessDef`s (whole — so the file the maintainer edits *is* what the bench shows), and the Starter's element roster. |
| `CellView` | compact per-cell render data | Which shell a cell lights is its **buoyancy** (`crust_kind`), not the provenance of its beds. Carries temp, differentiation, plate id, seam class, strata count, elevation, ore enrichment, `coast` class, rain, and the plate-step heading. |
| `coast_class(kind, elev_m, sea_m) -> u8` | pure classifier: the four grounds (land · shelf · deep bed · exposed floor) + bare mantle | Pure, so it is tested without a planet. `SHELF_*` are its class codes; `SHELF_EDGE` marks a cell whose neighbour is a different class (the coastline). |
| `TilePreview` | a materialised tile reduced to an RGBA image + caption | Lives in its own slot so a ~100 MiB materialisation is never cloned with the per-frame snapshot. |
| `SeedSpec` | a forge order: `seed`, `freq` (planet size in cells), per-element `scales` | t=0 boundary conditions — dialling them moves nothing until FORGE births a fresh world. |

The sim thread also owns a second background thread — the **erosion worker** (`Eroder` over
~3.5 M pixel-clusters at a restrained cadence) — so a rain pass belongs to neither the render
nor the sim thread.

## Interactions

### Signals it captures

Signals only — never keys or buttons (what produces a signal is profile data, out of scope).
There are three channels, and **there is no intent router**: a component captures the signals
it cares about, and the intent is implied by the signal.

| Signal | Channel | Fires / does |
|---|---|---|
| `Menu` | **declared intent** on the scene root — `"on_menu": "pause_open"` | pushes the shell's `PauseScene` (settings/quit). *Not* the same as the sim's own gate-pause card — see Sharp edges. |
| `PanelNext` / `PanelPrev` (tab) | **declared intents** — `"on_tab_next": "field_next"`, `"on_tab_prev": "field_prev"` | cycle the globe's field view forward / back. |
| `Confirm` (pointer click **is** a Confirm at what it hits, rule 37722F91; or nav-to-node + Confirm) | the walker fires the focused/hit node's own `action`/`bind` as a result | every button, checkbox and slider below. |
| `LookUp/Down/Left/Right`, `ZoomIn/Out` | captured by the **globe pane** while `gm_view` holds focus | orbit/zoom the planet. Wired entirely inside [`flicker-globe`](../../frontend/flicker-globe/README.md) via `GlobeWorld::look_from` + the pane-focus gate; this bench names no device. |
| `Cancel` | the walker's built-in back-out (no `on_cancel` is declared) | pops one entered pane level, else pops the scene context. **It does not close any popup** — see Sharp edges. |

### Results it fires (the action catalog)

The dispatcher (`apply_results`, `src/scene.rs`) reads every result — from either channel,
identically — in one place, and **returns** the sim commands it wants rather than sending
them (which is what lets the gates assert what a control *did*). View toggles and pending
Starter knobs are scene-local; the rest become `SimCommand`s.

| Result(s) | Fired by | Effect |
|---|---|---|
| `toggle_play` · `reset` · `reseed` | transport buttons | `TogglePlay` · `Reset` · `forge()` (new planet). `reset`/`reseed` also clear the gate-ack high-water so the next run's first gate pauses-and-tells. |
| `rate` | the rate slider (`bind`) | `SetRate`, guarded against its own echo (a drag does not send 60 commands/s). |
| `field_temperature` … `field_ore` (10) · `field_next` · `field_prev` | the field tab strip + the tab intents | select / cycle the view. Scene-local — no command leaves; only a mesh rebuild. |
| `cut` · `air` · `grid` | checkboxes (`bind`) | globe overlays: cutaway wedge · classified air veils · reference graticule. |
| `inspect` · `erode` | transport buttons | `Inspect(facing_cell)` · `ErodeToggle`. **`erode` ON is gated on the life-supporting light** (RAIN OFF is always allowed); the button also carries `enabled_bind: rain_allowed`. |
| `lv_veneer` … `lv_leach` (12) · `water_infall` · `water_coverage` | the lever rack + water dials | `SetLevers`, sent only on a real change. Rate levers ride a **multiple of the physics as written** (`1.0` = as the process chose); the two water controls keep their own units. |
| `gates_open` · `gates_close` · `hold_1` … `hold_24` | the process chip / gate console | open/close the console; `Hold{stage, !held}` toggles a process hold (the sanctioned ARM/RELEASE lever — guiding formation without writing a result). |
| `gate_resume` · `gate_view` | the gate-pause card | resume the run (and ack the transition) · jump to the view that shows what moved (and ack). |
| `seed_toggle` · `starter_open` · `starter_close` | reference/Starter toggles | show the bulk-seed panel · open/close the Starter console. |
| `seed_el_1` … `seed_el_12` · `seed_freq` · `preset_mercury` … `preset_europa` · `forge` | the Starter console | write **pending** scene state (nothing reaches the sim); a preset stages a whole input bundle; `forge` births the world from them (`Reseed`). |

**Exits: none.** The only stack move this bench makes is the pause `Push`; it does not
implement `Scene::route`.

### Model keys — the two-hop channel

Rust publishes a **raw** Model (`hud_model`); the pair script's `derive()` returns a
**display** Model over it; the tree binds the merge. Plus a transient `sig_<name>` mirror of
last frame's fired intents (S9). There are ~130 keys; the catalog is discoverable at three
sources rather than duplicated here:

- **Raw state + the resolved word-bank** — listed in the pair script's own header,
  [`godmode.lua`](../../../content/sensorium/scripts/godmode.lua): `playing`, `eroding`,
  `balanced`; `field_<action>_state`; `proc_<n>_name`/`_state`; `procs_running`/`_waiting`/
  `_held`; `a<n>_live`/`_in`; `gate_stage`/`_opened`/`_my`; `ledger_total`; and the `w_*`
  localized words the script picks and composes.
- **Derived display keys** — what `derive()` returns and the tree binds: `play_state`/
  `_label`/`_color`, `erode_label`, `field_*_style`, `proc_<n>`/`_color`, `hold_<n>_label`,
  `proc_summary`/`proc_chip_style`, `gate`/`gate_color`/`gate_headline`, `ledger_status`/
  `_color`, `a<n>_status`/`_color`/`_name_color`, `verdict`/`_color`, `observed`.
- **Pre-formatted measurement lines** — composed at the Rust publish sites (they ride the
  `fmt_mass`/`fmt_pressure` unit helpers, not the stringtable state words): `stats`,
  `interior`/`interior2`, `crust`/`crust2`, `air_line`, `life`/`life2`, `water`, `ledger_1..6`,
  `ev_1..8`, the `gate_*` card lines, the `legend_*` card, the `seed_*` Starter rows, and the
  refilled `a<n>_*` gauge rows. The `raw_model_publish_literals` gate scans these so raw
  English can never enter the Model.

Two invariants worth knowing: keys published-but-never-bound and bound-but-never-published
both render as nothing, silently. And a bind spelled like an action is a collision — which is
why the popup **state** keys are `gates_shown`/`starter_shown`/`gate_pause_shown`, deliberately
*not* the `gates_open`/`starter_open` action names.

### What it hands other crates

- To the **sim thread**: `SimCommand`s (best-effort sends).
- To the **globe**: the shell stack (`set_shells`) + line overlays (`set_arrows`), the seated
  rect (`seat`), and the pane-focus + pointer sample each frame. The globe returns the
  looked-at cell (`facing`) that `inspect` materialises.
- To the **shell**: `Transition::Push(PauseScene)` on `pause_open`.
- To the **frame graph**: the globe's offscreen pass + composite, then the 2D HUD overlay
  (the walker's commands + the one immediate bulk-seed swatch panel, the sanctioned per-datum
  exception because its colours are per-element with a hash fallback).

### Threads / workers

Two, both owned by the sim thread and detached from the renderer: the **sim thread**
(`flicker-sim` — owns the `World`/`Scheduler`/grid, advances while playing, pauses itself on a
gate edge) and the **erosion worker** (`flicker-erosion` — one `Eroder` over the inspected
tile at a ~350 ms cadence). The render thread only ever reads the two published slots.

## The ten field views

One roster (`FIELD_ACTIONS`) is the single source for the tab strip, the dispatcher, the
lit-tab styling, and the `view` name content uses — pinned self-consistent by
`the_view_roster_agrees_with_itself`. "Paints" is where the read lands: **Interior** recolours
the mantle shell (crust drawn above in its own colours), **Surface** recolours the crust,
**Overlay** recolours nothing and draws its own geometry.

| Action | View name (`processes.json`) | Paints | Shows |
|---|---|---|---|
| `field_temperature` | `heat` | Interior | mantle temperature, ramped over the frame's own min/max (structure, not an absolute scale) |
| `field_differentiation` | `core` | Interior | core-formation progress (slate → gold) |
| `field_plates` | `plates` | Surface | a stable hue per persistent plate id; diffuse lithosphere grey |
| `field_seams` | `seams` | Surface | divergent ridge / convergent trench / transform |
| `field_elevation` | `relief` | Surface | greyscale relief over the 2nd–98th elevation percentiles |
| `field_coast` | `coast` | Surface | the four grounds + a brightened coastline (`coast_class`) |
| `field_motion` | `motion` | Overlay | plate-step heading arrows, grouped by plate colour |
| `field_rain` | `rain` | Surface | rainfall (sqrt-scaled; a dry world reads honestly flat) |
| `field_strata` | `strata` | Surface | how many beds a column has stacked |
| `field_ore` | `ore` | Surface | richest metal seam vs. the planet's own share (log ramp) |

**The view name is the cross-crate seam** with `flicker-poc-chemistry`: a `ProcessDef.view` in
`processes.json` names the instrument that shows a process working (it lights that tab
"suggested" while the process runs, and the gate card offers a SHOW-ME button to it). That
string is authored in *another* crate but resolved *here* — and God Mode is the consumer that
makes it **fail loud**: `every_authored_view_names_a_real_one` fails the build on a typo, and
at runtime an unknown view name logs a `tracing::warn!` naming the legal views. (This closes
the split-authority concern noted in the `flicker-poc-chemistry` docs pass — the producer
cannot validate its own `view` strings; the consumer does.)

## Gates

`source ~/.cargo/env && cargo test -p flicker-godmode` — 22 tests, all green.

**The globe stage** (`globe_view.rs`)
- `the_authored_globe_stage_is_read` — `stages.godmode_globe` compiles and emits light (a
  declaration nothing consumes is a name that resolves to nothing).
- `the_stage_declares_the_simulated_shells` — the stage authors exactly `Shells` and no
  camera (the sim publishes the world; the maintainer flies it).
- `an_unknown_source_still_lights_the_globe` — a typo'd stage source falls back **lit**, not
  black (rule 4BB12A75: a style typo costs the look, never the picture).

**The pause hook** (`sim_thread.rs`)
- `the_watch_fires_on_edges_and_never_on_the_baseline` — a gate's `ready` edge pauses the run;
  the opening conditions are a baseline, not a transition.
- `a_held_stage_is_silent_and_the_lever_is_not_an_event` — a held stage announces nothing, and
  the maintainer's own hold/release never pauses the run on itself.
- `a_reset_forgets_the_previous_world` — a rebirth's opening state does not fire against the
  old world's.

**The dispatcher, levers & era gate** (`scene/tests.rs`)
- `each_lever_moves_exactly_its_own_field` — a table-driven rack of levers where a copy-paste
  row would silently point two controls at one field.
- `a_lever_at_its_echo_sends_nothing` — an unmoved rack (and rate dial) sends no command, so
  merely looking at the bench does not rebuild the world 60×/s.
- `a_rebirth_clears_the_gate_acknowledgement` — Reset/Reseed clear the read high-water, or the
  second run's first gate reads as already-acknowledged and pauses silently.
- `rain_waits_for_the_life_light` — RAIN ON is gated on the life-supporting verdict; RAIN OFF
  is always reachable.
- `the_verdict_lamp_lights_without_eating_the_life_line` — the lamp key (`life_light`) and the
  life-line text key (`life`) stay separate at the publish (a fixed name collision).

**The views & legends** (`scene/tests.rs`)
- `the_view_roster_agrees_with_itself` — `cycle()` reaches every `FIELD_ACTIONS` row exactly
  once and every label token resolves.
- `every_authored_view_names_a_real_one` — every non-empty `processes.json` `view` is a view
  the bench has (the cross-crate seam, tested at build time).
- `the_coast_view_separates_the_four_grounds` — the classifier and its colours keep shelf,
  deep bed, land and exposed floor visually distinct, with a brighter coastline.
- `the_rain_view_stays_dark_on_a_dry_world` — the ramp does not invent weather on a desert.
- `motion_arrows_draw_only_what_is_moving` — arrows appear only where the ground moves, grow
  with the plate step, group by plate, and vanish under a cutaway.
- `every_view_explains_its_colours` — every view publishes a legend (rows or a labelled ramp),
  every swatch path resolves, and the generated `legend.*` block *is* the paint the globe uses.
- `the_air_veil_never_closes_into_a_lid` — a saturated sky squeezes to leave two-thirds of the
  surface visible, keeps the between-gas ratios, and passes a trace sky through untouched.
- `godmode_does_not_double_light` — a shell colour is a function of the cell, never its
  direction: the terminator has exactly one source, the stage rig.

**The scene pair** (`scene/tests.rs`)
- `the_shipped_scene_authors_the_bench` — the shipped file parses, declares the pause + tab
  intents, has the globe's `surface` slot, refills one gauge row per `habitability::BAND`, and
  passes `unknown_kinds` / `raw_display_literals` / `raw_model_publish_literals`.
- `dispatch_fires_the_declared_pause_intent` — `Menu` fires `pause_open` through the authored
  tree via the walker.
- `the_pair_script_derives_the_state_words` — the pair actually meets: `derive()` yields the
  display words, glyphs and style paths over the raw publish.

Two roster gates in [`prism-alpha/src/main.rs`](../../../prism-alpha/src/main.rs) also cover
this crate (the manifest↔roster binding and the realm order).

## Sharp edges

- **Two different things are called "pause."** `pause_open` (the `Menu` intent) pushes the
  *shell's* settings/quit overlay. The *gate-pause card* (`gate_pause_shown`) is the sim
  self-pausing when a process gate moves, and shows a one-time summary of what changed. They
  are unrelated; do not wire one expecting the other.
- **`Cancel` does not close the popups.** No `on_cancel` is authored, so `Cancel` runs the
  walker's built-in back-out (pop an entered globe-pane level, else pop the scene context) —
  it never fires `gate_resume`/`gates_close`/`starter_close`. Each popup closes only via its
  own button, reachable by pointer or by nav-to-button + `Confirm`. (A source comment in
  `apply_results` claims otherwise; see the note the docs pass filed.)
- **`erode` ON needs a living world.** The button is disabled via `rain_allowed` until the
  five-axis verdict turns, and the dispatcher re-checks it (belt and braces). A dead world
  swallows the click with no command and no mirror flip. RAIN OFF is always allowed.
- **`self.eroding` is a scene-side mirror with no echo.** `ErodeToggle` is fire-and-forget and
  the sim publishes no rain state, so the label stays honest only by both sides counting the
  same presses. A dropped command would desync the label from the worker.
- **A dial that reads its own echo must not resend.** Every `SetLevers` rebuilds the pipeline,
  so the rate/lever/water controls send only on a change past a threshold. This is why the
  Model echoes the sim's *actual* rate, not the last request.
- **A forge is a new planet, and re-sends `StaticData`.** A changed `freq` is a new mesh; the
  render thread drops every mesh built on the old topology (`topology_stale`). Meshes appear
  the frame after `set_shells`.
- **The process console has headroom.** `PROCESS_ROWS = 24` against ~22 shipped stages; spare
  rows ride `proc_<n>_shown = false` and the flow layout skips them, so they occupy nothing.
  The count is deliberately spec-ward — grow the pipeline and no change is needed here.
- **`Default`/`shipped()` panic in production.** The only construction path is `scene(def)` /
  `new(def)`; a def-less bench would be a blank screen, so it fails loud rather than draws
  nothing.
- **The camera is not this crate's.** All look/zoom/focus behaviour lives in
  [`flicker-globe`](../../frontend/flicker-globe/README.md) (which has an open architectural
  ruling on its pointer path). God Mode only publishes shells and reads `facing`.
