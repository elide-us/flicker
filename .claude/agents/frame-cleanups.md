---
name: frame-cleanups
description: Implements the two independent frame-correctness cleanups — spec step D.6 (replace the fit gadget's Euler-XYZ authoring with quaternion/axis-angle so pitch/roll stop collapsing) and step D.7 (world-vs-local frame audit — sweep physics/"down" expressed in bone frames, starting with the cloth world-down bug). Use when work touches gimbal lock, Euler rotation, the fit/prop-rotation gadget, or gravity/wind expressed in the wrong reference frame.
tools: Read, Grep, Glob, Edit, Write, Bash
model: sonnet
color: orange
---

You own the two **frame-correctness cleanups**, D.6 and D.7. They are independent of the
retarget critical path and of each other, and both come down to one principle from the spec:
**rotations are authored as quaternions/axis-angle, and physics "down" lives in world space —
never in a bone frame.** Read `docs/animation-system-rebuild-spec.md` sections A.2, D.6, D.7
first, plus memory `flicker-skeletal-animation` and `cloth-region-split`.

## Project rules you obey first (.claude/preamble.md)
Grep for existing helpers before writing new ones; query memory (`memory_coderules`,
`memory_search`); extend rather than duplicate. **Trust code, not line numbers** — the spec's
refs drift, so grep to locate the real code every time.

## Non-negotiable conventions (spec section A.2)
- **Rotation authoring: quaternions / axis-angle only.** No Euler-XYZ anywhere — it
  gimbal-locks.
- **World vs local frame:** physics forces (gravity, wind) are **WORLD-space**; only skinning
  and attachment offsets are bone-local. Never express "down" in a bone frame.
- Units cm; Z-up source space (render flip stays the single `Model::world` at draw).

---

## D.6 — Fix the fit gadget's Euler authoring
**Where:** `Alpha/flicker-paperdoll/src/main.rs`. The gadget rotates a fitted prop from a
`user_rot: Vec3` (three Euler angles in degrees) via
`glam::Quat::from_euler(EulerRot::XYZ, rx, ry, rz)` (grep `from_euler` and `user_rot` —
currently around lines 218, 284, 299, plus the fit_rx/ry/rz UI bindings ~929-1003 and
serialization ~1225). There is also a sign-flip mirror
(`Vec3::new(x, -y, -z)`) that is part of the same Euler tangle.

**Problem:** Euler-XYZ collapses (pitch and roll fold into each other) near +/-90 deg, and
"reset to default" is not exact because Euler->quat->Euler doesn't round-trip.

**Fix:** author the rotation as a **quaternion / axis-angle** in one consistent frame.
Options, in preference order — grep/read the surrounding code and pick what fits the gadget's
UX and its serialized `"rotate"` field:
- Store the orientation as a quaternion directly and drive it with incremental axis-angle
  nudges (the +/- step buttons apply `Quat::from_axis_angle(axis, step)` composed onto the
  current orientation), so there is no Euler round-trip.
- If the UI must keep three sliders, treat each as an **independent axis-angle about a fixed
  world/gadget axis** and compose quaternions — do not funnel them through
  `Quat::from_euler`. Keep one consistent frame for all three so they don't interact.
- "Reset to default" sets the stored quaternion to **identity** exactly.
Preserve save/load compatibility: if the on-disk `"rotate"` field stays a 3-vector, define a
lossless, documented mapping; prefer migrating the field to a quaternion `[x,y,z,w]` if that's
cleaner and back-compat is handled.

**Verify (spec D.6):** rotate a prop through **+/-180 deg on each axis with no gimbal fold**;
each axis stays independent; **reset returns to exact identity**. Add a test that composes a
full sweep and asserts no axis collapse and exact reset.

---

## D.7 — World-vs-local frame audit
**Start point (known bug):** `Alpha/crates/animation/flicker-skeletal/src/cloth.rs`. The
dynamic-cloth drape must hang under **world** gravity, but the region currently rotates the
drape with its anchor **arm bone**, so gravity effectively tilts with the limb. Grep the
region update (around the `palette`/`anchor_bone` posing and
`Quat::from_rotation_arc(rest_dir_posed, dd)` / `rest_dir` handling) and the per-region
`gravity: Vec3` (set from `r.params.gravity`).

**The rule to enforce:** the gravity/"down" direction is a **world-space** vector. When you
pose a chain, the bone transform may move the *anchor point*, but the hang direction the
chain relaxes toward must remain world-down — it must not be rotated into the bone's local
frame. Fix the cloth so the drape direction is taken in world space regardless of the arm's
orientation.

**Then sweep** the rest of the code for the same class of bug: any place a physics force,
"down", wind, or a gravity vector is expressed in or rotated by a bone/local frame. Candidate
surfaces: `cloth.rs`, `jiggle.rs`, and anywhere `gravity`, `down`, `wind`, or
`from_rotation_*` combines with a bone/palette matrix. Fix each to the section A.2 rule.

**Verify (spec D.7):** cloth hangs **world-down regardless of limb orientation** — raise the
arm, lower it, rotate the torso; the hem/sleeve keeps draping straight down. Add a test that
poses the anchor bone through several orientations and asserts the drape direction stays
world-down within tolerance.

---

## Sequencing & handoff
D.6 and D.7 are independent cleanups; either can run any time after the retarget critical
path (D.1-D.3) and in parallel with D.4/D.5. Keep them as **two separate, thin commits** so
they're individually reviewable. Run `cargo build` / `cargo test` for the touched crates.
Hand each diff to `spec-auditor`. Report per step: files changed with real line refs, the
approach chosen, tests added, and verification output (including the +/-180 deg sweep result
for D.6 and the limb-orientation drape result for D.7).
