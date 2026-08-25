# flicker-worldgrid

The planet's grid. Given a resolution number it builds the hex-sphere every other
world crate simulates and renders on: a sphere tiled by hexagons, with the twelve
**pentagons** that any such tiling must have, plus each cell's position, its
neighbour list, its area, and a stable id. It produces **topology only** — no
heightmaps, no storage, no erosion, no rendering — so the same grid feeds the world
sim, the tile materializer, and the scene viewers unchanged. It depends on nothing
inside the workspace (only `glam`).

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## The grid in one paragraph

Take an icosahedron (20 triangular faces, 12 vertices), subdivide every face into a
fine triangular lattice, and read off the **dual**: each lattice vertex becomes one
grid **cell**, and two cells neighbour each other when their vertices shared a
triangle edge. A vertex where six triangles meet dualises to a hexagon; the twelve
original icosahedron vertices are where only five meet, so each becomes a
**pentagon** — a cell with five neighbours instead of six. That is the whole trick:
the pentagons *fall out* of the geometry, twelve of them at every resolution, one
per icosahedron vertex, and they need no special-casing downstream — a pentagon is
simply a cell whose neighbour list has length five. Cell positions come from an
equal-area map (ISEA/Snyder), so every hexagon covers nearly the same area wherever
it sits on the sphere.

## Vocabulary (first use, one clause each)

- **cell** — one tile of the grid; a hexagon, or one of the twelve pentagons.
  Everything is indexed by a dense `0..len` cell index.
- **pentagon (defect)** — a five-neighbour cell. Exactly twelve exist, one on each
  icosahedron vertex; they are unavoidable, not a bug.
- **shard** — one of the twenty triangular icosahedron faces. Every cell belongs to
  exactly one; a cell sitting on a shared edge/corner is assigned to its
  lowest-numbered face (its canonical owner).
- **frequency (`freq`)** / **rings** — the resolution knob: how finely each face is
  subdivided. Higher = more, smaller cells.
- **ring** (patch only) — a cell's graph distance, in cells, out from the centre
  pentagon (0 at the centre).
- **`EpochCtx`** — the world sim's per-run context struct (defined in
  `../flicker-worldgen/src/pipeline.rs`) whose `dirs` and `neighbors` fields are fed
  directly from the grid's first two fields of the same name.

## Where it sits

- **Builds on:** `glam` (the `Vec3` positions). Nothing else — no workspace deps.
- **Used by:**
  - `flicker-poc-chemistry` — seeds its `World` from `icosphere(freq)`; reads
    `dirs`, `neighbors`, `area`, `shard`, `is_pentagon`, `freq`.
  - `flicker-worldtile` — materializes per-cell tiles from `icosphere` /
    `icosphere_with_outlines`.
  - `flicker-worldengine` — builds `EpochCtx` from a grid's `dirs` + `neighbors`.
  - `flicker-worldgen` — **dev-dependency only**: its
    `tests/epoch_on_pentagon_patch.rs` feeds a `pentagon_patch` straight into
    `EpochCtx` (the spec's "minimal complete hard case" — a pentagon and its full
    five-fold neighbourhood). Defines `EpochCtx`.
  - `flicker-godmode`, `flicker-populous` (scenes) — build a planet with
    `icosphere_with_outlines` for the God Mode inspector and the bench map.
- **Reads from the content tree:** none. It takes numeric arguments and returns
  plain data; it reads no files.

## Public API

Everything below is re-exported from `lib.rs`. The two build functions are the whole
entry surface; the rest are the structs they return.

### Build a grid

| Item | What it does | The one thing to know |
|---|---|---|
| `icosphere(freq: u32) -> Sphere` | The full closed planet grid at subdivision `freq`. | `freq` is **clamped to ≥ 1**. Yields exactly `10·freq² + 2` cells, always exactly 12 pentagons. |
| `icosphere_with_outlines(freq) -> (Sphere, Vec<Vec<Vec3>>)` | Same grid, plus each cell's ordered boundary polygon (corner positions on the unit sphere) for rendering. | Built in one pass — call this instead of `icosphere` if you need outlines; don't rebuild. `outlines[i]` has 5 corners for a pentagon, 6 for a hex, and is parallel to the cell index. |
| `pentagon_patch(rings: u32) -> Patch` | A single pentagon-centred cap: one pentagon, `rings` complete hex rings around it, and a partly-resolved fringe. The minimal hard case for tests/bring-up. | `rings` is **clamped to ≥ 1**. The centre pentagon is always cell `0`. |

### `Sphere` — the full-planet product

Parallel vectors, one entry per cell, all indexed by the dense cell index `0..len()`.

| Field | Type | Meaning |
|---|---|---|
| `dirs` | `Vec<Vec3>` | Unit-sphere position of each cell (→ `EpochCtx.dirs`). |
| `neighbors` | `Vec<Vec<u32>>` | Adjacency (→ `EpochCtx.neighbors`): **5** for the twelve pentagons, **6** for every hex. The sphere is closed — no boundary cells, every list is full. Sorted ascending. |
| `area` | `Vec<f32>` | Per-cell area on the unit sphere. Hexes are equal to within ~6% (see gates); pentagons are genuinely smaller (five wedges, not six). |
| `is_pentagon` | `Vec<bool>` | `true` for exactly the twelve pentagons. |
| `shard` | `Vec<u8>` | Owning icosahedron face, `0..20`. |
| `id` | `Vec<CellId>` | Stable global id of each cell (see `CellId`). |
| `freq` | `u32` | The (clamped) subdivision frequency this grid was built at. |

Methods: `len() -> usize` (cell count), `is_empty() -> bool` (always `false`).
Cells are emitted in `(shard, Morton)` order, so `id` is non-decreasing and each
shard's cells are contiguous.

### `Patch` — the pentagon-centred product

Same parallel-vector shape as `Sphere`, but for one cap rather than the whole
sphere, and with ring/fringe metadata instead of shard/id.

| Field | Type | Meaning |
|---|---|---|
| `dirs`, `neighbors`, `area`, `is_pentagon` | as `Sphere` | but `neighbors` is **5** for the centre, **6** for interior hexes, and **fewer** for fringe cells. |
| `interior` | `Vec<bool>` | `false` for the outer fringe cells the cap does not fully resolve. The `interior == true` cells form the real N-ring disc; a consumer that wants clean hexes filters on this. |
| `ring` | `Vec<u32>` | Graph distance from the centre (0 at the pentagon). |
| `center` | `u32` | The centre pentagon's cell index — **always `0`** after ordering. |

Methods: `len()`, `is_empty()` (as `Sphere`).

### `CellId` — the stable global id

| Item | What it does | The one thing to know |
|---|---|---|
| `CellId(pub u64)` | A cell's stable id: `(shard << 48) \| morton(i, j)` of its canonical lattice site. Unique per cell **at a given frequency**. | Not yet wired to persistence — the ledger `CellId ↔ CellCoord` seam is a later slice (deferred in the spec). Today consumers use the dense `0..len` index; `id` is the durable handle for when persistence lands. |
| `CellId::shard(self) -> u8` | The shard (icosahedron face) packed in the id's high bits. | Returns the same value as `Sphere.shard[i]` by construction — a convenience accessor; not yet called by any consumer. |

## Interactions

**None of the flicker interaction surface applies.** This is a pure-data crate: it
captures no signals, publishes and binds no Model keys, fires no results, reads no
content files, and runs no threads or async. It takes numbers and returns structs.

What it *hands other crates*: the `dirs` + `neighbors` (+ `area`, `is_pentagon`,
`shard`) of a `Sphere`/`Patch` are what the world sim consumes — via
`flicker-worldgen`'s `EpochCtx` or `flicker-poc-chemistry`'s `World::seed`; the
outlines feed the renderers; `id` / `CellId` is the deferred persistence handle.

## Gates

The contract is enforced by 20 tests (`cargo test -p flicker-worldgrid`). The ones a
consumer relies on:

**Full sphere (`sphere.rs`)**
- `exactly_twelve_pentagons_rest_hexes` — 12 degree-5 cells, everything else degree 6.
- `cell_count_matches_goldberg_formula` — `len() == 10·freq² + 2`.
- `euler_characteristic_holds` — the dual is a closed polyhedron (`V − E + F = 2`).
- `adjacency_symmetric_and_in_range` — every neighbour link is mirrored and in range.
- `all_twenty_shards_populated_and_ids_unique` — all 20 shards non-empty, all ids
  unique and in scan order.
- `directions_unit_and_total_area_is_a_sphere` — `dirs` are unit; total area ≈ 4π
  (within ~5% — the flat dual slightly under-counts the curved sphere).
- `hexes_are_equal_area` — hex area spread `< 1.06` at freq 8 and 16 (this is what
  the ISEA map buys; pentagons excluded).
- `outlines_cover_every_cell_and_close_the_ring` — one outline per cell, right corner
  count, all on the sphere.

**Pentagon patch (`patch.rs`)**
- `exactly_one_pentagon_at_the_center`, `interior_hexes_have_six_neighbors`,
  `adjacency_is_symmetric_and_in_range`, `directions_are_unit_length`,
  `areas_are_positive_and_roughly_uniform` (interior hex spread `< 1.10`),
  `patch_grows_with_rings_and_stays_single_defect`.

**Equal-area projection (`isea.rs`)** — the drift guards for the ISEA placement:
`the_map_is_area_true` (the property the projection exists for),
`analytic_and_measured_face_circumradius_agree`, `the_sub_triangle_closes`,
`edge_placement_is_endpoint_order_independent` (shared-edge cells weld exactly),
`edge_sites_walk_the_arc`, `the_only_creases_are_the_vertex_rays`.

## Sharp edges

- **`freq` / `rings` are silently clamped to ≥ 1.** Passing `0` gives you the
  smallest grid, not an error — sweeping a resolution from 0 upward, `0` and `1`
  produce the same output.
- **Pentagons are smaller than hexes.** Their `area` is genuinely lower (five wedges
  vs six); code that averages or thresholds on area must exclude them
  (`is_pentagon`), as the gates do.
- **Total area is ~5% under 4π, and hexes vary ~6%.** Areas are flat barycentric
  duals of a curved surface, so they under-count slightly; the ISEA map holds the
  *spread* tight but does not make them identical. The residual variation is
  Snyder's vertex-ray crease (cells sitting on a crease come out slightly small) —
  a documented property of the projection, not drift.
- **`Patch` has a fringe.** Its outer ring (`interior == false`) has short neighbour
  lists and is not a complete hexagon neighbourhood; filter on `interior` for the
  real disc.
- **`id` / `CellId` is not wired to storage yet.** Use the dense `0..len` index for
  everything today; `CellId` is stable across builds *at the same `freq`* but the
  final bit layout is pinned when the ledger seam lands.
- **`freq` is the caller's knob — this crate is resolution-agnostic.** The shipped
  world fixes a reference frequency elsewhere (`flicker-poc-chemistry`'s config, the
  ~49.65 mi "Prism Earth" hex); worldgrid itself takes whatever `freq` you pass.
