//! Azimuthal **structure** of the ejecta cloud — the "wobble".
//!
//! Real supernova ejecta isn't a smooth shell: distinct nucleosynthesis zones are
//! blown out as plumes, Rayleigh–Taylor fingers clump it, and the whole thing is
//! anisotropic. So each element's ring carries an **angular density field** (over- and
//! under-dense arcs, its own clumps) plus a small **radial meander** (it isn't a
//! perfect circle), and the field **rotates** — with differential, Keplerian-style
//! shear, inner rings turning faster than outer ones.
//!
//! Both fields are a few seeded sinusoidal harmonics per element (deterministic from a
//! seed, rerollable), so the structure is reproducible and cheap. This is the precursor
//! layer for body detection: the overdensities are where mass will later be found to
//! collect into planetesimals.

use std::f32::consts::TAU;

/// Density harmonics per element (superpose into a few clump arcs).
const K_DENS: usize = 5;
/// Radial-meander harmonics per element (low order — gentle non-circularity).
const K_WOB: usize = 3;
/// Peak fractional radial meander at full strength.
const WOBBLE_AMP: f32 = 0.035;
/// Angular speed (rad/s) of a ring at the cloud's anchor radius (~5°/s) — a gentle drift;
/// per-ring differential shear scales it from here.
const OMEGA0: f32 = 0.087;
/// Shear exponent: `ω ∝ r^-SHEAR_EXP`. Deliberately shallower than strict Keplerian (1.5):
/// at 1.5 the inner rings spin so fast they must be clamped, which flattens ω across a
/// band's radial width and kills the *intra-band* winding (a clump can't shear into a
/// tail). A shallower slope keeps ω varying — and bounded — right across each band, so a
/// clump stretches into a trailing spiral as the inner edge laps the outer.
const SHEAR_EXP: f32 = 0.65;
/// Clamp only at the extremes (safety against tiny/huge radii); with `SHEAR_EXP` the
/// visible range sits well inside this, so the clamp essentially never bites.
const OMEGA_RATIO_MIN: f32 = 0.3;
const OMEGA_RATIO_MAX: f32 = 5.0;

#[derive(Clone, Copy)]
struct Harm {
    m: f32,
    phase: f32,
    amp: f32,
}

/// The cloud's per-element angular structure: density clumps + radial meander, rotating
/// under differential shear. `strength` scales how lumpy it is (0 → smooth rings).
pub struct CloudField {
    pub strength: f32,
    pub seed: u32,
    dens: Vec<[Harm; K_DENS]>,
    dens_norm: Vec<f32>,
    wob: Vec<[Harm; K_WOB]>,
    wob_norm: Vec<f32>,
}

impl CloudField {
    pub fn new(n: usize, seed: u32, strength: f32) -> Self {
        let mut f = Self {
            strength,
            seed,
            dens: Vec::new(),
            dens_norm: Vec::new(),
            wob: Vec::new(),
            wob_norm: Vec::new(),
        };
        f.rebuild(n);
        f
    }

    /// Roll a fresh clump pattern (same element count).
    pub fn reseed(&mut self, seed: u32) {
        self.seed = seed;
        let n = self.dens.len();
        self.rebuild(n);
    }

    fn rebuild(&mut self, n: usize) {
        self.dens = Vec::with_capacity(n);
        self.dens_norm = Vec::with_capacity(n);
        self.wob = Vec::with_capacity(n);
        self.wob_norm = Vec::with_capacity(n);
        for i in 0..n {
            let mut d = [Harm {
                m: 2.0,
                phase: 0.0,
                amp: 0.0,
            }; K_DENS];
            let mut dn = 0.0;
            for (k, slot) in d.iter_mut().enumerate() {
                let m = 2.0 + (hash3(self.seed, i as u32, (k * 3) as u32) % 6) as f32; // 2..7
                let phase = rand01(hash3(self.seed, i as u32, (k * 3 + 1) as u32)) * TAU;
                let amp = (0.45 + 0.55 * rand01(hash3(self.seed, i as u32, (k * 3 + 2) as u32)))
                    / (k as f32 + 1.0).powf(0.7);
                *slot = Harm { m, phase, amp };
                dn += amp;
            }
            self.dens.push(d);
            self.dens_norm.push(dn.max(1e-3));

            let ws = self.seed ^ 0x5151_5151;
            let mut w = [Harm {
                m: 1.0,
                phase: 0.0,
                amp: 0.0,
            }; K_WOB];
            let mut wn = 0.0;
            for (k, slot) in w.iter_mut().enumerate() {
                let m = 1.0 + (hash3(ws, i as u32, (k * 3) as u32) % 4) as f32; // 1..4
                let phase = rand01(hash3(ws, i as u32, (k * 3 + 1) as u32)) * TAU;
                let amp = (0.5 + 0.5 * rand01(hash3(ws, i as u32, (k * 3 + 2) as u32)))
                    / (k as f32 + 1.0);
                *slot = Harm { m, phase, amp };
                wn += amp;
            }
            self.wob.push(w);
            self.wob_norm.push(wn.max(1e-3));
        }
    }

    /// Angular speed (rad/s) of a ring at cast radius `r_au`, sheared around the cloud's
    /// `anchor_au` (Keplerian `∝ r^-3/2`, clamped). Anchoring to the **live radial span**
    /// — not a fixed 1 AU — keeps a visible inner-faster / outer-slower spread at any
    /// explosion size; otherwise, with every ring beyond 1 AU, all `ω` hit the floor and
    /// the field crept uniformly.
    pub fn omega(&self, r_au: f32, anchor_au: f32) -> f32 {
        let ratio = (anchor_au.max(0.01) / r_au.max(0.05))
            .powf(SHEAR_EXP)
            .clamp(OMEGA_RATIO_MIN, OMEGA_RATIO_MAX);
        OMEGA0 * ratio
    }

    /// Azimuthal density multiplier at `theta` (the pattern rotated by `rot` radians):
    /// ~1 on average, >1 in clumps, <1 in gaps. `strength = 0` → flat 1.
    pub fn density(&self, i: usize, theta: f32, rot: f32) -> f32 {
        if i >= self.dens.len() {
            return 1.0;
        }
        let mut p = 0.0;
        for h in &self.dens[i] {
            p += h.amp * (h.m * (theta - rot) - h.phase).cos();
        }
        (1.0 + self.strength * p / self.dens_norm[i]).max(0.05)
    }

    /// Fractional radial meander at `theta` (rotated by `rot`) — the ring isn't circular.
    /// Roughly `[-WOBBLE_AMP, WOBBLE_AMP]`, scaled by strength.
    pub fn wobble(&self, i: usize, theta: f32, rot: f32) -> f32 {
        if i >= self.wob.len() {
            return 0.0;
        }
        let mut p = 0.0;
        for h in &self.wob[i] {
            p += h.amp * (h.m * (theta - rot) - h.phase).cos();
        }
        WOBBLE_AMP * self.strength.min(1.0) * p / self.wob_norm[i]
    }
}

/// A small integer hash (xorshift-multiply mix) → reproducible per-element parameters
/// without pulling in an RNG crate.
fn hash3(a: u32, b: u32, c: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B1)
        ^ b.wrapping_mul(0x85EB_CA77)
        ^ c.wrapping_mul(0xC2B2_AE3D).wrapping_add(0x1656_67B1);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2545_F491);
    x ^= x >> 13;
    x = x.wrapping_mul(0x9E37_79B1);
    x ^= x >> 16;
    x
}

/// Hash bits → `f32` in `[0, 1)`.
fn rand01(h: u32) -> f32 {
    (h >> 8) as f32 / 0xFF_FFFF as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_flat() {
        let f = CloudField::new(27, 0xABCD, 0.0);
        for &th in &[0.0, 1.0, 2.5, 5.0] {
            assert!((f.density(3, th, 0.0) - 1.0).abs() < 1e-6);
            assert!(f.wobble(3, th, 0.0).abs() < 1e-6);
        }
    }

    #[test]
    fn density_varies_around_the_ring_when_lumpy() {
        let f = CloudField::new(27, 0xABCD, 1.0);
        let a = f.density(5, 0.0, 0.0);
        let b = f.density(5, 2.0, 0.0);
        let c = f.density(5, 4.0, 0.0);
        assert!(a > 0.0 && b > 0.0 && c > 0.0);
        assert!((a - b).abs() + (b - c).abs() > 1e-3, "ring is not uniform");
    }

    #[test]
    fn inner_rings_shear_faster_than_outer() {
        let f = CloudField::new(27, 1, 0.6);
        let anchor = 5.0;
        assert!(
            f.omega(0.4, anchor) > f.omega(50.0, anchor),
            "Keplerian shear: inner faster"
        );
    }

    #[test]
    fn shear_survives_across_visible_rings() {
        // Two rings either side of the anchor must differ enough to read as shear — the
        // clamp must not flatten them to one rate (the bug that hid the shear).
        let f = CloudField::new(27, 1, 0.6);
        let anchor = 11.0;
        let inner = f.omega(6.0, anchor);
        let outer = f.omega(20.0, anchor);
        assert!(
            inner > 1.5 * outer,
            "adjacent visible rings shear (got {inner} vs {outer})"
        );
    }
}
