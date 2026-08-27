# flicker-primitive

The engine's **shape and field layer**: implicit shapes the voxel contour turns
into cluster data (or the editor stamps into it), plus the continuous scalar
fields — the world heightmap and the shared seeded noise — that anything can
sample and agree on. It is pure compute: no GPU, no I/O, no content files, no
storage. It depends only on the `clayengine` foundation, so it and the voxel
storage layer stay peers — the one place they meet is the voxel crate's
`contour()`, which reads a primitive from here and writes cluster data.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

## Vocabulary (flicker terms used below)

- **Primitive** — an implicit shape that answers two questions about a point:
  is it inside the solid, and where/which-way does the surface cross an edge. It
  is a *source* the contour queries, not stored data.
- **SDF** (signed-distance field) — a shape given as one function `distance(p)`:
  negative inside, positive outside, zero on the surface. The six analytic
  shapes are SDFs; the crate derives a Primitive from any SDF.
- **Hermite data** — the two facts the contour needs at a surface-crossing edge:
  the crossing **position** and the surface **normal** there.
- **Contour** — the voxel crate's bake/edit-time pass that samples a primitive on
  a grid and writes a cluster's vector data. It lives in
  [`flicker-voxel`](../../world/flicker-voxel/README.md); this crate only
  supplies its *input*. Contouring never runs at render time.
- **Cluster** — the world's unit of voxel storage: a `CLUSTER_DIM³` = **256³**
  block (`CLUSTER_DIM` comes from `clayengine`). Heights and centers below are in
  cluster-local **voxel units**.
- **Salt vs seed** — `seed` is the one world/recipe knob; `salt` selects an
  *independent* field under that seed, so two fields of the same world never
  correlate.

## Where it sits

- **Builds on:** `clayengine` — the only dependency; supplies `CLUSTER_DIM`
  (256). The scalar-field modules take plain `f32`/`f64`, so the crate pulls in
  no math dependency.
- **Used by:**
  - [`flicker-voxel`](../../world/flicker-voxel/README.md) — `contour()` consumes
    the `Primitive` trait; the crate also re-exports this whole surface plus the
    `heightmap` module, so a voxel caller depends only on `flicker-voxel`.
  - `flicker-texture` — samples the **2D** noise face (`value2_tiled`, `fbm2`,
    `worley2_tiled`, `ridged`, `billow`, `contrast`, `Fbm`) for material surfaces.
  - [`flicker-worldgen`](../../world/flicker-worldgen/README.md) — samples the
    **3D** noise face (`value3`, `fbm3`) for epoch kernels and re-exports the
    shaping helpers.
  - `flicker-pocclusters` — builds `HeightField::island(offset)` as the
    live-contour fallback so a missing island bake reproduces the same terrain.
- **Reads from the content tree:** nothing. This crate has no file, theme, or
  package inputs — every output is a pure function of its arguments (plus, for
  `world_height`, one process-wide seed snapshot — see Sharp edges).

## Public API

### The contour contract

The contour consumes a shape through **one trait**; everything else is a way to
get an implementor of it.

| Item | What it is | The one thing to know |
|---|---|---|
| `trait Primitive` | The contour's input contract: `is_solid(x,y,z) -> bool` and `edge_hermite(a,b) -> Hermite` | Sampling is **origin-based**: integer coords address a voxel's *minimum corner*, not its center. Cell `(cx,cy,cz)` spans `[cx,cx+1]³`. |
| `struct Hermite` | `{ position: [f32;3], normal: [f32;3] }` — one surface crossing | Same origin-based, cluster-local voxel frame as `is_solid`. |
| `fn Primitive::edge_hermite(a,b)` | Crossing on the edge between adjacent grid samples `a`,`b` | **Precondition:** the caller guarantees `a` and `b` differ in solidity; on a non-crossing edge the result is degenerate (no panic, no error). |
| `trait Sdf` | A shape as `distance(p:[f32;3]) -> f32`, negative inside | Object-safe, so `Scene` holds `Vec<Box<dyn Sdf>>`. Impl this to define an analytic shape. See the extension note below — a bare `Sdf` impl is **not** automatically a `Primitive` outside this crate. |

Out-of-range queries are expected: the contour asks about a boundary voxel's
neighbours outside `[0, CLUSTER_DIM)`, and a globally-defined primitive should
answer truthfully there so cluster-boundary faces are not spuriously exposed. All
shapes here do.

### Shapes

| Item | Fields / ctor | Notes |
|---|---|---|
| `struct FlatField` | `{ height }`, `FlatField::at_half()` | Horizontal plane, solid below `height`. The known-correct reference/smoke input; impls `Primitive` directly. `at_half()` = `FLAT_HEIGHT`. |
| `const FLAT_HEIGHT` | `= CLUSTER_DIM/2` = **128** | The cluster's vertical midpoint (normalized height 0.5). |
| `struct HeightField` | `new(seed, offset)`, `from_default_seed(offset)`, `island(offset)` | 2D terrain primitive; caches a `256²` column grid at construction (~65K samples). Impls `Primitive` **and** `Sdf` — see the dual-path Sharp edge. |
| `HeightField::height_at(x,z)` | `-> f32` | Nearest **integer**-column surface height (world coords). Cache hit inside the footprint; procedural fallback outside it (continuous across clusters). |
| `HeightField::height_bilinear(x,z)` | `-> f32` | **Interpolated** surface height at fractional coords; reads only the cache. `C0`-continuous, faceted at column boundaries. |
| `struct Sphere` | `{ center, radius }` | SDF. Smoothest check for DC normals on a curved surface. |
| `struct Cube` | `{ center, half }` | SDF. Axis-aligned; edges/corners facet under single-vertex-per-cell DC (expected, not a bug). |
| `struct Cylinder` | `{ center, radius, half_height }` | SDF. Y-axis capped; cap rims facet. |
| `struct Cone` | `{ center, base_radius, half_height }` | SDF. Apex points **+Y**; base at `center.y − half_height`, apex at `center.y + half_height`. |
| `struct HalfSphere` | `{ center, radius }` | SDF. Upper dome; flat base sits on `y = center.y`. |
| `struct HalfCylinder` | `{ center, radius, half_height }` | SDF. Y-axis cylinder keeping the half with `x ≤ center.x`. |
| `struct Scene` | `Scene::world()`, `world_at(offset)`, `gallery()` | Union of `Box<dyn Sdf>` parts; composite distance is the **min** over parts (implicit merge, no overlap resolution). Impls `Sdf` + `Primitive`. |

**Adding a shape.** The six analytic shapes impl `Sdf`, then a **crate-private**
macro (`impl_sdf_primitive!`) turns each into a `Primitive`. There is no blanket
`impl<T: Sdf> Primitive` (it would collide with the direct impls on
`FlatField`/`HeightField`). So inside this crate, impl `Sdf` + invoke the macro;
**from another crate**, an `Sdf` impl gives you `.distance()` and
`Scene`-composability but *not* a standalone `Primitive` — to contour a new shape
on its own, impl the two `Primitive` methods directly, or add the shape here.
(See Findings — this is the crate's one leaky seam.)

### `heightmap` — the world surface

The world's surface is one continuous function `(world_x, world_z) -> height` in
voxel units. Because it is one function, neighbouring clusters that sample the
same world coordinates agree exactly with zero coordination — that is what keeps
baked cluster seams continuous.

| Item | Signature | Notes |
|---|---|---|
| `world_height` | `(x, z) -> f32` | Uses the **process-wide default field**, seeded once from `FLICKER_SEED` (decimal or `0x…`), else `DEFAULT_SEED`. Always finite, in **[96, 160]** (`128 ± 32`). |
| `world_height_seeded` | `(x, z, seed) -> f32` | **Pure** — touches no environment; identical `(x,z,seed)` returns identical bits. The basis for all tests. |
| `island_height` | `(x, z) -> f32` | The Prism island: a radial dome (raised-cosine falloff) + light noise, centered in the 3×3 Prism field. The **one** definition of the island shape — the bake tool and the pocclusters live fallback both build `HeightField::island`, which samples this. |
| `seed_from_env` | `() -> u64` | The world seed from `FLICKER_SEED`, or `DEFAULT_SEED`. The only environment access in the crate. |
| `const DEFAULT_SEED` | `u64 = 0xCAFE_F00D_D15E_A5E5` | Seed used when `FLICKER_SEED` is unset or unparseable. |

### `noise` — the shared seeded lattice

One deterministic value-noise implementation, two faces: **3D** for world-gen,
**2D** (tileable) for textures. All sampling functions return `[0, 1)`. Identical
`(point, salt, seed)` always returns identical bits.

| Item | Signature | Notes |
|---|---|---|
| `value3` | `(x,y,z, salt, seed) -> f64` | Trilinear 3D value noise. The world-gen face. |
| `fbm3` | `(x,y,z, octaves, salt, seed) -> f64` | fBm over `value3`; doubling frequency, halving amplitude, renormalized. |
| `value2_tiled` | `(x,y, period, salt, seed) -> f64` | Bilinear 2D value noise, lattice wrapped on `period` cells. |
| `value2` | `(x,y, salt, seed) -> f64` | `value2_tiled` with no wrap (`period = 0`). |
| `fbm2` | `(x,y, Fbm, salt, seed) -> f64` | fBm over `value2_tiled`; per-octave frequency snaps to an integer so a tiled field keeps its seam. |
| `worley2_tiled` | `(x,y, period, salt, seed) -> f64` | Tiled cellular (Worley) F1 distance; reads as cracked plates / pebbles. |
| `struct Fbm` | `{ octaves, lacunarity, gain, period }` | Octave settings as **named fields** (transposing `lacunarity`/`gain` would silently produce a different field). `Fbm::default()` = `4 / 2.0 / 0.5 / 0` (untiled); `Fbm::tiled(period)`. |
| `ridged` | `(v) -> f64` | `1 − |2v−1|` — sharp crests. |
| `billow` | `(v) -> f64` | `|2v−1|` — rounded lobes. |
| `contrast` | `(v, amount) -> f64` | Push away from / toward 0.5; clamped to `[0,1]`. |

**Tiling:** `period = 0` disables the wrap (the unbounded field world-gen uses);
a positive `period` (in lattice cells) makes `f(x) == f(x + period)` a property of
the lattice itself — no blended seam. Warping a tiled field preserves tiling.

## Interactions

- **Signals / results / Model keys:** none. This is a pure compute crate with no
  UI surface — it captures no signals, fires no results, and reads/writes no
  Model keys.
- **What it hands other crates:** the `Primitive` trait (the contour's input),
  the `Sdf` trait + shape structs, and the two scalar-field modules (`heightmap`,
  `noise`). In the three-layer voxel model the primitive is **layer-1 throwaway
  input** — it exists only to derive a cluster's vector data, then is discarded.
- **Threads / workers / async:** none, with one caveat — `world_height` lazily
  builds a process-wide default field in a `OnceLock` on first call (see Sharp
  edges). `world_height_seeded` and everything else are pure.

## Gates

`cargo test -p flicker-primitive` — **40 tests**, all green. The contracts they pin:

- **Sampling & crossings** — `flat_field_solidity_boundary`,
  `flat_field_edge_crossing_is_on_plane_with_up_normal`,
  `heightfield_is_solid_matches_height`,
  `heightfield_vertical_edge_crosses_at_surface`: origin-based solidity and
  on-surface crossings with correct normals.
- **Each shape's sign + outward normal** — `{sphere, cube, cylinder, cone,
  half_sphere, half_cylinder}_is_solid_sign` and `…_edge_hermite_normal_outward`.
- **Heightmap determinism & continuity** — `same_seed_is_byte_identical`,
  `different_seeds_differ_somewhere`, `heights_are_finite_and_in_band`,
  `field_is_continuous_at_fine_scale`, `no_seam_at_cluster_boundaries`,
  `field_has_real_variation`, `world_height_uses_default_seed_when_env_unset`.
- **Island fixture** — `island_center_at`,
  `island_is_a_dome_that_floods_into_an_island`, `island_heights_stay_in_band`,
  `island_has_no_seam_at_cluster_boundaries`.
- **Seed parsing** — `parse_seed_{decimal, hex, whitespace_trimmed,
  absent_or_garbage_is_default}`.
- **Noise** — `deterministic_and_in_range`, `varies_across_space_salt_and_seed`,
  `fbm_is_spatially_smooth`, `tiled_fields_repeat_bit_exactly_at_the_seam`,
  `tiled_fields_repeat_within_float_error`, `a_warped_tiled_field_still_tiles`,
  `an_untiled_field_does_not_repeat`, `shaping_folds_about_the_midpoint`,
  `worley_has_near_and_far_samples`.

## Sharp edges

- **Origin sampling, not center.** `is_solid(x,y,z)` classifies the voxel at its
  *minimum corner*. For `FlatField::at_half()` (height 128), voxel `y = 127` is
  solid and `y = 128` is empty. The dual vertex `(0.5,0.5,0.5)` then lands at the
  cell center for inactive cells.
- **`HeightField` contours to two different surfaces depending on the path.**
  Used standalone as a `Primitive`, it samples `height_at` (nearest integer
  column) with an integer-grid gradient normal. Used inside a `Scene` (as an
  `Sdf`), it samples `height_bilinear` with a central-difference normal (step
  `SDF_EPS = 0.5`). The bilinear path is deliberately kept off the procedural
  fallback so a `Scene`'s ~16M inside-tests stay fast. Both are legitimate; just
  don't expect byte-identical geometry between the two.
- **`Scene::world()` moves only the terrain, not the shapes.** The analytic
  gallery sits at fixed world coordinates; only the heightmap cache follows the
  `offset`. To contour into a non-origin cluster use `world_at(cluster_offset)`,
  or the terrain will be cached at the wrong footprint. `world()` == `world_at([0,0,0])`.
- **Height band is [96, 160]**, i.e. `BASE_HEIGHT 128 ± AMPLITUDE 32` — modest on
  purpose, to stay clear of dual-contouring's steep-slope "tangential cell" blind
  spot. (Some source comments still cite the pre-halving `[64,192]` / `AMPLITUDE 64`
  — see Findings; the live numbers are 128 ± 32.)
- **`FLICKER_SEED` is snapshotted once.** The `world_height` default field is
  built on first call and cached for the process; changing the env var afterward
  has no effect. Use `world_height_seeded` when you need an explicit, pure seed.
- **`edge_hermite` trusts its caller.** It assumes the two endpoints differ in
  solidity; on a non-crossing edge it returns a degenerate crossing rather than
  erroring. The contour only ever calls it on real crossings.
