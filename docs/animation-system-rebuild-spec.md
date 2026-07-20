# Canonical Rig + Animation Spec — fresh-session handoff (2026-07-18)

This is the **authoring target and execution plan** for cleaning the skeletal-animation
system, bringing in the Motifect locomotion animations, and future-proofing the rig for a
character-creator face + an eventual Epic **DNA/RigLogic** upgrade. A fresh session can
execute the steps in §D top-to-bottom.

**Read first:** memory `animation-system-rebuild`, `multibody-rig-retarget`,
`flicker-skeletal-animation`; docs `flicker-multibody-rig-handoff.md`,
`flicker-animation-handoff.md`, `flicker-combat-animation-handoff.md`.

---

## 0. The one-paragraph frame

This is a **cleanup, not a rebuild.** The engine skeleton (`PrismHumanBaseA`, 63 bones) is
already the **UE-mannequin layout**, and the runtime (`flicker-skeletal`: `pose` / `skin` /
`state` / `jiggle` / `cloth`) is quaternion-based and topology-agnostic — **keep it.** What
we clean is the **data + conventions + import/retarget pipeline**, and we **add** the two
things that are genuinely missing: a **face group** (bones + morphs) and a **clean
retargeter** for the new animations. Everything stays **UE4-mannequin compatible** so it
round-trips to Unreal and a future DNA upgrade is *additive*, not a redo.

The Katanami character and its clip library are **retired** at the end (§D.8); nothing new
should depend on them.

---

## A. The canonical rig (the authoring target)

### A.1 Skeleton — UE-mannequin body + face group

Canonical = the current **`PrismHumanBaseA` 63-bone UE-mannequin skeleton**, unchanged,
**plus a face group**. Author every base model (A/B) and every outfit against this exact
skeleton, in an **A-pose bind**.

Groups (all present today unless marked **ADD**):
- **Spine/root:** `root, pelvis, spine_01, spine_02, spine_03, neck_01, head`
- **Arms (l/r):** `clavicle, upperarm, (upperarm_twist_01), lowerarm, (lowerarm_twist_01), hand`
- **Hands (l/r):** `thumb_01..03, index_01..03, middle_01..03, ring_01..03, pinky_01..03` — **keep the fingers; they are wanted, not filler.**
- **Legs (l/r):** `thigh, (thigh_twist_01), calf, (calf_twist_01), foot, ball`
- **Sockets:** `Weapon_L, Weapon_R` (attach points, never animated)
- **Face group — ADD, children of `head`:** `jaw`, `eye_l`, `eye_r` (→ **66 bones**).
  Use UE/MetaHuman-compatible names. Design this as an **open group**: later we can graduate
  toward the full MetaHuman facial bone set (the skeleton is variable-count, so additions
  need no format change). `jaw` is the minimum for animated speech/expression; `eye_l/r`
  give gaze; both give Motifect's jaw/eye channels somewhere to land.

**A-pose is correct** — UE4 Mannequin, UE5 Manny, and MetaHuman are all A-pose. Do **not**
author T-pose models. (Motifect ships T-pose; that gap is bridged in the retarget, §C.)

### A.2 Conventions (lock these; they are the "clean format")

| Concern | Rule |
|---|---|
| **Units** | centimetres, throughout. |
| **Source axes** | **Z-up** (source/rig space). The engine renders **Y-up**; the flip is **one** `Model::world` matrix, applied only at draw. Never bake axis flips into bone data. |
| **Motifect axes** | Motifect BVH is **Y-up** → convert Y-up→Z-up **in the retargeter**, baked into the emitted clip. Do **not** add a runtime axis hack. |
| **Rotation authoring** | **quaternions / axis-angle only.** No Euler-XYZ authoring anywhere (it gimbal-locks; see §D.6). Clip playback is already clean quats+slerp — keep it that way. |
| **World vs local frame** | **physics forces (gravity, wind) are WORLD-space; only skinning + attachment offsets are bone-local.** Never express "down" in a bone frame. (The cloth `down` bug is one violation; audit for more — §D.7.) |
| **Bind pose** | A-pose — the retarget reconciles against the **mesh's actual bind** (its real, pitched-foot A-pose), never an idealized reference pose (§C.2). *(The earlier flat-foot "RBP" reference was the toe-up/crossed-arm bug — superseded 2026-07-18.)* |
| **Frame timing** | fixed **30 fps**, **integer-tick** sampling (`tick_rate_hz`, `duration_ticks`). TAE iframe/hitbox windows key off exact ticks — never resample to a different rate; preserve source frames 1:1. |

### A.3 Facial morph-target convention (the character creator)

Face customization ("souls-like create-a-face") = **identity morph targets** (per-vertex
position deltas from the base face), blended by player sliders — the same mechanism as
MetaHuman Creator and DNA identity morphs. This is **DNA-forward**: a future DNA import maps
its identity morphs onto ours by name.

- A morph target = `{ name, [vertex_index → delta_xyz] }` (sparse; only affected verts).
- Author a **starter identity set** on the base face, e.g. `nose_width, nose_length,
  brow_height, jaw_width, chin_length, cheek_fullness, eye_size, eye_spacing, lip_fullness,
  head_round_narrow`. Keep names clean + descriptive; **scope open** to grow toward a
  FACS-style *animation* blendshape set later (identity morphs = shape; animation
  blendshapes = expression — start with identity).
- **This support does not exist yet** — see §B. It must be added to the format + runtime.

---

## B. The format contract (`flicker.rig`) — what's clean, what to change

Defined in `Alpha/crates/animation/flicker-skeletal/src/format.rs`.

**Clean / keep:** `skeleton.bones[]` (name/parent/local/inverse_bind, column-major
row-vector matrices — see `mat4_from_contract`), `mesh` (vertices p/n/uv/joints/weights,
submeshes, materials, `cloth`), `clips[]` (`tracks[].{bone,keys[]}`, `Keyframe{t,T,R,S}`,
`tick_rate_hz`, `duration_ticks`), the `retarget` flag (rotation-only playback — keep target
rest translations, apply clip rotations).

**ADD — facial morphs (does not exist today):** `RigFile` has **no** `morphs` field and there
is **zero** morph code in the runtime. `skin_outfit.py` writes `"morphs": []` but the loader
ignores it. To support the creator:
1. Add a `Morph { name: String, deltas: Vec<(u32,[f32;3])> }` type + `morphs: Vec<Morph>` to
   `Mesh` (serde-default so old files still load).
2. Add a runtime blend pass: `pos += Σ weight_i · morph_i.delta[v]` **before** skinning
   (morphs deform the bind mesh; skinning then poses it). One pass in `skin.rs` /
   `OutfitLayer` alongside the existing skin.
3. Expose per-morph weights (the creator UI drives them; static per-character at runtime).

**REMOVE (after the retarget lands, §D):** the Katanami import hacks that reconciled
Katanami's FBX axes. **STATUS (D.3, 2026-07-18): already gone.** A prior refactor removed the
runtime axis hacks — `pose::sample_local_poses` now applies clip quats directly with no flip.
The two lines the earlier draft named are **NOT hacks and must stay:** `format.rs:463-467`
`Mat4::from_rotation_x(-π/2)` is the **sanctioned** single Z-up→Y-up flip in `Model::world`
(the one draw-time flip §A.2 mandates), and the `Quat::from_rotation_y(π/2)` in `pose.rs`
lives inside the `blend_hits_endpoints_and_midpoint` **unit test** (a generic fixture).
Grep-verified: those are the crate's only two `from_rotation_*` calls; no Katanami
axis-reconciliation remains in the runtime. Clean retargeted data already plays without any
hack (Aaron confirmed the in-app walk). **No deletion required** — this REMOVE item is closed.

---

## C. The retarget (Motifect → our rig) — the load-bearing piece

**Source:** `Alpha/content/source/Motifect/Motifect_locomotion_complete_v1_0/` — 45 clips,
**BVH** (text, easiest) + FBX. Rig = generic/Mixamo-style **77 joints**, **Y-up, T-pose,
30 fps, cm**. Because BVH is text, the retargeter is a **pure-Python tool** (shape of
`tools/skin_outfit.py`), emitting `flicker.rig` clip JSON — **no Blender**.

### C.1 Bone name map (Motifect → UE-mannequin), mostly 1:1

```
Hips→pelvis   Spine1→spine_01  Spine2→spine_02  Chest→spine_03   Neck1(+Neck2)→neck_01  Head→head
LeftShoulder→clavicle_l   LeftArm→upperarm_l   LeftForeArm→lowerarm_l   LeftHand→hand_l
LeftLeg→thigh_l   LeftShin→calf_l   LeftFoot→foot_l   LeftToeBase→ball_l
LeftHandThumb1..3→thumb_01..03_l   LeftHandIndex1..3→index_01..03_l   (Middle/Ring/Pinky alike)
   … mirror Right … Jaw→jaw   LeftEye→eye_l   RightEye→eye_r
```
- **Neck 2→1:** compose `Neck1·Neck2` into `neck_01` (or map `Neck1→neck_01`, fold `Neck2`).
- **Drop** what we don't have: `*End` sites, `HeadEnd`, the finger **4th** joints
  (`*Index4` etc.). **Twist bones** (`*_twist_01_*`) have no Motifect source → leave at rest
  (identity) or derive procedurally later. `Weapon_L/R` never animated.

### C.2 Rest reconciliation — bridges T→A, foot pitch, and axes in ONE step

> **REWRITTEN 2026-07-18 (fix).** The original draft below the line rebased the source's motion
> onto an idealized flat-foot **"Retarget Base Pose" (RBP)**. That was wrong: the source rest is a
> **T-pose** (the BVH zero pose = identity rotations), so rebasing onto an **A-pose** RBP
> **double-counted** the T→A difference — arms over-rotated and **crossed** — and the flat-foot
> leveling fought the mesh's pitched bind — **toe-up feet**. The fix reconciles against the mesh's
> **actual bind** instead (§C.2 current, below). The `.rbp.json` reference file is now **unused**.

Every clip stores rotations **relative to its own skeleton's rest**, so you cannot drop
Motifect rotations onto our A-pose directly. Reconcile each source bone against the mesh's
**actual bind** — the pose the mesh is really skinned in (A-pose, *pitched* foot) — via a
per-bone **source-matched base pose** `Sm_b` that rotates each source rest bone so it points
along **our bind bone-direction**. No idealized reference pose; no foot leveling; no rebind.

Per bone `b`, per frame `t` (all in Z-up after the Y-up→Z-up convert):
```
A_b       = our bind GLOBAL rest rotation     (FK the skeleton's OWN `local` rotations — the pose the mesh is skinned in)
Sm_b      = source-matched base pose          (min rotation taking the source rest bone-DIRECTION → our bind bone-direction)
Sa_b(t)   = source animated GLOBAL rotation   (FK the BVH motion at frame t)
Ta_b(t)   = Sa_b(t) · Sm_b⁻¹ · A_b     ← source motion FROM its matched base pose, replayed FROM our bind
local_b(t)= Ta_parent(t)⁻¹ · Ta_b(t)   ← store this per-bone local rotation in the clip
```
Translation: **rotation-only** — keep our bone rest translations (set `retarget: true`);
only the **root** (pelvis) carries motion (root position, scaled by proportion if needed).

**Why this is clean:** at the matched base pose `Sa_b=Sm_b ⇒ Ta_b=A_b`, i.e. the skeleton sits in
the **actual bind** → skinned through the bind there is **no deform anywhere** (not just "where
RBP==bind"), and the source's per-bone articulation drives the rest. Because `Sm_b` aligns the
source's T-pose bone-direction onto our A-pose bind direction **per bone**, the T→A difference,
the pitched foot, and bone-axis differences are ALL absorbed into the single `·Sm_b⁻¹·A_b`
reconciliation. The foot needs **no leveling and no rebind** — the pitched bind IS the reference,
so the walk's own flat-footed stance renders flat. `Sm_b` is stable per source rig (identical
across all Motifect clips). Same principle as the proven Katanami limb-align (memory
`multibody-rig-retarget` / guid 03BBF8F4): align rest bone-directions, no per-bone compensation
onto an invented reference.

### C.3 Retargeter output + tests
- Emit one `flicker.rig`-shaped file per clip (or a clip library) at **30 fps, integer
  ticks, preserving source frame count 1:1** (TAE). Clip local TRS per §C.2.
- **Unit-test:** (1) identity — if `Sm_b==A_b` (matched base pose == bind) the retarget reduces
  to the source's local motion; (2) reproduction — the bind foot stays **pitched −37.5°
  (unmutated)** and every mapped limb **tracks the source** (arms uncrossed, feet track) across
  all frames; (3) timing — output tick count == source frame count, `tick_rate_hz==30`; (4) determinism;
  (5) in-place/root-motion — the **in-place** variant's pelvis X/Y translation is constant
  (== rest) across every frame while Z (bob) varies; the **root-motion** variant preserves the
  source's planar travel.

### C.4 Root motion vs in-place (the locomotion split)

Every clip is **classified** as one of two variants, and the retargeter **emits both** from
each source clip — we need both and pick per-clip usage later (do NOT pre-prune):
- **In-place** — `clips/In-Place/<clip>.json` (bare stem; the loader cycles it by plain name).
  The pelvis's **horizontal (planar) translation is pinned to its RBP rest** — X/Y held over
  the origin — while **vertical bob (Z) and all rotations are kept.** Forward travel and lateral
  sway are removed → the clip plays on a treadmill. **This is what the paperdoll previews for
  now** (locomotion must not drift the model off-camera).
- **Root motion** — `clips/RootMotion/<clip>.json` (the loader namespaces it `RM/<stem>`).
  The **full pelvis translation is preserved** (planar travel intact) so gameplay can move the
  character through the world. *(Deferred refinement: extract the planar delta onto the `root`
  bone channel so gameplay consumes it cleanly while the pelvis keeps only its oscillation.)*

Up is **+Z** (post Y-up→Z-up), so "planar" = the **X/Y** components: pin those to rest, keep Z.
Stationary clips (idles) come out identical in both trees (their planar delta is already ~0).
This mirrors the existing `In-Place/` + `RootMotion/` loader taxonomy
(`flicker-skeletal::format::load_dirs`, RM namespacing).

---

## D. Execution plan (do in order; each is a thin, verifiable slice)

**D.1 — Lock the canonical skeleton.** ✅ **DONE.** Add `jaw, eye_l, eye_r` under `head` to the
base-A skeleton (and the spec) → **66 bones**. *(The separate flat-foot "RBP" data-file the
earlier draft called for is **SUPERSEDED** — the retarget reconciles against the mesh's actual
bind, §C.2; `Alpha/content/retarget/PrismHumanBaseA.rbp.json` is now unused and can be removed.)*
*Verify:* skeleton loads; bone count 66.

**D.2 — Build the BVH→`flicker.rig` retargeter** (`tools/retarget_bvh.py`, pure-Python). ✅ **DONE.**
Parse BVH hierarchy+motion → Y-up→Z-up → name-map (§C.1) → reconcile against the bind (§C.2) →
emit **both** the in-place and root-motion variants (§C.4) at 30 fps. *Verify:* the §C.3 unit
tests green (**5/5**, pure-Python, no app), including the (5) in-place/root-motion split.

**D.3 — Prove one clip in the app.** ✅ **DONE (2026-07-18).** `walk_forward` retargeted onto base A
and confirmed in-window (Aaron): foot flat, walk clean, no drift. *Fix landed en route:* the
original §C.2 RBP rebase produced toe-up feet + crossed arms; rewritten to reconcile against the
mesh's actual bind via a per-bone source-matched base pose (`Sm_b`). All **45** locomotion clips
are now bulk-converted (both In-Place + RootMotion variants).

**D.4 — Conform base B (male).** 🟡 **Conform DONE + verified; in-window check pending.** Ran the
male source through `tools/blender/rename_meshy_to_canonical.py` headless (Blender 5.1.2) with
`--katanami-json katanami/Katana_Morph_Color1.json` and **`--canonical-json PrismHumanBaseA.json`**
— pointing the inference at the 66-bone base A so `infer_canonical_bones` auto-added the 30
fingers, 8 twists, 2 sockets **and the jaw/eye_l/eye_r** in one pass →
`Alpha/content/characters/PrismHumanBaseB/PrismHumanBaseB.json`: **66 bones, retarget=True,
126054 verts, rest-skin error 0.000000 cm, 170 cm tall, weights 1.0**, texture saved beside it.
Bone set is name-identical to base A → outfits skin to both by name. ✅ **Verified in-window
(Aaron):** base B conforms, skins, and animates via a debug **A↔B toggle (`X`)** in the paperdoll.
Two expected cosmetics (by design of this quick toggle, NOT defects): (1) base B borrows base A's
`texture_0.png` (same basename — its own texture is saved beside its JSON); (2) the shared retarget
clips are **baked for base A** (they embed base A's skeleton), so base B plays base A's limb
orientations (torso identical; limbs ~4–11° off its own) — it animates as a *conformed* variant.
If base B should ever carry its OWN limb motion, that's a per-body re-bake or a body-agnostic
retarget — **deferred, not needed now**.

**D.5 — Facial morphs + face bones.** 🟡 **Runtime core DONE** (format + blend pass +
unit-tested); content + UI remain. Landed: `Morph`/`MorphDelta` types + serde-default `morphs`
field on `Mesh` (`format.rs`), and `skin::skin_morphed` / `apply_morphs` — a **sparse pre-skin
blend pass** (`pos += Σ wᵢ·deltaᵢ` before skinning; `skin(mesh, palette)` stays byte-identical;
a unit test proves the weighted blend). **Remaining (bundle into the Blender content session):**
author the starter identity-morph set (§A.3) as sculpted targets; **re-weight the lower face to
the `jaw` bone** (base A was skinned before D.1 added jaw/eye, so nothing binds to them yet);
wire the create-a-face UI slider. *Verify:* a morph slider visibly reshapes the face; jaw animates.

**D.6 — Fix the fit gadget's Euler authoring.** ✅ **DONE.** Replaced the fit gadget's
`Quat::from_euler(EulerRot::XYZ,…)` (driven by `user_rot: Vec3`) with a **nudge-accumulated
quaternion** (`user_rot: glam::Quat`): ±X/±Y/±Z nudges compose world-axis rotations
(`Quat::from_axis_angle · user_rot`, normalized, no clamp), the mirror is a true reflection
`(x,−y,−z,w)`, `fits.json` stores the quat `[x,y,z,w]` (legacy 3-float Euler auto-converted on
load), and reset is exact `IDENTITY`. HUD rows read out the Euler decomposition for display
only — authoring is nudge-only (the absolute HUD-set path is removed). *Verify (in-window):*
nudge a prop through ±180° on each axis with no gimbal fold; reset returns to identity.
**22/22 paperdoll tests green** incl. the mirror-reflection + round-trip tests.

**D.7 — World-vs-local frame audit.** ✅ **DONE (audit — no code change needed).** Swept
`flicker-skeletal` for physics/"down" expressed in a bone frame: **gravity is world-space at
every site** (`jiggle.rs:112` applies `gravity·dt²` unrotated; the only down-vectors are the
world z-down constants), and `driver_rot` is applied **only** to the stiffness home-shape +
attachment frames — which §A.2 permits (bone-local *shape/offsets*, not forces). The original
cloth-down bug was already resolved by the Model-B drape rewrite (cloth verts ride the
world-down-hanging chain). No wind or other force vectors exist. Proven by
`jiggle::hangs_straight_down_at_rest`, `cloth::region_drapes_off_the_rest_shape`,
`anchor_move_carries_the_hang` (all green). *Verify (in-window):* cloth hangs world-down
regardless of limb orientation.

**D.8 — Retire Katanami (model/rig only).** ✅ **DONE (2026-07-18) — scoped by Aaron, NARROWER
than the original wording:** KEEP the Katanami clip library + `Katanami.pack.json` (the pack /
animation set is unchanged — our custom body plays the Katanami clips by shared bone name);
retire only the unused Katanami MODEL/RIG. Done: `flicker-packeditor` now loads the custom base
body **PrismHumanBaseA** + the Katanami clips/pack (`load_dirs([PrismHumanBaseA, katanami])`, pack
untouched; 5 tests green); deleted the unused Katanami model/rig assets — 14 body/hair/eye
**textures** (~134 MB) + `Mesh_Katana.json`. **KEPT** `Katana_Morph_Color1.json` — a model asset
but still the live **conform reference** (`--katanami-json`), so not "no longer used". Paperdoll
already runs on base A. *Verify (in-window):* the packeditor previews the pack on the base body.
*(Wiring a Prism pack over the Motifect retarget clips for real locomotion is a later new-system
step — the pack still references the Katanami clips.)*

**Sequencing:** D.1→D.2→D.3 are the critical path (they validate the retarget + kill the
hacks). D.4/D.5 are parallelizable after D.3. D.6/D.7 are independent cleanups. D.8 last.

---

## E. File map

| Thing | Path |
|---|---|
| Format contract + loader | `Alpha/crates/animation/flicker-skeletal/src/format.rs` |
| Pose/FK + retarget playback | `…/flicker-skeletal/src/pose.rs` (`from_rotation_y(π/2)` hack @192) |
| Skinning | `…/flicker-skeletal/src/skin.rs` (add morph blend here) |
| State machine / TAE | `…/flicker-skeletal/src/state.rs` |
| Cloth (world-down audit) | `…/flicker-skeletal/src/cloth.rs` |
| Canonical skeleton (base A) | `Alpha/content/characters/PrismHumanBaseA/PrismHumanBaseA.json` |
| Base B source (to conform) | `Alpha/content/source/PrismHumanBaseB/…fbx` |
| Motifect animations | `Alpha/content/source/Motifect/Motifect_locomotion_complete_v1_0/` (BVH + Animations) |
| Fit gadget (Euler → quat) | `Alpha/flicker-paperdoll/src/main.rs` (`user_rot`, `from_euler` @299) |
| The `-90° X` import hack | `format.rs:464` |
| New retargeter (to write) | `tools/` (e.g. `tools/retarget_bvh.py`) |

---

## F. Requirements this rig must serve (design guardrails)

- **Two body shapes:** A = female, B = male. Same skeleton topology, different proportions;
  outfits skin **by bone name** so one garment fits both (and all scales).
- **≥10 scales** (dwarf/fae → elf/half-orc): same topology, scaled proportions + retarget;
  the `retarget` rotation-only path already keeps a rig's own proportions.
- **TAE per-frame accuracy** for iframe/hitbox windows → §A.2 frame-timing rule is
  non-negotiable.
- **Dungeon-Maker toolkit:** players compose a boss from unlocked creatures + specialized
  attacks under a **difficulty budget** (≈2000 pts for a 6-limb boss w/ fewer abilities vs
  ≈1000 for a humanoid w/ more). This is a **gameplay layer on a topology-agnostic rig** —
  a six-limb creature is just a different skeleton; the skeleton/clip **format special-cases
  nothing**. Keep the format generic (variable bone counts, name-keyed tracks) so it serves
  arbitrary creatures.
- **DNA-forward:** UE4/MetaHuman naming end-to-end so the eventual Epic DNA/RigLogic upgrade
  layers on top (identity morphs → DNA identity; body skeleton → DNA body; jaw/eye/face group
  → DNA facial) rather than forcing a rewrite. (RigLogic itself is **not** in scope now — it's
  rig *evaluation*, not dynamics; see memory `cloth-region-split` for why it was parked.)
