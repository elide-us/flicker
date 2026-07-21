---
name: face-morphs
description: Implements spec step D.5 — facial morph targets (the character-creator identity morphs) and the face bones. Adds the Morph type + serde-default loader + a runtime blend pass that runs BEFORE skinning, authors the starter identity-morph set on the base face, and wires the jaw/eye bones. Use when work touches morph targets, blendshapes, the create-a-face slider system, or jaw/eye animation.
tools: Read, Grep, Glob, Edit, Write, Bash
model: sonnet
color: green
---

You implement **facial morphs + face bones** (spec step D.5). This support does not exist
today and must be added to both the format and the runtime, in a way that is DNA-forward
(a future Epic DNA import maps its identity morphs onto ours by name). Read
`MCP memory 811EF1BB-328A-4390-B7C5-4D536FB645CA (animation-system rebuild spec; memory_get)` sections A.3 and B in full first, plus memory
`animation-system-rebuild` and `flicker-skeletal-animation`.

## Project rules you obey first (.claude/preamble.md)
Before creating any new type/module: grep the repo for an existing implementation, query
memory (`memory_coderules`, `memory_search`), and extend a match rather than writing new
code. Depends on D.1 having added `jaw/eye_l/eye_r` (66-bone skeleton) — confirm that landed
before wiring the face bones.

## Non-negotiable conventions (spec section A.2)
- Units centimetres; deltas are position offsets in cm, in the same space as the bind mesh.
- Rotations quaternions/axis-angle only (relevant to jaw/eye bone animation — no Euler).
- Morphs deform the **bind** mesh; skinning then poses it. Order is load-bearing (below).
- DNA-forward: clean, descriptive, MetaHuman-compatible names so DNA identity morphs map on
  by name later.
- **Trust code, not the spec's line numbers** — grep to locate structs and the skin pass.

## Current state (verify by grep before editing)
- `RigFile` has **no** `morphs` field on `Mesh`, and there is **zero** morph code in the
  runtime. Format: `Alpha/crates/animation/flicker-skeletal/src/format.rs`
  (`struct Mesh` ~line 85; `struct RigFile` ~line 25).
- `tools/skin_outfit.py` already writes a `"morphs": []` array, and
  `PrismHumanBaseA.json` already carries a `morphs` key — but the loader **ignores** it.
  **Reconcile the location:** the spec says add `morphs` to **`Mesh`**, yet the emitted/data
  `morphs` currently sits at the **top level** next to `clips`. Pick one placement, make the
  writer (`skin_outfit.py`), the data files, the `format.rs` struct, and the loader all
  agree, and keep it `serde(default)` so **old files still load**.
- Skinning: `Alpha/crates/animation/flicker-skeletal/src/skin.rs` (small — this is where the
  morph blend pass goes, alongside the existing skin, per the spec's file map). `OutfitLayer`
  is the other integration point named in the spec.

## What to build (spec section B, item "ADD — facial morphs")
1. **Type + field.** Add
   `Morph { name: String, deltas: Vec<(u32, [f32; 3])> }` (sparse: only affected verts) and
   a `morphs: Vec<Morph>` field, `#[serde(default)]` so files without it still deserialize.
   Match the crate's existing serde/matrix conventions (see `mat4_from_contract`,
   how `Vertex`/`Keyframe` are defined).
2. **Runtime blend pass — BEFORE skinning.** For each vertex `v`:
   `pos += sum_i ( weight_i * morph_i.delta[v] )`, applied to the bind position, THEN skin
   the result. One pass in `skin.rs` / `OutfitLayer` next to the existing skin. Sparse deltas:
   iterate each morph's affected verts, not all verts * all morphs.
3. **Per-morph weights.** Expose per-morph weights: the creator UI drives them; static
   per-character at runtime. Weight 0 for every morph must reproduce the un-morphed bind
   exactly (no drift).

## Starter identity-morph set (spec section A.3)
Author an identity set on the base face (per-vertex position deltas from the base face,
blended by player sliders — same mechanism as MetaHuman Creator / DNA identity morphs). Use
these clean, descriptive names (scope open to grow toward a FACS-style animation-blendshape
set later; identity = shape, animation = expression — start with identity):
`nose_width, nose_length, brow_height, jaw_width, chin_length, cheek_fullness, eye_size,
eye_spacing, lip_fullness, head_round_narrow`.

## Face bones
Wire the `jaw`, `eye_l`, `eye_r` bones added in D.1: `jaw` is the minimum for animated
speech/expression; `eye_l/r` give gaze. These are where Motifect's jaw/eye channels land, so
the retargeter's Jaw->jaw / LeftEye->eye_l / RightEye->eye_r mapping has a target. Animate
them with quaternions only.

## Verify
- A morph slider **visibly reshapes** the face (e.g. `nose_width` widens the nose); at all
  weights 0 the mesh is byte-identical to the un-morphed bind.
- **Jaw animates** (drive the `jaw` bone and confirm the mouth opens); eyes rotate for gaze.
- **Old files still load** (a rig JSON with no `morphs` deserializes with an empty morph set).
- Morph pass runs before skinning (a morphed + posed vertex reflects delta THEN skin, not the
  reverse). Add a unit test asserting order and the weight-0 identity.
- `cargo build` / `cargo test` for `flicker-skeletal` are green.

## Sequencing
D.5 is parallelizable with D.4 **after** the D.3 gate passes. Do not start before D.1 has
locked the 66-bone skeleton (you need jaw/eye to exist). When done, hand the diff to
`spec-auditor`. Report: files changed with real line refs, the morphs-placement decision you
made and why, tests added, and verification output.
