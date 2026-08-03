# flicker

A proof-of-concept game-systems sandbox written in Rust on top of wgpu and Luau,
targeting Apple Silicon first and extending to Windows, iPadOS, iOS, Linux, and
Android via the WebGPU spec. It exercises two pillars independently: a **voxel
client renderer** (sparse-octree clusters, dual-contour meshing, render-time LOD)
and an **offline procedural planet generator** (an ISEA icosahedral hex-sphere fed
through a six-epoch planet-evolution simulation). Designed from the start for
networked persistent-world games, with a strict data-only Luau scripting boundary
and a thin client-side networking crate that talks to separate game-server and
web-backend projects.

flicker is one POC within the larger **ClayEngine** ecosystem. For full
orientation — the runtime model, load-bearing design decisions, per-subsystem
state, and deferred ideas — see [CLAUDE.md](CLAUDE.md).

## Build

```
cargo build --workspace        # stable toolchain
cargo test  --workspace        # 170+ tests
cargo run -p flicker-world     # the planet viewer (ISEA hex-sphere + epoch viz)
cargo run -p voxel-cluster     # the voxel client demo (contour + mesh + LOD)
cargo run -p hex-sphere -- 16  # headless topology check + PLY export
```

Use `--release` for any voxel or perf work; debug contour + mesh is slow.

## Status

Pre-alpha — not a playable game yet, but several subsystems are real and working:

- **Voxel engine** — sparse-octree clusters, `contour()` + dual-contour `mesh()`,
  render-time-stride LOD, neighbour-aware seams; lit meshes with procedural sky,
  fog, and sun/moon/stars/eclipse. (Streaming + edge neighbours deferred.)
- **World generation** — an equal-area ISEA hex-sphere topology (12 pentagons, 20
  shards) driving six deterministic formation epochs: composition seeding,
  differentiation, plate tectonics with isostasy, hydrosphere + atmosphere,
  mineralization + microbial life, and erosion + biomes + deposits. Conserved as
  absolute element masses on a sparse ledger; visualized per-cell in `flicker-world`.
- **Engine shell** — winit event loop, stack-based scene manager, Luau-hosted HUD,
  generic worker pool, input/rebind, gzip cluster I/O.

## Workspace layout

The umbrella crate `flicker` re-exports every sub-crate, so a game depends on one
name. Bottom-up:

### Engine foundation

- `clayengine` — world-defining constants only (cluster dim, LOD count, feet/voxel); zero deps.
- `flicker-primitive` — stampable SDF shapes + procedural heightmaps; CSG via min-distance.
- `flicker-core` — math/time re-exports, input (bindings, gamepad/deadzone), gzip, fixed-step loop.
- `flicker-render` — wgpu device/surface/queue and pipelines: lit mesh, sky, billboard, lines, sprite, text.
- `flicker-2d` — Sprite / Tilemap / Camera2D primitives over the render pipeline (stub).
- `flicker-script` — Luau VM host with a strict data-only engine↔UI boundary.
- `flicker-voxel` — cluster sparse storage, `contour()`, `mesh()`, render-time LOD, neighbour reads, nav.
- `flicker-worker` — generic closure-based worker pool.
- `flicker-materials` — tier-① material vocabulary (periodic table + materials, swappable source).
- `flicker-net` — client-side transport / state-sync / auth stubs (servers are separate repos).
- `flicker-app` — winit event loop, frame orchestration, the `App` trait + `run()`.
- `flicker-scene` — stack-based scene manager (replace/push/pop/quit, overlays).
- `flicker-widgets` — UI toolkit over render + script: component walker, `ui/*.lua` library, templates.
- `flicker` — umbrella re-export; the one name games depend on.

### World-generation stack

- `flicker-worldgrid` — ISEA hex-sphere topology only: pentagon patches, icosphere generation.
- `flicker-worldstate` — the conserved ledger substrate (sparse element→mass `Composition`, `Cell`, `Ledger`).
- `flicker-worldgen` — the six-epoch pipeline, `HexState`, `EpochCtx`, and per-cell `FieldSampler`.

### Apps & examples

- `crates/flicker-world` — the current app: icosphere planet viewer + epoch viz + scene shell + Lua HUD.
- `examples/voxel-cluster` — primary voxel demo: 3×3 cluster field, fly camera, dynamic LOD + async re-mesh.
- `examples/hex-sphere` — headless topology test; prints a verification report and writes a per-shard PLY.
- `examples/hex-world` — icosphere explorer + a working vertical water-cycle prototype (awaiting re-homing).
- `examples/hex-map` — superseded flat two-map demo (kept for reference; do not extend).
- `examples/hello-sprite`, `square-chase`, `mesh-smoke` — minimal 2D / mesh references.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
