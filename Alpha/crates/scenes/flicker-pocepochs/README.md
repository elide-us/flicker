# flicker-pocepochs

The **Epoch Simulation** bench, a *scene crate*: a library that supplies one `Scene`
behaviour driving a live planet-evolution sim. It seeds a fresh world, cools it forward
through the epochs on Play, colours the globe three ways (material / heat / the sliced-open
layer stack), and reads the five life-supporting condition axes as they emerge. The planet
is the shared `flicker-globe` world drawn full-window; a declarative HUD floats over it with
the transport controls and the habitability panel.

The one sentence a newcomer needs: this bench drives the **`Simulation`** (interactive tick)
driver of `flicker-worldengine` — *not* the `WorldEngine` (`.epoch` capture) driver — and
turns each tick into a coloured globe plus an observer readout.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

**Vocabulary used below** (each is a flicker word, not a general one): a **scene crate** is a
library supplying one `Scene`, launched by name from the `prism-alpha` roster; a **scene
file** is the authored `*.scene.json` that declares the component **tree**; its **pair
script** is the same-named `.lua` that turns raw numbers into display strings; the **Model**
is the per-frame key→value table the engine hands to Lua and to the tree's binds; the
**walker** is the Rust pass that lays out, hit-tests and draws the tree; a **signal** is a
device-independent input verb (`Menu`, `Confirm`, `ModeNext`); an **intent** is a signal
binding declared as data on the scene root (`"on_menu": "pause_open"`); a **result** is a
name fired into the frame's results map; a **surface** is the drawing ground the scene's own
3D element occupies under the UI. Sim words: a **Simulation** is the tick driver (one tick =
crust moving one hex, ~`MY_PER_TICK` My); the **observer** (`observe()`) reads a world into
condition **axes** with a low/high band each; a **census** is this crate's cached per-tick
count of the temperature and which layer kinds have emerged; a **view** is one of the three
colourings; a **shell** is one sparse globe mesh; the **cutaway** is a longitude wedge cut
through the stack. This crate is the sibling of
[`../flicker-clicktrainer/README.md`](../flicker-clicktrainer/README.md), the smallest scene
crate — read it for the 2D-scene shape.

## Where it sits

- **Builds on:** `flicker` (the umbrella: `Scene`/`Transition`, `Renderer`/`FrameGraph`,
  `ScriptHost`/`ValueMap`, the walker entry points `run_ui`/`render_hud`, `strings`) ·
  **`flicker-worldengine`** (`Simulation`, `observe`, `LayerKind`, `MY_PER_TICK`, `classify`,
  `Tables`, `Phase` — see
  [`../../world/flicker-worldengine/README.md`](../../world/flicker-worldengine/README.md);
  this bench drives the `Simulation` column of that crate's two-driver table) ·
  **`flicker-globe`** (`GlobeWorld`, `ShellSpec`, `RADIUS`, `in_wedge`, `build` — the ONE
  shared globe world; see
  [`../../frontend/flicker-globe/README.md`](../../frontend/flicker-globe/README.md)) ·
  `flicker-input-core` (the `ActionSignal` vocabulary — catalog in
  [`../../input/flicker-input-core/README.md`](../../input/flicker-input-core/README.md)) ·
  `flicker-input-router` (`InputHandler`/`Router`) · `flicker-shell` (`PauseScene`, `Theme`,
  `input_profile`) · `flicker-core` (the content-roots service) · `serde_json` · `tracing`.
- **Used by:** `prism-alpha` only, and only through [`scene`](#public-api). Its roster entry
  is in [`../../../prism-alpha/src/main.rs`](../../../prism-alpha/src/main.rs) (`roster()`,
  id `"pocepochs"`, title `"Epoch Simulation"`, realm `REALM_GAMEMASTER` — grouped with
  `populous` and `godmode`).
- **Reads from the content tree:**

| Path | When | If missing |
|---|---|---|
| [`content/sensorium/scenes/pocepochs.scene.json`](../../../content/sensorium/scenes/pocepochs.scene.json) | at launch, by the kernel — the parsed `SceneDef` is handed to `scene()` | no `tree` ⇒ `tracing::error!` and **no HUD draws** (the globe still renders) |
| [`content/sensorium/scripts/pocepochs.lua`](../../../content/sensorium/scripts/pocepochs.lua) | compiled in via `include_str!` (`src/scene.rs:38`), loaded in `from_parts` | load error ⇒ `tracing::error!`; the HUD binds raw numbers, so display strings and `color_bind` paths go empty |
| [`content/data/`](../../../content/data) (the sim's element/compound tables + seed) | `Simulation::from_repo_seeded` in `from_parts`/`rebuild` (`src/scene.rs:203`) | `.expect(...)` — **panics.** Compile-time relative path; breaks installed builds (see Sharp edges) |
| [`content/data/stringtable.json`](../../../content/data/stringtable.json) | per frame — the `$pe_*` / `$poc_*` / `$hab_*` tokens the model resolves and the panels draw | the raw token text draws |
| `ui_theme.json` colours (`$bronze`, `$stone*`, `$sig_green`, …) | via `load_shared_styles` in `from_parts` | the scene's `$token` colours fall back to compiled defaults, silently |
| stage `pocepochs_globe` (authored **in this scene file's own `stages` block**, `pocepochs.scene.json:5`) | in `GlobeWorld::new` at construction | an unresolved stage name is the failure rule 4BB12A75 exists for; the gate below pins it |

To change what the HUD looks like or says, edit the scene file and pair script — see
[`../../../content/sensorium/README.md`](../../../content/sensorium/README.md) for the
authoring format. This file does not re-teach it.

## Public API

The crate's re-exported surface is tiny — three items reachable from `lib.rs`:

| Item | For | The one thing to know |
|---|---|---|
| `pub fn scene(def: &SceneDef) -> Box<dyn Scene>` | the roster factory — the only intended entry point | The `SceneDef` is the *parsed scene file*; the kernel resolves it from the manifest when the `"pocepochs"` row is launched. |
| `pub struct WorldScene` | the `Scene` implementation | All frame state lives here (the `Simulation`, the `GlobeWorld`, the authored tree, the pair script). Fields are private. |
| `pub fn WorldScene::new(def: &SceneDef) -> Self` | the unboxed constructor | Builds **everything** up front — sim, globe, styles, script, and the refilled axis rows — because the globe's look is authored and must exist before the first frame. `enter` is left with the GPU only. |

The `Scene` trait methods it implements are `enter`, `exit`, `update`, `render`.

There is also `pub fn WorldScene::shipped()`, but it is **test-only**: under `#[cfg(test)]`
it parses the shipped scene file and builds the bench; under `#[cfg(not(test))]` it is
`unreachable!(...)` (`src/scene.rs:191`). `impl Default` calls it, so `WorldScene::default()`
**panics in a shipping build** — deliberately loud (there is no def-less bench). A second host
constructs via `new(def)`, never `shipped`/`default`.

`mod appearance` is **private** — the data→colour vocabulary (`ViewMode`, `cell_stack`,
`material_color`, `cell_heat_color`, `legend_entries`, `element_rgb`, `phase_color`,
`cycle_view`/`cycle_view_back`, `R_BASE`). Its items are `pub` for the scene module and the
crate's tests, but they are **not** part of the crate's external API. Its per-datum colours
(an element with no chosen hue is hashed one) are the sanctioned exception the authoring guide
names — the walker's colour channel is dotted style paths, and a per-datum colour has no path,
so the two colour panels are Rust-drawn (below), not walker-drawn.

**Tuning — compiled, not authored.** These are private `const`s in `src/scene.rs`; the scene
file's `params` block is empty and is never read, so changing them means a rebuild.

| Const | Value | Meaning |
|---|---|---|
| `PLANET_FREQ` | `48` | default planet size (grid frequency, ~½ Earth) |
| `SIZE_MIN` / `SIZE_MAX` / `SIZE_STEP` | `12` / `96` / `6` | range and step for the size −/+ controls (96 ≈ full Earth) |
| `PLAY_TICKS_PER_SEC` | `6.0` | sim ticks advanced per second while playing |
| `AXIS_ROW_H` / `AXIS_BAR_H` | `42.0` / `12.0` | geometry of a refilled condition-axis row |

## Interactions

### Signals it captures

Signals only — never keys or buttons; what produces a signal is profile data, out of scope
here (DFE3E44E). A **click is a `Confirm` signal targeted at whatever the pointer hits**
(37722F91) — there is no separate intent router; the walker captures the signals it cares
about. The single dispatch in `update` is one `WalkerHandler` (`src/scene.rs:704`, built
`.with_nav(...).with_rects(...).with_intents(...)`); the input **pump** resolves this frame's
events and the scene owns no resolver.

| Signal | Channel | Effect |
|---|---|---|
| `Menu` | declared intent — `"on_menu": "pause_open"` on the scene root | `src/scene.rs:763` returns `Transition::Push(PauseScene)`, built from the profile's `"World"` context map. |
| `Confirm` | the walker, on the focused/pointed transport control | Fires that control's `action` (or toggles the checkbox's bound key) as a result — see the results table. |
| `NavUp/Down/Left/Right`, `PanelNext/Prev` | the walker's nav ring, over the two panes (`pe_readout` = nav_ordinal 1, `pe_hab` = 2) and the `pe_readout` tab_group (the 7 transport controls, nav_ordinal 1–7) | Moves focus; `Confirm` then activates. This is the non-pointer reach for the transport row. |
| `ModeNext` | declared intent — `"on_mode_next": "view_next"` | **Fires nothing today** — `ModeNext` is a `Reserved` signal with no binding in any shipped profile. See Finding 1. |
| `ModePrev` | declared intent — `"on_mode_prev": "view_prev"` | **Fires nothing today** — same cause. `view_prev` is therefore unreachable by any shipped input. See Finding 1. |
| look / zoom axes | `GlobeWorld::look_from(\|s\| signals.axis(s, input))` (`src/scene.rs:781`) | Flies the planet's orbit camera — but see the fullscreen-pane caveat in Sharp edges. |
| pointer sample | the walker's **root pointer** (`frame.root_pointer()`, `src/scene.rs:696`) | Drag/wheel the planet — present only when no UI claims the cursor (`hud_hit` false), so a drag on the HUD never flies the planet. |

### Results the walker fires / the behaviour consumes

All consumed in `update` (`src/scene.rs:722-763`). Each transport control names its result as
an `action` in the scene file; `pe_cut` is a checkbox writing its bound key.

| Result | Fired by (scene file id) | Consumed → effect |
|---|---|---|
| `toggle_play` | `pe_play` button | flips `playing`, zeroes the play accumulator |
| `reset` | `pe_reset` button | pause and `go_to_tick(0)` — back to the Epoch-1 seed |
| `reseed` | `pe_reseed` button | roll a new random seed → a fresh planet (same size) |
| `size_down` / `size_up` | `pe_size_down` / `pe_size_up` buttons | `resize(∓1)` step of grid frequency (rebuilds, back to tick 0) |
| `view_next` | `pe_view` button (**and** the dead `on_mode_next`) | `cycle_view` material→heat→layers, republish shells |
| `view_prev` | the dead `on_mode_prev` only — **no button** | `cycle_view_back`; unreachable today (Finding 1) |
| `cut` | `pe_cut` checkbox (`bind: "cut"`, `visible_bind: "layers_view"`) | toggles the wedge — **read only while the Layers view is active** (`src/scene.rs:751`); off that view the checkbox is hidden and the read is skipped so it can't clear |
| `pause_open` | the `Menu` intent | `Transition::Push(PauseScene)` |
| `hud_hit` | the walker, whenever the pointer is over an interactive/styled HUD region | gates the planet camera — `over_hud` suppresses the root pointer this frame |

**Exits: none.** The scene declares no `exits` and the crate does not implement
`Scene::route`; the only stack move is the pause `Push`.

### Model keys — published (raw) then derived

Two hops. Rust's `hud_model` (`src/scene.rs:442`) publishes **raw** values; `pocepochs.lua`'s
`derive()` folds **display strings** over them (`model()`, `src/scene.rs:519`); the merged map
is what the tree's binds read. Localization stays engine-side — the raw publish already
resolves the `w_*` / `air_kind` / `a{n}_name` words from the stringtable, and the script only
composes.

| Raw key (published) | Type / note | Feeds |
|---|---|---|
| `tick` `my` `temp_k` `cells_n` `core_n` `crust_n` `ocean_n` `atm_n` | Numbers — sim clock + census counts (recomputed on a tick move, never per frame) | `stats_val` |
| `playing` `cut` | Bool | `play_state`/`play_label`; `cut` is also the checkbox's two-way bind |
| `view` | **Number 0..2** (Material/Heat/Layers — an index is a number, 1B64FF03) | `view_line`/`view_label`/`layers_view` |
| `freq` | Number — grid frequency | `size_line` |
| `w_tick` `w_cells` … `w_view_0/1/2` `w_in_band` `w_life_supporting` `w_air` (23 words) | Text, stringtable-resolved | composed by the script — never raw English |
| `a{n}_v` | Number, **`-1.0` = no signal yet** | the gauge `bind` on axis row *n* |
| `a{n}_live` `a{n}_in_band` | Bool | the script's per-axis status + name-colour |
| `a{n}_name` `a{n}_lolab` `a{n}_hilab` | Text, resolved observer metadata | the row's name + end-caption `text_bind`s |
| `axes_total` `axes_live` `axes_in_band` `life` `no_life` `air_kind` | Numbers / Bools / resolved Text | the verdict footer + the life light |
| `sig_<name>` | Bool `true`, transient | the S9 mirror of intents fired last frame — published once, then dropped; nothing binds it here |

| Derived key (`pocepochs.lua`) | Bound by (channel) |
|---|---|
| `stats_val` `view_line` `size_line` `verdict` `observed` `air` | `text_bind` |
| `play_state` `play_label` `view_label` | `text_bind` / `label_bind` |
| `play_state_color` `verdict_color` | `color_bind` (dotted style paths) |
| `layers_view` | `visible_bind` on `pe_cut` |
| `a{n}_status` | `text_bind` on the row |
| `a{n}_status_color` `a{n}_name_color` | `color_bind` on the row |

The habitability rows are not authored — the `pe_axis_rows` container (`pocepochs.scene.json`)
is **refilled at construction** (`refill_axis_rows`, `src/scene.rs:252`), one row per observer
axis, each gauge's band baked from `observe(world(0))`. So a row's numbers come from the raw
publish, its words/colours from the script, and its geometry from Rust — three files for one
row.

### What it hands other crates

- The `GlobeWorld` as the frame graph's **root pass**, straight into the swapchain
  (`world.render_root`, `src/scene.rs:808`) — no offscreen target, no blit.
- The HUD as **one overlay** at `base + 1.0` (`src/scene.rs:822`): the walker's `HudCommand`
  list via `render_hud`, then **two Rust-drawn panels** (the surface-view legend, top-right,
  and the Epoch-1 element-distribution readout, left). Those two are drawn directly with
  `draw_sprite`/`draw_text` because their swatch colours are per-datum — the sanctioned
  exception, not a walker path.
- `Transition::Push(PauseScene)` on `pause_open`.

No threads, no workers, no async — `Simulation::ensure` fills ticks lazily on the calling
thread.

## Gates

`cargo test -p flicker-pocepochs` — **7 tests, all green** (verified 2026-08-24).

| Test | What it holds |
|---|---|
| `plays_epoch2_convection_and_resets_to_epoch1` | starts paused at tick 0; Play advances the tick; Reset returns to Epoch 1 |
| `the_shipped_scene_file_authors_the_bench` | the real scene file parses, names behaviour `"pocepochs"`, declares `Menu → "pause_open"`, and carries the `pe_axis_rows` container + all seven transport control ids |
| `hud_tree_is_well_formed_and_draws_from_the_observer_model` | the full model path over the **real** file + script: no unknown kinds, no raw display literals, no raw copy published into the Model; the refilled gauges carry the observer's real bands; `derive()` yields the six display keys (`play_state == "PAUSED"`); the walked tree draws the title, state word, hab header, buttons and gauge bars |
| `the_declared_pause_intent_fires_through_the_authored_tree` | `Menu` is consumed at the walker layer and fires `pause_open` |
| `epoch1_element_distribution_is_populated_and_sensible` | the seed's element distribution is non-empty, sorted descending, and sums to most of the planet |
| `the_planet_is_the_shared_world_and_the_stack_rides_a_per_cell_radius` | the standing anti-reinvention proof: `stages.pocepochs_globe` resolves and says the simulation publishes the shells; a surface read is one sphere; the layer stack is one per-column shell per kind with the mantle at the base ball; the cutaway opens only above the mantle; the per-column framing costs the same vertices/triangles as the sphere it absorbed |
| `resize_changes_the_planet_and_reseed_changes_the_composition` | resize shrinks + returns to tick 0; reseed rolls a new seed and keeps the size |

Two gates in `prism-alpha/src/main.rs` also cover this crate: `roster_holds_the_migrated_benches`
pins `["populous", "godmode", "pocepochs"]` in the Game Master realm, and the roster-vs-scene
gate binds the id to the scene file's existence.

## Sharp edges

- **The view-cycle intents fire to nothing; `view_prev` is unreachable.** `on_mode_next`/
  `on_mode_prev` resolve to `ModeNext`/`ModePrev`, which no shipped profile binds. The `pe_view`
  button is the only working view control and only goes forward. See Finding 1 — this is
  unfinished wiring, not dead code.
- **`shipped()` / `default()` panic in a shipping build.** Only `new(def)` constructs a real
  bench outside tests.
- **The planet flies under the HUD panels.** The globe is full-window with no pane to gate on,
  so the pointer camera can drag the planet from anywhere the HUD doesn't claim, and stick look
  needs a fullscreen-pane answer in `flicker-globe` (flagged at migration; behaviour-preserving).
- **`from_repo_seeded` breaks installed builds.** It resolves `Alpha/content/data` via a
  compile-time relative path and `.expect(...)`s — fine in the dev tree, a panic anywhere else.
  Cross-crate (`flicker-worldengine`), already flagged as domain work.
- **`observe()` sweeps every cell once per frame** for the habitability panel, and tick
  checkpoints grow unbounded on a long Play. Both are known perf-parity carries.
- **The pair script is a soft dependency.** If `pocepochs.lua` fails to load the globe still
  renders and the HUD still lays out, but every `text_bind`/`label_bind`/`color_bind` naming a
  *derived* key draws empty / falls to a default colour — silently. The well-formed gate is what
  keeps that off the screen.
- **A typo'd bind, token, style path or stage name fails to nothing** (4BB12A75): a missing
  Model key draws an empty string, a missing dotted style path yields a compiled default, a
  missing `$token` draws its raw text. The `unknown_kinds` / `raw_display_literals` /
  `raw_model_publish_literals` gates cover the kinds and the copy, not a mistyped bind key.
- **`-1.0` is the null for a gauge.** `a{n}_v` publishes `-1.0` when an axis has no signal yet;
  it is a bare literal on the Rust side, preserved through the bind.

## Findings — implementation gaps this README exposed

1. **Unfinished wiring / silent-name-failure (headline) — the view-cycle intents fire to
   nothing, and `view_prev` cannot be reached at all.** `pocepochs.scene.json:105-106` declares
   `on_mode_next → view_next` and `on_mode_prev → view_prev`; these resolve
   (`flicker-widgets/src/intents.rs:61-64`) to `ActionSignal::ModeNext`/`ModePrev`, which are
   `Reserved` and bound by **no** shipped profile (confirmed by MCP incident **A50A2ABA**). The
   `pe_view` button (`action: "view_next"`) is the only working view control and only advances,
   so `cycle_view_back` (`src/appearance.rs:56`) / the `view_prev` arm (`src/scene.rs:739`) are
   unreachable, and both declared intents fire with zero events and zero warning. The scene.rs
   module doc (`src/scene.rs:12-14`) presents these intents as live controls. *Why a human
   trips:* they author the pad view-cycle exactly as the reference shows, and nothing happens or
   warns. *Fix direction (Aaron decides):* finish the wiring — bind `ModeNext`/`ModePrev` in a
   profile (install the chord layer, or add a nav-ring binding), or add a "prev view" button —
   **not** remove the capability; `cycle_view_back` is a spec-ward tool (F42DA5E0).

2. **Undocumentable magic / latent silent-partial — the pair script hard-codes the axis count
   while Rust refills from the observer.** `pocepochs.lua:22` sets `local AXES = 5` and derives
   `a{n}_status` / `a{n}_status_color` / `a{n}_name_color` for `1..5` only, but
   `refill_axis_rows` (`src/scene.rs:252-254`) builds one row per `observe(world(0)).axes` and
   `hud_model` publishes `axes_total` (`src/scene.rs:506`). The observer exposes exactly five
   axes today, so the two agree — but the count lives in two places, and the script never reads
   the `axes_total` it is handed. *Why a human trips:* add a sixth condition axis to the observer
   and the sixth row draws its gauge and name but no status word and no name-colour, silently.
   *Fix direction:* drive the Lua loop from the published `axes_total` instead of the `AXES`
   literal.

3. **Minor contract note — `shipped()` / `Default` are test-only landmines on the public
   surface.** `src/scene.rs:191-196` + `646-650`: `WorldScene::default()` calls `shipped()`,
   which is `unreachable!` outside tests, so `default()` panics in a shipping build. It fails
   *loud* (compliant with 4BB12A75), but it is a `pub` seam a second host could reach for. *Fix
   direction:* likely none needed — documented here as the reason `new(def)` is the only real
   constructor.

*No manufactured gaps.* The bench is otherwise clean: the model round-trips through real gates,
the anti-reinvention proof (shared globe + per-column radius) is a standing test, and every
documented symbol, token, path and test name resolves.
