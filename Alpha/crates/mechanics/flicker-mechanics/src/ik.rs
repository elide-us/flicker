//! IK SOLVERS + THE LIMB MODEL — G1 of the gait generator / IK solver design (37704D6B, Aaron's
//! ruling CD36B9BE): pure glam math over a skeleton's WORLD frames (`globals`, parents preceding
//! children), renderer- and signal-agnostic the way the gizmo math is, so the Clayworks bench, the
//! runtime driver and the tests bend chains with ONE set of solvers.
//!
//! - [`ccd`] is the Clayworks rig-step reach test, MOVED here (2026-09-08) — an extraction, not a
//!   copy: every bone of a chain turns about its own position so the end joint reaches a target,
//!   carrying its subtree.
//! - [`two_bone`] is the analytic limb solver every leg and foreleg uses: root → mid → end, the
//!   middle joint bending toward a POLE, the reach clamped so an unreachable target straightens
//!   the limb instead of tearing it.
//! - [`limbs_of`] reads a skeleton's chains off its canon NAMES (with the modular skeleton's
//!   numbered pair prefixes, `arm2_…`, `leg2_…`) and each pole off the REST pose — so a stifle
//!   bends forward, a hock back and a bird's ankle backward exactly as its module authored it,
//!   with no table to maintain. Every limb ends in an EFFECTOR: the ground-contact joint the
//!   foot planner plants (hoof, forehoof, a paw's digit, a bird's toe base) or the wing tip.

use glam::{Mat4, Quat, Vec3};

/// Which side of the body a limb is on, from its `_l` / `_r` suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimbSide {
    Left,
    Right,
}

/// What a chain is, by the names it was found under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimbKind {
    /// thigh → calf → foot; the effector is the hoof, else the ball, else the foot.
    Leg,
    /// upperarm → lowerarm → hand; the effector is the forehoof, else the foredigit, else the hand.
    Arm,
    /// A bird's calf → foot (the raised ankle) → ball (the toe base, the effector); the thigh
    /// stays inside the body.
    BirdLeg,
    /// upperarm → lowerarm → hand → the wing tip (a bird's `wing_tip`, a bat's leading digit
    /// tip) — a flap chain, not a planted one.
    Wing,
}

/// One solvable chain of a skeleton: three joints and the effector below them, with the
/// direction the middle joint bends toward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimbChain {
    pub kind: LimbKind,
    pub side: LimbSide,
    pub root: usize,
    pub mid: usize,
    pub end: usize,
    /// The joint a foot plant or a flap targets — `end`'s ground-contact child, or `end` itself.
    pub effector: usize,
    /// The unit direction the middle joint bends toward, read off the rest pose.
    pub pole: Vec3,
    /// The girdle joint above the root (a clavicle or scapula) that may turn a little for reach
    /// when the two-bone solve runs out of leg — a quadruped's foreleg swings from its shoulder
    /// blade. `None` for legs and for arms without one.
    pub shoulder: Option<usize>,
}

/// Turn `pivot_bone` and every bone under it by `rotation` about the pivot bone's own world
/// position. `parents` precede their children.
pub fn turn_subtree(globals: &mut [Mat4], parents: &[i32], pivot_bone: usize, rotation: Quat) {
    let Some(pivot) = globals.get(pivot_bone).map(|g| g.w_axis.truncate()) else {
        return;
    };
    turn_about(globals, parents, pivot_bone, pivot, rotation);
}

/// `root` and every bone under it, in bone order (`parents` precede their children).
pub fn subtree(parents: &[i32], root: usize) -> Vec<usize> {
    let mut inside = vec![false; parents.len()];
    let mut out = Vec::new();
    for i in root..parents.len() {
        if i == root {
            inside[i] = true;
        } else if let Ok(p) = usize::try_from(parents[i]) {
            if p < i && inside[p] {
                inside[i] = true;
            }
        }
        if inside[i] {
            out.push(i);
        }
    }
    out
}

/// Turn `bone` and its subtree by `rotation` about `pivot`, a world point that need not be
/// the bone's own position — the joint a feather group or a digit swings from. (A joint turns
/// about its own position with [`turn_subtree`].)
pub fn turn_about(globals: &mut [Mat4], parents: &[i32], bone: usize, pivot: Vec3, rotation: Quat) {
    let turn =
        Mat4::from_translation(pivot) * Mat4::from_quat(rotation) * Mat4::from_translation(-pivot);
    for i in subtree(parents, bone) {
        if let Some(g) = globals.get_mut(i) {
            *g = turn * *g;
        }
    }
}

/// CCD reach: turn each bone of `chain` (nearest the end first) about its own position so joint
/// `end` lands on `target`, carrying every descendant along — `sweeps` passes, or until the joint
/// is within `tolerance`. The chain's own positions never move (each turns about itself), so a
/// one-bone chain swings the joint on its sphere and a two-bone chain reaches like a limb.
/// Returns whether the end came within `tolerance`.
pub fn ccd(
    globals: &mut [Mat4],
    parents: &[i32],
    chain: &[usize],
    end: usize,
    target: Vec3,
    sweeps: usize,
    tolerance: f32,
) -> bool {
    for _ in 0..sweeps {
        for &b in chain {
            let (Some(pivot), Some(tip)) = (globals.get(b), globals.get(end)) else {
                continue;
            };
            let pivot = pivot.w_axis.truncate();
            let (v1, v2) = (tip.w_axis.truncate() - pivot, target - pivot);
            let (Some(a), Some(c)) = (v1.try_normalize(), v2.try_normalize()) else {
                continue;
            };
            turn_subtree(globals, parents, b, Quat::from_rotation_arc(a, c));
        }
        if globals
            .get(end)
            .is_some_and(|g| (g.w_axis.truncate() - target).length() < tolerance)
        {
            return true;
        }
    }
    globals
        .get(end)
        .is_some_and(|g| (g.w_axis.truncate() - target).length() < tolerance)
}

/// The two-bone solution as POSITIONS: where `mid` and `end` go for a chain of the current
/// lengths whose `root` stays put, reaching for `target` with the middle joint bent toward
/// `pole`. An unreachable target straightens the limb along the root→target line; a target
/// inside the folded reach folds it. The bend plane is the root→target line and the pole (the
/// current mid's own offset when the pole lies on the line).
pub fn two_bone_positions(
    root: Vec3,
    mid: Vec3,
    end: Vec3,
    target: Vec3,
    pole: Vec3,
) -> (Vec3, Vec3) {
    let l1 = (mid - root).length();
    let l2 = (end - mid).length();
    let Some(dir) = (target - root).try_normalize() else {
        return (mid, end);
    };
    let reach = (target - root).length();
    let eps = 1e-4 * (l1 + l2).max(1e-6);
    let dist = reach.clamp((l1 - l2).abs() + eps, (l1 + l2 - eps).max(eps));
    // Where along the line the middle joint's foot sits, and how far off it stands.
    let a = ((l1 * l1 - l2 * l2 + dist * dist) / (2.0 * dist)).clamp(-l1, l1);
    let h = (l1 * l1 - a * a).max(0.0).sqrt();
    let perpendicular = |v: Vec3| (v - dir * v.dot(dir)).try_normalize();
    let side = perpendicular(pole)
        .or_else(|| perpendicular(mid - root))
        .unwrap_or_else(|| dir.any_orthonormal_vector());
    let new_mid = root + dir * a + side * h;
    let new_end = root + dir * dist;
    (new_mid, new_end)
}

/// Solve a two-bone chain IN PLACE on the world frames: `root`'s subtree turns so root→mid points
/// at the solved middle, then `mid`'s subtree turns so mid→end points at the solved end — the
/// effector below rides along. Returns how far the end joint still is from `target` (zero when
/// it was reachable).
pub fn two_bone(
    globals: &mut [Mat4],
    parents: &[i32],
    root: usize,
    mid: usize,
    end: usize,
    target: Vec3,
    pole: Vec3,
) -> f32 {
    let at = |g: &[Mat4], i: usize| g.get(i).map(|m| m.w_axis.truncate());
    let (Some(r), Some(m), Some(e)) = (at(globals, root), at(globals, mid), at(globals, end))
    else {
        return f32::INFINITY;
    };
    let (new_mid, new_end) = two_bone_positions(r, m, e, target, pole);
    if let (Some(a), Some(b)) = ((m - r).try_normalize(), (new_mid - r).try_normalize()) {
        turn_subtree(globals, parents, root, Quat::from_rotation_arc(a, b));
    }
    if let (Some(m2), Some(e2)) = (at(globals, mid), at(globals, end)) {
        if let (Some(a), Some(b)) = ((e2 - m2).try_normalize(), (new_end - m2).try_normalize()) {
            turn_subtree(globals, parents, mid, Quat::from_rotation_arc(a, b));
        }
    }
    at(globals, end).map_or(f32::INFINITY, |e| (e - target).length())
}

/// How far the girdle joint follows a planted limb's swing toward its target, as a fraction of
/// the root's bearing change — a scapula rotates a third as far as the leg beneath it.
pub const SHOULDER_FOLLOW: f32 = 0.4;

/// PLANT a limb's EFFECTOR on `target`: the shoulder blade (if the chain has one) follows the
/// swing, then the leg is solved in ONE step, exactly.
///
/// THE LEG SWINGS AS ONE, THEN MAKES UP ITS LENGTH. The whole limb turns about its root to
/// bear on the target, every bone by the same angle; what is left is how far the target is,
/// and for that the HANG KEEPS ITS LEAN: what hangs below the end joint — a cannon and pastern
/// under a hock or a carpus, a paw under a wrist — stays as the swing left it, the end joint
/// is solved for by the two bones above it ([`two_bone`]), and the end joint then turns so the
/// hang points at the target again. Only where the two bones run out of length (or fold up
/// against each other) does the hang lean: just far enough, toward the limb's root, to bring
/// the end joint back within their reach. (Without the swing the two bones carry the whole of
/// a stride under a cannon held still, and a thigh swept 70–95° through one walk cycle.)
///
/// It replaces an iteration that re-read the hang after each two-bone solve as the cannon
/// turned with the bone above it. That converges only while the hang is the SHORTER lever; on
/// a hoofed leg it is the longer (47 % of one measured hind leg, against 25 % for its gaskin),
/// and the iteration ran away — ten centimetres of travel left the hoof oscillating 28 and 40 cm
/// off its target, to be rescued by CCD from wherever it had stopped (measured on the 17
/// hoofed sources, 2026-10-02: the leg's pose came out of the rescue, and its leftover of up to
/// 0.4 cm read as a leg with no reach).
///
/// A target past the whole chain's length straightens it, and the chain then bends for what is
/// left (CCD, distal joints first, the girdle joint last). Returns how far the effector still
/// is from `target`.
pub fn plant(globals: &mut [Mat4], parents: &[i32], limb: &LimbChain, target: Vec3) -> f32 {
    let at = |g: &[Mat4], i: usize| g.get(i).map(|m| m.w_axis.truncate());
    // The shoulder blade swings with the leg: turn the girdle joint a fraction of the way from
    // the root's current bearing toward the target's, before the leg itself solves.
    if let Some(sh) = limb.shoulder {
        if let (Some(s), Some(r)) = (at(globals, sh), at(globals, limb.root)) {
            if let (Some(now), Some(want)) = ((r - s).try_normalize(), (target - s).try_normalize())
            {
                let axis = now.cross(want);
                let angle = now.dot(want).clamp(-1.0, 1.0).acos() * SHOULDER_FOLLOW;
                if let Some(axis) = axis.try_normalize() {
                    if angle > 1e-4 {
                        turn_subtree(globals, parents, sh, Quat::from_axis_angle(axis, angle));
                    }
                }
            }
        }
    }
    // THE LEG SWINGS AS ONE: the whole limb turns about its root until the effector's bearing
    // is the target's — a leg is a pendulum from its hip, the cannon swinging with the bones
    // above it — and the joints are left only its LENGTH to make up.
    // The middle joint's bend goes round with it: a stifle points forward of its own leg.
    let mut pole = limb.pole;
    if let (Some(r), Some(e)) = (at(globals, limb.root), at(globals, limb.effector)) {
        if let (Some(now), Some(to)) = ((e - r).try_normalize(), (target - r).try_normalize()) {
            let swing = Quat::from_rotation_arc(now, to);
            turn_subtree(globals, parents, limb.root, swing);
            pole = swing * pole;
        }
    }
    let (Some(root), Some(mid), Some(end), Some(effector)) = (
        at(globals, limb.root),
        at(globals, limb.mid),
        at(globals, limb.end),
        at(globals, limb.effector),
    ) else {
        return f32::INFINITY;
    };
    // Where the end joint goes with the hang at the lean it has...
    let hang = effector - end;
    let mut want = target - hang;
    // ...unless the two bones cannot put it there: then on the sphere of the hang's length
    // round the target, at the nearest point to that lean their reach does take in.
    let (l1, l2) = ((mid - root).length(), (end - mid).length());
    let eps = 1e-4 * (l1 + l2).max(1e-6);
    let (near, far) = ((l1 - l2).abs() + eps, (l1 + l2 - eps).max(eps));
    let gap = (want - root).length();
    if let (Some(lean), Some(to_root)) = (hang.try_normalize(), (root - target).try_normalize()) {
        if gap > far || gap < near {
            let (span, length) = ((root - target).length(), hang.length());
            let limit = if gap > far { far } else { near };
            let cos = ((span * span + length * length - limit * limit) / (2.0 * span * length))
                .clamp(-1.0, 1.0);
            let off = |v: Vec3| (v - to_root * v.dot(to_root)).try_normalize();
            let side = off(-lean)
                .or_else(|| off(pole))
                .unwrap_or_else(|| to_root.any_orthonormal_vector());
            let sin = (1.0 - cos * cos).max(0.0).sqrt();
            want = target + (to_root * cos + side * sin) * length;
        }
    }
    two_bone(globals, parents, limb.root, limb.mid, limb.end, want, pole);
    if let (Some(e), Some(f)) = (at(globals, limb.end), at(globals, limb.effector)) {
        if let (Some(now), Some(to)) = ((f - e).try_normalize(), (target - e).try_normalize()) {
            turn_subtree(globals, parents, limb.end, Quat::from_rotation_arc(now, to));
        }
    }
    let mut miss = at(globals, limb.effector).map_or(f32::INFINITY, |e| (e - target).length());
    // Past the whole chain's length: let every joint bend for what is left, the small ones
    // first, the girdle joint last.
    if miss > 1e-3 {
        let mut chain = Vec::new();
        if limb.effector != limb.end {
            let mut walk = limb.effector;
            while let Some(&p) = parents.get(walk) {
                if p < 0 || p as usize == limb.end {
                    break;
                }
                chain.push(p as usize);
                walk = p as usize;
            }
        }
        chain.extend([limb.end, limb.mid, limb.root]);
        chain.extend(limb.shoulder);
        ccd(globals, parents, &chain, limb.effector, target, 24, 1e-3);
        miss = at(globals, limb.effector).map_or(f32::INFINITY, |e| (e - target).length());
    }
    miss
}

/// Every limb chain a skeleton carries, read off its canon names (`thigh_l`, `arm2_upperarm_r`,
/// …) with each pole taken from `rest` — the middle joint's offset from the root→end line, or a
/// forward bend when the rest is straight. Pairs come out left before right, in bone order.
pub fn limbs_of(names: &[&str], rest: &[Mat4]) -> Vec<LimbChain> {
    let find = |name: &str| names.iter().position(|n| *n == name);
    let at = |i: usize| rest.get(i).map(|m| m.w_axis.truncate());
    let pole_of = |root: usize, mid: usize, end: usize, default: Vec3| -> Vec3 {
        let (Some(r), Some(m), Some(e)) = (at(root), at(mid), at(end)) else {
            return default;
        };
        let Some(axis) = (e - r).try_normalize() else {
            return default;
        };
        let off = (m - r) - axis * (m - r).dot(axis);
        off.try_normalize().unwrap_or(default)
    };
    let mut out = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let Some((base, suffix)) = name.rsplit_once('_') else {
            continue;
        };
        let side = match suffix {
            "l" => LimbSide::Left,
            "r" => LimbSide::Right,
            _ => continue,
        };
        let s = suffix;
        if let Some(prefix) = base.strip_suffix("thigh") {
            let n = |b: &str| format!("{prefix}{b}_{s}");
            let (Some(calf), Some(foot)) = (find(&n("calf")), find(&n("foot"))) else {
                continue;
            };
            if let (Some(ball), Some(_)) = (find(&n("ball")), find(&n("hallux"))) {
                out.push(LimbChain {
                    kind: LimbKind::BirdLeg,
                    side,
                    root: calf,
                    mid: foot,
                    end: ball,
                    effector: ball,
                    pole: pole_of(calf, foot, ball, Vec3::Y),
                    shoulder: None,
                });
                continue;
            }
            let effector = find(&n("hoof")).or(find(&n("ball"))).unwrap_or(foot);
            out.push(LimbChain {
                kind: LimbKind::Leg,
                side,
                root: i,
                mid: calf,
                end: foot,
                effector,
                pole: pole_of(i, calf, foot, -Vec3::Y),
                shoulder: None,
            });
        } else if let Some(prefix) = base.strip_suffix("upperarm") {
            let n = |b: &str| format!("{prefix}{b}_{s}");
            let (Some(lowerarm), Some(hand)) = (find(&n("lowerarm")), find(&n("hand"))) else {
                continue;
            };
            let wing_tip = find(&n("wing_tip")).or(find(&n("wing_digit_1_03")));
            let (kind, effector) = match wing_tip {
                Some(tip) => (LimbKind::Wing, tip),
                None => (
                    LimbKind::Arm,
                    find(&n("forehoof"))
                        .or(find(&n("foredigit")))
                        .unwrap_or(hand),
                ),
            };
            out.push(LimbChain {
                kind,
                side,
                root: i,
                mid: lowerarm,
                end: hand,
                effector,
                pole: pole_of(i, lowerarm, hand, Vec3::Y),
                shoulder: find(&n("clavicle")),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight three-joint chain along +X (root at the origin), with a child below the end.
    fn chain() -> (Vec<Mat4>, Vec<i32>) {
        let at = |v: Vec3| Mat4::from_translation(v);
        (
            vec![
                at(Vec3::ZERO),
                at(Vec3::new(10.0, 0.0, 0.0)),
                at(Vec3::new(20.0, 0.0, 0.0)),
                at(Vec3::new(20.0, 0.0, -3.0)),
            ],
            vec![-1, 0, 1, 2],
        )
    }

    /// The two-bone solve lands the end on a reachable target within a hair, keeps both bone
    /// lengths, bends the middle joint toward the pole, straightens for a target out of reach,
    /// carries the effector below the end along, and leaves the root where it was.
    #[test]
    fn two_bone_reaches_bends_toward_the_pole_and_clamps() {
        let (mut g, parents) = chain();
        let target = Vec3::new(12.0, 8.0, 0.0);
        let miss = two_bone(&mut g, &parents, 0, 1, 2, target, Vec3::Z);
        assert!(miss < 1e-3, "miss {miss}");
        let p = |i: usize| g[i].w_axis.truncate();
        assert_eq!(p(0), Vec3::ZERO, "the root stays");
        assert!(
            ((p(1) - p(0)).length() - 10.0).abs() < 1e-3
                && ((p(2) - p(1)).length() - 10.0).abs() < 1e-3,
            "lengths kept"
        );
        assert!(p(1).z > 1.0, "the knee bends toward the pole: {}", p(1));
        assert!(
            (p(3) - p(2)).length() - 3.0 < 1e-3,
            "the effector rides below the end"
        );
        // Out of reach: straight along the line, the miss reported.
        let (mut g, parents) = chain();
        let far = Vec3::new(0.0, 40.0, 0.0);
        let miss = two_bone(&mut g, &parents, 0, 1, 2, far, Vec3::Z);
        let p = |i: usize| g[i].w_axis.truncate();
        assert!(
            (miss - 20.0).abs() < 1e-2,
            "20 short of a target 40 away: {miss}"
        );
        assert!(
            p(2).y > 19.9 && p(1).y > 9.9 && p(1).x.abs() < 1e-2,
            "straightened along the line"
        );
        // The opposite pole bends the other way.
        let (mut g, parents) = chain();
        two_bone(&mut g, &parents, 0, 1, 2, target, -Vec3::Z);
        assert!(g[1].w_axis.z < -1.0);
    }

    /// THE LEG SWINGS AS ONE AND THE HOOF LANDS IN ONE STEP — a hoofed hind leg with a cannon
    /// longer than its gaskin (one measured source's joints). A target as far from the hip as
    /// the hoof rests turns every bone by the same angle: the leg keeps its shape. A target
    /// nearer the hip folds the two bones over the hock and the cannon keeps its lean to the
    /// leg's line; one at the very end of the leg's length leans the cannon for it; all land
    /// within a hundredth of a millimetre and no bone changes length. Only past the whole
    /// chain's length is there a miss. (The iteration this replaced left the hoof 28–40 cm off
    /// a target 10 cm from its rest.)
    #[test]
    fn a_long_cannon_swings_with_its_leg_and_the_hoof_lands_in_one_step() {
        let at = |v: [f32; 3]| Mat4::from_translation(Vec3::from_array(v));
        let rest = vec![
            at([13.8, 44.2, 112.3]), // hip
            at([7.5, 56.0, 82.0]),   // stifle
            at([3.8, 72.5, 56.6]),   // hock
            at([5.6, 83.4, 11.0]),   // fetlock
            at([2.0, 88.9, 1.8]),    // hoof
        ];
        let parents = vec![-1, 0, 1, 2, 3];
        let names = ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"];
        let limb = limbs_of(&names, &rest)[0];
        assert_eq!((limb.end, limb.effector), (2, 4));
        let p = |g: &[Mat4], i: usize| g[i].w_axis.truncate();
        let (hip, hoof) = (p(&rest, 0), p(&rest, 4));
        // Every joint-to-joint distance of the chain: its bones' lengths and its SHAPE.
        let shape = |g: &[Mat4]| -> Vec<f32> {
            (0..5)
                .flat_map(|i| (i + 1..5).map(move |j| (i, j)))
                .map(|(i, j)| p(g, i).distance(p(g, j)))
                .collect()
        };
        let bones = |g: &[Mat4]| [1, 2, 3, 4].map(|i| p(g, i).distance(p(g, i - 1)));
        let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-2);
        // The cannon's lean to the leg's own line, hip to hoof.
        let lean = |g: &[Mat4]| {
            (p(g, 4) - p(g, 2))
                .normalize()
                .dot((p(g, 4) - p(g, 0)).normalize())
        };
        // A pure swing: 40° forward about the hip, the hoof as far from it as it rests.
        let swung = hip + Quat::from_axis_angle(Vec3::X, -40.0_f32.to_radians()) * (hoof - hip);
        let mut g = rest.clone();
        let miss = plant(&mut g, &parents, &limb, swung);
        assert!(miss < 1e-3, "a pure swing: {miss} cm off");
        assert!(
            same(&shape(&g), &shape(&rest)),
            "the leg swings as one and keeps its shape"
        );
        // Nearer the hip (a lifted hoof, a hoof stepped under the body): the two bones fold.
        for (ahead, up) in [(30.0_f32, 0.0_f32), (50.0, 0.0), (40.0, 15.0)] {
            let mut g = rest.clone();
            let target = hoof + Vec3::new(0.0, -ahead, up);
            let miss = plant(&mut g, &parents, &limb, target);
            assert!(miss < 1e-3, "{ahead} ahead, {up} up: {miss} cm off");
            assert!(same(&bones(&g), &bones(&rest)), "no bone changes length");
            assert!(
                (lean(&g) - lean(&rest)).abs() < 1e-3,
                "{ahead} ahead, {up} up: the cannon keeps its lean to the leg's line"
            );
            assert!(
                p(&g, 0).distance(p(&g, 2)) < p(&rest, 0).distance(p(&rest, 2)) - 1.0,
                "{ahead} ahead, {up} up: the two bones fold over the hock"
            );
        }
        // At the very end of the leg's length — a shade further back than it rests: the cannon
        // leans for it, and the hoof still lands.
        let mut g = rest.clone();
        let miss = plant(&mut g, &parents, &limb, hoof + Vec3::Y * 2.0);
        assert!(miss < 1e-3, "at the end of its length: {miss} cm off");
        assert!(lean(&g) > lean(&rest) + 1e-4, "the cannon leaned for it");
        // Past the whole chain: a miss, reported.
        let mut g = rest.clone();
        let miss = plant(&mut g, &parents, &limb, hoof + Vec3::Y * 20.0);
        assert!(miss > 5.0, "20 cm past a straight leg: {miss} cm off");
    }

    /// The CCD (Clayworks' reach test, moved here) brings the end within tolerance on a
    /// reachable target and reports a miss otherwise, never moving the chain's own pivots.
    #[test]
    fn ccd_reaches_within_tolerance() {
        let (mut g, parents) = chain();
        let target = Vec3::new(5.0, 12.0, 4.0);
        assert!(ccd(&mut g, &parents, &[1, 0], 2, target, 12, 0.05));
        assert!((g[2].w_axis.truncate() - target).length() < 0.05);
        assert_eq!(g[0].w_axis.truncate(), Vec3::ZERO);
        let (mut g, parents) = chain();
        assert!(!ccd(
            &mut g,
            &parents,
            &[1, 0],
            2,
            Vec3::new(0.0, 50.0, 0.0),
            12,
            0.05
        ));
    }

    /// The limb model reads chains off canon names: a humanoid leg (ball effector), a hoofed
    /// second-pair hind leg (`leg2_`, hoof effector, the stifle's forward pole from the rest), a
    /// foreleg (forehoof effector, the elbow's backward pole), a bird leg (calf → foot → ball,
    /// found by its hallux), a bird wing (wing tip) and a bat wing (the leading digit's tip);
    /// a straight rest falls back to a forward knee.
    #[test]
    fn limbs_are_read_off_the_names_with_poles_from_the_rest() {
        let mut names: Vec<&str> = Vec::new();
        let mut parents: Vec<i32> = Vec::new();
        let mut rest: Vec<Mat4> = Vec::new();
        let mut push = |n: &'static str, p: i32, at: Vec3| {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(at));
        };
        push("root", -1, Vec3::ZERO);
        // A straight humanoid leg.
        push("thigh_l", 0, Vec3::new(5.0, 0.0, 90.0));
        push("calf_l", 1, Vec3::new(5.0, 0.0, 45.0));
        push("foot_l", 2, Vec3::new(5.0, 0.0, 5.0));
        push("ball_l", 3, Vec3::new(5.0, -10.0, 0.0));
        // A hoofed hind leg as a SECOND pair, stifle forward, hock back.
        push("leg2_thigh_r", 0, Vec3::new(-9.0, 30.0, 90.0));
        push("leg2_calf_r", 5, Vec3::new(-9.0, 20.0, 55.0));
        push("leg2_foot_r", 6, Vec3::new(-9.0, 36.0, 30.0));
        push("leg2_ball_r", 7, Vec3::new(-9.0, 34.0, 10.0));
        push("leg2_hoof_r", 8, Vec3::new(-9.0, 33.0, 0.0));
        // A foreleg, elbow back.
        push("upperarm_l", 0, Vec3::new(9.0, -70.0, 100.0));
        push("lowerarm_l", 10, Vec3::new(9.0, -64.0, 80.0));
        push("hand_l", 11, Vec3::new(9.0, -68.0, 45.0));
        push("forehoof_l", 12, Vec3::new(9.0, -70.0, 0.0));
        // A bird leg with a hallux.
        push("thigh_r", 0, Vec3::new(-6.0, 0.0, 50.0));
        push("calf_r", 14, Vec3::new(-6.0, -6.0, 35.0));
        push("foot_r", 15, Vec3::new(-6.0, 4.0, 20.0));
        push("ball_r", 16, Vec3::new(-6.0, -2.0, 0.0));
        push("hallux_r", 17, Vec3::new(-6.0, 3.0, 0.0));
        // A bird wing and a bat wing as a second pair.
        push("upperarm_r", 0, Vec3::new(-8.0, 0.0, 120.0));
        push("lowerarm_r", 19, Vec3::new(-30.0, 2.0, 125.0));
        push("hand_r", 20, Vec3::new(-52.0, 3.0, 127.0));
        push("wing_tip_r", 21, Vec3::new(-80.0, 5.0, 126.0));
        push("arm2_upperarm_l", 0, Vec3::new(10.0, 6.0, 122.0));
        push("arm2_lowerarm_l", 23, Vec3::new(30.0, 8.0, 124.0));
        push("arm2_hand_l", 24, Vec3::new(45.0, 9.0, 125.0));
        push("arm2_wing_digit_1_03_l", 25, Vec3::new(85.0, 8.0, 124.0));

        let limbs = limbs_of(&names, &rest);
        let find = |kind: LimbKind, side: LimbSide, root: &str| {
            limbs
                .iter()
                .find(|l| l.kind == kind && l.side == side && names[l.root] == root)
                .copied()
                .unwrap_or_else(|| panic!("{kind:?} {side:?} rooted at {root}"))
        };
        let leg = find(LimbKind::Leg, LimbSide::Left, "thigh_l");
        assert_eq!(names[leg.effector], "ball_l");
        assert!(
            (leg.pole + Vec3::Y).length() < 1e-3,
            "a straight leg defaults to a forward knee"
        );
        let hind = find(LimbKind::Leg, LimbSide::Right, "leg2_thigh_r");
        assert_eq!(names[hind.effector], "leg2_hoof_r");
        assert!(
            hind.pole.y < -0.9,
            "the stifle bends forward: {}",
            hind.pole
        );
        let fore = find(LimbKind::Arm, LimbSide::Left, "upperarm_l");
        assert_eq!(names[fore.effector], "forehoof_l");
        assert!(fore.pole.y > 0.9, "the elbow bends back: {}", fore.pole);
        let bird = find(LimbKind::BirdLeg, LimbSide::Right, "calf_r");
        assert_eq!((names[bird.mid], names[bird.end]), ("foot_r", "ball_r"));
        assert!(bird.pole.y > 0.9, "the ankle bends back: {}", bird.pole);
        let wing = find(LimbKind::Wing, LimbSide::Right, "upperarm_r");
        assert_eq!(names[wing.effector], "wing_tip_r");
        let bat = find(LimbKind::Wing, LimbSide::Left, "arm2_upperarm_l");
        assert_eq!(names[bat.effector], "arm2_wing_digit_1_03_l");
        assert_eq!(limbs.len(), 6);
        // Planting aims the EFFECTOR: the hoof lands on the target, the hock above it.
        let mut posed = rest.clone();
        let target = Vec3::new(-9.0, 20.0, 0.0);
        let miss = plant(&mut posed, &parents, &hind, target);
        assert!(miss < 1e-2, "the hoof lands: miss {miss}");
        assert!(
            (posed[hind.effector].w_axis.truncate() - target).length() < 1e-2
                && posed[hind.root].w_axis.truncate() == rest[hind.root].w_axis.truncate()
        );
    }
}
