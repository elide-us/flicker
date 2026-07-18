# flicker multi-body rig + retarget — handoff

**Objective (user, 2026-07-16):** create a **TAE-compatible canonical rig + FBX→canonical
transform** so ONE shared `.pack` animation library drives bodies of any size, proportion
and scale — **halflings, dwarves, humans, elves, titans**. Meshes come from **Meshy.ai**.

Status 2026-07-16: the *stance* retarget is correct and user-confirmed on a human-vs-human
pair. **Steps 1 and 2 are BUILT and verified** — the transform emits the full **63-bone
canonical rig** with mesh-derived hip width, and the knees no longer cross on walk (§10, §11).
The asset has been regenerated. Open: finger weighting (§9.3) and the §8 IK question.
Everything below is measured, not assumed.

> **Read §10 and §11 before §2–§4.** Sections 2–4 are the original plan and retain
> **corrected** errors (the "up +6.5" hip height; the `femur = 0.26 × height` target; the
> "groin line is 95"). The corrections are inline, but §10/§11 are what shipped.

---

## 1. The canonical target rig — 63 bones

`Alpha/content/characters/base_human_female/BaseHumanFemale.json` **is** the canonical bone
set (verified). 63 bones = Katanami's 101 **minus 38**: `Breast_left/right`, `Cloth_B_*`,
`Pevis_Cloth_*`, `Hair_*`, `sleeve_*`, `ik_*`. Strict subset — nothing in BHF is absent from
Katanami. Matches the user's "Katanami without the cloth jiggle and breast bones".

Contains: `root`, `pelvis`, `spine_01..03`, `neck_01`, `head`, `clavicle_l/r`,
`upperarm(+_twist_01)`, `lowerarm(+_twist_01)`, `hand`, **30 finger bones**
(thumb/index/middle/ring/pinky × 3 × 2), `thigh(+_twist_01)`, `calf(+_twist_01)`, `foot`,
`ball`, `Weapon_L/R`.

> **Caveat:** BHF uses **Katanami's skeleton verbatim** — 0.0000 cm across all 63 shared
> bones (hip width 18.7, shoulder 24.2, head z 168.3). It is the canonical **bone set**, NOT
> a canonical proportion. Its mesh is 189 cm because the Meshy female was hand-fitted to
> Katanami's A-pose first.

Meshy gives **23** canonical bones after rename — a strict subset of the 63 (nothing to drop).

---

## 2. The transform — the plan (user's design, 2026-07-16)

### Step 1 — infer the extra bones (40) + rename
Gap = **30 fingers + 8 twists + 2 weapon sockets**.
- **Twists** interpolate along their parent limb.
- **Fingers**: no Meshy source → **assume a straight hand of standard proportion** (user's
  call). Refine later with in-engine tools (paperdoll / packeditor already exist for this).
- **Weapon_L/R**: derive from the hand.

> **⛔ There is NOTHING to strip on the Meshy path** (user + measured, 2026-07-16). An earlier
> revision of this section said to reuse `rig_meshy_base.py`'s weight transference, 31-jiggle
> dissolve and 101→63 reduction. **All three are Katanami-side artifacts and must NOT be
> imported here:**
> - **Verified:** PrismHumanBaseA (23 bones) and `musefit/Muse001` (24) contain **zero**
>   breast/cloth/hair/sleeve/ik bones. The only Meshy-side drop is `head_end`/`headfront`
>   (tip markers), already handled by `DROP` in `rename_meshy_to_canonical.py`.
> - **101→63 reduction:** the Meshy path never sees 101 bones. Nothing to reduce.
> - **31-jiggle dissolve:** needed only because *Katanami's mesh weights* referenced the jiggle
>   bones (a plain delete zeroed his hip band). Meshy meshes are weighted to their own bones.
> - **Weight transference:** Data-Transferring Katanami's weights **is** the superseded
>   conform-to-one-skeleton approach (§5) — it is what makes every body Katanami-shaped.
>   Reusing it reintroduces the exact defect that cannot carry the roster.
>
> **Breast bones** were an artifact of the Katanami model's author; our sources do not have
> them. **Jiggle bones are not part of the rig or the animations** — they are **additive and
> per-asset** (see §7: cloth jiggle for hem/ribbon/sleeves), to be **determined and added
> back**, never stripped. Step 1 is therefore **rename (done) + infer the 40** — the strip
> half does not exist.
>
> **Consequence — the canonical set is also a RESOLUTION FILTER.** Clip tracks resolve by
> **name** (`format.rs`; unresolved tracks are collected, never an error — so Katanami's 38
> extra tracks are silently ignored on a 23/63-bone target). Adding a jiggle bone back under
> a Katanami-matching name (`Breast_left`, `Cloth_B_L_01`, …) silently opts that body into
> **Katanami's authored motion for it**. Naming new jiggle bones is therefore a deliberate
> opt-in/opt-out of the existing library motion, not a cosmetic choice.

### Step 2 — re-measure + scale
Derive the rig's anatomy **from the MESH**, not from Meshy's joint guesses (§3). Apply the
UE4 relative-orientation convention (the existing `reorient_to_canonical` base-frames +
limb-align already do this).

> **Meshy constraint (user, 2026-07-16):** the user can only place the **groin** for the
> pelvis — **hip WIDTH is not settable**; Meshy produces the pelvis bone with no size
> control. **Therefore the pipeline MUST derive hip width.** It cannot be fixed by a better
> export.

### Step 3 — result
A properly-spaced **63-bone skeleton at the body's own scale/proportions** that translates
the pack animations correctly.

---

## 3. Measured findings — what to trust from Meshy

PrismHumanBaseA (Meshy female, 170 cm) ÷ Katanami (189 cm):

| height | pelvis height | thigh length | **hip width** | **shoulder width** |
|---|---|---|---|---|
| 0.91× | 0.90× | 0.86× | **0.54×** | **1.28×** |

- **Lengths and heights are coherent → trust Meshy's bone lengths.**
- **Joint WIDTHS are not → derive them.**
- **Uniform scale is free.** Rotations are scale-invariant, so a pure 170-vs-189 scale can
  never break a rotation-copy retarget. The bug is the **non-uniform** width mismatch.
  (Do NOT normalise the roster to one height — different heights are the objective.)

Measured against **her own mesh** (the authority — *not* Katanami):
- **Shoulders are CORRECT.** Joint x=15.5 vs outer silhouette 19.1 (3.6 cm inside; target
  3–5) and 4.7 cm below the shoulder top 144.4 (target 4–6). **Meshy places shoulders well —
  leave them.** An earlier "shoulders too lateral" claim was **wrong**: it compared against
  Katanami, who is stylised-narrow (12.1/side).
- **Hips are wrong in WIDTH only** (corrected 2026-07-16 — this line used to read "on both
  axes"). x=5.2 — only 30% of the way to the widest hip (measured 16.88 left / 17.35 right);
  should be ~50% → **8.67** (out **+3.52**). **FIXED and shipped** (§11).
  > **The height was never wrong.** z=86.2 sits at her **measured crotch (~85)**, and her thigh
  > length is **trusted Meshy data** (§3's own table: `thigh 0.86×` is on the *coherent* list).
  > The old "up +6.5" came from forcing femur = 0.26 × height — human anthropometry, not a
  > measurement of her. See §4 and §11.
- **Consequence:** her knees crossed during walk; Katanami's never do. His rotations swing from
  an 18.7 cm hip, hers from 10.2. **Now measured per-frame** (§11): min knee separation
  **−0.22 cm, 15/65 frames crossed → +6.71 cm, 0/65** after the fix (Katanami +6.64). The older
  "−3.6 vs +4.4" figures were `min(left) − max(right)` across *different* frames, not a
  per-frame separation — same body, looser metric.

---

## 4. Anatomical placement rules (the derivation spec)

- **Hip ball (femoral head):** 50% from midline to the **widest hip**. NOT the outer hip bump
  (greater trochanter); NOT down where the thigh visibly starts. **The width half of this rule
  is CORRECT and is what shipped** (§11).
  > **⛔ CORRECTED 2026-07-16 — the height half of this rule was wrong.** It used to read "at
  > the **groin line** (where the legs meet)" and paired with "groin line is 95" for
  > PrismHumanBaseA. **95.6 is the PELVIS BONE's z, not her crotch** — someone read the bone and
  > called it the groin. Measured from her mesh, the midline reads full torso depth down to z=86
  > and the legs part by z=84, so her **crotch is ~85** and her thigh bone (86.2) already sits at
  > it. The published target of 92.8 did not come from the groin at all — it came from forcing
  > femur = 0.26 × height. **Do not place hip HEIGHT from this file.** See §11: height is trusted
  > Meshy data and must not be corrected.
- **Shoulder ball (glenohumeral):** just inside the deltoid (~3–5 cm in from the outer
  silhouette), ~4–6 cm below the shoulder top — roughly straight above the armpit. It
  genuinely *is* near the outside; the deltoid is a thin cap over the joint.
- **~~Sanity check that catches it every time:~~ femur ≈ 0.26 × height — ⛔ DO NOT APPLY.**
  This is **human** anthropometry. It is what produced the bogus "up +6.5", and conforming a
  body to it is the same defect as `rig_meshy_base.py` conforming every body to Katanami (§5) —
  a dwarf's femur is not 0.26 × height either. It may be read as a *descriptive* note about a
  human body; it is **never** a correction target. Bone LENGTH is trusted Meshy data (§3).
- **Depth:** femoral head ~mid-depth in the pelvis, slightly forward.
- **Always rig the naked body.** Rigging a clothed silhouette makes the rigger guess depth
  off fabric → knees point forward. Clothes skin to the body's skeleton.

---

## 5. Architectural history — do not relitigate

- `tools/blender/rig_meshy_base.py` = **conform-to-one-skeleton**: brings the MESH to
  Katanami's skeleton and Data-Transfers his weights. Produces **Katanami's skeleton
  verbatim** → every body is Katanami-shaped → **cannot express the roster**. Superseded
  2026-07-15 by `rename_meshy_to_canonical.py` (per-character rig fitted to each mesh,
  keeping its proportions).
- **Its docstring predicted the current bug:** *"the skeleton's rest CANNOT be changed
  without breaking every animation … hands collapse onto the forearm, **knees push
  through**"*. The supersession traded *"animations always correct / every body
  Katanami-shaped"* for *"bodies keep proportions / animations need real retargeting"*. We
  are paying the second half.
- It also demanded a **manual per-body pose+scale fit** ("PREREQUISITE — POSE ALIGNMENT") →
  can't carry a roster.
- **Keep from it: NOTHING for the Meshy path** (corrected 2026-07-16 — see the §2 Step 1
  banner). Its weight transference, 31-jiggle dissolve and 101→63 reduction are all
  Katanami-side artifacts with no Meshy-side counterpart; the transference *is* the
  conform-to-one-skeleton defect. The script remains the **historical producer of
  `BaseHumanFemale.json`** (the canonical bone set) and is correct in that role — leave it
  alone, but do not mine it for the multi-body pipeline.

---

## 6. Engine state (landed 2026-07-15/16)

- **Absolute-orientation retarget + limb-frame alignment** (`_LIMB_CHILD` = arms, legs,
  feet): rotate each limb's rest frame by the minimal rotation mapping Katanami's limb
  direction → this body's, so the child-joint offset lies along the bone axis. Every limb
  direction then matches Katanami's idle to **0.0°**. **Torso bones are NEVER limb-aligned**
  — orienting the pelvis toward its first child tilts the whole body 28° (reverted twice).
  **User-confirmed 100% in-window.**
- **⛔ `retarget_rot = t·s⁻¹` was WRONG** — additive-from-bind re-introduces the over-swing
  (measured identical to baseline). **Removed** from `format.rs`/`pose.rs`. **Do not
  reintroduce a per-bone rotation compensation.**
- **Pelvis translation delta** (restores lost hip motion): retarget non-root translation =
  `target_rest + (clip_T − source_rest)`, with `source_rest` read from the clip file's own
  skeleton (`ResolvedTrack::source_rest`). Zero for constant-offset limbs (proportions
  preserved); the real hip sway/bob for the pelvis, rebased to this rig's hip height. Walk
  hip bob **0 cm → 3 cm**. Root keeps the clip translation (root track is (0,0,0) — all hip
  motion lives on the pelvis track).
- **Paperdoll `B`**: cyan joint wireframe **+ orange bone-frame axes** (`bone_axis_segments`
  — repurposed the vestigial depth-tested pass, NOT a new toggle; root skipped). The orange
  pass is the **only** viz that shows bone ORIENTATION; the position-only wireframe cannot.
- Tests: skeletal **19/19**, paperdoll **5/5**, clippy clean.

---

## 7. Outfit (parked)

`musefit/Muse001.json` — Meshy clothing skinned to the base armature via Blender automatic
weights, baked head/hands stripped (spatial cut: head above the collar; hands distal to the
wrist along the arm axis), exported `outfit=True`, renamed to canonical, paperdoll key `4`.
Known: auto-weights bind the skirt hem **33/33 to the two calves** → tears on stride.
**User's call (2026-07-16): produce separate cloth parts rather than one welded Meshy figure;
cloth jiggle bones needed for hem/ribbon/sleeves.** Parked — do not keep hacking the welded mesh.

---

## 8. The open architectural question

Even with correct hip width, **FK rotation-copy is proportion-blind**. It works when
proportions are close. For the full roster (dwarves, halflings, titans), **IK retargeting** —
planted feet, targeted hands/weapon sockets — is likely required. Correcting the widths gets
human-ish bodies working; it does **not** make a dwarf work. **The user has not chosen this
— do not build it unasked.**

---

## 9. Next

1. ~~Build step 2's mesh-derived hip placement~~ — **LANDED 2026-07-16** (§11). Knees verified
   to stop crossing. **WIDTH only — the "up +6.5" was WRONG, see §11.**
2. ~~Build step 1's bone inference~~ — **LANDED 2026-07-16** (§10). Step 1 is complete.
3. **Follow-on (deferred, user's call):** weight each body's own hand mesh to its inferred
   finger bones so the fingers actually DEFORM. Until then they resolve and rotate but move
   no vertices. NOT `rig_meshy_base.py`'s Data-Transfer (that is Katanami's weights onto a
   Katanami-shaped body — the §5 defect); this is each body's own hand auto-weighted to its
   own derived bones.

---

## 10. Step 1 — LANDED 2026-07-16 (`infer_canonical_bones`)

`rename_meshy_to_canonical.py` now infers the 40 and exports the full **63-bone canonical
rig**. New required arg **`--canonical-json`** (`BaseHumanFemale.json`) — the bone SET +
geometry authority. It must NOT be the Katanami rig: his 101 carry the 38 jiggle/ik bones
that are not in the canonical set. Data-driven: whatever the canonical rig has and this rig
lacks is inferred, so the tool follows the canonical set if it changes.

**It runs AFTER `reorient_to_canonical`, and that ordering is what makes it correct** — every
parent frame is already canonical, so each bone hangs off its parent at the reference's own
local offset, scaled:
- **Twists** — their parent (upperarm/lowerarm/thigh/calf) is **limb-aligned**, so its axis
  already points down THIS body's limb. The reference's offset scaled by the limb-length ratio
  therefore lands the twist at the **same fraction along this body's own limb**. *"Interpolate
  along the parent limb" falls out of the limb-align for free* — there is nothing to
  interpolate by hand. Verified: all 8 hold the reference's fraction to <1e-4.
- **Fingers/sockets** — `hand_l/r` is deliberately NOT limb-aligned (keeps Katanami's world
  orientation), so the chain reproduces the reference hand's orientation exactly, which is what
  the clips' absolute rotations expect. Verified: finger world directions match the reference to
  **0.0002°**. A uniform scale preserves direction, so fingers need no limb-align of their own.

**Hand scale = the FOREARM ratio, and it is mesh-VALIDATED, not assumed.** A body's arms need
not scale with its height — PrismHumanBaseA's forearm is **1.010×** the reference while she is
only **0.910×** its height. Measured: her mesh hand is **17.20 cm** wrist→fingertip (sane for
170 cm); the reference's **bone** hand (`hand_l`→`middle_03_l`) is **15.56 cm**; allowing the
normal ~1.5–2 cm fingertip pad puts her true ratio at **~0.98–1.01**. The forearm proxy lands
within **1–3%**; a height proxy (0.910) would undersize hands by 7–9%. **The hand follows the
ARM, not the height.** Each hand uses its own forearm, so the two scales differ slightly.

> **⚠️ `BaseHumanFemale`'s MESH WEIGHTS ARE JUNK — use only its BONES.** The median vertex in
> its hand subtree sits **26.7 cm from the wrist** (a real hand is ~18). A `rig_meshy_base.py`
> nearest-vertex Data-Transfer artifact — exactly the sloppiness §5 warns about. Its bones are
> Katanami's and are sound. Do not re-derive hand scale from its mesh; it will lie.

**Verified** (drove the real functions against the committed post-reorient
`PrismHumanBaseA.json`, no Blender needed — it is already in the state the function expects):
63 bones / set == canonical · mesh bytes untouched (no joint index disturbed) · topological
order held · `inverse_bind` inverts world for all 63 (worst 8.9e-14) · no NaN/inf · idempotent
· **Walk_nonWeapon resolves 23/101 → 63/101 (+40)**, the 38 unresolved being Katanami's
jiggle/ik, ignored by design.

**What this does NOT do:** the inferred bones carry **no weights**. `Weapon_L/R` are attachment
points and are immediately functional; the **twists and fingers resolve and rotate but deform
nothing** until each body's hand mesh is weighted to them (§9.3). Expect hands to still look
rigid in the paperdoll — that is correct, not a regression.
3. Re-verify in the paperdoll — **the user runs the window** (CLAUDE.md §8).

**Process lesson (user, emphatic):** verify **absolute orientation** in-engine / a real
render, against the **correct reference clip** — never motion-delta, rest-skin, or
head-above-feet; all three are blind to a rigid rotation. Diagnose and propose *before*
building; do not run with a literal reading of a reorientation.

---

## 11. Step 2 — LANDED 2026-07-16 (`derive_hip_placement`), WIDTH ONLY

`rename_meshy_to_canonical.py` now derives the femoral heads' **width** from the body's own
mesh. Runs **BEFORE** `reorient_to_canonical` (which consumes rest positions and then rebuilds
every frame + `inverse_bind`, so the rest mesh is untouched). Only the thigh joints move; every
other bone keeps its exact world position and the child locals absorb the shift — the knee is
already correct (bone x 7.09 vs flesh 7.0) and must not move.

**Measured from flesh owned by `pelvis`/`thigh_*`, which excludes the arms BY WEIGHT.** This is
not incidental: a naive "widest vertex at hip height" reads **~40 cm** on an A-posed body,
because at hip height the **hands** are the widest thing in the z-band. Per-side, so an
asymmetric body works (hers: left 16.88, right 17.35).

### WIDTH only — and why height must NOT be corrected

The plan called for "out +3.3, **up +6.5**". **The width is right; the height is wrong.** Per
this repo's own trust boundary (§3, memory `03BBF8F4`): Meshy's **lengths are coherent and
trusted** — `thigh 0.86×` is explicitly on the trusted list — and only its **joint widths** are
not (`hip 0.54×`). Width is also precisely what the user *cannot set in Meshy*, which is why the
pipeline must derive it. So:
- Her thigh length (38.08) is **trusted data**, and her hip already sits at her **measured
  crotch** (~85; bone at 86.2). Nothing about the height is broken.
- "Up +6.5" came only from forcing **femur = 0.26 × height**, a human ratio. Applying it
  conforms a body toward human-standard proportions — **the exact defect that killed
  `rig_meshy_base.py`** (§5), and fatal to a roster containing dwarves and titans.
- The knees crossing is a **width** symptom by the handoff's own diagnosis: Katanami's rotations
  swing from an 18.7 cm hip, hers from 10.2, so the same adduction carries her knees past centre.

### Verified — measured, not asserted

Replicated `pose.rs` playback in Python and ran `Walk_nonWeapon` through the real retarget math
(`reorient` reads only POSITIONS from its input, so it is idempotent — hip-fix → reorient on the
committed rig reproduces the pipeline exactly):

| | min knee separation | frames crossed |
|---|---|---|
| Katanami (authoring reference) | +6.64 cm | 0/65 |
| PrismHumanBaseA **before** | −0.22 cm | **15/65** |
| PrismHumanBaseA **after** | **+6.71 cm** | **0/65** |

Femoral-head separation **10.18 → 17.11 cm** (Katanami 18.7). Feet **−7.34 → −0.40** (his −0.63;
feet passing close is normal for a walk). The calf x-ranges reproduce the 2026-07-16 diagnosis
exactly (−2.25..1.42 vs its −2.2..1.4) — same body, measured **per-frame** rather than as a
cross-frame bound (its "−3.6" was `min(l) − max(r)` across *different* frames).

### The asset was regenerated

**The tool changing does NOT change the asset** — this is why the paperdoll showed nothing after
step 1. `Alpha/content/characters/PrismHumanBaseA/PrismHumanBaseA.json` has been rebuilt headless
and installed: **63 bones, 115221 verts** (an exact match with the previous asset ⇒ `--decimate 0.3`
confirmed as the original setting), `retarget=true`, hips at ±8.67/−8.44. Reproduce with:

```
/Applications/Blender.app/Contents/MacOS/Blender --background --factory-startup \
  --python tools/blender/rename_meshy_to_canonical.py -- \
  --fbx Alpha/content/source/PrismHumanBaseA/Meshy_AI_Female_Human_Base_Mod_biped_Character_output.fbx \
  --out Alpha/content/characters/PrismHumanBaseA/PrismHumanBaseA.json \
  --katanami-json Alpha/content/characters/katanami/Katana_Morph_Color1.json \
  --canonical-json Alpha/content/characters/base_human_female/BaseHumanFemale.json \
  --decimate 0.3
```

**Expect in the paperdoll:** knees no longer cross; hands still rigid (inferred bones carry no
weights — §10); fingers/twists inert until weighted (§9.3).

### Weights — MEASURED, none need correcting (2026-07-16)

The user's step-2 ask included "adjust any weights that need to be corrected." **Measured answer:
none do, as a result of the hip move.** Do not re-weight the hips.

**Why, structurally:** moving a joint changes the **pivot**, not the **assignment**. Meshy's hip
weights say "this vertex is ~50% hip flesh" — still true after the move. What changed is that the
50% now pivots around her real femoral head instead of a point 3.5 cm too medial. Same weights,
better pivot. (Meshy smooth-skins: the hip band is a soft ~50/50 pelvis/thigh blend gradated by
HEIGHT, not laterally — so widening the joint does not strand a lateral weight boundary.)

**Measured:** skinned the mesh at the walk's extreme frames with **identical weights**, old hips
vs new. Hip/groin band (29786 edges), edge-length distortion vs rest:

| tick | OLD hips (Meshy) | NEW hips (fixed) |
|---|---|---|
| 16 | mean 3.95% / worst 59.0% | mean 3.95% / worst 62.0% |
| 48 | mean 3.64% / worst 63.3% | mean 3.62% / worst 61.0% |

Mean is **unchanged**; the worst case moves ±3% in **both** directions across frames — noise, not a
trend. 94% of thigh-owned verts are no further from the femur than before.

> **Pre-existing (NOT from the hip move) — flagged, not chased.** A distortion hotspot exists at
> the **crotch**: 12 edges >50%, 448 >30%, 1262 >20% (of 29786), the worst all clustered at
> **x≈2.8–4.8, z≈83.6–84.3** — her crotch line (~85), on ~2 mm edges. **Identical before and
> after**, so it is Meshy's weighting, not this change: the classic crotch pinch on a stride, in a
> largely self-occluded region, on a body the user has visually confirmed. Out of scope; recorded
> so it is not re-discovered. (It is NOT a micro-edge artifact — the hip band's p1 edge is 1.55 mm
> and **zero** edges are sub-0.5 mm. An earlier claim that it was measurement noise was wrong.)
