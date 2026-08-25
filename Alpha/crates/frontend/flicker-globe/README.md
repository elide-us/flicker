# flicker-globe

Draw a planet — a hex-tiled sphere — into a bench's viewport, or into the whole window,
flown by an orbit camera. This is the **one** globe in Prism: God Mode, the Epoch
Simulation and the Populous bench all ask this crate for their planet, so the mesh, the
reference frame, the offscreen plumbing and the camera cannot drift into three
subtly-different pictures.

Hand it a **shell list** (what the world is made of), tell it where on screen it sits,
put it in the input chain, and it renders.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file is a guideline for using the crate; the
> public items document themselves in source.

A few flicker words, defined once:

- **bench** — a scene/tool app in the roster (God Mode, Populous, the Epoch Simulation).
- **walker** — the UI layout + hit engine (in [`flicker-widgets`](../flicker-widgets/README.md))
  that lays out panels and reserves a rectangle for each live-drawing region.
- **surface** — a live-drawing region the walker lays out: a nested viewport inside a
  bench, or the root screen. A globe is drawn into a surface.
- **seat** — telling this world *where* the walker placed its surface this frame.
- **stage** — the authored look a surface is drawn with (clear colour, lights, draw
  layers), compiled from the style files by the one stage compiler.
- **shell** — one layer of the globe: hex patches on a sphere at a radius, coloured per
  cell. A world is a *stack* of shells.
- **signal** — an `ActionSignal` resolved by the input system from whatever device
  produced it. The camera hears signals, never keys or a stick.

## What it can draw

- **Cap shells** — the ordinary planet: a flat hex patch per cell. A cell whose colour
  closure returns `None` is skipped, so a shell is **sparse** — a crust shell has holes
  where the mantle is still bare.
- **Column shells** — the same tiling built as **closed solids with real depth**: top
  face, bottom face, and side walls whose edges lie along the corner directions, out
  from the centre of the world. A stack of them reads as the gently widening cone a
  radial column actually is. This is the hex-stack ledger's drawing form.
- **The graticule** — one shared reference frame: parallels, meridians, and the four
  latitudes that mean something. Not decoration: the insolation law reads latitude off
  the Y axis, so the equator, tropics and polar circles mark where the surface
  temperature bands actually change.
- **A line overlay** — grouped by colour, drawn over the world. The authored graticule
  is always drawn underneath it, never instead of it.

Two colour helpers ship here so no bench grows its own: `temp_color` is **THE** heat ramp
(cool deep-blue → red → white-hot) and `lerp3` is the primitive other ramps are built
from. Both take and return plain RGB triples, so a painted field drops straight into a
shell's colour closure. `temp_color` reads *relative to the field's own span* —
deliberately, because a heat view's job is to show where the heat is.

## Using it

The whole globe is one object. A bench constructs it against an **authored stage** (a
`stages.<name>` block in its style file), names the panel whose focus hands it the
camera, publishes a shell list, seats it where the walker reserved a rect, and renders.

Two things a caller must get right, because both fail quietly:

- **Seat before you render, and in the right phase.** The walker publishes the surface
  slot in its *update* pass; a windowed globe seats during update and declares its pass
  in render. Miss the seat and render is a silent no-op — correct when off screen, a bug
  when you meant to draw.
- **Windowed and root are different calls.** A globe inside a panel renders offscreen
  and composites; a globe that *is* the screen renders straight to the swapchain with no
  seat and no target. Picking the wrong one draws nothing or double-composites.

New shells are built on the CPU when published and uploaded at the next render, so the
caller needs no renderer to publish and the old shells stay on screen until their
replacements exist. GPU memory is freed manually — a scene that leaves its viewport
without freeing holds memory for a picture nobody sees.

### Several views of one planet

A bench that shows the same world several ways — one tab per data layer — can hold each
as a **named shell set**: *bake* a set to build it without changing what is on screen,
and *show* a key to swap which set draws. Showing is free: nothing is rebuilt, so
switching views never stalls a frame.

The point is that meshes follow **data, not selection**. Bake each view when the data
behind it changes; on a tab switch, just show. The alternative — building a view when
its tab is entered — puts a multi-million-vertex build inside the frame the user is
waiting on, and the previous view stays on screen through the hitch.

Publishing a single unnamed shell list still works and is still the right call for a
world with one view; it is the same mechanism aimed at one well-known key.

### Framing one cell up close

The same component can show *one cell of the planet* instead of the planet: rotate that
cell's outline upright, build it as a column at the planet's **true** radius, then
re-frame the camera to a few tile-widths and aim it at the cell. Populous's Hex page is
the worked example (`flicker-populous/src/scene.rs`).

Build at the true radius, not at a convenient small number — the radial taper is a
function of distance from the centre of the world, so shrinking the radius flattens the
very cone the column exists to show. And because the cell therefore sits far from the
origin, the camera must be **aimed** at it: an orbit around the origin would swing it
off screen. Re-framing supplies the *scale*; aiming supplies the *place*. Both are
needed.

## Interactions

**Signals it captures.** `LookUp`, `LookDown`, `LookLeft`, `LookRight`, `ZoomIn`,
`ZoomOut` — consumed while this world owns the camera, passed otherwise. Every other
signal always passes. Signals only: nothing in this crate names a key, a button or a
device (rules 37722F91 / DFE3E44E). The six names live in exactly one place here, so a
bench binds them in the shared input map and remapping them in Settings reaches the
planet.

**Focus gate.** A world that names a panel owns the camera while the walker's entered
pane is that panel; a **root** world (no panel) owns it whenever no pane is entered.
When it doesn't own the camera the look deflection is zeroed and the signals pass
through to whatever sits below.

**Pointer.** A left-drag orbits and the wheel zooms — but only through the walker's
per-surface pointer sample, which exists only while the cursor is over the planet with
no UI painted over it, or while a press that began there is still held. So a drag that
began on a panel can never turn the planet, and the camera never reads a raw device.

**Model keys: none.** This is a Rust component driven by direct method calls, not by the
Lua Model a scene hands to Luau. It fires no actions, exits or kernel transitions — it
draws and reports; the bench decides what to do with that.

**Content:** nothing read directly. The authored stage comes from the caller's
already-loaded style JSON, by name.

**Threads:** none. Synchronous, per-frame.

## Where it sits

- **Builds on:** `flicker` core (render + UI types) ·
  [`flicker-input-core`](../../input/flicker-input-core/README.md) (the signal catalog
  and the player's look settings) ·
  [`flicker-input-router`](../../input/flicker-input-router/README.md) (the handler seam)
  · `glam`, `serde_json`, `tracing`.
- **Used by:** [`flicker-godmode`](../../scenes/flicker-godmode/README.md) (N sparse
  shells rebuilt as its simulation advances; the cutaway; the heat ramps) ·
  [`flicker-populous`](../../scenes/flicker-populous/README.md) (**two** worlds — the
  planet, and an inspector framing one cell as a column stack) ·
  [`flicker-pocepochs`](../../scenes/flicker-pocepochs/README.md) (a **root** world that
  is the whole screen, plus the mesh builder for its cutaway stack).

Some public items have no caller outside this crate today — the view seam the world is
built from, the empty-overlay constant, the column builder reached through the shell
spec. They are engine surface, deliberately: this is a toolbox built toward the spec,
not narrowed to what scenes happen to use (rule F42DA5E0).

## Gates

`source ~/.cargo/env && cargo test -p flicker-globe` — **16 tests**, all green: 6 in
`world.rs` (the stage drives the picture; the camera answers only bound signals, only
while focused), 5 in `camera.rs` (orbit, zoom, framing, and that the player's
sensitivity and invert flags reach the planet), 5 in `lib.rs` (the shared graticule, the
inset trick, and that a column is a closed solid with radial walls).

One **external** gate keeps this the only globe:
`no_scene_reads_a_device_or_names_a_pane_style` (in
[`flicker-widgets`](../flicker-widgets/README.md)) fails the moment any scene grows its
own shell builder or orbit camera again.

## Sharp edges

- **A typo'd stage name degrades, it does not error.** The world warns and draws the
  scene's own shells default-lit. Deliberate — a style typo should cost the authored
  look, never the planet — but the loudness is a log line, not a panic. A stage layer a
  globe cannot draw is likewise named at construction, not drawn as nothing.
- **A column's depth is clamped, never skipped.** Zero or negative depth would invert a
  solid's own side walls, so it is forced to a hairline instead. That means a cell asked
  for with zero depth draws as a **sliver, not as nothing** — absence is the colour
  closure's `None`, for cap shells and column shells alike. Worth knowing before
  per-layer depths arrive as authored data.
- **Re-framing the camera moves the zoom clamps with it,** since they derive from the
  framed radius. That is the intent for an inspector view and a bug if you call it on a
  whole-planet world. Re-framing without also passing the fill fraction discards the
  opening framing the world was constructed with.
- **New shells appear next frame** — publishing is CPU-only; the upload is at the next
  render.
- **Showing a set that was never baked draws an EMPTY globe.** It warns loudly and the
  result is visibly wrong rather than a stale other view quietly standing in — the
  deliberate choice (rule 4BB12A75), but it means a typo'd or not-yet-baked key costs
  you the whole picture, not one layer of it.
- **The look deflection is a bare tuple.** Build it with the crate's own resolver rather
  than hand-assembling it; the axis order lives in that one function.

## Known finding — the camera-input surface is under an open ruling

The pointer-sample camera path (reading the walker's pointer delta and wheel to move the
camera) is the subject of an **open, unresolved** architectural conflict with rule
37722F91 (*all input events are signals*). Both camera paths ship today and both are
described above. The status and the options are Aaron's call — read conflict `79CF541E`
in MCP.

If you extend this crate, do not add a *third* device read: the fix direction is to fold
the pointer path onto the signal path, never to grow it.
