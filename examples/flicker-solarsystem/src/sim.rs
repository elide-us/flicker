//! The formation simulation — a heliocentric N-body integration of the giant-impact
//! phase, recorded into a [`Timeline`] for playback.
//!
//! Embryos ([`crate::disk::seed_embryos`]) orbit the star under its gravity and their
//! own mutual pull (velocity-Verlet / leapfrog). When two (inflated) radii overlap a
//! collision is resolved by [`crate::collide::resolve`] — merge, hit-and-run, erosion,
//! or disruption — and the body set is rebuilt. Bodies flung onto unbound orbits are
//! recorded as **ejected**; ones that fall into the star are **consumed**. The whole
//! run is integrated once (on enter / reseed) and decimated into snapshots; the scene
//! plays the recording back, so scrubbing a path-dependent sim is free and
//! deterministic per seed.
//!
//! Caveat (see the handoff): collision radii are inflated, so the integration spans a
//! *compressed* dynamical time. The "Myr" the HUD shows is a cosmetic rescaling of the
//! recorded span, not a faithful clock.

use glam::DVec3;

use crate::astro::{G, M_STAR};
use crate::body::Body;
use crate::collide::{resolve, Regime};
use crate::disk::{seed_embryos, Nebula, DISK_INNER, DISK_OUTER};

/// Integration substeps per the innermost orbital period (resolution vs cost).
const SUBSTEPS_PER_ORBIT: f64 = 50.0;
/// Hard cap on integration steps (the runaway backstop / wall-clock bound).
const MAX_STEPS: usize = 500_000;
/// Don't stop before this many steps even if quiet (let the disk stir + giants perturb,
/// and the slower outer zone complete enough orbits to evolve).
const MIN_STEPS: usize = 130_000;
/// Stop early once this many consecutive steps pass with no collision.
const STALL_STEPS: usize = 80_000;
/// Record a snapshot every N steps (decimation → ~MAX_STEPS/N frames).
const RECORD_EVERY: usize = 100;
/// Check for ejections/infall every N steps (cheaper than every step).
const ESCAPE_CHECK_EVERY: usize = 50;
/// Beyond this heliocentric radius (AU), an unbound body has left the system.
const R_EJECT: f64 = DISK_OUTER * 3.0;
/// Inside this radius (AU), a body has fallen into the star.
const R_INFALL: f64 = DISK_INNER * 0.3;
/// Gravitational softening (AU) — keeps accelerations finite through close passes.
const SOFTENING: f64 = 1.0e-4;
/// Safety cap on body count (debris runaway backstop).
const MAX_BODIES: usize = 250;
/// **The "how much gravity bodies exert on their bands" lever.** Real accretion isn't
/// geometric: a body's effective capture cross-section is enhanced by gravitational
/// focusing — the Safronov factor `1 + (v_esc/v_rel)²` — so a *massive* body passing a
/// neighbour *slowly* sweeps up a far larger reach than its size alone. That's what makes
/// the biggest body in a zone run away, accrete its neighbours, and **clear the band**.
/// This multiplies the focusing term; higher → stronger runaway growth → fewer, larger,
/// better-separated planets. The factor is clamped to [`FOCUS_MAX`] so a near-zero
/// relative velocity can't make the reach blow up.
const ACCRETION_FOCUSING: f64 = 1.0;
const FOCUS_MAX: f64 = 12.0;
/// Absolute ceiling on the (inflated + focused) capture distance (AU), so two bodies never
/// merge while obviously far apart on screen.
const MAX_CAPTURE_AU: f64 = 0.22;

/// A collision event (for the render's impact flash + the HUD log).
#[derive(Clone, Debug)]
pub struct Event {
    pub t_year: f64,
    pub site: DVec3,
    pub regime: Regime,
}

/// A recorded instant of the run — the full bodies as they were, so the scene can render
/// them *and* run the live verdict/classification/export off the current moment (not just
/// the final state).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub t_year: f64,
    pub bodies: Vec<Body>,
}

/// The full recorded run: decimated snapshots, the collision events, the surviving
/// bodies (with composition, for the verdict/HUD), and tallies.
pub struct Timeline {
    pub snapshots: Vec<Snapshot>,
    pub events: Vec<Event>,
    pub span_year: f64,
    pub seeded: usize,
    pub ejected: usize,
    pub consumed: usize,
    /// The initial conditions this system formed from (supernova size, disk mass, metallicity).
    pub nebula: Nebula,
}

impl Timeline {
    /// The snapshot at or just before fractional time `f` in `[0, 1]`.
    pub fn sample(&self, f: f64) -> Option<&Snapshot> {
        if self.snapshots.is_empty() {
            return None;
        }
        let target = f.clamp(0.0, 1.0) * self.span_year;
        // Snapshots are time-ordered; find the last with t_year <= target.
        let mut idx = 0;
        for (i, s) in self.snapshots.iter().enumerate() {
            if s.t_year <= target {
                idx = i;
            } else {
                break;
            }
        }
        Some(&self.snapshots[idx])
    }
}

/// Run the formation simulation for `seed` with the given `supernova_size` (the master
/// initial-condition dial, `0..1`) and return its recorded timeline.
pub fn run(seed: u64, supernova_size: f64) -> Timeline {
    let nebula = Nebula::new(seed, supernova_size);
    let mut bodies = seed_embryos(seed, &nebula);
    let seeded = bodies.len();

    // Timestep from the innermost orbital period.
    let dt = initial_dt(&bodies);
    let mut events: Vec<Event> = Vec::new();
    let mut snapshots: Vec<Snapshot> = Vec::new();
    let mut ejected = 0usize;
    let mut consumed = 0usize;

    let mut acc = accelerations(&bodies);
    let mut t_year = 0.0f64;
    let mut since_collision = 0usize;

    snapshots.push(snapshot(t_year, &bodies));

    let mut step = 0usize;
    while step < MAX_STEPS && bodies.len() > 1 {
        // Velocity-Verlet (kick-drift-kick).
        let half = 0.5 * dt;
        for (b, a) in bodies.iter_mut().zip(&acc) {
            b.vel += *a * half;
            b.pos += b.vel * dt;
        }
        let acc2 = accelerations(&bodies);
        for (b, a) in bodies.iter_mut().zip(&acc2) {
            b.vel += *a * half;
        }
        acc = acc2;
        t_year += dt;
        step += 1;

        // One collision per step (the set changes; rescan next step).
        if let Some((i, j)) = find_collision(&bodies) {
            let out = resolve(&bodies[i], &bodies[j]);
            events.push(Event {
                t_year,
                site: out.site,
                regime: out.regime,
            });
            // Replace i, j with the outcome bodies.
            let (lo, hi) = (i.min(j), i.max(j));
            bodies.remove(hi);
            bodies.remove(lo);
            bodies.extend(out.bodies);
            cap_bodies(&mut bodies);
            acc = accelerations(&bodies);
            since_collision = 0;
        } else {
            since_collision += 1;
        }

        // Ejections / infall (periodic).
        if step.is_multiple_of(ESCAPE_CHECK_EVERY) {
            let before = bodies.len();
            remove_escapers(&mut bodies, &mut ejected, &mut consumed);
            if bodies.len() != before {
                acc = accelerations(&bodies);
            }
        }

        if step.is_multiple_of(RECORD_EVERY) {
            snapshots.push(snapshot(t_year, &bodies));
        }

        if step >= MIN_STEPS && since_collision >= STALL_STEPS {
            break;
        }
    }

    // Final snapshot — the settled system. The scene reads survivors from the snapshots
    // (the last one is the final state), so there's no separate `finals` to keep in sync.
    snapshots.push(snapshot(t_year, &bodies));

    Timeline {
        snapshots,
        events,
        span_year: t_year.max(dt),
        seeded,
        ejected,
        consumed,
        nebula,
    }
}

/// Timestep (yr): the shortest embryo period / [`SUBSTEPS_PER_ORBIT`].
fn initial_dt(bodies: &[Body]) -> f64 {
    let min_period = bodies
        .iter()
        .filter_map(|b| b.period())
        .fold(f64::INFINITY, f64::min);
    let p = if min_period.is_finite() {
        min_period
    } else {
        // Fallback: period at the inner disk edge.
        std::f64::consts::TAU * (DISK_INNER.powi(3) / (G * M_STAR)).sqrt()
    };
    (p / SUBSTEPS_PER_ORBIT).max(1.0e-5)
}

/// Newtonian accelerations (AU/yr²): the star plus all mutual pulls, softened.
fn accelerations(bodies: &[Body]) -> Vec<DVec3> {
    let n = bodies.len();
    let mut acc = vec![DVec3::ZERO; n];
    let soft2 = SOFTENING * SOFTENING;
    for i in 0..n {
        // Central star at the origin.
        let ri = bodies[i].pos;
        let r2 = ri.length_squared() + soft2;
        let inv_r3 = 1.0 / (r2 * r2.sqrt());
        acc[i] -= G * M_STAR * ri * inv_r3;
    }
    // Mutual gravity (symmetric).
    for i in 0..n {
        for j in (i + 1)..n {
            let d = bodies[j].pos - bodies[i].pos;
            let r2 = d.length_squared() + soft2;
            let inv_r3 = 1.0 / (r2 * r2.sqrt());
            acc[i] += G * bodies[j].mass * d * inv_r3;
            acc[j] -= G * bodies[i].mass * d * inv_r3;
        }
    }
    acc
}

/// The first colliding pair, if any. A pair "collides" when their separation drops below
/// the **gravitationally-focused** capture distance: the inflated geometric reach times the
/// Safronov factor `√(1 + focusing·(v_esc/v_rel)²)`. Massive bodies passing slowly therefore
/// reach much further than their size — runaway growth that clears the band — while fast,
/// light fly-bys capture only on near-geometric contact.
fn find_collision(bodies: &[Body]) -> Option<(usize, usize)> {
    let n = bodies.len();
    for i in 0..n {
        let geom_i = bodies[i].collision_radius();
        let phys_i = bodies[i].physical_radius();
        for j in (i + 1)..n {
            let d = bodies[j].pos - bodies[i].pos;
            let dist2 = d.length_squared();
            let geom_reach = geom_i + bodies[j].collision_radius();
            // Gravitational focusing from the pair's mutual escape vs relative speed.
            let r_phys = (phys_i + bodies[j].physical_radius()).max(1e-12);
            let v_rel2 = (bodies[j].vel - bodies[i].vel).length_squared().max(1e-9);
            let v_esc2 = 2.0 * G * (bodies[i].mass + bodies[j].mass) / r_phys;
            let focus = (1.0 + ACCRETION_FOCUSING * v_esc2 / v_rel2)
                .sqrt()
                .min(FOCUS_MAX);
            let reach = (geom_reach * focus).min(MAX_CAPTURE_AU);
            if dist2 < reach * reach {
                return Some((i, j));
            }
        }
    }
    None
}

/// Drop bodies that have left the system (unbound and far) or fallen into the star,
/// tallying each.
fn remove_escapers(bodies: &mut Vec<Body>, ejected: &mut usize, consumed: &mut usize) {
    bodies.retain(|b| {
        let r = b.helio_radius();
        if r < R_INFALL {
            *consumed += 1;
            return false;
        }
        if r > R_EJECT && !b.is_bound() {
            *ejected += 1;
            return false;
        }
        true
    });
}

/// If debris has pushed the count over the cap, drop the least-massive bodies.
fn cap_bodies(bodies: &mut Vec<Body>) {
    if bodies.len() <= MAX_BODIES {
        return;
    }
    bodies.sort_by(|a, b| b.mass.total_cmp(&a.mass));
    bodies.truncate(MAX_BODIES);
}

/// Snapshot the current bodies (a full clone, so the scene can run the live verdict).
fn snapshot(t_year: f64, bodies: &[Body]) -> Snapshot {
    Snapshot {
        t_year,
        bodies: bodies.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The final settled bodies = the last snapshot's bodies.
    fn finals(tl: &Timeline) -> &[Body] {
        &tl.snapshots.last().unwrap().bodies
    }

    #[test]
    fn run_is_deterministic() {
        let a = run(123, 0.5);
        let b = run(123, 0.5);
        assert_eq!(a.seeded, b.seeded);
        assert_eq!(finals(&a).len(), finals(&b).len(), "same seed → same survivors");
        assert_eq!(a.events.len(), b.events.len(), "same seed → same events");
    }

    #[test]
    fn formation_accretes_and_stays_finite() {
        let tl = run(7, 0.6);
        assert!(tl.seeded >= 8, "seeded a populated disk");
        assert!(!finals(&tl).is_empty(), "at least one body survives");
        // Accretion happened: collisions reduce the body count.
        assert!(!tl.events.is_empty(), "some collisions occurred");
        // Everything finite.
        for b in finals(&tl) {
            assert!(b.pos.is_finite() && b.vel.is_finite() && b.mass.is_finite());
            assert!(b.mass > 0.0);
        }
        for s in &tl.snapshots {
            for sb in &s.bodies {
                assert!(sb.pos.is_finite());
            }
        }
    }

    #[test]
    fn timeline_samples_in_order() {
        let tl = run(99, 0.5);
        let first = tl.sample(0.0).unwrap().t_year;
        let last = tl.sample(1.0).unwrap().t_year;
        assert!(last >= first, "later fractional time → later snapshot");
        assert!(tl.span_year > 0.0);
    }
}
