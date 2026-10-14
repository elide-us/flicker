//! THE DRIVER: a skeleton's limbs, their planners, the phase clock and the body solver, stepped
//! once per tick into the skeleton's WORLD frames. Owns no renderer, no signals, no pack: the
//! caller (the runtime's pose step, the tester, a bench preview) hands it a heading, a speed and
//! a surface and takes back posed globals.

use glam::{Mat4, Quat, Vec3};

use super::body::{BodyFrame, BodySolver};
use super::pattern::{pattern, select, GaitKind, GaitPattern, LocomotionFamily};
use super::planner::{FootPlanner, REACH_MARGIN};
use super::surface::SurfaceQuery;
use crate::ik::{ccd, limbs_of, plant, subtree, turn_about, turn_subtree, LimbKind, LimbSide};

/// A planted foot counts as reaching its target within this (cm) when the reach is measured.
pub const REACH_TOLERANCE: f32 = 0.05;
/// Radians of trunk bend per spine joint per leg length of `sway` — the sprawler's lateral
/// undulation travels down the spine a third of a cycle behind each joint before it.
pub const SPINE_UNDULATION: f32 = 1.0;
/// The undulation's phase lag per spine joint (radians of the cycle).
pub const SPINE_LAG: f32 = std::f32::consts::FRAC_PI_3;

/// Radians a girdle YAWS per leg length its two feet stand apart fore-and-aft: the hip of the
/// foot that reaches ahead goes with it (a walking horse's pelvis swings some 5–8° each way,
/// its feet half a leg apart).
pub const GIRDLE_YAW: f32 = 0.25;
/// Radians a girdle ROLLS per leg length one of its feet is lifted over the other: the hip over
/// a swinging leg drops.
pub const GIRDLE_ROLL: f32 = 0.5;
/// How much of a stiff leg's arc a girdle SINKS by: a leg of fixed length carries its hip
/// highest as it passes upright and lowest at the two ends of its stance; a real leg flexes
/// through half of that.
pub const GIRDLE_SINK: f32 = 0.5;
/// No girdle turns further than this (radians), whatever its feet do.
pub const GIRDLE_MAX: f32 = 0.2;
/// The summed bearing of a girdle's feet at which they carry it whole — below it (a pair
/// leaving the ground together, the instant a trot changes diagonals) the sink fades out with
/// them instead of dropping away.
pub const BEARING_FULL: f32 = 0.5;

/// THE CURL: the proboscis's idle pitch at each of its joints, at its peak (radians) — a wave
/// that, lagged down an eight-bone chain ([`CURL_LAG`]), curls the tip up through most of a
/// half turn and lets it hang again.
pub const CURL_AMPLITUDE: f32 = 0.2;
/// The curl's lag from one proboscis joint to the next (radians of its cycle): the curl travels
/// down the chain, root to tip.
pub const CURL_LAG: f32 = 0.6;
/// One idle curl (seconds).
pub const CURL_PERIOD_S: f32 = 4.0;
/// The idle REACH: once in this many seconds the proboscis's tip is sent to the ground ahead of
/// the body and brought back ([`REACH_WINDOW`]).
pub const REACH_PERIOD_S: f32 = 9.0;
/// The share of [`REACH_PERIOD_S`] the tip is away on its reach — a smooth bump, out and back.
pub const REACH_WINDOW: f32 = 0.45;

/// THE PROBOSCIS (ruling 7881216F, Aaron: *"trunk should curl and reach"*) — the one appendage
/// that is DRIVEN rather than hung: the recipe's chain off the head (`proboscis_01..`), read off
/// the canon names root first ([`proboscis_of`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Proboscis {
    /// Its joints, root first.
    pub chain: Vec<usize>,
    /// Root joint to tip joint at rest (cm): what its reach is measured against.
    pub length: f32,
}

/// The first proboscis a skeleton carries, by its canon names; `None` without one.
pub fn proboscis_of(names: &[&str], rest: &[Mat4]) -> Option<Proboscis> {
    let chain: Vec<usize> = (1..)
        .map(|k| format!("proboscis_{k:02}"))
        .map_while(|name| names.iter().position(|n| *n == name))
        .collect();
    if chain.is_empty() {
        return None;
    }
    let at = |i: usize| rest.get(i).map_or(Vec3::ZERO, |m| m.w_axis.truncate());
    let length = chain.windows(2).map(|w| at(w[0]).distance(at(w[1]))).sum();
    Some(Proboscis { chain, length })
}

/// A GIRDLE — the trunk joint a left/right pair of planted limbs hangs from (a pelvis over two
/// hind legs, the withers over two forelegs): the pair's nearest common ancestor. The trunk is
/// not one rigid frame over four moving legs — each girdle RIDES its own two feet
/// ([`Locomotion::ride_girdles`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Girdle {
    pub bone: usize,
    /// Its two feet, left and right, as indices into [`Locomotion::feet`].
    pub feet: [usize; 2],
    /// The children of `bone` that lead on to ANOTHER girdle or to the head — what a girdle
    /// does not carry when it turns: the spine between the girdles takes up their difference,
    /// and the head is held steady over the shoulders.
    pub relief: Vec<usize>,
    /// How far apart its two limb roots stand at rest (cm).
    pub width: f32,
}

/// Every girdle of a skeleton: each left foot with its right twin (the same chain top under
/// the other suffix) and the nearest joint both hang from. Rear-most first (the canon faces −Y).
fn girdles_of(names: &[&str], parents: &[i32], rest: &[Mat4], feet: &[FootPlanner]) -> Vec<Girdle> {
    let parent = |i: usize| parents.get(i).and_then(|&p| usize::try_from(p).ok());
    let line = |i: usize| std::iter::successors(Some(i), |&b| parent(b)).collect::<Vec<_>>();
    let top = |f: &FootPlanner| f.limb.shoulder.unwrap_or(f.limb.root);
    let at = |i: usize| rest[i].w_axis.truncate();
    let mut out: Vec<Girdle> = Vec::new();
    for (l, left) in feet.iter().enumerate() {
        let Some(twin) = names[top(left)]
            .strip_suffix("_l")
            .map(|b| format!("{b}_r"))
        else {
            continue;
        };
        let Some((r, right)) = feet.iter().enumerate().find(|(_, f)| names[top(f)] == twin) else {
            continue;
        };
        let above = line(top(left));
        let Some(bone) = line(top(right))
            .into_iter()
            .skip(1)
            .find(|b| above.contains(b))
        else {
            continue;
        };
        out.push(Girdle {
            bone,
            feet: [l, r],
            relief: Vec::new(),
            width: at(left.limb.root).distance(at(right.limb.root)),
        });
    }
    let head = names.iter().position(|n| *n == "head");
    let bones: Vec<usize> = out.iter().map(|g| g.bone).collect();
    for g in &mut out {
        g.relief = (0..parents.len())
            .filter(|&c| parent(c) == Some(g.bone))
            .filter(|&c| {
                let under = subtree(parents, c);
                bones.iter().any(|b| *b != g.bone && under.contains(b))
                    || head.is_some_and(|h| under.contains(&h))
            })
            .collect();
    }
    out.sort_by(|a, b| at(b.bone).y.total_cmp(&at(a.bone).y));
    out
}

/// A body walking under the generator.
#[derive(Clone, Debug)]
pub struct Locomotion {
    pub family: LocomotionFamily,
    pub feet: Vec<FootPlanner>,
    /// The trunk's spine joints, pelvis end first (`spine_01`, `spine_02`, `spine_03`), for
    /// the sprawler's undulation.
    pub spine: Vec<usize>,
    /// The girdles the trunk rides, rear-most first.
    pub girdles: Vec<Girdle>,
    /// The driven chain off the head, if the body has one.
    pub proboscis: Option<Proboscis>,
    /// Where the proboscis is told to reach this tick (world); `None` leaves it to its idle —
    /// the curl, and a reach to the ground ahead now and then ([`Locomotion::drive_proboscis`]).
    pub reach: Option<Vec3>,
    /// The body's own clock (seconds since it was made): what the idle motions run on.
    pub clock: f32,
    pub body: BodySolver,
    pub gait: GaitKind,
    pub pattern: GaitPattern,
    /// The cycle phase, 0..1.
    pub phase: f32,
    /// The leg length every distance scales with (the planted limbs' mean hip height).
    pub leg_len: f32,
    /// Where the controller says the body is and faces.
    pub position: Vec3,
    pub heading: Vec3,
    pub frame: BodyFrame,
    /// The worst foot's distance from its target after planting this tick (cm) — a reach
    /// diagnostic, not a correction.
    pub residual: f32,
}

impl Locomotion {
    /// Read a skeleton's limbs off its bone `names` and REST world frames (the root at the
    /// origin on the ground, facing −Y). Wings are not planted; a bird's legs are.
    pub fn new(names: &[&str], parents: &[i32], rest: &[Mat4], family: LocomotionFamily) -> Self {
        let limbs = limbs_of(names, rest);
        let mut feet = Vec::new();
        for limb in limbs {
            if limb.kind == LimbKind::Wing {
                continue;
            }
            let root = rest[limb.root].w_axis.truncate();
            let effector = rest[limb.effector].w_axis.truncate();
            let leg_len = (root.z - effector.z).abs().max(1.0);
            let slot = match (limb.kind, limb.side) {
                (LimbKind::Arm, LimbSide::Left) => 0,
                (LimbKind::Arm, LimbSide::Right) => 1,
                (_, LimbSide::Left) => 2,
                (_, LimbSide::Right) => 3,
            };
            feet.push(FootPlanner::new(limb, effector, leg_len, slot));
        }
        let girdles = girdles_of(names, parents, rest, &feet);
        // A PAIR STANDS IN THE MIDDLE OF ITS TWO FEET. A rest pose taken from a body caught
        // mid-stride has one foot of a pair planted well ahead of its hip and the other well
        // behind, a leg at full stretch — and a foot that cycles round its OWN rest footprint
        // has nowhere to go from there (measured on the 17 hoofed sources: every hind pair
        // stood 20–77 cm apart fore and aft; one leg's reach read zero both ways on 4 of them
        // and capped the whole body's stride at nothing, a hoof missing its target by up to
        // 20 cm). The pair's NEUTRAL footprint is the middle of the two, fore and aft, on the
        // ground the lower one stands on: where the pair stands as a whole. A square pair is
        // its own middle.
        for g in &girdles {
            let [l, r] = g.feet;
            let (a, b) = (feet[l].rest_offset, feet[r].rest_offset);
            for f in [l, r] {
                feet[f].rest_offset.y = 0.5 * (a.y + b.y);
                feet[f].rest_offset.z = a.z.min(b.z);
            }
        }
        for foot in &mut feet {
            foot.reach_ahead = foot.measure_reach(rest, parents, -Vec3::Y, REACH_TOLERANCE);
            foot.reach_back = foot.measure_reach(rest, parents, Vec3::Y, REACH_TOLERANCE);
        }
        // The SHORTEST leg is the pendulum the gait is selected on: the limiting limb sets the
        // Froude speed and the stride every foot must reach through (a hoofed hind leg swings
        // from a hip near the croup, its foreleg from a shoulder well below the withers — the
        // mean over-strode the forelegs by a centimetre at the walk, 2026-09-29).
        let leg_len = feet.iter().map(|f| f.leg_len).fold(f32::INFINITY, f32::min);
        let leg_len = if leg_len.is_finite() { leg_len } else { 1.0 };
        let spine = ["spine_01", "spine_02", "spine_03"]
            .iter()
            .filter_map(|n| names.iter().position(|b| b == n))
            .collect();
        let gait = GaitKind::Stand;
        Self {
            family,
            feet,
            spine,
            girdles,
            proboscis: proboscis_of(names, rest),
            reach: None,
            clock: 0.0,
            body: BodySolver {
                rest_height: 0.0,
                cling: 0.0,
            },
            gait,
            pattern: pattern(gait),
            phase: 0.0,
            leg_len,
            position: Vec3::ZERO,
            heading: -Vec3::Y,
            frame: BodyFrame::level(Vec3::ZERO, -Vec3::Y),
            residual: 0.0,
        }
    }

    /// The longest stride (in leg lengths) whose stance travel every foot can reach through:
    /// a foot stances from `reach_ahead` of its footprint to `reach_back` behind it, over
    /// `duty` of the cycle.
    pub fn stride_cap(&self) -> f32 {
        // The trunk's sway carries the hips sideways, spending some of each foot's reach.
        let sway = self.pattern.sway * self.leg_len;
        let less_sway = |reach: f32| (reach * reach - sway * sway).max(0.0).sqrt();
        let span = self
            .feet
            .iter()
            .map(|f| less_sway(f.reach_ahead) + less_sway(f.reach_back))
            .fold(f32::INFINITY, f32::min);
        if !span.is_finite() {
            return f32::INFINITY;
        }
        span * REACH_MARGIN / self.pattern.duty.max(1e-3) / self.leg_len
    }

    /// Step the body: advance `position` along `heading` at `speed` (cm/s) for `dt`, pick the
    /// gait, run the clock, stand the trunk over its feet, place the whole rig in that frame
    /// (from `rest`) and plant every foot. Returns the trunk's frame.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        globals: &mut [Mat4],
        parents: &[i32],
        rest: &[Mat4],
        heading: Vec3,
        speed: f32,
        surface: &dyn SurfaceQuery,
        dt: f32,
    ) -> BodyFrame {
        let heading = Vec3::new(heading.x, heading.y, 0.0)
            .try_normalize()
            .unwrap_or(self.heading);
        self.heading = heading;
        let velocity = heading * speed;
        self.position += velocity * dt;
        let gait = select(self.family, speed, self.leg_len);
        if gait != self.gait {
            self.gait = gait;
            self.pattern = pattern(gait);
        }
        // The clock: the cycle scales with speed so the stride length holds — the table's
        // stride, or as much of it as the shortest-reaching leg can stance through.
        let stride = self.pattern.stride.min(self.stride_cap());
        let period = if speed.abs() < 1.0 || self.pattern.reference_speed <= 0.0 {
            self.pattern.period_s
        } else {
            (stride * self.leg_len / speed.abs())
                .clamp(self.pattern.period_s * 0.4, self.pattern.period_s * 2.5)
        };
        self.phase = (self.phase + dt / period).rem_euclid(1.0);
        // The trunk over last tick's stance contacts.
        let contacts: Vec<Vec3> = self
            .feet
            .iter()
            .filter_map(|f| f.contact().map(|c| c.point))
            .collect();
        let normals: Vec<Vec3> = self
            .feet
            .iter()
            .filter_map(|f| f.contact().map(|c| c.normal))
            .collect();
        let bob =
            self.pattern.bob * self.leg_len * (self.phase * std::f32::consts::TAU * 2.0).sin();
        self.body.cling = self.pattern.cling;
        let sway = self.pattern.sway * self.leg_len * (self.phase * std::f32::consts::TAU).sin();
        let mut frame = self
            .body
            .solve(&contacts, &normals, self.position, heading, bob);
        frame.origin += frame.right * sway;
        self.frame = frame;
        // Place the whole rig rigidly in the frame, then plant every foot.
        let carry = frame.matrix();
        for (g, r) in globals.iter_mut().zip(rest) {
            *g = carry * *r;
        }
        // The sprawler's trunk: a lateral wave travelling down the spine (the legs hanging off
        // it are planted after, so their feet hold).
        if self.pattern.sway > 0.0 {
            let amplitude = self.pattern.sway * SPINE_UNDULATION;
            for (k, &joint) in self.spine.iter().enumerate() {
                let angle =
                    amplitude * (self.phase * std::f32::consts::TAU - k as f32 * SPINE_LAG).sin();
                turn_subtree(
                    globals,
                    parents,
                    joint,
                    Quat::from_axis_angle(frame.up, angle),
                );
            }
        }
        let reach = self.leg_len * 0.6;
        let (pattern, phase) = (self.pattern, self.phase);
        // Where every foot goes this tick, before any leg is solved: the trunk rides them.
        let targets: Vec<Vec3> = self
            .feet
            .iter_mut()
            .map(|foot| foot.target(phase, &pattern, period, &frame, velocity, surface, reach))
            .collect();
        // A sprawler's trunk is its wave and a climber lies on its branch; every other body
        // is carried on legs under it.
        if !matches!(pattern.kind, GaitKind::Sprawl | GaitKind::Climb) {
            self.ride_girdles(globals, parents, &frame, &targets);
        }
        let mut worst = 0.0f32;
        for (foot, target) in self.feet.iter().zip(&targets) {
            worst = worst.max(plant(globals, parents, &foot.limb, *target));
        }
        self.residual = worst;
        // The head is where the girdles and the legs have put it: what hangs off it moves last.
        self.clock += dt;
        let floor = if contacts.is_empty() {
            frame.origin.z - self.body.rest_height
        } else {
            contacts.iter().map(|c| c.z).sum::<f32>() / contacts.len() as f32
        };
        self.drive_proboscis(globals, parents, &frame, floor);
        frame
    }

    /// THE PROBOSCIS CURLS AND REACHES (ruling 7881216F). Its idle is a CURL — a wave of pitch
    /// about the body's right axis travelling down the chain, so the tip curls up and hangs
    /// again ([`CURL_AMPLITUDE`], [`CURL_LAG`], [`CURL_PERIOD_S`]) — and now and then a REACH:
    /// the tip sent to the ground half its length ahead of its root and brought back
    /// ([`REACH_PERIOD_S`], [`REACH_WINDOW`]), solved by CCD from the curl, tip joint first, so
    /// a target past its length straightens it toward the target and no further. A target the
    /// controller sets (`reach`) takes the idle's place, held for as long as it is set.
    fn drive_proboscis(
        &self,
        globals: &mut [Mat4],
        parents: &[i32],
        frame: &BodyFrame,
        floor: f32,
    ) {
        let Some(p) = &self.proboscis else {
            return;
        };
        let (Some(&root), Some(&end)) = (p.chain.first(), p.chain.last()) else {
            return;
        };
        let t = self.clock * std::f32::consts::TAU / CURL_PERIOD_S;
        for (k, &joint) in p.chain.iter().enumerate() {
            let angle = CURL_AMPLITUDE * (t - k as f32 * CURL_LAG).sin();
            turn_subtree(
                globals,
                parents,
                joint,
                Quat::from_axis_angle(frame.right, angle),
            );
        }
        let at = |g: &[Mat4], i: usize| g.get(i).map(|m| m.w_axis.truncate());
        let (Some(root_at), Some(tip)) = (at(globals, root), at(globals, end)) else {
            return;
        };
        let (goal, weight) = match self.reach {
            Some(target) => (target, 1.0),
            None => {
                let u = (self.clock / REACH_PERIOD_S).rem_euclid(1.0);
                if u >= REACH_WINDOW {
                    return;
                }
                let ahead =
                    Vec3::new(root_at.x, root_at.y, floor) + frame.forward * (0.5 * p.length);
                (
                    ahead,
                    (std::f32::consts::PI * u / REACH_WINDOW).sin().powi(2),
                )
            }
        };
        let tip_first: Vec<usize> = p.chain.iter().rev().copied().collect();
        ccd(
            globals,
            parents,
            &tip_first,
            end,
            tip.lerp(goal, weight),
            6,
            0.5,
        );
    }

    /// THE TRUNK RIDES ITS GIRDLES (Aaron on the Elk, 2026-10-02: *"the ik motion moving just
    /// the feet therefore looks incredibly awkward"* — the trunk was one rigid frame and the
    /// walk swung four legs under a statue). Each [`Girdle`] answers its own two feet, read
    /// off where they are going this tick (`targets`, one per foot):
    ///   * it YAWS toward the foot that reaches ahead ([`GIRDLE_YAW`]) and ROLLS down over the
    ///     foot that is lifted ([`GIRDLE_ROLL`]) — about its own joint, carrying its limbs (and
    ///     a tail) and nothing else of the trunk: the children that lead on to another girdle or
    ///     to the head are turned back about the same joint, so the spine between two girdles
    ///     takes up their difference and the head stays steady over the shoulders;
    ///   * it SINKS as a stiff leg's hip does ([`GIRDLE_SINK`]): by the arc of each foot that
    ///     bears it, weighed by how fully it bears ([`FootPlanner::bearing`]) — highest as a
    ///     leg passes upright, lowest when the pair changes over — and by what its roll would
    ///     otherwise lift the standing hip. It never rises over its rest, so no leg is asked
    ///     for length it does not have. The trunk PITCHES and heaves as one to put the
    ///     rear-most and the front-most girdle each at its own height: the bob of a walk is the
    ///     legs' own, a quarter-cycle apart fore and hind.
    ///
    /// The feet are planted after, on the same targets, so nothing here moves a planted foot.
    fn ride_girdles(
        &self,
        globals: &mut [Mat4],
        parents: &[i32],
        frame: &BodyFrame,
        targets: &[Vec3],
    ) {
        let pattern = &self.pattern;
        let mut sunk: Vec<(Vec3, f32)> = Vec::with_capacity(self.girdles.len());
        for g in &self.girdles {
            let Some(joint) = globals.get(g.bone).map(|m| m.w_axis.truncate()) else {
                continue;
            };
            let read = |f: usize| {
                let foot = &self.feet[f];
                let ahead = (targets[f] - foot.footprint(frame)).dot(frame.forward);
                (
                    ahead,
                    foot.lift(pattern),
                    foot.bearing(pattern),
                    foot.leg_len,
                )
            };
            let ((al, ll, bl, len_l), (ar, lr, br, len_r)) = (read(g.feet[0]), read(g.feet[1]));
            let leg = 0.5 * (len_l + len_r);
            // The canon's left is +X: a turn about `up` carries the left hip BACK, one about
            // `forward` lifts it.
            let yaw = (-GIRDLE_YAW * (al - ar) / leg).clamp(-GIRDLE_MAX, GIRDLE_MAX);
            let roll = (-GIRDLE_ROLL * (ll - lr) / leg).clamp(-GIRDLE_MAX, GIRDLE_MAX);
            let turn =
                Quat::from_axis_angle(frame.up, yaw) * Quat::from_axis_angle(frame.forward, roll);
            turn_about(globals, parents, g.bone, joint, turn);
            for &c in &g.relief {
                turn_about(globals, parents, c, joint, turn.inverse());
            }
            let arc = |ahead: f32, len: f32| len - (len * len - ahead * ahead).max(0.0).sqrt();
            let borne = bl + br;
            let sink = if borne > 1e-4 {
                (bl * arc(al, len_l) + br * arc(ar, len_r)) / borne
                    * (borne / BEARING_FULL).min(1.0)
            } else {
                0.0
            };
            sunk.push((joint, GIRDLE_SINK * sink + roll.abs() * 0.5 * g.width));
        }
        let carry = match sunk.as_slice() {
            [] => return,
            [(_, sink)] => Mat4::from_translation(-frame.up * *sink),
            [(rear, sr), .., (front, sf)] => {
                let span = (*front - *rear).dot(frame.forward);
                let pitch = if span.abs() > 1e-3 {
                    -(sf - sr) / span
                } else {
                    0.0
                };
                let mid = 0.5 * (*rear + *front);
                Mat4::from_translation(mid - frame.up * 0.5 * (sr + sf))
                    * Mat4::from_quat(Quat::from_axis_angle(frame.right, pitch))
                    * Mat4::from_translation(-mid)
            }
        };
        // The root stays the controller's frame; everything under it rides.
        for (g, p) in globals.iter_mut().zip(parents) {
            if *p >= 0 {
                *g = carry * *g;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gait::pattern::FOOT_SLOTS;
    use crate::gait::surface::{FlatFloor, HeightFn};

    /// A synthetic hoofed quadruped at rest: root on the ground, the trunk 60 cm long at 56 cm,
    /// four three-joint legs ending in hooves on the ground (stifles forward, elbows back).
    fn quadruped() -> (Vec<&'static str>, Vec<i32>, Vec<Mat4>) {
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut rest = Vec::new();
        let mut push = |n: &'static str, p: i32, at: Vec3| -> i32 {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(at));
            rest.len() as i32 - 1
        };
        push("root", -1, Vec3::ZERO);
        push("pelvis", 0, Vec3::new(0.0, 30.0, 56.0));
        // The trunk chain pelvis (1) → spine_01 (2) → spine_02 (3) → spine_03 (4); the hind
        // legs hang off the pelvis, the forelegs off spine_03.
        push("spine_01", 1, Vec3::new(0.0, 10.0, 57.0));
        push("spine_02", 2, Vec3::new(0.0, -10.0, 58.0));
        push("spine_03", 3, Vec3::new(0.0, -30.0, 58.0));
        for (side, sign) in [("l", 1.0), ("r", -1.0)] {
            let x = 9.0 * sign;
            // Hind leg off the pelvis: stifle forward, hock back, hoof on the ground.
            let names_h: [&'static str; 5] = match side {
                "l" => ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"],
                _ => ["thigh_r", "calf_r", "foot_r", "ball_r", "hoof_r"],
            };
            let mut i = push(names_h[0], 1, Vec3::new(x, 30.0, 53.0));
            i = push(names_h[1], i, Vec3::new(x, 18.0, 36.0));
            i = push(names_h[2], i, Vec3::new(x, 36.0, 18.0));
            i = push(names_h[3], i, Vec3::new(x, 33.0, 6.0));
            push(names_h[4], i, Vec3::new(x, 33.0, 0.0));
            // Foreleg off the withers: elbow back, knee forward, hoof on the ground.
            let names_f: [&'static str; 5] = match side {
                "l" => [
                    "upperarm_l",
                    "lowerarm_l",
                    "hand_l",
                    "foredigit_l",
                    "forehoof_l",
                ],
                _ => [
                    "upperarm_r",
                    "lowerarm_r",
                    "hand_r",
                    "foredigit_r",
                    "forehoof_r",
                ],
            };
            let mut j = push(names_f[0], 4, Vec3::new(x, -34.0, 48.0));
            j = push(names_f[1], j, Vec3::new(x, -22.0, 30.0));
            j = push(names_f[2], j, Vec3::new(x, -34.0, 12.0));
            j = push(names_f[3], j, Vec3::new(x, -35.0, 6.0));
            push(names_f[4], j, Vec3::new(x, -35.0, 0.0));
        }
        (names, parents, rest)
    }

    /// Two seconds of walking on a level floor: the body advances with the controller, every
    /// planted hoof stays within a millimetre of its contact from tick to tick (no sliding),
    /// every landing is on the floor, a swinging hoof rises, and four feet take their slots.
    #[test]
    fn a_quadruped_walks_the_floor_without_sliding() {
        let (names, parents, rest) = quadruped();
        let mut walker = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        assert_eq!(walker.feet.len(), 4, "four planted limbs");
        let mut slots: Vec<usize> = walker.feet.iter().map(|f| f.slot).collect();
        slots.sort();
        assert_eq!(slots, vec![0, 1, 2, 3], "one foot per slot: {FOOT_SLOTS:?}");
        let floor = FlatFloor { height: 0.0 };
        let mut globals = rest.clone();
        let dt = 1.0 / 60.0;
        let mut last: Vec<Option<(Vec3, Vec3)>> = vec![None; 4]; // (contact, effector) per foot
        let mut max_slide: f32 = 0.0;
        let mut lifted = false;
        let mut landings = 0;
        for tick in 0..120 {
            let frame = walker.step(&mut globals, &parents, &rest, -Vec3::Y, 100.0, &floor, dt);
            assert_eq!(walker.gait, GaitKind::Walk, "a metre a second walks");
            assert!(
                walker.residual < 0.5,
                "every hoof reaches its target: {} cm at tick {tick}",
                walker.residual
            );
            assert!(
                (frame.origin.y - (-100.0 * dt * (tick + 1) as f32)).abs() < 1e-2,
                "the body advances"
            );
            for (k, foot) in walker.feet.iter().enumerate() {
                let effector = globals[foot.limb.effector].w_axis.truncate();
                match foot.contact() {
                    Some(c) => {
                        assert!(c.point.z.abs() < 1e-3, "landed on the floor");
                        match last[k] {
                            Some((prev_c, prev_e)) if prev_c == c.point => {
                                max_slide = max_slide.max((effector - prev_e).length());
                            }
                            _ => landings += 1,
                        }
                        last[k] = Some((c.point, effector));
                    }
                    None => {
                        if effector.z > 2.0 {
                            lifted = true;
                        }
                        last[k] = None;
                    }
                }
            }
        }
        assert!(
            max_slide < 0.1,
            "a planted hoof never slides: {max_slide} cm"
        );
        assert!(lifted, "a swinging hoof rises");
        assert!(landings >= 8, "each foot landed at least twice: {landings}");
        assert!(walker.phase > 0.0 && walker.phase < 1.0);
    }

    /// THE TRUNK RIDES ITS GIRDLES. A walking quadruped's pelvis and withers each answer
    /// their own two feet: each hip line swings both ways about the vertical and tips both ways
    /// off the level, the two out of step with each other (a walk's fore and hind pairs are a
    /// quarter-cycle apart), and the trunk pitches between them. No hip ever rides over its
    /// rest height, the spine between the girdles and the head beyond the withers keep the
    /// body's own heading, and every hoof still reaches its target and holds it.
    #[test]
    fn a_walkers_trunk_rides_its_girdles() {
        let (mut names, mut parents, mut rest) = quadruped();
        let bone = |names: &[&str], n: &str| names.iter().position(|b| *b == n).unwrap();
        let withers = bone(&names, "spine_03");
        names.push("neck_01");
        parents.push(withers as i32);
        rest.push(Mat4::from_translation(Vec3::new(0.0, -40.0, 64.0)));
        names.push("head");
        parents.push(names.len() as i32 - 2);
        rest.push(Mat4::from_translation(Vec3::new(0.0, -52.0, 74.0)));
        let (pelvis, loin, mid, neck, head) = (
            bone(&names, "pelvis"),
            bone(&names, "spine_01"),
            bone(&names, "spine_02"),
            bone(&names, "neck_01"),
            bone(&names, "head"),
        );
        let mut walker = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        assert_eq!(
            walker
                .girdles
                .iter()
                .map(|g| (g.bone, g.relief.clone()))
                .collect::<Vec<_>>(),
            vec![(pelvis, vec![loin]), (withers, vec![neck])],
            "the pelvis over the hind legs, the withers over the forelegs, rear first"
        );
        let floor = FlatFloor { height: 0.0 };
        let mut globals = rest.clone();
        let at = |g: &[Mat4], i: usize| g[i].w_axis.truncate();
        // A hip line's bearing off square and its tip off level, degrees, in the body's frame.
        let line = |g: &[Mat4], frame: &BodyFrame, l: &str, r: &str| {
            let d = at(g, bone(&names, l)) - at(g, bone(&names, r));
            let (ahead, across, up) = (d.dot(frame.forward), -d.dot(frame.right), d.dot(frame.up));
            (
                ahead.atan2(across).to_degrees(),
                up.atan2(across).to_degrees(),
            )
        };
        let mut span = [[f32::INFINITY, f32::NEG_INFINITY]; 6];
        let mut widen = |k: usize, v: f32| {
            span[k] = [span[k][0].min(v), span[k][1].max(v)];
        };
        let mut last: Vec<Option<(Vec3, Vec3)>> = vec![None; 4];
        let mut max_slide: f32 = 0.0;
        for tick in 0..300 {
            let frame = walker.step(
                &mut globals,
                &parents,
                &rest,
                -Vec3::Y,
                100.0,
                &floor,
                1.0 / 60.0,
            );
            assert!(
                walker.residual < 0.5,
                "every hoof reaches: {} at {tick}",
                walker.residual
            );
            for (k, foot) in walker.feet.iter().enumerate() {
                let effector = at(&globals, foot.limb.effector);
                last[k] = match (foot.contact(), last[k]) {
                    (Some(c), Some((prev_c, prev_e))) if prev_c == c.point => {
                        max_slide = max_slide.max((effector - prev_e).length());
                        Some((c.point, effector))
                    }
                    (Some(c), _) => Some((c.point, effector)),
                    (None, _) => None,
                };
            }
            if tick < 90 {
                continue; // the first cycle starts every foot mid-air
            }
            let (hind, fore) = (
                line(&globals, &frame, "thigh_l", "thigh_r"),
                line(&globals, &frame, "upperarm_l", "upperarm_r"),
            );
            widen(0, hind.0);
            widen(1, fore.0);
            widen(2, hind.1);
            widen(3, fore.1);
            widen(4, hind.0 - fore.0);
            widen(
                5,
                (at(&globals, withers) - at(&globals, pelvis)).dot(frame.up),
            );
            for hip in ["thigh_l", "thigh_r", "upperarm_l", "upperarm_r"] {
                let i = bone(&names, hip);
                let over = (at(&globals, i) - frame.origin).dot(frame.up) - at(&rest, i).z;
                assert!(over < 0.05, "{hip} rides {over} cm over its rest at {tick}");
            }
            // The spine between the girdles and the head keep the body's heading: their own
            // lateral axis stays the frame's, but for the trunk's pitch.
            for steady in [loin, mid, head] {
                let across = globals[steady].x_axis.truncate();
                assert!(
                    (across + frame.right).length() < 1e-3,
                    "{} keeps the heading: {across} at {tick}",
                    names[steady]
                );
            }
        }
        assert!(
            max_slide < 0.1,
            "a planted hoof never slides: {max_slide} cm"
        );
        for (k, what) in [(0, "the pelvis yaws"), (1, "the withers yaw")] {
            assert!(
                span[k][0] < -3.0 && span[k][1] > 3.0,
                "{what} both ways: {:?}",
                span[k]
            );
        }
        for (k, what) in [(2, "the pelvis rolls"), (3, "the withers roll")] {
            assert!(
                span[k][0] < -1.5 && span[k][1] > 1.5,
                "{what} both ways: {:?}",
                span[k]
            );
        }
        assert!(
            span[4][1] - span[4][0] > 6.0,
            "the two girdles turn out of step: {:?}",
            span[4]
        );
        assert!(
            span[5][1] - span[5][0] > 0.5,
            "the trunk pitches between them: {:?}",
            span[5]
        );
    }

    /// A hoofed body as one source rests it (its own joints): the hind legs caught mid-stride —
    /// the left trailing at full stretch with a long cannon under its hock, the right stepped
    /// forward under the belly — both hooves on the floor 77 cm apart; the forelegs square.
    fn resting_mid_stride() -> (Vec<&'static str>, Vec<i32>, Vec<Mat4>) {
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut rest = Vec::new();
        let mut push = |n: &'static str, p: i32, at: [f32; 3]| -> i32 {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(Vec3::from_array(at)));
            rest.len() as i32 - 1
        };
        push("root", -1, [0.0, 0.0, 0.0]);
        let pelvis = push("pelvis", 0, [0.0, 44.2, 106.8]);
        let mut spine = pelvis;
        for (n, y, z) in [
            ("spine_01", 15.0, 112.0),
            ("spine_02", -15.0, 116.0),
            ("spine_03", -41.4, 120.0),
        ] {
            spine = push(n, spine, [0.0, y, z]);
        }
        let hind: [(&[&'static str; 5], [[f32; 3]; 5]); 2] = [
            (
                &["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"],
                [
                    [13.8, 44.2, 112.3],
                    [7.5, 56.0, 82.0],
                    [3.8, 72.5, 56.6],
                    [5.6, 83.4, 11.0],
                    [2.0, 88.9, 1.8],
                ],
            ),
            (
                &["thigh_r", "calf_r", "foot_r", "ball_r", "hoof_r"],
                [
                    [-12.1, 44.2, 112.3],
                    [-10.8, 41.4, 73.1],
                    [-12.6, 41.4, 34.7],
                    [-18.1, 28.6, 11.0],
                    [-10.8, 12.2, 1.8],
                ],
            ),
        ];
        for (chain, joints) in hind {
            let mut p = pelvis;
            for (n, at) in chain.iter().zip(joints) {
                p = push(n, p, at);
            }
        }
        let fore: [&[&'static str; 6]; 2] = [
            &[
                "clavicle_l",
                "upperarm_l",
                "lowerarm_l",
                "hand_l",
                "foredigit_l",
                "forehoof_l",
            ],
            &[
                "clavicle_r",
                "upperarm_r",
                "lowerarm_r",
                "hand_r",
                "foredigit_r",
                "forehoof_r",
            ],
        ];
        for (chain, sign) in fore.into_iter().zip([1.0_f32, -1.0]) {
            let mut p = spine;
            for (n, [x, y, z]) in chain.iter().zip([
                [9.5, -41.4, 123.8],
                [14.5, -56.6, 82.6],
                [14.8, -53.6, 58.4],
                [16.6, -63.3, 32.3],
                [15.2, -81.8, 7.9],
                [14.8, -86.4, 1.8],
            ]) {
                p = push(n, p, [sign * x, y, z]);
            }
        }
        (names, parents, rest)
    }

    /// A BODY RESTING MID-STRIDE STILL WALKS. Each foot stepping round its OWN rest footprint,
    /// the trailing leg has nowhere to go behind it and its twin nowhere ahead: the two feet of
    /// a pair tread different ground, one forever trailing and one forever leading. Stepping
    /// round the MIDDLE of the pair, both hind feet share one neutral footprint with travel
    /// either side of it, the stride is a walker's, and every hoof reaches its target.
    #[test]
    fn a_body_resting_mid_stride_walks_round_the_middle_of_each_pair() {
        let (names, parents, rest) = resting_mid_stride();
        let bone = |n: &str| names.iter().position(|b| *b == n).unwrap();
        let y = |n: &str| rest[bone(n)].w_axis.y;
        let walker = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        let hind: Vec<&FootPlanner> = walker.feet.iter().filter(|f| f.slot >= 2).collect();
        let middle = 0.5 * (y("hoof_l") + y("hoof_r"));
        assert!(
            hind.len() == 2 && hind.iter().all(|f| (f.rest_offset.y - middle).abs() < 1e-4),
            "both hind feet step round the middle of the pair"
        );
        for f in &hind {
            assert!(
                f.reach_ahead > 0.25 * f.leg_len && f.reach_back > 0.25 * f.leg_len,
                "{}: travel both sides of the middle: {} ahead, {} back",
                names[f.limb.effector],
                f.reach_ahead,
                f.reach_back
            );
        }
        // Each foot round its OWN rest footprint, as the walk once stepped: the trailing leg
        // can only come forward, its twin only go back — two feet of one pair that never
        // tread the same ground.
        let own: Vec<(f32, f32)> = hind
            .iter()
            .map(|f| {
                let mut f = (*f).clone();
                f.rest_offset = rest[f.limb.effector].w_axis.truncate();
                (
                    f.measure_reach(&rest, &parents, -Vec3::Y, REACH_TOLERANCE),
                    f.measure_reach(&rest, &parents, Vec3::Y, REACH_TOLERANCE),
                )
            })
            .collect();
        let leg = hind[0].leg_len;
        assert!(
            own[0].1 < 0.1 * leg && own[0].0 > 0.5 * leg && own[1].0 < 0.25 * leg,
            "round their own footprints: the trailing leg {:?}, its twin {:?}",
            own[0],
            own[1]
        );
        let mut walker = walker;
        walker.pattern = pattern(GaitKind::Walk);
        assert!(
            walker.stride_cap() > 0.7,
            "a walker's stride: {} leg lengths",
            walker.stride_cap()
        );
        let floor = FlatFloor { height: 1.8 };
        let mut globals = rest.clone();
        let speed = crate::gait::speed_for(GaitKind::Walk, walker.leg_len);
        for tick in 0..240 {
            walker.step(
                &mut globals,
                &parents,
                &rest,
                -Vec3::Y,
                speed,
                &floor,
                1.0 / 60.0,
            );
            assert!(
                tick < 60 || walker.residual < 0.5,
                "every hoof reaches its target: {} cm at tick {tick}",
                walker.residual
            );
        }
    }

    /// A sprawler's trunk undulates: walking the same floor, the withers swing left and right
    /// of the pelvis across the cycle while a walker's trunk stays straight; the feet still
    /// plant on the floor and reach their targets.
    #[test]
    fn a_sprawlers_trunk_undulates_and_a_walkers_stays_straight() {
        let (names, parents, rest) = quadruped();
        let floor = FlatFloor { height: 0.0 };
        let withers = names.iter().position(|n| *n == "spine_03").unwrap();
        let pelvis = names.iter().position(|n| *n == "pelvis").unwrap();
        let lateral = |family: LocomotionFamily, speed: f32| -> (f32, f32) {
            let mut body = Locomotion::new(&names, &parents, &rest, family);
            assert_eq!(body.spine.len(), 3, "three spine joints found");
            let mut globals = rest.clone();
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for _ in 0..120 {
                let frame = body.step(
                    &mut globals,
                    &parents,
                    &rest,
                    -Vec3::Y,
                    speed,
                    &floor,
                    1.0 / 60.0,
                );
                assert!(body.residual < 0.5, "feet reach: {}", body.residual);
                let w = globals[withers].w_axis.truncate() - globals[pelvis].w_axis.truncate();
                let side = w.dot(frame.right);
                lo = lo.min(side);
                hi = hi.max(side);
            }
            (lo, hi)
        };
        let (lo, hi) = lateral(LocomotionFamily::Sprawler, 40.0);
        assert!(
            lo < -3.0 && hi > 3.0,
            "the sprawler's withers swing both ways: {lo}..{hi}"
        );
        let (lo, hi) = lateral(LocomotionFamily::Walker, 100.0);
        assert!(
            lo.abs() < 1e-3 && hi.abs() < 1e-3,
            "the walker's trunk is straight: {lo}..{hi}"
        );
    }

    /// Standing still stands: no foot moves, the body stays put, the gait is Stand.
    /// A body with a proboscis: a trunk and head standing still, a six-bone chain hanging from
    /// the face. Stepped through its idle the chain CURLS — its tip swings tens of centimetres
    /// while its root stays on the head — and told to reach, its tip lands on a target within
    /// its length in one step and only straightens toward one beyond it.
    #[test]
    fn a_proboscis_curls_in_its_idle_and_reaches_what_it_is_told_to() {
        let mut names = Vec::new();
        let mut parents = Vec::new();
        let mut rest = Vec::new();
        let mut push = |n: &'static str, p: i32, at: Vec3| -> i32 {
            names.push(n);
            parents.push(p);
            rest.push(Mat4::from_translation(at));
            rest.len() as i32 - 1
        };
        push("root", -1, Vec3::ZERO);
        let pelvis = push("pelvis", 0, Vec3::new(0.0, 30.0, 100.0));
        let s1 = push("spine_01", pelvis, Vec3::new(0.0, 0.0, 102.0));
        let s3 = push("spine_03", s1, Vec3::new(0.0, -40.0, 104.0));
        let n1 = push("neck_01", s3, Vec3::new(0.0, -60.0, 110.0));
        let head = push("head", n1, Vec3::new(0.0, -80.0, 112.0));
        let mut parent = head;
        for (k, name) in [
            "proboscis_01",
            "proboscis_02",
            "proboscis_03",
            "proboscis_04",
            "proboscis_05",
            "proboscis_06",
        ]
        .into_iter()
        .enumerate()
        {
            parent = push(name, parent, Vec3::new(0.0, -95.0, 100.0 - 15.0 * k as f32));
        }
        let mut body = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        let p = body
            .proboscis
            .clone()
            .expect("the chain is read off its names");
        assert_eq!(p.chain.len(), 6);
        assert!((p.length - 75.0).abs() < 1e-3, "root to tip: {}", p.length);
        let floor = FlatFloor { height: 0.0 };
        let tip_of = |g: &[Mat4]| g[p.chain[5]].w_axis.truncate();
        let root_of = |g: &[Mat4]| g[p.chain[0]].w_axis.truncate();
        // THE CURL, over one period of standing still: the tip wanders, the root does not.
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        let mut root_moved = 0.0_f32;
        let mut globals = rest.clone();
        for _ in 0..80 {
            body.step(&mut globals, &parents, &rest, -Vec3::Y, 0.0, &floor, 0.05);
            let tip = tip_of(&globals);
            lo = lo.min(tip);
            hi = hi.max(tip);
            root_moved = root_moved.max(root_of(&globals).distance(Vec3::new(0.0, -95.0, 100.0)));
        }
        assert!(
            (hi - lo).length() > 20.0,
            "the tip curls through tens of centimetres: {lo} .. {hi}"
        );
        assert!(
            root_moved < 1.0,
            "the root stays on the head: {root_moved} cm"
        );
        // THE REACH: a point well within its length lands in one step...
        let want = Vec3::new(20.0, -120.0, 60.0);
        body.reach = Some(want);
        body.step(&mut globals, &parents, &rest, -Vec3::Y, 0.0, &floor, 0.05);
        let got = tip_of(&globals);
        assert!(
            got.distance(want) < 1.0,
            "the tip reaches the target: {got} vs {want}"
        );
        // ...and one beyond it straightens the chain toward it, no further than it is long.
        let far = Vec3::new(0.0, -95.0, -200.0);
        body.reach = Some(far);
        body.step(&mut globals, &parents, &rest, -Vec3::Y, 0.0, &floor, 0.05);
        let got = tip_of(&globals);
        let root = root_of(&globals);
        assert!(
            (root.distance(got) - p.length).abs() < 1.0 && got.z < root.z - 70.0,
            "straight toward it: tip {got}, root {root}"
        );
        // Let go, the idle takes it back: the tip hangs near its rest again within a period.
        body.reach = None;
        body.clock = CURL_PERIOD_S * 10.0 + REACH_PERIOD_S * REACH_WINDOW + 0.1;
        body.step(&mut globals, &parents, &rest, -Vec3::Y, 0.0, &floor, 0.05);
        assert!(
            tip_of(&globals).distance(Vec3::new(0.0, -95.0, 25.0)) < 30.0,
            "hanging again: {}",
            tip_of(&globals)
        );
    }

    #[test]
    fn standing_still_stands() {
        let (names, parents, rest) = quadruped();
        let mut walker = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        let floor = FlatFloor { height: 0.0 };
        let mut globals = rest.clone();
        for _ in 0..30 {
            walker.step(
                &mut globals,
                &parents,
                &rest,
                -Vec3::Y,
                0.0,
                &floor,
                1.0 / 60.0,
            );
        }
        assert_eq!(walker.gait, GaitKind::Stand);
        assert!(walker.position.length() < 1e-4);
        for foot in &walker.feet {
            let e = globals[foot.limb.effector].w_axis.truncate();
            assert!(
                (e - rest[foot.limb.effector].w_axis.truncate()).length() < 0.5,
                "every hoof holds its footprint"
            );
        }
    }

    /// A slope: walking up a ramp the feet land on the ramp (not the plane below it) and the
    /// body rises with them; a climber's frame tilts with the ramp, a walker's stays level.
    #[test]
    fn feet_land_on_a_ramp_and_a_climber_tilts_to_it() {
        let (names, parents, rest) = quadruped();
        let ramp = HeightFn {
            height: |_x: f32, y: f32| -0.25 * y, // rises toward −Y, the walking direction
            step: 1.0,
        };
        let mut walker = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Walker);
        let mut globals = rest.clone();
        for _ in 0..180 {
            walker.step(
                &mut globals,
                &parents,
                &rest,
                -Vec3::Y,
                100.0,
                &ramp,
                1.0 / 60.0,
            );
        }
        for foot in &walker.feet {
            if let Some(c) = foot.contact() {
                assert!(
                    (c.point.z - (-0.25 * c.point.y)).abs() < 1e-2,
                    "on the ramp: {c:?}"
                );
            }
        }
        assert!(
            walker.frame.origin.z > 20.0,
            "the body climbed: {}",
            walker.frame.origin.z
        );
        assert!(
            (walker.frame.up - Vec3::Z).length() < 1e-4,
            "a walker stays level"
        );
        let mut climber = Locomotion::new(&names, &parents, &rest, LocomotionFamily::Climber);
        let mut globals = rest.clone();
        for _ in 0..180 {
            climber.step(
                &mut globals,
                &parents,
                &rest,
                -Vec3::Y,
                30.0,
                &ramp,
                1.0 / 60.0,
            );
        }
        assert_eq!(climber.gait, GaitKind::Climb);
        assert!(
            climber.frame.up.y > 0.1,
            "a climber's up leans into the slope: {}",
            climber.frame.up
        );
    }
}
