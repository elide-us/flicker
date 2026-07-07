# CLAUDE.md — flicker

Orientation for any AI agent (or human) picking up this repo. It captures **what
flicker is, the decisions driving its direction, the current state of each
subsystem, and the ideas that haven't materialized yet** — plus how to reach the
project's MCP server and memory. It is a map, not a roadmap: prioritisation and
next-step direction are the user's (Aaron / Elideus) to set, not the agent's.

> Keep this file current. When a subsystem's state changes materially, or a
> pivot lands, update the relevant section here rather than only in a handoff doc.

---

## 1. The bigger picture — ecosystem, POCs, and the runtime model

### The three projects this work sits inside
flicker is one POC within a larger ecosystem. An agent may be dropped into any of
the sibling projects (and onto any of several machines — §7), so keep the whole
shape in mind:

- **ClayEngine** — the engine + game-systems family. **flicker is part of
  ClayEngine**, alongside `ClayEngineOSS` and the **live MMO servers** (a
  Windows-IOCP advanced networking stack built for true-MMO scale).
- **TheOracle** — the web backend + data foundation for everything. *This is the
  same backend the project's MCP server exposes* (`oracle_*` schema reflection +
  `memory_*` tools — §9).
- **Prism** — the **setting / lore** for the MMO. (The world-gen periodic table is
  seeded from "Prism BookIII"; the pentagon defects are lore features — §2.)

### What flicker is
flicker is a **proof-of-concept sandbox** that exercises several pieces of the game
system independently, on a Rust / wgpu / Luau stack (~19 single-responsibility
crates; wgpu 22 → Metal/D3D12/Vulkan, winit 0.30, mlua/Luau, glam). The umbrella
crate `flicker` re-exports every sub-crate so a game depends on one name.
Apple-Silicon-first, extending to Windows / iPadOS / iOS / Linux / Android via the
WebGPU spec. Designed from the start for **networked persistent-world games**: a
thin client-side `flicker-net` talks to *separate* server projects. Status:
**pre-alpha**.

### The POCs and what each one is for
- **Client rendering** → `examples/voxel-cluster`. The in-game **client**:
  contour, mesh, LOD, fly camera, lighting/sky — what a player's machine runs.
- **World generation** → `examples/hex-world`, `examples/hex-map` (superseded),
  and the current `crates/flicker-world`. This is a **static, offline server
  process that sits beside the game**; it generates the planet's **starting
  point** for the live simulation. Not interactive, not the game client.

### The runtime model: static gen → slow live sim → GC
1. **World generation (static / offline)** bakes the initial planet — the epoch
   pipeline (§5) — and hands off a starting state. The runtime never re-runs it.
2. **The live simulation** then evolves that state **very slowly, batch-like**: it
   moves sediment, repopulates resource nodes, runs the water cycle. It is part of
   a larger **garbage-collection-style system that reclaims areas players have
   abandoned**.
3. **Player edits aggregate back on an erosion batch pass.** Detail is materialized
   near players; their changes are written into the macro data when the live sim
   sweeps. E.g. a player removes 100 iron from a cluster and leaves → the next
   erosion pass aggregates the cluster back **with that 100 iron gone** → a later
   re-render of the area produces a fresh cluster that no longer contains it.
   Conserved; never per-frame.

---

## 2. Overarching themes & load-bearing decisions

These are the "why" choices that constrain everything downstream. Don't relitigate
them without the user; do honour them.

### The spatial data model (hex → pixel → cluster → voxel)
The whole world is **hexagons tiling a sphere**. The scale chain:
- **Hex** ≈ 49.6 mi across, stored as a **2K texture**. The 12 **pentagon** tiles
  (on the icosa vertices) are "hidden" lore special-cases — **mountain peaks and
  ocean troughs, outside player reach** — never normal playable terrain.
- **Each texture pixel = 128 ft of game space = one voxel cluster.**
- **A voxel cluster is a sparse octree**, LOD **0–8**. **LOD 8 = a single vector
  for the whole cluster** and is the default stored at world-map scale; finer LODs
  **populate on demand as players get near**.
- A cluster is generated from two inputs: (a) **world-scale procedural
  indicators**, and (b) the cluster's **metadata** — material-distribution
  percentages, hardness, brittleness, saturation (among others).
- **A voxel is not a fixed thing — it is a container for a portion of the
  cluster's material distribution.** What a voxel *expresses* is computed from that
  distribution + local conditions: a voxel holding >1000 carbon compressed hard
  enough becomes **diamond**; mostly SiO → **sand**; etc. Change the distribution
  and the same voxel materialises as something else — this is the concrete reason
  for "shape is disposable, data is truth" below.

### Engine / voxel
- **Shape is disposable; data is the truth.** Terrain geometry (meshes, heights,
  contours) is a *pure function* of stored data, re-derived on demand. Only data
  that can't be regenerated is stored durably.
- **Three-layer voxel model** (`docs/architecture.md` — the load-bearing invariant):
  1. **Contour input** — throwaway primitive/edit shape fed to `contour()`.
  2. **Cluster vector data** — the LOD-0 compressed cluster file *is the data*
     (gzipped JSON today). Saved, loaded, **edited**. Player edits mutate *this*.
  3. **Mesh output** — ephemeral GPU/CPU meshes, hot/cold cached, recycled. Lose
     one → re-derive from layer 2.
- **Never re-contour at runtime.** Coarser LODs are produced by **render-time
  stride** over the *same* stored cluster data, through one LOD-agnostic mesher —
  not by re-running `contour()`. (An early multi-cluster path that re-contoured per
  LOD is a *defect to remove*, not a pattern.)
- **Derived artifacts go off-thread** on a worker pool (`flicker-worker`); jobs are
  best-effort and restartable. LOD swaps are cheap (stride) *and* hitch-free (pool).
- **±1 LOD adjacency invariant** between neighbouring clusters (mesh panics
  otherwise); seams close via low-side ownership + one extra boundary cell row,
  byte-equal vertices, no patch geometry.
- **Navigation is locomotion-gated** — no NavMesh / no collisions in fly mode (the
  only mode today). Dormant *by design*, not omission.
- **Strict Lua boundary** — engine ↔ Lua exchange only plain `Value` (bool/number/
  text), never handles or GPU resources. UI layout/state lives in Lua; swapping a
  UI is swapping a `.lua` file.

### World generation
- **Absolute element amounts, never densities or fractions.** The ledger stores
  conserved masses so conservation is plain add/subtract — no per-iteration
  renormalisation, no float leak. (This is *the* reason for equal-area cells.)
- **Equal-area icosahedral hex grid (ISEA).** If cell areas varied, equal amounts
  wouldn't mean equal concentration and every epoch would have to area-weight.
  Equal-area deletes that bug class.
- **The planet must read as ONE continuous planet** — blend per-cell state across
  neighbours, not per-hex noise tiles. "Visualize X" ≠ "elaborate the sim of X."
- **Epochs are a planet-evolution sim**: each epoch reads the previous layer's
  fields and refines/adds; **every epoch must visually CONTINUE the prior** (e.g.
  Epoch-2 light crust floats visibly into Epoch-3 continents). All layers are kept
  for visualization.
- **Topology is separable from the sim.** The epoch pipeline is already
  topology-agnostic — it consumes `EpochCtx { dirs, neighbors (variable length),
  seed }`. A **pentagon is first-class**: a cell whose neighbour vec has length 5.
- **Pass-based accounting, not a live PDE.** Formation epochs are one-shot
  transforms; the runtime water cycle ticks on a *geological* cadence (one sweep
  pass over the array), never per-frame. Water is a sediment conveyor.

---

## 3. Current direction & authoritative docs

The project has pivoted several times. **Current authoritative direction:**

- **Topology = ISEA icosahedral hex-sphere** in `flicker-worldgrid`
  (12 pentagons on the icosa vertices, 20 triangular shards).
  → `docs/hex-sphere-handoff.md` (decisions locked; Slices 1–3 built).
- **App = `flicker-world`** (binary crate, name provisional) — the icosphere
  planet viewer that consolidates the older `hex-world` / `hex-map` app
  architecture; scene-driven, orbit camera, per-cell epoch-field colour, Lua HUD
  with per-epoch knob panels. → `docs/flicker-world-handoff.md`.
- **Epoch design of record** → `docs/clayengine_world_generation_spec_v2.md`
  (§"Epoch specifications", Phase-1-simple / Phase-2-sophisticated per epoch).
- **Celestial / system formation = `examples/flicker-sol2`** (the viewer) **+ `crates/flicker-system`**
  (the boxed GPU-free sim). A **scene-driven app** (Logo splash → Menu → Sim, with Pause/Settings
  overlays, like flicker-world) that is a **thin Lua-UI shell** over the sim. Two phases over one
  dataset: **Phase 1 — distribution:** one colour ring per Prism element at its atomic-weight cast
  distance (heavier = nearer), differentially sheared + clumpy, with overdensity **dots** (the hot
  spots). **Phase 2 — collapse:** an **inward gravitational collapse run on that cast cloud** (NOT
  a separate condensation disk) — the dominant central lump becomes the star, bodies accrete into
  planets/moons/rings, the habitable world is highlighted. (This rebuilt collapse REPLACED the four
  failed 2026-06-23 attempts — condensation disk → Hill-grid accretion → sub-disk moons → aggregate
  field; do NOT resurrect those.) **EVERYTHING IS DERIVED FROM THE STARTING VALUES** (Prism table,
  cloud distribution, cast params, seed) — never a parallel system or invented tables. **Locked
  (2026-06-23):** per-element cloud *tonnage* is DERIVED from two dials — supernova **Mass** + 
  **Metallicity** (metals-vs-H/He; Sun ≈ 1.4%) — split across elements by the **cosmic-abundance
  curve** (iron peak, post-iron cliff); WHERE = f(atomic mass), HOW MUCH = f(atomic number). The
  boxed sim exposes `SystemConfig`/`Tuning` in → `System` facade → `SystemState`/`Epoch3Handoff`
  out (Workstream A, Session A); the viewer routes those through the Lua HUD — a bottom-right
  control panel + top-right stats overlay (Workstream B, Session B). Model of record: MCP memory
  (decisions "flicker-sol2 mass source LOCKED" + "flicker-sol2 formation sim ROLLED BACK"). →
  `docs/flicker-sol2-handoff.md`, `docs/flicker-sol2-epoch3-pipeline-roadmap.md` (the multi-session
  task inventory + Session A/B outcomes). **`flicker-celestial` is ABANDONED / superseded by this**
  — do NOT build on it or resurrect its model.

### Abandoned / superseded (left in tree, do NOT resurrect or reuse as a path)
- The **flat two-map / bent-rings / σ-zipper** hex model in `examples/hex-map`
  (`topology.rs`, `gadget.rs`, `snap_map*.rs`, spiral ordering, record-flip viz).
  **Slated for deletion** in the `flicker-celestial` refactor (user-flagged cleanup —
  it confuses "what is right" vs modern flicker-world). Its flat *within-hex* math
  (`examples/hex-map/src/geom.rs`) may be referenced as a copy source first; nothing else.
- The **polar-cap defect-concentration** sketch (concentrating curvature at poles).
- `flicker-worldsim` — a redundant world-sim crate that was an anti-pattern; the
  renderer + celestial sim already live in the voxel path. (Reflected in memory
  `voxel-cluster-is-the-renderer`.)
- **`flicker-celestial`** (the crate + `docs/flicker-celestial-*.md`, `examples/flicker-solarsystem`) —
  the condensation/Body-tree/N-body refactor. **Abandoned/superseded by `flicker-sol2`**
  (user, 2026-06-23: "almost everything in that simulation is just completely fucked and
  doesn't do anything the way we want it to"). Left in tree as history; do NOT consume,
  extend, or treat its docs as the design of record. The real system-formation work is
  flicker-sol2.

---

## 4. Crate map

Bottom-up; the umbrella `flicker` re-exports all of them.

### Engine foundation
| Crate | Concern | State |
|---|---|---|
| `clayengine` | World-defining constants only (`CLUSTER_DIM`, `MAX_LOD` derived via log2, `VOXEL_COUNT`, feet/voxel). Zero deps. | done |
| `flicker-primitive` | Stampable SDF shapes (Sphere/Cube/Cylinder/Cone/Flat/Height…) + procedural heightmap; CSG via min-distance. Reads `clayengine` only, never storage. | done for basics |
| `flicker-core` | Math/time re-exports, input (`InputState`, `Bindings`/`Action`, gamepad/deadzone), gzip, fixed-step loop. | mature |
| `flicker-render` | wgpu device/surface/queue; pipelines: mesh(3D, lit, fog), sky (procedural gradient+sun/moon+stars+eclipse), billboard, lines, sprite (atlas), text (glyphon), triangle. Mesh storage = free-list. | lighting/sky/fog/eclipse done; post-process deferred |
| `flicker-2d` | Sprite / Tilemap / Camera2D primitives on the render pipeline. | stub |
| `flicker-script` | Luau VM host (`ScriptHost`); strict data-only boundary; HUD command list out. | done — the engine↔UI seam |
| `flicker-voxel` | Cluster sparse storage, `contour()`, `mesh()` (dual-contour/QEF), `derive_lod()` (render-time stride), neighbour reads, nav. No GPU deps. | core + seams done; streaming/edge-neighbours deferred |
| `flicker-worker` | Generic closure-based worker pool. Task-agnostic. | done |
| `flicker-materials` | Tier-① vocabulary: `Tables` loading `periodic_table.json` + `materials.json` via swappable `TableSource` (JSON now, DB later). 26 elements, 256-material index (~20 resolved). | active |
| `flicker-net` | Client-side transport / state-sync / auth stubs. Servers are separate repos. | skeleton |
| `flicker-app` | winit event loop, frame orchestration, `App` trait + `run()`. | done |
| `flicker-scene` | Stack-based scene manager (`Transition`: Replace/Push/Pop/Quit; overlays). | done |
| `flicker-ui` | UI helpers over render+script (`render_hud`, widgets). | partial |
| `flicker` | Umbrella re-export — the one name games depend on. | — |

### World-generation stack
| Crate | Concern | State |
|---|---|---|
| `flicker-worldgrid` | **Topology only** for the ISEA hex-sphere: `pentagon_patch(rings)`, `icosphere(freq)` → `{dirs, neighbors, area, is_pentagon, shard, id}`. Feeds `EpochCtx`. | Slices 1–3 done; ISEA projection (3b) + ledger `CellId↔CellCoord` (4) pending |
| `flicker-worldstate` | The conserved ledger substrate: `Composition` (sparse element→mass, conservation-safe add/remove/merge), `Cell`, `Ledger`. | defined; Epoch-output→Ledger hookup deferred |
| `flicker-worldgen` | The epoch pipeline (`epoch1..6.rs`), `HexState`, `EpochCtx`, `FieldSampler` (per-cell hardness/relief/vein fields). | Epochs 1–6 built; see §5 |
| `flicker-celestial` | ~~unified celestial sim~~ — **ABANDONED/superseded by `flicker-sol2`** (§3). Left in tree as history; do NOT consume or extend. | abandoned |
| `flicker-system` | **The boxed-up star-system formation sim** (Phase 1+2), GPU-free lib extracted from `examples/flicker-sol2` (Workstream A, 2026-06-24). API: `SystemConfig`/`Tuning` in, `System` facade, `SystemState`/`Epoch3Handoff` out. NOT `flicker-celestial`. → `docs/flicker-sol2-epoch3-pipeline-roadmap.md`. | active |

### Apps & examples
- `crates/flicker-world` — **the current app**: icosphere viewer + epoch-viz + app
  shell (Menu/Loading/World/Pause/Settings), Lua HUD, logo splash, rebind capture.
  `cargo run -p flicker-world`. Controls: drag=rotate, wheel=zoom, V=cycle field,
  ↑/↓=epoch, R=reseed (per-layer), `[`/`]`=grid freq.
- `examples/flicker-sol2` — **supernova ejecta → star-system formation viewer**, a scene-driven app
  (Logo splash → Menu → Sim, with Pause/Settings overlays) that is a **thin Lua-UI shell over
  `flicker-system`**. **Phase 1 (distribution):** a colour ring per Prism element at its
  atomic-weight cast distance, sheared + clumpy, with overdensity **dots**; hover/click a ring to
  focus. **Phase 2 (collapse):** ignite the cloud into a planetary system — bodies, moons, rings,
  motion arcs, gravity-well overlay, highlighted habitable world. **Every control + readout is Lua**:
  a **bottom-right control panel** (phase nav · dial sliders · pause/dots/well checkboxes ·
  seed/new-system/reset buttons) + a **top-right stats overlay** (`scripts/sim_ui.lua` +
  `ui_elements.json`; the splash/menu/pause use `logo.lua`/`modal.lua`). Drag dials · wheel zoom ·
  Esc → Pause. The sim modules (cloud/cast/mass/detect/collapse) live in `flicker-system`, **not**
  `src/` — the example owns only `scene.rs` (the `Sim` scene) / `shell.rs` (the app shell) /
  `draw.rs` / `well.rs`. `cargo run -p flicker-sol2`. → `docs/flicker-sol2-handoff.md`,
  `docs/flicker-sol2-epoch3-pipeline-roadmap.md`.
- `examples/voxel-cluster` — primary voxel demo: 3×3 cluster field, contour+mesh,
  fly camera, dynamic LOD + async re-mesh, Lua debug HUD, pause/settings.
- `examples/hex-sphere` — **headless** topology test: builds the icosphere, prints
  a verification report, writes a per-shard-coloured PLY (pentagons red).
  `cargo run -p hex-sphere -- [freq] [out.ply]`. CI-friendly, no GPU.
- `examples/hex-world` — icosphere explorer + a working **vertical water-cycle
  prototype** (`layers.rs`, conserved to <0.1% over 300 ticks) — stranded on the
  old flat topology, awaiting re-homing.
- `examples/hex-map` — **superseded** flat two-map demo (see §3).
- `examples/hello-sprite`, `square-chase`, `mesh-smoke` — minimal 2D / mesh refs.

---

## 5. The epoch pipeline (world-gen heart)

Six **formation epochs** — deterministic one-shot transforms accumulating per-hex
`HexState`. Spec reserves 9 total; epochs 7–9 are runtime layers (GM/underground/
surface) not yet reconciled with formation epochs. All six are **built with real
physics** (epochs 1–4 especially) and **visualized** in `flicker-world`.

| # | Name | Does | Status |
|---|---|---|---|
| 1 | Composition seed | Distribute element mass (heavy→equator, volatile→pole) + correlated fBm; normalise to target mass. | ✅ real |
| 2 | Differentiation | Density-sort: light crust floats, heavy sinks; thin crust → volcanic. Continuity into E3. | ✅ real |
| 3 | Plate tectonics | Voronoi plates (multi-source BFS), motion vectors, boundary classes, **isostatic elevation** (buoyancy base + tectonic deform), orogeny, hotspot chains, crust age. Cross-hex `Plate` records. | ✅ real |
| 4 | Hydrosphere + atmosphere | Water endowment from H/O budget (**product**, law-of-mass-action, not min); binary-search sea level to hold the volume; temperature (latitude + lapse + axial tilt); outgassed atmosphere; precipitation; prebiotic precursors. | ✅ real |
| 5 | Mineralization + microbial life | Hydrothermal signature; greedy ore-vein tracing along faults → metal into crust; microbial cradles at vents. (Per-hex vein membership; cross-hex vein *paths* deferred.) | ✅ |
| 6 | Erosion + biomes + life + deposits | Hydraulic erosion + thermal creep over drainage; Whittaker biomes; flora; **time-gated** coal/oil (organics + decomposer onset) and chalk/limestone (warm shallow seas). Cross-hex `Watershed`. | ✅ |

**`FieldSampler`** derives per-cell spatial fields (composition-weighted hardness,
domain-warp relief + crystallization + orogeny folds, vein filaments) — the bridge
toward 2048²-per-hex materialization (not yet built).

**Material model** (`docs/material-model-handoff.md`): Tier-① tables
(`data/materials/*.json`) → Tier-② conserved `Composition` ledger → Tier-③
re-derived spatial fields → (future) per-cluster voxel materialization. Hex ≈
49.65 mi (2048 texels × 128 ft). Reseed is **per-layer**: reseeding epoch *e*
re-rolls only that epoch's knobs; upstream stays byte-identical.

---

## 6. Ideas not yet materialized / deferred (inventory, not a plan)

Recorded so they aren't lost or re-discovered. **Prioritisation is the user's
call** — this is descriptive, not a committed roadmap.

- **Re-home the water cycle** — promote `examples/hex-world/layers.rs` to a real
  crate, seed it from Epoch-4 output, run it on the icosphere topology with
  cross-hex halo exchange. (`docs/water-cycle-handoff.md`, `epoch-data-audit-…`.)
- **Per-cluster materialization** — the deterministic `(hex, state, field, pixel)
  → 2048² heightmap stack` that bridges world-gen to the voxel renderer.
  (`docs/material-model-impl-handoff.md`; the heightmap→voxel-cluster bridge is the
  stated overarching goal — memory `voxel-cluster-is-the-renderer`.)
- **ISEA equal-area projection (Slice 3b)** — replace the cheap projection so cell
  areas collapse toward 1.0 (currently ~1.75× spread). Pin the frequency for ≈49.65 mi.
- **Ledger integration (Slice 4)** — resolve `CellId ↔ CellCoord`; migrate Epoch-6
  `HexState` into `flicker-worldstate::Ledger`.
- **Cross-hex vein paths**, **formed-material classifier** (composition →
  granite/basalt/limestone → real rock hardness), **Rivulet DAG** for O(1) sediment
  transport, **dynamic convection**, **3D caves/overhangs**, **greenhouse feedback**.
- **Epoch 5 viewer pass** (ore/vein view), richer tectonics/hydrosphere reads,
  cell pick/inspect, plate-motion/age overlay.
- **Keybinding disk persistence — BLOCKED** on `flicker-core` `InputMap` serde
  (non-string key). Rebinds persist per-session only.
- **Display-apply + gameplay input routing** in `flicker-world` (camera/world
  controls are hardcoded, not yet through `Action`/`InputMap`).
- **Epoch 7–9 ↔ runtime-layer reconciliation** (world-gen epochs ARE runtime
  layers — memory `epoch-layer-reconciliation`).
- **Streaming / edge+corner neighbours / LOD8 fallback** in the voxel path.

---

## 7. Build, test, dev-box

```
cargo build --workspace          # stable toolchain (rustfmt + clippy components)
cargo test  --workspace          # 170+ tests
cargo clippy --workspace
cargo run -p flicker-world       # the planet viewer (the user runs this, see §8)
cargo run -p hex-sphere -- 16    # headless topology check + PLY
```

- Profiles: dev `opt-level=1`, deps `opt-level=3`; release `lto=thin`,
  `codegen-units=1`, `strip`. Use `--release` for any voxel/perf work (debug
  contour+mesh is slow).
- **Two active dev/test machines — you may be on either** (memory `dev-box-profile`):
  - **MacBook Pro** — M5 Pro, Apple Silicon, unified memory, high-end CPU/GPU.
  - **Windows desktop** — discrete **nVidia RTX 3060**, ample memory.

  The old **MacBook Neo** (A18, ~8 GB RAM) is **out of the dev loop** — no longer a box
  to design or budget around. Both active boxes are strong, but strong hardware is **not
  licence to be wasteful**: use the correct standard technique, not brute force (e.g.
  instanced GPU skinning to draw many characters, never per-model CPU skin + per-frame
  re-upload). **Build for what the feature needs**; GPU/wgpu viewers are always fine
  (don't default to headless). The remaining scaling ceiling is the
  **heightmap/materialization** layer (8 MiB/hex × many cells) — two independent axes
  (cell count vs per-cell heightmap resolution); keep both bounded, never pay both at once.

---

## 8. How to work in this repo (conventions the user enforces)

These are durable preferences (also in Claude Code's per-project memory, §9). They
override default agent behaviour:

- **Stay out of git.** Git is the user's domain — produce code/docs, never run git
  commands, even when asked to "commit". Current branch is `macbook` (the user
  works across machines and PRs/merges themselves).
- **The user verifies the app themselves.** Never launch the windowed app, use
  computer-use, or change committed/default state to "see" behaviour.
  `cargo build/test/clippy` are fine; the user runs the window and reports.
- **Scope discipline / let the user drive.** Strict spec leashes — flag adjacent
  work, don't drift into it. Don't author roadmaps or slice-ladders; don't treat a
  short "proceed"/"continue" as licence to expand scope. Report plainly, then let
  the user choose. Thin slices.
- **Generate via references, not patch-after.** Build cross-boundary output in one
  pass through neighbour references; don't stitch independent results afterward.
- **Big work wraps into `docs/*-handoff.md`** at context boundaries — that corpus
  is how state survives across sessions. Read the relevant handoff before
  continuing a thread.

---

## 9. Memory & the MCP server

There are **two** memory systems in play. Use both; they serve different scopes.

### 9a. Claude Code per-project memory (local, file-based)
Lives at `~/.claude/projects/-Users-elideus-Repos-flicker/memory/` on this machine
and is auto-loaded each session. One fact per file with frontmatter; `MEMORY.md` is
the index (one line per memory). Used for **durable working preferences and project
context** that should be in front of the agent every session (the conventions in
§8 come from here). Machine-local to Claude Code. To add/maintain: write a memory
file + add an index line in `MEMORY.md` (see the harness memory instructions).

### 9b. Elideus MCP server (cloud, cross-session, cross-client)
The project's own backend ("Elideus Group" / oracle) exposed as an MCP server. It
provides two tool families:

- **`memory_*`** — a persistent, **project-scoped** memory store shared across
  machines, sessions, and clients (Claude Code, Claude.ai, Desktop). Use this for
  durable cross-machine decisions/specs/session-summaries.
- **`oracle_*`** — live SQL-Server schema reflection for the Elideus backend DB
  (list/describe tables, full schema, RPC domains/models). Relevant to the backend,
  not to flicker's Rust code — but it lives on the same server.

**Connecting it (the user does this once, per client):** In **Claude.ai or Claude
Desktop → Settings → Connectors** (the "Search Tools" / Add-connector UI), add the
Elideus MCP server and authorize it (OAuth; memory needs the `mcp:memory` scope,
schema-write the `mcp:schema:write` scope). Once connected, its tools surface in
Claude Code as deferred tools named `memory_*` / `oracle_*` (load their schemas via
ToolSearch before calling — e.g. `select:…memory_store`). The server prefix is a
per-connection id, so reference the tools by their short suffix.

**Using the memory tools (the common ones):**

| Tool | Use |
|---|---|
| `memory_store(project, kind, title, body, tags?, thread_guid?)` | Save an entry. For flicker use **`project: "flicker"`**. `kind` ∈ `decision \| invariant \| spec \| note \| session_summary \| snippet`. `body` is markdown. |
| `memory_search(query?, project?, kind?, tags?, limit?, offset?)` | Free-text (LIKE) search over title/body/tags, with exact project/kind filters. |
| `memory_list_recent(project?, limit?)` | Most-recently-modified entries (filter to `flicker`). |
| `memory_get(key_guid)` | Fetch one entry by guid. |
| `memory_update(...)` | Edit an existing entry (prefer updating over duplicating). |
| `memory_thread_create` / `memory_thread_get` | Group related entries into a thread. |

Conventions for flicker memory entries: use `project: "flicker"`; pick the kind
deliberately (`decision`/`invariant` for the load-bearing §2 calls, `spec` for
design specs, `session_summary` to hand off long sessions, `note`/`snippet`
otherwise); tag with subsystem words (`worldgen epoch tectonics`, `voxel lod
seam`, `ui lua`, etc.). Search before storing to avoid duplicates; update in place.

> **Seeded.** The cloud store now has a **"flicker — foundations"** thread with
> three entries: orientation (ecosystem/POCs/runtime model), spatial data model +
> core invariants, and current direction + abandoned paths. **Keep using it** —
> when a durable cross-machine decision/spec/invariant lands, store it (or
> `memory_update` the relevant entry); don't wait to be asked. The repo
> `docs/*-handoff.md` remain the long-form record; this store is the searchable
> cross-machine index of what's load-bearing.

---

## 10. The docs corpus

`docs/` holds the design specs and per-thread handoffs — the real long-form record.
Start with the §3 authoritative trio, then by subsystem:

- **World-gen / epochs:** `clayengine_world_generation_spec_v2.md`,
  `flicker-world-system-spec.md`, `material-model-handoff.md`,
  `material-model-impl-handoff.md`, `epoch-pipeline-review.md`,
  `epoch-data-audit-handoff.md`, `epoch3-isostasy-handoff.md`,
  `epoch4-hydrosphere-handoff.md`, `biosphere-epoch-handoff.md`,
  `water-cycle-handoff.md`.
- **Topology / app:** `hex-sphere-handoff.md`, `flicker-world-handoff.md`,
  `hex-world-handoff.md`, `hex-world-data-handoff.md`, `hex-map-handoff.md` (super.).
- **Voxel engine:** `architecture.md` (the source-of-truth invariant),
  `voxel-mesh-regen.md`, `voxel-lod-cache-handoff.md`, `voxel-crosslod-seam-handoff.md`,
  `voxel-multicluster-lod-seams.md`, `voxel-seam-design.md`,
  `voxel-mesh-worker-handoff.md`, `voxel-terrain-walking-handoff.md`,
  `voxel-virtual-voxel-inspector.md`.
- **Engine systems:** `lighting-handoff.md`, `ui-lua-handoff.md`, `ui.md`,
  `input-settings-handoff.md`, `FlickerSetupSpec.md`.
</content>
