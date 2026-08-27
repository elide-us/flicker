# flicker-worker

A small, generic **worker pool**: a fixed set of background OS threads that run
self-contained units of work off the main/render thread, so long calculations never block a
frame. It is the lowest-level threading primitive in the `core` cluster — pure `std`, zero
dependencies. A *job* is just a closure (`FnOnce() + Send`) that **captures its own inputs
and routes its own output**; the pool only *runs* jobs, it never names a task's input or
result type. One pool therefore serves every kind of off-thread work — voxel mesh
derivation, texture baking, world-sim cell sweeps — without a central job/result enum that
would grow with each new caller.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits
- **Builds on:** nothing — pure `std` (`std::thread`, `std::sync::mpsc`, `Arc`, `Mutex`).
- **Used by:**
  - [`flicker-pocclusters`](../../scenes/flicker-pocclusters/) — voxel LOD mesh derivation
    (the first and reference user): one job per cluster derives + meshes off-thread, tagged
    with a generation so superseded results are dropped on arrival.
  - [`flicker-sablework`](../../scenes/flicker-sablework/) — off-thread texture bakes.
  - [`flicker-poc-chemistry`](../../world/flicker-poc-chemistry/) — the world-sim scheduler's
    per-cell sweep (chunked jobs, blocks on a completion barrier so a tick stays synchronous).
- **Reads from the content tree:** none. This crate is below the content/Model/signal layer.

## The pattern (a job captures inputs and routes its own result)

The pool hands you no results — a job produces output by doing something the closure itself
captured (typically sending on a channel). This is the whole contract:

```rust
use std::sync::{Arc, mpsc};
use flicker_worker::WorkerPool;

let pool = WorkerPool::with_default_size();
let (tx, rx) = mpsc::channel();

let source = Arc::new(load_source());     // read-only inputs, shared to every job
for id in 0..job_count {
    let source = Arc::clone(&source);     // each job captures its own inputs …
    let tx = tx.clone();                  // … and its own result channel
    pool.submit(move || {
        let result = expensive(&source, id);  // pure CPU work, off the main thread
        let _ = tx.send((id, result));        // route the output — routing is the caller's job
    });
}
drop(tx); // drop the original sender so the drain below ends once every job's clone is gone

// On the thread that owns the results (e.g. the render thread), drain and apply:
for (id, result) in rx.iter() {
    apply(id, result);
}
```

Any *best-effort / discard-if-superseded* logic (the voxel path bumps a `generation` per
cluster and ignores results whose generation no longer matches) lives entirely in the caller
— the pool has no notion of it.

## Public API

`WorkerPool` is the only public item.

| Item | What it is for | The one thing to know |
|---|---|---|
| `WorkerPool` | Owns the worker threads and the shared job queue. | Dropping it shuts the pool down cleanly (see Drop). |
| `WorkerPool::new(threads: usize) -> Self` | Create a pool with an explicit worker count. | `threads` is clamped to **at least 1** (`0` → `1`). |
| `WorkerPool::with_default_size() -> Self` | Create a pool sized to the machine. | `max(1, available_parallelism − 1)` — leaves one core for the main/render thread. |
| `WorkerPool::submit<F>(&self, job: F)` where `F: FnOnce() + Send + 'static` | Enqueue one job to run on whichever worker is free. | Returns nothing. **No ordering** between jobs. A no-op if the pool is shutting down or all workers have died (see Sharp edges). |
| `WorkerPool::thread_count(&self) -> usize` | Number of worker threads **spawned**. | Spawned, not live — it does not drop after a worker dies (see Sharp edges #1). |
| `impl Drop for WorkerPool` | Shut down. | Closes the queue, then **joins** every worker — so it blocks until all in-flight jobs finish. |

## Threading model
- A fixed set of OS threads is spawned in `new` / `with_default_size` and lives until the
  pool is dropped.
- One shared **FIFO queue** (`std::sync::mpsc`) behind a `Mutex`. Each worker locks the queue
  only long enough to pull one job, then **releases the lock before running it** — so the work
  itself never serialises on the queue lock and jobs run concurrently.
- No work-stealing, no priorities, no per-job cancellation, no async runtime. A submitted job
  always runs to completion (it is never interrupted mid-run); "cancellation" is done by the
  caller discarding the *result* (the generation pattern above).

## Interactions
- **Signals / Model / content:** none — this is a threading primitive below those layers.
- **What it hands other crates:** background threads that execute the caller's closures. All
  input-sharing (`Arc<…>` / `Arc<RwLock<…>>` to read-only source data) and all result routing
  (a channel the closure captured) are set up by the caller, not the pool.
- **Threads / async:** the whole crate is threads — see Threading model.

## Gates
Run with `cargo test -p flicker-worker` (pure `std`, so also buildable standalone at edition
2021). Four tests:

| Test | What it locks down |
|---|---|
| `runs_every_submitted_job` | Every one of 200 submitted jobs actually runs and its result arrives. |
| `new_clamps_to_at_least_one_thread` | `new(0)` yields 1 worker; `new(3)` yields 3. |
| `default_size_has_at_least_one_thread` | `with_default_size()` never produces a zero-thread pool. |
| `drop_joins_without_hanging` | `Drop` closes the queue and joins idle workers without deadlocking. |

## Sharp edges
1. **A panicking job silently and permanently shrinks the pool.** The worker loop runs a job
   directly; if it panics, the panic unwinds and that worker thread is gone for good — the
   pool never re-spawns it. `thread_count()` still reports the original count, so it
   over-reports live capacity after a panic; if *every* worker has died, `submit` becomes a
   silent no-op (its send error is ignored). The default panic hook prints the panic to
   stderr, but the lost capacity and the stale count are silent. Keep jobs panic-free, or
   have the closure catch its own errors. (See finding #1.)
2. **No result handle.** `submit` returns `()`. If a job produces nothing observable and
   captures no channel, its work is invisible — there is no join handle or future to await.
3. **No ordering.** Jobs run on whichever worker is free; do not assume submission order.
4. **No backpressure.** The queue is unbounded — a caller that submits far faster than the
   workers drain grows memory without limit. Throttle at the call site if that is possible.
5. **`Drop` blocks.** It joins every worker, so teardown waits for all in-flight jobs to
   finish. A job that blocks forever will hang `Drop` forever.
6. **Jobs are `FnOnce() + Send + 'static`** — they must own everything they touch (share
   read-only inputs via `Arc`, never a borrow of caller-stack data).
