# flicker-shell — the front-end shell service (handoff)

**Status:** landed (2026-07-07). First of the "engine = services, harden each API"
deep passes (§ the architecture direction, below). `crates/flicker-shell`.

## What it is

`flicker-shell` is the reusable **game front-end shell** service: intro splash →
main menu → settings → pause overlay, plus the gothic UI theme and display/
settings persistence. Every flicker client needs this identical front-end, so it
lives here as a service instead of being copied into each client. It sits **on
top** of the engine (render/scene/script/ui/app) — same layer as a client — and
depends on the `flicker` umbrella.

Extracted from the `voxel-cluster` POC that was copied into `Alpha/flicker-csg`
(the user's "the skeleton got pulled into here instead of remaining in crates").

## Public API (the whole contract)

```rust
// A client's entire main():
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()…init();
    if handle_bake_args()? { return Ok(()); }        // game-specific, optional
    flicker_shell::run(flicker_shell::ShellConfig {
        game_scene: Box::new(|| Box::new(GameScene::new())),
    })
}
```

- `run(ShellConfig) -> anyhow::Result<()>` — restores display settings, runs
  splash → menu → *your scene* → pause/settings, owns the winit loop. Blocks.
- `ShellConfig { game_scene: GameSceneFactory }` — the one thing the shell can't
  know: `GameSceneFactory = Box<dyn Fn() -> Box<dyn Scene>>`. START calls it.
- `Theme` — the client builds one (`Theme::build`) to draw its loading widget
  while its world cooks and to hand to the `PauseScene` it pushes.
- `PauseScene::new(theme, &bindings, &controls, &gamepad_config)` — the client
  pushes this (a `flicker_scene::Transition::Push`) when the player opens pause.
- `take_pending_input() -> Option<(InputMap, AbstractControls, GamepadConfig)>` —
  the client polls this to apply input changes made in pause→settings.

The seam is closed by references, not stitched: `LogoScene` carries the game
factory forward into `MenuScene`, which calls it on START — the shell never names
the game type.

## What moved vs. stayed

- **Moved → flicker-shell:** the gothic theme (`theme.rs`, was `ui.rs`), display
  settings (`display.rs`), the settings model + statics, all front-end scenes
  (`shell.rs`: Logo/Menu/Pause/UnifiedSettings/ConfirmDisplay/ModalUi + helpers),
  and the shell resources — `scripts/{logo,modal,settings}.lua` + the shell
  sections of `ui_elements.json` (`modal`/`screens`/`settings`/`logo`/`loading`)
  + the two publisher/engine logo PNGs.
- **Stayed in the client (`Alpha/flicker-csg`):** `GameScene` + all voxel logic
  (LOD/nav/picking/inspector), `world_lighting`, `hud.lua` + the `hud` JSON
  section (its own `ScriptHost`, own `UI` global — no merge with the shell's),
  and `main`.

## Resources are embedded

Shell scripts + `ui_elements.json` are `include_str!`-embedded and the logos are
`include_bytes!`-embedded, so a new client inherits the whole front-end with
**zero copied files** — that was the point (the POC re-loaded them per client via
`CARGO_MANIFEST_DIR`). Loading embedded layouts needed a new
`flicker_ui::load_ui_json_str(script, &str)` (the sibling of the path-based
`load_ui_json`) — a small strengthening of the flicker-ui service's API.

## Naming note

`crates/flicker-shell` (this) vs `Alpha/flicker-skeletal` (the CPU skeletal-
**animation** runtime) are different services one letter apart. The app-shell is
`flicker-shell` precisely so it never collides with `flicker-skeletal`.

## Not done / next (descriptive, not committed)

- `ShellConfig` is minimal (just `game_scene`). Branding — menu **title** (still
  the shell-default "FLICKER" from the embedded JSON), a per-game **logo** to
  prepend to the splash — are the obvious next fields; no call sites change.
- The **`take_pending_input` push side is unwired** (a designed seam): the
  pause→settings scene doesn't yet write `INPUT_SETTINGS`, so the getter returns
  `None` today. Same behaviour as the POC.
- The settings statics (`GAME_SETTINGS`/`INPUT_SETTINGS`) are process globals —
  fine for one window, a smell to revisit if a cleaner API is wanted.
- `Alpha/flicker-skeletal` (animation) is an engine service currently parked in
  `Alpha/`; the user flagged it belongs in `crates/` eventually ("that's ok" =
  deferred).

## The architecture direction this sits inside

The engine is being reframed as **a set of services**, each crate a coherent
service with a hardened public API. Deep passes, one service at a time. The
renderer's multi-target work (`RenderTargetHandle` / `create_render_target` /
`render_to_texture`) is the exemplar; this shell extraction is the first
follow-on. Known next candidates (user's call which/when): a **Camera** service
(1st/3rd-person, spring-arm — today the controller is reimplemented per client),
a **World runtime** service (owns the cluster field + LOD + nav that the client
`GameScene` orchestrates today), and resolving the **`flicker-scene` name
collision** (screen-stack vs. a future spatial scene/object graph).
