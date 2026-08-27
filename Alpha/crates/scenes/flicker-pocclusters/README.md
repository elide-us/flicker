# flicker-pocclusters

The **Prism Test Room** — an island voxel sandbox that is the engine's *integration
locus*: the one scene where the renderer's every tool is turned on at once (env-lit sun,
sky day/night cycle, sun shadows, flooded water with animated waves, ground fog, bloom,
golden-hour grading, grass scatter) over a live 3×3 voxel-cluster terrain you can fly or
walk. Registered in the `prism-alpha` launcher's **Adventurer realm** (a *realm* is one
tab of the scene picker) as "Prism Test Room". If you are adding a renderer feature and
want somewhere real to see it, this scene is where it lands.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:**
  - `flicker` (the umbrella crate) for `render` (the `Renderer`, `FrameGraph`, `StageDef`,
    `CompositeTarget`, `LightRig`, mesh types), `scene` (`Scene`, `SceneInput`,
    `Transition`), `script` (`ScriptHost`, `UiNode`, the Model `ValueMap`), `ui` (the
    component **walker** `run_ui`, `chat_panel`, `SceneDef`, `Sections`, the stringtable
    `strings`), and `net::chat` (`ChatClient`).
  - `flicker-voxel` for the cluster field: contouring, meshing, LOD, and the LOD2 nav
    surface — see **[../../world/flicker-voxel/README.md](../../world/flicker-voxel/README.md)**.
  - `flicker-input-core` / `flicker-input-router` for the signal event bus (`ActionSignal`,
    `InputHandler`, `Router`).
  - `flicker-shell` (splash/menu/settings/**pause** overlay + the gothic `Theme`),
    `flicker-orrery` (the shared planet-layout roster the sky draws), `flicker-worker`
    (the off-thread mesh/nav pool), `flicker-materials` (the draw palette),
    `flicker-content` (content-roots + `PropSet`), `flicker-primitive` (the island
    heightfield sampler), `flicker-skeletal` (the grass prop mesh loader), `flicker-core`
    (the gz-at-rest bake seam).
- **Used by:** `prism-alpha` — its scene roster registers `flicker_pocclusters::scene`
  (`Alpha/prism-alpha/src/main.rs`). No other crate depends on it; it is a leaf scene
  package (`publish = false`, library only, no binary).
- **Reads from the content tree** (all resolved through the content-roots service, so the
  working directory does not matter):
  | Path | When | If missing |
  |---|---|---|
  | `content/sensorium/scenes/pocclusters.scene.json` | passed in as the `SceneDef` at construction | no HUD tree → `tracing::error!`, the world still runs |
  | `content/sensorium/scripts/pocclusters.lua` | embedded at **compile** time (`include_str!`) | HUD shows raw values only |
  | `content/sensorium/resources/ui_theme.json` (+ the scene's own `stages` block) | folded into the styles root; the `pocclusters_world` / `pocclusters_sun_shadow` **stages** are compiled from it in `enter` | world draws with no fog / no shadow (see Sharp edges) |
  | `package/bakes_island/cluster_{x}_{y}_{z}.json[.gz]` | LOD-0 cluster **bakes** loaded on `enter` (gz-first, then loose) | falls back to live-contouring the *same* island, so the terrain is identical |
  | `package/props/environment/GrassField/GrassField.set.json` + variant meshes | grass **scatter** loaded once in `enter` | no grass (fail-soft) |
  | `data/` material catalog (`materials.json`) | the draw palette, set on `enter` | mesh field draws loud magenta |

Authoring the scene file, the pair script, and the stage recipe is **not** re-taught here —
see **[../../../content/sensorium/README.md](../../../content/sensorium/README.md)** for
the scene format and **[../../../content/sensorium/STAGES.md](../../../content/sensorium/STAGES.md)**
for the stage/recipe schema. This crate's sibling
**[../flicker-clicktrainer/README.md](../flicker-clicktrainer/README.md)** is the smaller
scene built on the same `Scene` + pump-input shape; read it first if this one is too much.

## Public API

The crate's entire external surface is one factory:

| Item | What it is for | The one thing to know |
|---|---|---|
| `pub fn scene(def: &SceneDef) -> Box<dyn Scene>` | Build the Prism Test Room as a boxed `Scene` for `prism-alpha` (or any shell host). | The `SceneDef` carries the parsed `tree` (the HUD component tree) and folded `styles` (theme + this scene's `stages`). Everything else — world generation, camera, pause, chat — the scene owns internally. |

Everything else is **internal** (`mod celestial`, `mod route`, `mod scatter` are private
modules, so their `pub` items are crate-visible only). A debugger's map:

| Module | Holds |
|---|---|
| `lib.rs` | `GameScene` (the `Scene` impl), the cluster field + async mesh/nav build, camera + walk physics, ray-pick + the virtual-voxel inspector, chat client plumbing, and the render composition. |
| `celestial.rs` | The from-Home day/night sky: sun/moon/eclipse `LightRig`, the golden-hour warmth curve, planet + constellation overlays, and the panel readout formatters (`fmt_clock`, `fmt_moon`, …). |
| `route.rs` | The four input **handlers** the scene dispatches through — `RootHandler`, `CommandHandler` (chat text entry), `GameplayBase` — plus the walker layer from `flicker-widgets`. |
| `scatter.rs` | Pure, GPU-free grass placement: `scatter(weights, &ScatterParams, height_at) -> Vec<GrassPlacement>`. Deterministic (hash-per-cell, no RNG state). |

## Interactions

### Signals it captures

A *signal* (`ActionSignal`) is the engine's device-independent input verb — the scene answers
signals **by name**, never keys (rule DFE3E44E). All input arrives already resolved from the
pump as `SceneInput`, and is dispatched through a 4-layer handler chain
(`root → command → walker → gameplay`; highest priority first):

| Signal(s) | Channel | Effect |
|---|---|---|
| `Menu` | declared `on_menu = "pause_open"` on the scene-file root, consumed by the walker layer | fires the `pause_open` intent → the scene pushes the pause overlay |
| `PrimaryAction` (press) | `GameplayBase` (runs only on a Pass past the HUD/chat) | ray-picks a cluster face → sets the inspector selection |
| `LookLeft` / `LookRight` / `LookUp` / `LookDown` | `signals.pointer_delta(..)` (mouse, right-drag gated by the profile) **and** `signals.axis(..)` (stick) | camera yaw/pitch |
| `MoveForward` / `MoveBackward` / `StrafeLeft` / `StrafeRight` | `signals.axis(..)` (unifies held keys + stick into one 0..1 path) | walk/fly movement in the XZ plane |
| `MoveUp` / `MoveDown` | `signals.axis(..)` | vertical movement (fly mode only) |

The camera reads look through the **signal** path (`pointer_delta` / `axis`), i.e. it is on
the compliant side of the open input conflict about direct-pointer camera reads (that
conflict is about `flicker-globe`, not this scene).

**Ruled raw-key exception (chat text entry, `CommandHandler`):** the in-world chat line is a
`TextEntry`-mode keyboard owner whose trigger (`T`), submit (`Enter`), and cancel (`Esc`)
are read from the **raw** input snapshot, not from action-map bindings — the sanctioned
text-entry exception (MCP 4B15929B; a proper `OpenChat` signal is owed). While chat owns the
keyboard the scene reports `TextEntry` from `Scene::input_context()`, the pump resolves the
(empty) TextEntry map, and every gameplay query reads zero — so movement/look/pick suppress
automatically. `CommandHandler::capture` also swallows any routed signal above the walker,
so `Menu` cannot open the pause menu while typing.

### Results / intents it fires

- **`pause_open`** — the walker fires this declared intent name; `update` maps it to
  `Transition::Push(PauseScene)` (handing the pause overlay the theme + the live look/pad
  controls). The scene root has **no** hardcoded Menu arm — the binding is data in the scene
  file.
- **`sig_<name>` mirror** — every intent that fired last frame is republished once into the
  next HUD Model as a transient `sig_<name>` key (S9), then dropped, so a script can observe
  a signal.
- **Chat side-effects** — the walker returns `chat_send` / `chat_join` / `chat_part` edges
  and the command handler returns submit/cancel; the scene turns these into `ChatCommand`s
  on the `ChatClient` (a leading `/` is a client command: `/join /part /leave /nick /names`).

### Model keys

The *Model* is the per-frame key→value table the engine hands the walker (and the pair
script). This scene publishes on two channels:

**To the stage recipe** — the *only* numbers the simulation feeds the authored passes, via
`stage_inputs()`. Each is a `*_bind` the `pocclusters_world` recipe names (the
`the_root_stage_authors_the_ground_fog_and_publishes_its_binds` gate proves the two sets
match exactly, so a bind can never resolve to nothing):

| Key | Consumed by | Meaning |
|---|---|---|
| `fog_floor` | `ground_fog` | lowest walkable nav floor (the fog slab sits 2 below it, 12 above) |
| `fog_density` | `ground_fog` | the Celestial Cycle's Fog control over its default (`fog / DEFAULT_FOG`) |
| `fog_time` | `ground_fog` (drift) | the scene's own clock, seconds |
| `grade_warmth` | `tonemap_grade` (`GradeStrength` slot) | golden-hour strength from the live sun elevation — 0 at noon and at night, peaking as the sun sits on the horizon |

The stage **clock** rides the same channel as a typed field (`inputs.clock(..)`), and it is
the *same* accumulator as `fog_time` — one scene clock, so the light drivers' flicker and the
fog's drift can never diverge, and both are deterministic rather than wall-clock.

**To the HUD tree** — `hud_model()` publishes raw runtime variables + stringtable-resolved
word tokens each frame; the pair script `pocclusters.lua`'s `derive()` composes them into the
display strings (the five-line split — Rust owns stringtable resolution, the script owns
composition). The full catalog of `bind` / `text_bind` names is the scene file's business
(see the Sensorium README). The scene **reads back** these two-way control results from the
walker each frame:

- Debug toggles (bool): `wireframe`, `arrows`, `navmesh`, `camera_lod`, `lod_billboards`,
  `walk` (surface-walk vs fly).
- Sliders (number): `move_speed`.
- Celestial Cycle: `cc_sun`, `cc_moon`, `cc_year`, `cc_speed`, `cc_fog`, `cc_lat`,
  `cc_epoch` (numbers) + `constellations`, `planets`, `celestial_paths` (toggles).
- Chat: `chat_tab`, `chat_scroll`, `chat_input`, and the `chat_send` / `chat_join` /
  `chat_part` button edges.
- `hud_hit` (pointer over any UI region) — gates the world-pick so a checkbox click does not
  also pick a face behind the panel.

Declared **surface** gates (`Sections`) published into both walker passes: `has_pick`
(inspector), `no_pick` (the "nothing selected" row), `chat` (the floating window's root,
always on today). A *surface* here is a declared, visibility-gated region of the screen.

### What it hands other crates / threads

- **Renderer:** the material draw palette (`set_material_palette`), uploaded cluster + grass
  meshes, the loading widget, and — for the sun shadow — the offscreen depth **Target** plus
  the light-view-projection via `set_shadow_source` (the consumer role; see Render below).
- **`prism-alpha`:** a `Transition::Push(PauseScene)` when the pause intent fires.
- **Workers:** a `flicker-worker` `WorkerPool` runs per-cluster derive+mesh+nav jobs off the
  main thread; results are generation-tagged and applied as a set once the whole field
  reports in (a stale generation is dropped). This is why the world "cooks" under the loading
  widget on entry.
- **Chat:** a `ChatClient` owns a background socket thread; inbound events are drained each
  Active frame and dropped on `exit`.

## Render composition

`render` declares surfaces into the frame graph (a *surface* runs a compiled *stage* — a
recipe of typed *passes* — into a composite target). Two surfaces plus an overlay:

1. **Sun-shadow producer** → an offscreen depth **Target** (`CompositeTarget::Target`). Runs
   the `pocclusters_sun_shadow` stage: renders the terrain casters from the sun's point of
   view into a depth map. The light-view-projection is captured *at the moment the depth
   renders* (the surface's clock is throttled to `rate {hz:20}`), so a throttled frame always
   samples the stale depth with the matrix it was actually drawn with.
2. **World** → the screen (`CompositeTarget::Screen`). Runs the `pocclusters_world` stage,
   whose recipe derives to this order:
   `sky → shadow_map → scene → water_surface → ground_fog → bloom → tonemap_grade`.
   The scene wires two resources into this surface: it binds the sun-shadow depth (the
   consumer `shadow_map` line) before the lit `scene` pass, and the Celestial Cycle owns the
   camera framing + the sky. The recipe authors **no** camera — the cycle does.
3. **HUD + chat overlay** — this frame's walker draw commands, then the floating chat window
   over them.

While `Booting`, `render` draws only the loading widget; the scene goes `Active` once the
whole field is meshed and every nav-range cluster has a nav surface.

Two contrasting wiring stories worth internalising:

- **Water is pure recipe data.** The flooded sea is a `water_surface` pass declared in the
  scene file (`sea_level 120`, five wave sources, env-lit by the live sky through
  `env_strength`). It reads the scene depth and writes HDR; there is **no** Rust `set_water`
  call and **no** reflection render-target — the water is real animated geometry.
- **The shadow needs Rust.** Its knobs live in data (the consumer's `light` + `bias` on the
  world recipe's `shadow_map` line; the producer's caster-box `extent` on the
  `pocclusters_sun_shadow` stage — read out by `shadow_knobs`), but the scene must hand the
  depth Target + captured matrix to the consumer each frame via `set_shadow_source`.

The **Celestial Cycle** overwrites the two sky **slots** of the stage's authored `LightRig`
(slot 0 the sun, slot 1 the moon — the same slots the sky pass reads back) and *composes
over* the rest, leaving the room's authored `hearth` point light (slot 2) standing and
flickering. It also draws the seven worlds on the ecliptic (equal apparent size, from the
shared orrery roster), the constellation figures (the gold Chalice + 12 placeholders — their
names/selection are kept for a later HUD pass, not yet surfaced), and the golden-hour warmth
that `grade_warmth` publishes.

## Gates

Run `cargo test -p flicker-pocclusters` (21 tests). The contract gates a change must keep
green:

| Test | Breaks if |
|---|---|
| `stage_tests::the_root_stage_authors_the_ground_fog_and_publishes_its_binds` | the world recipe's pass order changes, or the scene publishes a different key set than the recipe binds (`fog_floor`/`fog_density`/`fog_time`/`grade_warmth`), or the golden-hour warmth stops reaching the tonemap |
| `stage_tests::the_authored_shadow_knobs_reach_the_runtime` | the sun-shadow `light`/`bias`/`extent` authored in the scene file drift from what `render` reads, or land on the wrong stage/role |
| `stage_tests::the_water_floods_the_island_with_animated_waves` | the water pass loses its derived order (after `scene`, before fog/tonemap), its `sea_level 120`, its 3 radial + 2 directional wave sources, or its env-lit `env_strength`; or a `pocclusters_reflect` stage reappears |
| `stage_tests::the_hearth_survives_the_celestial_composition` | the Celestial Cycle stops composing over the authored rig and drops the room's hearth light |
| `script_smoke::the_pair_script_derives_the_display_strings` | `pocclusters.lua`'s `derive()` stops turning the raw Model into the HUD strings |
| `script_smoke::hud_tree_walks_with_model` | the authored HUD tree fails to walk with a real Model |
| `script_smoke::the_loader_reads_the_island_bake_set` / `the_nine_island_bakes_load_and_contour_nonempty` | the island bake set fails to load, or the contour fallback produces empty geometry |
| `grass_integration::grass_set_scatters_above_the_waterline` | the real promoted GrassField no longer scatters above the waterline (skips if the assets are not promoted) |
| `scatter::tests::*` (`only_places_above_the_waterline`, `drowned_or_degenerate_is_empty`, `is_deterministic`, `variant_mix_tracks_weights`) | grass placement stops staying above water, stops being deterministic, or stops tracking the variant weights |
| `celestial::tests::the_golden_hour_curve_peaks_low_and_is_zero_at_noon_and_night` | the warmth curve tints midday or the night, or is no longer smooth/bounded |
| `celestial::tests::the_disc_radii_match_the_shipped_sky_shader` | the sun/moon disc radii in `celestial.rs` and the bare literals in `sky.wgsl` drift apart (eclipse desync) |
| `route::tests::*` (6) | the input chain changes: the root's dead Menu arm returns, the pause intent stops firing (or fires while chat owns the keyboard), the T/Esc/Enter hand-off or the trigger-key guard breaks, or the world-pick stops requiring a `PrimaryAction` Pass |

## Sharp edges

- **Shadow-stage failure is silent.** If `pocclusters_sun_shadow` is renamed or fails to
  compile, `enter` falls back to `StageDef::default()` with **no** log (`unwrap_or_default`),
  so the world simply renders unshadowed — unlike the world stage, which logs an error on the
  same failure. The `the_authored_shadow_knobs_reach_the_runtime` gate is what actually
  catches this; a scene-file edit that skips the tests gets no runtime clue.
- **`SHADOW_SIZE` (2048²) is the one shadow knob not authorable from the scene file.** The
  shadow map resolution is a Rust const because `StageDef` has no resolution field and the
  attachment schema is a fraction of the surface rect, not an absolute texel count. `extent`,
  `bias`, `light`, and `rate` are all in data; only the size is in Rust (tracked in MCP).
- **Grass lives in its own field, not `self.meshes`.** It is uploaded once in `enter`; the
  LOD re-mesh (`drain_and_apply`) frees and rebuilds `self.meshes` every swap, so grass kept
  there would be freed. Grass is near-field-culled at `GRASS_VIEW_RADIUS` (130 voxels).
- **Coordinate/unit trap for props.** The world is **Y-up in voxels** (1 voxel = 6 in =
  15.24 cm); `sea_level` and `island_height` are voxel-Y values. Props are authored Z-up in
  cm, so each grass instance is `T(world) · Ry(yaw) · Rx(-90°) · Scale(cm→voxel)`. Using a
  prop's own `Model.world` would be ~15× too small.
- **The chat window is a bespoke second walker pass.** It runs its own `run_ui` over the
  floating panel with scene-owned move/resize (the walker has no window geometry); a proper
  chat-component design pass is owed (MCP). Its move/resize reads raw mouse state for window
  management, which is geometry, not input arbitration.
- **The world pick is brute-force CPU ray-triangle** over the ~9 cluster meshes — fine at
  this field size; add spatial acceleration if the field grows. The selection is dropped on
  every re-mesh (it was anchored to the old triangles).
- **Nav (and collision) exist only in surface-walk mode.** Fly mode generates no nav and has
  no collision; toggling `walk` re-meshes the field so nav appears/disappears with it.
