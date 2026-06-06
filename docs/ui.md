# UI architecture

How the front-end (logo, menu, loading, pause, settings) is built today, and
where it's headed. The UI currently lives in the `voxel-cluster` example
(`examples/voxel-cluster/src/`); the reusable mechanics are in engine crates
(`flicker-scene`, `flicker-render`). Re-verify line/symbol references — they
drift.

## Layers

1. **Scene manager — `flicker-scene`** (engine). A stack of `Scene`s driven as a
   `flicker_app::App`. Per frame it updates only the **top** scene and renders
   the **visible slice** (topmost opaque scene + any overlays above it,
   bottom-up). Scenes reshape the stack with a `Transition` (`Replace`, `Push`,
   `Pop`, `Quit`, `None`). Structural changes apply in `render` because
   `enter`/`exit` need `&mut Renderer`. See the crate docs for the full model.

2. **Render primitives — `flicker-render::Renderer`** (engine). 2D draw calls
   (`draw_sprite`, `draw_text`, `draw_triangle`), `measure_text` (real glyph
   metrics, see below), texture upload, and window control (`set_windowed`,
   `set_borderless_fullscreen`, `set_exclusive_fullscreen`, `monitor_size`,
   `is_fullscreen`) — winit stays behind the renderer.

3. **Gothic UI toolkit — `ui.rs`** (example, promotable). Procedurally generates
   its raster art (no binary assets, deterministic, tunable): a `Canvas` pixel
   buffer + helpers (stone fill, bevel, cracks, gold filigree scrollwork) bake
   the `Theme` textures (panel, button) once at `Theme::build`. Plus the
   immediate-style widgets: `modal_layout` + `Theme::draw_panel` (title +
   subtitle + two buttons), `draw_loading` (panel + progress bar), `wordmark`
   (logo), and the compact `Dropdown` (drawn from primitives, dynamic height).
   Palette and sizing are constants at the top of the file.

4. **Concrete scenes + settings — `main.rs`** (example, game-specific). The
   front-end scenes and the settings panel / display model live here.

## Front-end flow

```
Logo ──Replace──▶ Menu ──Replace──▶ Game ──Push──▶ Pause
(splash,         (START / QUIT,     (Booting→Active   (overlay; Resume/Esc
 ~2.2s or         settings panel)    loading gate)     Pop, Quit)
 click)                                                  │
                                                         └─ settings panel
Menu / Pause ──Push──▶ ConfirmDisplay (overlay, 15s confirm-or-revert)
```

- **`LogoScene`** — large wordmark over the backdrop; auto-advances to the menu
  after `LOGO_DURATION` or on click / Space / Escape.
- **`MenuScene`** — gothic panel (`FLICKER`, START / QUIT) + the settings panel.
- **`GameScene`** — the voxel demo as a `Scene`. Boots with physics off and the
  3D clipmap **not drawn** (loading widget only) until the spawn field is meshed
  *and* every nav-range cluster has a nav surface, then flips to `Active`.
  Escape pushes the pause overlay. No settings panel while active.
- **`PauseScene`** — overlay over the frozen game; reuses the game's `Theme`.
  Resume / Escape pop; Quit exits. Shows the settings panel (so resolution is
  changeable in-game **only** while paused).
- **`ConfirmDisplayScene`** — overlay pushed after a confirmable display change
  (see below).

## Font measurement

`Renderer::measure_text(text, size) -> Vec2` shapes a throwaway glyphon buffer
and returns the real `(max line width, total height)`. `ui::centered_text` uses
it for exact horizontal centring (it replaced an advance-ratio estimate). Note:
it takes `&mut Renderer` (glyphon shaping needs `&mut FontSystem`), so it's
called from `render` / `enter`; widget geometry that's needed in `update`
(e.g. dropdown box width) is **measured once and cached** at `enter`.

## Display settings (`display.rs`)

- **Modes** (`DisplayMode`): `Windowed`, `BorderlessFullscreen` ("Fullscreen
  Window"), `ExclusiveFullscreen` ("Fullscreen").
- **Resolution ladder** (`resolution_options`): the debug default **960×540**,
  then the in-ratio rungs **720p / 1080p / 1440p** strictly below native, then
  **Native** — deduped and kept short. The app always starts at the windowed
  960×540 debug default.
- **Apply**: `DisplaySetting::apply` calls the renderer's window methods. The
  **window is the source of truth**; a process-wide `CURRENT` (a `Mutex`)
  mirrors the last-applied setting so panels show the selection and the confirm
  overlay can revert — the seed of "feature management beyond game state".
- **Confirm-or-revert**: a **resolution** change (or switching to exclusive
  fullscreen) applies instantly, then pushes `ConfirmDisplayScene` — a 15-second
  countdown with **Keep** / **Revert**; on Revert or timeout it restores the
  previous setting. Windowed/borderless toggles apply outright (no confirm). The
  overlay works identically over the menu or the pause screen because it's just
  a pushed scene.

The settings panel (`SettingsPanel`) is two stacked dropdowns (Mode,
Resolution) anchored top-right with an inset (`SETTINGS_INSET`) — top-left is
reserved for gameplay bars. Drawn at ~half the menu's scale.

## The Lua UI layer + boundary contract

Almost all UI now lives in Lua (`flicker-script` + `scripts/*.lua`): Lua owns
layout / labels / interaction; the engine owns rendering and data. Every screen
reads its layout from `ui_elements.json` (the `UI` global). Ported:

- **`scripts/modal.lua`** — one shared gothic-modal **component** renders the
  **menu**, **pause**, and **confirm** screens, differing only by their
  `UI.screens.*` instance (overlay / title / button items) + the `Model.screen`
  the scene selects (and `Model.subtitle` for the confirm countdown). The Rust
  scenes (`MenuScene` / `PauseScene` / `ConfirmDisplayScene`) are thin shells
  over a shared `ModalUi` helper — they keep only their transitions.
- **`scripts/hud.lua`** — the in-game debug **stat readouts** (styled from
  `UI.hud.stats`, content formatted from `Model`) + the feature **checkboxes**
  (`UI.hud.checkboxes`).
- **`scripts/logo.lua`** — the intro splash: a **sequence of full-screen logos**
  (business → engine, from `assets/*.png`) that each fade in / hold / fade out
  before the menu. The script owns the timeline + fade from `UI.logo`
  (`fade`/`hold`/`fit`/`images`) + `Model.elapsed`, and reports `done`; the
  `LogoScene` decodes the PNGs (the `image` crate), exposes them as textures,
  and advances on `done` (or click / Space / Escape). The hold-time is future
  room to stream the menu's background scene.

This retired the Rust modal/menu/logo drawing from `ui.rs` (`draw_panel`,
`draw_button`, `scrim`, `dim`, `wordmark`, `ModalButton`, the modal hit-test);
`ui.rs` now mainly **bakes the gothic textures** the Lua screens draw with
(`Theme::lua_textures` hands them over by name).

### Interactive widgets (`scripts/widgets.lua`)

A reusable immediate-mode widget toolkit — **slider**, **stepper** (numeric
value box), **dropdown** — loaded into every screen's VM as the `Widgets` global
(`ScriptHost::set_lua_module`). Each widget splits into `*_update` (hit-test +
interaction, from the script's `update`) and `*_draw` (emit commands). Widget
*values* are not stored in Lua — they live in the engine `Model` (two-way): the
update returns the new value, the host applies it, and next frame the `Model`
carries it back for the draw. Only transient interaction (a slider's drag flag,
a dropdown's open flag) is kept script-side, keyed by widget id. Styles come
from JSON. The slider needs the **held** mouse state — `update` now also gets
`down` (appended after the existing args, so older scripts are unaffected).

These are demonstrated in the in-game HUD (`UI.hud.controls`): a **move-speed
slider**, a **sensitivity stepper**, and a **locomotion dropdown**, each wired
to a real config value through the value channel. They are the building blocks
for the upcoming lighting controls (time-of-day slider, moon/season alignment).

**Still Rust:** the **loading** widget (data in `UI.loading`, render port pending
— entangled in the game-boot gate) and the **settings dropdowns**
(`SettingsPanel`) — the latter can now port to Lua on the `Widgets.dropdown`.
Keyboard text entry for the stepper/value box is a planned enhancement.

**The boundary is strict and is the project's only Lua↔Rust seam.** `mlua` is
confined to `flicker-script`; no other crate depends on it. The contract is a
small set of plain-data types in that crate — nothing else crosses, never a
renderer handle or GPU resource. Four channels, all named-value / plain-data:

- **Input** (engine → script): the interaction snapshot (mouse, click edge,
  screen size), passed to `update`/`draw`.
- **Data model** (engine → script): a `ValueMap` of named engine values (fps,
  positions, counts, a setting's current value) published each frame via
  `ScriptHost::set_model` and read by the script as the `Model` global. This is
  how `hud.lua` renders live stats and how a slider will show its current value.
  Sibling: the static `Textures` global (`set_texture_ids`) — name → engine
  texture id, for `sprite` draw commands.
- **Layout / config** (engine → script): a JSON tree exposed as a global via
  `ScriptHost::set_global_json` (objects → tables, arrays → 1-indexed tables).
  `ui_elements.json` is the first use: a **named element tree** (`UI.menu.panel.w`,
  `UI.menu.title.color`, `UI.menu.items[]`) the host parses and hands to the
  scripts, so a screen reads its layout from data instead of hardcoded constants.
  `menu.lua` is fully driven this way — edit the JSON to move / resize / restyle
  the menu (relaunch to apply; calling `set_global_json` again hot-reloads). It's
  the "describe the UI as data" layer — not HTML/CSS, but the same idea, kept
  deliberately thin.
- **Results + draw** (script → engine): `update` returns a `ValueMap` of named
  results (toggles, momentary actions, widget values); `draw` returns
  `HudCommand`s (`rect`/`sprite`/`text`, each with a painter's-order `layer` and
  optional centre alignment) that the consumer's shared `render_hud` turns into
  draw calls.

`Value` (bool / number / text) is the only scalar currency; the JSON channel adds
nested tables/arrays of those. The contract is **validated at build time** by
`flicker-script`'s `model_round_trip` + `set_global_json_marshals_nested_tree`
tests and the example's `script_smoke` tests (which load the real scripts *and*
`ui_elements.json` and run a frame — so a malformed layout or a name the script
reads but the data lacks fails the build). That in-Rust validation is why **no
external binding-generation / codegen step is needed** while the boundary stays
Rust-internal; if Lua scripts ever version or ship independently of the engine,
revisit with a generated contract + CI check (parked).

The `ui.rs` widgets remain example-local; the generic machinery (canvas,
panel/button/dropdown, layout, hit-testing) can still promote to a `flicker-ui`
crate once the widget set settles, leaving the game-specific gothic theme behind.
