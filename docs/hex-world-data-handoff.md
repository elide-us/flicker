# Handoff — hex-world pivots from render POC to the data/sim layer

> **⚠ PARTLY SUPERSEDED** by the *HexWorld — Flat Neighbor Graph & Celestial
> Orientation* spec. The topology/render model here is replaced by the flat hex
> graph (no sphere in data or render). What still stands: the end goal — a hex's
> owned heightmap layers → `materialize()` → the cluster vector voxel-cluster
> consumes. Read that goal here; read the data model from the flat-graph spec.

> The render question is **resolved**; `examples/hex-world` now turns toward its
> real job — being the *data* layer that feeds voxel-cluster. This doc records
> the resolution and frames the data work so the next session starts there.

## What the render POC settled (don't re-litigate)

- **The defect is invisible to rendering.** A hex-sphere is a fullerene → it is
  **3-valent**: every edge borders 2 faces, every corner exactly 3 — pentagon or
  hex, no exceptions. The "5 vs 6 neighbour" count is a property of a face's full
  *ring*, and the renderer **never enumerates a ring**.
- **≤3 hexes ever render.** Horizon < one hex, so you only ever see the hex
  you're on plus, near an edge/corner, 1–2 others (`MAX_TILES = 3`). So 5/6,
  pentagons, pole caps, neighbour offsets — **all moot in the render path.** The
  unavoidable 720° of defect lives only in the (faked) sim + the polar caps.
- Current code: plain pointy-top hex, gnomonic-flattened onto the hex you're on;
  render set = the 3 hexes nearest the look direction (a crude stand-in for the
  real edge→1 / corner→3 lookups — fine for proving the model).
- The proven hook: **standing-position → (hex, position-within-hex)** is the same
  resolution voxel-cluster's camera/streaming needs. That's the bridge target.

## The real job now — heightmap layers → voxel-cluster data

The chain (see `docs/flicker-world-system-spec.md` §3–4 + `docs/architecture.md`
"source-of-truth invariant"):

```
hex (2048² cluster-columns)          ← the data we build here
   each pixel = a material ledger      (composition vector + trait fields)
   stacked layers (top/middle/bottom)  (the "three dots")
        │  cluster-column materialization
        ▼
column ledger → LOD8 cluster vector   ← the "seam-to-voxel" step (spec §13.5)
        │
        ▼
voxel-cluster contour/mesh/LOD        ← existing renderer, today fed by the
                                          procedural WorldScene::world_at()
```

The bridge to build: **replace `WorldScene::world_at(offset)` in
`examples/voxel-cluster` with a materializer that reads a hex's column data**
instead of pure procedural noise. The renderer doesn't change — only its input
source (that's the architecture invariant: input source changes, output path
doesn't).

## Suggested first slice (data, not pixels)

1. **Hex column data structure** in hex-world: a hex owns a grid of columns;
   each column is the layer stack (top elevation + material per layer). Start
   small (e.g. 64² columns, 2–3 layers) — content can still come from
   `world_height_seeded` at first; the point is it's now the hex's *owned data*,
   not sampled live.
2. **`materialize(column) → cluster feed`** — the seam-to-voxel function: a
   column's stacked layer heights/materials → the voxel cluster's solidity +
   material (a `Primitive` the existing `contour()` consumes, or the LOD8 vector
   directly). This is the one genuinely new piece.
3. **Wire it into voxel-cluster** behind `world_at` and confirm a baked cluster
   matches what the procedural path produced — proving the data path is sound
   before making the data interesting.

## Open decisions to carry in

- Materialize → a `Primitive` (let `contour()` do the work) vs. emit the LOD8
  cluster vector directly (skip contour). Spec leans Primitive; confirm.
- Column vertical model: how a 2D pixel's layer stack becomes the 3D voxel
  column (the depth axis the spec's "three layers" implies but the heightmap
  doesn't carry yet).
- Whether hex-world keeps the lat/lon addressing or moves to the 6-strips
  structure — **the renderer won't care; only the sim/data will.**
