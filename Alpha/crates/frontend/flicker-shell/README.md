# flicker-shell

The reusable **game front-end**. Every flicker game is one in-game scene plus the
same wrapper around it: an intro splash, a main menu, a settings screen, and a
pause overlay, all sharing one Prism chrome and one `settings.json`. That wrapper
is identical across games, so it lives here as a service instead of being copied
into each client. A client hands [`run`](#public-api) its launchable scenes and
gets the whole front-end — flow, window loop, persistence, update check — for a
five-line `main`.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

### Flicker words used below
- **Scene** — one screen/state the app is in (a splash, the menu, your game). The
  engine runs a stack of them; `flicker-scene` owns the stack.
- **Behaviour** — the *kind* of a scene, named by a string in the scene file. The
  shell knows three (`splash`, `menu`, `loading`); a client registers its own.
- **Signal** — an abstract input intent (`Confirm`, `Cancel`, `NavUp`, `Menu`…).
  Nothing here binds a key; the engine resolves the player's device into signals
  and the shell reads only signals. The catalog lives in `flicker-input-core`.
- **Result** — a named thing a screen fires when a button is hit (`quit`,
  `settings`, `resume`…). The scene routes results to transitions.
- **Model** — the per-frame `key → value` table the walker draws from and Lua
  reads; a scene *publishes* keys and the tree *binds* them.
- **Pair script** — the `<SceneName>.lua` beside a scene file; it configures
  hardened Rust components, never structure or logic (security law). The Lua↔Rust
  boundary is [`flicker-script`](../../scripting/flicker-script/README.md); the
  components it configures are [`flicker-widgets`](../flicker-widgets/README.md).
- **Realm** — a play-mode tier on the launcher menu (Adventurer / DM / Game
  Master / Developer); a launchable scene tags into one.
- **Token** — a `theme.tokens` colour name (`stone1`, `bronze`) or a `$string`
  stringtable key; both resolve from content, never hardcoded.

## Where it sits

Frontend cluster — a client sits *beside* this crate, not below it.

- **Builds on:** `flicker` (the engine umbrella — render/scene/script/ui/app/net),
  `flicker-content` (the content-root + package seam it reads scenes/assets
  through), `flicker-input-core` (signals, `InputProfile`, `InputMap`),
  `flicker-input-router` (the focus/nav walker wiring the menu uses).
- **Used by:** `prism-alpha` (the shipped app — it calls `run` with the full
  scene roster) and every launchable scene crate whose pause menu is
  [`PauseScene`] and whose HUD calls [`publish_signal_bindings`]
  (e.g. `flicker-solarbirth`).
- **Reads from the content tree** — the front-end's screens are content, embedded
  at **compile time** (so a missing file is a build failure, not a black window)
  except the scene *roster*, which is read at runtime so a scene is authored
  without a recompile. How to author any of these lives in
  [`../../../content/sensorium/README.md`](../../../content/sensorium/README.md);
  this crate only loads them.

  | Content | When read | If missing / broken |
  |---|---|---|
  | `scenes/` folder (the roster) | runtime, once at `run` (via the manifest) | **fatal panic in the terminal** before the window opens — no roster, no boot scene |
  | `scenes/Main.scene.json` + `scripts/Main.lua` | compile-time embed; parsed at menu enter | build failure; a parse failure logs loud and draws an empty surface |
  | `scenes/shared/{pause,confirm,settings}.scene.json` + `scripts/shared/{pause,confirm,settings}.lua` | compile-time embed; parsed when the overlay opens | build failure; a parse failure falls back to a bare screen that still declares its close intent |
  | `scripts/{TegLogo,CeLogo,Loading}.lua` | compile-time embed | build failure |
  | `resources/ui_theme.json` (the one palette) · `resources/ui_style.json` | compile-time embed | build failure; an unknown token renders **magenta** + warns |
  | `data/stringtable.json` | compile-time embed; loaded for the persisted locale at `run` | build failure; an unknown `$token` shows the raw token |
  | splash images (by content-relative path) | runtime, per splash | the splash plays its backdrop and still advances; logs the full path |
  | fonts, `muse.png`, `cursor.png`, `prism_pad_glyphs.png` | compile-time embed | build failure; a decode failure degrades (system font / dropped sprite / platform arrow) |

## Public API

Everything below is reachable from the crate root; the splash/menu/settings/
confirm/loading scenes and all persistence are internal. Types not defined here
are linked to their owning crate.

### Entry point & configuration

| Item | What it is for | The one thing to know |
|---|---|---|
| `run(ShellConfig) -> anyhow::Result<()>` | The single call a client makes. Restores the saved window, runs splash → menu → *your scene* → pause/settings, owns the winit loop. | **Blocks** until the window closes. Panics before opening a window if the scene folder will not index (fail-loud by design). |
| `struct ShellConfig` | What the client hands `run`: `scenes: Vec<SceneEntry>`, `settings_dir: Option<PathBuf>`, `scene_select: bool`, `app_version: &'static str`. | `scene_select=true` renders the scenes as a launcher panel (using each `SceneInfo`) instead of plain menu buttons. A non-empty `app_version` (prism-alpha's, not this lib's) lights the version line + one-shot update check; empty = no network. |
| `ShellConfig::single(dir, label, factory)` | The common one-scene client: a single launch button. | `label: None` ⇒ "ENTER WORLD". The factory takes no args (the synthetic scene file is handled for you). |
| `struct SceneEntry` (`::new`, `.with_info`, `.with_realm`) | One launchable scene: `id`, `label`, Prism `variant` (`primary`/`secondary`/`danger`), and its `factory`. | The `id` is what a scene file's `behaviour` names to bind this factory, and the result name the menu fires to launch it. `.with_realm` is repeatable (a tool shared across realms). A bare entry (no info, no realm) stays a root-menu button. |
| `type SceneFactory = Rc<dyn Fn(&SceneDef) -> Box<dyn Scene>>` | Builds the client scene, receiving its authored [`SceneDef`](../flicker-scene/README.md). | `Rc`, not `Box`: the menu and a "return to menu" rebuild reuse one factory set. The shell never names a scene type. |
| `struct SceneInfo` (`::new`) | Rich launcher-row metadata (`name`, `mode`, `region`, `desc`, `meta`). | Only used when `scene_select` is on and the entry carries it; a plain button needs just the `SceneEntry` label. |
| `REALM_ADVENTURER` · `REALM_DM` · `REALM_GAMEMASTER` · `REALM_DEVELOPER` | The four play-mode realm tags for `.with_realm`. | A realm-less entry stays on the root menu; a realm'd entry lists on that realm's tier-2 page. |
| `builtin_behaviours() -> Vec<&'static str>` | The behaviour names the shell builds itself (`splash`, `menu`, `loading`). | For a client's manifest gate to assert every *other* authored behaviour has a registered `SceneEntry`. |
| `user_settings_dir(app: &str) -> PathBuf` | The per-user config dir for an **installed** build (`%APPDATA%` · `~/Library/Application Support` · `$XDG_CONFIG_HOME`), where the install tree is read-only. | Pass the result as `ShellConfig.settings_dir`. Best-effort; falls back to `"."` so startup never blocks. |

### The pause overlay & theme (what a client pushes / draws with)

| Item | What it is for | The one thing to know |
|---|---|---|
| `struct PauseScene` · `PauseScene::new(theme, input_map, controls, gamepad_config)` | The pause pop-up a client pushes (`Transition::Push`) when the player opens the menu. Resume / Settings / Main Menu / Quit. | Reuses the game's already-built `Theme`. **Only `theme` + `input_map` are used** — `controls` and `gamepad_config` are accepted and ignored (see [Sharp edges](#sharp-edges)). |
| `struct Theme` (`::build`, `.lua_textures`, `.draw_loading`) | The shared Prism chrome: registers the six serif faces, uploads white + the Muse + the controller-glyph atlas, and draws the one Rust-drawn widget (the loading panel). | `Copy` (handles only). Every colour is a `theme.tokens` lookup, so the Rust-drawn loading screen can never drift from the Lua screens. A client builds one to draw its own loading widget while its world cooks and to hand to `PauseScene`. |

### The input seam (settings ⇄ the running game)

The settings screen lives in the shell; the running game applies the player's
choices through these. All four read the one persisted `InputProfile` /
`GameSettings`; none binds a key (signals only).

| Item | What it is for | The one thing to know |
|---|---|---|
| `input_profile() -> InputProfile` | The whole persisted profile (per-context keybinds incl. World rebinds, analog tuning, gamepad config). | A scene SEEDS its `InputMap` from this at enter, so last session's rebind is live on frame one. |
| `current_world_map() -> InputMap` | The committed `World` context map, cheaper than cloning the whole profile. | The runner polls this each frame through the pump's rebind seam; a live rebind reaches every pump-driven scene. Non-draining. |
| `input_controls() -> AbstractControls` | The mouse-LOOK controls only (sensitivity + invert) for a scene to seed at enter. | Carries look only — a scene keeps its own `move_speed`. |
| `take_pending_input() -> Option<(InputMap, AbstractControls, GamepadConfig)>` | Drains a settings change made mid-game (pushed on Apply/Back) for the scene to apply live. | `None` when nothing changed since the last poll. The returned `GamepadConfig` is always the default (see [Sharp edges](#sharp-edges)). |
| `publish_signal_bindings(model, map, signals)` | The one place a scene HUD turns an authored **signal** into the key/glyph the player presses. Per signal it sets `bind_<Name>` (keycap text), `glyph_<Name>` (pad atlas cell), plus one `input_device`. | Keys are the PascalCase `ActionSignal::name()` (`bind_Interact`, `glyph_Menu`). The scene authors `"signal": "Interact"` on a footer `option`; the walker ([`flicker-widgets`](../flicker-widgets/README.md)) picks the face by `input_device`. |

## The shell furniture (the front-end flow)

`run` boots the scene the **manifest** marks `boot` and follows the chain the
scene *files* route. The shell only knows three behaviours; everything else is a
file in `content/sensorium/`, so a new splash or menu page is authored, not coded.

| Screen | Behaviour | Where it is authored | What it does |
|---|---|---|---|
| Publisher / engine splash | `splash` | `scenes/*.scene.json` + `scripts/{TegLogo,CeLogo}.lua` | Plays one logo on a fade/hold timeline, then fires `next` (or `exit`); the file routes where each goes. One image per scene. |
| Loading (intro page 3) | `loading` | a `scenes/*.scene.json` + `scripts/Loading.lua` | A native progress panel on a simulated timer; publishes `loading_progress`, fires `done` at full. |
| Main menu | `menu` | `scenes/Main.scene.json` + `scripts/Main.lua` | The launcher: realm buttons page the selector; a scene row launches by id; Settings / Quit; version line + update chip. Buttons come from the client's `SceneEntry` set. |
| Pause | *(shared modal, not a roster scene)* | `scenes/shared/pause.scene.json` + `scripts/shared/pause.lua` | Resume / Settings / Main Menu / Quit. Pushed by any game via [`PauseScene`]. A `Cancel` signal resumes, via the tree's `on_cancel="resume"`. |
| Display confirm | *(shared modal)* | `scenes/shared/confirm.scene.json` + `scripts/shared/confirm.lua` | The keep/revert countdown after a resolution change; auto-reverts on timeout (no cancel affordance by design). |
| Settings | *(shared modal)* | `scenes/shared/settings.scene.json` + `scripts/shared/settings.lua` | Audio / Video / Input tabs; the Input·Keyboard page derives its rows from the rebindable signal catalog. Commits on close through the input seam above. |

Pause, confirm, and settings are **shared modal scene pairs** under
`scenes/shared/` (loaded directly, skipped by the roster index), not Rust-built
trees — a human overrides their defaults by authoring their own copies. Their
`.lua` partners are the worked examples. The `pause`/`confirm` pop-ups render
through the private `MenuView` walker; their chrome styles ride `Main.scene.json`'s
`styles` carrier. To author or vary any of these, see the
[Sensorium authoring guide](../../../content/sensorium/README.md).

## Interactions

- **Signals it captures** — every shell screen declares `input_context() = Menu`,
  so the central pump resolves the player's device into the Menu-context signals
  and the screens read only those (never keys — rule DFE3E44E): **`Confirm`**
  (activate the focused button / click through a splash), **`Cancel`** (back out —
  the pause tree's `on_cancel="resume"`, the settings root's `settings_close`),
  **`NavUp/Down/Left/Right`** (focus traversal), **`TabPrev/TabNext`** (page/tab),
  **`Menu`** (skip a splash). A **signal is the intent** (rule 37722F91): a screen
  captures the signals it cares about via the tree's declarative `on_<signal>`
  props and the nav walker — there is no separate intent router. The pointer is a
  signal source too: a click is a `Confirm` at whatever it hits.
- **Results / intents it fires** — the menu fires each `SceneEntry.id` (launch),
  `settings`, `quit`, `open_update_page`, and `mode_<realm>` (paging). Pause fires
  `resume`, `settings`, `main_menu` (a `ReplaceRoot` back to a fresh menu),
  `quit`. Confirm fires `keep` / `revert`. Settings fires `settings_close` and its
  dialog results (`confirm_save`/`confirm_discard`/`confirm_cancel`/`restore_ok`).
  Splash/loading fire `next` / `exit`, which the **scene file** routes (this crate
  never learns the targets).
- **Model keys** — the shell publishes and its trees bind: `app_version` +
  `update_available` (menu), `subtitle` (confirm countdown), `loading_progress`
  (loading bar), `has_scenes`/`panel_head`/`menu_footer` (launcher), and the
  one-frame `sig_mode_<realm>` mirror that `Main.lua` latches into
  `shown_realm_<n>` page visibility. [`publish_signal_bindings`] is the shell's
  *tool* for a client scene to publish `bind_<Name>`/`glyph_<Name>`/`input_device`,
  which the `nav_footer` component (`flicker-widgets`) binds.
- **What it hands other crates** — the winit run loop and the `SceneManager` (via
  `flicker::app::run_with_input`), the resolved `Theme` handles, and the
  settings→game input seam (the four getters above).
- **Threads** — one background one-shot GitHub-releases check when `app_version`
  is set (the menu polls its result per frame; any failure is silent). Nothing
  else is async.

## Gates

`cargo test -p flicker-shell` — **43 tests, all green** (2026-08-24). The
contract-holding ones:

- **`the_shipped_screens_name_only_kinds_the_engine_knows`** — every kind named in
  Main/pause/confirm/settings trees is a real component (no phantom kinds).
- **`pause_and_confirm_build_from_scene_json`** — pause/confirm parse from their
  shared JSON, pause declares `on_cancel=resume`, no unknown kinds or raw literals.
- **`pause_and_confirm_example_scripts_light_every_gated_button`** — every
  `visible_bind` button in a shared modal is lit by its script's default
  `arrange()`; a gated button the script forgets would vanish → build failure.
- **`shared_modals_render_and_draw_their_buttons`** — the real tree+script path
  draws every authored pause/confirm button and the live confirm countdown.
- **`modal_buttons_variants_are_retired_from_the_carrier`** — the carrier keeps
  the modal *chrome* styles but not the retired button-variant block.
- **`the_default_scene_chain_resolves_by_id` / `a_registered_bench_id_resolves_through_the_fallthrough` / `every_shipped_scene_file_loads_and_its_exits_resolve`** — the manifest→behaviour→factory roster resolves boot, benches, and every file's exits.
- **`menu_arrange_latches_the_realm_page_and_lights_one_slice` / `the_main_menu_composes_from_the_rust_components` / `scene_load_buttons_are_pad_navigable_on_the_live_menu`** — the menu pages by signal, composes from real kinds, and its rows are pad-reachable.
- **`derived_keyboard_page_matches_the_rebindable_set` / `the_retired_keyboard_schema_is_gone`** — the settings keyboard rows derive from the signal catalog, not a hand-authored schema.
- **`a_resolution_pick_reports_a_number_that_changes_the_resolution` / `the_resolution_options_are_device_enumerated`** — a resolution choice is a number that drives the window; options come from the real monitor.
- **`esc_in_settings_fires_settings_close_through_the_bus` / `settings_dialogs_intercept_the_close_intent`** — the close request arrives as the bus `Cancel` intent (`settings_close`), and an open dialog intercepts it.
- **`rebind_survives_gamesettings_round_trip` / `old_settings_without_profile_still_load`** — a World rebind persists across relaunch; a pre-profile `settings.json` still loads.
- **`glyph_names_are_positional` / `keycap_text_rides_the_stringtable_not_display` / `publish_sets_the_glyph_face_and_device` / `publish_reflects_which_family_is_bound`** — `publish_signal_bindings` emits vendor-neutral positional glyph names, resolves keycaps through the stringtable, and stamps the device.
- **`every_loading_token_resolves` / `cursor_decodes_and_tints`** (theme) and
  **`enumerate_*` / `resolution_index_is_nearest_over_the_list`** (display) — the
  loading palette is complete and the resolution ladder dedupes/sorts/nearest-matches.

## Sharp edges

- **`PauseScene::new` ignores two of its four arguments.** `controls:
  &AbstractControls` and `gamepad_config: &GamepadConfig` are accepted and
  discarded — only `theme` and `input_map` are used. A client need not compute
  real values for them; the signature implies the pause menu honours look/pad
  config, and it does not.
- **Controller-tuning settings are persisted but not yet wired.** The stick
  sensitivity / deadzone / trigger / stick-invert / deadzone-shape / sprint /
  raw-input fields serialize into `settings.json`, but nothing reads them back and
  no `GamepadConfig` is ever built from them — every one in the shell is the
  default. Only mouse sensitivity + pitch-invert round-trip to the running game
  today. Don't rely on the controller-tuning knobs affecting input.
- **Process-wide state.** `GameSettings`, the display setting, the scene registry,
  and the update check are process globals (`thread_local` / `LazyLock`) — fine
  for one window, and the whole shell runs on the winit thread (`SceneFactory` is
  `Rc`, not `Send`).
- **A broken scene folder is a startup panic, not a build error.** The roster is
  read at runtime so scenes are authored without a recompile; the cost is that a
  tree that will not index fails loudly in the terminal at launch. The embedded
  screens (theme/scripts/shared modals) are the opposite — a missing one is a
  build failure.
- **The window is truth, `settings.json` mirrors it.** Display changes apply
  straight to the window; `current()` mirrors the last-applied so the confirm
  overlay can revert. Window size/position persist on exit (fullscreen exits keep
  the last windowed placement).
- **Only prism-alpha phones home.** The update check runs only when `app_version`
  is non-empty; it is NOTIFY-only, guarded to the `elide-us/flicker` releases
  page, and silent on any failure.
