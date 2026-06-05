# flicker architecture

## Overview

flicker is a 2D game engine packaged as a Cargo workspace of single-responsibility
crates. Game content lives outside this workspace; the engine is generic and the
games that depend on it are opinionated. This document is a stub — fill in real
content as the architecture solidifies.

## Voxel data model — the source-of-truth invariant

> **Invariant.** The LOD-0 compressed cluster file *is the data.* It is the single
> source of truth for everything a cluster contains. Every live, cached, or rendered
> artifact is **derived in memory** from that data. We only ever *read* and *mutate*
> the cluster's stored vector data; we never regenerate it at runtime.

Three layers, and what may touch each:

1. **Contour input — throwaway.** The primitive or edit shape fed to `contour()`. It
   exists only to *derive* a cluster's vector data, then is discarded. Stamping a
   sphere into the world contours it, writes the derived vectors into the cluster, and
   throws the input away.
2. **Cluster vector data — the source of truth.** The QEF corner vectors + dense state
   field held in `Cluster` and persisted as the LOD-0 cluster file (gzipped JSON
   today). This is what we save, load, and *edit*. Player edits mutate **this**, never
   a mesh.
3. **Mesh output — ephemeral.** GPU/CPU meshes derived from layer 2 for rendering.
   Hot/cold cached, recycled on a lazy GC. Disposable: lose one and we re-derive it
   from layer 2. *This* is what "the cache" means.

**Contouring is a bake/edit-time activity only** — it is how layer-1 input becomes
layer-2 data, and it must never run at render time. In particular, **rendering a
coarser LOD does not re-contour.** The *same* unified, LOD-agnostic mesher produces
every LOD: it self-strides the cluster's stored vector data through the field reader
(this cluster's corner + the neighbors' corners across each seam) and emits quads —
identical logic at every stride, a mechanism already exercised and tested. The mesh
**output path does not change** between LODs; what changes is only the **input
source** — corners are read from the stored cluster data, never from a re-contoured
primitive.

> Wiring detail for the render-time-stride slice: `CornerVector` is stored
> *cell-relative* (per-axis byte over `[-0.5, 1.5]`) and decoded as
> `voxel + corner·stride`, so a corner only decodes correctly at the stride it was
> encoded for. Sourcing a coarser LOD's corners from finer stored data is therefore a
> small per-cell derive (pick/aggregate a representative + re-encode cell-relative to
> the coarse cell), *or* store corners stride-independently so the read is a literal
> passthrough. Either way it is a fraction of the cost of re-contouring (no 256³
> primitive sweep, no QEF re-solve) — the representation choice is settled in that
> slice.

Re-contouring to change LOD stride is wasted work: the data is already in the cluster.
*That is the data.*

> History: an early multi-cluster/LOD path re-contoured per LOD because `contour()`
> baked LOD-specific corners into the cluster and the LOD-0 file couldn't satisfy a
> coarser request. That is a **defect to remove** (render-time stride), not a pattern
> to cache around. See `docs/voxel-lod-cache-handoff.md`.

## Voxel crate layering

The voxel world is its own crate stack, lowest to highest:

- **`clayengine`** — the foundation. World-defining "magic numbers" only
  (`CLUSTER_DIM`, `VOXEL_COUNT`, `MAX_LOD`, `FEET_PER_VOXEL`); depends on nothing.
  `MAX_LOD` is *derived* from `CLUSTER_DIM` (`log2`) so the two can't drift.
- **`flicker-primitive`** — stampable shapes (the editor's CSG sources and the
  world-gen input) plus the procedural `heightmap`. Depends only on `clayengine`,
  **never on storage**.
- **`flicker-voxel`** — cluster storage, contouring, meshing, nav, LOD derivation.
  Depends on both of the above.

The rule that keeps the boundary honest: primitives and storage are **peers** that
both read `clayengine`'s constants, so neither imports the other. `contour` is the
one place they meet — it consumes a primitive and writes cluster data.

## Mesh & navigation generation

Layer-3 meshes — and nav surfaces — are *derived* artifacts, so their generation
belongs **off the main thread on a worker pool**. As the camera moves and LODs
change, the main thread enqueues derive-and-mesh jobs and uploads results when they
complete, instead of blocking a frame on the work. Jobs are **best-effort and
restartable**: rapid edits or fast camera motion can obsolete an in-flight job, which
is abandoned and re-queued rather than awaited. Render-time stride already made a LOD
*swap* cheap; the worker pool is what makes it **hitch-free**.

**Navigation is locomotion-gated.** A NavMesh is generated for clusters around the
player **only when a walking/collision locomotion mode is active**. In **fly mode —
the only mode today — no NavMesh is generated and the engine produces no collisions**
(collision is unimplemented; this is the standing behavior for when it lands). A UI
toggle will switch into a surface-walking mode for nav/collision testing later; until
then nav generation is dormant **by design, not by omission**.

## Crate boundaries

Each crate owns one concern: math/time/input, rendering, 2D primitives, scripting,
networking, and the windowed app shell. The umbrella `flicker` crate re-exports all
of them so downstream games depend on a single name. Stub — expand once boundaries
are exercised by real implementations.

## The fixed-step loop

The engine drives simulation at a fixed timestep with interpolated rendering. The
loop lives in `flicker-core` and is plugged into the winit event loop by
`flicker-app`. Stub — flesh out once the loop is implemented.

## The sprite rendering pipeline

Sprites are batched into a single draw call per atlas using a wgpu render pipeline
defined in `flicker-render`. `flicker-2d` builds the higher-level Sprite, Tilemap,
and Camera2D abstractions on top of that pipeline. Stub — describe the actual
pipeline once it exists.

## Scripting integration

`flicker-script` embeds Luau via mlua. Games register host bindings through a
typed registration API; scripts can drive entity behavior and dialogue. Stub —
describe the binding lifecycle once it is implemented.

## Networking model

`flicker-net` is the client side of the live-session, sync, and auth protocols.
Servers live in separate repositories. Hot-path messages use bincode; control-path
calls use JSON over WebSocket or HTTPS. Stub — document message envelopes here.

## Client/server split

The engine ships only the client; live game state, chat, position telemetry, and
combat physics run in separate server projects. Persistent storage (loot, inventory,
character data) goes through an existing web backend. Stub — diagram the trust and
data-flow boundaries once they stabilize.
