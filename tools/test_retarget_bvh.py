#!/usr/bin/env python3
"""
test_retarget_bvh.py — the five section-C.3/C.4 gate tests for the BVH retargeter. Pure Python,
NO app. Run:  python3 tools/test_retarget_bvh.py

  1. Identity    — Sm==A (source base pose == bind) and names 1:1 -> the rebase reduces to the
                   source LOCAL motion (composition-order/handedness check).
  2. Reproduces  — FK the retargeted walk_forward on the target rig: EVERY mapped limb points
                   where the SOURCE limb points (<2deg, all frames). This is the property that
                   fixes both the crossed arms and the toe-up feet; the raw base-A bind foot is
                   still pitched ~-38deg (we track the source instead of mutating the bind).
  3. Timing      — native retarget: output tick count == source frame count, tick_rate_hz == 30
                   (source native rate), ticks 0..N-1.
  4. Determinism — retargeting the same input twice yields byte-identical JSON.
  5. In-place/RM — In-Place pins the pelvis planar (X/Y) to rest (Z bob kept); RootMotion
                   keeps the pelvis's full planar travel (spec section C.4).
  6. 60Hz canon  — resample_rig upsamples 30->60 (memory 302BBB85): source frames survive
                   VERBATIM on the even ticks (per-frame TAE accuracy), odd ticks are the
                   equidistant slerp / lerp midpoints; same-rate is a no-op; deterministic.
"""
import os
import sys
import math
import json
import tempfile
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import retarget_bvh as R

REPO = R.REPO
WALK = os.path.join(REPO, "Alpha/content/source/Motifect/"
                          "Motifect_locomotion_complete_v1_0/BVH/walk_forward.bvh")


def q_close(a, b, tol=1e-6):
    """Quaternion equality up to double-cover sign."""
    a = R.qnorm(np.asarray(a, float)); b = R.qnorm(np.asarray(b, float))
    return abs(abs(float(np.dot(a, b))) - 1.0) < tol


# --------------------------------- test 1 --------------------------------------

def test_identity():
    """Synthetic chain root->a->b. When the source base pose Sm equals the bind reference A_b,
    Ta_b = Sa.inv(Sm).A = Sa.inv(A).A = Sa, so local_b = inv(Sa_parent).Sa_b = the source LOCAL
    rotation. Assert the rebase composition reproduces the source local motion exactly (this is
    the order/handedness check for the rewritten section-C.2 rebase)."""
    target = {"names": ["root", "a", "b"], "parent": [-1, 0, 1]}
    name_map = {"root": "root", "a": "a", "b": "b"}

    def fk(locals_by_name):
        g = {}
        g["root"] = R.qnorm(locals_by_name["root"])
        g["a"] = R.qmul(g["root"], R.qnorm(locals_by_name["a"]))
        g["b"] = R.qmul(g["a"], R.qnorm(locals_by_name["b"]))
        return g

    # arbitrary bind (A) + source anim LOCAL rotations
    bind_local = {"root": R.q_axis("Y", 12.0), "a": R.q_axis("X", -30.0), "b": R.q_axis("Z", 20.0)}
    anim_local = {"root": R.q_axis("Z", 40.0), "a": R.q_axis("Y", 55.0), "b": R.q_axis("X", -25.0)}
    A = fk(bind_local)                  # arbitrary bind globals (A_b)
    Sa = fk(anim_local)                 # source anim globals
    Sm = dict(A)                        # Sm == A  (the reduction precondition)

    out = R.rebase_frame(Sa, Sm, A, target, name_map)
    for nm in ("root", "a", "b"):
        assert q_close(out[nm], anim_local[nm]), (nm, out[nm], anim_local[nm])
    return "Sm==A reduces the rebase to the source local motion for root/a/b"


# --------------------------------- test 2 --------------------------------------

def _clip_dir_maxerr(rig, target, joints, frames):
    """FK the retargeted clip exactly as the runtime does (global[i] = global[parent] * local[i],
    clip R where a track exists else the bone's rest local rotation) and return the per-bone MAX
    angle (deg) between the posed bone's world direction and the SOURCE bone's world direction,
    over every frame."""
    names, idx, parent, bones = target["names"], target["idx"], target["parent"], target["bones"]
    rest_transl = target["rest_transl"]
    rest_rot = [R._local_rot_quat(b) for b in bones]
    clip = rig["clips"][0]
    trk = {t["bone"]: {k["t"]: np.array(k["R"], float) for k in t["keys"]} for t in clip["tracks"]}
    jidx = {j.name: i for i, j in enumerate(joints)}
    layout = R.bvh_channel_layout(joints)

    def our_dir(bn):
        c = R.DIR_CHILD.get(bn)
        if c is None or c not in idx:
            return None
        v = np.array(rest_transl[idx[c]], float); n = np.linalg.norm(v)
        return v / n if n > 1e-9 else None

    def src_dir(bn):
        c = R.DIR_CHILD.get(bn); sc = R.NAME_MAP.get(c) if c else None
        if sc not in jidx:
            return None
        o = np.array(joints[jidx[sc]].offset, float); n = np.linalg.norm(o)
        return R.qrot(R.C_YUP_TO_ZUP, o / n) if n > 1e-9 else None

    keys = ["upperarm_l", "lowerarm_l", "hand_l", "thigh_l", "calf_l", "foot_l",
            "upperarm_r", "lowerarm_r", "hand_r", "thigh_r", "calf_r", "foot_r"]
    maxerr = {k: 0.0 for k in keys}
    for f in range(clip["duration_ticks"]):
        g = [None] * len(bones)
        for i, nm in enumerate(names):
            lr = trk[nm][f] if (nm in trk and f in trk[nm]) else rest_rot[i]
            g[i] = lr if parent[i] < 0 else R.qmul(g[parent[i]], lr)
        lq, _ = R.bvh_frame_locals(joints, layout, frames[f])
        sg = R.bvh_global_rotations(joints, lq)
        Sa = {joints[i].name: R.convert_global(sg[i]) for i in range(len(joints))}
        for k in keys:
            od, sd = our_dir(k), src_dir(k)
            if od is None or sd is None:
                continue
            a, b = R.qrot(g[idx[k]], od), R.qrot(Sa[R.NAME_MAP[k]], sd)
            d = max(-1.0, min(1.0, float(np.dot(a / np.linalg.norm(a), b / np.linalg.norm(b)))))
            maxerr[k] = max(maxerr[k], math.degrees(math.acos(d)))
    return maxerr


def test_reproduces_source():
    target = R.load_target()
    idx = target["idx"]; rest_transl = target["rest_transl"]; parent = target["parent"]; bones = target["bones"]

    # Raw base-A REST foot->ball pitch: the taint EXISTS in the bind and we do NOT mutate it —
    # the fix tracks the source instead of leveling the bone.
    gq = [None] * len(bones)
    for i, b in enumerate(bones):
        lq = R._local_rot_quat(b)
        gq[i] = lq if parent[i] < 0 else R.qmul(gq[parent[i]], lq)
    v = R.qrot(gq[idx["foot_l"]], np.array(rest_transl[idx["ball_l"]]))
    rest_pitch = math.degrees(math.asin(v[2] / (np.linalg.norm(v) + 1e-9)))
    assert rest_pitch < -25.0, "base-A rest foot should still be pitched down (~-38deg), got %.1f" % rest_pitch

    # Retarget the real walk; every posed limb must point where the SOURCE limb points.
    joints, frames, _ = R.parse_bvh(WALK)
    rig = R.retarget_clip(WALK, target)
    maxerr = _clip_dir_maxerr(rig, target, joints, frames)
    worst_bone = max(maxerr, key=maxerr.get); worst = maxerr[worst_bone]
    assert worst < 2.0, "limb %s deviates %.1fdeg from the source (retarget not tracking)" % (worst_bone, worst)
    return ("bind foot still %.1fdeg (unmutated); all %d limbs track the source <%.2fdeg over %d frames "
            "(arms uncrossed, feet track)" % (rest_pitch, len(maxerr), worst, len(frames)))


# --------------------------------- test 3 --------------------------------------

def test_timing():
    target = R.load_target()
    joints, frames, ft = R.parse_bvh(WALK)
    nframes = len(frames)
    rig = R.retarget_clip(WALK, target)
    clip = rig["clips"][0]
    assert clip["tick_rate_hz"] == 30, clip["tick_rate_hz"]
    assert clip["duration_ticks"] == nframes, (clip["duration_ticks"], nframes)
    for tr in clip["tracks"]:
        assert len(tr["keys"]) == nframes, (tr["bone"], len(tr["keys"]))
        assert [k["t"] for k in tr["keys"]] == list(range(nframes)), tr["bone"]
    assert abs(ft - 1.0 / 30.0) < 1e-4, "source frame time not 30fps: %s" % ft
    return "%d source frames -> %d ticks @ 30hz, integer ticks 0..%d, 1:1 (TAE)" % (nframes, nframes, nframes - 1)


# --------------------------------- test 4 --------------------------------------

def test_determinism():
    target = R.load_target()
    with tempfile.TemporaryDirectory() as d:
        a = os.path.join(d, "a.json"); b = os.path.join(d, "b.json")
        R.dump_json(R.retarget_clip(WALK, target), a)
        R.dump_json(R.retarget_clip(WALK, target), b)
        ba = open(a, "rb").read(); bb = open(b, "rb").read()
        assert ba == bb, "output not byte-identical across runs"
    return "two runs -> byte-identical output (%d bytes)" % len(ba)


# --------------------------------- test 5 --------------------------------------

def _pelvis_keys(rig):
    for tr in rig["clips"][0]["tracks"]:
        if tr["bone"] == "pelvis":
            return tr["keys"]
    raise AssertionError("no pelvis track")


def test_in_place_vs_root_motion():
    """C.4 locomotion split. RootMotion keeps the pelvis's full planar (X/Y) travel; In-Place
    pins that planar translation to the pelvis REST while keeping the vertical bob (Z) and every
    rotation. Discriminating both ways: the in-place check fails if the pin is off (RM planar
    moves hundreds of cm), the root-motion check fails if travel were stripped."""
    target = R.load_target()
    rest = target["rest_transl"][target["idx"]["pelvis"]]
    rx, ry = round(float(rest[0]), 6), round(float(rest[1]), 6)

    rig_rm = R.retarget_clip(WALK, target)
    rig_ip = R.make_in_place(rig_rm, target)
    ip, rm = _pelvis_keys(rig_ip), _pelvis_keys(rig_rm)

    # In-place: pelvis X/Y constant == rest across EVERY frame; Z (bob) still varies.
    assert all(abs(k["T"][0] - rx) < 1e-6 for k in ip), "in-place pelvis X not pinned to rest"
    assert all(abs(k["T"][1] - ry) < 1e-6 for k in ip), "in-place pelvis Y not pinned to rest"
    zbob = max(k["T"][2] for k in ip) - min(k["T"][2] for k in ip)
    assert zbob > 1.0, "in-place lost the vertical bob (Z span %.2f)" % zbob

    # Root-motion: planar travel preserved — pelvis X/Y span the source's real travel.
    rmx = [k["T"][0] for k in rm]; rmy = [k["T"][1] for k in rm]
    planar = math.hypot(max(rmx) - min(rmx), max(rmy) - min(rmy))
    assert planar > 50.0, "root-motion pelvis planar travel missing (span %.1f cm)" % planar

    # In-place only touches translation — rotations must match the root-motion variant.
    assert all(q_close(a["R"], b["R"]) for a, b in zip(ip, rm)), "in-place altered pelvis rotation"

    return ("in-place pelvis X/Y pinned to rest (%.3f,%.3f), %.1fcm Z bob kept; "
            "root-motion %.0fcm planar travel preserved" % (rx, ry, zbob, planar))


# --------------------------------- test 6 --------------------------------------

def test_resample_60hz():
    """The 30->60 Hz canon resample (memory 302BBB85). The retarget math is native-rate; this
    upsamples it x2. Source frames must survive VERBATIM on the even ticks (per-frame TAE
    accuracy), the odd ticks must be the equidistant slerp midpoints, a same-rate request is a
    no-op, and the transform is deterministic."""
    target = R.load_target()
    native = R.retarget_clip(WALK, target)
    nclip = native["clips"][0]
    n = nclip["duration_ticks"]
    assert nclip["tick_rate_hz"] == 30, nclip["tick_rate_hz"]

    up = R.resample_rig(native, 60)
    uc = up["clips"][0]
    assert uc["tick_rate_hz"] == 60, uc["tick_rate_hz"]
    assert uc["duration_ticks"] == 2 * n - 1, (uc["duration_ticks"], n)

    ntrk = {t["bone"]: t["keys"] for t in nclip["tracks"]}
    for tr in uc["tracks"]:
        keys = tr["keys"]
        assert len(keys) == 2 * n - 1, (tr["bone"], len(keys))
        assert [k["t"] for k in keys] == list(range(2 * n - 1)), tr["bone"]
        src = ntrk[tr["bone"]]
        # even ticks == source frames, byte-for-byte (T/R/S preserved through the rate change)
        for k in range(n):
            e = keys[2 * k]
            assert e["T"] == src[k]["T"] and e["R"] == src[k]["R"] and e["S"] == src[k]["S"], \
                ("even-tick source not verbatim", tr["bone"], k)
        # odd ticks == the u=0.5 interpolant: unit quat, equidistant from both source neighbours
        # (implementation-independent slerp property), and T is the exact lerp mean.
        for k in range(n - 1):
            mid = keys[2 * k + 1]
            mq = np.asarray(mid["R"], float)
            assert abs(np.linalg.norm(mq) - 1.0) < 1e-5, ("odd-tick R not unit", tr["bone"], k)
            q0 = R.qnorm(np.asarray(src[k]["R"], float))
            q1 = R.qnorm(np.asarray(src[k + 1]["R"], float))
            d0, d1 = abs(float(np.dot(mq, q0))), abs(float(np.dot(mq, q1)))
            assert abs(d0 - d1) < 1e-4, ("odd-tick not equidistant (not slerp 0.5)", tr["bone"], k)
            expT = [round(0.5 * (src[k]["T"][c] + src[k + 1]["T"][c]), 6) for c in range(3)]
            assert all(abs(mid["T"][c] - expT[c]) < 1e-6 for c in range(3)), ("odd-tick T not lerp", tr["bone"], k)

    # same-rate resample is a no-op
    same = R.resample_rig(native, 30)["clips"][0]
    assert same["duration_ticks"] == n and same["tick_rate_hz"] == 30, "same-rate resample mutated the clip"

    # deterministic across runs
    with tempfile.TemporaryDirectory() as d:
        pa = os.path.join(d, "a.json"); pb = os.path.join(d, "b.json")
        R.dump_json(R.resample_rig(native, 60), pa)
        R.dump_json(R.resample_rig(native, 60), pb)
        assert open(pa, "rb").read() == open(pb, "rb").read(), "resample output not deterministic"

    return "%d src frames -> %d ticks @60hz; source verbatim on even ticks, slerp/lerp mids on odd" % (n, 2 * n - 1)


def main():
    tests = [("1 identity", test_identity), ("2 reproduces", test_reproduces_source),
             ("3 timing", test_timing), ("4 determinism", test_determinism),
             ("5 in-place/RM", test_in_place_vs_root_motion),
             ("6 60hz canon", test_resample_60hz)]
    failed = 0
    for label, fn in tests:
        try:
            msg = fn()
            print("PASS  test %-14s  %s" % (label, msg))
        except AssertionError as e:
            failed += 1
            print("FAIL  test %-14s  %s" % (label, e))
        except Exception as e:
            failed += 1
            print("ERROR test %-14s  %r" % (label, e))
    print("\n%d/%d tests passed" % (len(tests) - failed, len(tests)))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
