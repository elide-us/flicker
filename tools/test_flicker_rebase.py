#!/usr/bin/env python3
"""
test_flicker_rebase.py — gate for the shared rest-rebase primitive (tools/flicker_rebase.py).

Proves the ONE primitive's two representations agree: the minimal swing rotation aligning u->v is
the SAME whether computed as a quaternion (`q_between`, used by the offline-BVH bake in
retarget_bvh.py) or a matrix (`align_mat`, used by the Blender rig tool). If these ever diverge the
two retarget call sites would silently disagree — this pins them together.

Scope note: antiparallel inputs are excluded. `align_mat`'s degenerate (u == -v) fallback hardcodes
a Z-axis 180° (valid only for in-plane inputs), a pre-existing property kept verbatim from the
Blender tool's `_align`; rest-rebase aligns corresponding limb DIRECTIONS between similar skeletons,
which are never antiparallel, so the fallback never fires in practice.

Run:  python3 tools/test_flicker_rebase.py
"""
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import flicker_rebase as fr


def q_close(a, b, tol=1e-6):
    """Quaternion equality up to double-cover sign."""
    a = fr.qnorm(np.asarray(a, float)); b = fr.qnorm(np.asarray(b, float))
    return abs(abs(float(np.dot(a, b))) - 1.0) < tol


def _mat3_of(m4):
    return np.array([[m4[r][c] for c in range(3)] for r in range(3)], dtype=np.float64)


# Deterministic (no RNG) direction pairs across a spread of angles, plus identity. NONE antiparallel.
_DIRS = [
    ([1, 0, 0], [0, 1, 0]),
    ([0, 1, 0], [0, 0, 1]),
    ([1, 0, 0], [1, 0, 0]),                    # identical -> identity
    ([1, 2, 3], [-3, 1, 2]),
    ([0.2, -0.5, 0.84], [0.7, 0.7, 0.14]),
    ([1, 1, 0], [0, 1, 1]),
    ([-2, 0.3, 1.1], [0.4, -1.7, 0.9]),
    ([0.9, 0.1, 0.42], [0.8, -0.2, 0.56]),     # near-parallel (small angle)
]


def test_matrix_and_quat_align_agree():
    """`align_mat` (matrix / Rodrigues) and `q_between` (quaternion) are the SAME rotation, and both
    actually carry u onto v. This is the unification invariant: the two call sites' primitives match."""
    worst_axis = 0.0
    for u, v in _DIRS:
        qm = fr.quat_from_mat3(_mat3_of(fr.align_mat(u, v)))
        qb = fr.q_between(u, v)
        assert q_close(qm, qb), ("matrix != quat", u, v, qm, qb)
        ru = fr.qrot(qb, np.asarray(u, float))
        vn = np.asarray(v, float) / np.linalg.norm(v)
        cos = max(-1.0, min(1.0, float(np.dot(ru / np.linalg.norm(ru), vn))))
        worst_axis = max(worst_axis, math.degrees(math.acos(cos)))
    return "matrix == quaternion for %d dir-pairs; align carries u->v within %.1e deg" % (len(_DIRS), worst_axis)


def test_primitive_properties():
    """Sanity on the shared quaternion kernel: identity, unit norm, a known axis round-trip."""
    assert q_close(fr.q_between([1, 0, 0], [1, 0, 0]), [0, 0, 0, 1]), "parallel -> identity"
    q = fr.q_between([1, 2, 3], [3, -1, 2])
    assert abs(np.linalg.norm(q) - 1.0) < 1e-9, "q_between returns a unit quaternion"
    r = fr.qrot(fr.q_axis("Z", 90.0), [1, 0, 0])
    assert np.allclose(r, [0, 1, 0], atol=1e-6), ("Z+90 takes +X to +Y", r)
    # convert_global is a similarity transform: it preserves the rotation angle.
    ang = 2.0 * math.acos(min(1.0, abs(fr.qnorm(fr.q_axis("Y", 37.0))[3])))
    cg = fr.convert_global(fr.q_axis("Y", 37.0))
    ang2 = 2.0 * math.acos(min(1.0, abs(fr.qnorm(cg)[3])))
    assert abs(ang - ang2) < 1e-9, "Y-up->Z-up convert must preserve the rotation angle"
    return "identity / unit-norm / axis round-trip / angle-preserving convert all hold"


def main():
    tests = [("1 matrix==quat", test_matrix_and_quat_align_agree),
             ("2 properties", test_primitive_properties)]
    failed = 0
    for label, fn in tests:
        try:
            print("PASS  test %-14s  %s" % (label, fn()))
        except AssertionError as e:
            failed += 1; print("FAIL  test %-14s  %s" % (label, e))
        except Exception as e:  # noqa: BLE001
            failed += 1; print("ERROR test %-14s  %r" % (label, e))
    print("\n%d/%d tests passed" % (len(tests) - failed, len(tests)))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
