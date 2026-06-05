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

## Planned: move the front-end UI into the Lua layer

Today the menus/pause/loading/settings are **hardcoded in Rust**. The Lua layer
(`flicker-script`, `scripts/hud.lua`) currently only drives the in-game debug
checkboxes — it returns `Rect`/`Text` draw-commands and reads `Toggles`. The
next step (tracked separately) is to let Lua define the front-end UI
data-drivenly through a generic Rust 2D interface: Lua owns layout / labels /
interaction; Rust provides the gothic textures, `measure_text`, and the draw
primitives. The `ui.rs` widgets are split so the generic machinery (canvas,
panel/button/dropdown, layout, hit-testing) can promote to a `flicker-ui` crate,
leaving the game-specific theme behind.
