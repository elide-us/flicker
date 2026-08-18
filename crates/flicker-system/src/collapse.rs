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

use glam::Vec2;

use crate::cloud::CloudField;
use crate::config::Tuning;
use crate::mass::CloudMass;
use crate::model::{CastParams, Ejecta};

// All the tunable levers (softening, drag, densities, Hill/Roche fractions, body-type and
// playability thresholds, …) now live in `Tuning` (see `config.rs`) and are read off `Sim::tuning`
// — so the whole simulation is driven by `SystemConfig`. Only true physical/numerical constants
// remain here.

/// Gravitational constant in AU³ / (M☉ · yr²) — `4π²`.
const G: f32 = 39.478_418;
/// g/cc → M☉/AU³ (1 M☉ = 1.989e33 g, 1 AU = 1.496e13 cm), so the physical radius lands in AU.
const GCC_TO_MSUN_AU3: f32 = 1.683e6;
/// Largest integration step (years) — a frame's sim-time is split into substeps no longer
/// than this so fast inner orbits stay stable.
const MAX_DT: f32 = 0.02;

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
    /// Display tint for this type — six distinct, saturated hues so types read apart at a glance
    /// (gold / amber / blue / red / cyan / stone), each still evocative of the material.
    pub fn color(self) -> [f32; 3] {
        match self {
            BodyType::Star => [1.00, 0.82, 0.18],        // gold
            BodyType::GasGiant => [0.96, 0.56, 0.16],    // amber-orange (Jupiter)
            BodyType::IceGiant => [0.26, 0.50, 1.00],    // deep blue (Neptune)
            BodyType::RockyPlanet => [0.90, 0.30, 0.24], // red (Mars/rust)
            BodyType::IcyBody => [0.40, 0.88, 0.92],     // bright cyan
            BodyType::Asteroid => [0.64, 0.62, 0.52],    // neutral stone
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
    /// How much of body `i`'s mass is actually an orbiting **ring** (tidally-shredded satellite
    /// debris), not solid body. A subset of `mass[i]` (so total mass stays conserved); the
    /// renderer draws this fraction as a disc around the body. 0 for bodies with no ring.
    pub ring_mass: Vec<f32>,
    /// Per-element atomic numbers (length `n_el`), for composition-based body typing.
    pub el_numbers: Vec<u8>,
    /// Elapsed simulated time, years.
    pub time: f32,
    /// The physics / typing / playability levers this run uses (from `SystemConfig`).
    pub tuning: Tuning,
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
        tuning: Tuning,
    ) -> Self {
        let n_el = ej.elements.len();
        let mut pos = Vec::new();
        let mut vel = Vec::new();
        let mut mass = Vec::new();
        let mut comp = Vec::new();

        // Body 0 = the star: `star_gas_frac` of the gas (H/He), placed at the centre, at rest.
        let mut star_comp = vec![0.0f32; n_el];
        let mut star_mass = 0.0f32;
        for (i, e) in ej.elements.iter().enumerate() {
            if e.number <= 2 {
                let take = cm.tonnage[i] * tuning.star_gas_frac;
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
                cm.tonnage[i] * (1.0 - tuning.star_gas_frac)
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
                vel.push(t_hat * (tuning.disk_spin * v_circ));
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
            ring_mass: vec![0.0; n],
            el_numbers,
            time: 0.0,
            tuning,
            init_total,
        }
    }

    /// Read off what body `i` became, from its mass and composition (gas = H/He,
    /// ice = C/N/O volatiles, rock = everything heavier). Display only — emergent, never
    /// a thing the simulation acts on.
    pub fn classify(&self, i: usize) -> BodyType {
        let m = self.mass[i];
        if m >= self.tuning.star_mass {
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
        if m > self.tuning.giant_mass {
            if gas >= ice && gas >= rock {
                BodyType::GasGiant
            } else if ice >= rock {
                BodyType::IceGiant
            } else {
                BodyType::RockyPlanet
            }
        } else if m > self.tuning.planet_mass {
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

    /// Whether body `i` is currently a PLAYABLE world — a narrow physical gate, re-evaluated every
    /// frame so a body can gain or lose it as it migrates / grows / its star changes (emergent,
    /// never selected): a **rocky** body (solid surface), in the star's **habitable zone**
    /// (liquid-water *temperature* — distance scales with √luminosity, `L ≈ M★^3.5` on the main
    /// sequence), and massive enough to hold ~1 atm of *pressure* but not so massive it runs away
    /// to a giant. Temperature + pressure is the user's stated criterion; composition is NOT a
    /// gate (terrestrials form dry and get water delivered later) — but the chosen world's exact
    /// composition (`comp[i]`) is what feeds the next phase.
    pub fn is_playable(&self, i: usize) -> bool {
        if i == 0 || !self.alive[i] {
            return false;
        }
        let m = self.mass[i];
        if !(self.tuning.playable_mass_min..=self.tuning.playable_mass_max).contains(&m) {
            return false;
        }
        if self.classify(i) != BodyType::RockyPlanet {
            return false;
        }
        // Habitable zone: equilibrium temperature in the liquid-water band ⇒ orbital distance
        // scales with √(stellar luminosity).
        let sqrt_l = self.mass[0].max(1e-6).powf(3.5).sqrt();
        let r = self.pos[i].length();
        (self.tuning.hz_inner_frac * sqrt_l..=self.tuning.hz_outer_frac * sqrt_l).contains(&r)
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
        let fade = (-self.time / self.tuning.gas_tau).exp();
        let k_active = (self.tuning.drag * fade * dt).clamp(0.0, 1.0);
        let k_floor = (self.tuning.drag_floor * dt).clamp(0.0, 1.0);
        let soft2 = self.tuning.softening * self.tuning.softening;
        let n = self.mass.len();
        for i in 1..n {
            if !self.alive[i] {
                continue;
            }
            // The body `i` orbits: its force-dominant attractor (honest capture), or a planet it
            // remains a bound satellite of within the Hill radius (retention through the host's
            // inward migration). Capture falls out of the forces; we never reach in and grab.
            let pi = self.pos[i];
            let prim = self.host_of(i, pi);
            let rel = pi - self.pos[prim];
            let r = rel.length().max(1e-3);
            // Circular speed in the *softened* well the integrator actually uses, so the drag
            // target is a true fixed point. This only bites for tight orbits (r ~ SOFTENING) —
            // i.e. moons, where the bare √(Gm/r) would over-spin them onto a decaying orbit; for
            // the wide disk (r ≫ SOFTENING) it is identical to Keplerian.
            let v_circ = (G * self.mass[prim] * r * r / (r * r + soft2).powf(1.5)).sqrt();
            let t_hat = Vec2::new(-rel.y / r, rel.x / r);
            let v_prim = self.vel[prim];
            // Around the STAR, the fading drag is slightly sub-circular so disk solids migrate
            // in, cross, and accrete into planets. Around a PLANET (a satellite), it targets
            // exactly circular — circularise the captured moon, never feed it inward to its
            // death. The perpetual floor always targets circular (stable settled orbits).
            let active_frac = if prim == 0 {
                self.tuning.drag_target_frac
            } else {
                1.0
            };
            let mut v = self.vel[i];
            v += (v_prim + t_hat * (active_frac * v_circ) - v) * k_active;
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

    /// The body `i` orbits: its force-dominant attractor (`primary_of` — the honest capture
    /// test), or, when the star wins on raw force, the planet `i` is nonetheless a *bound*
    /// satellite of and sitting within `HILL_FRAC` of its Hill radius. The second clause RETAINS
    /// a moon through its host's inward migration (when the closing star would otherwise strip it
    /// on instantaneous force alone) — retention of an existing satellite, never a grab.
    fn host_of(&self, i: usize, p: Vec2) -> usize {
        let prim = self.primary_of(i, p);
        if prim != 0 {
            return prim;
        }
        let star_m = self.mass[0].max(1e-30);
        let vi = self.vel[i];
        let (mut best, mut best_sep) = (0usize, f32::MAX);
        for j in 1..self.mass.len() {
            if j == i || !self.alive[j] || self.mass[j] <= self.mass[i] {
                continue;
            }
            let sep = (p - self.pos[j]).length();
            let r_hill = self.pos[j].length() * (self.mass[j] / (3.0 * star_m)).cbrt();
            if sep >= self.tuning.hill_frac * r_hill || sep >= best_sep {
                continue;
            }
            // Bound to j? (negative specific relative orbital energy.)
            let mu = G * (self.mass[j] + self.mass[i]);
            let eps = 0.5 * (vi - self.vel[j]).length_squared() - mu / sep.max(1e-9);
            if eps < 0.0 {
                best = j;
                best_sep = sep;
            }
        }
        best
    }

    /// The body `i` currently orbits: `0` (the star) for a planet, or the planet index it is a
    /// captured moon of. Public view of `host_of` (the renderer uses it to tell moons apart).
    pub fn orbit_host(&self, i: usize) -> usize {
        self.host_of(i, self.pos[i])
    }

    /// Bulk density of body `i` (M☉/AU³), volume-additive over its gas/ice/rock composition
    /// (`1/ρ = Σ fₑ/ρₑ`). Derived from `comp` — no per-body density input. Drives the physical
    /// collision radius and the Roche radius (a gas giant puffy, a rocky body dense).
    fn density(&self, i: usize) -> f32 {
        let row = &self.comp[i * self.n_el..(i + 1) * self.n_el];
        let m = self.mass[i].max(1e-30);
        let mut inv_rho = 0.0f32; // Σ fₑ/ρₑ in (g/cc)⁻¹
        for (e, &w) in row.iter().enumerate() {
            let rho_e = match self.el_numbers[e] {
                1 | 2 => self.tuning.rho_gas_gcc,
                6..=8 => self.tuning.rho_ice_gcc,
                _ => self.tuning.rho_rock_gcc,
            };
            inv_rho += (w / m) / rho_e;
        }
        let rho_gcc = if inv_rho > 0.0 {
            1.0 / inv_rho
        } else {
            self.tuning.rho_rock_gcc
        };
        rho_gcc * GCC_TO_MSUN_AU3
    }

    /// Physical (collision) radius of body `i` in AU: `R = (3m / 4πρ)^(1/3)`, with ρ from its
    /// composition. **Microscopic next to the accretion reach** (`radius_au`) — and that gap is
    /// exactly what leaves room for a satellite to orbit instead of being swallowed.
    fn phys_radius(&self, i: usize) -> f32 {
        (3.0 * self.mass[i] / (4.0 * std::f32::consts::PI * self.density(i))).cbrt()
    }

    /// Tidal-disruption ("Roche") radius (AU) of host `a` for satellite `b`. The rigid-body Roche
    /// limit is `R_a·(2·ρ_a/ρ_b)^(1/3)`; we keep that **density ratio** (the emergent physics) but
    /// scale it to the sim's resolvable accretion radius (`radius_au`, not the true `phys_radius`,
    /// which is sub-softening) via `TIDAL_FRAC`. A satellite whose pericenter dips inside this is
    /// torn into a ring instead of holding as a moon: a low-density icy body shreds far out (wide
    /// ring), a dense rock barely shreds (just stays a close moon or merges) — so ice giants ring
    /// and captured rocks tend to survive or fall in.
    fn roche_radius(&self, a: usize, b: usize) -> f32 {
        self.radius_au(a)
            * self.tuning.tidal_frac
            * (2.0 * self.density(a) / self.density(b).max(1e-30)).cbrt()
    }

    /// Pericenter and apocenter (AU) of body `b`'s orbit about body `a`, from their real relative
    /// position and velocity (the conic from specific energy + angular momentum). Apocenter is
    /// +∞ for an unbound (e ≥ 1) orbit. The honest "is it orbiting — where, and how tightly?"
    /// test that decides moon vs ring vs collision. No Hill range, no reach-in grab.
    fn orbit_peri_apo(&self, a: usize, b: usize) -> (f32, f32) {
        let d = self.pos[b] - self.pos[a];
        let w = self.vel[b] - self.vel[a];
        let r = d.length().max(1e-9);
        let mu = (G * (self.mass[a] + self.mass[b])).max(1e-30);
        let lz = d.x * w.y - d.y * w.x;
        let p = lz * lz / mu;
        let eps = 0.5 * w.length_squared() - mu / r;
        let e = (1.0 + 2.0 * eps * lz * lz / (mu * mu)).max(0.0).sqrt();
        let peri = p / (1.0 + e);
        let apo = if e < 1.0 {
            p / (1.0 - e)
        } else {
            f32::INFINITY
        };
        (peri, apo)
    }

    /// One symplectic-Euler step: accelerate every live mote by every other, then kick + drift.
    fn integrate(&mut self, h: f32) {
        let n = self.mass.len();
        let soft2 = self.tuning.softening * self.tuning.softening;
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
                // Detection gate: only consider pairs whose accretion reaches overlap — a cheap
                // early-out; the real decision is the orbit test below.
                let reach = self.tuning.radius_k * (self.mass[i].cbrt() + self.mass[j].cbrt());
                if (self.pos[i] - self.pos[j]).length_squared() >= reach * reach {
                    continue;
                }
                let (a, b) = if self.mass[i] >= self.mass[j] {
                    (i, j)
                } else {
                    (j, i)
                };
                // Protection/rings apply only to satellites of a PLANET, never of the star. A
                // planet has a *bounded* domain — its Hill/L1 sphere, set by the L1 point with the
                // star — inside which satellites legitimately orbit. The star is the root dominant
                // with no outer L1 boundary, so close material is ABSORBED (falls through to the
                // contact merge), not held as a "moon of the star". `a != 0` is that distinction.
                //
                // For a genuine satellite of a planet (`a` is `b`'s dominant attractor — b truly
                // orbits a), its orbit decides its fate:
                //   • pericenter inside the surface           → a real hit → merge into the body.
                //   • whole orbit inside the tidal/Roche zone → a close, settled moon torn apart
                //     by tides → SHREDDED into a ring (`to_ring`).
                //   • otherwise                               → a MOON; leave it in orbit.
                // Gating the ring on *apocenter* (the whole orbit, not just pericenter) keeps
                // eccentric infalling debris out of the rings — that merges/accretes as normal —
                // so only genuinely close, circularised satellites ring. Sibling and moon–moon
                // pairs aren't satellites of each other and fall straight through to the merge.
                let mut to_ring = false;
                if a != 0 && a == self.host_of(b, self.pos[b]) {
                    let (q, apo) = self.orbit_peri_apo(a, b);
                    if q > self.phys_radius(a) + self.phys_radius(b) {
                        if apo < self.roche_radius(a, b) {
                            to_ring = true;
                        } else {
                            continue;
                        }
                    }
                }
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
                if to_ring {
                    // Shredded debris orbits `a` as a ring. Its mass + composition stay folded
                    // into `a` (conserved exactly, like a merge); `ring_mass` records how much is
                    // ring so the renderer draws it as a disc instead of growing the body.
                    self.ring_mass[a] += mb;
                }
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
        self.tuning.radius_k * self.mass[i].cbrt()
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
        self.mass
            .iter()
            .zip(&self.alive)
            .filter(|(_, &a)| a)
            .map(|(&m, _)| m)
            .sum()
    }

    /// Starting total mass, for a conservation check.
    pub fn init_total(&self) -> f32 {
        self.init_total
    }
}

/// Small integer hash (xorshift-multiply) → reproducible per-parcel draws without an RNG crate.
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
    use crate::config::Tuning;
    use crate::mass::{CloudMass, MassParams};
    use crate::model::{load_tables, Ejecta};

    fn sim() -> Sim {
        let ej = Ejecta::from_tables(&load_tables());
        let cast = CastParams::default();
        let cloud = CloudField::new(ej.elements.len(), 0xC10D_5EED, 0.6);
        let cm = CloudMass::derive(&ej, &MassParams::default());
        Sim::from_cloud(&ej, &cast, &cloud, &cm, 12, Tuning::default())
    }

    #[test]
    fn collapse_conserves_total_mass() {
        let mut s = sim();
        let before = s.init_total();
        for _ in 0..300 {
            s.step(0.02);
        }
        let after = s.total_mass();
        assert!(
            (after - before).abs() < 1e-3 * before,
            "mass conserved: {before} -> {after}"
        );
    }

    #[test]
    fn motes_merge_into_fewer_bodies() {
        let mut s = sim();
        let start = s.live_count();
        for _ in 0..400 {
            s.step(0.02);
        }
        assert!(
            s.live_count() < start,
            "collapse coalesces motes ({start} -> {})",
            s.live_count()
        );
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
        assert!(
            bound > 0.95 * s.total_mass(),
            "system stays bound: {bound} of {}",
            s.total_mass()
        );
    }

    #[test]
    fn a_moon_stays_in_orbit_around_its_planet() {
        // A star, a gas-giant planet on a circular orbit, and a small rocky moon on a circular
        // orbit about the planet — far outside the planet's (microscopic) physical radius but
        // inside its sphere of influence. With the angular-momentum-aware merge + the satellite
        // drag, the moon must SURVIVE (not be swallowed) and keep orbiting its planet.
        let n_el = 3; // [H (gas), O (ice), Fe (rock)]
        let el_numbers = vec![1u8, 8, 26];
        let (star_m, planet_m, moon_m) = (1.0f32, 5.0e-3f32, 1.0e-7f32);
        let (planet_r, moon_r) = (5.0f32, 0.15f32);

        let planet_p = Vec2::new(planet_r, 0.0);
        let moon_p = planet_p + Vec2::new(moon_r, 0.0);
        let v_planet = Vec2::new(0.0, (G * star_m / planet_r).sqrt());
        let v_moon = v_planet + Vec2::new(0.0, (G * planet_m / moon_r).sqrt());

        let mut comp = vec![0.0f32; 3 * n_el];
        comp[0] = star_m; // star = H
        comp[n_el] = planet_m; // planet = H (gas giant)
        comp[2 * n_el + 2] = moon_m; // moon = Fe (rock)
        let init_total = star_m + planet_m + moon_m;
        let mut s = Sim {
            n_el,
            pos: vec![Vec2::ZERO, planet_p, moon_p],
            vel: vec![Vec2::ZERO, v_planet, v_moon],
            mass: vec![star_m, planet_m, moon_m],
            comp,
            alive: vec![true; 3],
            ring_mass: vec![0.0; 3],
            el_numbers,
            time: 100.0, // past the gas era — only the circularising floor drag acts
            tuning: Tuning::default(),
            init_total,
        };

        for _ in 0..3000 {
            s.step(0.02);
        }
        assert!(
            s.alive[2],
            "the moon survived (was not swallowed by its planet)"
        );
        let d = (s.pos[2] - s.pos[1]).length();
        assert!(
            (0.02..0.6).contains(&d),
            "moon stays in orbit about its planet (d = {d} AU)"
        );
    }

    #[test]
    fn an_icy_satellite_inside_roche_shreds_into_a_ring() {
        // Star (far, dominant), a gas-giant host, and a low-density ICY satellite on a circular
        // orbit whose whole orbit sits inside the host's tidal/Roche zone. It must be torn into a
        // ring (host gains ring mass; the satellite is gone), not survive as a moon — and mass is
        // conserved. `merge()` is a pure decision on the current state, so this exercises the
        // shred branch directly without needing the integrator to hold a sub-resolution orbit.
        let n_el = 3; // [H (gas), O (ice), Fe (rock)]
        let (star_m, host_m, sat_m) = (1.0f32, 5.0e-3f32, 1.0e-7f32);
        let host_p = Vec2::new(5.0, 0.0);

        let mut comp = vec![0.0f32; 3 * n_el];
        comp[0] = star_m; // star = H
        comp[n_el] = host_m; // host = H (gas giant, low density)
        comp[2 * n_el + 1] = sat_m; // satellite = O (icy, shreds easily)
        let mut s = Sim {
            n_el,
            pos: vec![Vec2::ZERO, host_p, host_p],
            vel: vec![Vec2::ZERO, Vec2::ZERO, Vec2::ZERO],
            mass: vec![star_m, host_m, sat_m],
            comp,
            alive: vec![true; 3],
            ring_mass: vec![0.0; 3],
            el_numbers: vec![1u8, 8, 26],
            time: 0.0,
            tuning: Tuning::default(),
            init_total: star_m + host_m + sat_m,
        };
        // Place the satellite on a circular orbit inside the host's Roche radius (and outside its
        // tiny surface), so apocenter < roche → shred.
        let roche = s.roche_radius(1, 2);
        let surface = s.phys_radius(1) + s.phys_radius(2);
        assert!(
            roche > surface,
            "icy body has a real tidal shell: roche {roche} > surface {surface}"
        );
        let r = 0.5 * (surface + roche);
        s.pos[2] = host_p + Vec2::new(r, 0.0);
        s.vel[2] = Vec2::new(0.0, (G * host_m / r).sqrt()); // circular ⇒ peri ≈ apo ≈ r

        let before = s.total_mass();
        s.merge();
        assert!(
            !s.alive[2],
            "the icy satellite was shredded (no longer a standalone body)"
        );
        assert!(
            s.ring_mass[1] > 0.0,
            "its host gained a ring ({} M_sun)",
            s.ring_mass[1]
        );
        assert!(
            (s.total_mass() - before).abs() < 1e-12,
            "mass conserved through shredding"
        );
    }

    #[test]
    fn the_star_absorbs_close_bodies_rather_than_hosting_moons() {
        // A body deep inside the star's reach must merge INTO the star (the root dominant absorbs
        // its domain) — it must not be held as a protected "moon of the star". Contrast with
        // `a_moon_stays_in_orbit_around_its_planet`, where a planet's bounded Hill/L1 domain DOES
        // host a protected moon.
        let n_el = 3;
        let (star_m, body_m) = (1.0f32, 1.0e-5f32);
        let mut comp = vec![0.0f32; 2 * n_el];
        comp[0] = star_m; // star = H
        comp[n_el + 2] = body_m; // body = Fe (rock)
        let mut s = Sim {
            n_el,
            pos: vec![Vec2::ZERO, Vec2::new(0.2, 0.0)],
            vel: vec![Vec2::ZERO, Vec2::new(0.0, (G * star_m / 0.2).sqrt())],
            mass: vec![star_m, body_m],
            comp,
            alive: vec![true; 2],
            ring_mass: vec![0.0; 2],
            el_numbers: vec![1u8, 8, 26],
            time: 0.0,
            tuning: Tuning::default(),
            init_total: star_m + body_m,
        };
        // 0.2 AU is well inside the star's accretion reach (~0.8 AU) but far outside its surface.
        s.merge();
        assert!(
            !s.alive[1],
            "the close inner body was absorbed by the star, not kept as a moon"
        );
        assert!(s.ring_mass[0] == 0.0, "the star grows no ring");
    }

    #[test]
    fn playable_worlds_emerge_and_obey_the_gate() {
        // The gate must actually fire for some emergent systems (else the highlight is dead code),
        // and every world it flags must obey the physical gate: rocky, in the HZ, in the mass band.
        let ej = Ejecta::from_tables(&load_tables());
        let cast = CastParams::default();
        let cm = CloudMass::derive(&ej, &MassParams::default());
        let t = Tuning::default();
        let sqrt_l = 0.966f32.powf(3.5).sqrt();
        let (hz_in, hz_out) = (t.hz_inner_frac * sqrt_l, t.hz_outer_frac * sqrt_l);
        let mut total = 0usize;
        for seed in [
            0xC10D_5EED_u32,
            0x1111_2222,
            0xABCD_0001,
            0xDEAD_BEEF,
            0x5A5A_1234,
        ] {
            let cloud = CloudField::new(ej.elements.len(), seed, 0.6);
            let mut s = Sim::from_cloud(&ej, &cast, &cloud, &cm, 24, t);
            for _ in 0..40000 {
                s.step(0.02);
            }
            for i in 1..s.mass.len() {
                if s.is_playable(i) {
                    total += 1;
                    assert_eq!(
                        s.classify(i),
                        BodyType::RockyPlanet,
                        "playable world is rocky"
                    );
                    assert!(
                        (t.playable_mass_min..=t.playable_mass_max).contains(&s.mass[i]),
                        "in mass band"
                    );
                    let r = s.pos[i].length();
                    assert!((hz_in..=hz_out).contains(&r), "in the habitable zone");
                }
            }
        }
        assert!(
            total > 0,
            "at least one playable world emerges across the seed set"
        );
    }

    #[test]
    fn velocities_and_positions_stay_finite() {
        let mut s = sim();
        for _ in 0..500 {
            s.step(0.02);
        }
        for (i, &a) in s.alive.iter().enumerate() {
            if a {
                assert!(
                    s.pos[i].is_finite() && s.vel[i].is_finite(),
                    "no blow-up at mote {i}"
                );
            }
        }
    }
}
