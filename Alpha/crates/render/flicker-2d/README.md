# flicker-2d

The home reserved for the engine's standalone **2D sprite/game layer** — the ported
ClayEngine `Sprite` family that will let legacy 2D games run on flicker. **It is an
intentional stub today:** `src/lib.rs` is a single module doc-comment and the crate
compiles to an empty library — there is no public API yet. If you are looking for 2D
drawing you can call *right now*, it is not in this crate; the [signpost](#where-the-2d-pieces-live-today)
below says which crate owns each piece.

> Design of record — why it is shaped this way, its scope, the ClayEngine class mapping,
> and the porting plan — lives in the project's MCP memory (spec *"ClayEngine Sprite-class
> port → flicker-2d"*, `E535BAC4`), not here. This file documents how to use the crate.

## Status

- **No public API.** `src/lib.rs` declares the crate and nothing else. `cargo test -p
  flicker-2d` runs 0 tests.
- **Nothing to import.** Do not `use flicker_2d::…` anything — there are no items. The
  umbrella re-export `flicker::two_d` resolves to this empty crate.
- Its intended contents (the hand-ported 2D sprite/game classes, incl. the animated
  `SpriteStrip`) are **not built yet**. Reach for the design of record above for scope;
  this README is not a roadmap.

## Where the 2D pieces live today

The crate name suggests all of 2D sprites lives here. It does not — today the capability
is split across two other crates, with this one still empty. Reach accordingly:

| Concern | Lives in | Reach for | Notes |
|---|---|---|---|
| Raster draw (the sprite-batch primitive) | `flicker-render` | `Renderer::draw_sprite` | CPU painter's order by `layer`; quads batch by texture. |
| Atlas / sub-rectangle draw | `flicker-render` | `Renderer::draw_sprite_uv` (`uv:[u0,v0,u1,v1]`, `FULL_TEXTURE`) | The `uv` sub-rect **is** the atlas draw. |
| Rotation (centre or arbitrary pivot) | `flicker-render` | `Renderer::draw_sprite_ex` (`rotation`, `pivot`) | Shipped and tested engine-side. |
| The image **widget** (`sprite` component) | `flicker-widgets` | component kind `"sprite"` | Presentation-only: fixed full-texture UV, backdrop/fit/fade. No atlas, no rotation, no animation. |
| Animated sheet (`SpriteStrip`) + the sprite/game classes | **`flicker-2d`** | — | The frame-stepping that would drive `draw_sprite_uv` over time. **Absent today.** |

For the raster API itself, see [`../flicker-render/README.md`](../flicker-render/README.md).
Do not re-derive raster concepts from this file — the human usage guide is
[`Alpha/content/sensorium/RASTER_AND_SPRITES.md`](../../../content/sensorium/RASTER_AND_SPRITES.md).

## Where it sits

- **Builds on (declared, wired ahead of use):** `flicker-render`, `flicker-core`, `glam`,
  `tracing`. The crate calls none of them yet.
- **Used by (also wired ahead):** `flicker` (umbrella) re-exports it as `flicker::two_d`;
  `flicker-app` declares the dependency. Neither references an item from this crate — there
  are none to reference. The wiring exists so callers get a stable path once the classes land.
- **Reads from the content tree:** nothing today.

## Interactions

None — the crate has no code. It captures no signals, publishes/binds no Model keys, and
hands nothing to other crates.

## Gates

None — no code, no tests. `cargo test -p flicker-2d` = 0 passed.

## Sharp edges

- **The doc-comment oversells the crate.** `src/lib.rs` reads *"Sprite, Tilemap, and
  Camera2D primitives"* — none exist here, and only the `Sprite` family is in the design of
  record; `Tilemap` and `Camera2D` are named nowhere in MCP. Treat the crate name as a
  reservation, not an inventory, and use the signpost table above.
- **`flicker::two_d` is an empty namespace.** The re-export compiles and resolves, but
  exposes nothing today.
