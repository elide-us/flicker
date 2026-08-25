# flicker-controllertester

The live **input-bus inspector** bench, and the reference *sub-signal diagnostics* scene: it
shows you the whole input pipeline at once — the raw controller and keyboard on the left, a
reference character acting out whatever the gameplay layer accepts in the centre, and a real
resolver→router chain reporting, per frame, which layer consumed each input on the right. Flip
the context tab bar (World / Menu / Radial / TextEntry) and watch the **same** physical button
resolve to a different signal and get consumed by a different layer. It is a diagnostic bench,
launched by name from the `prism-alpha` roster.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## The one thing to understand first: chrome over exhibit

This crate is deliberately split by a line that **no other scene has** (ruling **DC217431**).
Two halves share the screen:

- **The chrome** — the header, the mouse readout, and the context **tab bar** — is an ordinary
  migrated scene: an authored tree + pair script fed by the signal **pump**. It obeys the
  project's input law (all input is signals; nothing is wired to a device).
- **The exhibit** — everything below the tab bar (the controller diagram, the analog latch,
  the golem stage, and the three bus panels) — is **scene-drawn straight from the raw device
  snapshot**, on purpose. Its subject matter *is* the machinery under the signal layer, so it
  keeps raw pad/key/mouse/analog reads and runs its own demo resolver and router. **These raw
  reads are sanctioned by design — they are the thing being demonstrated, not a signal-bypass
  defect.** Do not "fix" the exhibit to consume signals; that would delete the bench.

The demo router chain is **display-only**: it still *shows* its scene-root layer consuming a
`Menu` press in the panels, but that consumption drives nothing — the scene's actual pause
rides the chrome's declared `Menu` intent through the pump.

**Vocabulary used below** (each is a flicker word): a **scene crate** is a library supplying
one `Scene` behaviour, paired with an authored scene file and a Lua pair script; the **chrome**
is the authored UI that frames a scene; a **signal** (`ActionSignal`) is a device-independent
input verb (`Menu`, `Confirm`, `TabNext`) — what *produces* it is profile data, out of scope
here; an **intent** is a signal binding declared as data on the scene file's root
(`"on_menu": "pause_open"`); a **result** is a name fired into the frame's results map; the
**Model** is the per-frame key→value table the engine hands to Lua and to the tree's binds; the
**pump** is the one dispatch of this frame's signals through the **walker** (the Rust pass that
lays out, hit-tests, navigates and draws the tree); a **surface** is the drawing ground the
scene's own 2D/3D content occupies under the UI; a **context** (`InputContext`) is which
binding map is on top of the stack; a **resolver** turns a raw input snapshot into fired signal
edges against a context's map; a **router** dispatches those edges down a handler chain and
records, in a **`DispatchReport`**, which layer consumed each; the **latch** is the analog
channel's volatile per-frame sample (`AnalogFrame`); the **golem stage** is the reference body
that plays locomotion from consumed signals.

## Where it sits

- **Builds on:** `flicker` (the umbrella: `Scene`/`Transition`, `Renderer`/`FrameGraph`,
  `ScriptHost`/`ValueMap`, and the walker entry points `run_ui`/`render_hud`) ·
  `flicker-input-core` (the `ActionSignal` vocabulary, `Resolver`, `ContextualBindings`,
  `InputContext`, `InputMap`, `InputState`, `AnalogFrame` — catalog in
  [`../../input/flicker-input-core/README.md`](../../input/flicker-input-core/README.md)) ·
  `flicker-input-router` (`Router`/`InputHandler`/`Flow`/`DispatchReport`/`InputEvent` — see
  [`../../input/flicker-input-router/README.md`](../../input/flicker-input-router/README.md)) ·
  `flicker-skeletal` (the golem stage's rig/clip/pose/skin path) · `flicker-core::roots` (the
  package tree the golem body resolves through) · `flicker-shell` (`PauseScene`, `Theme`,
  `input_profile`) · `serde_json` · `tracing`.
- **Used by:** `prism-alpha` only, and only through [`scene`](#public-api). Its roster entry is
  in [`../../../prism-alpha/src/main.rs`](../../../prism-alpha/src/main.rs) (`roster()`, id
  `"controllertester"`, title `"Controller Tester"`, realm `REALM_ADVENTURER`, category
  `Diagnostic` / `Input / Bus`).
- **Sibling reference:** [`../flicker-clicktrainer/README.md`](../flicker-clicktrainer/README.md)
  is the smallest complete 2D scene crate; this one is the same pattern plus a sanctioned
  sub-signal exhibit and a 3D root surface.
- **Reads from the content tree:**

| Path | When | If missing |
|---|---|---|
| [`content/sensorium/scenes/controllertester.scene.json`](../../../content/sensorium/scenes/controllertester.scene.json) | at launch, by the kernel — the parsed `SceneDef` is handed to `scene()` | no `tree` ⇒ `tracing::error!` at construction, and **the whole per-frame update short-circuits** (see Sharp edges) |
| [`content/sensorium/scripts/controllertester.lua`](../../../content/sensorium/scripts/controllertester.lua) | compiled in via `include_str!` (`src/lib.rs:85`), loaded in `from_parts` | load error ⇒ `tracing::error!` and the chrome binds go unfilled (no status line, no mouse line, no tab washes) |
| [`content/data/stringtable.json`](../../../content/data/stringtable.json) | per draw — the 28 `$ctt_*` tokens the chrome and the feed name | the raw token text draws |
| `package/characters/GolemBase_Low/` + `package/retarget/clips/locomotion/` | in `enter`, via `GolemStage::load` through `flicker-core::roots` | load error ⇒ the stage panel shows the error instead of a body (fail loud); the rest of the bench runs |

To change what the chrome looks like or says, edit the scene file — see
[`../../../content/sensorium/README.md`](../../../content/sensorium/README.md) for the authoring
format. This file does not re-teach it. The exhibit below the chrome is Rust drawing code, not
authored.

## Public API

Three items reachable from `lib.rs`. `GolemStage` (in the private `mod golem`) is **not**
re-exported — it is a crate-internal detail of the centre stage.

| Item | For | The one thing to know |
|---|---|---|
| `pub fn scene(def: &SceneDef) -> Box<dyn Scene>` | the roster factory — the only intended entry point | The `SceneDef` is the *parsed scene file*; the kernel resolves it from the manifest when the roster row fires. Delegates to `ControllerTester::new`. |
| `pub struct ControllerTester` | the `Scene` implementation | All frame state lives here. Nothing outside the crate constructs it today; `new`/`scene` is the seam a second host would use. |
| `pub fn ControllerTester::new(def: &SceneDef) -> Self` | the runtime constructor | Clones the authored tree + style blocks out of the def and loads the pair script. Builds **no** GPU state: `enter` does that (it needs the `Renderer`), including loading the golem. |
| `pub fn ControllerTester::shipped() -> Self` | a test/`Default` seam | **`#[cfg(test)]` only** parses the bundled scene file and builds a real bench with no app. In a non-test build it is `unreachable!(…)`, so `ControllerTester::default()` (which calls it) **panics** outside tests — the runtime path is always `new(def)`. |

The `Scene` trait methods it implements are `enter`, `update`, `render`. It leaves
`input_context`, `route`, `is_overlay`, `pointer_captured` and `exit` at their defaults.

**Tuning — compiled, not authored.** The exhibit's palette and geometry are private `const`s in
`src/lib.rs` (the `BG`/`PANEL_*`/`ON_*` colours, `TAB_Y`/`TAB_H`, the `KEYS` readout row, the
`CONTEXTS` ring) and in `src/golem.rs` (`DEAD`/`RUN_TILT` stick thresholds, `CLAY`/`GROUND`
tints). The scene file's `params` block is `{}` and is never read. Changing any of these means a
rebuild.

## Interactions

### 1. Signals the CHROME captures (the real input seam)

Signals only — never keys or buttons; what produces a signal is profile data, out of scope here.
The chrome runs **one** walker dispatch per frame
(`WalkerHandler::hud(...).with_nav(...).with_rects(...).with_intents(...)`, `src/lib.rs:837`),
so the tab bar is a fully navigable `tab_group` (pad/keyboard, not pointer-only).

| Signal | Channel | Effect |
|---|---|---|
| `Menu` | **declared intent** — `"on_menu": "pause_open"` on the `surface` root | fires `pause_open`; `src/lib.rs:914` returns `Transition::Push(PauseScene)`, built from the **profile's** `"World"` map (never the demo maps). |
| `TabNext` / `TabPrev` | **declared intents** — `"on_tab_next": "ctx_next"`, `"on_tab_prev": "ctx_prev"` | fire `ctx_next` / `ctx_prev`; `src/lib.rs:864` cycles the `CONTEXTS` ring (World→Menu→Radial→TextEntry, wrapping). |
| `Confirm` (or a pointer click) on a focused/hovered tab | the tab `button`'s `"action"` (`ctx_world` / `ctx_menu` / `ctx_radial` / `ctx_textentry`) | `src/lib.rs:854` selects that context directly. |
| `NavUp`·`NavDown`·`NavLeft`·`NavRight` (d-pad) · `PanelNext`·`PanelPrev` (stick) | subscribed via `with_nav` | move focus across the four tabs (nav-tier contract). |

The pre-migration tester polled the **raw** `Tab` key and pad `Back` button to cycle, and
hit-tested the mouse against the tab rectangles. Both were **deleted**: cycling and tab
selection are signals now.

### 2. The sanctioned SUB-SIGNAL exhibit (raw reads, by design — DC217431)

Everything here reads the raw device snapshot directly and runs its **own** demo resolver and
router. This is the bench's subject matter, not a bypass. None of it rides the Model or the
pump.

- **Raw `InputState` reads** (`src/lib.rs` render + `src/golem.rs`): `gamepad(0).button_down`
  and `axis_value` (the controller diagram), `key_down` for the 20-key `KEYS` row, the mouse
  raws, and `input.analog_latch()` → `AnalogFrame` (seq, staleness, per-frame stick/trigger
  deltas — the analog panel).
- **The demo resolver** (`src/lib.rs:876`): `Resolver::resolve_frame` turns the snapshot into
  `Fired` edges against the **selected** context's `demo_bindings()` map (`world_map` /
  `menu_map` / `radial_map` / `text_map`). Because the same physical button lives in different
  maps, it resolves to a different signal per tab — the point of the bench.
- **The demo router chain** (`src/lib.rs:890`), highest priority first — a real `Router`
  dispatch producing a `DispatchReport`:

  | # | Layer | Role in the exhibit |
  |---|---|---|
  | 0 | `SystemLayer` | capture-only; claims `Quit` before anyone sees it |
  | 1 | `SceneRoot` | declares base `World`; consumes the `Menu` press **for display** (drives nothing) |
  | 2 | `ModalLayer` | declares the active non-World context; **exclusive capture** for `TextEntry`, consumes UI signals for `Menu`/`Radial` |
  | 3 | `GameplayBase` | last; consumes the gameplay-group signals — these are what animate the golem |

- **The golem stage** (`src/golem.rs`): folds the report into locomotion — **only** signals
  layer 3 actually consumed become motion, so pushing a modal context visibly stops the body.
  That is the routing made physical, not a bug.

The signal partition is exhaustive on purpose: `is_ui_signal` (`src/lib.rs:256`) has no `_`
arm, so a newly added `ActionSignal` variant fails to compile until it is classified —
the exhibit can never silently drop one.

### 3. Results it fires (through the walker)

| Result | Produced by | Consumed by |
|---|---|---|
| `pause_open` | the declared `Menu` intent | `src/lib.rs:914` → `Transition::Push(PauseScene)` |
| `ctx_next` / `ctx_prev` | the declared `TabNext` / `TabPrev` intents | `src/lib.rs:864` cycles the context ring |
| `ctx_world` / `ctx_menu` / `ctx_radial` / `ctx_textentry` | the four tab `button` actions | `src/lib.rs:854` selects that context |

**Exits: none.** The crate does not implement `Scene::route`; the only stack move it makes is
the pause `Push`.

### 4. Model keys (chrome only — the exhibit never touches the Model)

Two hops, the standard split: Rust publishes **raw** values, the pair script's `derive()`
returns **display** values, and the merged map is what the tree's binds read.

| Key | Type | Published by | Read by |
|---|---|---|---|
| `active_ctx` | Text (`"world"`/`"menu"`/`"radial"`/`"textentry"`) | `hud_model` `src/lib.rs:565` | `controllertester.lua` — picks the tab washes |
| `connected`·`slots`·`tick` | Bool / Number / Number | `hud_model` | `controllertester.lua` — composes the status line |
| `mouse_x`·`mouse_y`·`mouse_l`·`mouse_r`·`mouse_m` | Number / Bool | `hud_model` | `controllertester.lua` — composes the mouse line |
| `w_gamepad0`·`w_connected`·`w_not_detected`·`w_slots`·`w_tick`·`w_mouse` | Text | `hud_model` (resolved from `$ctt_*` tokens) | `controllertester.lua` — the words it slots into the readouts |
| `status` | Text | `controllertester.lua:43` | tree `text_bind` (header) |
| `status_color` | Text (style path) | `controllertester.lua:46` | tree `color_bind` (header) |
| `mouse_line` | Text | `controllertester.lua:50` | tree `text_bind` (mouse cell) |
| `ctx_world_style`·`ctx_menu_style`·`ctx_radial_style`·`ctx_textentry_style` | Text (style path) | `controllertester.lua:37` | the four tab buttons' `style_bind` |
| `sig_<name>` | Bool `true`, transient | `UiIntents::mirror_into` `src/lib.rs:613` | **nothing** — published for scripts that observe fired intents; this pair does not |

The tree binds exactly seven keys: `status`, `status_color`, `mouse_line`, and the four
`ctx_*_style`. All are produced by `derive()`, so there is no unbound-key hole — but a
`text_bind`/`style_bind` naming a key the Model does not carry draws nothing, silently (see
Sharp edges).

### 5. Style paths the tree names

All resolve inside the scene file's own `styles.controllertester` block:
`controllertester.title.color` · `controllertester.dim.color` (static), and — reached through
the binds above — `controllertester.tab_active` / `controllertester.tab_idle` (the tab washes,
picked by `derive()`) and `controllertester.ok` / `controllertester.off` (the status colour).

### 6. What it hands the shell

- One `FrameGraph` per frame: `fg.root` draws the **3D golem** as the screen surface's element
  (straight into the swapchain), then `fg.overlay` draws the 2D exhibit and blits the walker's
  chrome commands last (via `render_hud`), so the chrome sits over the stage.
- `Transition::Push(PauseScene)` on `pause_open`, from a `Theme` built once in `enter`.
- The window title, set once in `enter` (see Sharp edges).
- One live skinned-mesh upload per frame for the golem, freed before the next (`src/golem.rs`).

No threads, no workers, no async.

## Gates

`cargo test -p flicker-controllertester` — **11 tests, all green** (the golem content-load test
skips when the package tree is absent).

| Test | What it holds |
|---|---|
| `tests::the_shipped_scene_authors_the_chrome` | The real scene file parses; the root declares `on_menu`/`on_tab_next`/`on_tab_prev`; one tab button per context carries its `ctx_*` action; every component kind is known; every display literal is a `$token`; no raw display copy is published from `lib.rs`. |
| `tests::dispatch_fires_the_declared_chrome_intents` | Through the real tree, a `Menu` press fires `pause_open` and a `TabNext` fires `ctx_next` at the walker layer. |
| `tests::the_pair_script_derives_the_chrome_readouts` | `derive()` yields non-empty display text for `status` and `mouse_line`, the selected context's tab wears `tab_active` and the rest `tab_idle`, and selecting a context moves the active wash with it. |
| `tests::world_routes_signals_to_expected_layers` | The demo chain: System captures `Quit`, Scene root consumes `Menu`, Gameplay base consumes `Dodge`. |
| `tests::menu_context_routes_ui_to_modal` | With `Menu` active, the Modal layer consumes UI signals (`Confirm`). |
| `tests::textentry_owner_captures_everything` | With `TextEntry` active, the Modal layer is an exclusive owner and captures in the capture pass. |
| `tests::owner_follows_active_context` | The focus-chain owner is the layer whose declared context equals the active one, across World/Menu/TextEntry. |
| `tests::demo_maps_resolve_context_sensitively` | The same `East` button resolves to `Dodge` in World and `Cancel` in Menu. |
| `golem::tests::signals_map_to_the_packs_state_vocabulary` | The digital signal→pack-state table (Idle/Walk/Run/strafe/back/crouch families; Jump outranks all). |
| `golem::tests::the_stick_walks_then_runs_by_tilt` | The analog channel: dead zone stays Idle, gentle tilt walks, full tilt runs, direction follows the dominant axis. |
| `golem::tests::the_golem_loads_and_signals_move_the_machine` | The real content round-trip: loads the body + shared clips + pack, guards Y-up orientation on both the bind pose and the animated Idle pose, and drives one frame into `Run` (`run_jog`). |

Two gates in `prism-alpha/src/main.rs` also cover this crate: the roster pin for the migrated
benches, and the check that every roster id resolves to an authored scene file.

## Sharp edges

- **The exhibit's raw reads are sanctioned — do not "signal-ify" them.** Ruling DC217431 makes
  this bench the one surface whose subject matter is the sub-signal machinery. A red-team pass
  that flags the raw pad/key/mouse reads as a `37722F91` bypass is wrong here; the boundary is
  the tab bar, and everything below it is the exhibit.
- **The demo chain is display-only.** The focus-chain and consumed panels show `SceneRoot`
  consuming a `Menu` press, but that consumption drives nothing — the real pause rides the
  chrome's declared `on_menu`. Reading the panel as "the demo chain opened the pause" is wrong.
- **A missing chrome tree freezes the whole exhibit, not just the chrome.** `update` takes the
  authored tree at the top and early-returns `Transition::None` if it is absent
  (`src/lib.rs:817`); the resolver, the demo dispatch and `stage.drive` all sit **after** that
  return. So a scene file with a broken/removed `tree` disables the sub-signal exhibit too,
  after only a one-time construction-time error log — even though the exhibit is conceptually
  independent of the chrome.
- **`ControllerTester::default()` panics outside tests.** `Default` calls `shipped()`, which is
  `unreachable!(…)` in a non-test build. This is a deliberate loud fail, but a host reaching for
  `.default()` in release gets a panic, not a blank scene.
- **A typo'd tab action fails to nothing.** The `ctx_*` matcher in `update` compares against a
  fixed list; a scene file that ships `"action": "ctx_wrld"` produces a tab that lights and
  navigates but selects no context, with no warning (`4BB12A75` seam).
- **A typo'd `style_bind`/`text_bind`/style path fails to nothing.** A missing style segment
  resolves to null and every reader falls back to a compiled default; a misspelled bind key
  draws an empty string. Nothing warns.
- **Losing the pair script is a soft failure.** If `controllertester.lua` fails to load,
  `script` is `None`, the chrome binds go unfilled (no status line, no mouse line, tabs stuck on
  their default wash), and only a `tracing::error!` marks it. The derive gate is what keeps that
  off the screen.
- **The golem stopping is the routing, not a hang.** Under any pushed (non-World) context the
  gameplay-base layer consumes no gameplay signals, so the body stands still. That is the
  inspector working.
- **The stage is framed by four opaque "hole-punch" bands, and this is why chrome order
  matters.** A `FrameGraph::root` element fills the window and cannot be scissored to the centre
  rect, so the overlay paints four background bands around the stage viewport and the chrome
  must draw last, over them. This is a **tracked, banked** deferral (the fix is a nested
  `surface` node + a surface slot — a scene-migration slice, marked `// LEGACY (banked)` at
  `src/lib.rs:977`), not a live defect to solve here.
- **The window title is set in `enter`.** A pre-existing standalone habit — in the unified
  launcher the shell, not the scene, owns the title. Harmless, but the scene reaches past its
  seam to set it.
- **Colours in `styles.controllertester` are raw rgba quads.** The block hard-codes colour
  arrays rather than `$token` refs; editing a tab colour here means typing a quad. This is one
  instance of the project-wide colour-token sweep, not unique to this file.
