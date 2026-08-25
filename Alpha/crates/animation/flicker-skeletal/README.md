# flicker-skeletal

The CPU-authoritative skeletal-animation runtime: it owns the `flicker.rig` file
format (the one internal format for a character, prop, garment, or clip library),
loads it into a ready-to-play `Model`, samples clips into posed bone transforms,
skins the mesh, swings its cloth, and runs the animation/combat **state machine**.
GPU-free and window-free — a renderer turns the transforms and skinned vertices this
crate produces into draw calls; nothing here touches wgpu, and nothing here reads the
input bus. It sits in the `animation` cluster and is the load-bearing dependency of
every crate that shows or edits a character.

> Design of record — why it is shaped this way, decisions, history — lives in the
> project's MCP memory, not here. This file documents how to use the crate.

---

## Where it sits

- **Builds on:** `flicker-core` (the `compression` seam — every file is read by its
  *logical* path, `.gz`-first with a raw fallback, so a mounted `package.flk` and a
  loose dev tree read identically), plus `glam` (matrices/quats), `serde`/`serde_json`
  (the wire format), `anyhow` (load errors).
- **Used by:** `flicker-content` (bakes rigs into this format), `flicker-mechanics`,
  `flicker-flight`, and the scene/editor crates `flicker-assetpipeline` (Clayworks),
  `flicker-loomforge` (the pack/TAE editor), `flicker-controllertester` (the golem
  stage), and `flicker-pocclusters`. Each takes the `format`/`pose`/`skin` layers to
  render or the `state` layer to drive.
- **Reads from the content tree:** paths are supplied by the caller, not hardcoded.
  In practice the caller points the loader at:
  - `Alpha/content/package/characters/<Name>/` — a character bundle: one `*.rig.json`
    (`.gz` at rest) carrying the mesh + skeleton, optionally a `clips/` subtree, and a
    `*.pack.json` combat/state graph beside it.
  - `Alpha/content/package/retarget/clips/` — the shared clip library a base body
    borrows (`load_dirs` with the body dir + a clip dir; tracks resolve by bone name).
  - Missing path / empty dir → `load_dir`/`load_dirs` **error** with the dirs it
    searched (`"no .json rig/clip assets found in …"`); a rig with zero bones errors
    `"rig skeleton has no bones"`. Loads never silently succeed on nothing.

This crate defines the concepts other crates author against; the human authoring
guides live beside the content: `Alpha/content/README.md` (the material/texture
standard) and the Clayworks/Loomforge editors. The joint-placement rigging guide and
all design rationale live in MCP.

---

## Vocabulary (flicker words used below)

| Term | One-line meaning |
|---|---|
| **rig** (`flicker.rig`) | The one self-describing JSON asset format. Which sections are populated *is* the asset type: character = skeleton+mesh+skin(+clips); prop = mesh+attach; outfit = reduced-skeleton+mesh+skin+cloth; clip library = skeleton+clips. |
| **clip** | One baked animation: per-bone tracks of dense, per-tick `T/R/S` keyframes. |
| **track** | One bone's keyframes within a clip, keyed by bone **name** (not index). |
| **pack** (`flicker.pack`) | The combat/animation **state graph** + TAE timeline, authored as a *separate* file so the rig stays a purely mechanical FBX-derived atom. |
| **bind / inverse-bind** | The rest-pose matrix / its inverse; skinning maps a bind vertex to its posed position with `global × inverse_bind`. |
| **source space vs engine space** | Files are authored **Z-up, centimetres** (source). The engine renders **Y-up, metres**; the flip is one `Model::world` matrix applied at draw, never baked into bone data. |
| **retarget** | Play a shared clip on a rig of *different* proportions by rebasing translation onto this rig's own rest, applying the clip's rotations verbatim. See the invariant below. |
| **tick** | The fixed 60 Hz step the clips are baked to and the state machine advances on. |
| **TAE timeline** | Tick-stamped combat metadata on a state (hitbox windows, i-frames, cancel/parry windows, footsteps, sfx). |
| **socket / attach vs attach_points** | `attach` = how *this* asset mounts onto a body (a prop's grip). `attach_points` = where *this* body offers to hang props (`hand_r`, `holster_l`). Inverses of each other. |

The **canonical skeleton is 67 bones** (design of record in MCP). Note: this crate's
loader is deliberately **bone-count-agnostic** — it loads whatever a rig declares and
resolves clips by name, so an outfit's reduced skeleton and a full body both load. The
`67` canon is a `flicker-content` baseline constant, enforced upstream at bake/conform
time, *not* by this loader (the only mention here is one `#[ignore]`d integration test).

---

## The format at a glance

A minimal character `*.rig.json` (elided; real files carry hundreds of thousands of
verts):

```json
{
  "format": "flicker.rig", "version": 2,
  "source": { "source_axis": "Z_up", "source_unit": "cm", "textures": ["Body_BaseColor.png"] },
  "skeleton": { "bones": [
    { "name": "root",   "parent": -1, "local": [16 floats], "inverse_bind": [16 floats] },
    { "name": "pelvis", "parent":  0, "local": [16 floats], "inverse_bind": [16 floats] }
  ] },
  "mesh": {
    "vertices": [ { "p":[x,y,z], "n":[x,y,z], "uv":[u,v], "joints":[0,0,0,0], "weights":[1,0,0,0] } ],
    "indices": [ ... ],
    "submeshes": [ { "material": 0, "start": 0, "count": 36 } ],
    "materials": [ { "name":"body", "base_color":"Body_BaseColor.png", "orm":"Body_ORM.png" } ]
  },
  "retarget": false
}
```

A clip keyframe uses **uppercase** `T`/`R`/`S` (translation / rotation `[x,y,z,w]` /
scale) plus lowercase `t` (tick); a whole clip file is a rig file whose `clips[]` is
populated and whose `skeleton` is a redundant copy used only for retarget rebasing.

### Load-bearing invariants — the caller contract

These are **acceptance criteria**, not background: anything producing a `flicker.rig`
(the Blender exporter, `flicker-content`'s baker, a test fixture) must satisfy every
one, and anything consuming a `Model` may rely on every one. Verify each, not the
subset you happened to reason about — a partial miss (a dropped V-flip, a stray
transpose) passes bone-frame tests and still ships a scrambled character.

| # | Invariant | Where it bites |
|---|---|---|
| 1 | **Matrices are 16 floats, row-major storage / FBX row-vector convention** — translation is in the LAST ROW (`m[12..14]`). Decode with `Mat4::from_cols_array` and **NO `.transpose()`**: reading row-major floats as columns yields the correct column-vector matrix. A double-transpose is the classic skinning-explosion bug. | `mat4_from_contract` (format.rs:487). Applies to `local` + `inverse_bind` only; clip `T/R/S` are decomposed values, unaffected. |
| 2 | **Units cm, source Z-up.** The loader scales ×0.01 when `source.source_unit == "cm"` and rotates −90° about X when `source.source_axis == "Z_up"` (both case-insensitive), folded into `Model::world`. The flip is **never baked into bone data**. | `load_dirs` framing (format.rs:670–693). |
| 3 | **UV `v → 1 − v` (top-origin).** The exporter stores flipped V; a loader/baker that forgets it renders a scrambled texture atlas over correct geometry. | Stored in `Vertex::uv`; the crate carries it verbatim — producers must pre-flip. |
| 4 | **Weights are 4-influence, zero-padded, normalized to 1.0.** Skinning sums the four `joints`/`weights` slots and skips zero-weight ones. | `skin::skin_morphed` (skin.rs:61). |
| 5 | **Clip tracks are keyed by bone NAME** — share-by-name is the whole retarget contract; clip bone order need not match skeleton order. Unresolved names are collected, never fatal (see Sharp edges). | `resolve_clips` (format.rs:559). |
| 6 | **A synthesized `root` bone at the feet is bone 0.** Root keeps the clip's translation (root motion); every other bone's translation is rebased under retarget. | `pose::sample_local_poses` root branch (pose.rs:38). |

### The retarget contract (the recurring "lies on its side" class)

When `Model::retarget` is true, `sample_local_poses` computes each **non-root** bone's
translation as `this_rig_rest + (clip_T − source_rest)` — it keeps the clip's *animated
translation delta* but rebased onto this rig's own proportions, and applies the clip's
**rotations verbatim**. For a constant-offset bone (a limb) `clip_T == source_rest`, so
it collapses to the rig's own rest offset (proportions preserved); for a bone that
truly translates (the pelvis's hip sway/bob) the delta survives at this rig's hip
height. `source_rest` is the clip file's own skeleton rest for that bone; it falls back
to the target rest (delta 0) when the clip lacks the bone.

Because clip **rotations are absolute (applied verbatim)**, a body driven by the shared
clip library must be **conformed to the canonical skeleton** first — a bind whose
pelvis frame is 90° off makes every clip rotate the whole body onto its side. That is a
bind-vs-canonical mismatch every time, never a camera/`world` bug. (The conform step is
`flicker-content`'s job; this crate is where the symptom shows.)

> Naming caveat: `RigFile::retarget`'s doc comment calls this "rotation-only" — it is
> not literally rotation-only (translation is *rebased*, not dropped). Read it as
> "rotations verbatim, translation rebased." See finding #2.

---

## Public API

### `format` — the format contract + loaders

**Loaders** (all gz-transparent via `flicker-core`):

| Item | For | The one thing to know |
|---|---|---|
| `load_dir(dir) -> Model` | Load a character bundle: pick the rig, resolve all clips against it. | Recurses `dir`. Rig authority = the file with the **most mesh vertices** (tie-break by bone count) — the dense mesh wins over skeleton-only clip files. |
| `load_dirs(&[dir…]) -> Model` | Same, across several dirs — a base body borrowing another dir's clip library. | Same authority rule across all dirs. Clips sorted by name for a stable cycle order. |
| `rig_bones(&RigFile) -> Vec<Bone>` | Decode one rig's skeleton to runtime `Bone`s (matrices converted) without a disk round-trip. | The in-memory seam — Clayworks builds a playable rig straight from the baker's output value. |
| `resolve_clips(&RigFile, &[Bone], rm_namespace) -> Vec<ResolvedClip>` | Resolve a file's clips against a skeleton by bone name, carrying `source_rest` for retarget. | `rm_namespace=true` prefixes names `RM/…` (the RootMotion library convention). |
| `load_mesh(path) -> Mesh` / `load_mesh_with_attach(path) -> (Mesh, Attach)` | Load a static prop (geometry only; bones/clips ignored). | `_with_attach` also returns the folded-in mount record — no `fits.json` sidecar. |
| `load_outfit(path, &base) -> Mesh` / `load_outfit_with_attach(path, &base) -> (Mesh, Attach)` | Load a reduced-skeleton garment and remap its joint indices into `base`'s index space by bone name, so it skins with the base pose palette. | A bone absent from `base` collapses that influence to root (index 0) with an `eprintln!` warning. A file with no skeleton block is returned unchanged. |

**Wire types (verbatim `flicker.rig`):** `RigFile` · `Source` · `Skeleton` ·
`BoneRaw` · `Mesh` · `Submesh` · `Material` (`base_color`/`normal`/`roughness`/
`metalness`/`ao`/`emit`/`orm` basenames + flat `color`) · `Vertex` · `Morph` /
`MorphDelta` (create-a-face identity morphs) · `Cloth` / `ClothRegion` / `ClothChain` /
`ClothBind` / `ClothParams` · `Attach` (mount; `slot` is a serde alias for `socket`;
absent block defaults to identity quat + unit scale, not all-zeros) · `AttachPoint` ·
`Collision` / `CollisionVolume` / `CollisionShape` (`sphere`/`capsule`/`box`, tagged by
`kind`) / `CollisionRole` (`physics`/`hitbox`/`attach`, default `physics`) · `Clip` /
`Track` / `Keyframe`.

**Engine (resolved) types:** `Bone` (matrices decoded to `glam::Mat4`) ·
`ResolvedTrack` (carries `source_rest`) · `ResolvedClip` (carries `unresolved` names) ·
`Model` (the assembled, ready-to-play value: `bones`, `clips`, `mesh`, `source`,
`world`, `orbit_radius`, `retarget`, `attach`, `collision`).

> `attach_points`, `collision`, `emit`/`orm` and the create-a-face `morphs` are
> **first-class schema the format carries ahead of the runtime that reads it** — some
> sections are consumed only by a later slice (collision runtime lives in the
> `mechanics` cluster). They are populated and round-tripped now so content can carry
> them — not dead surface.

### `pose` — forward kinematics

| Item | For |
|---|---|
| `sample_local_poses(&[Bone], &ResolvedClip, tick, retarget) -> Vec<Mat4>` | Sample a clip at an integer tick → per-bone LOCAL transforms (untracked bones keep rest). Applies the retarget rebase above. Keys are dense/per-tick; the index clamps past the end (caller wraps `tick` within duration). |
| `global_transforms(&[Bone], &[Mat4]) -> Vec<Mat4>` | Accumulate LOCAL → GLOBAL in one forward pass (bones are stored parent-before-child). |
| `blend_local_poses(&[Mat4], &[Mat4], w) -> Vec<Mat4>` | Crossfade two LOCAL pose sets at the TRS level (lerp T/S, shortest-arc slerp R), `w` clamped `0..1` (no extrapolation). Used only while a transition ramps. |

### `skin` — CPU linear-blend skinning

| Item | For |
|---|---|
| `palette(&[Bone], &[Mat4]) -> Vec<Mat4>` | Build the skinning palette `global[b] × inverse_bind[b]`. |
| `skin(&Mesh, &palette) -> Vec<SkinnedVertex>` | 4-influence LBS of every vertex (source space; the `world` matrix is applied downstream, same as the skeleton lines). |
| `skin_morphed(&Mesh, &palette, &morph_weights)` | As `skin` but first blends facial morphs by weight (parallel to `mesh.morphs`; `&[]` is byte-identical to `skin`). |
| `apply_morphs(&Mesh, &morph_weights) -> Vec<[f32;3]>` | Just the reshaped bind positions — the create-a-face preview without skinning. |
| `SkinnedVertex { position, normal }` | The output vertex (UVs are static, read straight from the mesh). |

### `jiggle` + `cloth` — secondary motion

Animation-agnostic, deterministic (no rng, no wall-clock), dt-clamped so a frame hitch
can't explode a chain.

| Item | For |
|---|---|
| `jiggle::JiggleChain` (`new`, `step(anchor, driver_rot, dt)`, `positions`, `len`, `is_empty`) | One pinned verlet chain — the reusable core for a necklace/cord/hem/sleeve. `pos[0]` is pinned to the driver bone; free segments swing, spring toward rest, and lag. |
| `jiggle::JiggleParams` (`gravity`, `stiffness`, `damping`, `iterations`, `max_dt`) | The physical dials. Default gravity `-980` cm/s². |
| `cloth::ClothSim` (`build(&Cloth, &verts, &bones)`, `update(&palette, dt, &mut skinned)`, `is_empty`) | Per-garment dynamic cloth: bound verts are *positioned by* their chain (they drape off the modelled pose), overwriting only bound verts in the skinned buffer. A region whose anchor bone is missing is skipped with an `eprintln!`. |

`ClothParams` (the wire form in `format`) mirrors `JiggleParams` as plain arrays; its
default gravity is `-600` (a lighter garment feel), so the two default sets differ by
design.

### `state` — animation/combat state machine + TAE timeline

Sits **on top of** `pose` — it decides which clip plays at what tick; `pose` turns that
into transforms. Advanced on the fixed 60 Hz tick.

| Item | For |
|---|---|
| `StateMachine::build(&StateMachineDef, &[ClipRef]) -> Result` | Build a runnable machine; resolve state/clip/next names to indices. Unknown names → `warnings()` (not fatal); only a missing `initial` errors. |
| `.tick(&Inputs) -> TickReport` | One atomic tick: advance the play-head, fire crossed events, evaluate transitions (any-state → per-state → `next`). |
| `.advance(dt_secs, &Inputs) -> TickReport` | Accumulate a frame's `dt` into whole ticks (capped at 8/frame so a long frame can't spiral). |
| `.blend() -> Option<BlendView>` | The in-flight crossfade (outgoing clip+tick and an eased 0→1 weight); the caller samples both clips and `blend_local_poses`. |
| `.force_state_by_name(name) -> bool` | Driver-commanded transition (e.g. a randomly chosen hit reaction) — keeps the machine deterministic, randomness in the caller. |
| queries | `current_state_name` · `current_clip` (`usize::MAX` = missing → rest pose) · `current_tick` · `current_duration` · `current_root_motion` · `warnings` · `reset`. |
| `load_pack(path) -> StateMachineDef` / `read_pack(path) -> PackFile` / `write_pack(path, &PackFile)` | Read just the graph / the whole file (header + `_note`) / save (gz at rest, additive-serde clean). |

**Authored (wire) vocabulary the pack catalogs:** `PackFile` · `StateMachineDef`
(`initial`, `default_blend_ticks`, `tick_rate_hz`, `any`, `states`) · `StateDef`
(`clip`, `looping`, `next`, `root_motion`, `stamina_cost`, `blocking`, `guard_angle`,
`transitions`, `events`) · `TransitionDef` (`to`, `on`, `window`, `priority`,
`blend_ticks`, `on_incoming`) · `TickWindow` · `Trigger` (below) · `EventDef` /
`EventKind` (`footstep`/`hitbox_active`/`iframe`/`cancel_window`/`parry`/`hyper_armor`/
`telegraph`/`sfx`/`equip`/`weapon_trail`) · `CombatMeta` (hitbox capsule + damage +
`effects` + `response_mask` + `parry_window_scale`) · `HitType`
(`slash`/`thrust`/`strike`/`sweep`/`grab`) · `Response` + `ResponseMask`
(default = all; exactly one = a **perilous** attack) · `EffectChannel` / `EffectSpec`.
Runtime outputs: `Inputs`, `ClipRef`, `TickReport`, `FiredEvent`, `ActiveWindow`,
`BlendView`.

**`Trigger` catalog** (what gates a transition — these are abstract gameplay inputs the
caller sets on `Inputs`, **never keys**; see Interactions): held — `move`, `move_stop`,
`move_forward`/`move_left`/`move_right`/`move_back`, `run`, `run_stop`, `crouch`,
`crouch_stop`, `crouch_still`; edges — `jump`, `attack`, `hit`, `die`; and `clip_done`.

> Most of `CombatMeta`/`EffectSpec`/`Response`/`on_incoming` is **authored-only today**
> — the machine fires/reports windows; a later mechanics slice acts on them (spawns
> capsules, applies damage). This is schema carried ahead of its runtime, not dead
> code. The one genuinely unfinished seam is the `Response`↔`Trigger` gap (finding #3).

---

## Interactions

- **Input signals:** none directly. This crate is **bus-free** — it never reads
  `flicker-input-core` and matches no keys or buttons. It consumes an abstract
  `Inputs` struct (booleans like `move_`, `attack`) that the *caller* populates; the
  caller is where `ActionSignal`s map to `Inputs`. So a scene's signal→input mapping is
  out of scope here (correct per the signal-level rule — the crate is trigger-agnostic).
- **What it hands other crates:** a `Model` (bones + resolved clips + mesh + `world`
  matrix + orbit radius), per-frame `Vec<Mat4>` pose/palette buffers, `Vec<SkinnedVertex>`
  skinned geometry, and a driven `StateMachine` reporting `FiredEvent`s/`ActiveWindow`s
  the renderer/gameplay consume. A renderer applies `Model::world` to both the skinned
  mesh and the bone lines so they register.
- **Threads / async:** none. Pure synchronous CPU; every op is a plain function call.
- **Files:** reads/writes `*.rig.json`(`.gz`) and `*.pack.json`(`.gz`) through
  `flicker-core::compression` (gz at rest, logical-path addressing).

---

## Gates

`source ~/.cargo/env && cargo test -p flicker-skeletal` → **38 pass, 1 ignored**.

| Test | Guards |
|---|---|
| `format::self_describing_sections_deserialize` | `emit`/`orm`, the `attach` mount (`slot`→`socket` alias), and tagged `collision` shapes/roles round-trip. |
| `format::legacy_rig_without_new_sections_defaults` | A pre-self-describing file still loads; absent `attach` defaults to identity/unit, not all-zeros. |
| `format::load_mesh_with_attach_reads_inline_attach` | A prop's inline mount is surfaced from its one file (folded `fits.json`). |
| `format::outfit_joints_remap_by_name` / `outfit_unknown_bone_pins_to_root` | Outfit joints remap into base index space by name; an unknown bone pins to root, never out of bounds. |
| `format::loads_canonical_base_a_with_face_group` *(ignored — real content)* | The reference rig loads with 67 bones and `jaw`/`eye_l`/`eye_r` parented to `head`. Run with `-- --ignored`. |
| `pose::retarget_keeps_rest_translation_for_nonroot` / `retarget_applies_translation_delta_for_moving_bone` | The retarget rebase: constant-offset bones keep rig rest; a translating bone keeps its delta at this rig's height; root keeps clip motion. |
| `pose::blend_hits_endpoints_and_midpoint` / `blend_clamps_weight_no_extrapolation` | TRS crossfade endpoints/midpoint; weight clamps (no extrapolation). |
| `skin::morph_blend_displaces_only_targeted_verts_scaled_by_weight` | Sparse morphs move only targeted verts; empty weights == `skin`. |
| `jiggle::hangs_straight_down_at_rest` / `anchor_is_pinned_to_the_driver` / `free_end_lags_then_settles` / `motion_decays_with_damping` / `deterministic` / `a_frame_hitch_does_not_explode` | The verlet solver: gravity hang, pinned anchor, lag, damping decay, bit-for-bit determinism, dt-clamp stability. |
| `cloth::region_drapes_off_the_rest_shape` / `anchor_move_carries_the_hang` / `parses_tool_json_and_builds` | Cloth drapes off the modelled pose, tracks the anchor bone, and builds from the tool's JSON. |
| `state::existing_packs_do_not_acquire_the_new_combat_fields_on_save` | **The additive-serde guard** — a save must not inject a single new combat default into a hand-authored pack. |
| `state::real_packs_round_trip_stably` / `pack_round_trips_and_preserves_note` / `combat_metadata_is_optional_and_round_trips` | Load→save→load is stable; `_note` and window/one-shot events survive; combat metadata is optional. |
| `state::response_mask_defaults_to_all_and_detects_perilous` / `reactive_transition_round_trips_by_name` | Mask default = all, one answer = perilous; `on_incoming`/`HitType` round-trip by name. |
| `state::move_and_stop_locomotion` / `directional_move_routes_by_held_direction` / `clip_done_auto_advances_via_next` / `timeline_event_fires_once` / `hitbox_window_reports_active` / `cancel_window_gates_the_combo` / `any_state_hit_interrupts` / `missing_clip_is_a_warning_not_a_panic` / `starts_in_initial` / `force_state_by_name_enters_target_and_ignores_unknown` / `blend_starts_ramps_and_clears` / `zero_blend_ticks_is_a_hard_cut` / `next_autoadvance_blends_by_default` | The state-machine behaviours: locomotion/directional routing, clip-done chains, timeline firing, hitbox/cancel windows, any-state interrupt, missing-clip warning, crossfade start/hard-cut/auto-advance. |

---

## Sharp edges

- **Two validation seams are silent unless the consumer reads them.** A clip track
  naming a bone the skeleton lacks is dropped into `ResolvedClip::unresolved`; a pack
  naming an unknown clip/state/`next` is dropped into `StateMachine::warnings()`. The
  crate does **not** emit either (no log/`eprintln`), so a mistyped name fails to
  *nothing* — that bone rides rest, or the state holds the rest pose — unless the caller
  inspects the field. (The outfit-remap and cloth paths, by contrast, *do* `eprintln!`
  on the same class of miss.) A missing `initial` state is the one hard error. Finding #1.
- **`load_dir`/`load_dirs` pick the rig by a heuristic.** The densest mesh (most verts,
  tie-break bone count) becomes the rig; everything else contributes clips. Point it at
  a dir with two comparable meshes and the pick is silent.
- **Magic path components decide clip handling.** A path containing a component literally
  named `clips` makes flat top-level clip files "legacy duplicates" that are silently
  skipped; a component named `RootMotion` namespaces those clips `RM/<stem>`. Rename the
  folder (`Clips`, `clip`) and behaviour changes with no warning. Likewise scale/axis
  key off `source_unit == "cm"` and `source_axis == "Z_up"` exactly — an unrecognized
  string silently means "unit scale / no rotation." Finding #4.
- **`retarget` requires a conformed bind.** A body playing the shared clips must be
  conformed to the canonical skeleton or it animates on its side (rotations are absolute).
  Not a bug in this crate — verify the bind, not the camera.
- **`current_clip()` / a state's clip can be `usize::MAX`** (missing clip) — the caller
  must fall back to the rest pose, not index the clip list with it.
- **Clip play-head clamps, it doesn't wrap.** `sample_local_poses` clamps `tick` to the
  last key; the *caller* wraps `tick` within the clip duration (the `StateMachine` does).
- **`Inputs` edges are caller-latched.** `jump`/`attack`/`hit`/`die` are edges the caller
  must set true only on the pressed tick; the machine does not debounce them.
