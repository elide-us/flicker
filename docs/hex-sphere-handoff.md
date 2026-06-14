# Handoff — Hex-Sphere World Topology (`flicker-worldgrid`)

**Status:** Spec, decisions locked. **Slices 1–3 built** (`flicker-worldgrid`
pentagon patch + epoch chain + the full icosahedral sphere) plus the
`examples/hex-sphere` test client. Next: Slice 3b (equal-area ISEA projection)
or Slice 4 (ledger `CellId ↔ CellCoord`).
**Audience:** Claude Code (implementation), Elideus (architecture review).
**Supersedes:** the `examples/hex-map` bent-rings / σ-zipper / two-flat-map
topology *and* the polar-cap defect-concentration sketch. Both are **abandoned**
(left in place, not deleted, not reused).
**Builds on:** `docs/material-model-handoff.md` (the per-cell data model the grid
feeds), `docs/clayengine_world_generation_spec_v2.md` (the epoch pipeline),
`docs/flicker-world-system-spec.md`.

---

## 1. Decisions (locked)

1. **Grid:** an **ISEA** (Icosahedral Snyder Equal-Area) hex tiling at a
   **single fixed resolution**. Twelve mandatory defects are **pentagons on the
   twelve icosahedron vertices** (Goldberg-class connectivity, ISEA positions).
2. **Sharding:** the **20 equal triangular icosahedron faces**. The 12 defects
   land on the **vertices where 5 faces meet** — i.e. on shard *corners*, which
   are exactly the seam-reconciliation zones.
3. **Crate:** a new **`flicker-worldgrid`** subcrate that produces **topology
   only** and feeds the *existing* `EpochCtx`. It does **not** own heightmaps,
   storage, erosion, or rendering.
4. **Durable-claims tier:** **deferred** (it's storage-lifecycle, not topology).
5. **`examples/hex-map`:** **abandoned for now** — not a dependency, not reused
   as a crate. Its flat within-a-hex math (`geom.rs`) may be referenced/copied,
   nothing else.

---

## 2. Why (geometry + system reasons, compressed)

- **The defect is unavoidable.** Gauss–Bonnet fixes 720° of integrated curvature
  on any hex-sphere → exactly **12 pentagons** at every resolution. You can only
  place them. **Spreading (12 × 60°) beats concentrating** (polar cap = 360°/pole
  → the near-pole ring tears to absorb it). Twelve shallow dents, not two craters.
- **Why equal-area (ISEA), not gnomonic Goldberg/H3.** The ledger stores
  **absolute element amounts**, not densities (`material-model-handoff` §1/§9). If
  cell areas varied ~2× (gnomonic), equal amounts would no longer mean equal
  concentration and every epoch + erosion pass would have to area-weight.
  Equal-area deletes that bug class. Its cost — *shape* distortion near the
  pentagons — lands inside the defect zones we already quarantine (§7).
- **Why single fixed resolution.** The planet is one hex level; we don't need
  H3's aperture-7 hierarchy. The octree LOD fold is **intra-hex** (powers of two,
  voxel→texel→hex) and never interacts with the **inter-hex** planet grid, so the
  "lossless fold" property (`material-model` storage chain) is safe regardless of
  the grid projection.
- **Why 20-face sharding.** Equal area → even parallel load; and it puts the
  pentagons on shard corners, making "defects = seam corners" literally true.
  (12 + 20 truncated-icosa shards would be wildly unequal in size.)

---

## 3. The seam into the existing sim (load-bearing — read this)

The epoch pipeline is **already topology-agnostic**. It consumes:

```rust
// crates/flicker-worldgen/src/pipeline.rs
pub struct EpochCtx<'a> {
    pub tables: &'a Tables,
    pub dirs: &'a [Vec3],          // per-cell unit position on the sphere
    pub neighbors: &'a [Vec<u32>], // adjacency — VARIABLE length per cell
    pub seed: u64,
}
```

- Every epoch walks `for &nb in &ctx.neighbors[i]` — it **never assumes 6**.
- A **pentagon is therefore already first-class**: a cell whose neighbor vec has
  length 5. (This closes the old "open decision #5" — no special-casing needed at
  the sim layer.)
- The only real topology today is the toy `ring(n)` in tests
  (`pipeline.rs`). **`flicker-worldgrid` is what replaces it for the real
  planet.** Dependency direction: **`flicker-worldgen` → `flicker-worldgrid`**.

So the crate's whole job is: produce `dirs` + `neighbors` (plus the per-cell
metadata in §4) and hand them to `EpochCtx`. Nothing in the sim changes.

---

## 4. The grid model

- **Base:** icosahedron — 20 faces, 12 vertices, 30 edges.
- **Combinatorics vs geometry are separable.** The *adjacency graph* (who
  neighbors whom, where the 12 pentagons are) is independent of the *projection*.
  Build the adjacency first; ISEA only sets vertex **positions** and **areas**.
  Slice 1 may use a cheap projection for positions and swap in true ISEA later
  without touching adjacency.
- **Per-cell outputs** (one row per hex):
  - `dir: Vec3` — unit-sphere position (this is `EpochCtx.dirs`).
  - `neighbors: Vec<u32>` — 6 normally, **5 for the 12 pentagons**.
  - `area: f32` — ≈ equal under ISEA (track the variance as a test bound).
  - `is_pentagon: bool`.
  - `shard: u8` — which of the 20 triangular faces (corner cells belong to one
    canonically; the 5-face sharing is recorded in adjacency).
  - `key: u64` — within-shard space-filling-curve order (Hilbert/Morton — §8).
  - `id: CellId` — stable global id (§4.1).
- **Resolution knob:** the triangular sub-grid frequency per face sets hex count
  and hex size. Choose the frequency so hex ≈ **49.65 mi** (the "50-mile
  segment"; `material-model` §4), or the nearest the construction allows.

### 4.1 Cell identity

- The sim runs on **dense `0..n` indices** (as it does today) — fast, cache-local.
- **Persistence** uses a stable global **`CellId`** (e.g. a `u64` packing
  `face | in-face (i,j) | pentagon-flag`).
- `flicker-worldgrid` owns the **index ↔ CellId ↔ neighbor** mappings.
- **Ledger integration (decision at Slice 4):** the ledger is keyed by
  `CellCoord { x: i32, z: i32 }` (`crates/flicker-worldstate/src/ledger.rs`),
  which is flat-2D and can't hold an icosahedral address. Either redefine the
  ledger key to `CellId`, or add a `CellId ↔ CellCoord` mapping. The ledger's
  persistence format is explicitly undecided, so this is a clean seam.

---

## 5. Scale (carried from the design doc; downstream of topology)

Resolution chain (intra-hex octree fold is exact, powers of two):

| Unit | Size | Derivation |
|------|------|------------|
| Voxel | 6 in | base |
| Texel | 128 ft | 256 voxels × 6 in |
| Hex | 49.65 mi | 2048 texels × 128 ft |

Heightmap ≈ **8 MiB/hex** (16-bit). Reference small planet Mercury ≈ **100 GiB**
heightmap floor. **`flicker-worldgrid` owns none of this** — it's listed only so
the two scaling axes in §6 are explicit.

---

## 6. Test posture for the dev box (the 1/8 constraint, done right)

The test box is an A18 "Mac Neo" — 6 cores + a Metal GPU, RAM-limited (see memory
`dev-box-profile`). So **the GPU is not the constraint** (rendering the grid is
trivial; a wgpu viewer is welcome); the ceilings are **RAM and CPU**, and they
only bite at the **heightmap / materialization layer**, not the topology layer.

The icosahedron has **no 4-fold axis** (its symmetry is 5/3/2-fold), so a
geometric octant ("x>0,y>0,z>0") slices pentagons and faces arbitrarily and may
contain **zero complete defects**. Don't cut geographically. Cut by topology:

- **Primary test patch:** an **N-ring disc centered on one pentagon vertex**. It
  contains the pentagon, its 5 hex neighbors, the 5 seams radiating from the
  vertex, and the full 5-fold neighborhood — the **minimal complete hard case**.
- **Secondary patch:** a small disc in a **hex-cluster interior** — regular grid
  plus one ordinary inter-shard seam (the "boring" case).

**Two independent scaling axes — never pay both at once on the test box:**

1. **Cell count / topology** — controlled by ring count N.
2. **Per-cell heightmap resolution** — 2048² = 8 MiB/hex; even 1/8 of Mercury is
   GiB-scale before any sim.

Topology + epoch bring-up → **many cells × tiny/no heightmaps**. Storage +
materialization bring-up (later) → **few cells × full 2048²**.

---

## 7. Defect treatment (downstream; recorded so the grid leaves room)

The 12 pentagons become intentional content; each vertex authored as one of:
**anomaly zone** (expose the curvature as navigation weirdness), **core warp
gate** (sink the cell, repurpose as teleport), or **unsurvivable peak** (raise it
above playable altitude). All keep players off the singular point. Useful
corollary for the grid: because pentagons are never normal playable terrain,
their **heightmap parameterization (the old "open decision #4") is relaxed** — a
pentagon cell needn't carry a standard hex heightmap. This is deferred, not part
of `flicker-worldgrid`.

---

## 8. Slice plan

- **Slice 1 — topology kernel (no heightmaps, no storage, no render). ✅** Built
  `crates/flicker-worldgrid`: `pentagon_patch(rings)` subdivides the five-face
  apex cap and dualises it → `Patch { dirs, neighbors, area, is_pentagon,
  interior, ring, center }`. The pentagon falls out as the lone interior degree-5
  cell. Tests assert exactly one pentagon at the centre (degree 5), interior
  hexes degree 6, symmetric in-range adjacency, unit-length dirs, positive
  roughly-uniform areas.
- **Slice 2 — sim on real topology. ✅** `flicker-worldgrid` is now a dev-dep of
  `flicker-worldgen`; `tests/epoch_on_pentagon_patch.rs` feeds a `pentagon_patch`
  straight into `EpochCtx` and runs `six_epoch_stack` **unmodified**. Asserts the
  six layers thread through, every epoch did its work (so the neighbour-driven
  epochs 3 & 6 handled the 5-neighbour cell), no field goes non-finite, the
  pentagon stays an ordinary cell, and `planet-continuity-goal` holds: Epoch-1 Fe
  differs across neighbours ≈ 0.54× the global spread (vs ≈ 1.0 for noise).
- **Slice 3 — full icosahedral sphere + sharding. ✅** `icosphere(freq)` (in
  `sphere.rs`; shared subdivision/dual extracted to `mesh.rs`) builds the whole
  grid: all 20 faces → one closed dual → `Sphere { dirs, neighbors, area,
  is_pentagon, shard, id, freq }`. Shard = the canonical icosahedron face (lowest
  index for a shared cell); `CellId = (face << 48) | morton(i, j)`; cells emitted
  in `(shard, Morton)` order. Tests: **exactly 12 pentagons** (rest hexes), cell
  count `10·freq²+2`, Euler V−E+F = 2, symmetric adjacency, all 20 shards
  populated, unique ids in scan order, total area ≈ 4π, hex-area spread bounded.
- **Test client — `examples/hex-sphere`. ✅** Headless (no GPU dependency,
  CI-friendly): prints the verification report and writes a per-shard-coloured PLY
  (pentagons red) to open in any 3-D viewer. `cargo run -p hex-sphere -- [freq]
  [out.ply]`. Uses `icosphere_with_outlines` (adds per-cell corner polygons via
  `mesh::cell_corners`). Confirmed at freq 16: 2562 cells, 12 pentagons, Euler 2,
  area 12.551 ≈ 4π, hex spread 1.75×. (Headless by choice for a quick check — a
  Metal/wgpu viewer is fine on the A18; see §6.)
- **Slice 3b — equal-area ISEA projection.** Replace the cheap normalised-
  barycentric positions with the Snyder equal-area icosahedral map so the hex-area
  spread collapses toward 1.0 (the reason ISEA was chosen — comparable absolute
  amounts in the ledger). Isolated because it's substantial, orthogonal math: it
  moves points only, leaving the Slice-3 graph/shards/ids untouched. Also pin the
  sub-grid frequency that hits ≈ 49.65 mi.
- **Slice 4 — ledger integration.** Resolve `CellId ↔ CellCoord` (§4.1) with
  `flicker-worldstate`.
- **Deferred (not this crate):** pentagon heightmap parameterization, the §7
  defect treatments, the durable-claims storage tier, navigation/heading frames
  (the holonomy at the 12 defects).

---

## 9. Reuse / abandon

- **Reuse:** the `EpochCtx` seam (§3); `flicker-materials` / `flicker-worldstate`
  / `flicker-worldgen` (the sim is built — Epochs 1–4 run with real physics, 5–6
  scaffolded); the flat within-hex math in `examples/hex-map/src/geom.rs` as a
  *reference* to copy from.
- **Abandon:** `examples/hex-map` `topology.rs` (σ-zipper), `gadget.rs`,
  `map_structure.rs`, `snap_map.rs`, `snap_segment.rs`, spiral ordering, the
  two-map record-flip visualization. Left in the tree, no longer the path.
- **Test client:** `examples/hex-sphere` replaces `hex-map` as the grid's
  troubleshooting tool — headless PLY export, not a wgpu app (the weak box). A
  GPU viewer that colours cells by an *epoch field* (elevation / biome — the "see
  the continuous planet" payoff) is the natural follow-on; it'd add `flicker-
  worldgen` + `flicker-materials` deps to the example and reuse the same outlines.

---

## 10. Open decisions still live (smaller now)

1. **Within-shard space-filling curve:** Morton chosen for Slice 3 (simple, good
   locality); revisit Hilbert if scan locality ever matters more.
2. **`CellId` bit layout:** Slice 3 ships `(face << 48) | morton(i,j)`; the final
   layout is pinned at Slice 4 when it meets the ledger key.
3. **ISEA construction variant / sub-grid frequency:** the equal-area
   parameterization and the per-face frequency that hits ≈ 49.65 mi — now Slice 3b
   (the cheap projection ships in Slice 3; adjacency doesn't depend on it).
4. **Epoch count:** the design doc says 9; `flicker-worldgen` has 6 formation
   epochs and memory has 7/8/9 = GM/underground/surface runtime layers. Reconcile
   when 7–9 are spec'd; **out of scope** for topology.

---

## 11. Verify

`cargo build/test -p flicker-worldgrid`. Topology asserts (neighbor counts, Euler
check, area variance, symmetric adjacency); the epoch harness runs end-to-end on
the pentagon patch including the 5-neighbor cell.

---

## 12. Decided vs deferred

**Decided (load-bearing):** ISEA equal-area, single fixed resolution; 12
pentagons on the 12 icosa vertices; 20 equal triangular shards with defects on the
shared corners; `flicker-worldgrid` produces topology only and feeds `EpochCtx`;
pentagons are first-class via the variable-length `neighbors` vec; adjacency is
separable from the ISEA projection; test by pentagon-centered disc, not geographic
octant; two independent test-scaling axes.

**Deferred / TBD:** §10 items, plus pentagon heightmaps, defect treatments,
durable-claims tier, navigation/holonomy frames, and the full epoch 7–9 layer
reconciliation.
