"""
rename_meshy_to_canonical.py — headless SLICE 1 of the multi-body retarget pipeline.

Import a Meshy auto-rigged character FBX (24-bone Mixamo-style rig), rename its bones to
our canonical (Katanami/UE4) names so the shared clip library resolves by name, drop the
head tip bones, INFER the canonical bones Meshy never produces (30 fingers + 8 twists + 2
weapon sockets → the 63-bone target), and export a flicker.rig with `retarget: true`
(rotation-only playback so the body keeps its OWN proportions while borrowing the library's
rotations).

    Blender --background --factory-startup --python rename_meshy_to_canonical.py -- \
        --fbx <Character_output>.fbx --out <PrismHumanBaseA>.json \
        --katanami-json <Katana_Morph_Color1>.json \
        --canonical-json <BaseHumanFemale>.json [--decimate 0.3]

NOTHING is stripped here beyond Meshy's own head-tip markers (`DROP`). Meshy sources carry
no breast/cloth/hair/sleeve/ik bones — those are artifacts of the Katanami model alone, and
jiggle bones are ADDITIVE per-asset (added back for cloth hem/ribbon/sleeves), never removed.
Do NOT mine `rig_meshy_base.py` for a strip or weight-transfer step; see
docs/flicker-multibody-rig-handoff.md §2.
"""
import bpy, sys, os, math, json, argparse, importlib.util

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ap = argparse.ArgumentParser()
ap.add_argument("--fbx", required=True)
ap.add_argument("--out", required=True)
ap.add_argument("--decimate", type=float, default=0.3)
ap.add_argument("--katanami-json", required=True,
                help="canonical reference rig — supplies the UE X-down-bone axis convention")
ap.add_argument("--canonical-json", required=True,
                help="the canonical 63-bone rig (BaseHumanFemale.json) — supplies the target bone SET "
                     "and the finger/twist/socket geometry scaled onto this body. NOT the Katanami "
                     "rig: his 101 carry 38 jiggle/ik bones that are not part of the canonical set.")
ap.add_argument("--tools", default=os.path.dirname(os.path.abspath(__file__)))
args = ap.parse_args(argv)
OUT_DIR = os.path.dirname(os.path.abspath(args.out))
os.makedirs(OUT_DIR, exist_ok=True)

# The rest-rebase align primitive is SHARED with the offline BVH bake — ONE primitive (memory
# 614E5958). Load flicker_rebase.py from tools/ (the parent of this blender/ dir) via the same
# module-by-path mechanism used for io_scene below. (Blender bundles numpy, which it imports.)
_rebase_py = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "flicker_rebase.py")
_rb_spec = importlib.util.spec_from_file_location("flicker_rebase", _rebase_py)
_fr_rebase = importlib.util.module_from_spec(_rb_spec)
_rb_spec.loader.exec_module(_fr_rebase)

# Meshy (Mixamo-style) -> canonical (see memory 03BBF8F4). Spine is bottom-up in Meshy:
# Spine02 is the LOWEST, so Spine02->spine_01.
RENAME = {
    "Hips": "pelvis",
    "Spine02": "spine_01", "Spine01": "spine_02", "Spine": "spine_03",
    "neck": "neck_01", "Head": "head",
    "LeftShoulder": "clavicle_l", "LeftArm": "upperarm_l",
    "LeftForeArm": "lowerarm_l", "LeftHand": "hand_l",
    "RightShoulder": "clavicle_r", "RightArm": "upperarm_r",
    "RightForeArm": "lowerarm_r", "RightHand": "hand_r",
    "LeftUpLeg": "thigh_l", "LeftLeg": "calf_l", "LeftFoot": "foot_l", "LeftToeBase": "ball_l",
    "RightUpLeg": "thigh_r", "RightLeg": "calf_r", "RightFoot": "foot_r", "RightToeBase": "ball_r",
}
DROP = {"head_end", "headfront"}  # Meshy head-tip markers; not deform bones

def log(*a): print("[rename]", *a)

# ── convention conversion: Meshy (Mixamo, Y-down-bone) → canonical (UE, X-down-bone) ──
def _m(m): return [[m[c*4+r] for c in range(4)] for r in range(4)]
def _cm(M): return [M[r][c] for c in range(4) for r in range(4)]
def _mul(A, B): return [[sum(A[r][k]*B[k][c] for k in range(4)) for c in range(4)] for r in range(4)]
def _inv(M):
    n = 4; A = [row[:] + [1.0 if i == j else 0.0 for j in range(n)] for i, row in enumerate(M)]
    for i in range(n):
        p = max(range(i, n), key=lambda r: abs(A[r][i])); A[i], A[p] = A[p], A[i]
        d = A[i][i]; A[i] = [x/d for x in A[i]]
        for r in range(n):
            if r != i:
                f = A[r][i]; A[r] = [a-f*b for a, b in zip(A[r], A[i])]
    return [row[n:] for row in A]
def _fk(bones):
    G = [None]*len(bones)
    for i, b in enumerate(bones):
        L = _m(b["local"]); p = b["parent"]; G[i] = L if p < 0 else _mul(G[p], L)
    return G
def _rot3(A): return [[A[r][c] if c < 3 else 0.0 for c in range(4)] for r in range(3)] + [[0, 0, 0, 1]]
def _pos(G): return [G[0][3], G[1][3], G[2][3]]
def _sub(a, b): return [a[i]-b[i] for i in range(3)]
# Minimal swing rotation (4x4) taking unit vector u onto unit vector v — THE shared rest-rebase
# primitive (memory 614E5958), identical to retarget_bvh's `q_between` in quaternion form (pinned
# by tools/test_flicker_rebase.py). Was a local `_align`/Rodrigues copy; now the one canonical impl.
_align = _fr_rebase.align_mat
# Limb bones whose rest frame is aligned to THIS body's own limb (key -> the child joint down the
# same chain that defines the limb direction): arms, legs, feet. Torso bones (pelvis, spine,
# clavicle, neck, head) are NOT limb-aligned — they keep Katanami's world orientation, so the
# stance's torso/shoulders/hips reproduce as authored and the body cannot tilt.
_LIMB_CHILD = {"upperarm_l": "lowerarm_l", "lowerarm_l": "hand_l",
               "upperarm_r": "lowerarm_r", "lowerarm_r": "hand_r",
               "thigh_l": "calf_l", "calf_l": "foot_l", "foot_l": "ball_l",
               "thigh_r": "calf_r", "calf_r": "foot_r", "foot_r": "ball_r"}

def derive_hip_placement(data):
    """Correct the femoral heads' WIDTH from the body's own mesh. Meshy plants them too medial and
    offers no control — the user can place only the groin, so hip width is NOT settable at export
    and the pipeline MUST derive it (user, 2026-07-16). Runs BEFORE reorient_to_canonical, which
    consumes rest POSITIONS; reorient then rebuilds every frame and inverse_bind from the corrected
    ones, so the rest mesh is untouched.

    Rule (handoff §4): the femoral head sits 50% of the way from the midline to the WIDEST HIP.
    Measured from flesh owned by pelvis/thigh, which excludes the arms by WEIGHT — a plain
    "widest vertex at hip height" reads ~40cm on an A-posed body because the HANDS are the widest
    thing in that z-band. Per-side, so an asymmetric body is handled.

    WIDTH ONLY — deliberately. Meshy's bone LENGTHS are coherent and trusted (thigh 0.86x); only
    its joint WIDTHS are not (hip 0.54x) — memory 03BBF8F4. Her thigh is therefore trusted data and
    her hip already sits at her own measured crotch (bone z 86.2 vs crotch ~85). An earlier plan
    also raised the hip +6.5 to force femur = 0.26 x height; that is a HUMAN ratio, and conforming
    a body to it is the same defect as rig_meshy_base.py conforming every body to Katanami — a
    dwarf's femur is not 0.26 x height either. Do not reintroduce a height correction.

    (The handoff's "groin line is 95" was wrong — 95.6 is the PELVIS BONE's z, not the crotch.
    Measured from the mesh: the midline reads full torso depth down to z=86 and the legs part by
    z=84, so the crotch is ~85. Its target of ~92.8 came from the human femur ratio, not the groin.)

    The knees are the reported symptom and are a WIDTH problem: Katanami's rotations swing from an
    18.7cm hip, hers from 10.2, so the same adduction carries her knees past the centerline and
    through each other. Only the thigh joints move; every other bone keeps its world position (the
    knee is already correct — bone x 7.09 vs flesh 7.0), so the child locals are rebuilt to absorb
    the change and the stance/proportions are otherwise untouched."""
    bones = data["skeleton"]["bones"]
    idx = {b["name"]: i for i, b in enumerate(bones)}
    if not {"pelvis", "thigh_l", "thigh_r"} <= set(idx):
        log("WARNING: hip placement skipped — pelvis/thigh_l/thigh_r not all present")
        return None
    G = _fk(bones)
    mid = _pos(G[idx["pelvis"]])[0]
    verts = data["mesh"]["vertices"]

    def widest(side):
        """Furthest hip FLESH from the midline on `side`, from pelvis/thigh-owned verts only."""
        own = {idx["pelvis"], idx[f"thigh_{side}"]}
        sgn = 1.0 if side == "l" else -1.0
        d = [sgn * (v["p"][0] - mid) for v in verts
             if any(v["joints"][k] in own and v["weights"][k] >= 0.5 for k in range(4))]
        d = [x for x in d if x > 0.0]
        return max(d) if d else None

    W = [[row[:] for row in G[i]] for i in range(len(bones))]
    report = {}
    for side in ("l", "r"):
        w = widest(side)
        t = idx[f"thigh_{side}"]
        if w is None:
            log(f"WARNING: hip placement skipped for thigh_{side} — no hip flesh found")
            continue
        cur = _pos(G[t])[0]
        tgt = mid + (0.5 * w if side == "l" else -0.5 * w)
        W[t][0][3] = tgt          # WIDTH only: x moves, y/z (and every other bone) untouched
        report[side] = (cur, tgt, w)

    # Rebuild locals from the corrected world frames. Only the thigh frames changed, so every other
    # bone keeps its exact world position — the child locals simply absorb the parent's shift.
    for i, b in enumerate(bones):
        p = b["parent"]
        b["local"] = _cm(W[i] if p < 0 else _mul(_inv(W[p]), W[i]))
        b["inverse_bind"] = _cm(_inv(W[i]))
    return report


def reorient_to_canonical(data, kat_json_path):
    """Rebuild each bone's rest frame so the shared Katanami clip library reproduces Katanami's
    STANCE on THIS body — at this rig's own POSITIONS (proportions), upright.

    Two steps:
      1. Base frames G0 = Katanami's WORLD orientation + this rig's positions (fixes the Mixamo
         Y-down-bone vs UE X-down-bone convention; whole-body orientation is Katanami's).
      2. LIMB-ALIGN each limb bone (arms, legs, feet — see _LIMB_CHILD): rotate its frame
         (orientation only, position fixed) by the minimal rotation mapping Katanami's limb
         direction onto THIS body's, so the bone's axis points down this body's own limb. Torso
         bones stay at G0 (Katanami's orientation), so pelvis/spine/shoulders cannot tilt and the
         torso stance reproduces as authored.

    Why this reproduces the stance: playback is ABSOLUTE-orientation retarget (rotation-only; each
    bone keeps its own rest translation for proportions; NO per-bone rotation compensation). With a
    limb bone's rest frame aligned to its limb, its child-joint offset lies along the bone axis, so
    under a clip the limb points where Katanami's bone points — the stance translates 1:1 to any
    proportions. If a limb frame is left at Katanami's orientation on a body of different build, that
    offset is off-axis and the limb over-swings (elbows pulled in, feet turned — the mistranslation).
    There is deliberately NO retarget_rot: a per-bone t*s^-1 compensation turns this into
    additive-from-bind, which re-introduces the over-swing (measured), so it is intentionally absent.

    Verified by ABSOLUTE orientation (per the 2026-07-15 process lesson, memory 03BBF8F4): every limb
    direction matches Katanami's idle to <1deg while torso bones stay byte-identical to G0, so the
    body cannot tilt. A prior attempt that re-oriented the pelvis toward its child tilted the whole
    body 28deg and was reverted — hence torso bones are never limb-aligned here."""
    Kb = json.load(open(kat_json_path))["skeleton"]["bones"]
    Kidx = {b["name"]: i for i, b in enumerate(Kb)}; KG = _fk(Kb)
    bones = data["skeleton"]["bones"]; FG = _fk(bones)
    idx = {b["name"]: i for i, b in enumerate(bones)}

    # 1. base canonical frames: Katanami WORLD orientation + THIS rig's position.
    G0 = []
    for i, b in enumerate(bones):
        pos = _pos(FG[i])
        src = KG[Kidx[b["name"]]] if b["name"] in Kidx else FG[i]
        G0.append([[src[r][c] if c < 3 else pos[r] for c in range(4)] for r in range(3)]
                  + [[0.0, 0.0, 0.0, 1.0]])

    # 2. limb-align each limb frame (orientation only). Map Katanami's limb dir v -> this body's u.
    T = [[row[:] for row in G0[i]] for i in range(len(bones))]
    for i, b in enumerate(bones):
        nm = b["name"]; ch = _LIMB_CHILD.get(nm)
        if ch and nm in Kidx and ch in Kidx and ch in idx:
            u = _sub(_pos(G0[idx[ch]]), _pos(G0[i]))          # body limb dir (world)
            v = _sub(_pos(KG[Kidx[ch]]), _pos(KG[Kidx[nm]]))  # Katanami limb dir (world)
            newR = _mul(_align(v, u), _rot3(G0[i]))           # rotate the frame v->u (minimal)
            T[i] = [[newR[r][c] if c < 3 else _pos(G0[i])[r] for c in range(4)]
                    for r in range(3)] + [[0.0, 0.0, 0.0, 1.0]]

    # 3. derive local + inverse_bind from the final frames T (absolute retarget; no retarget_rot).
    for i, b in enumerate(bones):
        p = b["parent"]
        newL = T[i] if p < 0 else _mul(_inv(T[p]), T[i])
        b["local"] = _cm(newL)
        b["inverse_bind"] = _cm(_inv(T[i]))
        b.pop("retarget_rot", None)
    return sum(1 for b in bones if b["name"] in Kidx)

def infer_canonical_bones(data, canon_json_path):
    """Add the canonical bones Meshy never produces — for the 63-bone target: 30 fingers, 8
    twists, 2 weapon sockets — derived from the canonical reference rig scaled onto THIS body's
    own limbs. Data-driven: whatever the canonical rig has and this rig lacks gets inferred.

    Runs AFTER reorient_to_canonical, and that ordering is what makes it correct — every parent
    frame is already canonical, so each inferred bone can simply be hung off its parent in the
    reference's own local offset:

      * A twist's parent (upperarm/lowerarm/thigh/calf) is LIMB-ALIGNED by reorient — its axis
        already points down THIS body's limb. So the reference's local offset, applied in that
        frame and scaled by this body's limb-length ratio, lands the twist at the same FRACTION
        along this body's own limb. "Interpolate along the parent limb" falls out of the
        limb-align for free; there is nothing to interpolate by hand.
      * `hand_l/r` is deliberately NOT limb-aligned — it keeps Katanami's world orientation — so
        a finger chain hung off it reproduces the reference hand's orientation exactly, which is
        what the clips' absolute rotations expect. A uniform scale preserves direction, so the
        inferred fingers need no limb-align of their own (it would be a no-op).

    SCALE: Meshy gives no hand-length source (`hand_*` is a leaf), so fingers and the weapon
    sockets are sized by the FOREARM ratio — "a straight hand of standard proportion" (user's
    call, 2026-07-16); refine in-engine (paperdoll/packeditor) later. Bone LENGTHS are the part
    of Meshy's output we trust; its joint WIDTHS are not (memory 03BBF8F4), and nothing here
    depends on a width. Each hand is sized by its OWN forearm, so the two scales differ slightly.

    The forearm proxy is VALIDATED against the body's own mesh (2026-07-16), not assumed — it
    matters because a body's arms need not scale with its height: PrismHumanBaseA's forearm is
    1.010x the reference while she is only 0.910x its height. Measured: her mesh hand is 17.20 cm
    wrist->fingertip (sane for 170 cm); the reference's BONE hand (hand_l -> middle_03_l joint) is
    15.56 cm; allowing the normal ~1.5-2 cm of fingertip pad beyond the last joint puts her true
    hand ratio at ~0.98-1.01. The forearm proxy (1.010) lands within 1-3% of that; a height-derived
    proxy (0.910) would undersize the hands by 7-9%. The hand follows the ARM, not the height.
    (Do NOT try to re-derive this from the reference's MESH: BaseHumanFemale's hand weights are
    junk — the median vertex in its hand subtree sits 26.7 cm from the wrist, a Data-Transfer
    artifact of `rig_meshy_base.py`. Its BONES are Katanami's and are sound; its weights are not.)

    WEIGHTS: the inferred bones carry NONE. They are appended after the mesh's per-vertex joint
    indices are baked, so no vertex references them and no existing index shifts. Weapon_L/R are
    attachment points and are immediately functional; the twists and fingers resolve and rotate
    but cannot DEFORM until each body's own hand mesh is weighted to them — the follow-on."""
    Cb = json.load(open(canon_json_path))["skeleton"]["bones"]
    Cidx = {b["name"]: i for i, b in enumerate(Cb)}
    CG = _fk(Cb)
    bones = data["skeleton"]["bones"]
    idx = {b["name"]: i for i, b in enumerate(bones)}
    G = _fk(bones)  # canonical world frames (== reorient's T; local was derived from it)

    def _dist(Gx, a, b):
        pa, pb = _pos(Gx[a]), _pos(Gx[b])
        return math.sqrt(sum((pa[i] - pb[i]) ** 2 for i in range(3)))

    def limb_ratio(limb):
        """This body's `limb` bone length / the reference's — trusting Meshy's LENGTHS."""
        ch = _LIMB_CHILD.get(limb)
        if not (ch and limb in idx and ch in idx and limb in Cidx and ch in Cidx):
            return 1.0
        ref = _dist(CG, Cidx[limb], Cidx[ch])
        return _dist(G, idx[limb], idx[ch]) / ref if ref > 1e-9 else 1.0

    # Fingers + weapon sockets hang off the hand; Meshy has no hand length, so size them by the
    # forearm (lowerarm -> hand is that limb in _LIMB_CHILD).
    hand_scale = {"hand_l": limb_ratio("lowerarm_l"), "hand_r": limb_ratio("lowerarm_r")}
    gap = [b for b in Cb if b["name"] not in idx]  # Cb is topological -> parents precede children

    scale = {}
    for b in gap:
        pnm = Cb[b["parent"]]["name"] if b["parent"] >= 0 else None
        if pnm in scale:          # chain continuing off an inferred bone (finger _02/_03)
            scale[b["name"]] = scale[pnm]
        elif pnm in hand_scale:   # fingers + weapon sockets
            scale[b["name"]] = hand_scale[pnm]
        elif pnm in _LIMB_CHILD:  # twists: keep their fraction along their own parent limb
            scale[b["name"]] = limb_ratio(pnm)
        else:
            scale[b["name"]] = 1.0

    added = []
    for b in gap:
        nm = b["name"]
        pnm = Cb[b["parent"]]["name"] if b["parent"] >= 0 else None
        if pnm not in idx:
            log(f"WARNING: skipping '{nm}' — parent '{pnm}' is absent from this rig")
            continue
        s = scale[nm]
        L = _m(b["local"])
        newL = [[L[r][c] if c < 3 else L[r][3] * s for c in range(4)] for r in range(3)] \
            + [[0.0, 0.0, 0.0, 1.0]]
        W = _mul(G[idx[pnm]], newL)  # parent frame is already canonical
        bones.append({"name": nm, "parent": idx[pnm], "local": _cm(newL),
                      "inverse_bind": _cm(_inv(W))})
        idx[nm] = len(bones) - 1
        G.append(W)
        added.append(nm)
    return added, hand_scale


_spec = importlib.util.spec_from_file_location("fr_local", os.path.join(args.tools, "io_scene_flicker_rig.py"))
fr = importlib.util.module_from_spec(_spec); _spec.loader.exec_module(fr)

# ---- import FBX ----
before = set(bpy.data.objects)
bpy.ops.import_scene.fbx(filepath=args.fbx)
new = [o for o in bpy.data.objects if o not in before]
arm = next(o for o in new if o.type == "ARMATURE")
mesh = next(o for o in new if o.type == "MESH")
log(f"imported armature '{arm.name}' bones={len(arm.data.bones)}  mesh '{mesh.name}' verts={len(mesh.data.vertices)}")

# strip any FBX namespace prefix (e.g. 'mixamorig:') so the raw Meshy names match RENAME
bpy.ops.object.select_all(action="DESELECT"); arm.select_set(True); bpy.context.view_layer.objects.active = arm
bpy.ops.object.mode_set(mode="EDIT")
for eb in arm.data.edit_bones:
    if ":" in eb.name:
        eb.name = eb.name.split(":")[-1]
# drop tip bones
for nm in list(DROP):
    eb = arm.data.edit_bones.get(nm)
    if eb:
        arm.data.edit_bones.remove(eb)
# rename bones
unmapped = []
for eb in arm.data.edit_bones:
    if eb.name in RENAME:
        eb.name = RENAME[eb.name]
    elif eb.name not in RENAME.values():
        unmapped.append(eb.name)
bpy.ops.object.mode_set(mode="OBJECT")
if unmapped:
    log(f"WARNING: {len(unmapped)} bone(s) had no canonical mapping (left as-is): {unmapped}")
# rename the matching vertex groups (bone rename does NOT rename groups)
for vg in mesh.vertex_groups:
    base = vg.name.split(":")[-1]
    if base in RENAME:
        vg.name = RENAME[base]
    elif base != vg.name:
        vg.name = base
log(f"renamed to canonical; {len(arm.data.bones)} bones: {sorted(b.name for b in arm.data.bones)}")

# ---- decimate for a responsive per-frame CPU skin ----
if args.decimate < 0.999:
    bpy.ops.object.select_all(action="DESELECT"); mesh.select_set(True); bpy.context.view_layer.objects.active = mesh
    dm = mesh.modifiers.new("dec", "DECIMATE"); dm.decimate_type = "COLLAPSE"; dm.ratio = args.decimate
    bpy.ops.object.modifier_apply(modifier=dm.name)
    log(f"decimated mesh -> {len(mesh.data.vertices)} verts (ratio {args.decimate})")

# ---- unit scale: land bones in cm (Meshy FBX imports at 0.01 obj scale, bones cm-internal) ----
pelvis = arm.data.bones.get("pelvis")
pelvis_z = abs(pelvis.head_local.z) if pelvis else 0.0
unit_scale = 1.0 if pelvis_z > 10.0 else 100.0
log(f"pelvis head_local z={pelvis_z:.2f} -> unit_scale={unit_scale}")

# ---- save the base-color texture beside the output (paperdoll loads by basename from the body dir) ----
saved = []
for m in mesh.data.materials:
    if not m or not m.use_nodes:
        continue
    for n in m.node_tree.nodes:
        if n.type == "TEX_IMAGE" and n.image:
            out = os.path.join(OUT_DIR, (os.path.splitext(os.path.basename(n.image.filepath))[0] or n.image.name) + ".png")
            try:
                n.image.filepath_raw = out; n.image.file_format = "PNG"; n.image.save(); saved.append(os.path.basename(out))
            except Exception as e:
                log(f"texture save warning ({n.image.name}): {e}")
log(f"saved textures: {saved}")

# ---- export flicker.rig with retarget=true ----
data = fr.export_rig(arm, mesh, unit_scale=unit_scale, uv_name="", export_clips=False,
                     source_file=os.path.splitext(os.path.basename(args.out))[0], retarget=True)
hips = derive_hip_placement(data)   # BEFORE reorient: it consumes rest positions
if hips:
    for side, (cur, tgt, w) in hips.items():
        log(f"hip thigh_{side}: x {cur:+.2f} -> {tgt:+.2f} (out {abs(tgt)-abs(cur):+.2f}); "
            f"widest hip flesh {w:.2f} cm from midline")
n_reor = reorient_to_canonical(data, args.katanami_json)
log(f"reoriented {n_reor} bones to the canonical (UE X-down-bone) convention")
added, hand_scale = infer_canonical_bones(data, args.canonical_json)
_grp = lambda pred: sum(1 for n in added if pred(n))
log(f"inferred {len(added)} canonical bones "
    f"({_grp(lambda n: n.startswith(('thumb_', 'index_', 'middle_', 'ring_', 'pinky_')))} fingers, "
    f"{_grp(lambda n: '_twist_' in n)} twists, {_grp(lambda n: n.startswith('Weapon_'))} sockets); "
    f"hand scale l={hand_scale['hand_l']:.3f} r={hand_scale['hand_r']:.3f} (forearm ratio)")
with open(args.out, "w") as f:
    json.dump(data, f)
log(f"WROTE {args.out}: {len(data['skeleton']['bones'])} bones, "
    f"{len(data['mesh']['vertices'])} verts, retarget={data['retarget']}")
