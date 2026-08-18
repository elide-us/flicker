# The raster engine, colour, and sprites

**Companion to [`README.md`](README.md).** That guide is for authoring Prism
*scenes* (JSON trees + Lua pairs). This one is for the layer beneath: how the 2D
raster engine draws, how colour resolves through the one palette, how sprites
are printed, and how the ClayEngine **Sprite** class will be re-homed so the old
DirectX 2D games run on flicker unchanged.

> **Scope.** Usage guide. The design of record — the sprite-strip port plan, the
> rotation gap and its resolution, the crate/asset placement — lives in the MCP
> memory bank, never here. This file tells you how to *use* the surface; MCP
> tells you *why it is shaped that way*.

---

## The three layers

Everything a sprite does on screen bottoms out in the raster primitive. Keep the
layers distinct — it is what keeps each one honest.

| Layer | Surface | Home | Audience |
|---|---|---|---|
| **2D game** | the ported `Sprite` / `SpriteStrip` / `SpriteString` classes | `flicker-2d` crate *(stub today)* | Rust game code |
| **Widget** | the `sprite` component + `HudCommand::Sprite` | `flicker-widgets` | scene authors (JSON + Lua) |
| **Raster** | `draw_sprite_uv` + the sprite pipeline | `flicker-render` | the engine |

A faithful sprite-sheet animation needs *nothing* from the widget layer — it
drives the raster primitive directly. That is why the routine is simple: pure
CPU frame-advance on top of `draw_sprite_uv`, zero engine changes.

---

## The raster engine

A **DirectXTK-style SpriteBatch**: CPU painter's order, no depth buffer for 2D.

The 2D overlay is drawn by four pipelines — **ui-panel**, **triangle**,
**sprite**, **text** — that share one sort key, an ambient `layer: f32`. Each
stably sorts its submissions into ascending layer bands; the encoder renders,
*per layer*, in that fixed pipeline order. So within one layer a panel sits under
a sprite sits under text, and across layers the whole band-set repeats ascending.
The depth attachment is shared with the 3D pass but never written or tested by
2D (DirectXTK's `DepthNone` default).

- **Painter's order is the whole model.** A scene's stack position sets its base
  layer, so overlays sort above the scene beneath with no per-widget bookkeeping.
  Offset from `renderer.layer()` to stack sub-elements (a dropdown over its panel).
- **Batching.** Adjacent quads sharing **layer + texture + clip** coalesce into
  one draw call. Any of the three changing breaks the run — which is why an
  **atlas** pays off (many images, one texture handle, one run).
- **Tint is decoded sRGB → linear.** Theme tokens are sRGB; the sampler already
  linear-decodes an sRGB texture on read, so the shader decodes only the *tint*
  before multiplying. White tint `(1,1,1)` is a fixed point — an untinted textured
  sprite is unchanged.
- **Sampler is nearest** on mag/min/mip by default (pixel-exact art); blend is
  straight alpha.

---

## Colour — the one palette

Colour never appears as a number in authored content. Every colour is a `$token`
into the single palette at `resources/ui_theme.json → theme.tokens`, a map of
name → `[r, g, b, a]` floats. **104 tokens**, a committed *dark* theme: cold
carved stone lit by sapphire rune-light, aged bronze the only structural metal, a
seven-colour Septisigil for stats.

| Family | Tokens | Role |
|---|---|---|
| Surfaces | `stone0`…`stone5`, `edge0`…`edge4` | panels dark→light; borders |
| Text | `ink`, `ink_bright`, `dim`, `faint` | parchment ink on stone |
| Metal | `bronze`, `bronze_deep`, `bronze_dim` | the only structural metal |
| Accent | `sap_base`/`sap_border`/`sap_hover`/`sap_press`, `sapphire`, `rune_glow` | interactive + lit focus |
| Resources | `hp_*`, `mana_*`, `stam_*` | the three resource ramps |
| Septisigil | `sig_white/yellow/red/orange/black/blue/green` | the seven stat colours |
| Derived | `sheen`, `scrim`, `panel_bg`, `hairline`, `accent_wash`, `danger_*`, `*_glow` | alpha/shade variants |

**The palette cannot fork.** A build gate
(`ui_theme_json_is_the_theme_and_nothing_else`) fails if any node but `theme`
appears in that file, and a `theme` key anywhere else is refused at parse. Add a
token here; reference it as `"$name"` everywhere else. "Style" — weight, radius,
gradient axis, feather — is a *separate* concern from colour and rides component
defaults, `ui_style.json`, or a scene's own `styles` block.

---

## Printing sprites

### The renderer primitive (SpriteBatch)

```rust
// Whole-texture blit. position = top-left px; size = px; color = RGBA tint 0..1.
draw_sprite(texture, position, size, color);

// Atlas draw. uv = [u0,v0,u1,v1] normalized, origin top-left.
// FULL_TEXTURE = [0,0,1,1] is exactly what draw_sprite passes.
draw_sprite_uv(texture, position, size, color, uv);
```

Ambient state, set separately, persists until changed (reset each `begin_frame`):

```rust
set_layer(layer: f32)     // painter's-order key; higher = on top
layer() -> f32            // read it back to offset from
set_clip(rect: [f32; 4])  // scissor: x, y, w, h in px
clear_clip()              // back to full frame
```

> **Sharp edge — `set_clip` is not an `Option` setter.** It takes a bare
> `[f32;4]` rect; you turn clipping *off* with `clear_clip()`, not `set_clip(None)`.

### The `sprite` widget (scene authors)

For UI trees, `sprite` is *the* image component (splash was folded into it — a
splash is a sprite with a fade timeline). A bare `tex` blits an engine texture
into the node's whole rect, tinted white × `alpha`. Optional features make it a
presenter: `backdrop` (a slate behind), `fit` (contain-fit the native size,
letterbox), and `fade_in`/`hold`/`fade_out` (an alpha ramp on the scene clock).

> **The widget is presentation-only by design.** It hardcodes `uv:[0,0,1,1]` —
> *"an atlas sub-rect is the glyph face's business, not this one's."* No atlas
> frame, no rotation, no animation. Sprite-sheet animation lives in the
> `flicker-2d` layer, not here.

---

## Atlasing with UV sub-rects

`uv` is the whole story of atlasing: pack many images into one texture, address
each by its normalized sub-rectangle. One texture handle → one bind → one run.
The shipped example is the 16-glyph controller atlas (`prism_pad_glyphs.png`, a
4×4 grid of 64px cells).

```rust
// One cell of an N×N grid atlas → its UV rect.
fn cell_uv(index: u32, cols: u32, rows: u32) -> [f32; 4] {
    let (cw, ch) = (1.0 / cols as f32, 1.0 / rows as f32);
    let (col, row) = (index % cols, index / cols);
    let (u0, v0) = (col as f32 * cw, row as f32 * ch);
    [u0, v0, u0 + cw, v0 + ch]
}
```

That function is the seed of the animated sheet below — an animation is
`cell_uv` (or a per-frame source rect) called with an index that advances on the
clock.

---

## Animated sprite sheets — `SpriteStrip`

**Port the existing ClayEngine class; do not build a parallel system.**

In ClayEngine the animated sheet is `SpriteStrip`: a texture plus a
`vector<RECT>` of frame source-rects, advanced by an elapsed-time accumulator.
It maps 1:1 onto flicker — each frame RECT becomes a UV rect, the draw is one
`draw_sprite_uv`, and the advance is CPU-side with **no renderer change**.

| ClayEngine member | flicker equivalent |
|---|---|
| `m_frames: vector<RECT>` | precomputed `[f32;4]` UVs |
| `m_time_per_frame` | `1.0 / fps` |
| `m_current_frame` / `m_frame_elapsed_time` | same |
| `m_animation_paused` | same |
| `m_has_static` / `m_draw_static` | a fixed rest frame |
| `Update(dt)` / `Draw()` | `update(dt)` → `draw_sprite_uv` |

The advance is the entire routine: accumulate `dt`, step frames while
`elapsed >= time_per_frame`, wrap the index; on draw, pick the current (or the
static) frame's UV and emit one `draw_sprite_uv`.

Reference source: ClayEngine
[`Sprite.h`](https://github.com/elide-us/ClayEngineOSS/blob/main/ClayEngineLibrary/Sprite.h).

---

## Porting the ClayEngine Sprite class

The hierarchy is small: `DestinationExtension` (position + size) and
`AdvancedSpriteExtension` (origin, scale, rotation, colour, depth, H/V mirror)
combine into `BaseSprite`, specialized by `Sprite` (texture + source rect,
`Contains`, `Draw`), `SpriteStrip` (above), and `SpriteString` (text + drop
shadow). Most of it maps onto the primitive for free:

| Capability | flicker path | Status |
|---|---|---|
| Destination position + size | `draw_sprite_uv` args | free |
| Source RECT (px) | normalize → `uv` | free |
| Colour / alpha tint | `color: [f32;4]` | free |
| Depth | `set_layer` (painter's order) | free |
| Scale | multiply into `size` | free |
| H / V mirror (flip) | swap `u0↔u1` / `v0↔v1` in the uv | free |
| SpriteStrip animation | CPU frame-advance | free |
| SpriteString drop shadow | `draw_text` twice, offset | free |
| `Contains(point)` | point-in-rect on destination | free |
| **Rotation about an origin** | `draw_sprite_ex(…, rotation, pivot)` | shipped |

**Rotation shipped** (2026-08-17). `draw_sprite_ex(tex, pos, size, color, uv,
rotation, pivot)` spins a single quad by `rotation` radians about `pivot` (screen
px). Pass `position + size * 0.5` for the common centre-of-sprite spin, or an
arbitrary point for an off-centre pivot (a turret turning about its hull mount).
Screen y is down, so a positive angle turns clockwise. `draw_sprite` and
`draw_sprite_uv` are unchanged — they delegate with no rotation (a zero-angle fast
path). It's a CPU corner-rotate baked into the vertices at push time, so batching
is unaffected. **One limitation:** the scissor clip stays axis-aligned, so a
rotated sprite is still clipped by an unrotated rect (a non-issue for free-moving
game sprites).

---

## Where the pieces live

- **The Sprite classes** → the `flicker-2d` crate. Its `lib.rs` already reserves
  it for "Sprite, Tilemap, Camera2D primitives built on flicker-render."
- **Sheet PNG assets** → `package/sprites/<subject>/` (physical runtime blobs go
  in `package/`, by type then subject, like `package/characters/<char>/`). Not
  built yet — add the folder when the first sheet lands.
- **Design of record** → MCP memory.

---

## Sharp edges

- **Don't build a second sprite-sheet system.** `SpriteStrip` is the routine;
  port it, don't reinvent it.
- **`set_clip` has no `None`.** Use `clear_clip()`.
- **The `sprite` widget won't animate or atlas.** That is deliberate; reach for
  the `flicker-2d` layer.
- **Rotation uses `draw_sprite_ex`, not the widget.** Centre pivot is
  `position + size * 0.5`; the scissor clip stays axis-aligned under rotation.
- **One palette.** New colours are tokens in `ui_theme.json`; a `theme` key
  anywhere else is refused, loudly.
