# Handoff — camera walking on procedurally generated terrain

> Standalone handoff for a fresh session. Re-verify the anchors below (line numbers
> drift). Builds on `docs/architecture.md` (the source-of-truth invariant + crate
> layering + mesh/nav generation) and `docs/voxel-mesh-worker-handoff.md` (the W1–W4
> worker slices). This document supersedes prior assumptions where they conflict.

## Destination

**A camera that walks on procedurally generated terrain** — gravity/collision against
a navmesh, on a world that bakes itself outward from spawn. The voxel/LOD/worker
substrate is built and working; what's missing is (a) a world bigger than the demo's
fixed patch and (b) locomotion that actually consumes the nav.

## Verified current state (checked against the repo this session)

- **Branch `main`. Uncommitted:** `examples/voxel-cluster/src/main.rs`,
  `examples/voxel-cluster/scripts/hud.lua`, `docs/voxel-mesh-worker-handoff.md` (the W4a
  locomotion-gate work + handoff edits). **Commit these first.** Everything else is
  committed (`2c8b440 dynamic LOD mesh off thread`, `8710259 Primitives separation`).
- **Tests green:** ~170 pass, 0 fail — `flicker-voxel` 119, `flicker-primitive` 27,
  `flicker-worker` 4, `clayengine` 1, plus others. **fmt/clippy:** pre-existing drift
  and warnings exist in older files (`contour.rs`, `bake.rs`, `primitive.rs`, the
  virtual-voxel viz) — *not* from recent work; recently-touched files are clean. A blanket
  `cargo fmt` would tidy but creates unrelated noise.
- **Worker slices (`docs/voxel-mesh-worker-handoff.md`):** W1 (renderer slot recycling /
  leak fix), W2 (`flicker-worker` generic pool), W3 (async mesh: `submit_field_jobs` /
  `drain_and_apply`), **W4a (locomotion nav gate)** — all **DONE and runtime-confirmed**
  (the user verified W3: no hitch on LOD swap, triangle count plateaus). **W4b
  (streaming)** is deferred and unstarted.
- **`examples/voxel-cluster`:** a **static 3×3 field** (`FIELD_DIM = 3`). Clusters are
  generated in `ensure_source()` via `Scene::world_at(offset)` → `contour()`; it
  `try_load_bake_field` first (reads `bake/cluster_*.json.gz` if present) **else contours
  at runtime**. The async pipeline re-meshes the whole field on LOD change via worker
  jobs (`build_cluster` is the pure per-cluster unit). Render LOD is **clamped to 7**
  (`derive_lod` at LOD8 expands the footprint over the whole cluster — pathological).
- **Nav** is generated in `build_cluster` **only in surface-walk mode** (the
  `"surface_walk"` HUD checkbox; default off = fly = no nav, no collision). It comes from
  `ClusterNav::compute_nav(cluster, neighbors)` — the **dense state field, not the mesh**.
- **No walking/collision exists.** Camera is fly-only (WASD + R/F + right-drag look).
  `surface_walk` only *generates* nav for inspection; nothing consumes it.

### Corrections to assumptions (verify, don't trust prior wording)

- The seeded wave field is **`flicker_primitive::heightmap::world_height_seeded(x, z,
  seed)`** (and `world_height(x, z)` using `DEFAULT_SEED`). There is **no
  `heightmap_terrain_at`**. The terrain primitive is `HeightField::from_default_seed(offset)`;
  `Scene::world_at` wraps it **and also adds demo shapes** (sphere/cube/cylinder/dome) —
  the terrain generator should use the bare `HeightField`/`world_height_seeded`, not the
  cluttered demo `Scene`. Fixed `DEFAULT_SEED` is what gives free cross-cluster seam
  continuity.
- **Runtime bake-writing does NOT exist.** `BakedCluster::from_cluster` +
  `to_disk_bytes` + `fs::write` exist only in `run_bake_mode` (the `--bake` CLI). The
  runtime path reads-or-contours but never writes. Objective 2 (write-on-first-encounter)
  must wire the existing write capability into the runtime generate path.
- **The LOD8 single vector is not stored anywhere.** The bake is LOD-0 only. The "instant
  backdrop from each cluster's stored LOD8 vector" (objective 4) requires building that
  storage; today there's nothing to read instantly, and `derive_lod(_, LOD8)` is the
  pathological-expansion path. (This is the long-parked "default LOD8 vector" gap.)

## Invariants — do not violate

1. **JSON-is-truth / no runtime regeneration.** The LOD-0 cluster file *is* the data
   (layer 2). The seed/heightmap is layer-1 throwaway input. Generate a cluster **once**
   on first encounter, write its JSON, then load/edit that file forever after. Never
   re-generate or re-contour at runtime. (`docs/architecture.md`.)
2. **±1 LOD adjacency is a hard panic.** `crates/flicker-voxel/src/mesh.rs:~412` asserts
   `|neighbor_lod - self_lod| <= 1` and panics otherwise (the cross-LOD seam is strictly
   2:1). You may **never** hand a cluster a neighbor more than one LOD away. This is the
   central constraint on the LOD scheduler and on the LOD8 backdrop.
3. **Nav from the state field, not the mesh.** `compute_nav` reads dense solidity; it must
   stay independent of meshing.
4. **Render-time stride.** Coarse LODs derive from LOD-0 stored data (`derive_lod`); the
   mesher is LOD-agnostic. No re-contour to change stride.

## The objectives (decided this session)

1. **Reset the 9 static clusters** → a dynamic generator that bakes clusters outward from
   spawn on the worker pool, using `world_height_seeded` (terrain-only).
2. **Bake-on-first-encounter; the JSON is the source of truth.** Generate once → write
   JSON → thereafter load/edit. Seed determinism matters only for that first bake.
3. **Bounded horizon for the test bed.** The true horizon (LOD8 begins) is 65,280 ft ≈ a
   **510-cluster radius ≈ ~817k clusters ≈ ~1 TB / multi-hour** — *do not fill to it*.
   Pick a small bounded radius (low hundreds of clusters at most), fill outward to that
   bound, then stop. No streaming/eviction (that's W4b).
4. **LOD8-first is a separate backdrop layer, not in-place refinement.** Because of the ±1
   panic you cannot draw all-LOD8 and refine near clusters in place against LOD8
   neighbors. Instead: an **all-LOD8 backdrop** (uniform, instant from each cluster's
   *stored* LOD8 vector — see correction above; nonexistent today) with a **ring-balanced
   refined field drawn over it**. The scheduler assigns ring-appropriate LODs across the
   active set and **must never give a LOD0 cluster a LOD8 neighbor** — pass the
   ring-appropriate neighbor LOD, or `None` at the backdrop boundary. Cold first run: no
   stored LOD8 yet, so the world fills visibly from empty; the instant backdrop only
   applies on warm runs once files exist.
5. **Physics off the navmesh.** Nav is locomotion-gated to the near rings, from the state
   field. Gameplay only needs the **spawn neighborhood** complete; the rest of the bounded
   horizon keeps baking in the background without blocking. This is what makes the slow
   fill harmless.
6. **Application-state FSM.** Add **Startup → MainMenu (single Start button) → Loading →
   Playing** in `flicker-app` (today the `App` trait is just init/update/render — no FSM),
   gating which subsystems `update()` ticks. **Loading → Playing** fires once the spawn
   cluster + immediate neighbors are meshed at target LOD and nav is generated around the
   player; physics + LOD resolution engage then.
7. **Fill-statistics HUD.** Live counters off the worker pool / `drain_and_apply`: cluster
   count, resident memory, bake + mesh timing, on-disk bytes — the readout that validates
   the per-cluster efficiency story as the world grows.
8. **The core missing capability: walking/collision movement against the navmesh.** W4a
   generates nav in `surface_walk` mode but nothing consumes it. This is the capability
   that delivers the destination.

## Recommended sequence (shortest path to "camera walks on terrain" first)

The destination capability is **walking** (obj 8), and physics-off-nav (obj 5) means
**walking only needs the spawn neighborhood** — which the current static 3×3 already is
(procedurally generated, with nav available in walk mode). So the fastest milestone does
**not** require the dynamic generator, LOD8 backdrop, or bounded-horizon fill:

- **Phase A — first walking milestone (small, high-value):**
  1. **App FSM (obj 6)** — Startup → MainMenu → Loading → Playing in `flicker-app`, with
     Loading→Playing gated on "spawn neighborhood meshed + nav ready." Forces
     `surface_walk` on in Playing.
  2. **Walking/collision locomotion (obj 8)** — a walk locomotion mode that consumes the
     nav (or the dense state field directly) to keep the camera on the surface: gravity +
     ground-clamp/step, WASD in the surface plane. Build it on the existing spawn
     neighborhood. **This is the destination capability — do it before the world-scale
     work.**
  - Milestone: camera walks on the existing generated patch.
- **Phase B — make it a world:**
  3. **Dynamic terrain generator (obj 1–3)** — replace the fixed 3×3 with outward
     bake-from-spawn (terrain-only `world_height_seeded`), **write-on-first-encounter**
     (wire `BakedCluster::to_disk_bytes` into the runtime path), bounded radius (low
     hundreds), background fill that never blocks Playing.
  4. **LOD8 backdrop + ring scheduler (obj 4)** — first build the **stored LOD8 vector**
     (single vector per cluster; also resolves the `derive_lod`-at-LOD8 expansion cost,
     e.g. via the snap-on-read mesh tweak noted in the worker handoff), then the backdrop
     layer + a scheduler that assigns ring-appropriate LODs respecting the ±1 rule (None at
     the backdrop seam).
  5. **Fill-statistics HUD (obj 7).**

Rationale: Phase A proves the destination on a tiny world; Phase B scales it. Phase B's
hardest dependency is the ±1 scheduler + the not-yet-existing LOD8 vector — both isolated
from walking, so they don't block the milestone.

## Confirm first (unverified / in flight)

- **Commit the uncommitted W4a working tree** before starting.
- The static 3×3 currently re-meshes the *whole* field on any LOD change and each job
  re-derives its 4 neighbors — fine at 9 clusters, but the generator (Phase B) wants
  changed∪neighbors-only and shared derived clusters (noted in the worker handoff).
- `Scene::world_at` bundles demo primitives; decide the terrain-only generation entry
  (likely bare `HeightField` / `world_height_seeded`) before wiring the generator.

## Pinned — parked, do not pursue (record only)

Having the generator emit **sparse coarse-LOD data directly from the wave function**
(e.g. a far cluster as a single LOD8 vector instead of 8M voxels) would make a deep
horizon tractable — but it **breaks the layer-2 invariant** (LOD-0 *is* the data) and adds
a generate/derive step into the mesh path. **Parked.** It's the lever that makes the true
65,280-ft horizon feasible and there are other reasons to revisit it later; record it,
don't build it now.
