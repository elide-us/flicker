# flicker — instanced-skinning field viewer + session handoff

**Entry point for a fresh session.** This is the brief for the next **big task** (the animation
**field viewer**, built on a new **instanced GPU-skinning** pipeline), plus the map of what
landed this session and every memory reference to orient from. Written 2026-07-06 on branch
`macbook`.

> **Read first for the foundation:** `docs/flicker-combat-animation-handoff.md` — the full
> combat/animation record. Its §9–§13 are all from this session; §13 is the latest state.
> Then `docs/flicker-animation-handoff.md` (the rig/pose/skin primitives).

Honour the repo conventions (CLAUDE.md §8): stay out of git; the **user verifies the window**
(build/clippy/test only from the agent); thin slices; don't author roadmaps beyond what's here;
`docs/*-handoff.md` + MCP memory are the durable record.

---

## 1. Where things stand (foundation is DONE)

The skeletal-animation + combat-state-machine stack is built and green:

- **`Alpha/flicker-skeletal`** — the GPU-free runtime, extracted from the example this session
  (`format` / `pose` / `skin` / `state`). It is the CPU-authoritative pose layer + the combat
  state machine + TAE event timeline. `pose::palette()` already emits `global × inverse_bind`
  skinning palettes — **the exact input the GPU-skinning pipeline consumes.**
- **State machine** — 19 states (locomotion incl. directional walk/run/crouch, jump chain,
  Attack_1, any-state Dame/Death), authored in `examples/flicker-animation/assets/Katanami.pack.json`.
  Crossfade blending, cancel/combo windows, directional (`move_left/right/back/forward`) triggers.
- **91-clip library** consumed via a recursive `clips/` loader (In-Place clips bare, RootMotion
  `RM/`-namespaced).
- **State-machine diagram** — `examples/flicker-animation/tools/gen_state_diagram.py` +
  `state_diagram_template.html` generate an interactive HTML diagram **live from the pack**
  (artifact `ae85675d`). The pack is the source of truth.
- Tests: `cargo test -p flicker-skeletal` (14) + `-p flicker-animation` (5); clippy clean.

What is NOT built: the GPU-skinning pipeline, the field viewer, hitbox/hurtbox capsules,
stamina/poise/i-frame *enforcement*, the weapon-pack loader + equip/pickup, movement into
`flicker-csg`, Block/Guard/Parry clips (authoring gap), a real locomotion blend space.

---

## 2. THE BIG TASK — the instanced-skinning field viewer

**Vision (the user's).** A living 3D field of **all** the model's animations, laid out in
clusters (like the diagram), each model **playing its clip**, with **transition arcs + arrows**
between them. Fly the camera over the field to see how the whole machine connects. As you drive
the state machine by input, the **active node highlights** and (optionally) a top-right PiP shows
the same model in player-view. **Unused clips are visibly orphaned** — present in their cluster
with no wires (the user's spot-the-orphan insight; e.g. `RM/CrouchLoop` — the **moving** crouch-idle
loop — was an unused orphan; it is now wired as the default `Crouch` state's clip, see §7i).

This is simultaneously a **dev tool** (see + tune the whole machine in motion), an **engine
flex**, and the **foundation for real gameplay** (crowds / party / enemies all need instanced
skinning). It replaces the *need* for an in-engine `.pack` editor for a long time — authoring
stays in JSON (source of truth), the field is the live read view (add JSON hot-reload and your
text editor + the 3D field IS the editor; do NOT build in-client form UI = "Excel in the client").

### Layer 1 — the `flicker-render` instanced GPU-skinning pipeline (foundational)

This is the "GPU palette skinning" step the animation handoffs earmarked as *deferred to alpha*.
The current viewer's per-frame **CPU-skin-and-reupload of one character** is a POC shortcut —
NOT the pattern. The correct technique (industry standard; "load one tree, draw it a million
times") is instanced GPU skinning:

- **Static skinned mesh, uploaded once** — one vertex buffer: `position/normal/uv/tangent` +
  `joints[4]`/`weights[4]` as attributes. Shared by all instances.
- **Per-instance bone-palette storage buffer** — `mat4[instance][bone]`, indexed in the vertex
  shader by `@builtin(instance_index)`. **Rewritten each frame** from `pose::palette()` per
  instance (CPU posing is cheap: ~instances × 94 bone mults; the *upload* is the concern).
- **Per-instance model transform** — field position (instance-step vertex buffer, or in the
  storage buffer).
- **Skinned WGSL vertex shader** — skins position **and** normal/tangent through the instance's
  palette; the **existing PBR fragment path** is reused unchanged.
- **One instanced draw call:** `draw_indexed(0..idx, 0, 0..N)`.

**The two genuinely fiddly bits — use the MiniEngine reference (§4) here:**
1. **Per-frame palette upload.** ~91 × 94 × 64 B ≈ 0.5 MB rewritten every frame; a naive
   re-upload stalls. Model it on MiniEngine's `Core/LinearAllocator.h` / `UploadBuffer.h` ring
   patterns (→ a wgpu staging/ring buffer), not a fresh buffer per frame.
2. **Bind-group budget.** wgpu default `max_bind_groups = 4`, and the PBR material path already
   packs a 5-texture group (group 1). The palette + instance-transform storage is a **new** group
   — pack carefully (MiniEngine's `DynamicDescriptorHeap.h` discipline is the analog).

Additive, like the textured/PBR pipeline was — don't disturb `draw_mesh` / `draw_textured_mesh_pbr`.

### Layer 2 — the field-viewer app (new example)

On top of the pipeline: cluster/field **layout** (reuse the diagram's cluster grid, lifted to a
3D plane); **fly camera** (mirror `examples/voxel-cluster`'s); **transition arcs** via the
existing lines pipeline + **billboard arrows**; **per-instance clips** driven by `flicker-skeletal`;
**active-state highlight + camera follow** (state machine driven by input); **pack hot-reload**
(file watch → re-layout); a **skeleton↔skinned toggle** (skeletons are cheap — FK + lines — a fine
aesthetic/LOD option, but with instancing all 91 can be full characters); **orphan clips shown
unwired** in their clusters.

### Suggested thin slices (verify each; the user drives)

1. **Pipeline proof** — draw N copies of the character in a grid, each with its own palette
   (static poses, no animation yet). Proves the GPU path + palette storage buffer + instanced
   draw + upload strategy. Verify: N characters each holding a different clip's frame, stable fps.
2. **Animate the field** — each instance runs a clip via `pose::sample_local_poses` → globals →
   `palette`; a field of animating characters.
3. **Spatial graph** — cluster layout + fly camera + transition arcs (lines) + arrows.
4. **Drive it** — the state machine marks an *active* instance; highlight + camera follow;
   unused clips rendered as dim, wire-less orphans.
5. **Polish** — pack hot-reload; skeleton/skinned toggle; (later) the top-right PiP player-view.

---

## 3. Honest flags / open design forks (don't silently resolve)

- **Directional locomotion is a discrete-state stand-in for a 2D blend space.** The pack wires
  Walk/Run/Crouch × direction as discrete strafe states — it works but the transition mesh is
  combinatorial (that's *why* blend spaces exist). A real locomotion blend space is the production
  answer; design it alongside the field-viewer / lock-on work, don't keep hand-wiring strafes.
- **Block / Guard / Parry** have **no clip** (only `RunBlock`). The Katanami artist doesn't
  animate → author via the **Blender Lab MCP** (`blender_mcp` is now cloned in `~/Repos/`; the
  official `projects.blender.org/lab/blender_mcp`, NOT the community one). Not yet connected to
  Claude Code. Plan in memory `2B313623`.
- **The state-diagram generator is a STOP-GAP.** The eventual authoring tool is a **DB-backed
  graph editor on TheOracle** (a whole content-management section for the game systems — models,
  animation primitives, state machines, TAE rules). The load-bearing design there is the
  **schema**, not the UI. Intent in memory `63E594C8`.
- **Combat netcode (server-authoritative TAE) + the player timeline bar** are designed but NOT
  built — memory `CF309B5C`. Relevant only when combat goes networked; the timeline bar's data
  already exists in `state.rs` (play-head, active windows, rejected inputs).

---

## 4. Key reference material

- **MiniEngine (`~/Repos/DirectX-Graphics-Samples/MiniEngine`)** — the user's reference of record
  for engine techniques; translate D3D12/C++ → wgpu/Rust. For the instancing: `Core/GpuBuffer.h`,
  `UploadBuffer.h`, `LinearAllocator.h`, `BuddyAllocator.h`, `DynamicDescriptorHeap.h` (buffer /
  upload / descriptor patterns) and `Model/` + `ModelViewer/` (skinning). Memory
  `miniengine-graphics-reference`.
- **Dev boxes:** two active — **M5 MacBook Pro** + **Windows / RTX 3060** desktop. The Mac Neo is
  **out of the loop** (don't budget around it). Strong hardware ≠ license to brute-force — use the
  standard technique (instancing). Memory `dev-box-profile`.
- `cargo` isn't on PATH by default → `source ~/.cargo/env` in Bash calls (memory `cargo-env-not-sourced`).

---

## 5. Memory references (orient fast)

**MCP store (`project: "flicker"`)** — the searchable cross-machine index. Load a tool via
ToolSearch (`select:…memory_get`) then `memory_get(<guid>)`.

| guid | kind | what |
|---|---|---|
| `F36DF40F` | spec | combat / animation state-machine + content-pack + souls-like metadata DESIGN brief |
| `D754CF13` | decision | state-machine + TAE timeline Slice 1 landed |
| `3CC75503` | decision | animation runtime extracted → `Alpha/flicker-skeletal` |
| `FB11FF99` | decision | crossfade blending landed (SM stays pose-free; TRS slerp) |
| `2B313623` | note | 91-clip extraction + Block/Guard/Parry gap + Blender-MCP authoring plan |
| `6B1E8681` | decision | 91-clip library ADOPTED (recursive `clips/` loader; In-Place bare / `RM/` namespaced) |
| `CF309B5C` | spec | combat TIMING MODEL — server-authoritative TAE netcode + player timeline bar |
| `63E594C8` | note | INTENT: TheOracle content-management section for game systems; schema is load-bearing |

Earlier/foundational (referenced above): `1E75AEBA` (rig/clip format spec), `04BE5862` (viewer +
textured/PBR pipeline), `386AC689` (granny-tier / TAE-authority / CPU-pose canon), `50EA9C0F`
(`Alpha/` crates dir), `F1F14C20` (voxel UV/normal invariant — relevant when the textured/skinned
pipeline meets voxel geometry).

> Not yet in MCP (documented in the combat handoff only): directional locomotion + the
> state-diagram generator (both combat handoff §13). Store them if a new session finds it useful.

**Local Claude Code memory** (`~/.claude/projects/-Users-elideus-Repos-flicker/memory/`,
auto-loaded each session): `dev-box-profile`, `miniengine-graphics-reference`,
`less-code-every-calculation-counts`, `clarify-intent-before-building`,
`user-verifies-app-themselves`, `cargo-env-not-sourced`, `alpha-crates-flicker-csg`.

---

## 6. Build / verify

```
source ~/.cargo/env
cargo test  -p flicker-skeletal        # 14
cargo test  -p flicker-animation       # 5 (fixture + library/pack guards)
cargo clippy -p flicker-skeletal --all-targets -- -D warnings   # clean
cargo run   -p flicker-animation       # the USER runs the window (WASD move, G graph/manual, L blend)
python3 examples/flicker-animation/tools/gen_state_diagram.py out.html   # regen the diagram from the pack
```
Note: `-D warnings` across the whole example dep-tree trips **pre-existing** `flicker-core` lints
(`new_without_default`, …) unrelated to this work — scope `-D warnings` to `flicker-skeletal`.

---

## 7. Slice 1 LANDED — the instanced GPU-skinning pipeline (macbook, 2026-07-06)

The `flicker-render` instanced-skinning pipeline (Layer-1 core) is built and headless-verified.

- **`crates/flicker-render/src/pipeline_skinned.rs`** + **`shaders/skinned.wgsl`** — one static
  skinned mesh, N instances, **one instanced draw call**. `SkinnedVertex` (position/normal/uv +
  `joints[4]`/`weights[4]`, bind pose, uploaded once). Group 0 = camera + scene uniforms; group 1
  = **read-only storage** `palettes` + `instances`, read in the **vertex stage**. Vertex shader
  skins pos+normal (4-influence LBS) from `instances[instance_index].palette_offset`, applies the
  instance model matrix, then camera. Fragment = simple two-light Lambert over neutral steel (the
  proof; texturing/PBR is a later slice). Exported from `lib.rs`: `SkinnedMeshPipeline` /
  `SkinnedVertex` / `SkinnedMeshHandle`.
- **API:** `upload` (static mesh) · `set_camera_matrix` / `set_scene_uniform` · `draw_instanced(
  device, queue, mesh, models: &[Mat4], palettes: &[Mat4], bone_count)` — writes both storage
  buffers (grows + rebuilds the skin bind group on demand) and queues the instanced draw ·
  `render(pass)` · `free` / `clear`. Palettes are the flat concatenation (instance `i` bone `b` at
  `i*bone_count + b`) — exactly what `flicker-skeletal::skin::palette()` produces per instance.
- **Verified headless:** a test (`skinned_pipeline_compiles_and_draws_instanced`) creates the
  pipeline (compiles the WGSL + validates layouts **incl. storage-in-vertex**), uploads a
  triangle, and **executes a real 2-instance skinned draw to an offscreen colour+depth target**
  under a validation error scope — passes on this Metal box, skips cleanly with no adapter.
  `cargo build/clippy -p flicker-render` clean.
- **Upload strategy is `queue.write_buffer` for now** (correct; a per-frame ring/linear allocator
  per MiniEngine `Core/LinearAllocator.h` is the follow-up when the field runs 91 live palettes).

**NOT done (next slices, need the window to verify — the user's macOS is currently flaky):**
the field-viewer **example** (cluster layout, fly cam, arcs, active-node highlight, orphan clips,
hot-reload) + animating each instance via the pose layer from example code. Visual correctness
(do the characters actually skin right?) is a **windowed check** still pending.

## 7b. Renderer integration LANDED — the plumbing half of Slice 2 (macbook, 2026-07-06)

The skinned pipeline is now wired into the top-level `Renderer` (`crates/flicker-render/src/renderer.rs`),
so the field-viewer example can drive it with no direct pipeline access — additive, exactly like the
textured-mesh path was.

- **Field + lifecycle:** `Renderer` owns a `skinned: SkinnedMeshPipeline`, constructed in `new`,
  `clear()`ed in `begin_frame`. In `end_frame` it forwards the frame's `view_projection`
  (`set_camera_matrix`) and the same `scene_to_uniform(&scene, camera_pos)` the mesh/textured
  pipelines get (`set_scene_uniform`), and `render`s in the **opaque pass** right after
  `mesh_textured` (depth-write, `LessEqual`; before lines/billboards) — so skinned characters
  depth-compose with flat + textured meshes.
- **Public API** (grouped with the mesh-upload/draw methods): `upload_skinned_mesh(vertices,
  indices) -> SkinnedMeshHandle` · `free_skinned_mesh(handle)` · `draw_skinned_instanced(mesh,
  models: &[Mat4], palettes: &[Mat4], bone_count)`. The last uploads palettes + per-instance
  transforms and queues the one instanced draw *now* (`queue.write_buffer`, ordered before submit).
  **One skinned mesh per frame** — a second `draw_skinned_instanced` this frame replaces the queued
  draw (the field-viewer's one-mesh-many-instances shape; documented on the method). Reachable
  through the umbrella as `flicker::render::{draw_skinned_instanced, SkinnedVertex, SkinnedMeshHandle}`.
- **Verified:** `cargo build/clippy -p flicker-render` clean (no flicker-render source warnings; the
  `-D warnings` failures are the pre-existing `flicker-core` path-dep lints, per §6's note). The
  pipeline's own GPU test still passes (real 2-instance draw). `cargo build -p flicker-animation`
  (a renderer consumer via the umbrella) still builds → the API addition is non-breaking.
- **Still unverified in a window:** nothing *uses* the Renderer API yet — that's the field-viewer
  example (Slices 3–5) + the per-instance pose→palette animation. Visual correctness of the wiring
  is proven only by the headless pipeline test until the example exists and the user runs it.

## 7c. Example Slice 1 LANDED + VISUALLY VERIFIED — `flicker-field-viewer` (macbook, 2026-07-06)

The **pipeline-proof** example is built and the user confirmed the window: a grid of **91
Katanami characters** (one instance per clip), each frozen at a *distinct* pose, correctly
skinned (upright, not inside-out / exploded), **rock-solid fps** — all in **one instanced draw
call**. This is the first real consumer of `Renderer::draw_skinned_instanced` and the end-to-end
visual proof of the GPU-skinning path.

- **`examples/flicker-field-viewer`** (new crate; registered in the workspace). Reuses
  `examples/flicker-animation/assets` via `../flicker-animation/assets` — **no asset duplication**.
  `cargo run -p flicker-field-viewer`. Controls: drag = rotate · wheel = zoom · Esc = quit.
- **How it draws:** builds one bind-pose `SkinnedVertex` buffer from `model.mesh` (uploaded once
  via `upload_skinned_mesh`); grid layout centred on the origin (√N cols, spacing = `orbit_radius
  × 2.6`); per instance samples its clip at the **midpoint frame** (`pose::sample_local_poses →
  global_transforms → skin::palette`) and concatenates the palettes; per-instance model =
  `translate(grid_cell) × Model::world` (source→engine, same convention as the single-char viewer);
  one `draw_skinned_instanced(mesh, &models, &palettes, bone_count)` per frame. Neutral-steel
  Lambert (no textures yet — a later slice). Fragment/cull/bind conventions all correct on Metal.
- **Verified:** `cargo build/test/clippy -p flicker-field-viewer` clean; the headless test
  (`field_builds_one_instance_per_clip_with_full_palettes`) loads the **real** 91-clip library and
  asserts one instance per clip + `palettes.len() == instances × bones`, all finite. Plus the
  user's window (above).
- **Numbers observed:** 91 instances · 94 bones/instance · 11 925 verts/instance.

## 7d. Slice 2 (animate) VERIFIED + Slice 3a (fly cam) LANDED (macbook, 2026-07-06)

- **Slice 2 (animate) — user-verified in the window.** Every instance plays its clip: a shared
  elapsed-time clock drives each instance's play-head (each loops at its own `tick_rate_hz` over its
  duration); `rebuild_palettes()` re-samples all clips each frame and re-submits. This is the real
  per-frame CPU-pose → GPU-skin path (Slice 1's static palettes were the only shortcut). **Space** =
  play/pause. Perf still rock-solid.
- **Slice 3a (navigation) — LANDED, pending window.** Two cameras: **Orbit** (survey — drag rotate,
  wheel zoom) and **Fly** (WASD move along look dir · E/Q up/down · Shift sprint · right-drag look ·
  wheel = fly speed), toggled with **Tab**. Entering Fly seeds `FlyCam::sync_from` the current orbit
  pose so the switch doesn't jump. Look convention mirrors `voxel-cluster` (`yaw -= dx·sens`,
  `pitch -= dy·sens`). Headless test `fly_cam_syncs_from_orbit_without_jumping` guards the seed.

### Known edge case (user-flagged, deferred): RootMotion clips drift out of their cell
`RM/` clips bake **world translation** into the pose, so an instance playing one (e.g. `RM/Slide`)
walks out of its grid cell — and clips with no return (Slide) never come back. In-Place variants
stay put. **Fix when layout needs it:** in-place display — strip the root bone's horizontal
translation each frame (keep Y so jumps still rise) before FK. Cheap; deferred per user ("no biggie").

## 7e. Slice 3b (spatial graph) STRUCTURE LANDED (macbook, 2026-07-06) — pending window

The field is no longer a flat √N grid — it's the **laid-out state graph** (`src/graph.rs`).

- **Nodes:** the pack's 19 **states** = **connected** nodes (each plays its state's clip); the ~72
  clips no state references = **orphan** nodes. `connected_count + orphan_count == clips` (91).
- **Layout = dynamic weighting.** Connected states are placed by a **Fruchterman–Reingold**
  force-directed relaxation (400 iters, deterministic — seeded from the HTML diagram's cluster
  grouping via a ported `group_of`, per-node golden-angle spiral seed; edges attract, all pairs
  repel, `k = unit·7`). Orphans go in a plain grid in a **separate zone** (+Z of the connected
  bounds). Whole field centred on the origin for the orbit camera. `unit = model.orbit_radius`.
- **Connections drawn** as ground-plane lines (`draw_lines`) between connected nodes — **unique
  undirected** pairs from per-state `transitions` + `next`. (Any-state edges hit→Dame / die→Death
  are deferred — they need an ANY marker node like the HTML; noted, not drawn.)
- **Cameras:** R/F added for fly up/down (matches voxel-cluster; E/Q kept).
- **Verified:** `cargo test/clippy -p flicker-field-viewer` clean (4 tests incl. `graph_has_states_
  orphans_and_links`: ≥15 states, orphans>0, links non-empty, finite positions). Window pending.
- **NOT done in this slice (the *look* of the vision):** the golden translucent glow **rings** each
  character stands in, node **selection** → orange/rust ring, and the below-ground **cloud/"glass"**
  effect. Those are the next slices — the graph node positions + `Node.connected`/`Node.label` are
  the hooks they consume.

## 7f2. Scene + selection model landed (macbook, 2026-07-06) — pending window

- **Floor scene:** one lit floor plane (`floor_quad`, double-sided) sized to a container with
  two aligned regions — a square **graph region** (states, force-directed then *fitted* into the
  box) and a **waiting region** grid (orphan clips) — each with a drawn boundary outline. Camera
  frames the whole floor; characters stand on it (floor at the lowest bind vertex = soles).
- **Isolated-node fix:** degree-0 states are **pinned** at their seed in the FR layout so they
  can't drift and inflate the fit (was collapsing the graph to a corner). Guard test
  `connected_graph_fills_its_region`.
- **Reaction states dropped:** any-state targets (Dame/Death) are excluded from the flow graph
  (reachable only via any-edges we don't draw); their clips fall through to the waiting area. Now
  **17 flow states + 74 orphans**. Guard in `graph_has_states_orphans_and_links`.
- **Selection model (matches the HTML):** **no always-on edge web**. Click a node (pick =
  unproject cursor → floor-plane hit → nearest node within a threshold) → it's marked (orange/rust
  box) and **only its connections** are drawn; the HUD shows its label + connection count. Clicking
  empty floor deselects. This is the "lay out nodes, reveal wires on select" behaviour; the orange
  marker previews the selection colour (the real glow ring is the next visual slice).

## 7g2. Grouped **box** layout + ground rings (macbook, 2026-07-06) — pending window

Restructured the scene from two regions into **labelled group boxes** (`graph.rs` `GroupBox` +
`BoxKind`), laid left→right, each vertically centred, floor sized to their union:
- **Movement** box — the connected locomotion states (idle/walk/run/strafe/crouch, groups
  core/walk/run/crouch/misc) placed by force-directed relaxation, fitted into a square box.
- **Jump / Attack / Reactions** boxes — one per triggered sequence (grid layout). **Dame/Death came
  back** into the graph (their own Reactions box) rather than being dropped to waiting.
- **In-Place / RootMotion** waiting boxes — the unused clips split by the `RM/` prefix.
- **Ground rings:** a gold circle under **every** character (always). The **selected** node's ring
  turns orange/rust and reveals only its connections (click-to-select unchanged); the HUD shows the
  selected node's label **and which box it's in** (`box_label_of`).
- Box outlines colour-coded by kind (`box_color`). Tests updated: `scene_has_group_boxes_and_
  everything_on_floor`, reactions-in-graph guard. `cargo test/clippy` clean (5 tests).
- **NOT yet:** in-world **box labels** (`GroupBox.label` is shown in the HUD but not floating over the
  boxes — 3D text is deferred); the **glow** effect on rings (flat line-loops for now); sequence
  ordering within triggered boxes (grid, name-sorted, not Start→Loop→End order).

### Deferred decision (user-raised): reorganise the clip `.json` files to match these groups
The user suggested arranging the animation `.json` files on disk into the same grouping. **Not done**
— it touches shared assets (`examples/flicker-animation/assets/clips/**`) that `flicker-animation`
also loads via the recursive `clips/` loader (In-Place bare / `RM/` namespaced), so it needs its own
scoped change + confirmation. Flag, don't drift.

## 7h. Render-to-texture primitive (flicker-render) + paperdoll panel (macbook, 2026-07-06)

**New shared engine capability** — offscreen render targets — added to `flicker-render` (used by all
examples, workspace builds clean). Foundation for the user's four use cases (paperdoll · circular
minimap · in-world 2D/minigame panels · camera-locked HUD surfaces): all are "render a sub-scene into
an offscreen texture, then display that texture."

- **API** (`crates/flicker-render/src/renderer.rs` + `texture.rs::from_view`):
  - `create_render_target(w, h) -> RenderTargetHandle` — offscreen colour (swapchain format, so every
    pipeline renders into it unchanged) + private `Depth32Float`; the colour texture is registered in
    the texture store.
  - `target_texture(handle) -> Option<TextureHandle>` — sample the result via the normal sprite /
    billboard / mesh paths.
  - `render_to_texture(target, clear, |r| { … set_camera / draw_* … })` — runs a **self-contained
    sub-frame** into the target and submits it. `clear=[0.0;4]` = transparent cut-out.
  - **Contract:** call `render_to_texture` **before** queuing main-frame draws (it resets the per-frame
    queues on entry/exit). Volumetric-in-offscreen unsupported (samples the main depth). Exported:
    `flicker::render::RenderTargetHandle`.
- **Implementation:** `end_frame` was split into `prepare_frame(size)` (`&mut self` — uploads camera/
  scene uniforms + `prepare()`s all pipelines for a given render size) and `encode_passes(&self, color,
  depth, clear)` (immutable — the two passes), so the swapchain frame and offscreen targets share one
  encode path. All 6 `flicker-render` tests pass; clippy clean.
- **Paperdoll (the chosen first use case), wired into the field viewer:** selecting a node renders that
  character alone (`model.world`, its live pose slice, an orbit portrait camera, transparent clear)
  into a 512² target, drawn as a bottom-right panel over a dark backdrop. Poses are now rebuilt at the
  top of `render()` (both the paperdoll + the field read them). `cargo build/test/clippy
  -p flicker-field-viewer` clean (5 tests). **Windowed check pending** (does the portrait frame/animate
  right — camera yaw/pitch/distance may need tuning).
- **NOT built (next, on this primitive):** the circular-mask minimap (needs an overhead/ortho camera +
  a display-time alpha mask on the sprite), world-space 2D panels (draw the target texture on a world
  quad), and a headless test (the `Renderer` needs a window, so RTT is proven by build + windowed check
  for now).

## 7i. Interactive PiP (state-machine driven) + pack box + input mode (macbook, 2026-07-06)

Re-aligned to the **original spec** (the PiP is a *controllable* character, not a static render of a
clicked node):
- **Interactive PiP:** the field viewer now owns a `StateMachine` (built from `Katanami.pack.json`,
  like `flicker-animation`). In **Character** mode WASD/Shift/C/Space/F/H/X drive it; the PiP panel
  (bottom-right, via the render-to-texture paperdoll) shows that character animating with crossfade
  blending (`pip_palette()` = SM clip/tick + `blend_local_poses`). The field's 91 clips keep looping
  independently.
- **Active-node highlight:** the graph node for the SM's current state glows **green** and moves
  through the graph as you drive the character (`active_node()` matches `current_state_name`). Click-
  select (orange ring + connections) is unchanged and independent.
- **Input mode (Tab):** `InputMode::{Camera, Character}` replaces the orbit/fly toggle. Character =
  orbit camera + WASD→SM (default); Camera = fly camera (WASD flies). Resolves the WASD conflict.
- **Pack box:** the graph's action sub-boxes (Movement FR + Jump/Attack/Reactions grids) are now
  **nested inside one enclosing `BoxKind::Pack` box** (prominent blue, labelled with the pack name) —
  the box represents the `.pack` file. Reactions (Dame/Death) are **back in the graph** (their own
  sub-box). The waiting table (In-Place/RootMotion) sits to the right, outside the pack. `graph::build`
  now takes `pack_name`. This is the seed of the "many pack-boxes → animation browser/editor" vision;
  the layout is read from the pack (`load_pack`), not hardcoded — the movement layout force-directs the
  real transitions; `group_of` (name-prefix) only assigns sub-boxes.
- `cargo build/test/clippy -p flicker-field-viewer` clean (5 tests). **Windowed check pending.**
- **Open tuning / next:** PiP portrait camera angle; whether the field's active node should *sync its
  pose* to the SM (currently loops independently, just highlighted); in-world box **labels** (pack name
  shown in HUD, not yet floating over the box — 3D text still deferred); glow on rings; the `.json`
  file reorg (still deferred — shared assets).

### 7i-follow-ups landed (macbook, 2026-07-06)
- **Exit arrows:** the active node now draws **arrows to every reachable state** (`graph.out_edges` per
  node + `graph.any_targets` for hit/die-from-anywhere; `arrow_segments` shaft+head on the floor). As
  you drive the character the graph "lights up" along the transitions. Green ring + yellow-green arrows.
- **Camera persistence:** `OrbitCam` gained a movable `target` + `sync_from_camera`; Tab Camera→Character
  now snaps the orbit to the flown view (target ahead of the fly cam) instead of reverting to the centre.
  Both Tab directions are now continuous.
- **Crouch-idle "challenge mode" (LANDED):** the user chose to **overload Shift** — sprint normally,
  *stillness* while crouched (a decision-matrix combo). Added `Trigger::CrouchStill` (`crouch && run`,
  snake_case `crouch_still`) to `Alpha/flicker-skeletal/src/state.rs` (reuses the existing `crouch`+`run`
  inputs — no new key), and a **`CrouchIdle` state** to `Katanami.pack.json` with priority-5 `crouch_still` edges from
  Crouch/Crouch_Move/Crouch_Move_L/Crouch_Move_R (beats the move edges), exiting on `run_stop`→Crouch /
  `crouch_stop`→Idle. **CLIP IDENTITIES (corrected — my earlier labels were backwards; the *behaviour*
  is right, do not change the pack):** the **`Crouch` clip = the STILL crouch** (a static hold); **`RM/
  CrouchLoop` = the MOVING crouch-idle loop.** Assignment: the default `Crouch` state plays
  `RM/CrouchLoop` (moving idle); Shift → `CrouchIdle` plays the still `Crouch` clip — so Shift reads as
  "become still," which is what the user wanted. Both crouch clips are now used (neither an orphan).
  **Shared** with `flicker-animation` (both viewers gain it); all its tests + skeletal (14) +
  field-viewer (5) pass. NOTE: `RM/CrouchLoop` (now the *default* crouch) is root-motion, so in the PiP it may drift
  (the deferred in-place-display fix); the In-Place `Crouch` clip is the drift-free alternative if wanted.
  Macos aside: Option = Alt (`Key::LeftAlt`/`RightAlt` exist) if a dedicated modifier is ever preferred.

### Visual enhancements: night sky + glass floor + ground clouds (LANDED, macbook, 2026-07-07)
Reuses existing pipelines — no new renderer work:
- **Celestial night sky:** `night_scene()` (sun below horizon, cool moonlight, dark starry palette) +
  `renderer.draw_sky()` each frame → the procedural sky pass draws stars/Milky Way/moon behind the field.
  The PiP portrait sets its own brighter `pip_scene()` inside its `render_to_texture` closure so it stays
  well-lit against the dark field.
- **Glass floor:** the floor is now drawn **translucent** (`MeshDrawOptions.tint = [.., 0.42]` — the mesh
  shader passes `tint.a` through, and the mesh pipeline is `ALPHA_BLENDING`), so the night sky shows
  through it. Characters still depth-sort correctly (floor writes depth).
- **Ground fog → VOLUMETRIC (the real fix, new shared render feature):** the quad/billboard clouds
  couldn't composite right (order-independent transparency is the hard part) and floor-aligned quads went
  through the textured **alpha-*test*** path (hard discs). Replaced with a proper **volumetric ground-fog
  pass** in `flicker-render` (`pipeline_ground_fog.rs` + `shaders/ground_fog.wgsl`): a fullscreen
  **raymarch** of a horizontal fbm-noise fog slab, **depth-aware** (samples the scene depth so geometry
  occludes it), premultiplied-alpha "over". Because density is integrated *along the ray*, overlapping fog
  self-composites correctly and edges fade continuously — no spawn/wrap/pop. Mirrors the volumetric-disk
  pipeline (fullscreen, depth-bound, never writes depth; `set_depth` on resize; headless shader-compile
  test). API: `Renderer::set_ground_fog(GroundFog { color, bottom, top, density, noise_scale, coverage,
  wind, time, height_power, bounds_min, bounds_max, edge_fade })` (exported `flicker::render::GroundFog`),
  rendered in the overlay pass right after the volumetric disk. **Localised (not infinite):** the shader
  fades density to 0 across an `edge_fade` feather at the `bounds_min..bounds_max` XZ rectangle edges, so
  the fog stays over the field floor instead of reaching the horizon. The field viewer sets bounds =
  `graph.floor_min/max`, `edge_fade = unit·6`, and a calm `density 0.5` / `coverage 0.72` / dim cool colour
  (the first pass' bright dense fog washed the models out; `coverage` amps the *amount* of fog without
  touching the transparency, which is `density`). **Layering:** models + rings/boxes/arrows at `foot_y`; fog slab from
  `floor_y+0.05·unit` to `foot_y−0.05·unit`; glass floor at `foot_y−0.6·unit`. The field viewer drives it
  with a cool moonlit tint + gentle wind (drift = `time`). Removed the quad-cloud code
  (`CloudPuff`/`cloud_texture`/`cloud_quad_verts`/`hash01`). Reusable atmospheric-fog primitive. All
  flicker-render tests (7, incl. `ground_fog_pipeline_compiles_shader`) pass; workspace builds; clippy
  clean. Tunables: `density`, `coverage`, `noise_scale`, `wind`, `height_power`, fog band, colour.
- **Soft-alpha textured-mesh mode (also landed, now unused by the field viewer but kept):** the
  textured-mesh pipeline gained a **soft-alpha blend mode** gated on `PerDraw.flags.z` (`mesh_textured.wgsl`):
  mode 1 skips the hair-card cutout and outputs `alpha = texel.a × tint.a`; mode 0 (default) is the
  unchanged cutout, so characters/hair are untouched. `pipeline_mesh_textured::push(+soft)` → new
  `Renderer::draw_textured_mesh_soft(...)`. Reusable for ground decals / fog cards even though the fog is
  now volumetric.
- Tunables: floor tint/alpha, moonlight brightness, cloud count/size/speed/tint. `cargo build/test/clippy`
  clean (5 tests).

### Random hit reactions (LANDED, macbook, 2026-07-07)
Wired the 3 orphan Dame clips (`Dame_02`, `Dame_1`, `Dame_2` — plus `Dame_01`) as states in
`Katanami.pack.json` (`looping:false`, `next:Idle`), and **H now plays a random one from the group.**
Design: the **driver owns the randomness, the state machine stays deterministic** — added
`StateMachine::force_state_by_name(name) -> bool` (a "game commands this reaction" escape hatch;
crossfades with the default blend) to `Alpha/flicker-skeletal/src/state.rs`. The field viewer keeps a
tiny xorshift (`next_rand`) + a `dame_states` list (Dame_* connected nodes) and, on the H edge, forces
a random Dame — it **no longer feeds `hit` to the SM** (so the single `hit`→Dame_01 any-edge is bypassed
in the viewer; `flicker-animation` still uses it → single reaction there). The 3 clips are no longer
orphans (now in the Reactions box). Tests: skeletal 15 (incl. `force_state_by_name` guard), animation 5,
field-viewer 5; clippy clean. NOTE: `Dame_1`/`Dame_2` (dur 81, from the `Death_new` folder) are
**confirmed by the user as heavier "hit harder" reactions** (a right-cross knockback), NOT deaths —
keep them; the mix of quick flinches (41) + harder knockbacks (81) is intentional variety.

### Design concept (RECORDED, do NOT build unprompted): the context-sensitive "modifier"
The crouch-still work generalizes: **Shift is one "modifier" input whose verb is chosen by the
character's current state** — a decision matrix, one physical input serving many actions:
- **Walking** → run (sprint) — default.
- **Crouched** → stillness (crouch-idle). *(built — `Trigger::CrouchStill`)*
- **Falling** → prone → enables gliding. *(idea)*
- **Prone / behind door cover** → peek (up / lean). *(idea)*
Cheap because `state.rs::satisfied` already evaluates input **combos** (the `Move*`/`Run` triggers
are `input && input`); a context modifier is just per-state `modifier && <context>` trigger variants,
so the *state* selects the verb. Only crouch-still is implemented; the rest are future ideas — **do
not build them without being asked.** Also in local memory `context-modifier-input`.

## 7g. NEXT — glow rings, in-world labels, cloud, then Slices 4–5

- **Rings + glow:** a golden, semi-translucent ring with a glow effect on the ground under each
  character (per the user's vision). Connected vs orphan may key ring style off `Node.connected`.
- **Selection:** pick a node (ray vs node position) → its ring turns orange/red/rust; enables
  "select the connections" between rings.
- **Bonus — below-ground cloud:** a cloud effect streaking a bit below the ground the characters
  stand on ("walking on glass"). Candidate: the volumetric pipeline or a scrolling textured plane.
- **Slice 4 (drive it):** state machine marks an *active* node → highlight + camera follow.
- **Slice 5 (polish):** pack hot-reload, skeleton↔skinned toggle, PiP player-view. See §2.
- **Keybindings note (user):** a Lua-driven keybindings UI (not yet built) will eventually own all
  this input; functionality over UX while building — hardcoded keys are fine for now.
