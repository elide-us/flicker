//! Collision outcomes — the giant-impact "conflicts".
//!
//! When two bodies' (inflated) radii overlap, [`resolve`] decides what happens from
//! the impact speed `v_imp` relative to the mutual escape speed `v_esc` and the
//! impact angle (impact parameter `b`, `0` head-on … `1` grazing) — a deliberately
//! simplified read of the Leinhardt & Stewart (2012) / Genda et al. hit-and-run
//! regime maps:
//!
//! - **Merge** — slow (`v_imp ≲ v_esc`): one body, mass + momentum conserved.
//! - **Hit-and-run** — fast *and* grazing, comparable sizes: the projectile survives,
//!   the two bounce apart; the target skims off a little of the projectile's outer
//!   layers. Two bodies out.
//! - **Erosion / accretion** — head-on, moderate: a largest remnant from a simplified
//!   `Q*_RD` law (target may grow or shrink), the rest flung off as debris.
//! - **Disruption** — head-on, very fast: a small largest remnant, most mass in debris.
//!
//! Composition rides through every regime: a body is layered core→envelope, and what
//! a collision strips or sheds comes off the **outside first** (envelope, then mantle,
//! before the dense core) — the mechanism behind iron-rich and desiccated survivors.
//! Mass and linear momentum are conserved exactly across the returned bodies (nothing
//! is silently dropped); a debris fragment that later leaves the system is the sim's
//! job to record.

use glam::DVec3;

use crate::astro::{G, M_EARTH};
use crate::body::{Body, BodyKind, Moon};

/// Which regime fired — for the HUD and the render's impact flash.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Regime {
    Merge,
    HitAndRun,
    Erosion,
    Disruption,
    /// A much smaller body, passing slowly and grazing, is bound into orbit as a moon.
    Capture,
}

/// The result of one collision: the bodies that come out (mass + momentum conserving),
/// the regime, and where it happened (for the flash).
pub struct Outcome {
    pub bodies: Vec<Body>,
    pub regime: Regime,
    pub site: DVec3,
}

/// Merge if the *approach* speed is at or below the mutual escape speed — a gentle
/// fall-together, regardless of angle. (The impact speed `v_imp = √(v_inf²+v_esc²)`
/// is always ≥ `v_esc`, so it can't be the gate; it sets the erosion *energy*.)
const MERGE_VINF: f64 = 1.0;
/// Impact parameter above which a comparable-size, fast impact is a hit-and-run.
const B_GRAZE: f64 = 0.7;
/// Minimum projectile/target mass ratio for hit-and-run (a tiny projectile can't bounce).
const HNR_MIN_RATIO: f64 = 0.1;
/// Fraction of the projectile's outer layers the target skims off in a hit-and-run.
const HNR_ACCRETE: f64 = 0.15;
/// How much relative speed survives a hit-and-run (the rest is lost to deformation).
const HNR_RESTITUTION: f64 = 0.35;
/// Sets the disruption-energy scale: `Q*_RD ≈ C_DISRUPT · ½v_esc²`.
const C_DISRUPT: f64 = 1.6;
/// Most debris fragments a single collision can spawn (bounds the body count).
const MAX_DEBRIS: usize = 2;
/// Below this, debris mass is folded into the largest remnant rather than tracked
/// as its own (fast, tiny) body — keeps the integrator well-behaved.
const MIN_DEBRIS_MASS: f64 = 0.01 * M_EARTH;
/// A projectile lighter than this fraction of the target *and* passing slowly and grazing
/// is captured into orbit as a moon rather than swallowed. (Abstracts the dissipation —
/// gas drag / three-body — that real gravitational capture needs; a reasonable POC stand-in.)
const MOON_MAX_RATIO: f64 = 0.12;
/// Capture only when the approach is slower than this fraction of the *target's* escape speed.
const MOON_CAPTURE_VINF: f64 = 1.3;
/// Giants capture far more readily than a bare rock: a deep potential well plus a
/// circumplanetary gas disk lets them bind a passing body over a **much wider approach
/// angle** ([`GIANT_B_GRAZE`], vs the grazing-only [`B_GRAZE`]) and up to a slightly larger
/// mass ratio. This is how a **remote giant** assembles a satellite system out of the dwarfs
/// and icy bodies that wander through its Hill sphere, instead of simply swallowing them.
const GIANT_MOON_MAX_RATIO: f64 = 0.15;
const GIANT_B_GRAZE: f64 = 0.35;

/// Resolve a collision between two overlapping bodies.
pub fn resolve(a: &Body, b: &Body) -> Outcome {
    // Order by mass: the larger is the target.
    let (target, proj) = if a.mass >= b.mass { (a, b) } else { (b, a) };

    let m_t = target.mass;
    let m_p = proj.mass;
    let m_tot = m_t + m_p;

    let v_rel = proj.vel - target.vel;
    let v_inf = v_rel.length();
    let v_esc = mutual_escape(target, proj);
    let v_imp = (v_inf * v_inf + v_esc * v_esc).sqrt();

    // Impact parameter b = sin θ from the approach geometry (0 head-on … 1 grazing).
    let d = proj.pos - target.pos;
    let b_param = if d.length() > 0.0 && v_inf > 0.0 {
        (d.cross(v_rel).length() / (d.length() * v_inf)).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Contact point along the line of centres (for the flash + debris placement).
    let r_t = target.physical_radius().max(1e-9);
    let r_p = proj.physical_radius().max(1e-9);
    let site = proj.pos + (target.pos - proj.pos) * (r_p / (r_t + r_p));

    // --- regime selection ---------------------------------------------------
    // Moon capture: a much smaller body, passing slowly, is bound into orbit rather than
    // swallowed. Giants capture over a wider approach angle and a slightly larger mass ratio
    // (deep well + circumplanetary drag), so remote giants build up satellite systems from the
    // passing dwarfs and icy bodies instead of merging them.
    let v_esc_t = target_escape(target);
    let (moon_ratio, moon_graze) = if target.is_giant() {
        (GIANT_MOON_MAX_RATIO, GIANT_B_GRAZE)
    } else {
        (MOON_MAX_RATIO, B_GRAZE)
    };
    if m_p / m_t < moon_ratio && v_inf < MOON_CAPTURE_VINF * v_esc_t && b_param >= moon_graze {
        return capture(target, proj, site);
    }
    if v_inf <= MERGE_VINF * v_esc {
        return Outcome {
            bodies: vec![merge(target, proj)],
            regime: Regime::Merge,
            site,
        };
    }
    if b_param >= B_GRAZE && m_p / m_t >= HNR_MIN_RATIO {
        return hit_and_run(target, proj, v_inf);
    }

    // Erosion / accretion / disruption via a simplified largest-remnant law.
    let mu = m_t * m_p / m_tot; // reduced mass
    let q_r = 0.5 * mu * v_imp * v_imp / m_tot; // specific impact energy
    let q_star = C_DISRUPT * 0.5 * v_esc * v_esc; // disruption threshold
    let ratio = if q_star > 0.0 { q_r / q_star } else { f64::INFINITY };

    // Leinhardt-Stewart largest-remnant fraction. Below 1.8× the threshold the target
    // grows or is mildly eroded; above it the impact is super-catastrophic.
    let (m_lr, regime) = if ratio < 1.8 {
        let frac = (1.0 - 0.5 * ratio).clamp(0.0, 1.0);
        (frac * m_tot, Regime::Erosion)
    } else {
        let frac = (0.1 * (ratio / 1.8).powf(-1.5)).clamp(0.0, 0.9);
        (frac * m_tot, Regime::Disruption)
    };

    accretion_with_debris(target, proj, m_lr, regime, site)
}

/// Mutual surface escape speed (AU/yr) from physical radii.
fn mutual_escape(t: &Body, p: &Body) -> f64 {
    let rsum = t.physical_radius() + p.physical_radius();
    if rsum <= 0.0 {
        return 0.0;
    }
    (2.0 * G * (t.mass + p.mass) / rsum).sqrt()
}

/// The target body's own surface escape speed (AU/yr) — the scale for binding a moon.
fn target_escape(t: &Body) -> f64 {
    let r = t.physical_radius();
    if r <= 0.0 {
        return 0.0;
    }
    (2.0 * G * t.mass / r).sqrt()
}

/// Capture: the projectile is bound as a moon of the target rather than swallowed. The
/// moon is bookkept on the planet (mass/composition tracked apart, *not* folded into the
/// planet's bulk), and the pair takes the barycentre velocity so the orbit barely shifts.
fn capture(target: &Body, proj: &Body, site: DVec3) -> Outcome {
    let mut t = target.clone();
    let m_tot = target.mass + proj.mass;
    t.vel = (target.vel * target.mass + proj.vel * proj.mass) / m_tot;
    t.moons.push(Moon {
        mass: proj.mass,
        comp: proj.comp.clone(),
    });
    t.moons.extend(proj.moons.iter().cloned()); // the projectile's own moons come along
    Outcome {
        bodies: vec![t],
        regime: Regime::Capture,
        site,
    }
}

/// Perfect merger at the centre of mass; the merged planet keeps both bodies' moons.
fn merge(t: &Body, p: &Body) -> Body {
    let m = t.mass + p.mass;
    let pos = (t.pos * t.mass + p.pos * p.mass) / m;
    let vel = (t.vel * t.mass + p.vel * p.mass) / m;
    let mut comp = t.comp.clone();
    comp.add(&p.comp);
    let kind = if t.is_giant() || p.is_giant() {
        BodyKind::Giant
    } else {
        BodyKind::Protoplanet
    };
    let mut body = Body::new(pos, vel, comp, kind);
    body.moons = t.moons.clone();
    body.moons.extend(p.moons.iter().cloned());
    body
}

/// Hit-and-run: both survive; the target skims off a little of the projectile; they
/// bounce apart at reduced relative speed. Mass + momentum conserved (two bodies).
fn hit_and_run(target: &Body, proj: &Body, v_inf: f64) -> Outcome {
    let mut t = target.clone();
    let mut p = proj.clone();

    // Target skims the projectile's outermost layers.
    let skim = HNR_ACCRETE * p.mass;
    let stripped = p.comp.strip_outermost(skim);
    t.comp.add(&stripped);
    t.sync_mass();
    p.sync_mass();

    let m_tot = t.mass + p.mass;
    let p_total = target.mass * target.vel + proj.mass * proj.vel; // conserve pre-momentum
    let v_cm = p_total / m_tot;

    // Recede along the line of centres at a restituted fraction of the approach speed.
    let n_sep = (p.pos - t.pos).normalize_or_zero();
    let u_rec = HNR_RESTITUTION * v_inf;
    t.vel = v_cm - n_sep * (p.mass / m_tot) * u_rec;
    p.vel = v_cm + n_sep * (t.mass / m_tot) * u_rec;

    // Nudge apart so the inflated radii no longer overlap (avoid instant re-trigger).
    let sep = (t.collision_radius() + p.collision_radius()) * 1.05;
    let mid = (t.pos + p.pos) * 0.5;
    t.pos = mid - n_sep * (sep * 0.5);
    p.pos = mid + n_sep * (sep * 0.5);

    let site = mid;
    Outcome {
        bodies: vec![t, p],
        regime: Regime::HitAndRun,
        site,
    }
}

/// Accretion/erosion/disruption: a largest remnant of mass `m_lr` plus debris holding
/// the rest. Composition: combine both bodies, then peel the lost mass off the
/// outside; debris carries the peeled (volatile/mantle-rich) material.
fn accretion_with_debris(
    target: &Body,
    proj: &Body,
    m_lr: f64,
    regime: Regime,
    site: DVec3,
) -> Outcome {
    let m_tot = target.mass + proj.mass;
    let p_total = target.mass * target.vel + proj.mass * proj.vel;
    let v_cm = p_total / m_tot;

    let mut combined = target.comp.clone();
    combined.add(&proj.comp);

    let lost = (m_tot - m_lr).clamp(0.0, m_tot);
    let debris_comp = combined.strip_outermost(lost); // outermost peeled to debris
    // `combined` is now the remnant (mass ≈ m_lr).

    let remnant_kind = if target.is_giant() || proj.is_giant() {
        BodyKind::Giant
    } else {
        BodyKind::Protoplanet
    };

    // The largest remnant keeps both bodies' moons.
    let kept_moons: Vec<Moon> = target
        .moons
        .iter()
        .chain(proj.moons.iter())
        .cloned()
        .collect();

    // If the lost mass is negligible, fold it back — clean accretion, no tiny fragment.
    if debris_comp.total() < MIN_DEBRIS_MASS {
        combined.add(&debris_comp);
        let mut remnant = Body::new(site_to_com(target, proj), v_cm, combined, remnant_kind);
        remnant.moons = kept_moons;
        return Outcome {
            bodies: vec![remnant],
            regime: Regime::Merge,
            site,
        };
    }

    // Split debris into up to MAX_DEBRIS fragments with outward kicks; assign the
    // remnant the leftover momentum so total momentum is conserved exactly.
    let n = MAX_DEBRIS.max(1);
    let frag_mass = debris_comp.total() / n as f64;
    let dirs = debris_directions(target, proj);
    let v_esc = mutual_escape(target, proj);
    let kick = 1.1 * v_esc; // debris leaves a touch above escape

    let mut bodies = Vec::with_capacity(n + 1);
    let mut p_debris = DVec3::ZERO;
    let remnant_pos = site_to_com(target, proj);
    for (i, dir) in dirs.iter().take(n).enumerate() {
        let frac = frag_mass / debris_comp.total();
        let comp = debris_comp.scaled(frac);
        let vel = v_cm + *dir * kick;
        // Place each fragment just outside the remnant along its kick direction.
        let off = (target.physical_radius() + proj.physical_radius()) * 30.0 * (i as f64 + 1.0);
        let pos = remnant_pos + *dir * off;
        p_debris += comp.total() * vel;
        bodies.push(Body::new(pos, vel, comp, BodyKind::Debris));
    }

    let remnant_vel = (p_total - p_debris) / combined.total().max(1e-30);
    let mut remnant = Body::new(remnant_pos, remnant_vel, combined, remnant_kind);
    remnant.moons = kept_moons;
    bodies.insert(0, remnant);

    Outcome {
        bodies,
        regime,
        site,
    }
}

/// Centre of mass of two bodies.
fn site_to_com(t: &Body, p: &Body) -> DVec3 {
    (t.pos * t.mass + p.pos * p.mass) / (t.mass + p.mass)
}

/// Deterministic, distinct outward directions for debris from the collision geometry
/// (no RNG, so `resolve` stays pure).
fn debris_directions(t: &Body, p: &Body) -> [DVec3; 2] {
    let n = (p.pos - t.pos).normalize_or_zero();
    let v = (p.vel - t.vel).normalize_or_zero();
    // A tangent perpendicular to the line of centres, in the impact plane.
    let mut tangent = (v - n * v.dot(n)).normalize_or_zero();
    if tangent.length_squared() < 1e-12 {
        tangent = n.cross(DVec3::Y).normalize_or_zero();
    }
    // Two spray directions splayed off the tangent.
    let d0 = (tangent + n * 0.4).normalize_or_zero();
    let d1 = (-tangent + n * 0.4).normalize_or_zero();
    [d0, d1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{Composition, MaterialClass};

    fn rock(mass_me: f64, pos: DVec3, vel: DVec3) -> Body {
        let mut c = Composition::of(MaterialClass::Silicate, 0.7 * mass_me * M_EARTH);
        c.add(&Composition::of(MaterialClass::Metal, 0.3 * mass_me * M_EARTH));
        Body::new(pos, vel, c, BodyKind::Protoplanet)
    }

    fn giant(mass_me: f64, pos: DVec3, vel: DVec3) -> Body {
        // A gas-enveloped giant: mostly H/He over a small rocky core.
        let mut c = Composition::of(MaterialClass::Gas, 0.92 * mass_me * M_EARTH);
        c.add(&Composition::of(MaterialClass::Silicate, 0.08 * mass_me * M_EARTH));
        Body::new(pos, vel, c, BodyKind::Giant)
    }

    fn total_mass(bodies: &[Body]) -> f64 {
        bodies.iter().map(|b| b.mass).sum()
    }
    fn total_momentum(bodies: &[Body]) -> DVec3 {
        bodies.iter().map(|b| b.vel * b.mass).sum()
    }
    /// Relative momentum drift (machine-precision is ~1e-15 of |p|).
    fn momentum_drift(out: &[Body], a: &Body, b: &Body) -> f64 {
        let p0 = a.vel * a.mass + b.vel * b.mass;
        let drift = (total_momentum(out) - p0).length();
        drift / p0.length().max(1e-30)
    }

    #[test]
    fn slow_head_on_merges() {
        // Two equal rocks drifting together slowly → one body.
        let a = rock(1.0, DVec3::new(1.0, 0.0, 0.0), DVec3::new(-1e-4, 0.0, 0.0));
        let b = rock(1.0, DVec3::new(-1.0, 0.0, 0.0), DVec3::new(1e-4, 0.0, 0.0));
        let out = resolve(&a, &b);
        assert_eq!(out.regime, Regime::Merge);
        assert_eq!(out.bodies.len(), 1);
        assert!((total_mass(&out.bodies) - (a.mass + b.mass)).abs() < 1e-30);
    }

    #[test]
    fn small_slow_grazing_body_is_captured_as_a_moon() {
        // A tiny body grazing a big one slowly → bound as a moon, not swallowed.
        let big = rock(1.0, DVec3::ZERO, DVec3::ZERO);
        let small = rock(0.05, DVec3::new(0.0, 0.0, 5e-4), DVec3::new(-1.0, 0.0, 0.0));
        let out = resolve(&big, &small);
        assert_eq!(out.regime, Regime::Capture);
        assert_eq!(out.bodies.len(), 1, "one body remains — the planet");
        assert_eq!(out.bodies[0].moons.len(), 1, "with one captured moon");
        // The moon is bookkept separately: the planet's own mass is unchanged.
        assert!((out.bodies[0].mass - big.mass).abs() < 1e-30);
        assert!((out.bodies[0].moons[0].mass - small.mass).abs() < 1e-30);
    }

    #[test]
    fn a_giant_captures_on_a_wider_approach_than_a_rock() {
        // A small body passing slowly at a moderate angle (b ≈ 0.5): grazing enough for a
        // giant's wide capture cone, but not for a bare rock (which needs b ≥ B_GRAZE).
        let off = DVec3::new(1.732e-3, 0.0, 1.0e-3); // |dz|/|d| = 0.5 → b ≈ 0.5
        let proj_vel = DVec3::new(-1.0e-3, 0.0, 0.0); // very slow approach
        // Giant target → binds the dwarf/icy body as a moon.
        let g = giant(50.0, DVec3::ZERO, DVec3::ZERO);
        let small = rock(0.3, off, proj_vel);
        let out = resolve(&g, &small);
        assert_eq!(out.regime, Regime::Capture, "a giant binds the slow small body as a moon");
        assert_eq!(out.bodies.len(), 1, "one body remains — the giant");
        assert_eq!(out.bodies[0].moons.len(), 1, "with one captured moon");
        // Same mass + geometry onto a bare rock → not a capture (it just merges).
        let r = rock(50.0, DVec3::ZERO, DVec3::ZERO);
        let small2 = rock(0.3, off, proj_vel);
        assert_ne!(resolve(&r, &small2).regime, Regime::Capture, "a rock needs a grazing pass to capture");
    }

    #[test]
    fn fast_grazing_is_hit_and_run() {
        // Comparable masses, grazing geometry, well above escape → two bodies out.
        let a = rock(1.0, DVec3::ZERO, DVec3::new(0.0, 0.0, 0.0));
        // Projectile offset perpendicular to its velocity → high impact parameter.
        let b = rock(0.8, DVec3::new(0.0, 0.0, 5e-4), DVec3::new(-6.0, 0.0, 0.0));
        let out = resolve(&a, &b);
        assert_eq!(out.regime, Regime::HitAndRun);
        assert_eq!(out.bodies.len(), 2);
        // Mass conserved; momentum conserved (to machine precision).
        assert!((total_mass(&out.bodies) - (a.mass + b.mass)).abs() < 1e-28);
        let dp = momentum_drift(&out.bodies, &a, &b);
        assert!(dp < 1e-12, "hit-and-run conserves momentum, rel drift {dp}");
    }

    #[test]
    fn fast_head_on_erodes_or_disrupts_and_conserves() {
        // Head-on, very fast → erosion/disruption with debris.
        let a = rock(1.0, DVec3::new(1.0, 0.0, 0.0), DVec3::new(-8.0, 0.0, 0.0));
        let b = rock(1.0, DVec3::new(-1.0, 0.0, 0.0), DVec3::new(8.0, 0.0, 0.0));
        let out = resolve(&a, &b);
        assert!(matches!(out.regime, Regime::Erosion | Regime::Disruption | Regime::Merge));
        // Mass conserved across remnant + debris.
        assert!((total_mass(&out.bodies) - (a.mass + b.mass)).abs() < 1e-26);
        // Momentum conserved (to machine precision).
        let dp = momentum_drift(&out.bodies, &a, &b);
        assert!(dp < 1e-12, "momentum rel drift {dp}");
        for body in &out.bodies {
            assert!(body.pos.is_finite() && body.vel.is_finite());
        }
    }
}
