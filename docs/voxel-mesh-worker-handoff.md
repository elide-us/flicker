# Handoff — async worker pool for mesh (and nav) generation

> **This is a design handoff, not a ready-to-run plan.** A fresh session should
> re-verify the anchors (line numbers drift) and steer the open decisions before
> building the threading. The data-model invariants it builds on are in
> `docs/architecture.md` ("Voxel data model", "Mesh & navigation generation").

## Goal

Move the *derived* work — `derive_lod` + `mesh` (and later nav) — **off the main
thread onto a worker pool**, so a LOD swap or a streaming load no longer blocks a
frame. Render-time stride already made a swap *cheap* (no re-contour); this makes it
**hitch-free**. Jobs are **best-effort and restartable**: a request that's been
superseded (newer LOD for that cluster, or an edit) is discarded on completion, not
awaited.

## Where things stand (verified)

- **Single-threaded app loop.** `flicker-app` (`runner.rs:207` `run<A: App>`) drives a
  winit event loop. `App` (`lib.rs:39`): `init` / `update(dt, input, &Renderer)` /
  `render(&mut Renderer)` / `should_quit`. No threads, no tokio anywhere in
  `flicker-app` or `flicker-voxel` today — **this slice introduces threading.**
- **The hitch** is `examples/voxel-cluster` `rebuild()` (~`main.rs:545`): on
  `needs_rebuild` (set in `update()` when the LOD field changes), `render()` runs
  `derive_lod` + `flicker_voxel::mesh` + `renderer.upload_mesh` for all 9 clusters,
  synchronously, on the render thread.
- **`derive_lod` + `mesh` are pure CPU** (flicker-voxel/flicker-primitive, no wgpu) →
  safe off-thread. **`upload_mesh` must stay on the render thread** (owns the device).
- **`Cluster` is auto-`Send`+`Sync`** (no interior mutability), so the LOD-0
  source-of-truth `ClusterMap` shares to workers as `Arc<ClusterMap>`, read-only.
- **Renderer mesh storage is still append-only** (`renderer.rs:62`
  `meshes: Vec<LoadedMesh>`; `upload_mesh` ~`:225` only pushes; `render` ~`:418`).
  The leak from handoff #1 was never fixed; worker results need recycled slots to
  upload into. **This is the prerequisite (W1 below).**

## Design

```
                 main thread                         worker pool (N threads)
  Arc<ClusterMap>  (LOD-0 source of truth, read-only) ──────────────┐
        │                                                           │
  update(): LOD field / streaming / edits change →                 ▼
        bump per-cluster generation, enqueue Job ──jobs chan──▶  derive_lod(self + neighbors)
                                                                  → NeighborContext → mesh
  render(): drain Results, for each whose gen == current:  ◀─results chan── ClusterMesh
        renderer.free_mesh(old); h = renderer.upload_mesh(cm); swap handle
        (stale gen → drop the result)
```

- **Job** = `{ cluster grid pos, self LOD, the 4 neighbor LODs, generation }`. The
  neighbor LODs are needed because the mesh's seam geometry depends on them (a job is
  re-issued when self *or* a neighbor LOD changes).
- **Worker** derives the coarse cluster for self **and** each neighbor from the shared
  `Arc<ClusterMap>` LOD-0 source (`derive_lod`), builds the `NeighborContext`, runs
  `mesh`, returns the CPU `ClusterMesh`. Self-contained: a worker reads only the
  shared read-only source and its job.
- **Best-effort / restart** = a `generation: u64` per cluster grid position. The main
  thread bumps it on each new request and enqueues the job tagged with it. A result is
  applied only if its generation still matches the cluster's current desired
  generation; otherwise it's dropped. In-flight jobs are never cancelled mid-run — they
  finish and their result is discarded (cheap; `derive_lod`+`mesh` is fast).
- **Upload stays on the render thread**: workers produce `ClusterMesh` (CPU positions +
  indices); the main thread frees the old slot and uploads the new one into a recycled
  slot (W1).

## Slices

- **W1 — Renderer mesh-slot recycling. ✅ DONE.** `flicker-render`: `meshes:
  Vec<Option<LoadedMesh>>` + `free_mesh_slots` free-index list; `upload_mesh` reuses a
  free slot or appends; `Renderer::free_mesh(handle)` (drop → wgpu deferred-destroy, no
  fence); `MeshPipeline::render` skips `None`. Example `rebuild()` frees old handles via
  `meshes.drain(..)` before re-uploading. The append-only leak is gone. Also scaffolded:
  the example's source is now `Arc<RwLock<ClusterMap>>` (write-lock to populate, read-lock
  to derive) — ready for workers to clone-and-read.
- **W2 — Generic worker pool. ✅ DONE.** New `flicker-worker` crate: `WorkerPool` runs
  `Box<dyn FnOnce() + Send>` jobs on a fixed thread set (shared `Arc<Mutex<Receiver>>`
  queue), `with_default_size()` = `available_parallelism − 1`, `Drop` closes the queue
  and joins. Pure std, no deps; 4 tests, clippy/fmt clean. Jobs are self-contained
  closures that capture their inputs and route their own outputs — so the **mesh-job
  specifics (derive→mesh, `Arc<RwLock>` source, generation, result channel) live at the
  call site in W3**, not in the pool.
- **W3 — Wire the example. ✅ DONE (pending runtime confirmation).** The synchronous
  `rebuild()` is gone, replaced by `ensure_source` (populate LOD-0 once) +
  `submit_field_jobs` (bump generation, submit one `build_cluster` job per cell) +
  `drain_and_apply` (collect current-generation results, apply the full field as a set:
  free old slots, upload into recycled ones, rebuild draw data; drop stale). The pure
  free fn `build_cluster(source, x, z, lod_field, camera, gen)` is the worker unit —
  derives self+neighbours from the `Arc<RwLock>` source, meshes, and bundles
  mesh+pick+nav+arrows. `self.map` removed (inspector reads source; bounding boxes from
  grid). Builds + clippy + fmt clean. **Not runtime-verified** (GUI) — fly with
  camera-LOD on to confirm no hitch + plateau.
  - Notes for later: re-meshes the *whole* field on any LOD change (swap-on-complete),
    and each job re-derives its 4 neighbours — both fine at 9 clusters, both worth
    optimising at scale (changed∪neighbours only; share derived clusters across jobs).
    corner-arrows are now source-LOD-0 (a debug-viz simplification).
- **W4 — Streaming + nav (later).** Enqueue jobs as the player *moves* (load near,
  evict far), not only on LOD change. Generate **NavMesh dynamically around the player
  — gated by locomotion mode**: in **fly mode (the only mode today) no nav is generated
  and no collision is produced**; a UI toggle enables a surface-walking mode for
  nav/collision testing. (See `docs/architecture.md` "Mesh & navigation generation".)

## Open decisions

1. **Where the pool lives. ✅ RESOLVED → a new `flicker-worker` crate**, designed as a
   *generic* worker service (there will be many calculating tasks beyond meshing — nav,
   physics, etc.). The mesh job is one task type submitted to it, not the crate's whole
   purpose. Keep the job/result API task-agnostic (e.g. a trait or a closure-job).
3. **Source sharing under future edits. ✅ RESOLVED → `Arc<RwLock<ClusterMap>>`**, already
   scaffolded in the example (W1). Workers clone the `Arc` and take the read lock to
   derive; edits take the write lock.
2. **Channels / threading primitives.** `std::sync::mpsc` (simplest) vs. `crossbeam`
   (multi-consumer work-stealing, cleaner for a pool). Start with the simplest that gives
   one-producer/many-consumer for jobs and many-producer/one-consumer for results.
4. **Worker count / granularity.** One mesh job per cluster (worker derives self + 4
   neighbors) is the natural unit; pool size ~= cores − render/main. Confirm.

## Conformance note

`ORDER-1` (the Memory & Resource spec's lock-free atomic-ordering / x86→ARM
weak-ordering audit) becomes **live** the moment threads land. Using `std`/`crossbeam`
channels + `Arc` (no hand-rolled atomics or lock-free queues) sidesteps most of it for
the first cut — but any `Ordering::Relaxed`/`Acquire`/`Release` introduced later needs
the audit.

## Verification

- W1: `cargo build`/test green; example buffer/triangle count **plateaus** when flying
  (recycled slots), no longer climbs.
- W2: headless unit tests — a job through the pool returns a `ClusterMesh` byte-equal to
  the synchronous `derive_lod`+`mesh` path; stale-generation results are dropped.
- W3: fly with camera-LOD on — **no hitch** on LOD swaps; cluster count/triangles stable.
