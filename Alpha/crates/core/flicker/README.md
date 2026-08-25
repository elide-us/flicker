# flicker

The **umbrella crate**: one dependency that re-exports each engine sub-crate as a module, so
a consumer writes `flicker = { … }` once and reaches the whole engine as `flicker::render`,
`flicker::scene`, `flicker::script`, `flicker::ui`, … instead of listing a dozen crates. It
adds **no code of its own** — `src/lib.rs` is nothing but `pub use` aliases. This is the front
door the **scene / game-package crates** use (Populous, Quartermaster, Clicktrainer, …); the
application (`prism-alpha`) and a few engine crates instead depend on member crates directly
(both styles are supported — see *Where it sits*).

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits
- **Builds on:** re-exports (and therefore depends on) all eleven engine crates in the table
  below. It sits at the top of the crate graph purely as a facade — nothing depends *inward*
  on it except consumers.
- **Used by (via the umbrella):** the scene crates — `flicker-clicktrainer`,
  `flicker-quartermaster`, `flicker-populous`, `flicker-godmode`, `flicker-solarbirth`,
  `flicker-loomforge`, `flicker-sablework`, `flicker-componentcatalog`,
  `flicker-controllertester`, `flicker-pocclusters`, `flicker-pocepochs`,
  `flicker-assetpipeline` — plus the frontend crates `flicker-shell` and `flicker-globe`.
  Each declares `flicker = { version = "0.1.0", path = "../../core/flicker" }` and imports as
  `use flicker::render::…`.
- **Not used by:** `prism-alpha` (it hosts the scene crates and pulls a few engine crates
  directly as `flicker-shell.workspace = true` / `flicker-widgets.workspace = true`). So the
  umbrella is the scenes' front door, **not** a universal one.
- **Reads from the content tree:** nothing. A pure re-export crate; content paths belong to
  the member crates it re-exports.

## Public API — the module map

The entire surface. Each row is one `pub use <crate> as <module>;` in `src/lib.rs`; the type
you want lives in the linked crate's README, reached here as `flicker::<module>::<Item>`.

| `flicker::` module | Re-exported crate | Cluster | What it gives you | README |
|---|---|---|---|---|
| `core` | `flicker-core` | core | Shared low-level utilities: content roots/mount, gzip, settings, math helpers. Foundation; no rendering. | [../flicker-core/README.md](../flicker-core/README.md) |
| `app` | `flicker-app` | platform | The winit/wgpu application loop + window — the `App` a scene stack runs on. | [../../platform/flicker-app/README.md](../../platform/flicker-app/README.md) |
| `render` | `flicker-render` | render | The `Renderer` (2D draw calls, real glyph `measure_text`, textures, window control), the `FrameGraph`, and the stage compiler. | [../../render/flicker-render/README.md](../../render/flicker-render/README.md) |
| `two_d` | `flicker-2d` | render | The ClayEngine 2D sprite family (`SpriteStrip`, `draw_sprite_ex`, rotation). **Name note:** `2d` can't start a Rust identifier, so the module is `two_d`. | [../../render/flicker-2d/README.md](../../render/flicker-2d/README.md) |
| `input_core` | `flicker-input-core` | input | The **signal** catalog (`ActionSignal` — abstract intents like Confirm/Cancel, never keys) + binding/context model + per-frame snapshot. **Reached directly, not via the umbrella — see Sharp edges.** | [../../input/flicker-input-core/README.md](../../input/flicker-input-core/README.md) |
| `input_router` | `flicker-input-router` | input | The one input event bus + focus + consumer API. **Reached directly, not via the umbrella.** | [../../input/flicker-input-router/README.md](../../input/flicker-input-router/README.md) |
| `input_device` | `flicker-input-device` | input | Platform input sources + the 120 Hz analog sampler; fills the core snapshot. `flicker::input_device::last_input_context()` is the one input item consumers reach through the umbrella today. | [../../input/flicker-input-device/README.md](../../input/flicker-input-device/README.md) |
| `net` | `flicker-net` | net | Networking (tokio websockets / HTTP) for the persistent world. | [../../net/flicker-net/README.md](../../net/flicker-net/README.md) |
| `script` | `flicker-script` | scripting | The Lua host (`ScriptHost`) + the four-channel Lua↔Rust boundary: `Value`/`ValueMap` (the **Model** — the per-frame key→value table the engine hands to Lua), `HudCommand` (plain-data draw instruction), `UiNode`. mlua is confined to this crate. | [../../scripting/flicker-script/README.md](../../scripting/flicker-script/README.md) |
| `scene` | `flicker-scene` | frontend | The `Scene` stack + `Transition` (Replace/Push/Pop/Quit) driven as an app, and `SceneInput`. A **scene** = one screen/state on the stack. | [../../frontend/flicker-scene/README.md](../../frontend/flicker-scene/README.md) |
| `ui` | `flicker-widgets` | frontend | The reusable UI render surface: `run_ui`, `render_hud`, `SceneDef`, `UiState`, `UiIntents`, `WalkerHandler` (the **walker** = the focus/navigation cursor over a scene tree), plus `strings` and shared-style helpers. **Name note:** the module is `ui`, the crate is `flicker-widgets` (historically `flicker-ui`); grep source under `flicker-widgets`. | [../../frontend/flicker-widgets/README.md](../../frontend/flicker-widgets/README.md) |

Naming rule for the nine unremarkable rows: strip the `flicker-` prefix and turn `-` into `_`
(`flicker-input-core` → `input_core`). Only `two_d` and `ui` break that rule (both noted above).

### Depending on it
```toml
# in a scene crate's Cargo.toml (the current convention):
flicker = { version = "0.1.0", path = "../../core/flicker" }
```
```rust
// then reach any engine layer through the one crate — real example,
// Alpha/crates/scenes/flicker-clicktrainer/src/lib.rs:26
use flicker::render::{FrameGraph, Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
```
Note the raw `path = …` (not `flicker.workspace = true`) and the hardcoded `version` — see
Sharp edges for why, and for the separate input dependency a scene also needs.

## Interactions
**None of its own.** The umbrella captures no signals, publishes/binds no Model keys, fires no
results, spawns no threads, and hands other crates nothing beyond the re-exported types. A type
reached as `flicker::render::Renderer` **is** `flicker_render::Renderer` — same type, same
crate. All real interactions are documented in the member crate READMEs linked above.

## Gates
No unit tests — there is nothing to assert about `pub use X as Y` beyond "it compiles", which
`cargo build -p flicker` already enforces: if a member crate is renamed, dropped, or its path
breaks, the umbrella fails to build. That compile **is** the drift gate. `cargo test -p flicker`
is green (0 tests). Every crate this file links has its own gates in its own README.

## Sharp edges
- **Input is reached directly, not through the umbrella — the "one front door" is really
  "engine minus input".** Every scene that depends on `flicker` *also* declares
  `flicker-input-core.workspace = true` + `flicker-input-router.workspace = true` and imports
  input as `flicker_input_core::…` / `flicker_input_router::…`. The umbrella's `flicker::input_*`
  aliases exist but are used in only one place across the tree —
  `flicker::input_device::last_input_context()`. So a new scene needs **two** dependency lines
  (the umbrella *and* the input crates), and should follow the shipped convention: input via the
  direct crates, everything else via `flicker::`. This is a reported gap (Finding 1), not a rule
  you can derive from the module map — copy an existing scene crate's `Cargo.toml`.
- **`flicker.workspace = true` does not work.** The umbrella is not registered in the root
  `[workspace.dependencies]` (every other internal crate is), so you must spell out
  `path = "../../core/flicker"` and a literal `version`. Reported as Finding 2.
- **Don't mix import styles for one type in one crate.** If a crate depends on both `flicker`
  and, say, `flicker-render` directly, `flicker::render::Renderer` and `flicker_render::Renderer`
  are the same type reached two ways — pick one path per crate to keep imports readable.
- **The module name is not always the crate name.** `flicker::ui` → `flicker-widgets`,
  `flicker::two_d` → `flicker-2d`. When a compiler error or a doc points you at a
  `flicker::<mod>` item, open the crate in the module map, not a folder named after the module.
