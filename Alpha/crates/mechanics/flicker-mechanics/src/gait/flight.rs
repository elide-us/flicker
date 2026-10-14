//! FLIGHT — G4 of the gait generator / IK solver design (37704D6B, Aaron's ruling CD36B9BE):
//! the FLAP for the modular skeleton's fliers — the birds, the bat, and the winged bodies that
//! carry a second shoulder pair (`arm2_…`: the gargoyles, the dragons). Pure glam math over the
//! skeleton's WORLD frames like the rest of [`crate::gait`]: no renderer, no signals, no pack.
//!
//! - [`FlapCycle`] is the beat's knobs: amplitude and frequency (hover and soar are settings of
//!   the two), the phase lag that runs down the chain, the feather groups' extra lag and sweep,
//!   the glide's dihedral, and how far a bat's digit fans fold on the upstroke.
//! - [`Wings`] finds every [`LimbKind::Wing`] chain the skeleton carries and poses them: the
//!   shoulder turns about the body's FORWARD axis (the normal of the spread wing's span, so the
//!   wing beats up and down) by `dihedral + A·sin(2πft)`, mirrored per side so both wings rise
//!   together; the elbow, wrist and tip follow by a fraction of the amplitude each, a lag later,
//!   so the wing bows with the stroke; the feather groups lag most and sweep back toward the tail
//!   through the upstroke, spreading again through the down; a bat's digits fold toward the wrist
//!   on the upstroke, the fan closing onto its trailing digit.
//! - [`Wings::glide`] is the spread rest raised by the dihedral (the soar); [`Wings::folded`]
//!   the perch — the Z-fold along the flank a landing eases into and a take-off out of, the leg
//!   IK taking the support meanwhile ([`super::Locomotion`]). A take-off runs the fold amount
//!   down and the beat's amplitude up; a landing the reverse.
//!
//! The wings are authored SPREAD (the bird's A-pose): the span runs out along ±X from the
//! shoulder, the body facing −Y with +Z up, in centimetres — and every pose here starts from
//! that rest, carried by whatever the body did to the joint the wing hangs from, so posing is a
//! function of the phase, never an accumulation.

use std::f32::consts::TAU;

use glam::{Mat4, Quat, Vec3};

use crate::ik::{limbs_of, subtree, turn_about, turn_subtree, LimbChain, LimbKind, LimbSide};

/// A crow's beat: about 3.6 a second.
pub const CROW_FREQUENCY_HZ: f32 = 3.6;
/// A crow's half-stroke at the shoulder, radians (±40°).
pub const CROW_AMPLITUDE_RAD: f32 = 0.7;
/// A bat's beat: quicker.
pub const BAT_FREQUENCY_HZ: f32 = 8.0;
/// A bat's half-stroke at the shoulder, radians (±46°).
pub const BAT_AMPLITUDE_RAD: f32 = 0.8;
/// Hovering beats this much deeper than the level flap …
pub const HOVER_AMPLITUDE: f32 = 1.4;
/// … and this much faster.
pub const HOVER_FREQUENCY: f32 = 1.6;
/// How far a bat's LEADING digit folds back toward the wrist at `fold` 1, radians, summed over
/// its hinges (~137°); the fan closes onto its trailing digit, which folds a quarter as far.
pub const BAT_FOLD_RAD: f32 = 2.4;
/// The share of a digit's fold taken at the wrist (the whole digit swinging); the knuckles
/// split the rest, curling the tip in.
pub const FOLD_AT_WRIST: f32 = 0.5;
/// The perch's Z-fold, radians, in the wing's plane: the humerus back along the flank …
pub const PERCH_SHOULDER_RAD: f32 = 1.48;
/// … the forearm folded forward at the elbow …
pub const PERCH_ELBOW_RAD: f32 = 2.62;
/// … the hand folded back at the wrist …
pub const PERCH_WRIST_RAD: f32 = 2.62;
/// … the feather groups swept back along the tail …
pub const PERCH_FEATHER_RAD: f32 = 0.8;
/// … and the folded wing drooped against the flank.
pub const PERCH_DROOP_RAD: f32 = 0.6;

/// The beat's knobs. Angles in radians, the beat in cycles per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlapCycle {
    /// The shoulder's half-stroke: the wing rises this far above its beat line and dips this far
    /// below it.
    pub amplitude_rad: f32,
    /// Beats per second.
    pub frequency_hz: f32,
    /// The phase each joint down the chain lags the one above it — the elbow one lag behind the
    /// shoulder, the wrist two, the tip three.
    pub lag_rad: f32,
    /// The fraction of the amplitude each joint past the shoulder adds: the bow of the wing.
    pub distal_scale: f32,
    /// The extra phase a feather group (a bat's digit fan) lags the joint it hangs from.
    pub feather_lag: f32,
    /// How far a feather group sweeps back toward the tail at the height of the upstroke.
    pub feather_sweep_rad: f32,
    /// The glide's dihedral: each wing raised this far above the spread rest — the line the beat
    /// runs about.
    pub dihedral_rad: f32,
    /// How far (0..1 of [`BAT_FOLD_RAD`]) a bat's digit fans fold toward the wrist at the height
    /// of the upstroke.
    pub fold: f32,
}

impl FlapCycle {
    /// A crow's flap: the level beat the birds default to.
    pub const fn crow() -> Self {
        Self {
            amplitude_rad: CROW_AMPLITUDE_RAD,
            frequency_hz: CROW_FREQUENCY_HZ,
            lag_rad: 0.4,
            distal_scale: 0.35,
            feather_lag: 0.5,
            feather_sweep_rad: 0.5,
            dihedral_rad: 0.15,
            fold: 0.0,
        }
    }

    /// A bat's flap: quicker and deeper, the digit fans folding on the upstroke.
    pub const fn bat() -> Self {
        Self {
            amplitude_rad: BAT_AMPLITUDE_RAD,
            frequency_hz: BAT_FREQUENCY_HZ,
            lag_rad: 0.5,
            distal_scale: 0.3,
            feather_lag: 0.4,
            feather_sweep_rad: 0.0,
            dihedral_rad: 0.1,
            fold: 0.6,
        }
    }

    /// The hover: the same beat, deeper and faster.
    pub fn hover(self) -> Self {
        Self {
            amplitude_rad: self.amplitude_rad * HOVER_AMPLITUDE,
            frequency_hz: self.frequency_hz * HOVER_FREQUENCY,
            ..self
        }
    }

    /// The soar: no beat at all — the wings hold the glide (the spread rest with the dihedral),
    /// the feathers spread, the fans open. Posed at any time, it is [`Wings::glide`].
    pub fn soar(self) -> Self {
        Self {
            amplitude_rad: 0.0,
            feather_sweep_rad: 0.0,
            fold: 0.0,
            ..self
        }
    }
}

impl Default for FlapCycle {
    fn default() -> Self {
        Self::crow()
    }
}

/// A feather group (a bird's `wing_feathers_NN`): a bone hung off a chain joint, its HINGE,
/// that it sweeps about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeatherGroup {
    pub bone: usize,
    /// The chain joint the group hangs from (its parent).
    pub hinge: usize,
    /// Which chain joint that is: 0 the shoulder, 1 the elbow, 2 the wrist, 3 the tip — the
    /// group lags that joint's lag plus [`FlapCycle::feather_lag`].
    pub hinge_k: usize,
}

/// One wing: its flap chain and what hangs off it.
#[derive(Clone, Debug, PartialEq)]
pub struct Wing {
    /// The chain: `root` the shoulder joint (the upperarm), `mid` the elbow, `end` the wrist,
    /// `effector` the tip (a bird's `wing_tip`, a bat's leading digit tip); `shoulder` the
    /// clavicle it hangs from, which the flap never turns.
    pub limb: LimbChain,
    /// A bird's feather groups (the secondaries off the forearm, the primaries off the hand).
    pub feathers: Vec<FeatherGroup>,
    /// A bat's membrane digits, the leading one first: each its joints from the wrist outward
    /// (`wing_digit_N_01`, `_02`, `_03`).
    pub digits: Vec<Vec<usize>>,
    /// The bone path from the shoulder joint to the tip, summed (cm) — the tip never lies
    /// further from the shoulder than this.
    pub reach: f32,
}

/// The body's axes under a wing's anchor this tick and the wing's side sign (+1 left, −1
/// right): the same turn about `forward` raises the left wing and lowers the right, so every
/// angle is mirrored by `sign`.
#[derive(Clone, Copy, Debug)]
struct WingFrame {
    forward: Vec3,
    up: Vec3,
    sign: f32,
}

impl Wing {
    /// +1 for the left wing, −1 for the right.
    pub fn sign(&self) -> f32 {
        match self.limb.side {
            LimbSide::Left => 1.0,
            LimbSide::Right => -1.0,
        }
    }

    /// The chain's joints, shoulder to tip (the tip only when it is its own bone).
    fn joints(&self) -> impl Iterator<Item = usize> {
        let l = self.limb;
        let n = if l.effector == l.end { 3 } else { 4 };
        [l.root, l.mid, l.end, l.effector].into_iter().take(n)
    }

    /// Put the wing back at its rest under whatever carries the joint it hangs from (the
    /// clavicle, else the rig's placement): the shoulder's subtree becomes `carry · rest`.
    /// Returns the body's axes as carried, or `None` when the chain is missing from the frames.
    fn reset(&self, globals: &mut [Mat4], parents: &[i32], rest: &[Mat4]) -> Option<WingFrame> {
        let root = self.limb.root;
        if root >= globals.len() || root >= rest.len() {
            return None;
        }
        let anchor = parents.get(root).and_then(|p| usize::try_from(*p).ok());
        let carry = match anchor {
            Some(a) => *globals.get(a)? * rest.get(a)?.inverse(),
            None => Mat4::IDENTITY,
        };
        for i in subtree(parents, root) {
            if let (Some(g), Some(r)) = (globals.get_mut(i), rest.get(i)) {
                *g = carry * *r;
            }
        }
        let axis = |v: Vec3| carry.transform_vector3(v).try_normalize().unwrap_or(v);
        Some(WingFrame {
            forward: axis(-Vec3::Y),
            up: axis(Vec3::Z),
            sign: self.sign(),
        })
    }

    /// Fold the digit fans toward the wrist by `amount` (0..1 of [`BAT_FOLD_RAD`]), in the
    /// plane of the wing as bent at the wrist (`bent`, the flap's angle there): the leading
    /// digit folds fully and the fan closes onto its trailing digit, which folds least; each
    /// digit swings [`FOLD_AT_WRIST`] of its fold at the wrist and curls the rest at its
    /// knuckles.
    fn fold_digits(
        &self,
        globals: &mut [Mat4],
        parents: &[i32],
        frame: &WingFrame,
        bent: f32,
        amount: f32,
    ) {
        if self.digits.is_empty() || amount == 0.0 {
            return;
        }
        let normal = Quat::from_axis_angle(frame.forward, frame.sign * bent) * frame.up;
        let fan = self.digits.len() as f32;
        for (d, digit) in self.digits.iter().enumerate() {
            let total = amount * BAT_FOLD_RAD * (1.0 - d as f32 / fan);
            let hinges = digit.len();
            for (i, &bone) in digit.iter().enumerate() {
                let share = if hinges == 1 {
                    1.0
                } else if i == 0 {
                    FOLD_AT_WRIST
                } else {
                    (1.0 - FOLD_AT_WRIST) / (hinges - 1) as f32
                };
                let turn = Quat::from_axis_angle(normal, frame.sign * total * share);
                if i == 0 {
                    // The whole digit swings at the wrist — its parent's position, not its own.
                    let Some(wrist) = parents
                        .get(bone)
                        .and_then(|p| usize::try_from(*p).ok())
                        .and_then(|p| globals.get(p))
                        .map(|m| m.w_axis.truncate())
                    else {
                        continue;
                    };
                    turn_about(globals, parents, bone, wrist, turn);
                } else {
                    // A knuckle: the joint above it turns, carrying the rest of the digit.
                    turn_subtree(globals, parents, digit[i - 1], turn);
                }
            }
        }
    }
}

impl FeatherGroup {
    /// Turn the group about its hinge by `angle`, in the plane of the wing as bent at that
    /// hinge (`bent`, the flap's angle there) — back toward the tail for a positive angle.
    fn sweep(
        &self,
        globals: &mut [Mat4],
        parents: &[i32],
        frame: &WingFrame,
        bent: f32,
        angle: f32,
    ) {
        let Some(pivot) = globals.get(self.hinge).map(|m| m.w_axis.truncate()) else {
            return;
        };
        let normal = Quat::from_axis_angle(frame.forward, frame.sign * bent) * frame.up;
        let turn = Quat::from_axis_angle(normal, frame.sign * angle);
        turn_about(globals, parents, self.bone, pivot, turn);
    }
}

/// The wings of a body: every flap chain read off the skeleton, with the beat's knobs and its
/// clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Wings {
    pub wings: Vec<Wing>,
    pub cycle: FlapCycle,
    /// The beat's phase, 0..1 (cycles), advanced by [`Wings::step`].
    pub phase: f32,
}

impl Wings {
    /// Read a skeleton's wings off its bone `names` and REST world frames (the packaged rest:
    /// the root at the origin facing −Y, the wings spread along ±X). Every
    /// [`LimbKind::Wing`] chain counts — both sides, and a second pair (`arm2_…`) behind a
    /// pair of arms; the feather groups and digit fans are found under each chain's prefix.
    pub fn new(names: &[&str], parents: &[i32], rest: &[Mat4], cycle: FlapCycle) -> Self {
        let find = |name: &str| names.iter().position(|n| *n == name);
        let at = |i: usize| rest.get(i).map(|m| m.w_axis.truncate());
        let wings = limbs_of(names, rest)
            .into_iter()
            .filter(|l| l.kind == LimbKind::Wing)
            .map(|limb| {
                let s = match limb.side {
                    LimbSide::Left => "l",
                    LimbSide::Right => "r",
                };
                let root_name = names.get(limb.root).copied().unwrap_or_default();
                let prefix = root_name
                    .strip_suffix(&format!("upperarm_{s}"))
                    .unwrap_or_default();
                let joints = [limb.root, limb.mid, limb.end, limb.effector];
                // The feather groups: this side's `wing_feathers_*` under the chain's prefix,
                // each hinged on the chain joint that parents it.
                let group = format!("{prefix}wing_feathers_");
                let suffix = format!("_{s}");
                let feathers = names
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.starts_with(&group) && n.ends_with(&suffix))
                    .filter_map(|(bone, _)| {
                        let hinge = usize::try_from(*parents.get(bone)?).ok()?;
                        let hinge_k = joints.iter().position(|&j| j == hinge)?;
                        Some(FeatherGroup {
                            bone,
                            hinge,
                            hinge_k,
                        })
                    })
                    .collect();
                // The digits: `wing_digit_{d}_{seg}` from the wrist outward, the leading first.
                let mut digits = Vec::new();
                for d in 1.. {
                    let mut chain = Vec::new();
                    for seg in 1.. {
                        match find(&format!("{prefix}wing_digit_{d}_{seg:02}_{s}")) {
                            Some(bone) => chain.push(bone),
                            None => break,
                        }
                    }
                    if chain.is_empty() {
                        break;
                    }
                    digits.push(chain);
                }
                // The reach: the bone path from the tip up to the shoulder joint, summed.
                let mut reach = 0.0;
                let mut walk = limb.effector;
                while walk != limb.root {
                    let Some(p) = parents.get(walk).and_then(|p| usize::try_from(*p).ok()) else {
                        break;
                    };
                    if let (Some(a), Some(b)) = (at(walk), at(p)) {
                        reach += (a - b).length();
                    }
                    walk = p;
                }
                Wing {
                    limb,
                    feathers,
                    digits,
                    reach,
                }
            })
            .collect();
        Self {
            wings,
            cycle,
            phase: 0.0,
        }
    }

    /// Advance the clock by `dt` seconds at the cycle's frequency and pose the wings there.
    pub fn step(&mut self, globals: &mut [Mat4], parents: &[i32], rest: &[Mat4], dt: f32) {
        self.phase = (self.phase + dt * self.cycle.frequency_hz).rem_euclid(1.0);
        self.pose_phase(globals, parents, rest, self.phase);
    }

    /// Pose the wings at time `t` (seconds) of a beat that started at 0.
    pub fn pose(&self, globals: &mut [Mat4], parents: &[i32], rest: &[Mat4], t: f32) {
        self.pose_phase(globals, parents, rest, t * self.cycle.frequency_hz);
    }

    /// Pose the wings at `phase` (cycles; the fraction is used) on `globals`, from `rest`:
    /// each wing is put back at its rest under what carries it, then the shoulder turns about
    /// the body's forward by `dihedral + A·sin φ`, the joints below by `distal_scale · A ·
    /// sin(φ − k·lag)` (the angles add into a bow, all about the same axis), the feather
    /// groups sweep back through their lagged upstroke, and a bat's fans fold through theirs.
    pub fn pose_phase(&self, globals: &mut [Mat4], parents: &[i32], rest: &[Mat4], phase: f32) {
        let phi = phase.rem_euclid(1.0) * TAU;
        let c = self.cycle;
        for wing in &self.wings {
            let Some(frame) = wing.reset(globals, parents, rest) else {
                continue;
            };
            let mut bent = [0.0f32; 4];
            let mut total = 0.0;
            for (k, joint) in wing.joints().enumerate() {
                let angle = if k == 0 {
                    c.dihedral_rad + c.amplitude_rad * phi.sin()
                } else {
                    c.distal_scale * c.amplitude_rad * (phi - k as f32 * c.lag_rad).sin()
                };
                total += angle;
                bent[k] = total;
                let turn = Quat::from_axis_angle(frame.forward, frame.sign * angle);
                turn_subtree(globals, parents, joint, turn);
            }
            for f in &wing.feathers {
                let lag = f.hinge_k as f32 * c.lag_rad + c.feather_lag;
                let sweep = c.feather_sweep_rad * upstroke(phi - lag);
                f.sweep(globals, parents, &frame, bent[f.hinge_k], sweep);
            }
            let lag = 2.0 * c.lag_rad + c.feather_lag;
            wing.fold_digits(
                globals,
                parents,
                &frame,
                bent[2],
                c.fold * upstroke(phi - lag),
            );
        }
    }

    /// The glide: the spread rest with each wing raised by the dihedral, nothing else moved.
    pub fn glide(&self, globals: &mut [Mat4], parents: &[i32], rest: &[Mat4]) {
        for wing in &self.wings {
            let Some(frame) = wing.reset(globals, parents, rest) else {
                continue;
            };
            let turn = Quat::from_axis_angle(frame.forward, frame.sign * self.cycle.dihedral_rad);
            turn_subtree(globals, parents, wing.limb.root, turn);
        }
    }

    /// The perch, by `amount` (0 the spread rest, 1 fully folded): the Z-fold in the wing's
    /// plane — the humerus back along the flank, the forearm forward, the hand back — the
    /// feather groups swept along the tail, a bat's fans closed, and the folded wing drooped
    /// against the flank. A landing eases `amount` up as the beat dies; a take-off the reverse.
    pub fn folded(&self, globals: &mut [Mat4], parents: &[i32], rest: &[Mat4], amount: f32) {
        let amount = amount.clamp(0.0, 1.0);
        for wing in &self.wings {
            let Some(frame) = wing.reset(globals, parents, rest) else {
                continue;
            };
            let l = wing.limb;
            for (joint, angle) in [
                (l.root, PERCH_SHOULDER_RAD),
                (l.mid, -PERCH_ELBOW_RAD),
                (l.end, PERCH_WRIST_RAD),
            ] {
                let turn = Quat::from_axis_angle(frame.up, frame.sign * angle * amount);
                turn_subtree(globals, parents, joint, turn);
            }
            for f in &wing.feathers {
                f.sweep(globals, parents, &frame, 0.0, PERCH_FEATHER_RAD * amount);
            }
            wing.fold_digits(globals, parents, &frame, 0.0, amount);
            let droop =
                Quat::from_axis_angle(frame.forward, -frame.sign * PERCH_DROOP_RAD * amount);
            turn_subtree(globals, parents, l.root, droop);
        }
    }
}

/// How far into its upstroke a joint is at `phi` (radians into the beat, the joint's own lag
/// already taken off): 1 at mid-upstroke — the wing rising fastest through its beat line — 0
/// through the whole downstroke, and smooth between (a smoothstep of the cosine).
pub fn upstroke(phi: f32) -> f32 {
    let u = phi.cos().clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;
    use crate::gait::BodyFrame;

    /// A synthetic crow-sized bird (40 cm tall) at rest: root, pelvis, spine and head on the
    /// midline, then both wings exactly as the baseline authors them — clavicle → upperarm →
    /// lowerarm → hand → wing_tip spread out along ±X, the secondaries hung back off the
    /// forearm and the primaries off the hand.
    fn bird() -> (Vec<String>, Vec<i32>, Vec<Mat4>) {
        let h = 40.0;
        let socket = Vec3::new(0.0, 0.0, 0.74 * h);
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut rest = Vec::new();
        let mut push = |n: String, p: i32, at: Vec3| -> i32 {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(at));
            rest.len() as i32 - 1
        };
        push("root".into(), -1, Vec3::ZERO);
        let pelvis = push("pelvis".into(), 0, Vec3::new(0.0, 0.0, 0.56 * h));
        let spine = push("spine_03".into(), pelvis, Vec3::new(0.0, 0.0, 0.73 * h));
        push("head".into(), spine, Vec3::new(0.0, 0.0, 0.87 * h));
        for (side, sign) in [("l", 1.0), ("r", -1.0)] {
            let at = |x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
            let n = |b: &str| format!("{b}_{side}");
            let clavicle = push(n("clavicle"), spine, at(0.03, 0.0, 0.02));
            let upperarm = push(n("upperarm"), clavicle, at(0.08, 0.0, 0.03));
            let lowerarm = push(n("lowerarm"), upperarm, at(0.30, 0.02, 0.06));
            let hand = push(n("hand"), lowerarm, at(0.52, 0.03, 0.07));
            push(n("wing_tip"), hand, at(0.80, 0.05, 0.06));
            push(n("wing_feathers_01"), lowerarm, at(0.36, 0.16, 0.02));
            push(n("wing_feathers_02"), hand, at(0.62, 0.15, 0.02));
        }
        (names, parents, rest)
    }

    /// A synthetic bat (20 cm hanging height) at rest, its wings as the baseline authors them:
    /// the arm chain to the wrist, a clawed thumb forward, and four three-segment digits
    /// fanning from the wrist — the first along the span to the tip, the fourth trailing back
    /// along the body.
    fn bat() -> (Vec<String>, Vec<i32>, Vec<Mat4>) {
        let h = 20.0;
        let socket = Vec3::new(0.0, 0.0, 0.74 * h);
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut rest = Vec::new();
        let mut push = |n: String, p: i32, at: Vec3| -> i32 {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(at));
            rest.len() as i32 - 1
        };
        push("root".into(), -1, Vec3::ZERO);
        let pelvis = push("pelvis".into(), 0, Vec3::new(0.0, 0.0, 0.56 * h));
        let spine = push("spine_03".into(), pelvis, Vec3::new(0.0, 0.0, 0.73 * h));
        push("head".into(), spine, Vec3::new(0.0, 0.0, 0.87 * h));
        for (side, sign) in [("l", 1.0), ("r", -1.0)] {
            let at = |x: f32, y: f32, z: f32| socket + Vec3::new(x * h * sign, y * h, z * h);
            let n = |b: &str| format!("{b}_{side}");
            let clavicle = push(n("clavicle"), spine, at(0.03, 0.0, 0.02));
            let upperarm = push(n("upperarm"), clavicle, at(0.10, 0.0, 0.02));
            let lowerarm = push(n("lowerarm"), upperarm, at(0.30, 0.02, 0.04));
            let hand = push(n("hand"), lowerarm, at(0.45, 0.03, 0.05));
            let wrist = at(0.45, 0.03, 0.05);
            push(n("wing_thumb"), hand, at(0.48, -0.03, 0.06));
            let tips = [
                at(0.85, 0.02, 0.04),
                at(0.80, 0.12, 0.02),
                at(0.70, 0.22, 0.0),
                at(0.55, 0.30, -0.02),
            ];
            for (d, tip) in tips.into_iter().enumerate() {
                let mut parent = hand;
                for seg in 1..=3 {
                    let t = seg as f32 / 3.0;
                    let name = n(&format!("wing_digit_{}_{seg:02}", d + 1));
                    parent = push(name, parent, wrist + (tip - wrist) * t);
                }
            }
        }
        (names, parents, rest)
    }

    fn strs(names: &[String]) -> Vec<&str> {
        names.iter().map(String::as_str).collect()
    }

    fn at(g: &[Mat4], i: usize) -> Vec3 {
        g[i].w_axis.truncate()
    }

    /// The two tips mirror across the midline: opposite x, the same y and z.
    fn assert_mirrored(globals: &[Mat4], wings: &Wings, what: &str) {
        let (l, r) = (&wings.wings[0], &wings.wings[1]);
        assert_eq!(
            (l.limb.side, r.limb.side),
            (LimbSide::Left, LimbSide::Right)
        );
        let (tl, tr) = (at(globals, l.limb.effector), at(globals, r.limb.effector));
        assert!(
            (tl.x + tr.x).abs() < 1e-3 && (tl.y - tr.y).abs() < 1e-3 && (tl.z - tr.z).abs() < 1e-3,
            "{what}: the tips mirror: {tl} vs {tr}"
        );
    }

    /// Every bone under the shoulder keeps its length to its parent, and the tip lies no
    /// further from the shoulder than the wing reaches.
    fn assert_rigid(names: &[&str], wing: &Wing, globals: &[Mat4], rest: &[Mat4], parents: &[i32]) {
        for i in subtree(parents, wing.limb.root) {
            if let Ok(p) = usize::try_from(parents[i]) {
                let now = (at(globals, i) - at(globals, p)).length();
                let was = (at(rest, i) - at(rest, p)).length();
                assert!(
                    (now - was).abs() < 1e-3,
                    "{} keeps its length: {now} vs {was}",
                    names[i]
                );
            }
        }
        let span = (at(globals, wing.limb.effector) - at(globals, wing.limb.root)).length();
        assert!(
            span <= wing.reach + 1e-3,
            "the tip within the wing's reach: {span} > {}",
            wing.reach
        );
    }

    /// Nothing outside the wings' chains moves — not the clavicles, not the trunk.
    fn assert_body_still(names: &[&str], globals: &[Mat4], rest: &[Mat4]) {
        for (i, name) in names.iter().enumerate() {
            let winged = ["upperarm", "lowerarm", "hand", "wing"]
                .iter()
                .any(|k| name.contains(k));
            if !winged {
                assert!(
                    globals[i].abs_diff_eq(rest[i], 1e-5),
                    "{name} stays where the body put it"
                );
            }
        }
    }

    /// Over a beat (dihedral off) both tips rise and fall together — mirrored across the
    /// midline at every sample, above and below the rest by at least half the span's sine of
    /// the amplitude, half a cycle apart to within a few percent of the span — every bone keeps
    /// its length, the tip never leaves the wing's reach, and nothing but the wings moves; the
    /// wings come with two feather groups each and a reach that is the chain's length.
    #[test]
    fn both_tips_rise_and_fall_together_within_the_wings_reach() {
        let (names, parents, rest) = bird();
        let names = strs(&names);
        let cycle = FlapCycle {
            dihedral_rad: 0.0,
            ..FlapCycle::crow()
        };
        let wings = Wings::new(&names, &parents, &rest, cycle);
        assert_eq!(wings.wings.len(), 2, "both wings: {:?}", wings.wings);
        for w in &wings.wings {
            assert_eq!(w.feathers.len(), 2, "two feather groups");
            assert!(w.digits.is_empty(), "no digits on a bird");
            assert_eq!(
                names[w.limb.effector],
                format!("wing_tip_{}", if w.sign() > 0.0 { "l" } else { "r" })
            );
            let span = (at(&rest, w.limb.effector) - at(&rest, w.limb.root)).length();
            assert!(
                w.reach >= span && w.reach < span * 1.05,
                "the reach is the chain's length: {} for a span of {span}",
                w.reach
            );
        }
        let left = &wings.wings[0];
        let tip_rest = at(&rest, left.limb.effector);
        let span = (tip_rest - at(&rest, left.limb.root)).length();
        let n = 128;
        let mut zs = Vec::with_capacity(n);
        let mut globals = rest.clone();
        for k in 0..n {
            wings.pose_phase(&mut globals, &parents, &rest, k as f32 / n as f32);
            assert_mirrored(&globals, &wings, &format!("sample {k}"));
            for w in &wings.wings {
                assert_rigid(&names, w, &globals, &rest, &parents);
            }
            assert_body_still(&names, &globals, &rest);
            zs.push(at(&globals, left.limb.effector).z);
        }
        let (max, min) = zs
            .iter()
            .fold((f32::MIN, f32::MAX), |(a, b), &z| (a.max(z), b.min(z)));
        let lift = 0.5 * span * cycle.amplitude_rad.sin();
        assert!(
            max - tip_rest.z > lift && tip_rest.z - min > lift,
            "the tip rises {} and falls {} (at least {lift})",
            max - tip_rest.z,
            tip_rest.z - min
        );
        // A clean beat about the rest: half a cycle on, the tip is as far below as it was above
        // (to a few percent of the span — the rest's slight upward tilt bows the two strokes a
        // hair differently).
        for k in 0..n / 2 {
            let (up, down) = (zs[k] - tip_rest.z, zs[k + n / 2] - tip_rest.z);
            assert!(
                (up + down).abs() < 0.05 * span,
                "symmetric strokes at sample {k}: {up} vs {down}"
            );
        }
    }

    /// The lag runs down the chain: the humerus peaks a quarter cycle in, the forearm after
    /// it, the hand after that. The feather groups sweep back only through the upstroke —
    /// spread to the rest through the whole downstroke, swept most at the lagged mid-upstroke
    /// (by better than 0.3 rad of their 0.5) — their window centred one hinge lag plus the
    /// feather lag after the shoulder's mid-upstroke, so the primaries (off the wrist) lag most.
    #[test]
    fn the_stroke_lags_down_the_chain_and_the_feathers_sweep_back_on_the_upstroke() {
        let (names, parents, rest) = bird();
        let names = strs(&names);
        let cycle = FlapCycle {
            dihedral_rad: 0.0,
            ..FlapCycle::crow()
        };
        let wings = Wings::new(&names, &parents, &rest, cycle);
        let wing = &wings.wings[0];
        let l = wing.limb;
        // A segment's elevation out of the horizontal, and a feather group's sweep off the
        // segment it hangs from (the flap turns both together, so only the sweep changes it).
        let elevation = |g: &[Mat4], a: usize, b: usize| {
            let v = at(g, b) - at(g, a);
            v.z.atan2(v.x.abs())
        };
        let sweep_of = |g: &[Mat4], f: &FeatherGroup, child: usize| {
            (at(g, f.bone) - at(g, f.hinge)).angle_between(at(g, child) - at(g, f.hinge))
        };
        let group = |name: &str| {
            *wing
                .feathers
                .iter()
                .find(|f| names[f.bone] == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        let (secondaries, primaries) = (group("wing_feathers_01_l"), group("wing_feathers_02_l"));
        assert_eq!((secondaries.hinge_k, primaries.hinge_k), (1, 2));
        let n = 720;
        let (mut humerus, mut forearm, mut hand) = (Vec::new(), Vec::new(), Vec::new());
        let (mut sec, mut pri) = (Vec::new(), Vec::new());
        let mut globals = rest.clone();
        for k in 0..n {
            wings.pose_phase(&mut globals, &parents, &rest, k as f32 / n as f32);
            humerus.push(elevation(&globals, l.root, l.mid));
            forearm.push(elevation(&globals, l.mid, l.end));
            hand.push(elevation(&globals, l.end, l.effector));
            sec.push(sweep_of(&globals, &secondaries, l.end));
            pri.push(sweep_of(&globals, &primaries, l.effector));
        }
        let argmax = |v: &[f32]| {
            v.iter()
                .enumerate()
                .fold(
                    (0, f32::MIN),
                    |best, (i, &x)| if x > best.1 { (i, x) } else { best },
                )
                .0
        };
        let (h, f, t) = (argmax(&humerus), argmax(&forearm), argmax(&hand));
        assert!(
            (h as i32 - (n / 4) as i32).abs() <= 1,
            "the shoulder peaks a quarter cycle in: sample {h} of {n}"
        );
        assert!(
            h < f && f < t,
            "the elbow lags the shoulder and the wrist the elbow: peaks at {h}, {f}, {t} of {n}"
        );
        for (label, series, group, child, hinge_lag) in [
            ("secondaries", &sec, &secondaries, l.end, cycle.lag_rad),
            (
                "primaries",
                &pri,
                &primaries,
                l.effector,
                2.0 * cycle.lag_rad,
            ),
        ] {
            let base = sweep_of(&rest, group, child);
            let lag = hinge_lag + cycle.feather_lag;
            let swept: Vec<usize> = (0..n).filter(|&i| series[i] > base + 1e-3).collect();
            assert!(
                swept.len() > n * 47 / 100 && swept.len() <= n / 2,
                "{label}: swept through the upstroke half: {} of {n}",
                swept.len()
            );
            // The window's centre (circular, it wraps the cycle's start) is the lagged mid-upstroke.
            let (c, s) = swept.iter().fold((0.0f32, 0.0f32), |(c, s), &i| {
                let a = i as f32 / n as f32 * TAU;
                (c + a.cos(), s + a.sin())
            });
            let centre = s.atan2(c).rem_euclid(TAU);
            assert!(
                (centre - lag).abs() < 2.5 * TAU / n as f32,
                "{label}: the sweep is centred {lag} rad after mid-upstroke: {centre}"
            );
            let sample =
                |phi: f32| series[((phi.rem_euclid(TAU) / TAU) * n as f32).round() as usize % n];
            assert!(
                (sample(lag + PI) - base).abs() < 1e-4,
                "{label}: spread through the downstroke"
            );
            assert!(
                sample(lag) > base + 0.3,
                "{label}: swept back at the lagged mid-upstroke: {} over {base}",
                sample(lag)
            );
        }
    }

    /// The glide is the spread rest with each wing raised by the dihedral: the shoulder stays,
    /// every length holds, the tip's elevation rises by exactly the dihedral with no sweep, the
    /// tips mirror, the body never moves; with no dihedral it IS the rest; a soar posed at any
    /// time is the glide; a hover beats deeper and faster.
    #[test]
    fn glide_is_the_spread_rest_raised_by_the_dihedral_and_a_soar_is_the_glide() {
        let (names, parents, rest) = bird();
        let names = strs(&names);
        let cycle = FlapCycle::crow();
        assert!(cycle.dihedral_rad > 0.0);
        let wings = Wings::new(&names, &parents, &rest, cycle);
        let mut globals = rest.clone();
        wings.glide(&mut globals, &parents, &rest);
        let elevation = |t: Vec3, s: Vec3| {
            let v = t - s;
            v.z.atan2(v.x.abs())
        };
        for w in &wings.wings {
            assert_rigid(&names, w, &globals, &rest, &parents);
            let (s, t) = (at(&rest, w.limb.root), at(&rest, w.limb.effector));
            let (s2, t2) = (at(&globals, w.limb.root), at(&globals, w.limb.effector));
            assert!((s2 - s).length() < 1e-5, "the shoulder stays");
            assert!(
                (elevation(t2, s2) - elevation(t, s) - cycle.dihedral_rad).abs() < 1e-4,
                "raised by the dihedral: {} from {}",
                elevation(t2, s2),
                elevation(t, s)
            );
            assert!((t2.y - t.y).abs() < 1e-5, "no sweep in a glide");
        }
        assert_mirrored(&globals, &wings, "the glide");
        assert_body_still(&names, &globals, &rest);
        let flat = Wings::new(
            &names,
            &parents,
            &rest,
            FlapCycle {
                dihedral_rad: 0.0,
                ..cycle
            },
        );
        let mut g = rest.clone();
        flat.glide(&mut g, &parents, &rest);
        assert!(
            g.iter().zip(&rest).all(|(a, b)| a.abs_diff_eq(*b, 1e-5)),
            "without a dihedral the glide is the rest"
        );
        let soar = Wings::new(&names, &parents, &rest, cycle.soar());
        let mut posed = rest.clone();
        soar.pose(&mut posed, &parents, &rest, 0.37);
        assert!(
            posed
                .iter()
                .zip(&globals)
                .all(|(a, b)| a.abs_diff_eq(*b, 1e-4)),
            "a soar posed at any time is the glide"
        );
        let hover = cycle.hover();
        assert!(
            hover.amplitude_rad > cycle.amplitude_rad && hover.frequency_hz > cycle.frequency_hz
        );
        assert_eq!(FlapCycle::default(), FlapCycle::crow());
    }

    /// A bat's fans: four three-joint digits a wing, the leading one's tip the effector. At the
    /// lagged mid-upstroke the leading digit's tip draws toward the wrist as the fold knob
    /// rises — strictly, from its rest reach at 0 to better than eight percent in at 1 — the
    /// leading digit swinging furthest and the trailing one least (the fan closes onto it),
    /// both sides alike; through the downstroke the fan is spread whatever the knob says; and a
    /// whole beat keeps every length and the tip within reach.
    #[test]
    fn a_bats_digits_fold_toward_the_wrist_on_the_upstroke() {
        let (names, parents, rest) = bat();
        let names = strs(&names);
        let base = FlapCycle {
            dihedral_rad: 0.0,
            ..FlapCycle::bat()
        };
        let wings = Wings::new(&names, &parents, &rest, base);
        assert_eq!(wings.wings.len(), 2);
        for w in &wings.wings {
            assert_eq!(w.digits.len(), 4, "four digits");
            assert!(w.digits.iter().all(|d| d.len() == 3), "three joints each");
            assert!(w.feathers.is_empty(), "no feathers on a bat");
            assert!(names[w.limb.effector].starts_with("wing_digit_1_03_"));
        }
        let wing = &wings.wings[0];
        let (hand, tip) = (wing.limb.end, wing.limb.effector);
        let thumb = names.iter().position(|n| *n == "wing_thumb_l").unwrap();
        let reach_of = |g: &[Mat4]| (at(g, tip) - at(g, hand)).length();
        let rest_reach = reach_of(&rest);
        let lag = 2.0 * base.lag_rad + base.feather_lag;
        let (up_phase, down_phase) = (lag / TAU, (lag + PI) / TAU);
        // A digit's swing at the wrist, measured against the thumb (which flaps with the hand
        // but never folds), over its rest.
        let swing = |g: &[Mat4], d: usize| {
            let angle = |g: &[Mat4]| {
                (at(g, wing.digits[d][0]) - at(g, hand)).angle_between(at(g, thumb) - at(g, hand))
            };
            angle(g) - angle(&rest)
        };
        let mut last = f32::INFINITY;
        for fold in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let wings = Wings::new(&names, &parents, &rest, FlapCycle { fold, ..base });
            let mut g = rest.clone();
            wings.pose_phase(&mut g, &parents, &rest, up_phase);
            let d = reach_of(&g);
            if fold == 0.0 {
                assert!(
                    (d - rest_reach).abs() < 1e-3,
                    "unfolded, the digit lies straight"
                );
            } else {
                assert!(
                    d < last - 1e-3,
                    "fold {fold}: the tip draws toward the wrist: {d} after {last}"
                );
                let s: Vec<f32> = (0..4).map(|k| swing(&g, k)).collect();
                assert!(
                    s[0] > s[1] && s[1] > s[2] && s[2] > s[3] && s[3] > 0.0,
                    "fold {fold}: the fan closes onto its trailing digit: {s:?}"
                );
            }
            last = d;
            for w in &wings.wings {
                assert_rigid(&names, w, &g, &rest, &parents);
            }
            assert_mirrored(&g, &wings, &format!("fold {fold}"));
            assert_body_still(&names, &g, &rest);
            wings.pose_phase(&mut g, &parents, &rest, down_phase);
            assert!(
                (reach_of(&g) - rest_reach).abs() < 1e-3 && swing(&g, 0).abs() < 1e-4,
                "fold {fold}: spread through the downstroke"
            );
        }
        assert!(
            last < rest_reach * 0.92,
            "at a full fold the tip draws in by better than eight percent: {last} of {rest_reach}"
        );
        let mut g = rest.clone();
        for k in 0..96 {
            wings.pose_phase(&mut g, &parents, &rest, k as f32 / 96.0);
            for w in &wings.wings {
                assert_rigid(&names, w, &g, &rest, &parents);
            }
        }
    }

    /// The perch: at 0 the spread rest; at 1 the wing lies folded along the flank — the tip's
    /// reach out from the shoulder under half its spread, behind the shoulder, dropped below its
    /// rest — every length kept, the tips mirrored, the body still; a bat's folded wing keeps
    /// its lengths with the leading digit drawn in to the wrist.
    #[test]
    fn the_perch_folds_the_wing_along_the_flank() {
        let (names, parents, rest) = bird();
        let names = strs(&names);
        let wings = Wings::new(&names, &parents, &rest, FlapCycle::crow());
        let mut g = rest.clone();
        wings.folded(&mut g, &parents, &rest, 0.0);
        assert!(
            g.iter().zip(&rest).all(|(a, b)| a.abs_diff_eq(*b, 1e-5)),
            "unfolded is the rest"
        );
        wings.folded(&mut g, &parents, &rest, 1.0);
        for w in &wings.wings {
            assert_rigid(&names, w, &g, &rest, &parents);
            let (s, t) = (at(&rest, w.limb.root), at(&rest, w.limb.effector));
            let t2 = at(&g, w.limb.effector);
            let (spread, folded) = ((t.x - s.x).abs(), (t2.x - s.x).abs());
            assert!(
                folded < 0.5 * spread,
                "the tip draws in to the flank: {folded} of {spread}"
            );
            assert!(
                t2.y > s.y + 0.2 * spread,
                "the tip lies back along the flank: {t2}"
            );
            assert!(t2.z < t.z, "the folded wing droops: {} under {}", t2.z, t.z);
        }
        assert_mirrored(&g, &wings, "the perch");
        assert_body_still(&names, &g, &rest);
        let (names, parents, rest) = bat();
        let names = strs(&names);
        let wings = Wings::new(&names, &parents, &rest, FlapCycle::bat());
        let mut g = rest.clone();
        wings.folded(&mut g, &parents, &rest, 1.0);
        for w in &wings.wings {
            assert_rigid(&names, w, &g, &rest, &parents);
            let (hand, tip) = (w.limb.end, w.limb.effector);
            let (now, was) = (
                (at(&g, tip) - at(&g, hand)).length(),
                (at(&rest, tip) - at(&rest, hand)).length(),
            );
            assert!(
                now < was * 0.92,
                "the leading digit folds in: {now} of {was}"
            );
        }
        assert_mirrored(&g, &wings, "the bat's perch");
    }

    /// The flap rides whatever frame the body was placed in: with the rig carried to a heading
    /// and a height, the shoulders stay where the carry put them, the tips mirror across the
    /// body's own midplane and rise along the world's up; posing is a function of the phase,
    /// not an accumulation (re-posing already-posed frames matches a fresh pose); and the clock
    /// steps by the frequency.
    #[test]
    fn the_flap_rides_the_frame_the_body_is_placed_in() {
        let (names, parents, rest) = bird();
        let names = strs(&names);
        let cycle = FlapCycle {
            dihedral_rad: 0.0,
            ..FlapCycle::crow()
        };
        let mut wings = Wings::new(&names, &parents, &rest, cycle);
        let frame = BodyFrame::level(Vec3::new(100.0, 50.0, 30.0), Vec3::X);
        let carry = frame.matrix();
        let placed: Vec<Mat4> = rest.iter().map(|r| carry * *r).collect();
        let mut g = placed.clone();
        // A quarter cycle in: the top of the stroke.
        wings.pose(&mut g, &parents, &rest, 0.25 / cycle.frequency_hz);
        let (l, r) = (&wings.wings[0], &wings.wings[1]);
        for w in [l, r] {
            assert!(
                (at(&g, w.limb.root) - at(&placed, w.limb.root)).length() < 1e-4,
                "the shoulder stays where the carry put it"
            );
            assert_rigid(&names, w, &g, &placed, &parents);
            assert!(
                at(&g, w.limb.effector).z > at(&placed, w.limb.effector).z + 5.0,
                "the tip rises along the world's up"
            );
        }
        let across = at(&g, l.limb.effector) - at(&g, r.limb.effector);
        assert!(
            across.dot(frame.forward).abs() < 1e-3 && across.dot(frame.up).abs() < 1e-3,
            "the tips mirror across the body's midplane: {across}"
        );
        assert!(
            across.dot(-frame.right) > 0.0,
            "the left tip lies to the body's left"
        );
        let mut fresh = placed.clone();
        wings.pose(&mut fresh, &parents, &rest, 0.2);
        wings.pose(&mut g, &parents, &rest, 0.2);
        assert!(
            g.iter().zip(&fresh).all(|(a, b)| a.abs_diff_eq(*b, 1e-3)),
            "posing is a function of the phase"
        );
        let mut stepped = placed.clone();
        for _ in 0..10 {
            wings.step(&mut stepped, &parents, &rest, 1.0 / 60.0);
        }
        let want = (10.0 / 60.0 * cycle.frequency_hz).rem_euclid(1.0);
        assert!(
            (wings.phase - want).abs() < 1e-5,
            "the clock: {} for {want}",
            wings.phase
        );
        let mut direct = placed.clone();
        wings.pose_phase(&mut direct, &parents, &rest, wings.phase);
        assert!(stepped
            .iter()
            .zip(&direct)
            .all(|(a, b)| a.abs_diff_eq(*b, 1e-3)));
    }
}
