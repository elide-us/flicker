# flicker-scene

The application **kernel**. It owns the running set of **scenes** — one screen or mode of
the app (a logo, the menu, the loading screen, the game, a pause modal) — drives exactly the
active one each frame, and swaps between them on request. A game becomes a running window in
one line: `flicker_app::run(SceneManager::new(initial))`. This crate defines the `Scene`
contract every scene implements, the `Transition` vocabulary a scene uses to reshape the
flow, and the `SceneManager` that plays them.

It is the hub of the frontend cluster: every scene behaviour crate implements `Scene` against
it, so this README is what those crates link for the contract they satisfy.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Builds on:**
  - `flicker-app` — the `App` trait `SceneManager` implements, the `run` entry point, and the
    per-frame input struct (`FrameInput`, re-exported here as [`SceneInput`](#input-plumbing)).
  - `flicker-input-core` — `InputContext` and `InputState` (used in the `Scene` signatures;
    see [`flicker-input-core/README.md`](../../input/flicker-input-core/README.md)).
  - `flicker-render` — `Renderer` and `FrameGraph`, the draw-declaration surface a scene
    records into.
- **Used by:**
  - `flicker-widgets` — `scene_def.rs` defines the **scene file** (`SceneDef` / `SceneManifest`
    / `SceneExit`) and depends on this crate for `Transition` + `GotoMode`
    (`../flicker-widgets/src/scene_def.rs`).
  - `flicker-shell` — builds the **roster** (`SceneEntry`, `SceneFactory`) and constructs the
    `SceneManager` at boot (`../flicker-shell/src/shell.rs`). The shell's splash / logo /
    loading scenes are the reference `Scene` implementations.
  - every crate under `Alpha/crates/scenes/` — each bench (`flicker-solarbirth`,
    `flicker-pocclusters`, `flicker-clicktrainer`, …) implements `Scene`
    (see [`flicker-clicktrainer/README.md`](../../scenes/flicker-clicktrainer/README.md) for a
    worked example).
- **Reads from the content tree:** nothing directly. The manager is scene-agnostic plumbing;
  scene *files* under `Alpha/content/sensorium/scenes/` are read by `flicker-widgets`
  (`SceneManifest::load_dir`) and turned into scenes by the client's roster. This crate never
  names a scene, a file, or a Model key.

## Public API

### Implementing a scene — the `Scene` trait

A scene overrides only what it needs; every method but `update` and `render` has a default.

| Method | What it is for | The one thing to know |
|---|---|---|
| `update(dt, input, signals, renderer) -> Transition` | Advance one frame; return how to reshape the flow | Called on the **top scene only** — scenes beneath an overlay are frozen. Route `signals.events` through your handler chain; return `Transition::None` to stay put. |
| `render<'f>(&'f mut self, renderer, fg)` | **Declare** this scene's draw into the frame's shared `FrameGraph` | Declare-only: record your layers into `fg`; never call `fg.execute` — the manager owns the one graph per frame and executes it once. `'f` lets you borrow your own fields into the deferred draw closures. |
| `enter(renderer)` | Called once when the scene becomes active | Upload textures / build GPU state here. Runs during `render` (needs `&mut Renderer`), one frame after the transition that added the scene. Default: no-op. |
| `exit(renderer)` | Called once when the scene is removed | Free GPU resources here. Default: no-op. A revealed scene (after `Pop`) is **not** re-entered. |
| `is_overlay() -> bool` | `true` = keep the scene below visible beneath this one (a pause modal) | Default `false` (opaque, fully covers the screen). Drives which scenes are drawn and the `Paused` phase. |
| `input_context() -> Option<InputContext>` | Which input context the scene's active surface owns | **The authoritative context seam today** — the runner reads it (via `App::active_context`) and resolves the frame's signals for it. `None` (default) = the `World` base. Only the top scene's is consulted. |
| `pointer_captured() -> bool` | `true` = ask the runner to grab + hide the OS cursor (exclusive camera control) | Default `false` = free-mouse play. Only the top scene's is consulted; a pushed overlay defaults `false`, so opening a pause menu releases the cursor with no extra wiring. |
| `route(result) -> Option<Transition>` | Where a fired **result** name sends this scene | Consulted by the kernel on `Transition::Fire`. Default `None` = the scene names no destination for that result (**ordinary** — most results are handled in-scene). A file-backed scene returns `self.def.exit(result)` so the chain lives in DATA. |

**Vocabulary.** A **signal** is a resolved, device-agnostic input event (`Confirm`, `Cancel`,
`Menu`, `NavUp`, …; the catalog is `flicker-input-core`) — never a key or button. A **result**
(or **intent**) is a name a scene fires for *what happened* (`"done"`, `"quit"`), with no
knowledge of what comes next. An **exit** is a scene file's mapping from a result name to a
target scene id — see the scene file docs in `../flicker-widgets/src/scene_def.rs`.

### Reshaping the flow — `Transition`

`update` returns one of these; the manager applies it at the top of the next `render`.

| Variant | Effect | Note |
|---|---|---|
| `None` | Stay on the current scene | The common case. |
| `Replace(Box<dyn Scene>)` | Swap the top scene (exit old, enter new) | logo → menu → loading → game. |
| `ReplaceRoot(Box<dyn Scene>)` | Unwind the **whole** stack (exit each, top-down), start over | pause → main menu. `Replace` would orphan the scenes beneath. |
| `Push(Box<dyn Scene>)` | Overlay a scene, freezing the one below | game → pause. The scene below is untouched. |
| `Pop` | Remove the top, revealing the one below | pause → game. The revealed scene is **not** re-entered. Popping the last scene quits. |
| `Quit` | Exit the application | — |
| `Goto { id, mode }` | Go to the scene registered under `id`, letting the manager build it | The id-addressed form of Replace/ReplaceRoot/Push — the chain lives in the roster, not in constructor calls. A missing id is a **loud no-op** (logged `error`, stack untouched). |
| `Fire(String)` | Fire a named result; the **kernel** decides where it goes | Consults the active scene's `route` (its file's `exits`). An unrouted result is a **loud no-op** (logged `warn`). This is how a scene names an intent with zero knowledge of its successor. |

`GotoMode` (`Replace` / `ReplaceRoot` / `Push`) is the enum a roster spells to pick which of
the three forward moves a `Goto` performs — the same three stack moves, named so a scene file
can request one by string.

### Running the kernel — `SceneManager`

| Item | What it is for | The one thing to know |
|---|---|---|
| `SceneManager::new(initial) -> Self` | Start on a constructed scene | The whole game is then `flicker_app::run(SceneManager::new(initial))`. |
| `SceneManager::from_roster(entry, resolver) -> Option<Self>` | Start on the scene registered under `entry`, so even the entry point is roster data | Returns `None` when `entry` is not registered — a fatal boot misconfiguration the caller should panic on loudly (the shell does). |
| `.with_resolver(resolver) -> Self` | Wire a roster onto a `new`-built manager so its scenes can use `Transition::Goto` | Builder; without a resolver, every `Goto` logs and does nothing. |
| `.with_cursor(Option<CursorImage>) -> Self` | Register a custom hardware cursor at window creation | Appearance only — cursor visibility/capture is owned by the input-modality layer. |
| `.phase() -> Phase` | Read the kernel's lifecycle phase | **Observe-only today** — nothing gates on it yet (see Sharp edges). |

`SceneResolver = Box<dyn Fn(&str) -> Option<Box<dyn Scene>>>` is the injected id→scene lookup.
The manager deliberately does **not** own the roster: scene ids and their factories are a
client concern (`flicker-shell`'s `SceneEntry` / `SceneFactory`), and a crate this low must not
depend on the app above it.

### `Phase` — the lifecycle state

The kernel is always in exactly one phase, **derived** from the stack after each transition.

| Phase | Meaning | Reachable today? |
|---|---|---|
| `Starting` | Before the first scene is live | Yes — a freshly built manager, until `App::init` runs the first `enter`. |
| `Running` | The active scene is live | Yes — the resting state of an opaque top scene. |
| `Paused` | The top scene is an overlay; the one beneath is frozen | Yes — whenever an overlay (pause modal) is on top. |
| `Stopping` | Nothing left to run (quit, or the stack emptied) | Yes. |
| `Loading` | Bringing a scene's resources in | **Not yet** — entered by the resource-lifecycle step (unfinished wiring; see Sharp edges). |
| `Unloading` | Releasing a departing scene's resources | **Not yet** — same. |

<a name="input-plumbing"></a>
### Input plumbing types

| Item | What it is |
|---|---|
| `SceneInput<'a>` | Alias for `flicker_app::FrameInput` — the frame's resolved input, surfaced under the scene vocabulary. `signals.events` is the discrete signal bus; `signals.axis()` / `pointer_delta()` / `held()` are the continuous (analog / pointer-look) queries. |
| `FrameInput`, `InputEvent`, `RouteCtx` | Re-exported from `flicker-app` for convenience. `RouteCtx` is the router scratch a scene routes `events` into; the runner reconciles its context requests against the shared stack after `update`. |

## Interactions

- **Signals it captures — none, directly.** The manager is plumbing: it forwards the frame's
  resolved `signals` to the **top scene**, which composes its own handler chain and decides
  what to do. flicker-scene names no signal and wires nothing to a key (all input is signals —
  the pointer included; a scene subscribes to the signals it cares about).
- **Results / intents it routes.** A scene returns `Transition::Fire(name)`; the kernel calls
  the active scene's `route(name)` — backed by the scene file's authored `exits` — and enacts
  the returned `Goto`. `Fire` and `route` may chain (`resolve_indirection` loops until it
  reaches a concrete move). Today the shell's splash / logo / loading scenes use this chain
  (firing `next` / `exit`); bench scenes leave `route` at its default and are launched /
  returned via the menu firing `Transition::Goto` directly.
- **Context it forwards.** `App::active_context()` returns the top scene's `input_context()`,
  and `App::pointer_captured()` returns the top scene's `pointer_captured()` — the runner
  honors both (`../../platform/flicker-app/src/runner.rs`).
- **What it hands other crates.** One `flicker_render::FrameGraph` per frame: the manager
  stamps each visible scene's depth band (`stack position × SCENE_LAYER_STRIDE = 100.0`) onto
  the graph, lets every visible scene declare into it, and executes it exactly once — so an
  overlay can no longer erase the scene beneath it by running a graph of its own.
- **Model keys — none.** The Model (the per-frame key→value table a scene's walker binds
  against) lives inside a scene, not in the manager.
- **Threads / workers / async — none.**

## Gates

The crate's contract is pinned by five tests (`cargo test -p flicker-scene`, all green):

| Test | What breaks it |
|---|---|
| `cursor_passthrough` | `with_cursor` no longer feeds `App::cursor` verbatim, or a bare manager stops defaulting to `None`. |
| `visibility_slice` | `visible_start_in` stops picking the lowest scene to draw (the topmost opaque scene plus any overlays above it). |
| `two_visible_scenes_declare_into_one_graph` | A base scene and an overlay stop sharing the one `FrameGraph`, or stop getting distinct depth bands (the overlay could erase the scene beneath). |
| `kernel_phase_machine` | `settled_phase` stops being a pure function of quit + top-overlay, or a fresh manager stops starting in `Phase::Starting`. |
| `fire_routes_through_the_active_scenes_exits` | The `Fire → route → Goto → resolve` chain breaks, or an unrouted result stops being a loud no-op. |

The `enter` / `exit` / `apply_pending` wiring needs a live GPU `Renderer`, so it is exercised
in-window (verified by eye), not in these tests; the pure derivations above carry the logic.

## Sharp edges

- **Transitions apply during `render`, not `update`.** A scene requests a reshape from
  `update`, but the manager applies it at the top of the *next* `render` — because
  `enter` / `exit` need `&mut Renderer` and `update` only borrows it immutably. So a scene's
  `enter` runs one render call after the transition that added it.
- **`Pop` does not re-enter the revealed scene.** Only `enter` (on add) and `exit` (on remove)
  fire; a scene revealed by popping the overlay above it keeps whatever state it was frozen
  with, and gets no callback.
- **Two input channels, mid-migration.** `Scene::update` receives both `signals: &mut SceneInput`
  (the migrated path — resolved signal events + the continuous queries) and `input: &InputState`
  (raw device state). A scene converted to the central pump reads only `signals`; a
  not-yet-converted scene ignores `signals` and re-resolves from `input` internally. Both paths
  are live at once (a tracked half-migration — input-P3). Prefer `signals` in new scenes.
- **`input_context()` is the authoritative context seam; a parallel one is not yet wired.**
  The runner derives the active context from `Scene::input_context()`. A second representation
  exists — `InputHandler::declares_context()` in `flicker-input-router` — that the router's own
  docs say the runner should derive context from, but nothing production reads it yet. Until
  that wiring lands, treat `Scene::input_context()` as the single source; do not rely on a
  root handler's declared context taking effect.
- **`phase()` is observe-only.** Nothing in the app reads it and nothing gates on it yet, and
  `Loading` / `Unloading` are never entered — synchronous transitions settle straight to
  `Running`. It is the seam the resource-lifecycle step will drive (stream on `Loading`, free on
  `Unloading`, freeze on `Paused`); reading it today tells you `Starting` / `Running` /
  `Paused` / `Stopping` only.
- **A missing id / unrouted result fails loud but does not stop the frame.** `Goto` to an
  unregistered id logs `error`; `Fire` of an unrouted result logs `warn`; both leave the stack
  untouched and continue. The one hard-fail is boot: `from_roster` returns `None` for an
  unresolved entry, and the shell panics on it.
