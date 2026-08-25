# flicker-voxel

The voxel substrate: sparse per-cluster storage, dual-contour surfacing, render-time
level-of-detail, a walkable-surface (nav) derivation, and the durable on-disk **bake**
format. It is **pure data + compute** — no graphics, no `wgpu`, no `winit`, no threads, no
runtime file I/O. A caller hands it a shape to surface (a `Primitive`), gets back a
`Cluster`, and from that one cluster derives everything else — coarser LODs, a renderable
mesh, a nav grid, a compressed file on disk — without ever touching the source shape again.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## The one contract to internalize: LOD-0 *is* the data

Everything in this crate hangs off a single rule (the project's load-bearing voxel
invariant, held in MCP). Get this and the rest of the API falls into place:

1. **Contour input — throwaway.** The `Primitive` you feed `contour()` exists only to
   *derive* a cluster's vector data, then is discarded.
2. **Cluster vector data — the source of truth.** The dense state field + sparse corner
   vectors held in a `Cluster`, persisted at **LOD 0** as a `BakedCluster` file. This is
   what you save, load, and edit. *Edits mutate this — never a mesh.*
3. **Mesh / nav / coarse-LOD views — ephemeral.** Everything `mesh()`, `derive_lod()`, and
   `ClusterNav::compute_nav()` produce is *derived* from layer 2 and is disposable: lose it
   and re-derive. This is "the cache."

The load-bearing consequence for a caller: **coarser LODs are *derived* from LOD-0 data,
never re-contoured.** `contour()` is a bake/edit-time step; `derive_lod()` is the run-time
LOD step. See [Two routes to a coarse mesh](#two-routes-to-a-coarse-mesh--pick-derive_lod)
for the one place the API lets you break this rule.

## Vocabulary (flicker terms used below)

- **Voxel** — a 6-inch cube of space. Carries a *state* (is there matter?), a *corner
  vector*, and a *material*.
- **Cluster** — a 256×256×256 voxel volume (128 ft on a side). The unit of storage,
  addressing, meshing, and baking.
- **State** — one of `Empty` / `Solid` / `Viscous` / `Flowing`. Stored *densely* (2 bits ×
  256³ = 4 MB fixed per cluster). This is the "is this matter?" oracle; it drives geometry
  and the (map-layer) water cycle.
- **Corner vector** — a per-voxel 3-byte offset that places this cell's dual-contour surface
  vertex. Stored *sparsely* — only at voxels that actually carry surface information.
- **Material** — a packed 32-bit identity (primary/secondary catalog id + blend). Drives
  shading; ignored by geometry.
- **Contour** — surface a `Primitive` into a `Cluster` (dual contouring + QEF vertex solve).
  Bake/edit-time only.
- **LOD / stride** — level of detail. LOD `L` reads every `stride = 2^L`-th voxel. Range
  `0..=8`; LOD 0 is full resolution, LOD 8 is a single vector for the whole cluster.
- **Seam** — the shared boundary between two adjacent clusters. Meshed watertight by reading
  the neighbor's stored vertices directly (no halo copies) through a `NeighborContext`.
- **Bake** — the compressed LOD-0 cluster file (gzipped JSON). The durable artifact.

## Where it sits

- **Builds on:**
  - `clayengine` — world-defining constants only (`CLUSTER_DIM = 256`, `VOXEL_COUNT`,
    `MAX_LOD = log2(256) = 8`, `FEET_PER_VOXEL = 0.5`). Re-exports `CLUSTER_DIM` and
    `VOXEL_COUNT`.
  - `flicker-primitive` — the stampable shapes fed to `contour()` (`Sphere`, `Cube`,
    `HeightField`, `FlatField`, …) and the procedural `heightmap` module. **Re-exported**
    whole (see [Re-exports](#re-exports)), so callers depend only on `flicker-voxel`.
  - `flicker-core` — the gzip helpers (`compress_gzip` / `is_gzipped` /
    `decompress_gzip`) the bake format is built on.
- **Used by:**
  - `flicker-pocclusters` (the cluster scene) — the primary consumer: contours the 3×3
    island field, holds it in a `ClusterMap`, and drives per-cluster `derive_lod` → `mesh`
    and `compute_nav`. It runs those derives **off the main thread** via `flicker-worker`;
    this crate itself is thread-agnostic (pure functions), so the worker pool lives there,
    not here.
- **Writes to the content tree:** the `bake_island` binary (below) writes nine LOD-0 bakes
  to `Alpha/content/package/bakes_island/cluster_{x}_0_{z}.json.gz`; `flicker-pocclusters`
  reads them at startup. The crate does no other file I/O.

## The pipeline

```
Primitive ──contour(prim, material, id)──▶ Cluster  ◀── the LOD-0 source of truth
                                             │
   BakedCluster::from_cluster(id, cluster)   │   derive_lod(&cluster, lod)   ── coarse view
   → to_disk_bytes() ──▶ *.json.gz  ◀────────┤       (never re-contour)
   from_bytes() ──▶ BakedCluster ────────────┘
                                             │
                          mesh(&cluster, &neighbors, lod) ──▶ ClusterMesh   (renderable)
                          ClusterNav::compute_nav(&cluster, &neighbors) ──▶ ClusterNav (walk)
```

## Public API

### Cluster storage

| Item | What it is for | The one thing to know |
|---|---|---|
| `Cluster` | The 256³ volume. Dense state + sparse `(corner, material)` overrides. | Reads (`get`) are infallible; **intentionally non-`Clone`** (it is large — copy via `derive_lod(_, Lod::ZERO)` if you must). |
| `Cluster::empty()` | All-`Empty`, no overrides, `default_material = EMPTY`. | The usual starting point. |
| `Cluster::uniform(base)` | Every voxel `= base.state()`; `base.material()` becomes bulk-fill. | `base.corner()` is **ignored** — corners are sparse-only. |
| `Cluster::get(coord) -> Voxel` | State from the dense field; corner+material from the sparse override or defaults. | Never fails; a solid voxel with no override reports `default_material` + default corner. |
| `Cluster::set(coord, voxel)` | Write a voxel. | Stores a sparse override **only** if it differs from the state-appropriate default; a default write *removes* the entry (stays sparse). |
| `Cluster::set_state(coord, state)` | Fast path: flip only the dense state bit. | Does **not** touch any existing `(corner, material)` override at `coord`. |
| `Cluster::default_material()` / `set_default_material(m)` | The bulk-fill material for solid voxels without an override. | Contour sets it to the primitive's material; bake load restores it. |
| `Cluster::override_count()` | Count of sparse surface overrides (not solid-voxel count). | A uniform solid cluster has 0. |
| `Cluster::is_uniform()` | `true` iff no surface overrides (state may still be non-empty). | |
| `Cluster::overrides()` | Iterator of `(LocalCoord, Voxel)` over surface overrides only. | Order is unspecified (`HashMap`). |
| `Cluster::state_field()` / `replace_state_field(f)` | Borrow / wholesale-replace the dense `StateField`. | `replace_*` is the bake-loader path; use `set` for normal mutation. |
| `Voxel` | The value `get` returns: `(state, corner, material)`. | `Voxel::EMPTY` / `default()` is the "no voxel" value. Fields private; read via `state()`/`corner()`/`material()`. |
| `VoxelState` | `Empty` / `Solid` / `Viscous` / `Flowing`. | `is_filled()` = "renderer treats as matter" (everything but `Empty`); `from_bits`/`to_bits` are the 2-bit codec. |
| `StateField`, `STATE_FIELD_WORDS` | The dense 4 MB (`524_288 × u64`) state store. | `as_words`/`from_words` are byte-exact for bake + GPU upload; z-major linear index. |
| `CornerVector`, `CornerVector::DEFAULT` | Per-axis byte offset over `[-0.5, 1.5]`; `DEFAULT = (0.5,0.5,0.5)` = cell center. | Equality is on the **bytes**, not decoded floats (lossy, ≈1/255). Out-of-range clamps; NaN/±Inf fall back to default. |
| `Material`, `Material::EMPTY` | 32-bit packed identity: `primary`/`secondary`/`blend` (u8 each), bit 31 = direct-RGB escape. | `primary`/`secondary` are ids into the 256-slot catalog (`Alpha/content/data/materials.json`); id 0 = Air = `EMPTY`. `new` never sets the escape bit; `from_raw` round-trips anything. |
| `LocalCoord` | A validated `(x,y,z)` in `0..256`. | `new()` returns `None` out of range — the in-bounds guarantee every cluster read relies on. |
| `CLUSTER_DIM` (=256), `VOXEL_COUNT` | Re-exported from `clayengine`. | World-defining constants; do not hardcode 256. |

### Addressing

| Item | What it is for | The one thing to know |
|---|---|---|
| `ClusterId` | Packed `u32` = `[LOD:4][x:10][y:8][z:10]`, addressing which cluster at which LOD. | Ranges: x `0..=1023`, y `0..=255`, z `0..=1023`. **LOD field is 0..=15 but only 0..=8 are usable** — see finding #1. |
| `ClusterId::new(lod,x,y,z)` / `bits()` / `from_bits(u32)` | Construct / pack / unpack. | `new` panics if a component overflows its field; `from_bits` does **not** validate. |
| `ClusterId::lod/x/y/z()` | Field accessors. | |
| `ClusterId::world_offset() -> [f32;3]` | The cluster's `(0,0,0)` corner in world voxel units (`field × CLUSTER_DIM`). | Same at every LOD — LOD changes the sample stride *within* a cluster, not where it sits. |
| `ClusterId::MAX_LOD` / `MAX_X` / `MAX_Y` / `MAX_Z` | Bit-field ceilings (15 / 1023 / 255 / 1023). | `MAX_LOD = 15` is the **field width, not the usable max LOD** (8). Misleading — see finding #1. |
| `ClusterMap` | `HashMap<ClusterId, Cluster>` residency for a set of clusters. | `new`/`insert`/`get`/`iter`/`len`/`is_empty`. No remove (add when needed); `insert` moves the cluster in (non-`Clone`). |
| `Lod`, `Lod::ZERO`, `Lod::MAX` | The sample stride at which a cluster is read. | `Lod::MAX` = 8 = the real ceiling. `new(level)` returns `None` for `>8`. |
| `Lod::level/stride/sample_dim/cell_dim()` | `stride = 2^level`; `sample_dim = 256 >> level`; `cell_dim = sample_dim − 1`. | Sample sets nest: LOD `L+1` positions are a strict subset of LOD `L` — what keeps geometry consistent across levels. |

### Contour → derive → mesh → nav

| Item | What it is for | The one thing to know |
|---|---|---|
| `contour(&dyn Primitive, Material, ClusterId) -> Cluster` | **Bake/edit-time** surfacing: sample the primitive at world coords, dual-contour to QEF vertices, write dense state + sparse corners. | Queries the primitive at `world_offset + i·stride`, so adjacent clusters agree at the seam for free. **Panics** if the id's LOD > 8. Runtime LOD swaps use `derive_lod`, not this. |
| `derive_lod(&Cluster, Lod) -> Cluster` | **Run-time** coarse-LOD view of a LOD-0 cluster — *without* re-contouring. | State copied verbatim; each coarse cell's corner is the average of the LOD-0 corners in its `stride³` footprint, re-encoded. `Lod::ZERO` returns an independent copy. No primitive, no QEF. |
| `mesh(&Cluster, &NeighborContext, Lod) -> ClusterMesh` | Per-cell dual-contour into a flat-shaded CPU mesh; welds byte-equal vertices. | **Panics** if any neighbor's LOD differs from `lod` by >1 (2:1 max transition). A `None` neighbor face = open world edge (see Sharp edges). Cluster edge/corner (2–3 axis) seams are a deferred gap. |
| `ClusterMesh` | `{ vertices: Vec<ClusterVertex>, indices: Vec<u32> }`. | `is_empty`, `triangle_count`, `weld` (averages normals — smooth), `edge_use_histogram` (diagnostic: 2 = interior, 1 = boundary, >2 = bug). |
| `ClusterVertex` | `{ position: [f32;3], normal: [f32;3], material: u32 }`, cluster-local voxel units. | Field-for-field convertible to `flicker-render`'s `MeshVertex`; the *consumer* does the mapping (this crate stays graphics-free). |
| `NeighborContext` | The 6 face neighbors, each `Option<(&Cluster, Lod)>`, for seam-closure reads. | `None` = world boundary. `none()` / `default()` = all-`None`. Only the 6 *faces* are modeled — not the 12 edge / 8 corner neighbors. |
| `FaceDir` | `NegX/PosX/NegY/PosY/NegZ/PosZ`. | Used by the nav cross-seam queries. |
| `ClusterNav` | LOD2 walkable surface: a 64×64 grid of per-column top floor indices. | Derived, never baked. Built from the **dense state field**, never the mesh (a mesh can dapple/gap over a floor). |
| `ClusterNav::compute_nav(&Cluster, &NeighborContext)` | Derive the walkable grid. | Pure/deterministic. Records only the **topmost** floor per column (caves deferred). A floor needs ≥ 6 ft (3 cells) clearance. |
| `ClusterNav::floor_at(x,z)` / `linked(a,b)` / `floor_across(...)` / `linked_across(...)` | Read the floor / test walkable link (≤ 2-cell rise) within a cluster or across a seam. | `linked_across`/`floor_across` read the neighbor's edge column through `NeighborContext` at LOD2. |
| `cluster_center_world(ClusterId) -> [f32;3]` | World-space center of a cluster. | The point the ring gate measures camera distance to. |
| `in_nav_rings(camera, center) -> bool` | Ring 0–2 eligibility gate: is this cluster within 1792 ft of the camera? | A pure distance test, **not** a scheduler — decides *whether* to nav, not load order. |
| `NAV_DIM` (=64), `NAV_RING_OUTER_FT` (=1792.0) | Nav grid dimension and ring-2 outer boundary. | Derived from the locked LOD2 row + ring formula. |

### Bake / persistence

| Item | What it is for | The one thing to know |
|---|---|---|
| `BakedCluster` | A `Cluster` + `id` + `horizon_voxel`, ready to round-trip through the file format. | `id`, `cluster`, `horizon_voxel` are public fields. |
| `BakedCluster::from_cluster(id, cluster)` | Wrap a freshly contoured cluster (scans for the horizon voxel). | |
| `to_disk_bytes()` / `from_bytes(&[u8])` | **The canonical on-disk path**: compact JSON → gzip, and back. | `from_bytes` sniffs the gzip magic and also accepts raw JSON. Round-trip is byte-exact. |
| `to_json()` / `to_json_pretty()` / `from_json(&str)` | The JSON layer under the gzip envelope. | `pretty` sorts voxels `(z,y,x)` for stable diffs. |
| `find_horizon_voxel(&Cluster) -> Option<LocalCoord>` | The "dot on the horizon" pick for extreme-LOD rendering. | Rule 1: topmost non-default corner in column `(0,0)`; Rule 2: override nearest center; Rule 3: `None`. Free function so you can recompute after an edit without re-baking. |
| `BakeError` | Load/save failure enum. | Distinct variants for version / cluster-id / state-word-count / voxel-pos / gzip / UTF-8 / JSON — all **loud** (`from_*` returns `Err`, never a silent partial load). |
| `BAKE_VERSION` (=3) | Format version stamped in every file. | Loader **rejects** a mismatch (`UnsupportedVersion`) — no silent migration. Bump on every breaking schema change. |

### Generators (prototype)

`pub mod generators` — `solid_slab`, `heightmap_terrain_at`,
`heightmap_terrain_at_with_depth_materials`, and the `demo_materials` id constants. These are
**prototype builders** for spinning up a cluster in a line of test/example code; the module
doc flags them as `HashMap`-bound slow (up to ~8 M inserts). They are **not wired into the
shipped scene** — `flicker-pocclusters` live-contours `HeightField::island` instead. Kept as
tools, not dead code.

### Re-exports

From `flicker-primitive` (so a caller depends only on `flicker-voxel`): the `Primitive`
trait, `Hermite`, `Sdf`, `Scene`, and the shapes `Sphere`, `HalfSphere`, `Cube`, `Cone`,
`Cylinder`, `HalfCylinder`, `FlatField`, `HeightField`, plus the `heightmap` module
(`world_height_seeded`, `island_height`, `DEFAULT_SEED`, …). From `clayengine`:
`CLUSTER_DIM`, `VOXEL_COUNT`.

## Binary: `bake_island`

```
cargo run -p flicker-voxel --bin bake_island
```

Headless (no GPU/window). Contours the 3×3 Prism Test Room field from the procedural island
dome (`HeightField::island` / `heightmap::island_height`) and writes nine LOD-0 bakes to
`Alpha/content/package/bakes_island/cluster_{x}_0_{z}.json.gz`, which `flicker-pocclusters`
loads at startup. Each cell samples the *same* global island function at its own world
offset, so cross-cluster seams are continuous for free.

## Interactions

**None.** This is a pure-data/compute crate: no input signals, no Model keys, no rendering,
no threads, no async, no runtime file reads. Off-thread derivation and GPU upload are the
consumer's job (`flicker-pocclusters` + `flicker-worker` + `flicker-render`).

## Two routes to a coarse mesh — pick `derive_lod`

The API offers two ways to get a coarse-LOD mesh, and only one honors the source-of-truth
contract:

- **Right (run-time):** `contour` **once** at LOD 0 → `derive_lod(&lod0, Lod::L)` →
  `mesh(&coarse, &neighbors, Lod::L)`.
- **Wrong at run time (bake/test-only):** `contour(prim, mat, ClusterId::new(L, …))` →
  `mesh(&c, &neighbors, Lod::L)`. This re-evaluates the primitive per LOD — the exact
  "re-contour-per-LOD" the invariant forbids at run time. It is retained deliberately as the
  cross-check oracle for `derive_lod` (see the `derive_lod` gate below), **not** as a
  run-time path.

Nothing in the types stops you from the wrong route; the rule is convention. See finding #3.

## Gates

The contracts, by test name (`cargo test -p flicker-voxel` — 117 unit + 2 integration, all
green):

- **Source-of-truth / LOD derivation** (`derive_lod`): `derived_state_matches_source_everywhere`,
  `derived_corner_lands_on_surface_plane`, `uniform_source_derives_no_corners`, and the key
  oracle `derived_mesh_matches_recontoured_mesh_triangle_count` — a `derive_lod` LOD-2 mesh
  has the same triangle count as a mesh contoured directly at LOD 2.
- **Contour** (`contour`): `active_cell_with_empty_min_corner_gets_a_vertex` (the crack/spike
  regression), `seam_shell_cell_stored_on_max_voxel_for_flat_field`,
  `lod0_byte_identical_to_pre_lod_path`, `lod2_active_cell_corner_byte_equal_across_footprint`,
  `world_offset_shifts_primitive_sampling`.
- **Meshing seams** (`mesh`): `cluster_pair_x_seam_emits_crossing_only_on_low_side`,
  `cluster_pair_x_seam_byte_equal_world_positions`, `row_of_three_clusters_along_x_seam_closed`
  (watertightness: every interior edge used by exactly two triangles), plus the cross-LOD
  seam-count tests.
- **Bake round-trip** (`bake`): `round_trip_flat_field_cluster_is_byte_equal`,
  `round_trip_through_disk_bytes_is_byte_equal`, `from_bytes_accepts_uncompressed_json`,
  `state_field_round_trips_through_hex`, `surface_cells_ordering_is_deterministic_zyx`,
  `reject_unsupported_version`, `reject_out_of_range_voxel_pos`, and the horizon-rule tests.
- **Nav** (`nav`): `flat_floor_reports_index_and_empty_column_is_none`,
  `clearance_boundary_three_walks_two_does_not`, `slope_gate_links_at_delta_two_breaks_at_three`,
  `cross_cluster_seam_links_via_neighbor_context`,
  `cluster_top_clearance_treats_absent_vertical_neighbor_as_empty`, `compute_nav_is_deterministic`,
  `ring_gate_admits_near_and_rejects_far`, `nav_constants_match_locked_lod2_and_ring_table`.
- **Encoding invariants**: `ClusterId` `bit_layout_isolation` + overflow panics; `Lod`
  `lod_dim_relationships_at_every_level`; `Material` `bit_layout_matches_spec`; `CornerVector`
  `round_trip_precision_across_full_range`; `StateField` `state_field_is_4_mb`.
- **Island bake pipeline** (integration, `tests/island_bake.rs`):
  `every_island_cell_contours_and_round_trips`, `the_island_is_a_dome_in_the_expected_band`.

## Sharp edges

- **`ClusterId` LOD field accepts more than is usable.** The field is 4 bits (`0..=15`) but
  only `0..=8` are valid LODs; `ClusterId::MAX_LOD` (15) is the *field width*, not the usable
  max (that is `Lod::MAX` = 8). `new`/`from_bits`/bake-load accept LOD 9–15 without
  complaint; the mismatch surfaces later as a **panic** inside `contour`
  (`Lod::new(id.lod()).expect(...)`). See finding #1.
- **`contour` and `mesh` panic, by design, on out-of-contract input** — `contour` on a LOD>8
  id; `mesh` on a neighbor whose LOD differs by >1 (world-gen above must honor the 2:1
  transition rule). These are loud, intentional guards, not soft failures.
- **A `None` neighbor face is silently an open seam.** `mesh`/`nav` cannot distinguish "no
  neighbor here (true world edge)" from "you forgot to wire the neighbor" — both leave the
  seam open. If a cluster that *has* a live neighbor is meshed with that face `None`, you get
  a silent hole, not an error. Populate `NeighborContext` from your residency map.
- **Cluster edge/corner seams are a known, deferred gap.** A seam quad reaching a cluster
  edge (2 axes OOB) or corner (3 axes OOB) is dropped — `NeighborContext` models only the 6
  face neighbors. Symptom: a small gap at cluster corners (the tolerated slack in the
  cross-LOD seam tests). Nav is unaffected (it reads the state field, not the mesh).
- **`Cluster` is non-`Clone` and large** (≥ 4 MB state alone). Move it (via `ClusterMap`) or
  copy through `derive_lod(_, Lod::ZERO)`; there is no `.clone()`.
- **`CornerVector` equality is byte-wise, not float-wise** — two float inputs that encode to
  the same byte are equal; a decode→encode is lossy to ≈ 1/255.
- **Corners decode only at the stride they were encoded for.** A corner is stored
  cell-relative (`voxel + corner·stride`); reading LOD-0 corners at a coarse stride without
  going through `derive_lod` yields wrong positions. Use `derive_lod`.
