#!/usr/bin/env python3
"""
lint_clips.py — a mathematical QA pass over retargeted `flicker.rig` clips.

Scans each bone's per-frame quaternion track for the artifact classes that show up as
visible animation glitches, and reports `(clip, bone, frame, kind, magnitude)` WITHOUT
rendering anything. The kinds map to the failure modes seen in-window:

  HEMISPHERE  dot(q[t], q[t+1]) < 0. A quaternion and its negation are the SAME rotation,
              so a sign-flipped frame renders fine frozen but makes any slerp/blend that is
              not shortest-path take the 360-degree long way -> a one-frame "flip then snap
              back". FIXABLE by canonicalising the emitted signs (see --fix).
  LOOP-SEAM   the same test across the loop closure (last frame -> first frame) + the angle
              of that closure. Locomotion clips loop, so a kink here flips every cycle.
  POP         a frame the smooth motion does not pass through: the geodesic triangle excess
              ang(t-1,t)+ang(t,t+1) - ang(t-1,t+1). ~0 on a smooth path; large = a single-
              frame detour (a genuinely bad keyframe), which pops for one frame at any rate.
  TWIST       swing-twist decomposition about the bone's own length axis (deg). Large twist
              on a limb is the "thigh twists while crawling" glitch; because the *_twist_01
              helper bones carry no animation, a big femoral twist is not distributed and LBS
              candy-wrappers the hip.

    python3 tools/lint_clips.py Alpha/content/retarget/clips/In-Place/            # a directory
    python3 tools/lint_clips.py Alpha/content/retarget/clips/In-Place/army_crawl.json
    python3 tools/lint_clips.py <dir> --twist 40 --pop 18 --top 12                # tune + cap

Read-only. Pure Python (stdlib only) — the clip stores dense integer-tick keyframes, so
"frame" == tick. Report is grouped worst-first; a clean clip prints one OK line.
"""
import json
import os
import sys
import math
import argparse
from statistics import median

# ------------------------------- quaternion helpers ----------------------------
# Quaternions are [x, y, z, w] (glam / clip convention).

def qdot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]


def qnorm(q):
    n = math.sqrt(qdot(q, q))
    return [x / n for x in q] if n > 1e-12 else [0.0, 0.0, 0.0, 1.0]


def qconj(a):
    return [-a[0], -a[1], -a[2], a[3]]


def qmul(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return [aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz]


def quat_from_local(m16):
    """Rotation quat [x,y,z,w] from a bone's column-major 16-float `local` matrix (scale stripped)."""
    cols = []
    for c in range(3):
        v = [m16[c * 4 + 0], m16[c * 4 + 1], m16[c * 4 + 2]]
        n = math.sqrt(sum(x * x for x in v)) or 1.0
        cols.append([x / n for x in v])
    m = [[cols[c][r] for c in range(3)] for r in range(3)]  # m[row][col]
    t = m[0][0] + m[1][1] + m[2][2]
    if t > 0:
        s = math.sqrt(t + 1.0) * 2; w = 0.25 * s
        x = (m[2][1] - m[1][2]) / s; y = (m[0][2] - m[2][0]) / s; z = (m[1][0] - m[0][1]) / s
    elif m[0][0] > m[1][1] and m[0][0] > m[2][2]:
        s = math.sqrt(1 + m[0][0] - m[1][1] - m[2][2]) * 2; w = (m[2][1] - m[1][2]) / s
        x = 0.25 * s; y = (m[0][1] + m[1][0]) / s; z = (m[0][2] + m[2][0]) / s
    elif m[1][1] > m[2][2]:
        s = math.sqrt(1 + m[1][1] - m[0][0] - m[2][2]) * 2; w = (m[0][2] - m[2][0]) / s
        x = (m[0][1] + m[1][0]) / s; y = 0.25 * s; z = (m[1][2] + m[2][1]) / s
    else:
        s = math.sqrt(1 + m[2][2] - m[0][0] - m[1][1]) * 2; w = (m[1][0] - m[0][1]) / s
        x = (m[0][2] + m[2][0]) / s; y = (m[1][2] + m[2][1]) / s; z = 0.25 * s
    return qnorm([x, y, z, w])


def qrot(q, v):
    """Rotate 3-vector v by quaternion q."""
    r = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q))
    return r[:3]


def ang_between(a, b):
    """The actual rotation angle (deg) between two orientations — hemisphere-agnostic."""
    d = min(1.0, abs(qdot(a, b)))
    return math.degrees(2.0 * math.acos(d))


def twist_deg(q, axis):
    """Magnitude of q's rotation ABOUT `axis` (unit), via swing-twist decomposition (deg)."""
    proj = q[0] * axis[0] + q[1] * axis[1] + q[2] * axis[2]  # q.xyz . axis
    tw = qnorm([axis[0] * proj, axis[1] * proj, axis[2] * proj, q[3]])
    return math.degrees(2.0 * math.atan2(math.hypot(tw[0], math.hypot(tw[1], tw[2])), abs(tw[3])))


def child_axis_map(bones):
    """bone name -> unit direction to its down-chain child, in the bone's own local frame
    (the child bone's local translation). This is the axis a limb twists about."""
    kids = {}
    for i, b in enumerate(bones):
        p = b["parent"]
        if p >= 0:
            kids.setdefault(p, []).append(i)
    axes = {}
    for i, b in enumerate(bones):
        for c in kids.get(i, []):
            off = [bones[c]["local"][12], bones[c]["local"][13], bones[c]["local"][14]]
            n = math.sqrt(sum(x * x for x in off))
            if n > 1e-6:
                axes[b["name"]] = [off[0] / n, off[1] / n, off[2] / n]
                break  # first real child defines the length axis
    return axes


# --------------------------------- the linter ----------------------------------

def lint_clip(path, thr):
    data = json.load(open(path))
    clip = data["clips"][0]
    name = clip["name"]
    bones = data.get("skeleton", {}).get("bones", [])
    axes = child_axis_map(bones)
    rest_rot = {b["name"]: quat_from_local(b["local"]) for b in bones}
    flags = []          # (severity, kind, bone, frame, detail)
    for tr in clip["tracks"]:
        bone = tr["bone"]
        Q = [qnorm(k["R"]) for k in sorted(tr["keys"], key=lambda k: k["t"])]
        n = len(Q)
        if n < 3:
            continue
        av = [ang_between(Q[t], Q[t + 1]) for t in range(n - 1)]  # deg/frame
        med = median(av) if av else 0.0
        mad = median([abs(x - med) for x in av]) if av else 0.0
        spike = med + 6.0 * (mad if mad > 1e-6 else 1.0)         # robust outlier gate

        for t in range(n - 1):
            if qdot(Q[t], Q[t + 1]) < -1e-6:
                flags.append((100.0, "HEMISPHERE", bone, t, f"sign flip -> {t}->{t+1}"))

        # loop closure (locomotion clips loop): last -> first
        if qdot(Q[-1], Q[0]) < -1e-6:
            flags.append((90.0, "LOOP-SEAM", bone, n - 1, f"sign flip across loop {n-1}->0"))
        seam = ang_between(Q[-1], Q[0])
        if seam > spike and seam > 12.0:
            flags.append((seam, "LOOP-SEAM", bone, n - 1, f"{seam:.0f} deg jump {n-1}->0 (loop)"))

        for t in range(1, n - 1):
            a, b, c = av[t - 1], av[t], ang_between(Q[t - 1], Q[t + 1])
            pop = (a + b) - c  # geodesic triangle excess: detour off the smooth path
            if pop > thr["pop"] and b > spike:
                flags.append((pop, "POP", bone, t, f"detour {pop:.0f} deg (step {b:.0f} vs {c:.0f} thru)"))

        if bone in axes:
            rb = rest_rot.get(bone, [0.0, 0.0, 0.0, 1.0])
            ax = axes[bone]
            # Twist RELATIVE TO BIND, in the bone's own frame (q_delta = bind^-1 * q): strips the
            # swing, so a limb that merely flexes reads ~0 and only genuine ROLL about the bone
            # axis survives. Then the SWING at the peak disambiguates: a big roll with SMALL swing
            # is the bone spinning about its own length in place — anatomically implausible, the
            # candy-wrapper cause (AXIAL-ROLL). A big roll with big swing is just an extreme pose.
            tw = [twist_deg(qmul(qconj(rb), Q[t]), ax) for t in range(n)]
            mx = max(tw)
            if mx > thr["twist"]:
                at = tw.index(mx)
                f2 = qrot(qmul(qconj(rb), Q[at]), ax)
                sw = math.degrees(math.acos(max(-1.0, min(1.0, ax[0] * f2[0] + ax[1] * f2[1] + ax[2] * f2[2]))))
                if sw < 45.0:
                    flags.append((mx + 90.0, "AXIAL-ROLL", bone, at,
                                  f"{mx:.0f} deg roll vs bind, only {sw:.0f} deg swing (spins in place -> candy-wrap)"))
                else:
                    flags.append((mx, "TWIST", bone, at, f"{mx:.0f} deg roll vs bind (swing {sw:.0f} deg, extreme pose)"))

    flags.sort(key=lambda f: -f[0])
    return name, len(clip["tracks"]), flags


def main():
    ap = argparse.ArgumentParser(description="Mathematical QA pass over retargeted flicker.rig clips.")
    ap.add_argument("path", help="a clip .json or a directory of them")
    ap.add_argument("--twist", type=float, default=40.0, help="flag limb twist over this many deg (default 40)")
    ap.add_argument("--pop", type=float, default=18.0, help="flag single-frame detours over this many deg (default 18)")
    ap.add_argument("--top", type=int, default=8, help="max flags shown per clip (default 8)")
    ap.add_argument("--json", action="store_true", help="emit structured findings as JSON (for the import pipeline)")
    args = ap.parse_args()
    thr = {"twist": args.twist, "pop": args.pop}

    if os.path.isdir(args.path):
        files = sorted(os.path.join(args.path, f) for f in os.listdir(args.path) if f.endswith(".json"))
    else:
        files = [args.path]

    if args.json:
        out = []
        for f in files:
            name, ntracks, flags = lint_clip(f, thr)
            out.append({"clip": name, "tracks": ntracks,
                        "findings": [{"severity": round(sev, 1), "kind": kind, "bone": bone,
                                      "frame": frame, "detail": detail}
                                     for sev, kind, bone, frame, detail in flags]})
        print(json.dumps(out, indent=1))
        return

    totals = {}
    dirty = 0
    for f in files:
        name, ntracks, flags = lint_clip(f, thr)
        if not flags:
            print(f"OK    {name:<26} ({ntracks} tracks) — no anomalies")
            continue
        dirty += 1
        kinds = {}
        for sev, kind, *_ in flags:
            kinds[kind] = kinds.get(kind, 0) + 1
            totals[kind] = totals.get(kind, 0) + 1
        head = " ".join(f"{k}:{v}" for k, v in sorted(kinds.items()))
        print(f"\nFLAG  {name:<26} — {head}")
        # HEMISPHERE is one bulk-fixable class (sign canonicalisation) — count it, don't list each.
        listed = [f for f in flags if f[1] != "HEMISPHERE"]
        for sev, kind, bone, frame, detail in listed[:args.top]:
            print(f"        {kind:<10} {bone:<14} frame {frame:>3}  {detail}")
        if len(listed) > args.top:
            print(f"        … +{len(listed) - args.top} more (non-hemisphere)")
        if kinds.get("HEMISPHERE"):
            print(f"        HEMISPHERE {kinds['HEMISPHERE']} sign flips — bulk-fixable by canonicalising track signs")

    print(f"\n=== {dirty}/{len(files)} clips flagged · " +
          " ".join(f"{k}:{v}" for k, v in sorted(totals.items())) + " ===")


if __name__ == "__main__":
    main()
