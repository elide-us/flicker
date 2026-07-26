#!/usr/bin/env python3
"""
flicker_rebase.py — THE canonical rest-rebase primitive for flicker's retarget pipeline.

ONE home for the math that reproduces a source pose on our rig by reconciling each bone against a
MATCHED base pose, then applying the source's per-bone motion. The call sites are "the same
principle, separate implementations" (memory 614E5958) — unified onto this module:

  1. Motifect BVH -> clip bake  (tools/retarget_bvh.py)                       — quaternion, per frame.
  2. Katanami-clip Blender rig  (tools/blender/rename_meshy_to_canonical.py)  — matrix, static.
  3. Static garment fit         (future — the import editor / content pipeline) — static.

The KERNEL is the minimal SWING rotation aligning one bone-DIRECTION onto another. `q_between`
(quaternion) and `align_mat` (matrix / Rodrigues) are the SAME rotation in two representations —
proven equivalent by tools/test_flicker_rebase.py. Each call site feeds its own source/target
directions and reference pose into this kernel; the higher-level rebase FORMULA differs per site
(retarget_bvh reconciles against our own mesh bind `Ta = Sa·Sm⁻¹·A`; the Blender tool reconciles a
new body against Katanami's world orientation), but the alignment primitive lives here, once.

LAW (never reintroduce): the retarget is ABSOLUTE-orientation, rotation-only. A per-bone
`retarget_rot = t·s⁻¹` additive-from-bind compensation was WRONG — it re-introduces limb over-swing
(measured) — and is permanently removed. Keep FK; IK is a deferred, separate build (memory 614E5958).

Pure Python + numpy; NO bpy, so the Blender tool can import it. Quaternions are numpy `[x, y, z, w]`
(glam convention); matrices are plain nested lists (row-major, the flicker.rig contract).
"""
import math

import numpy as np


# --------------------------- quaternion algebra --------------------------------
# Quaternions are numpy [x, y, z, w] (glam convention). Hamilton product, so `qmul(a, b)` is
# "apply b, then a" and `qmul(parent, local)` composes a global rotation exactly as glam
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
    """Minimal (swing-only) quaternion rotating unit vector u onto unit vector v.

    THE rest-rebase kernel in quaternion form (== `align_mat` in matrix form)."""
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


def slerp(q0, q1, u):
    """Shortest-arc spherical linear interpolation between unit quats q0,q1 at u in [0,1].
    Constant angular-velocity midpoints — the 30->60 Hz clip resample uses this."""
    q0 = qnorm(np.asarray(q0, dtype=np.float64))
    q1 = qnorm(np.asarray(q1, dtype=np.float64))
    d = float(np.dot(q0, q1))
    if d < 0.0:            # take the shorter arc across the quaternion double cover
        q1 = -q1
        d = -d
    if d > 0.9995:         # nearly parallel -> nlerp (avoids the sin(theta)~0 blow-up)
        return qnorm(q0 + u * (q1 - q0))
    theta = math.acos(max(-1.0, min(1.0, d)))
    s = math.sin(theta)
    return qnorm((math.sin((1.0 - u) * theta) / s) * q0 + (math.sin(u * theta) / s) * q1)


def quat_from_mat3(m):
    """glam-style quaternion from a 3x3 rotation matrix (numpy, column-vector convention)."""
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


# Y-up -> Z-up basis change, applied as a similarity transform to every source global rotation and
# to the root translation. Rx(+90deg): source up (+Y) -> our up (+Z), source forward (+Z) -> our -Y.
C_YUP_TO_ZUP = q_axis("X", 90.0)


def convert_global(q, C=C_YUP_TO_ZUP):
    """Similarity transform of a source-space global rotation into our Z-up space."""
    return qmul(qmul(C, q), qconj(C))


# ------------------------- matrix form of the primitive ------------------------
# The Blender rig tool works in plain-list 4x4 matrices; `align_mat` is the matrix twin of
# `q_between` (the SAME minimal swing rotation). Kept verbatim from that tool so its output is
# unchanged, and pinned equal to `q_between` by tools/test_flicker_rebase.py.

def _vnorm(v):
    d = math.sqrt(sum(x * x for x in v)) or 1.0
    return [x / d for x in v]


def _vcross(a, b):
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]


def align_mat(u, v):
    """Minimal rotation (4x4 nested list) taking unit vector u onto unit vector v (Rodrigues).
    THE rest-rebase kernel in matrix form (== `q_between` in quaternion form)."""
    u = _vnorm(u); v = _vnorm(v); c = sum(u[i] * v[i] for i in range(3)); ax = _vcross(u, v)
    s = math.sqrt(sum(x * x for x in ax))
    if s < 1e-8:
        return [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]] if c > 0 \
            else [[-1, 0, 0, 0], [0, -1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]]
    x, y, z = [a / s for a in ax]; C = 1 - c
    return [[c + x * x * C, x * y * C - z * s, x * z * C + y * s, 0],
            [y * x * C + z * s, c + y * y * C, y * z * C - x * s, 0],
            [z * x * C - y * s, z * y * C + x * s, c + z * z * C, 0],
            [0, 0, 0, 1]]
