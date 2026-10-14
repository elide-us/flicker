//! THE FOOT PLANNER: one per limb. In STANCE the effector holds its world contact while the body
//! moves over it (the IK absorbs the motion). In SWING it lifts on an arc to a landing predicted
//! from where its rest footprint will be when it comes down, snapped onto the support by the
//! world's [`SurfaceQuery`]. A foot with nothing under it hangs at its rest footprint.

use glam::{Mat4, Vec3};

use super::body::BodyFrame;
use super::pattern::GaitPattern;
use super::surface::{Contact, SurfaceQuery};
use crate::ik::{plant, LimbChain};

/// The fraction of a foot's measured reach a stance or a landing may use.
pub const REACH_MARGIN: f32 = 0.9;

/// The most of its own residual on a foot's neutral footprint the reach measure excuses the
/// solver (cm): a shoulder blade's follow leaves a few millimetres; a foot that cannot be put
/// on its own footprint within this has no reach to speak of.
pub const SOLVER_NOISE: f32 = 0.5;

/// Where a foot is in its cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FootState {
    /// Planted on a contact (its point is the target).
    Stance(Contact),
    /// Swinging from `from` to the landing `to`; `to` is `None` while no support was found.
    Swing { from: Vec3, to: Option<Contact> },
}

/// One limb's planner.
#[derive(Clone, Debug)]
pub struct FootPlanner {
    pub limb: LimbChain,
    /// The foot's NEUTRAL footprint relative to the root frame, in the canon's body space: the
    /// effector's rest position, or — for one of a pair — the middle of the pair's two
    /// (`Locomotion::new`).
    pub rest_offset: Vec3,
    /// Hip-to-ground at rest — the leg length every distance in the table is a fraction of.
    pub leg_len: f32,
    /// Which [`super::pattern::FOOT_SLOTS`] entry times this foot.
    pub slot: usize,
    /// How far ahead of / behind its footprint the effector can be planted at ground level
    /// (planar cm), measured on the rest pose with the solver itself — the stride is capped so a
    /// stance never asks more of a leg than it has.
    pub reach_ahead: f32,
    pub reach_back: f32,
    pub state: FootState,
    /// This foot's cycle phase last tick, to catch the stance → swing edge.
    last_phase: f32,
}

impl FootPlanner {
    pub fn new(limb: LimbChain, rest_offset: Vec3, leg_len: f32, slot: usize) -> Self {
        Self {
            limb,
            rest_offset,
            leg_len: leg_len.max(1.0),
            slot,
            reach_ahead: f32::INFINITY,
            reach_back: f32::INFINITY,
            state: FootState::Swing {
                from: rest_offset,
                to: None,
            },
            last_phase: 0.0,
        }
    }

    /// Measure how far along `dir` (planar, from the foot's neutral footprint) the solver can
    /// plant this foot within `tolerance` — a bisection over fresh copies of `rest`. The
    /// tolerance is over the solver's OWN residual on that footprint: a leg whose shoulder blade
    /// follows its swing is not returned to the last millimetre even where it already stands,
    /// and read against a bare tolerance that floor made a straight foreleg's reach none at all
    /// ahead — so its whole stance trailed behind it (2026-09-30).
    pub fn measure_reach(&self, rest: &[Mat4], parents: &[i32], dir: Vec3, tolerance: f32) -> f32 {
        let Some(dir) = Vec3::new(dir.x, dir.y, 0.0).try_normalize() else {
            return 0.0;
        };
        let home = self.rest_offset;
        let miss = |d: f32| {
            let mut scratch = rest.to_vec();
            plant(&mut scratch, parents, &self.limb, home + dir * d)
        };
        let floor = miss(0.0).min(SOLVER_NOISE);
        let reaches = |d: f32| miss(d) < floor + tolerance;
        let (mut lo, mut hi) = (0.0, self.leg_len * 2.0);
        if reaches(hi) {
            return hi;
        }
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if reaches(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// What fraction of the stance travel lies AHEAD of the footprint: the forward share of the
    /// measured reach, or half when unmeasured.
    pub fn stance_bias(&self) -> f32 {
        let total = self.reach_ahead + self.reach_back;
        if total.is_finite() && total > 0.0 {
            (self.reach_ahead / total).clamp(0.1, 0.9)
        } else {
            0.5
        }
    }

    /// The effector's rest footprint carried into `frame`.
    pub fn footprint(&self, frame: &BodyFrame) -> Vec3 {
        frame.matrix().transform_point3(self.rest_offset)
    }

    /// Advance this foot to `cycle` (the gait's phase, 0..1, one cycle lasting `period` seconds)
    /// and answer where its effector goes this tick. `velocity` is the body's planar velocity
    /// (cm/s); `reach` how far below or above the footprint a support may be.
    #[allow(clippy::too_many_arguments)]
    pub fn target(
        &mut self,
        cycle: f32,
        pattern: &GaitPattern,
        period: f32,
        frame: &BodyFrame,
        velocity: Vec3,
        surface: &dyn SurfaceQuery,
        reach: f32,
    ) -> Vec3 {
        let lift = pattern.phase[self.slot.min(3)];
        // This foot's own phase: 0 at its lift, its stance lasting `duty` of the cycle after
        // the swing (`1 − duty`) — so the swing runs first from the lift.
        let local = (cycle - lift).rem_euclid(1.0);
        let swing_len = (1.0 - pattern.duty).max(1e-3);
        let footprint = self.footprint(frame);
        let swinging = local < swing_len && pattern.duty < 1.0;
        let lifted_now = swinging
            && (self.last_phase >= swing_len || matches!(self.state, FootState::Stance(_)));
        self.last_phase = local;
        if swinging {
            if lifted_now || matches!(self.state, FootState::Swing { to: None, .. }) {
                let from = match self.state {
                    FootState::Stance(c) => c.point,
                    FootState::Swing { from, .. } => from,
                };
                // Land where the footprint will be at touchdown plus half the stance travel
                // ahead of it, so the stance carries the foot symmetrically under the body.
                // (A foot joining mid-swing — the first tick, a gait change — has less swing
                // left, so its landing sits nearer.)
                // The stance window sits ahead/behind the footprint in proportion to how far
                // the leg reaches each way (a straight foreleg reaches further back).
                // A landing never asks more of the leg than it reaches (a fast turn or a
                // speed jump mid-swing would otherwise strand the foot ahead of the hip).
                let swing_left = (swing_len - local).max(0.0) * period;
                let travel = velocity.length() * pattern.duty * period;
                let lead = (travel * self.stance_bias()).min(self.reach_ahead * REACH_MARGIN);
                let ahead = velocity * swing_left + velocity.normalize_or_zero() * lead;
                let to = surface.support(footprint + ahead, frame.up, reach);
                self.state = FootState::Swing { from, to };
            }
            let FootState::Swing { from, to } = self.state else {
                unreachable!("set above")
            };
            let t = (local / swing_len).clamp(0.0, 1.0);
            let ease = t * t * (3.0 - 2.0 * t);
            let landing = to.map_or(footprint, |c| c.point);
            let arc =
                frame.up * pattern.step_height * self.leg_len * (t * std::f32::consts::PI).sin();
            from.lerp(landing, ease) + arc
        } else {
            match self.state {
                FootState::Stance(c) => c.point,
                FootState::Swing { to, .. } => {
                    // Touch down: plant on the landing — or, with none planned (the first
                    // tick, a blind swing), where a foot this far through its stance would be
                    // in the steady walk, so a start-up never strands a foot behind its reach.
                    let contact = to.or_else(|| {
                        let travel = velocity.length() * pattern.duty * period;
                        let progress =
                            ((local - swing_len) / pattern.duty.max(1e-3)).clamp(0.0, 1.0);
                        let lead =
                            (travel * self.stance_bias()).min(self.reach_ahead * REACH_MARGIN);
                        let trail = (travel * (1.0 - self.stance_bias()))
                            .min(self.reach_back * REACH_MARGIN);
                        let offset = lead - (lead + trail) * progress;
                        let along = velocity.normalize_or_zero() * offset;
                        surface.support(footprint + along, frame.up, reach)
                    });
                    match contact {
                        Some(c) => {
                            self.state = FootState::Stance(c);
                            c.point
                        }
                        None => footprint,
                    }
                }
            }
        }
    }

    /// How high this foot's swing carries it this tick (cm over the line from its lift to its
    /// landing; 0 while it is planted) — as [`Self::target`] last advanced it.
    pub fn lift(&self, pattern: &GaitPattern) -> f32 {
        let swing_len = (1.0 - pattern.duty).max(1e-3);
        if pattern.duty >= 1.0 || self.last_phase >= swing_len {
            return 0.0;
        }
        let t = (self.last_phase / swing_len).clamp(0.0, 1.0);
        pattern.step_height * self.leg_len * (t * std::f32::consts::PI).sin()
    }

    /// How fully this foot BEARS the body this tick: 0 while it swings and at the instants it
    /// lands and lifts, 1 at mid-stance — as [`Self::target`] last advanced it.
    pub fn bearing(&self, pattern: &GaitPattern) -> f32 {
        let swing_len = (1.0 - pattern.duty).max(1e-3);
        if pattern.duty < 1.0 && self.last_phase < swing_len {
            return 0.0;
        }
        let through = ((self.last_phase - swing_len) / pattern.duty.max(1e-3)).clamp(0.0, 1.0);
        (through * std::f32::consts::PI).sin()
    }

    /// The contact this foot stands on, if planted.
    pub fn contact(&self) -> Option<Contact> {
        match self.state {
            FootState::Stance(c) => Some(c),
            FootState::Swing { .. } => None,
        }
    }
}
