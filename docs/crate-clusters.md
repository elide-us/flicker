# flicker crate clusters (taxonomy)

**Status:** locked 2026-07-07. The organizing scheme for the workspace as crates
mature. Physical layout is `Alpha/crates/<cluster>/<crate>/`; crates migrate there
**incrementally, as each gets its API deep-pass** — folder location encodes
maturity (`Alpha/crates/…` = hardened; `crates/…` = POC / pre-alpha awaiting a
pass). Package names stay `flicker-*` (the cluster is the *folder*, not the crate
name). Clients and the umbrella live outside the clusters.

This is a pragmatic collapse of Jason Gregory's *Runtime Engine Architecture*
(the layer diagram): the bottom two-thirds is commodity in modern Rust, so we
only cluster flicker's **own** crates onto the layers that remain.

## What the ecosystem subsumes (we don't build these)

| Gregory layer | We get it from |
|---|---|
| Hardware / Drivers / OS / 3rd-party SDKs | the Rust crate ecosystem |
| Platform Independence + Graphics Device Interface | winit + wgpu (`flicker-app` / `flicker-render`) |
| Math library | glam (via `flicker-core`) |
| Threading library | std + `flicker-worker` |
| Scripting VM | mlua/Luau (via `flicker-script`) |
| Text/font, image I/O | glyphon, image |
| **Physics/collision solver** *(when needed)* | a middleware — Rapier/Avian (thin wrapper) |
| **Audio** *(when needed)* | a middleware — kira/rodio (thin wrapper) |

## Clusters

| Cluster | Crates today | Role / intended API |
|---|---|---|
| **core** | `clayengine` + `flicker-worker` (both `Alpha/crates/core/`), `flicker-core` (still `crates/`) | math/time/fixed-step/gzip/**input**; engine+world constants; job pool. Domain-free foundation. |
| **platform** | `flicker-app` | winit event loop, `App` trait, `run`. Window + game loop + HID host. *(Could fold into core.)* |
| **render** | `flicker-render`, `flicker-2d` | wgpu wrapper: device/surface, pipelines, `RenderTargetHandle`, `Camera` data, `SceneLighting`; 2D sprites/tilemap. |
| **effects** | *(none yet)* | Visual effects & overlays — particles/decals/post. Today sky/fog/volumetric live inside `flicker-render`; peel out later. |
| **content** | `flicker-materials`, `flicker-primitive` | Resources/assets: element+material tables; SDF **CSG primitives** (the shapes authoring tools stamp — also consumed by `world`'s `contour()`). Future: a resource manager + loaders. |
| **world** | `flicker-voxel`, `flicker-worldgrid`, `flicker-worldstate`, `flicker-worldgen`, `flicker-system` | flicker's signature layer: runtime voxel world (storage/contour/mesh/LOD/nav) + offline planet & system generation. |
| **animation** | `flicker-skeletal`, `flicker-flight` (both `Alpha/crates/animation/`) | rig format (`flicker.rig`), pose/FK, CPU skinning, animation/combat state machine + TAE timeline (skinned draw path is in `flicker-render`); **`flicker-flight`** = camera-cinematic service — a `.flight` format (eased camera-pose keyframes + looping tail) parsed + replayed to drive a camera (render-agnostic; emits poses). |
| **mechanics** | *(seed: `flicker-skeletal::state`)* | **Game mechanics services** — combat (TAE hitboxes → hits → damage/reactions), interactions, collision *queries*, and the **encounter construction kit**. Fed by `animation` + physics. |
| **scripting** | `flicker-script` | Luau host, data-only boundary. |
| **frontend** | `flicker-ui`, `flicker-scene`, `flicker-shell` | HUD render bridge + widgets; screen/state stack; the assembled front-end shell service. |
| **net** | `flicker-net` | client transport / state-sync / auth (stubs). |

**Outside the clusters:** `flicker` (umbrella facade — re-exports everything);
`flicker-csg` (a game **client**, not an engine service);
`flicker-celestial` (**abandoned — delete candidate**).

**Future clusters (no crates yet):** `physics` (rigid-body/collision *solver*
wrapper — middleware, added only for true dynamics: ragdolls/props/destruction);
`gameplay` (foundations — game-object model, event/messaging, world-object
runtime); `audio`; a dedicated `debug`/profiling crate (debug-draw/HUD is
scattered in render/ui today).

## Physics & mechanics — the modern reframe

Gregory has one "Collision & Physics" box low in the stack. Modern split:

1. **Dynamics solver = middleware** (bottom). Rigid-body + broad-phase collision
   is commodity (Rapier/Avian); we wrap it thinly *only when the game needs true
   dynamics*. May be minimal/absent for an animation-driven game.
2. **Game-facing collision + combat = mechanics** (gameplay layer). For TAE/
   Souls-style combat, "physics" is mostly *kinematic*: TAE events open hitboxes,
   hitbox↔hurtbox overlap resolves hits, capsule-vs-world sweeps handle
   locomotion. That's mechanics, sitting with combat + animation — **not** a
   low-level physics layer. The **encounter construction kit** is the authoring
   tool + runtime these services feed.

So "physics" is not one missing bucket; it's an optional middleware wrapper +
the `mechanics` cluster.

## Migration policy + status

- **Reorganize *while* refactoring — one crate at a time.** Do a crate's API
  deep-pass / standardization / refactor, harden it (build + test + clippy-clean),
  **then** move the hardened crate into its cluster — the way `flicker-shell` was.
  The table above is the **target map, not a batch to run.** `Alpha/crates/…` means
  "hardened"; everything else stays in `crates/`. It's fine if `flicker-shell` is
  the only one mature enough so far.
- **Folder convention (confirmed):** `Alpha/crates/<cluster>/<flicker-crate>/` —
  cluster subfolders are real folders (a brief "flat `Alpha/crates/<crate>`" detour
  was a misdirection, reverted). Package name unchanged; the `flicker` umbrella path
  + workspace member + `[workspace.dependencies]` path get updated per move. Prefer
  converting inter-crate deps to `.workspace = true` so a move touches only the
  folder + two root `Cargo.toml` lines.
- **Migrated so far:**
  - `flicker-shell` → `Alpha/crates/frontend/flicker-shell/` (2026-07-07, first).
- **Migrated (all 2026-07-07, `Alpha/crates/<cluster>/`):** `flicker-skeletal` → `animation`; `clayengine` + `flicker-worker` → `core`. (`flicker-core` + rest of the umbrella NOT moved yet.)

## Open items (refine as we go)

- **`physics` placement** — its own thin middleware-wrapper cluster vs. folded into
  `mechanics`. Decide when dynamics are actually needed.
- **Clients** — does `flicker-csg` live under `Alpha/crates/…`, stay a top-level
  `Alpha/` client, or move to `examples/`? (Currently top-level `Alpha/`.)
- **world vs content** — world-gen *produces* content but is world-domain; kept in
  `world`.
- **platform vs core** — `flicker-app` its own cluster or folded into `core`.
- Reconcile with the user's AzureDevOps layer diagram when it surfaces.
