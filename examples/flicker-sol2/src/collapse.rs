//! The gravitational **collapse** of the stage-2 disk into a planetary system.
//!
//! Runs on the *same* stage-2 disk the rings view shows (cast radius × clump density × the
//! Mass/Metallicity tonnage), not a parallel model. **The cheat:** the H/He is cast *outward*,
//! so a stable central star can't be simulated forming from it — instead we extract most of the
//! gas (`STAR_GAS_FRAC`) straight into a pinned central star (body 0). That dominant central mass
//! is what makes everything else stable.
//!
//! **The disk = the rest of the cloud** (the leftover gas + all the metals). Parcels are drawn
//! from each ring's angular-density CDF so they **cluster in the clumps** (the dots), and orbit
//! the star. Gas drag circularises them and, while the gas is present, migrates them inward so
//! they cross and **accrete** (merge on contact, conserving mass + composition) into planets:
//! leftover gas → gas giants, volatiles (C/N/O) → ice giants, metals → rocky worlds. What each
//! body becomes is read off the result — never selected.
//!
//! Because the star is a pinned dominant mass and the disk is light, the system is naturally
//! stable: the planets orbit in the star's well, nothing drifts, nothing is flung out.
//!
//! **Conserved to the float:** the star plus every live disk parcel equals the starting tonnage,
//! per element, every step — mass only moves and combines, never appears or vanishes.
//!
//! Units: AU, solar masses, years (so `G = 4π²`). `STAR_GAS_FRAC` / spin / drag are tuning
//! constants — the dynamics are meant to be watched and adjusted, not nailed on the first pass.

use std::f32::consts::TAU;

use flicker::render::Vec2;

use crate::cloud::CloudField;
use crate::mass::CloudMass;
use crate::model::{CastParams, Ejecta};

/// Gravitational constant in AU³ / (M☉ · yr²) — `4π²`.
const G: f32 = 39.478_418;
/// Plummer softening (AU): keeps gravity finite at tiny separations. Numerical hygiene,
/// not a physics clamp — merging removes close pairs before it matters.
const SOFTENING: f32 = 0.05;
/// Body radius from mass: `r = RADIUS_K · mass^(1/3)` (AU) — the contact distance for
/// merging and the drawn size. Tuned so accretion happens at sensible separations.
const RADIUS_K: f32 = 0.8;
/// Fraction of the cloud's gas (H/He) extracted straight into the central star — the "cheat".
/// The H/He is cast outward, so we can't stably simulate it collapsing inward; we steal most of
/// it and place it at the centre, and that dominant central mass is what makes the rest stable.
/// Must be < 1 so the leftover gas stays in the disk to build the gas giants. Lower → bigger
/// giants / a less dominant star.
const STAR_GAS_FRAC: f32 = 0.98;
/// Disk parcels orbit the star at this fraction of the circular speed (1.0 = a proper Keplerian
/// disk). Gas drag then circularises and accretes them into planets.
const DISK_SPIN: f32 = 1.0;
/// Drag rate (per year). Gas parcels get their motion **bled toward rest** (so they keep
/// falling to the centre); solid parcels get **circularised** toward their orbit (so they
/// settle as stable planets). Fades over `GAS_TAU` as the gas disperses, atop a small
/// perpetual floor. Real disk dissipation, not a clamp.
const DRAG: f32 = 0.6;
/// Solid-drag target (fraction of circular speed) for the **fading** part of the drag —
/// slightly sub-Keplerian, so while the gas is present the solids migrate inward, cross, and
/// accrete into a few planets. The perpetual floor then targets exactly circular, so the
/// settled planets stay put instead of draining into the star.
const DRAG_TARGET_FRAC: f32 = 0.95;
/// Gas-dispersal timescale (years): the drag fades as `exp(-t / GAS_TAU)`, mirroring a real
/// disk's gas blowing off after the collapse — strong while it settles, then gone.
const GAS_TAU: f32 = 35.0;
/// A small perpetual drag floor (per year) that never fades — keeps the settled solids gently
/// circularised so the system stays bound long after the gas is gone, instead of a bare
/// many-body N-body going chaotic and ejecting everything.
const DRAG_FLOOR: f32 = 0.02;
/// Largest integration step (years) — a frame's sim-time is split into substeps no longer
/// than this so fast inner orbits stay stable.
const MAX_DT: f32 = 0.02;

/// Body-type mass thresholds (solar masses) — read off what a body became, for display.
const STAR_MASS: f32 = 0.08; // ~hydrogen-fusion limit (≈ 80 Jupiter masses)
const GIANT_MASS: f32 = 9.0e-5; // ≈ 30 Earth masses
const PLANET_MASS: f32 = 1.5e-7; // ≈ 0.05 Earth masses

/// What a body turned out to be — read off its mass + composition for display only. The
/// simulation never branches on this; the type is emergent, never selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyType {
    Star,
    GasGiant,
    IceGiant,
    RockyPlanet,
    IcyBody,
    Asteroid,
}

impl BodyType {
    /// Display tint for this type.
    pub fn color(self) -> [f32; 3] {
        match self {
            BodyType::Star => [1.00, 0.90, 0.55],
            BodyType::GasGiant => [0.85, 0.72, 0.50],
            BodyType::IceGiant => [0.50, 0.72, 0.92],
            BodyType::RockyPlanet => [0.72, 0.52, 0.38],
            BodyType::IcyBody => [0.74, 0.86, 0.92],
            BodyType::Asteroid => [0.58, 0.55, 0.52],
        }
    }

    /// Short label.
    pub fn label(self) -> &'static str {
        match self {
            BodyType::Star => "star",
            BodyType::GasGiant => "gas giant",
            BodyType::IceGiant => "ice giant",
            BodyType::RockyPlanet => "rocky",
            BodyType::IcyBody => "icy",
            BodyType::Asteroid => "asteroid",
        }
    }
}

/// The collapsing cloud: a set of motes in struct-of-arrays form. A merged-away mote is
/// flagged `!alive` rather than removed, so indices stay stable within a step.
pub struct Sim {
    /// Number of elements (a `comp` row is this wide).
    pub n_el: usize,
    pub pos: Vec<Vec2>,
    pub vel: Vec<Vec2>,
    /// Total mass per mote (cached sum of its `comp` row), in solar masses.
    pub mass: Vec<f32>,
    /// Per-element mass, flat: mote `i`, element `e` is `comp[i * n_el + e]`.
    pub comp: Vec<f32>,
    pub alive: Vec<bool>,
    /// Per-element atomic numbers (length `n_el`), for composition-based body typing.
    pub el_numbers: Vec<u8>,
    /// Elapsed simulated time, years.
    pub time: f32,
    init_total: f32,
}

impl Sim {
    /// Ignite the system. **The cheat:** extract most of the gas (H/He) straight into a central
    /// star (body 0) — the H/He is cast outward, so a stable star can't be simulated forming from
    /// it, and that dominant central mass is what makes the rest stable. **Everything else** —
    /// the leftover gas and all the metals — is seeded into the disk from the clump density (so it
    /// clusters in the disturbances, not on a grid), orbiting the star, and collapses into
    /// planets: leftover gas → gas giants, volatiles → ice giants, metals → rocky worlds.
    /// Conserved: star + disk == the whole cloud.
    pub fn from_cloud(
        ej: &Ejecta,
        cast: &CastParams,
        cloud: &CloudField,
        cm: &CloudMass,
        per_el: usize,
    ) -> Self {
        let n_el = ej.elements.len();
        let mut pos = Vec::new();
        let mut vel = Vec::new();
        let mut mass = Vec::new();
        let mut comp = Vec::new();

        // Body 0 = the star: STAR_GAS_FRAC of the gas (H/He), placed at the centre, at rest.
        let mut star_comp = vec![0.0f32; n_el];
        let mut star_mass = 0.0f32;
        for (i, e) in ej.elements.iter().enumerate() {
            if e.number <= 2 {
                let take = cm.tonnage[i] * STAR_GAS_FRAC;
                star_comp[i] = take;
                star_mass += take;
            }
        }
        pos.push(Vec2::ZERO);
        vel.push(Vec2::ZERO);
        mass.push(star_mass);
        comp.extend_from_slice(&star_comp);

        // The disk = the rest of the cloud (leftover gas + all metals), drawn from each ring's
        // angular-density CDF so parcels CLUSTER in the clumps (the dots), orbiting the star.
        // Each ring's parcels share its disk tonnage equally → conserved exactly.
        const NB: usize = 256;
        let seed = cloud.seed;
        for (i, e) in ej.elements.iter().enumerate() {
            let tonnage = if e.number <= 2 {
                cm.tonnage[i] * (1.0 - STAR_GAS_FRAC)
            } else {
                cm.tonnage[i]
            };
            if tonnage <= 0.0 {
                continue;
            }
            let au = cast.distance_au(e.atomic_mass);
            let mut cdf = [0.0f32; NB];
            let mut acc = 0.0;
            for (b, slot) in cdf.iter_mut().enumerate() {
                acc += cloud.density(i, b as f32 / NB as f32 * TAU, 0.0).max(0.0);
                *slot = acc;
            }
            let cdf_total = acc.max(1e-6);
            let m = tonnage / per_el as f32;
            for k in 0..per_el {
                let u = rand01(hash3(seed, i as u32, k as u32)) * cdf_total;
                let b = cdf.partition_point(|&c| c < u).min(NB - 1);
                let th = (b as f32 + 0.5) / NB as f32 * TAU;
                let r = (au * (1.0 + cloud.wobble(i, th, 0.0))).max(0.1);
                let p = Vec2::new(r * th.cos(), r * th.sin());
                // Orbit the star (it dominates the gravity): circular velocity, CCW.
                let v_circ = (G * star_mass.max(1e-9) / r).sqrt();
                let t_hat = Vec2::new(-p.y / r, p.x / r);
                pos.push(p);
                vel.push(t_hat * (DISK_SPIN * v_circ));
                mass.push(m);
                let base = comp.len();
                comp.extend(std::iter::repeat_n(0.0, n_el));
                comp[base + i] = m;
            }
        }

        let n = mass.len();
        let init_total = mass.iter().sum();
        let el_numbers = ej.elements.iter().map(|e| e.number).collect();
        Self {
            n_el,
            pos,
            vel,
            mass,
            comp,
            alive: vec![true; n],
            el_numbers,
            time: 0.0,
            init_total,
        }
    }

    /// Read off what body `i` became, from its mass and composition (gas = H/He,
    /// ice = C/N/O volatiles, rock = everything heavier). Display only — emergent, never
    /// a thing the simulation acts on.
    pub fn classify(&self, i: usize) -> BodyType {
        let m = self.mass[i];
        if m >= STAR_MASS {
            return BodyType::Star;
        }
        let row = &self.comp[i * self.n_el..(i + 1) * self.n_el];
        let (mut gas, mut ice, mut rock) = (0.0f32, 0.0f32, 0.0f32);
        for (e, &w) in row.iter().enumerate() {
            match self.el_numbers[e] {
                1 | 2 => gas += w,
                6..=8 => ice += w,
                _ => rock += w,
            }
        }
        if m > GIANT_MASS {
            if gas >= ice && gas >= rock {
                BodyType::GasGiant
            } else if ice >= rock {
                BodyType::IceGiant
            } else {
                BodyType::RockyPlanet
            }
        } else if m > PLANET_MASS {
            if rock >= gas + ice {
                BodyType::RockyPlanet
            } else {
                BodyType::IceGiant
            }
        } else if rock >= gas + ice {
            BodyType::Asteroid
        } else {
            BodyType::IcyBody
        }
    }

    /// Advance the collapse by `dt` years: gravity (direct sum, softened) integrated with
    /// substeps, then a merge pass. Mass is conserved across both.
    pub fn step(&mut self, dt: f32) {
        let subs = (dt / MAX_DT).ceil().max(1.0) as usize;
        let h = dt / subs as f32;
        for _ in 0..subs {
            self.integrate(h);
        }
        self.dissipate(dt);
        self.merge();
        self.time += dt;
    }

    /// Disk dissipation. Each disk parcel is damped toward its circular orbit around the star —
    /// circularised so orbits stay bound, and (while the gas is present) slightly sub-circular so
    /// parcels migrate inward, cross, and accrete into a few planets. A perpetual floor targets
    /// exactly circular so the settled planets stay put. The star (body 0) is pinned and skipped.
    fn dissipate(&mut self, dt: f32) {
        let fade = (-self.time / GAS_TAU).exp();
        let k_active = (DRAG * fade * dt).clamp(0.0, 1.0);
        let k_floor = (DRAG_FLOOR * dt).clamp(0.0, 1.0);
        let n = self.mass.len();
        for i in 1..n {
            if !self.alive[i] {
                continue;
            }
            // Primary = the heavier body pulling `i` hardest, by the real G·m/r² force (not a
            // Hill range). The star (body 0) always qualifies; a planet wins only when it
            // genuinely out-pulls the star — and then `i` is that planet's moon. Capture falls
            // out of the forces; we never reach in and grab.
            let pi = self.pos[i];
            let prim = self.primary_of(i, pi);
            let rel = pi - self.pos[prim];
            let r = rel.length().max(1e-3);
            let v_circ = (G * self.mass[prim] / r).sqrt();
            let t_hat = Vec2::new(-rel.y / r, rel.x / r);
            let v_prim = self.vel[prim];
            // Circularise `i` around its primary's frame: fading drag → slightly sub-circular
            // (migration + accretion), perpetual floor → exactly circular (stable orbits).
            let mut v = self.vel[i];
            v += (v_prim + t_hat * (DRAG_TARGET_FRAC * v_circ) - v) * k_active;
            v += (v_prim + t_hat * v_circ - v) * k_floor;
            self.vel[i] = v;
        }
    }

    /// The body that gravitationally dominates `i` at position `p` — the heavier body exerting
    /// the largest `G·m/r²` on it. The star (body 0) is the default; a planet supersedes it only
    /// where it truly out-pulls the star, which is exactly the (real, computed) condition for `i`
    /// to be that planet's satellite. No Hill formula.
    fn primary_of(&self, i: usize, p: Vec2) -> usize {
        let mut prim = 0usize;
        let mut best = self.mass[0] / (self.pos[0] - p).length_squared().max(1e-6);
        for j in 1..self.mass.len() {
            if j == i || !self.alive[j] || self.mass[j] <= self.mass[i] {
                continue;
            }
            let f = self.mass[j] / (self.pos[j] - p).length_squared().max(1e-6);
            if f > best {
                best = f;
                prim = j;
            }
        }
        prim
    }

    /// One symplectic-Euler step: accelerate every live mote by every other, then kick + drift.
    fn integrate(&mut self, h: f32) {
        let n = self.mass.len();
        let soft2 = SOFTENING * SOFTENING;
        let mut acc = vec![Vec2::ZERO; n];
        for (i, a_i) in acc.iter_mut().enumerate() {
            // Body 0 is the pinned star — it exerts gravity (below) but does not move.
            if i == 0 || !self.alive[i] {
                continue;
            }
            let pi = self.pos[i];
            let mut a = Vec2::ZERO;
            for j in 0..n {
                if i == j || !self.alive[j] {
                    continue;
                }
                let d = self.pos[j] - pi;
                let r2 = d.length_squared() + soft2;
                a += d * (G * self.mass[j] / (r2 * r2.sqrt()));
            }
            *a_i = a;
        }
        for (i, &a) in acc.iter().enumerate() {
            if i != 0 && self.alive[i] {
                self.vel[i] += a * h;
                self.pos[i] += self.vel[i] * h;
            }
        }
    }

    /// Merge any two live bodies whose discs touch (distance < sum of radii). The heavier keeps
    /// its index; mass, momentum and composition combine (inelastic, conserved). The star (body
    /// 0) is the heaviest, so it absorbs anything that falls into it — but stays pinned at centre.
    fn merge(&mut self) {
        let n = self.mass.len();
        for i in 0..n {
            if !self.alive[i] {
                continue;
            }
            for j in (i + 1)..n {
                if !self.alive[j] {
                    continue;
                }
                let reach = RADIUS_K * (self.mass[i].cbrt() + self.mass[j].cbrt());
                if (self.pos[i] - self.pos[j]).length_squared() >= reach * reach {
                    continue;
                }
                let (a, b) = if self.mass[i] >= self.mass[j] { (i, j) } else { (j, i) };
                let (ma, mb) = (self.mass[a], self.mass[b]);
                let mt = ma + mb;
                if a != 0 {
                    // The star (body 0) stays pinned at the centre; any other survivor moves to
                    // the mass-weighted position and velocity.
                    self.pos[a] = (self.pos[a] * ma + self.pos[b] * mb) / mt;
                    self.vel[a] = (self.vel[a] * ma + self.vel[b] * mb) / mt;
                }
                for e in 0..self.n_el {
                    self.comp[a * self.n_el + e] += self.comp[b * self.n_el + e];
                }
                self.mass[a] = mt;
                self.alive[b] = false;
                if a != i {
                    // i was absorbed into j; nothing more to merge onto i.
                    break;
                }
            }
        }
    }

    /// The drawn / merge radius of mote `i` (AU).
    pub fn radius_au(&self, i: usize) -> f32 {
        RADIUS_K * self.mass[i].cbrt()
    }

    /// Live mote count (bodies + not-yet-merged parcels).
    pub fn live_count(&self) -> usize {
        self.alive.iter().filter(|&&a| a).count()
    }

    /// The heaviest live body's mass (M☉) — the emergent star, once collapse runs.
    pub fn largest_mass(&self) -> f32 {
        self.mass
            .iter()
            .zip(&self.alive)
            .filter(|(_, &a)| a)
            .map(|(&m, _)| m)
            .fold(0.0, f32::max)
    }

    /// Total live mass (M☉) — invariant; equals the starting tonnage.
    pub fn total_mass(&self) -> f32 {
        self.mass.iter().zip(&self.alive).filter(|(_, &a)| a).map(|(&m, _)| m).sum()
    }

    /// Starting total mass, for a conservation check.
    pub fn init_total(&self) -> f32 {
        self.init_total
    }
}

/// Small integer hash (xorshift-multiply) → reproducible per-parcel draws without an RNG crate.
fn hash3(a: u32, b: u32, c: u32) -> u32 {
    let mut x = a
        .wrapping_mul(0x9E37_79B1)
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
    use crate::mass::{CloudMass, MassParams};
    use crate::model::{load_tables, Ejecta};

    fn sim() -> Sim {
        let ej = Ejecta::from_tables(&load_tables());
        let cast = CastParams::default();
        let cloud = CloudField::new(ej.elements.len(), 0xC10D_5EED, 0.6);
        let cm = CloudMass::derive(&ej, &MassParams::default());
        Sim::from_cloud(&ej, &cast, &cloud, &cm, 12)
    }

    #[test]
    fn collapse_conserves_total_mass() {
        let mut s = sim();
        let before = s.init_total();
        for _ in 0..300 {
            s.step(0.02);
        }
        let after = s.total_mass();
        assert!((after - before).abs() < 1e-3 * before, "mass conserved: {before} -> {after}");
    }

    #[test]
    fn motes_merge_into_fewer_bodies() {
        let mut s = sim();
        let start = s.live_count();
        for _ in 0..400 {
            s.step(0.02);
        }
        assert!(s.live_count() < start, "collapse coalesces motes ({start} -> {})", s.live_count());
    }

    #[test]
    fn a_dominant_central_star_emerges() {
        let mut s = sim();
        // ~240 simulated years — the collapse (slowed by gas drag) settles into a single
        // dominant central star plus minor bodies.
        for _ in 0..2000 {
            s.step(0.02);
        }
        // The star should hold most of the cloud (Sol itself is ~99.9%).
        assert!(
            s.largest_mass() > 0.8 * s.init_total(),
            "a dominant central star grows: largest {} of {}",
            s.largest_mass(),
            s.init_total()
        );
    }

    #[test]
    fn the_system_stays_bound() {
        let mut s = sim();
        for _ in 0..2000 {
            s.step(0.02);
        }
        // Gas drag should keep essentially all the mass bound near the star — no wholesale
        // ejection into deep space.
        let bound: f32 = (0..s.mass.len())
            .filter(|&i| s.alive[i] && s.pos[i].length() < 80.0)
            .map(|i| s.mass[i])
            .sum();
        assert!(bound > 0.95 * s.total_mass(), "system stays bound: {bound} of {}", s.total_mass());
    }

    #[test]
    fn velocities_and_positions_stay_finite() {
        let mut s = sim();
        for _ in 0..500 {
            s.step(0.02);
        }
        for (i, &a) in s.alive.iter().enumerate() {
            if a {
                assert!(s.pos[i].is_finite() && s.vel[i].is_finite(), "no blow-up at mote {i}");
            }
        }
    }
}
