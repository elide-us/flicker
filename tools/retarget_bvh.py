#!/usr/bin/env python3
"""
retarget_bvh.py — retarget Motifect locomotion BVH clips onto the canonical
PrismHumanBaseA (UE-mannequin, 66-bone) rig, emitting `flicker.rig`-shaped clip JSON.

    python3 tools/retarget_bvh.py INPUT.bvh -o OUT_DIR/     # one clip -> both variants
    python3 tools/retarget_bvh.py BVH_DIR/  -o OUT_DIR/     # every *.bvh -> both variants

`-o` is a BASE directory. Each source clip emits TWO variants (spec section C.4):
    OUT_DIR/In-Place/<stem>.json     bare stem — the loader cycles it by plain name
    OUT_DIR/RootMotion/<stem>.json   the loader namespaces it RM/<stem>

Pure Python (numpy for vector/matrix arithmetic, matching tools/skin_outfit.py). NO Blender.

Pipeline (spec docs/animation-system-rebuild-spec.md sections C.1/C.2/C.4):
  parse BVH hierarchy+motion  ->  Y-up -> Z-up basis change  ->  name-map (C.1)  ->
  rest-rebase (C.2)  ->  emit 30 fps clip, integer ticks, source frame count 1:1 (TAE),
  in BOTH the root-motion and in-place locomotion variants (C.4).

Why this is a retarget and not a re-author:
  Every clip stores rotations RELATIVE to its own skeleton's rest, so Motifect rotations cannot
  drop onto our bind directly. We replay each bone's motion FROM a matched base pose ONTO our
  mesh's ACTUAL bind: `Ta_b = Sa_b . inv(Sm_b) . A_b`, where A_b is the bind global rotation and
  Sm_b is the SOURCE posed to match our bind directions (source_base_pose). At the matched pose
  the skeleton sits exactly at the bind (skin undeformed); the source's per-bone articulation
  drives the rest. This is the corrected section C.2: the earlier draft used an idealised
  flat-foot A-pose RBP as the reference and the source ZERO pose (a T-pose) as the base, so it
  double-counted the T->A gap (arms crossed inward) and the RBP foot-level fought the pitched
  bind (toe-up feet). The mesh's real bind is now the single source of truth; the .rbp.json is
  obsolete.

  Emitted clips are `retarget: true` -> ROTATION-ONLY playback: the target rig keeps its OWN
  rest translations (so one clip drives every body/scale), and only the pelvis carries
  animated translation (the root of the animated hierarchy — BVH `Hips`). The loader
  (format.rs) rebases pelvis translation as `target_rest + (clip_T - source_rest)`, and we
  embed the target skeleton verbatim so `source_rest == target_rest` and every constant-T
  limb track contributes zero translation delta.

Root motion vs in-place (section C.4): a single retarget pass computes the rotations ONCE;
the two variants differ ONLY in the pelvis-track translation. The root-motion variant keeps
the full pelvis translation (planar travel intact — moves through the world). The in-place
variant pins the pelvis PLANAR translation (X/Y — up is +Z post-convert) to the pelvis RBP
rest while keeping the vertical bob (Z) and all rotations, so the clip plays on a treadmill
and never drifts the model off-camera.

Axes: emitted data is genuine Z-up / cm. The Z-up->Y-up render flip is ONE Model::world
matrix at draw (format.rs, gated on source_axis=="Z_up") — we never bake an axis flip into
bone data. The Motifect Y-up->Z-up convert IS baked in here, as a similarity transform
`C . q . inv(C)` on every source global rotation (NOT a naive component swap).
"""
import json
import os
import sys
import math
import copy
import argparse
import numpy as np

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SKELETON = os.path.join(REPO, "Alpha/content/characters/PrismHumanBaseA/PrismHumanBaseA.json")
# NOTE: PrismHumanBaseA.rbp.json is now OBSOLETE — the retarget reference is the mesh's actual
# bind (FK'd from the skeleton) plus a per-clip source base pose, not a hand-authored RBP.

TICK_RATE_HZ = 30


# --------------------------- quaternion algebra --------------------------------
# Quaternions are numpy [x, y, z, w] (glam convention). Hamilton product, so `qmul(a, b)`
# is "apply b, then a" and `qmul(parent, local)` composes a global rotation exactly as glam
# `parent * local` and as pose.rs composes parent*child.

def qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return np.array([
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ], dtype=np.float64)


def qconj(a):
    return np.array([-a[0], -a[1], -a[2], a[3]], dtype=np.float64)


def qnorm(a):
    n = math.sqrt(float(a[0] * a[0] + a[1] * a[1] + a[2] * a[2] + a[3] * a[3]))
    return a / n if n > 0 else np.array([0.0, 0.0, 0.0, 1.0])


def qinv(a):
    # Unit-quaternion inverse == conjugate; normalise first for numerical safety.
    return qconj(qnorm(a))


def q_axis(axis_char, deg):
    """Right-handed rotation of `deg` degrees about a principal axis, as a quaternion."""
    a = math.radians(deg)
    s, c = math.sin(a * 0.5), math.cos(a * 0.5)
    if axis_char == "X":
        return np.array([s, 0.0, 0.0, c])
    if axis_char == "Y":
        return np.array([0.0, s, 0.0, c])
    if axis_char == "Z":
        return np.array([0.0, 0.0, s, c])
    raise ValueError(axis_char)


def qrot(q, v):
    """Rotate 3-vector v by quaternion q: q . (v,0) . inv(q)."""
    p = np.array([v[0], v[1], v[2], 0.0])
    r = qmul(qmul(q, p), qconj(q))
    return r[:3]


def q_between(u, v):
    """Minimal (swing-only) quaternion rotating unit vector u onto unit vector v."""
    un = np.linalg.norm(u)
    vn = np.linalg.norm(v)
    u = u / un if un > 1e-12 else u
    v = v / vn if vn > 1e-12 else v
    d = float(np.dot(u, v))
    if d > 0.999999:
        return np.array([0.0, 0.0, 0.0, 1.0])
    if d < -0.999999:  # antiparallel: any perpendicular axis, 180 deg
        axis = np.cross(u, np.array([1.0, 0.0, 0.0]))
        if np.linalg.norm(axis) < 1e-6:
            axis = np.cross(u, np.array([0.0, 1.0, 0.0]))
        axis = axis / np.linalg.norm(axis)
        return np.array([axis[0], axis[1], axis[2], 0.0])
    axis = np.cross(u, v)
    s = math.sqrt((1.0 + d) * 2.0)
    return qnorm(np.array([axis[0] / s, axis[1] / s, axis[2] / s, s / 2.0]))


def quat_from_mat3(m):
    """glam-style quaternion from a 3x3 rotation matrix (column-vector convention)."""
    t = m[0, 0] + m[1, 1] + m[2, 2]
    if t > 0.0:
        s = math.sqrt(t + 1.0) * 2.0
        w = 0.25 * s
        x = (m[2, 1] - m[1, 2]) / s
        y = (m[0, 2] - m[2, 0]) / s
        z = (m[1, 0] - m[0, 1]) / s
    elif m[0, 0] > m[1, 1] and m[0, 0] > m[2, 2]:
        s = math.sqrt(1.0 + m[0, 0] - m[1, 1] - m[2, 2]) * 2.0
        w = (m[2, 1] - m[1, 2]) / s
        x = 0.25 * s
        y = (m[0, 1] + m[1, 0]) / s
        z = (m[0, 2] + m[2, 0]) / s
    elif m[1, 1] > m[2, 2]:
        s = math.sqrt(1.0 + m[1, 1] - m[0, 0] - m[2, 2]) * 2.0
        w = (m[0, 2] - m[2, 0]) / s
        x = (m[0, 1] + m[1, 0]) / s
        y = 0.25 * s
        z = (m[1, 2] + m[2, 1]) / s
    else:
        s = math.sqrt(1.0 + m[2, 2] - m[0, 0] - m[1, 1]) * 2.0
        w = (m[1, 0] - m[0, 1]) / s
        x = (m[0, 2] + m[2, 0]) / s
        y = (m[1, 2] + m[2, 1]) / s
        z = 0.25 * s
    return qnorm(np.array([x, y, z, w]))


# Y-up -> Z-up basis change, applied as a similarity transform to every source global
# rotation and to the root translation. Rx(+90deg): source up (+Y) -> our up (+Z), source
# forward (+Z) -> our -Y; X unchanged. Verified by the foot/height diagnostics: the standing
# hip (source Y~93) lands at Z~93 (up), and the retargeted stance foot comes out level.
C_YUP_TO_ZUP = q_axis("X", 90.0)


def convert_global(q, C=C_YUP_TO_ZUP):
    """Similarity transform of a source-space global rotation into our Z-up space."""
    return qmul(qmul(C, q), qconj(C))


# ------------------------------- BVH parsing -----------------------------------

class BvhJoint:
    __slots__ = ("name", "parent", "offset", "channels")

    def __init__(self, name, parent):
        self.name = name
        self.parent = parent          # index into joints, or -1
        self.offset = [0.0, 0.0, 0.0]
        self.channels = []            # e.g. ["Zrotation","Yrotation","Xrotation"] (as declared)


def parse_bvh(path):
    """Parse a BVH file. Returns (joints, frames, frame_time).

    `joints` is a flat DFS-ordered list of BvhJoint. End Sites carry no channels and no name
    we map, so they are dropped. `frames` is a list of per-frame float lists. Channel order is
    respected AS DECLARED per joint (root: 6 = 3 pos + 3 rot; others: 3 rot)."""
    with open(path) as fh:
        tokens = fh.read().split("\n")

    joints = []
    stack = []          # stack of joint indices (or None for End Site)
    frames = []
    frame_time = 0.0
    i = 0
    n = len(tokens)
    in_motion = False

    def words(line):
        return line.strip().split()

    while i < n:
        w = words(tokens[i])
        i += 1
        if not w:
            continue
        head = w[0]
        if head in ("ROOT", "JOINT"):
            parent = None
            for s in reversed(stack):
                if s is not None:
                    parent = s
                    break
            parent = parent if parent is not None else -1
            j = BvhJoint(w[1], parent)
            joints.append(j)
            stack.append(len(joints) - 1)
        elif head == "End":  # "End Site" — OFFSET only, no CHANNELS; sentinel on the stack.
            stack.append(None)
        elif head == "OFFSET":
            if stack and stack[-1] is not None:
                joints[stack[-1]].offset = [float(w[1]), float(w[2]), float(w[3])]
        elif head == "CHANNELS":
            joints[stack[-1]].channels = w[2:]
        elif head == "}":
            stack.pop()
        elif head == "MOTION":
            in_motion = True
        elif in_motion and head == "Frames:":
            pass  # count is implied by the data rows we read
        elif in_motion and head == "Frame" and len(w) >= 3 and w[1] == "Time:":
            frame_time = float(w[2])
        elif in_motion:
            vals = [float(x) for x in w]
            if vals:
                frames.append(vals)

    return joints, frames, frame_time


def bvh_channel_layout(joints):
    """Flat list of (joint_index, [channel names]) in DFS order — the order motion floats
    are laid out per frame."""
    return [(idx, j.channels) for idx, j in enumerate(joints) if j.channels]


def bvh_frame_locals(joints, layout, frame):
    """Decode one motion frame into per-joint local rotation quats + the root position.

    Rotations are composed in the joint's DECLARED channel order: channels [Z,Y,X] ->
    q = qz . qy . qx (BVH applies them in listed order)."""
    local_q = [np.array([0.0, 0.0, 0.0, 1.0]) for _ in joints]
    root_pos = np.array([0.0, 0.0, 0.0])
    p = 0
    for idx, chans in layout:
        q = np.array([0.0, 0.0, 0.0, 1.0])
        pos = [0.0, 0.0, 0.0]
        for ch in chans:
            v = frame[p]
            p += 1
            if ch.endswith("position"):
                pos["XYZ".index(ch[0])] = v
            else:  # rotation
                q = qmul(q, q_axis(ch[0], v))
        local_q[idx] = q
        if joints[idx].parent == -1:
            root_pos = np.array(pos)
    return local_q, root_pos


def bvh_global_rotations(joints, local_q):
    """FK the source hierarchy: global rotation quat per joint (Y-up source space)."""
    g = [None] * len(joints)
    for idx, j in enumerate(joints):
        g[idx] = local_q[idx] if j.parent == -1 else qmul(g[j.parent], local_q[idx])
    return g


# ----------------------- name map (spec section C.1) ---------------------------
# target rig bone  ->  SOURCE joint whose GLOBAL rotation drives it.
# Leg names reconciled against the actual BVH: Motifect uses LeftLeg/LeftShin/LeftFoot/
# LeftToeBase (NOT Mixamo LeftUpLeg/LeftLeg/...), so thigh<-LeftLeg, calf<-LeftShin,
# foot<-LeftFoot, ball<-LeftToeBase — matching spec section C.1 exactly.
# neck_01 <- Neck2: Neck2's source GLOBAL already contains Neck1 (Sa_Neck2 = Sa_Neck1.local),
# so pointing neck_01 at Neck2 COMPOSES Neck1.Neck2 into the single neck bone (section C.1),
# and Head's own bend lands at `head`.
NAME_MAP = {
    "pelvis": "Hips",
    "spine_01": "Spine1",
    "spine_02": "Spine2",
    "spine_03": "Chest",
    "neck_01": "Neck2",   # composes Neck1 . Neck2
    "head": "Head",
    "jaw": "Jaw",
    "eye_l": "LeftEye",
    "eye_r": "RightEye",
}
_FINGERS = [("thumb", "Thumb"), ("index", "Index"), ("middle", "Middle"),
            ("ring", "Ring"), ("pinky", "Pinky")]
for _side_t, _side_s in (("l", "Left"), ("r", "Right")):
    NAME_MAP["clavicle_" + _side_t] = _side_s + "Shoulder"
    NAME_MAP["upperarm_" + _side_t] = _side_s + "Arm"
    NAME_MAP["lowerarm_" + _side_t] = _side_s + "ForeArm"
    NAME_MAP["hand_" + _side_t] = _side_s + "Hand"
    NAME_MAP["thigh_" + _side_t] = _side_s + "Leg"
    NAME_MAP["calf_" + _side_t] = _side_s + "Shin"
    NAME_MAP["foot_" + _side_t] = _side_s + "Foot"
    NAME_MAP["ball_" + _side_t] = _side_s + "ToeBase"
    for _ft, _fs in _FINGERS:
        for _n in (1, 2, 3):
            NAME_MAP["%s_%02d_%s" % (_ft, _n, _side_t)] = "%sHand%s%d" % (_side_s, _fs, _n)

# Bones deliberately left at rest (no Motifect source): twist bones + weapon sockets + root.
# They are simply absent from NAME_MAP, so no track is emitted for them (spec section C.1:
# "Twist bones ... leave at rest"; "Weapon_L/R never animated").


# --------------------------- target rig / RBP loading --------------------------

def m4(a):
    """16-float glam column-major array -> 4x4 numpy [row][col] (matches skin_outfit.m4)."""
    return np.array([[a[c * 4 + r] for c in range(4)] for r in range(4)], dtype=np.float64)


# our bone -> the child bone whose rest offset defines this bone's forward DIRECTION.
# Used to reconcile the source (T-pose) rest against our (A-pose) BIND per bone: we align the
# source rig's rest bone-direction onto our bind bone-direction (spec section C.2, rewritten).
# Bones with no entry (leaves: head/ball/finger tips/jaw/eyes) INHERIT their parent's
# reconciliation, so every mapped bone gets a well-defined base pose.
DIR_CHILD = {
    "pelvis": "spine_01", "spine_01": "spine_02", "spine_02": "spine_03",
    "spine_03": "neck_01", "neck_01": "head",
    "thigh_l": "calf_l", "calf_l": "foot_l", "foot_l": "ball_l",
    "thigh_r": "calf_r", "calf_r": "foot_r", "foot_r": "ball_r",
}
for _s in ("l", "r"):
    DIR_CHILD["clavicle_" + _s] = "upperarm_" + _s
    DIR_CHILD["upperarm_" + _s] = "lowerarm_" + _s
    DIR_CHILD["lowerarm_" + _s] = "hand_" + _s
    DIR_CHILD["hand_" + _s] = "middle_01_" + _s   # hand points toward the middle knuckle
    for _f in ("thumb", "index", "middle", "ring", "pinky"):
        DIR_CHILD["%s_01_%s" % (_f, _s)] = "%s_02_%s" % (_f, _s)
        DIR_CHILD["%s_02_%s" % (_f, _s)] = "%s_03_%s" % (_f, _s)


def _local_rot_quat(b):
    """Rotation quaternion of a bone's `local` matrix (column-vector 3x3, scale-stripped)."""
    m = m4(b["local"])[:3, :3].astype(np.float64)
    for c in range(3):
        n = math.sqrt(float(m[0, c] ** 2 + m[1, c] ** 2 + m[2, c] ** 2))
        if n > 1e-12:
            m[:, c] /= n
    return quat_from_mat3(m)


def load_target():
    """Load target skeleton bones + the mesh BIND reference for the retarget.

    Returns dict with bones (embedded verbatim), names, idx, parent, rest_transl, and:
      bind_global : name -> bone GLOBAL rest rotation quat (FK of the skeleton's OWN `local`
                    rotations) = the pose the mesh is actually skinned in (A-pose, pitched foot).
      bind_dir    : name -> unit world direction to the bone's DIR_CHILD at the bind pose.
    The retarget reconciles the source (T-pose) rest onto THIS bind (spec section C.2, rewritten:
    the mesh's ACTUAL bind is the reference, not an idealised flat-foot A-pose RBP — using an
    idealised reference rotated every clip away from the bind, toe-up feet and crossed arms)."""
    skel = json.load(open(SKELETON))
    bones = skel["skeleton"]["bones"]
    names = [b["name"] for b in bones]
    idx = {n: i for i, n in enumerate(names)}
    parent = [b["parent"] for b in bones]
    rest_transl = [[b["local"][12], b["local"][13], b["local"][14]] for b in bones]

    # FK the skeleton's own local rotations -> bind GLOBAL rotation per bone (A_b).
    locR = [_local_rot_quat(b) for b in bones]
    Ab = [None] * len(bones)
    for i in range(len(bones)):
        Ab[i] = locR[i] if parent[i] < 0 else qmul(Ab[parent[i]], locR[i])
    bind_global = {names[i]: Ab[i] for i in range(len(bones))}

    # bind world direction to each bone's DIR_CHILD (child rest offset rotated by the bone's bind).
    bind_dir = {}
    for i, nm in enumerate(names):
        c = DIR_CHILD.get(nm)
        if c is not None and c in idx:
            off = np.array(rest_transl[idx[c]], dtype=np.float64)  # child offset in bone frame
            n = np.linalg.norm(off)
            if n > 1e-9:
                bind_dir[nm] = qrot(Ab[i], off / n)

    return {"bones": bones, "names": names, "idx": idx, "parent": parent, "rest_transl": rest_transl,
            "bind_global": bind_global, "bind_dir": bind_dir}


# ----------------------- the rest-rebase (section C.2) -------------------------

def source_base_pose(joints, target):
    """Derive the SOURCE base pose Sm_b: the source rig posed so each bone points along OUR bind
    direction (spec section C.2, rewritten). This is the matched reference the section-C.2 rebase
    always needed but never had — the earlier code used the source ZERO pose (identity), which is
    a T-pose, so reconciling it onto our A-pose bind double-counted the T->A difference (crossed
    arms) and the RBP foot-level fought the pitched bind (toe-up feet).

    For BVH the source rest global is identity and a bone's rest world direction is just its child
    OFFSET (converted Y-up->Z-up); Sm_b is the minimal rotation taking that source direction onto
    our bind direction. Leaf bones (no DIR_CHILD) INHERIT their parent's Sm. Returns name->Sm quat
    (Z-up). The source rig is identical across all Motifect clips, so this is stable per clip."""
    jidx = {j.name: i for i, j in enumerate(joints)}
    names, parent = target["names"], target["parent"]
    bind_dir = target["bind_dir"]

    Sm = {}
    for i, nm in enumerate(names):
        src = NAME_MAP.get(nm)
        cbn = DIR_CHILD.get(nm)
        s_child = NAME_MAP.get(cbn) if cbn is not None else None
        if src is not None and nm in bind_dir and s_child in jidx:
            off = np.array(joints[jidx[s_child]].offset, dtype=np.float64)  # Y-up child offset
            n = np.linalg.norm(off)
            if n > 1e-9:
                s_dir = qrot(C_YUP_TO_ZUP, off / n)          # source rest bone dir, Z-up
                Sm[nm] = q_between(s_dir, bind_dir[nm])       # source rest dir -> our bind dir
                continue
        # leaf / unmapped: inherit the parent's reconciliation (identity if none up-chain)
        p = parent[i]
        Sm[nm] = Sm[names[p]] if p >= 0 and names[p] in Sm else np.array([0.0, 0.0, 0.0, 1.0])
    return Sm


def rebase_frame(Sa_zup, Sm, bind_global, target, name_map):
    """One frame of the rewritten section-C.2 retarget. Returns dict target_bone -> local quat.

    Per target bone b (all in Z-up, after the Y-up->Z-up convert), matched-base-pose retarget onto
    the mesh's ACTUAL bind (A_b = bind_global[b], Sm_b = source_base_pose):
        Ta_b = Sa_map(b) . inv(Sm_b) . A_b
    i.e. the source's motion FROM its base pose, replayed FROM our bind. At the matched base pose
    (Sa == Sm) this is exactly A_b (skin undeformed); the source's per-bone articulation drives the
    rest. Unmapped bones sit at the bind (Ta_b = A_b). Then
        local_b = inv(Ta_parent) . Ta_b
    is stored as the clip's per-bone local rotation."""
    names = target["names"]
    parent = target["parent"]

    Ta = [None] * len(names)
    for i, nm in enumerate(names):
        src = name_map.get(nm)
        A_b = bind_global[nm]
        if src is not None and src in Sa_zup:
            Ta[i] = qmul(qmul(Sa_zup[src], qinv(Sm[nm])), A_b)
        else:
            Ta[i] = A_b  # left at the bind rest

    out = {}
    for i, nm in enumerate(names):
        if name_map.get(nm) is None:
            continue  # only emit tracks for mapped bones
        p = parent[i]
        pg = Ta[p] if p >= 0 else np.array([0.0, 0.0, 0.0, 1.0])
        out[nm] = qnorm(qmul(qinv(pg), Ta[i]))
    return out


# -------------------------------- clip emission --------------------------------

def _round_q(q):
    return [round(float(x), 8) for x in q]


def _round_t(t):
    return [round(float(x), 6) for x in t]


def retarget_clip(bvh_path, target):
    """Retarget one BVH file -> a `flicker.rig` dict (schema per format.rs).

    This is the ROOT-MOTION variant: the pelvis carries its FULL retargeted translation
    (planar travel intact). The in-place variant is derived from this by `make_in_place`,
    which pins only the pelvis planar (X/Y) translation. A single retarget pass here computes
    all rotations ONCE; the two variants differ only in the pelvis-track translation (C.4)."""
    joints, frames, frame_time = parse_bvh(bvh_path)
    layout = bvh_channel_layout(joints)
    nframes = len(frames)
    Sm = source_base_pose(joints, target)     # source posed to match our bind (spec C.2, rewritten)
    bind_global = target["bind_global"]        # A_b: the mesh's actual bind, the retarget reference
    names = target["names"]
    idx = target["idx"]
    rest_transl = target["rest_transl"]

    # Pelvis (root of the animated hierarchy) carries motion. Rebase its trajectory off the
    # first frame and scale by the height proportion so the standing height matches our rig.
    _, root_pos0 = bvh_frame_locals(joints, layout, frames[0])
    src_hip_h = float(root_pos0[1])  # Y-up standing height
    tgt_hip_h = float(rest_transl[idx["pelvis"]][2])  # Z-up pelvis rest height
    prop = (tgt_hip_h / src_hip_h) if abs(src_hip_h) > 1e-6 else 1.0
    pelvis_rest = np.array(rest_transl[idx["pelvis"]], dtype=np.float64)

    tracks = {nm: [] for nm in NAME_MAP if nm in idx}

    for t in range(nframes):
        local_q, root_pos = bvh_frame_locals(joints, layout, frames[t])
        g = bvh_global_rotations(joints, local_q)
        Sa_zup = {joints[i].name: convert_global(g[i]) for i in range(len(joints))}
        locals_t = rebase_frame(Sa_zup, Sm, bind_global, target, NAME_MAP)

        delta = qrot(C_YUP_TO_ZUP, (root_pos - root_pos0)) * prop
        pelvis_T = pelvis_rest + delta

        for nm, q in locals_t.items():
            if nm not in tracks:
                continue
            T = pelvis_T if nm == "pelvis" else np.array(rest_transl[idx[nm]], dtype=np.float64)
            tracks[nm].append({"t": t, "T": _round_t(T), "R": _round_q(q), "S": [1.0, 1.0, 1.0]})

    ordered = [nm for nm in names if nm in tracks and tracks[nm]]
    clip_name = os.path.splitext(os.path.basename(bvh_path))[0]
    out = {
        "format": "flicker.rig", "version": 1,
        "source": {
            "file": os.path.basename(bvh_path), "fbx_version": "0",  # loader Source.fbx_version is a String
            "source_axis": "Z_up", "source_unit": "cm",
            "applied_transform": "bvh-retarget: Y_up->Z_up (Rx+90) + rest-rebase onto RBP",
            "textures": [],
        },
        "retarget": True,
        "skeleton": {"bones": target["bones"]},  # embed target rig verbatim (source_rest == target_rest)
        "mesh": {"vertices": [], "indices": [], "submeshes": [], "materials": []},
        "morphs": [],
        "clips": [{
            "name": clip_name,
            "tick_rate_hz": TICK_RATE_HZ,
            "duration_ticks": nframes,
            "tracks": [{"bone": nm, "keys": tracks[nm]} for nm in ordered],
        }],
    }
    return out


def make_in_place(rig, target):
    """Derive the IN-PLACE variant from a root-motion rig (spec section C.4).

    Pins the pelvis PLANAR translation to the pelvis RBP rest (X and Y — up is +Z after the
    Y-up->Z-up convert) while keeping the vertical bob (Z) and every rotation untouched. Only
    the pelvis track carries translation (all other bones are rotation-only, T == rest), so
    this touches ONLY the pelvis track. Net effect via the runtime rebase
    `target_rest + (clip_T - source_rest)`: planar delta -> 0 (pelvis holds over the origin),
    Z bob preserved. Aaron's spec: pelvis.T = [rest.x, rest.y, animated.z]."""
    idx = target["idx"]
    rest = target["rest_transl"][idx["pelvis"]]
    px, py = round(float(rest[0]), 6), round(float(rest[1]), 6)  # match the embedded source_rest

    ip = copy.deepcopy(rig)
    pinned = False
    for clip in ip["clips"]:
        for tr in clip["tracks"]:
            if tr["bone"] != "pelvis":
                continue  # only the pelvis (BVH Hips) carries translation
            for k in tr["keys"]:
                k["T"][0] = px   # pin planar X to rest
                k["T"][1] = py   # pin planar Y to rest
                # k["T"][2] (vertical bob) left animated
            pinned = True
    if not pinned:
        raise SystemExit("make_in_place: no pelvis track found to pin (name-map/emit changed?)")
    return ip


def dump_json(obj, path):
    """Deterministic serialisation: fixed key order (as built), stable float repr."""
    with open(path, "w") as fh:
        json.dump(obj, fh, indent=1)
        fh.write("\n")


def emit_variants(bvh_path, target, base_dir):
    """Retarget one BVH and write BOTH variants under base_dir/In-Place and base_dir/RootMotion."""
    rig_rm = retarget_clip(bvh_path, target)          # root motion (full pelvis travel)
    rig_ip = make_in_place(rig_rm, target)            # in place (pelvis planar pinned)
    stem = os.path.splitext(os.path.basename(bvh_path))[0]

    ip_dir = os.path.join(base_dir, "In-Place")
    rm_dir = os.path.join(base_dir, "RootMotion")
    os.makedirs(ip_dir, exist_ok=True)
    os.makedirs(rm_dir, exist_ok=True)

    ip_path = os.path.join(ip_dir, stem + ".json")
    rm_path = os.path.join(rm_dir, stem + ".json")
    dump_json(rig_ip, ip_path)
    dump_json(rig_rm, rm_path)

    clip = rig_rm["clips"][0]
    print("[retarget] %s -> In-Place/%s.json + RootMotion/%s.json (%d ticks, %d tracks)"
          % (os.path.basename(bvh_path), stem, stem, clip["duration_ticks"], len(clip["tracks"])))
    return ip_path, rm_path


def main():
    ap = argparse.ArgumentParser(description="Retarget Motifect BVH -> flicker.rig clip JSON "
                                             "(both In-Place + RootMotion variants).")
    ap.add_argument("input", help="a .bvh file or a directory of .bvh files")
    ap.add_argument("-o", "--out", required=True,
                    help="output BASE dir; In-Place/ + RootMotion/ subdirs are created under it")
    args = ap.parse_args()

    target = load_target()
    base = args.out
    os.makedirs(base, exist_ok=True)

    if os.path.isdir(args.input):
        files = sorted(f for f in os.listdir(args.input) if f.lower().endswith(".bvh"))
        for f in files:
            emit_variants(os.path.join(args.input, f), target, base)
    else:
        emit_variants(args.input, target, base)


if __name__ == "__main__":
    main()
