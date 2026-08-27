# flicker-worldtile

**The pixel tier** — the crate that turns one hex cell's aggregate (a single column
of strata) into the 2048² per-stratum maps that become the world's *truth* from
that moment on. Up to here the planet is ~92,000 columns of numbers, tens of
megabytes, cheap enough to run a whole geology on a laptop; but a number has no
inside — a cell knows it holds so much basalt, not *where in itself* the basalt is,
and you cannot walk around in a number. This crate is the one-way door through
which a cell's aggregate becomes a place: **conservative** (every gram is in the
maps and none invented), **continuous across the seam with its neighbours** (no
cliff around every hex), and **deterministic** (same world + cell → the same maps,
byte for byte). It is a headless, GPU-free library over in-memory data; it owns no
window, no files, and no simulation of its own — it reads a finished `World` and
answers the question *what does the ground here actually look like, pixel by pixel*.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## flicker words used here, defined once

- **aggregate** — the pre-migration truth: one **column** of strata per cell, the
  thing the whole geology (`flicker-poc-chemistry`) runs on. A column knows *how
  much* of each bed it holds, nothing about where inside the cell it sits.
- **column** — one cell's vertical rock stack (`flicker_poc_chemistry::Column`); a
  **layer** / **bed** / **stratum** is one deposited slab in it.
- **pixel = voxel cluster** — one pixel of a tile is one 128-ft-across cluster of
  ground, the same 128 ft the whole scale chain is built on. It is a *place*, not a
  quality setting.
- **tile** — the 2048×2048 square of pixels that holds one cell. A cell is a
  hexagon, so a tile has **corners that belong to nobody** (~3.5 M of its 4.19 M
  pixels fall inside the hexagon).
- **the truth migration** — the act of materialising a cell: after it runs, the
  maps are truth and the aggregate is a *rollup* of them. It runs **once** per
  cell and its output is irreplaceable (a lead body 7,000 clusters wide is a
  spatial fact you cannot recover by dividing a hex's lead by its pixel count).
- **composite / relief** — the total ground standing on a pixel (the sum of its
  strata thicknesses, in metres). This is the surface a walker stands on and the
  thing that must agree at a seam.
- **skirt** — the ground just *outside* the rim, frozen at migration, so water at
  the edge knows whether the world continues uphill or falls away — a tile is a
  piece of a planet, not an island.
- **conformable** — how the beds are laid at migration: every bed drapes the same
  composite shape in proportion, because the aggregate knows the *amount* of each
  bed and nothing about how it varied inside the cell.
- **pixel-born** — a stratum created at pixel scale by erosion depositing sediment,
  as opposed to one migrated from the column. `Tile.pixel_born` counts them.
- **span canon** — the fixed fact that a hex **is** `TILE_SPAN_M` across (2048 px ×
  128 ft) at every grid frequency, so a coarser grid is a *smaller planet*, not one
  with bigger hexes. One derivation, pinned by a guard test (see Gates).

## Where it sits

**Cluster:** `Alpha/crates/world` (the world-sim crates).

- **Builds on:**
  - [`flicker-poc-chemistry`](../flicker-poc-chemistry) — the aggregate it reads:
    `World` (its `grid`, `columns`, `cell_area_m2()`), `crust_thickness_m`,
    `density_kg_m3`, `bed_resistance`, and the size-chain constants `TILE_SPAN_M`
    and `radius_for_freq` (both re-exported from here — one derivation, both tiers).
  - [`flicker-worldgrid`](../flicker-worldgrid) — the icosphere topology behind
    `World.grid` (`Sphere`: `dirs`, `neighbors`, `is_pentagon`, `freq`) and the
    per-cell boundary polygons (`icosphere_with_outlines`) that become the mask.
  - `glam` — `Vec3` / `DVec3` / `DVec2` geometry.
  - *(dev only)* [`flicker-materials`](../../content/flicker-materials) — `Tables`,
    to grow a real world in the end-to-end tests.
- **Used by:**
  - [`flicker-godmode`](../../scenes/flicker-godmode) — the God Mode viewer's tile
    inspector and its per-pixel erosion worker thread (`src/sim_thread.rs`). It is
    the only consumer today, and it owns *all* input wiring (see Interactions).
- **Reads from the content tree:** **nothing.** This crate operates purely on the
  in-memory `World` the caller hands it; it opens no files. (The tests reach the
  content data dir *through* `flicker-poc-chemistry` to grow a world — that is test
  scaffolding, not a runtime dependency of this crate.)

## Public API

Everything below is reachable from `lib.rs`. Grouped by what you are doing, not by
Rust item kind.

### The scale chain (constants)

A caller almost never sets these; they are the pinned facts the rest of the crate
is measured against. `TILE_SPAN_M` and `radius_for_freq` are **re-exports** from
`flicker-poc-chemistry` so the pixel tier and the chemistry tier cannot drift apart.

| Item | What it is | The one thing to know |
|---|---|---|
| `TILE_DIM: u32` | Pixels across a tile — `2048`. | A hex *is* a 2048² map; not a resolution knob. |
| `PIXELS_PER_TILE: usize` | Pixels in a whole tile, corners included — `2048²`. | ~3.5 M of them are inside the hexagon; the rest belong to nobody. |
| `TILE_SPAN_M: f64` | How far a tile spans, m (49.65 mi). | Re-export. Fixed **at every frequency** — this is the span canon. |
| `FEET_PER_PIXEL: f64` | `128.0` — the width of one voxel cluster. | With `TILE_DIM` × `METRES_PER_FOOT` it derives `TILE_SPAN_M`; a guard test pins the two equal. |
| `METRES_PER_FOOT: f64` | `0.3048`. | Stated once, here. |
| `radius_for_freq(freq) -> f64` | The planet radius a grid frequency implies, m. | Re-export. **The span is fixed, so the radius follows the grid** — a coarse grid is a smaller planet. Pass its result as the tile radius; see Sharp edges. |

### Placement — where a tile's pixels are (`mask` module)

| Item | What it is | The one thing to know |
|---|---|---|
| `TileFrame` | A cell's frame: where its tile sits on the sphere and which way is up. | Derived from the **cell alone** (its centre + first corner), so two runs place every pixel identically. Gnomonic tangent-plane placement; distortion is parts-in-ten-thousand over a tile, well under a pixel. |
| `TileFrame::new(cell, centre, outline, radius_m)` | Build a cell's frame. | `outline` is the cell's real boundary; `east` points at its first corner. An empty outline falls back to an arbitrary (but stable) frame — see Sharp edges. |
| `TileFrame::plane_m(x, y) -> DVec2` | In-plane metres of a pixel *centre*, from the tile middle. | Pixel **centres**, so the map is symmetric about the cell. |
| `TileFrame::direction(x, y) -> DVec3` | Where a pixel sits on the unit sphere. | This is the position the shape field is sampled at. |
| `TileFrame::pixel_area_m2() -> f64` | Area of one pixel, m². | Same for every pixel — what makes a pixel a cluster-sized piece of ground anywhere. |
| `TileFrame::pixel_span_m() -> f64` | The span of one pixel, m (128 ft). | Defined in the `erosion` module; used as the run length for slope. |
| `HexMask` | Which pixels of a tile are inside the cell. | Built from the cell's real boundary polygon, so **a pentagon masks like anything else** — nothing counts sides. |
| `HexMask::new(frame, outline)` | Mask a tile from its boundary. | Even-odd point-in-polygon; needs ≥ 3 boundary points or the mask is empty (see Sharp edges). |
| `HexMask::contains(x, y) -> bool` | Is this pixel the cell's? | — |
| `HexMask::count() -> usize` | How many pixels the cell owns (~3.8 M). | The denominator for the mass correction. |
| `HexMask::iter()` | Walk every pixel inside the cell. | The standard way to touch a tile's real pixels — skips the corners. |

### Shape — what the ground does, as a function of *where you are* (`shape` module)

This is the one idea that makes the migration work: the surface is defined
**globally, in terms of position on the sphere**, and a tile merely *samples* it. A
tile therefore has no say in what the ground does at its own boundary, so two tiles
that meet get the same answer there — continuity by construction, no stitching pass.

| Item | What it is | The one thing to know |
|---|---|---|
| `Neighbourhood` | Everything a sample needs, gathered once per tile (a cell + its two rings of neighbours). | Gather once, sample 3.8 M times — the difference between a tile in a moment and a tile in a minute. Two rings, because influence reaches across a boundary. |
| `Neighbourhood::around(world, cell)` | Gather the neighbourhood for a cell. | Reads `world.grid.neighbors` and `crust_thickness_m` of each column in reach. |
| `Neighbourhood::relief_at(p) -> f64` | The crust **thickness** the ground has at position `p`, m. | A compactly-supported convex blend of the columns in reach — infinite weight at a column's own centre, exactly zero at the reach. At a cell centre it is that cell's own thickness. Named "relief" but returns thickness-as-height; see Findings. |
| `Neighbourhood::range() -> (f64, f64)` | The lo/hi thickness the columns in reach span. | What a blend over them is bounded by — used by the lean test. |
| `relief_at(world, cell, p) -> f64` | The same read without keeping a `Neighbourhood`. | Convenience for a caller with one position and no tile (the inspector asking "what is the ground here"). Rebuilds the neighbourhood each call — do not use it in a per-pixel loop. |

### Materialize — the one-way door (`materialize` module)

| Item | What it is | The one thing to know |
|---|---|---|
| `materialize(world, cell, radius_m, outline) -> Tile` | **Materialise one cell** — turn its column into the maps that become truth. | Deterministic; no seed (shape comes from the neighbourhood, mass from the ledger). ~131 ms for a 6-stratum tile. `radius_m` **must** be `radius_for_freq(world.grid.freq)` — see Sharp edges. |
| `Tile` | One materialised cell: a stack of thickness maps, bottom bed first. | Fields below. Thicknesses are `f32` m *deliberately* — quantisation is the storage tier's call, not the arithmetic that has to conserve mass. |
| `Tile.cell: u32` | The cell this is the inside of. | — |
| `Tile.frame: TileFrame` | Where its pixels are. | — |
| `Tile.mask: HexMask` | Which of them belong to it. | — |
| `Tile.strata: Vec<Vec<f32>>` | One `TILE_DIM²` thickness map per stratum, bottom → top. | Zero outside the mask. Beds are stored **separately** from the first because erosion makes them diverge immediately. |
| `Tile.skirt: HashMap<(u16,u16), f32>` | The ground just outside the rim, at the field's height. | How rim water knows the world continues; frozen at migration. |
| `Tile.pixel_born: usize` | How many topmost strata were laid by erosion, not migrated. | `0` fresh from the door; reintegration (T8) reconciles them with the ledger. |
| `Tile::composite_m(x, y) -> f32` | Total thickness on a pixel (the surface). | The ground a walker stands on. |
| `Tile::exposed_bed(x, y) -> Option<usize>` | Topmost bed with any thickness there. | What is exposed, and the bed erosion cuts next. `None` outside the mask. |
| `Tile::stratum_mass_kg(stratum, density) -> f64` | Mass of one stratum as the maps hold it, kg. | The left side of the trial balance. Weighs by `pixel_area_m2` — a different area convention from the column side; see Sharp edges. |
| `Tile::preview_rgba(scale) -> (Vec<u8>, u32)` | An RGBA image for the inspector, `scale`× smaller. | Nearest-sample reduction, not a mip — honest about a panel's size, not a quality image. Shaded by height, tinted by exposed bed, transparent outside the hex. |
| `Tile::summary() -> String` | One-line caption: cell · beds · clusters · height range. | For the inspector panel. |

### Erosion — cutting the honest shape into a place (`erosion` module, T7)

The migration lays a bland, honest shape; erosion is what earns the detail. Rain
gathers downhill across ~3.5 M pixels, cuts by **differential resistance** (soft
ground first — erode everything equally and, after normalising, nothing happened),
carries the load in **kilograms**, and settles it where the water slackens. Every
pass balances the books: `Σ(before) = Σ(after) + exported`, exactly.

| Item | What it is | The one thing to know |
|---|---|---|
| `Eroder` | The erosion engine for one tile; owns its scratch buffers. | Reuse one across passes — the buffers are ~100 MiB and `pass()` allocates nothing. `new()` / `Default`. |
| `Eroder::pass(tile, props, params) -> PassReport` | One pass of rain: route → gather → cut → carry → settle → slump talus. | Mutates `tile` in place. Deterministic. |
| `BedProps { density_kg_m3, resistance }` | What erosion needs to know about one stratum. | The cut is **divided by** `resistance` (0..1) — the caller reads it from the rock tier (`bed_resistance`) and density (`density_kg_m3`); this crate stays pure arithmetic. |
| `ErosionParams` | The dials of a pass (`rate`, `capacity_factor`, `rain`, `talus_slope`, `talus_rate`, `sediment`). | `Default` is the physics as written. `capacity_factor` is **dimensionless** (scales the same `√flow·slope` term as the cut) — an absolute-kg capacity was this module's first bug. |
| `PassReport { eroded_kg, deposited_kg, exported_kg }` | What one pass did — the ledger the caller reconciles. | `exported_kg` is **banked, not destroyed** — the caller owns handing it to the neighbour or the sea. |
| `tile_mass_kg(tile, props, params) -> f64` | Total mass standing on the tile, kg. | The left side of the pass ledger; pixel-born beds weigh in at the sediment density. |
| `erosion::demo::ridge(radius_m)` | A hard dike in a soft plain, filled flush — watch the ridge emerge *by subtraction*. | A watchable acceptance fixture; asserts nothing (the tests pin the mechanism). |
| `erosion::demo::canyon(radius_m, beds)` | A tilted layer cake — a trunk stream cuts down and the walls expose the stack. | The Columbia-gorge walk in miniature; a watchable fixture. |

## Interactions

- **Signals it captures / results it fires / Model keys:** **none.** This is a
  headless library with no scene surface. It captures no signals and names no keys
  (it could not — there is nothing here to wire input to). Its consumer
  `flicker-godmode` owns the inspector: it translates the operator's *inspect this
  cell* and *toggle the rain* signals into its own `SimCommand`s
  (`src/sim_thread.rs`), calls `materialize`, and drives an `Eroder` on a
  background thread. If you need the input contract, read that crate — this one has
  no input contract to document.
- **What it hands other crates:**
  - a `Tile` from `materialize` — the pixel-tier truth for one cell;
  - a `PassReport` per erosion pass — the ledger, including `exported_kg` the
    caller must bank for the neighbouring tile or the sea;
  - `Tile::preview_rgba` + `Tile::summary` — a ready-to-blit RGBA image and caption
    for an inspector panel.
- **Threads / workers:** none of its own. It is *designed to be run off-thread* by
  the caller — a `materialize` costs a tick, not a frame; an `Eroder::pass` over
  3.5 M pixels belongs on a background thread at a restrained cadence (which is
  exactly how `flicker-godmode` runs it). `Eroder` owns its buffers so a pass on a
  worker allocates nothing.

## Gates

The tests are the drift gates; a change must keep them green. Run
`cargo test -p flicker-worldtile`.

**Scale chain (`lib.rs`)**
- `scale_chain::the_tile_span_is_the_world_span` — this crate's own span derivation
  (`TILE_DIM × FEET_PER_PIXEL × METRES_PER_FOOT`) is *exactly* the `TILE_SPAN_M` the
  world constants carry. The drift between two derivations of this number is how one
  repo once simulated two different-sized planets at once.

**Placement (`mask.rs`)**
- `a_tile_spans_a_hex` — the scale chain closes: 2048 px at 128 ft is 49.65 mi.
- `the_corners_belong_to_nobody` — a hex keeps most of its tile but not the corners;
  the middle is always inside, the corner never is.
- `pixels_sit_where_the_frame_says` — the centre pixel is the cell centre; a pixel
  half a span out has moved half a span.
- `a_pentagon_masks_like_any_other_cell` — a five-sided cell owns pixels too.

**Shape (`shape.rs`)**
- `a_cell_centre_reads_its_own_column` — at a cell's centre the sampled ground *is*
  that cell's own thickness.
- `two_cells_agree_about_the_ground_between_them` — **the seam property**: two
  neighbours sampled at the same boundary point return the same ground (exact).
- `the_ground_leans_between_columns` — the blend stays within the range its columns
  set; the ground leans, never jumps.

**Materialize (`materialize.rs`)**
- `the_maps_hold_exactly_what_the_column_held` — the trial balance: the beds keep
  their proportions exactly (see Sharp edges on why proportions, not absolutes).
- `a_tile_leaves_its_edges_exactly_as_the_field_put_them` — the mass correction is
  zero on the boundary; the tile never moves its own edge (the seam's second half,
  tested by construction rather than by comparing two tiles).
- `materialising_twice_gives_the_same_tile` — deterministic, byte for byte.
- `a_pixel_carries_the_whole_stack_above_it` — there is ground in the middle and
  something exposed on top of it; nothing outside the hexagon.

**Erosion (`erosion.rs`)**
- `a_pass_conserves_every_gram` — `Σ(before) = Σ(after) + exported`, exactly.
- `erosion_is_deterministic` — same tile eroded twice → same maps and same books.
- `soft_ground_goes_first` — the same cutting budget takes more off soft rock than
  hard (the outcrop model at pixel scale).
- `cutting_through_a_bed_exposes_the_one_beneath` — the cascade: cut past a bed and
  the next one shows (how a canyon wall reveals the stack).
- `the_watershed_drains_across_the_rim` — a sloped tile with a low skirt exports
  mass (banked, not lost).
- `slack_water_lays_a_new_bed` — deposition grows a pixel-born bed; `pixel_born` → 1.
- `the_plain_lowers_and_the_dike_does_not_go_with_it` — the ridge emerges by
  subtraction (the mechanism, measured; the scenery is the maintainer's eye).
- `real_world::rain_on_a_real_tile_keeps_the_books` — T6 + T7 end to end on a grown
  world, beds carrying the rock tier's real reads; books balance and rain does
  something.

## Sharp edges

- **`materialize` is a one-way door.** It runs **once** per cell and its output is
  irreplaceable; nothing downstream may reconstruct a pixel by averaging the
  aggregate. `Σ(pixels) ≡ aggregate` stops being a way to *derive* pixels and
  becomes the *check* on them.
- **Pass `radius_for_freq(world.grid.freq)` as the tile radius — nothing enforces
  it.** The span is fixed, so the radius is fully determined by the grid frequency.
  A wrong radius (e.g. Earth's) produces a *silently wrong* tile where every pixel
  falls inside its own hexagon and the pixel tier means nothing. This is the exact
  failure the design record says broke the first test run — and nothing in
  `materialize` checks it, so the caller must get it right.
- **The `outline` must be the cell's real boundary from the same grid**
  (`icosphere_with_outlines`), ≥ 3 points, in order. A shorter/empty outline yields
  a stable-but-arbitrary frame and an empty mask, silently — the caller must pass a
  valid boundary.
- **Conservation holds to `f32`, not `f64`.** The maps are `f32`; erosion books the
  `f32` the map actually lost (not the `f64` plan), and folds the rounding shortfall
  into `exported_kg`, or a hundred passes of rounding would read as a leak. If you
  reconcile the ledger, trust `PassReport`, not your own re-derivation from the
  `f64` inputs.
- **Mass is checked as *proportions*, not absolutes.** `stratum_mass_kg` /
  `tile_mass_kg` weigh by the tile's `pixel_area_m2`, while the column side
  (`crust_thickness_m`) uses the grid's nominal `cell_area_m2` — two different area
  conventions. The two absolutes therefore differ; reconciling them is the storage
  tier's job (T8) and is a *tracked open item*, not a bug.
- **No persistence.** Maps live in memory; the blob format and the data-folder / DB
  shape are the storage tier's (T8).
- **`preview_rgba` is a preview**, not an image — nearest-sample, no mip.
- **`Eroder` is heavy.** ~100 MiB of scratch buffers; construct one and reuse it
  across passes rather than per pass.
