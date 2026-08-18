//! Runtime secondary-motion for dangly things — a necklace/cord, a hem, a sleeve, a
//! ribbon. A **jiggle chain** is a line of segments pinned at one end to a driving bone; the
//! free segments swing under gravity, spring back toward their rest shape, and lag the
//! driver's motion, so cloth and jewellery move on their own instead of riding the skeleton
//! rigidly.
//!
//! This is **mesh-associated, animation-agnostic** secondary motion: the solver only needs
//! the driver bone's world position each frame — it works on *any* clip, *any* body, and
//! carries no per-clip authored keys (unlike the Katanami rig, whose cloth motion was baked
//! into every clip). It is the reusable core the necklace is the first user of, and the hem
//! / sleeves / ribbon reuse verbatim.
//!
//! ## Model — position-based verlet
//! Segment positions are integrated with verlet (position + implicit velocity), then a few
//! constraint passes pull each segment back to its rest distance from its parent and, by
//! `stiffness`, back toward the parent-aligned rest direction. Verlet is chosen over an
//! explicit spring because it stays stable at a game `dt` without tiny timesteps. There is
//! **no randomness and no wall-clock** — given the same driver path and `dt` it is bit-for-bit
//! reproducible (so it can be tested, and never breaks resume).

use glam::Vec3;

/// Physical dials for a jiggle chain. Sensible cm/z-up defaults via [`JiggleParams::default`].
#[derive(Clone, Copy, Debug)]
pub struct JiggleParams {
    /// World gravity (cm/s², z-up ⇒ negative z). Pulls the free segments down.
    pub gravity: Vec3,
    /// Return-to-shape spring, `0..=1`. `0` = a limp rope (gravity only); higher pulls each
    /// segment back toward its parent-aligned REST direction, so the chain keeps its form and
    /// a stiff cord doesn't fully collapse.
    pub stiffness: f32,
    /// Velocity retained per step, `0..=1`. `1` = frictionless (swings forever); lower bleeds
    /// energy so the motion settles. ~0.9 reads as light jewellery.
    pub damping: f32,
    /// Distance-constraint relaxation passes per step. More = stiffer/less stretchy links:
    /// measured stretch under gravity is 4.5% @3, 1.1% @8, 0.5% @12 (Gauss-Seidel PBD). 8 is
    /// taut and still trivially cheap for a short chain.
    pub iterations: u32,
    /// `dt` is clamped to this many seconds before integrating, so a frame hitch can't launch
    /// the chain to infinity (verlet with a huge dt explodes).
    pub max_dt: f32,
}

impl Default for JiggleParams {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, 0.0, -980.0),
            stiffness: 0.15,
            damping: 0.9,
            iterations: 8,
            max_dt: 1.0 / 30.0,
        }
    }
}

/// A pinned chain of point masses. `pos[0]` is the anchor (driven by a bone each step); the
/// rest are free. Rendering/skinning reads [`JiggleChain::positions`]; bone rotations for a
/// skinned cord are derived from consecutive positions.
#[derive(Clone, Debug)]
pub struct JiggleChain {
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    /// Rest distance between segment `i` and its parent `i-1` (`rest[0]` unused).
    rest: Vec<f32>,
    /// Rest direction parent→child in the DRIVER's frame at build (unit); the stiffness term
    /// pulls each segment back toward this so the chain remembers its shape.
    rest_dir: Vec<Vec3>,
    params: JiggleParams,
}

impl JiggleChain {
    /// A straight chain of `segments` free links (so `segments + 1` points) hanging from
    /// `anchor` along unit `dir`, each `seg_len` apart. Starts settled (prev == pos ⇒ zero
    /// initial velocity).
    pub fn new(
        anchor: Vec3,
        dir: Vec3,
        seg_len: f32,
        segments: usize,
        params: JiggleParams,
    ) -> Self {
        let dir = dir.normalize_or_zero();
        let n = segments + 1;
        let mut pos = Vec::with_capacity(n);
        let mut rest = Vec::with_capacity(n);
        let mut rest_dir = Vec::with_capacity(n);
        for i in 0..n {
            pos.push(anchor + dir * (seg_len * i as f32));
            rest.push(if i == 0 { 0.0 } else { seg_len });
            rest_dir.push(dir);
        }
        Self {
            prev: pos.clone(),
            pos,
            rest,
            rest_dir,
            params,
        }
    }

    pub fn positions(&self) -> &[Vec3] {
        &self.pos
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    /// Advance one frame. `anchor` is where the pinned end is now (the driver bone's world
    /// position); `driver_rot` orients the stiffness rest directions so the chain's "home
    /// shape" follows the bone (pass identity for a world-fixed hang). `dt` in seconds.
    pub fn step(&mut self, anchor: Vec3, driver_rot: glam::Quat, dt: f32) {
        if self.pos.len() < 2 {
            if let Some(p) = self.pos.first_mut() {
                *p = anchor;
            }
            return;
        }
        let dt = dt.clamp(0.0, self.params.max_dt);
        let g = self.params.gravity * dt * dt;
        let damp = self.params.damping;

        // Verlet integrate the FREE segments (0 is the pinned anchor).
        for i in 1..self.pos.len() {
            let vel = (self.pos[i] - self.prev[i]) * damp;
            self.prev[i] = self.pos[i];
            self.pos[i] += vel + g;
        }
        self.pos[0] = anchor;
        self.prev[0] = anchor;

        // Relaxation: distance constraint + a stiffness pull toward the rest-shape direction.
        let stiff = self.params.stiffness.clamp(0.0, 1.0);
        for _ in 0..self.params.iterations {
            for i in 1..self.pos.len() {
                // Read both points into locals first (Rust can't borrow the same Vec twice in
                // one indexed expression), update, then write back.
                let parent = self.pos[i - 1];
                let mut cur = self.pos[i];
                // Stiffness: nudge toward where the segment would sit if the chain held its
                // rest direction off its parent (rotated by the driver). Keeps the form.
                if stiff > 0.0 {
                    let target = parent + (driver_rot * self.rest_dir[i]) * self.rest[i];
                    cur += (target - cur) * stiff;
                }
                // Distance constraint: restore |child - parent| == rest_len. The parent moves
                // half (or none, if it's the pinned anchor) so the correction propagates.
                let delta = cur - parent;
                let dist = delta.length();
                if dist > 1e-6 {
                    let diff = (dist - self.rest[i]) / dist;
                    if i - 1 == 0 {
                        cur -= delta * diff; // parent pinned → child takes all
                    } else {
                        self.pos[i - 1] = parent + delta * (0.5 * diff);
                        cur -= delta * (0.5 * diff);
                    }
                }
                self.pos[i] = cur;
            }
            self.pos[0] = anchor; // keep the anchor pinned through the passes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    fn settle(chain: &mut JiggleChain, anchor: Vec3, steps: usize) {
        for _ in 0..steps {
            chain.step(anchor, Quat::IDENTITY, 1.0 / 60.0);
        }
    }

    /// Left alone, the chain must hang straight DOWN under gravity — every free segment ends
    /// up ~rest_len below its parent, on the gravity axis. A limp chain (stiffness 0) is the
    /// clean case: pure gravity + distance constraint.
    #[test]
    fn hangs_straight_down_at_rest() {
        let params = JiggleParams {
            stiffness: 0.0,
            ..Default::default()
        };
        // Start pointing sideways (+x) so "settles downward" is a real change, not the start.
        let mut c = JiggleChain::new(Vec3::ZERO, Vec3::X, 5.0, 4, params);
        settle(&mut c, Vec3::ZERO, 2000);
        let p = c.positions();
        for i in 1..p.len() {
            let d = p[i] - p[i - 1];
            assert!(
                (d.length() - 5.0).abs() < 0.25,
                "segment {i} must hold its rest length (PBD ~1% stretch)"
            );
            assert!(
                d.z < -4.9,
                "segment {i} must hang below its parent (got dz {})",
                d.z
            );
            assert!(
                d.x.abs() < 0.2 && d.y.abs() < 0.2,
                "segment {i} must be on the gravity axis"
            );
        }
    }

    /// The anchor is always pinned exactly to the driver — the chain hangs FROM the bone.
    #[test]
    fn anchor_is_pinned_to_the_driver() {
        let mut c = JiggleChain::new(Vec3::ZERO, -Vec3::Z, 3.0, 5, JiggleParams::default());
        for k in 0..50 {
            let a = Vec3::new(k as f32, 0.0, 20.0);
            c.step(a, Quat::IDENTITY, 1.0 / 60.0);
            assert!(
                (c.positions()[0] - a).length() < 1e-4,
                "anchor must track the driver"
            );
        }
    }

    /// Move the driver sideways: the free end must LAG (secondary motion), not teleport with
    /// the anchor — then settle back under the new anchor. If it moved rigidly there'd be no
    /// jiggle at all.
    #[test]
    fn free_end_lags_then_settles() {
        let mut c = JiggleChain::new(Vec3::ZERO, -Vec3::Z, 4.0, 5, JiggleParams::default());
        settle(&mut c, Vec3::ZERO, 500);
        let tip_before = *c.positions().last().unwrap();
        // One big anchor jump.
        c.step(Vec3::new(30.0, 0.0, 0.0), Quat::IDENTITY, 1.0 / 60.0);
        let tip_after = *c.positions().last().unwrap();
        assert!(
            (tip_after.x - tip_before.x).abs() < 20.0,
            "the tip must LAG the 30cm anchor jump, not follow it rigidly (moved {})",
            tip_after.x - tip_before.x
        );
        // ...and eventually catch up under the new anchor (hangs below it).
        settle(&mut c, Vec3::new(30.0, 0.0, 0.0), 3000);
        assert!(
            (c.positions().last().unwrap().x - 30.0).abs() < 1.0,
            "the chain must settle under the new anchor position"
        );
    }

    /// Damping must bleed energy: after a perturbation the motion decays toward rest. Sample
    /// `early` in the first frames after the kick (the high-energy transient) — the chain
    /// settles within ~60 steps, so sampling later would read zero and make the ratio vacuous.
    #[test]
    fn motion_decays_with_damping() {
        let energy = |c: &JiggleChain| -> f32 {
            c.positions()
                .iter()
                .zip(&c.prev)
                .map(|(p, q)| (*p - *q).length())
                .sum()
        };
        let mut c = JiggleChain::new(Vec3::ZERO, -Vec3::Z, 4.0, 5, JiggleParams::default());
        settle(&mut c, Vec3::ZERO, 200);
        let kick = Vec3::new(25.0, 0.0, 0.0);
        for _ in 0..4 {
            c.step(kick, Quat::IDENTITY, 1.0 / 60.0); // kick + swing = the transient
        }
        let early = energy(&c);
        assert!(
            early > 1.0,
            "the kick must actually excite the chain (got {early})"
        );
        settle(&mut c, kick, 2000);
        let late = energy(&c);
        assert!(
            late < early * 0.05 && late < 0.1,
            "motion must decay to rest ({early} → {late})"
        );
    }

    /// Bit-for-bit reproducible — no rng, no wall-clock. Same driver path ⇒ same result.
    #[test]
    fn deterministic() {
        let run = || {
            let mut c = JiggleChain::new(Vec3::ZERO, -Vec3::Z, 3.0, 6, JiggleParams::default());
            for k in 0..300 {
                c.step(
                    Vec3::new((k as f32 * 0.1).sin() * 10.0, 0.0, 15.0),
                    Quat::IDENTITY,
                    1.0 / 60.0,
                );
            }
            c.positions().to_vec()
        };
        assert_eq!(run(), run(), "the solver must be deterministic");
    }

    /// A huge dt (frame hitch) must not explode the chain — dt is clamped.
    #[test]
    fn a_frame_hitch_does_not_explode() {
        let mut c = JiggleChain::new(Vec3::ZERO, -Vec3::Z, 4.0, 5, JiggleParams::default());
        c.step(Vec3::new(0.0, 0.0, 20.0), Quat::IDENTITY, 5.0); // a 5-second "hitch"
        assert!(
            c.positions()
                .iter()
                .all(|p| p.is_finite() && p.length() < 1e4),
            "a frame hitch must not launch the chain to infinity"
        );
    }
}
