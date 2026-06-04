# Handoff — LOD mesh residency cache + swap hysteresis (ring-scheduler, slice 2)

> **This is a session handoff, not a ready-to-execute plan.** It captures the findings and
> design direction from a planning conversation. A fresh session should re-verify the code
> anchors (line numbers will have drifted) and resolve the open questions at the end before
> implementing.

## Goal

Make LOD swapping behave well: **keep the right LOD meshes resident and refresh the rest** —
not "free everything aggressively," not "keep everything forever." Add **hysteresis** so LOD
doesn't thrash at distance boundaries. This is the residency-manager piece of the ring scheduler.

**Explicitly OUT of this slice:**
- **Async worker threads** (the piece that removes the synchronous re-contour *hitch*) — the next
  slice. `ORDER-1` (lock-free atomic orderings, the x86→ARM weak-ordering audit) only becomes
  live when that lands.
- **Motion prediction** — lands later with the physics engine.
- **Render-time-stride mesh refactor** (see "Deeper direction" below) — long-term, not now.

## Where things stand (already working, in-tree)

- Camera-driven per-cluster LOD on the 9-cluster field: `target_lod_for_cluster` (log2 distance
  policy, `LOD_BASE_DISTANCE = 128`), a `smooth_lod_field` ±1-adjacency pass (the mesher panics if
  4-adjacent clusters differ by >1 LOD), driven from `update()`, applied via `rebuild()`. In
  `examples/voxel-cluster/src/main.rs`.
- Nav-surface **LOD billboards** (restored world-space billboard pipeline in `flicker-render`:
  `pipeline_billboard.rs`, `shaders/billboard.wgsl`, `Renderer::draw_billboard`, per-texture
  billboard bind group). Digit sits on the navmesh surface at each cluster centre.
- `flicker-voxel` tests green (142). The mesher's read surface is unified (`FieldReader`).

**The two problems this slice targets**, both observed live:
1. Triangle/buffer count climbs forever as you fly — `Renderer.upload_mesh` pushes a new
   `LoadedMesh` into an **append-only** `meshes: Vec<LoadedMesh>` and nothing is ever freed; each
   swap leaks 9 clusters × 3 buffers (vertex/tri-index/edge-index).
2. Every LOD-field change triggers a **whole-field synchronous re-contour** (`rebuild()` makes a
   fresh `ClusterMap` and re-contours all 9) → a multi-hundred-ms-to-second hitch per swap.

## Key findings that drive the design

### A. The expensive work is LOD- and neighbor-independent (the pivotal insight)
`contour()` (`contour.rs`) builds the dense state field by calling `primitive.is_solid(...)` for
**all 256³ voxels** and writing per-voxel state — and that work is **the same at every LOD and
oblivious to neighbors** (it's ~16M evaluations, the dominant cost). LOD only changes the lighter
per-cell QEF/corner data. **Seam geometry** (the only neighbor-LOD-dependent part) lives entirely
in `mesh()`, not `contour()`.

Consequences for the cache:
- Cache the **costly, stable** thing — the contoured `Cluster` (state field) — keyed by
  `ClusterId` (LOD included). A re-request of the same `(cluster, LOD)` is a cache hit that skips
  re-contour. *Caveat:* today `contour()` bakes LOD-specific corner data into the `Cluster`, so a
  cached `Cluster` is genuinely per-`(cluster, LOD)`. That's fine for oscillation hits.
- The **GPU mesh is a cheap derived layer** and is specific to `(self-LOD + the 4 neighbor LODs)`.
  It goes stale the moment a neighbor swaps LOD. So do **not** cache GPU meshes keyed only by
  `(cluster, LOD)` — re-emit them from the cached contour against current neighbor LODs when self
  or any neighbor LOD changes.

### B. Eviction is in-flight-safe for free (conformance)
Submit model (`renderer.rs` ~428): single `queue.submit` + `frame.present()`, no fences, no
frames-in-flight counter, `desired_maximum_frame_latency: 2`. wgpu uses **deferred buffer
destruction** — dropping a `wgpu::Buffer` only frees GPU memory after the GPU finishes reading it.
So **evicting/freeing a mesh needs no explicit fence and no UMA/discrete branching**: `AUTH-2` /
`AUTH-3` / `PRIN-1` are satisfied by wgpu's model. The existing grow-on-demand pattern
(`next_power_of_two`) in `pipeline_lines`/`pipeline_sprite`/`pipeline_mesh` per-draw buffers is the
reuse template.

### C. Data-model ground truth — corrections to assumptions
The working assumption was "this should already be in ClusterMap / cluster LOD / the default LOD8
vector; extend the existing model." Verified reality:
- **`ClusterMap` (`cluster_map.rs`) is a bare `HashMap<ClusterId, Cluster>`** — `insert`/`get`/
  `iter`, **no remove, no eviction, no residency/LRU/retention bookkeeping**. Its own doc says
  "Later steps will reintroduce dirty tracking, JIT mesh management, worker-pool population." →
  **This is the right place to add the residency/cache bookkeeping** ("expand the existing data
  model").
- **`ClusterId` (`cluster_id.rs`) packs LOD as a first-class key field** → the map can already hold
  *multiple LODs of the same cluster simultaneously* as distinct keys. Reuse this as the cache key;
  no new key type needed.
- **The "default LOD8 vector" is NOT in the code.** `Cluster` (`cluster.rs`) holds only
  `state: StateField`, `surface_overrides`, `default_material`. The LOD8 single-vector / instant-draw
  field is a *spec invariant* (nav spec §0) that **has not been implemented**. If the cache wants a
  draw-during-contour fallback, it must be added — or use the coarsest currently-cached LOD as the
  stand-in. **This gap is the thing most likely to surprise the implementer.**
- The example's `rebuild()` builds a **fresh `ClusterMap::new()` every time** (`main.rs` ~546) and
  re-inserts — it does **not** use the map as a persistent residency cache. To get caching, stop
  rebuilding the map wholesale; query/populate a persistent residency `ClusterMap` instead.

## Recommended design

1. **Promote `ClusterMap` into a bounded LOD-residency cache.** Keyed by `ClusterId` (LOD incl.);
   holds contoured `Cluster`s; per-entry retention metadata (last-used frame/tick; optionally
   distance/ring). A budget; a `get_or_contour`-style accessor; evict lowest-value entries when over
   budget (dropping a `Cluster` is CPU-side-cheap; its GPU mesh is freed via #2 + wgpu deferral).
2. **Mesh-slot recycling in the renderer.** Replace append-only `meshes: Vec<LoadedMesh>` with a
   free-list (`Vec<Option<LoadedMesh>>` + free indices) and add `Renderer::free_mesh(handle)`;
   `upload_mesh` reuses a free slot or appends. Fixes the leak; eviction returns slots. (`AUTH-1`
   single owner; `AUTH-2/3` via drop+deferred-free; `PRIN-1` no device branching.)
3. **Hysteresis (±15%) on LOD selection.** A cluster at LOD `L` swaps to `L+1` only when distance
   exceeds the `L→L+1` boundary by ×1.15, and to `L-1` only when it drops below the `L-1→L`
   boundary by ×0.85 (dead-band). Requires per-cluster **current-LOD** state (add to the residency
   bookkeeping). Run the existing ±1-adjacency `smooth_lod_field` *after* hysteresis. The cache makes
   any residual mis-swap cheap (swap-back = hit).
4. **Eviction value (no motion prediction):** recency + distance/ring + **keep-adjacent-LOD** (retain
   the LOD just left and `L±1`, the probable next states under the band). Evict lowest when over
   budget.
5. **Incremental GPU re-emit on swap:** only re-mesh clusters whose self-LOD *or* a neighbor-LOD
   changed (neighbors included because of the seam dependency), pulling the contour from the cache
   (hit ⇒ no re-contour), re-uploading into recycled slots.

### Deeper direction (note, not this slice)
Because the state field is LOD-independent (finding A), the "right" long-term fix is **render-time
stride**: contour each cluster's state field *once*, mesh at any LOD on demand. That makes swaps
need no re-contour at all and largely obviates a multi-LOD contour cache. It's the deferred
mesh-pipeline refactor ("when stride becomes a render-time parameter, the bake satisfies every
rebuild"). A fresh session should consciously weigh "cache per-(cluster,LOD) contours" vs. "pursue
render-time stride" — they're somewhat alternative.

## Conformance checklist (Memory & Resource Architecture spec)
- `PRIN-1`: no `uma/unified/integrated/discrete/device_type` branches (grep stays clean).
- `AUTH-1`: renderer is sole owner of GPU buffers; `ClusterMap` sole owner of contoured clusters.
- `AUTH-2/3`: eviction/free via drop → wgpu deferred destruction; **no fences, no device-type gating.**
- `UPLOAD-1`: stay on `write_buffer`/`create_buffer`; no `MAPPABLE_PRIMARY`/UMA-direct-write assumption.
- `INTENT`: cluster/LOD meshes are the spec's `Stream` intent — but the full §2/§3 intent abstraction
  is **not** required for this slice; don't build it here.
- `ORDER-1`: N/A until the async slice adds threads.

## Key files (anchors will have drifted — re-grep)
- `crates/flicker-voxel/src/cluster_map.rs` — extend into the residency cache.
- `crates/flicker-voxel/src/cluster_id.rs` — LOD-keyed; reuse as cache key.
- `crates/flicker-voxel/src/cluster.rs` — `Cluster` fields; **LOD8 vector absent**.
- `crates/flicker-voxel/src/contour.rs` — 256³ state-field build is LOD/neighbor-independent (the cache target).
- `crates/flicker-voxel/src/mesh.rs` — seam geometry is the neighbor-LOD-dependent layer (`per_face_stride`).
- `crates/flicker-render/src/renderer.rs` — `meshes` append-only leak (`upload_mesh`); submit/present (no fences); `load_texture`/`upload_mesh`.
- `crates/flicker-render/src/{pipeline_lines,pipeline_sprite,pipeline_mesh}.rs` — grow-on-demand reuse template.
- `examples/voxel-cluster/src/main.rs` — `rebuild()` (wholesale rebuild), `target_lod_for_cluster`, `smooth_lod_field`, `lod_field`, `needs_rebuild`.

## Open questions for the new session
1. **Budget metric:** entry-count (simple) vs memory (truer, needs per-mesh byte accounting).
2. **Where current-LOD + retention metadata live:** extend `ClusterMap` entries, or a parallel
   residency table keyed by cluster grid position.
3. **LOD8 instant-draw vector:** implement it now (draw coarse while finer contours) or use
   coarsest-cached-LOD as a placeholder? (It's not in the data model today.)
4. **Cache contours per-(cluster,LOD) vs. pursue render-time stride** (finding A / Deeper direction).

## Verification (for the implementing session)
- `cargo build`/`clippy`/`fmt`; `cargo test -p flicker-voxel` stays green (142).
- Visual: fly around — buffer/triangle count should **plateau** (cached LODs reused), not climb;
  swapping back to a recently-used LOD should be **instant** (cache hit, no hitch); **no thrash** at
  boundaries (hysteresis); **no panic** (±1 smoothing). Add a HUD readout of residency/cache size to
  observe it directly.
