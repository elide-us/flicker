//! The M1 interior stages (spec §7.5 1–2 + the core-formation output): radiogenic
//! heat, core differentiation (iron sinking inward to form the core — the textbook
//! "iron catastrophe"), and mantle convection.
//!
//! **What M1 must produce (spec §10):** *the planet differentiates* — a metallic
//! core and a silicate mantle separate from the undifferentiated bulk seed. That
//! is [`CoreFormation`]. The two enabling stages are [`RadiogenicDecay`] (the heat
//! that keeps the interior molten enough to differentiate and convect) and
//! [`MantleConvection`] (the flow, advected by the mandatory **semi-Lagrangian
//! resample** of §6.1).
//!
//! **Conservation.** None of these move mass *out* of the ledger. Radiogenic decay
//! and convection change only temperature/velocity (energy + kinematics, not the
//! mass ledger); core formation *moves* siderophile mass from the mantle into the
//! core, debited and credited in lockstep. The audit after every stage confirms it.

use glam::Vec3;

use flicker_materials::ElementId;

use crate::mantle::MAGMA_OCEAN_K;
use crate::planet::World;
use crate::stage::{Stage, StageRng};

// ── Radiogenic heat (spec §5.1) ──────────────────────────────────────────────
//
// Heat from the two radionuclides the Prism table actually carries: uranium
// (²³⁸U + ²³⁵U) and potassium (⁴⁰K). Thorium (²³²Th) — Earth's third big source —
// is **absent from the 28-element table**, so the young planet runs a little
// cooler than Earth would. That is a correct consequence of the element set, not a
// bug; adding Th needs a Book III ruling, out of M1 scope.
//
// Specific heat production, W per kg of the *isotope*:
const H_U238: f64 = 9.46e-5;
const H_U235: f64 = 5.69e-4;
const H_K40: f64 = 2.92e-5;
// Half-lives, Myr:
const TAU_U238: f64 = 4468.0;
const TAU_U235: f64 = 703.8;
const TAU_K40: f64 = 1248.0;
// Isotopic mass fraction of the parent element **at formation** (t=0 = 4.5 Gya),
// back-extrapolated from present-day abundances. U-235 and K-40 were far more
// abundant then — the reason the early planet was hotter falls out of *these*
// numbers decaying forward, never a hardcoded "young = hot" constant.
const F_U238: f64 = 0.77;
const F_U235: f64 = 0.23;
const F_K40: f64 = 1.46e-3;

/// Total radiogenic power, watts, from `u_mass_kg` uranium and `k_mass_kg`
/// potassium at model time `t_myr` (t=0 = accretion). Isotopic transmutation to Pb
/// and He is **not** tracked in the mass ledger at M1 (a mass-conserving
/// simplification): decay produces heat only.
pub fn radiogenic_power_w(u_mass_kg: f64, k_mass_kg: f64, t_myr: f64) -> f64 {
    let u = F_U238 * H_U238 * 0.5f64.powf(t_myr / TAU_U238)
        + F_U235 * H_U235 * 0.5f64.powf(t_myr / TAU_U235);
    let k = F_K40 * H_K40 * 0.5f64.powf(t_myr / TAU_K40);
    u_mass_kg * u + k_mass_kg * k
}

/// Silicate specific heat, J/(kg·K).
const SPECIFIC_HEAT: f64 = 1000.0;
/// Seconds per Myr — radiogenic power (W = J/s) integrates over the tick's seconds.
const SECONDS_PER_MYR: f64 = 3.155_76e13;
/// Cold surface the mantle sheds heat toward, K.
const SURFACE_K: f64 = 300.0;
/// Newtonian cooling coefficient, per Myr per K above [`SURFACE_K`]. Tuned so the
/// planet cools over Gyr rather than instantly — the deep interior stays hot.
const COOLING_PER_MYR: f64 = 1.1e-3;

/// Atomic numbers of the radiogenic elements.
const K_Z: ElementId = 19;
const U_Z: ElementId = 92;

/// **RadiogenicDecay** — isotope decay heats each mantle cell; a cold surface cools
/// it. `dT = radiogenic − cooling`. Changes temperature only, never mass, so it is
/// trivially conservation-safe. Always live.
pub struct RadiogenicDecay;

impl Stage for RadiogenicDecay {
    fn name(&self) -> &'static str {
        "RadiogenicDecay"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let t = world.tick_myr;
        let dt_sec = dt_myr * SECONDS_PER_MYR;
        let m = &mut world.mantle;
        for c in 0..m.n_cells() {
            let cell_mass = m.cell_mass(c);
            if cell_mass <= 0.0 {
                continue;
            }
            let power = radiogenic_power_w(m.mass(c, U_Z), m.mass(c, K_Z), t);
            let d_radio = power / (cell_mass * SPECIFIC_HEAT) * dt_sec;
            let d_cool = COOLING_PER_MYR * (m.temp_k[c] - SURFACE_K) * dt_myr;
            m.temp_k[c] = (m.temp_k[c] + d_radio - d_cool).max(SURFACE_K);
        }
    }
}

// ── Core formation — iron sinks to the core (the M1 output) ──────────────────

/// Temperature above which metal can percolate and settle — differentiation only
/// proceeds while the cell is this molten (a **chemistry gate**, not a clock).
const FE_SEGREGATION_K: f64 = 1800.0;
/// Fraction of a molten cell's remaining core-destined metal that drains per Myr —
/// a molten cell fully differentiates in ~50 Myr (core formation is geologically
/// fast).
const DIFFERENTIATION_RATE: f64 = 0.02;

/// Siderophile partition: `(atomic number, fraction that ends in the core once
/// fully differentiated)`. These are **metal–silicate partition coefficients**,
/// not a target: the resulting core mass (~31% of the planet, iron-dominated)
/// *emerges* from them — nobody wrote "the core is a third of the planet". Iron
/// keeps ~15% behind as mantle FeO; the lithophiles (O, Si, Mg, Ca, Al, K, U…) are
/// absent here and stay in the mantle entirely.
const SIDEROPHILES: &[(ElementId, f64)] = &[
    (26, 0.85), // Fe
    (28, 0.92), // Ni
    (27, 0.90), // Co
    (16, 0.55), // S  — the core's light element
    (29, 0.50), // Cu
    (15, 0.40), // P
    (24, 0.10), // Cr — mildly siderophile
    (78, 0.98), // Pt — highly siderophile
    (79, 0.98), // Au
    (47, 0.85), // Ag
];

/// **CoreFormation** — while a cell is molten, siderophile metals sink out of the
/// mantle into the (global, well-mixed) core, toward their partition targets. Mass
/// moves mantle → core, conserved. Gated on temperature, never the tick number.
///
/// The per-cell reference is the homogeneous seed mass `budget/N`; this is valid
/// because M1 does **not** advect composition laterally (only temperature), so a
/// cell's element mass changes only by this drain. Deterministic: cells and
/// elements are visited in a fixed order (float addition into the core is not
/// associative, §11).
pub struct CoreFormation;

impl Stage for CoreFormation {
    fn name(&self) -> &'static str {
        "CoreFormation"
    }

    fn is_live(&self, state: &crate::planet::PlanetState) -> bool {
        // Nothing to differentiate once the mantle is cold everywhere or fully
        // drained — a chemistry gate, cheap to skip.
        state.mean_mantle_temp_k > SURFACE_K + 1.0 && state.differentiation_frac < 0.999
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        let n = world.mantle.n_cells();
        let n_f = n as f64;
        for c in 0..n {
            let t = world.mantle.temp_k[c];
            if t < FE_SEGREGATION_K {
                continue; // frozen — metal can't segregate through solid rock
            }
            // Metal settles faster in hotter, more molten rock, so core formation
            // SWEEPS out from the hot upwellings rather than fading uniformly — an
            // emergent spatial pattern tied to the convection field. Every molten
            // cell still reaches d=1; only the rate varies, so conservation and the
            // partition targets are unchanged.
            let hot = ((t - FE_SEGREGATION_K) / (MAGMA_OCEAN_K - FE_SEGREGATION_K)).clamp(0.05, 1.5);
            let d = (world.mantle.differentiation[c] + DIFFERENTIATION_RATE * hot * dt_myr).min(1.0);
            world.mantle.differentiation[c] = d;
            for &(e, phi) in SIDEROPHILES {
                let m0 = world.budget.accreted(e) / n_f;
                let target = m0 * (1.0 - d * phi);
                let drain = world.mantle.mass(c, e) - target;
                if drain > 0.0 {
                    let taken = world.mantle.remove(c, e, drain);
                    world.reservoirs.core.add(e, taken);
                }
            }
        }
    }
}

// ── Mantle convection — the semi-Lagrangian resample (spec §6.1) ─────────────

/// Velocity gain: temperature gradient → surface flow. Large; the CFL clamp below
/// governs the actual per-tick displacement, so this only has to be big enough that
/// gradient regions reach the clamp (flat regions stay still — no flow without a
/// gradient).
const CONVECTION_GAIN: f32 = 3.0e-5;
/// Courant limit: a parcel drifts at most this fraction of the local cell spacing
/// per tick, so the departure point stays inside the 1-ring and the resample is a
/// convex blend of a cell and its neighbours — **bounded, never piling** (§6.1).
const CFL_FRACTION: f32 = 0.5;

/// **MantleConvection** — derive a surface velocity field from the temperature
/// gradient (hot upwellings spread, cold downwellings converge), then advect the
/// temperature field by the **semi-Lagrangian resample** of §6.1: every cell looks
/// *upstream* and takes a distance-weighted average of the temperature it drifted
/// from. A convex blend neither scatters nor piles, so the field stays smooth — no
/// noise-field blow-up (the bug that cost the old code four sessions). Changes
/// temperature + velocity only; mass is untouched.
pub struct MantleConvection;

impl MantleConvection {
    /// Surface flow ∝ −∇T (hot upwellings spread, cold downwellings converge). The
    /// gradient is a **least-squares tangent-plane fit** to the neighbour
    /// temperatures ([`tangent_gradient`]), NOT a raw neighbour sum — so the flow
    /// follows the thermal field, not the icosphere mesh (no grid print-through, the
    /// shard-seam ghost that shaped crust along the icosa faces). Writes
    /// `mantle.velocity`.
    fn derive_velocity(world: &mut World) {
        let temp = world.mantle.temp_k.clone();
        let dirs = &world.grid.dirs;
        let neighbors = &world.grid.neighbors;
        for i in 0..temp.len() {
            let pi = dirs[i];
            let ti = temp[i];
            let grad = tangent_gradient(
                pi,
                neighbors[i].iter().map(|&j| (dirs[j as usize], (temp[j as usize] - ti) as f32)),
            );
            world.mantle.velocity[i] = -CONVECTION_GAIN * grad;
        }
    }

    /// Advect temperature one step by the semi-Lagrangian resample. The drift is
    /// `velocity · dt`, so transport scales with the tick length (stays consistent
    /// when the adaptive controller of §7.3 varies `dt`); the CFL clamp then keeps
    /// the departure point inside the 1-ring.
    fn advect_temperature(world: &mut World, dt_myr: f64) {
        let old = world.mantle.temp_k.clone();
        let dirs = &world.grid.dirs;
        let neighbors = &world.grid.neighbors;
        for i in 0..old.len() {
            let pi = dirs[i];
            // Local spacing → the CFL clamp AND the resample regulariser (scaled to
            // spacing² so the self-weight is grid-resolution-independent, not a fixed
            // absolute constant that would over-pin coarse grids).
            let mut spacing = 0.0f32;
            for &j in &neighbors[i] {
                spacing += (dirs[j as usize] - pi).length();
            }
            spacing /= neighbors[i].len().max(1) as f32;
            let eps = (spacing * spacing * 1e-3).max(1e-12);
            let mut disp = world.mantle.velocity[i] * dt_myr as f32;
            let max_disp = CFL_FRACTION * spacing;
            if disp.length() > max_disp {
                disp = disp.normalize_or_zero() * max_disp;
            }
            // Departure point: where this parcel drifted FROM (upstream).
            let departure = (pi - disp).normalize_or_zero();
            // Convex resample of the old field over {i} ∪ neighbours by inverse
            // squared chord-distance to the departure point.
            let mut wsum = 0.0f32;
            let mut tsum = 0.0f64;
            let mut accumulate = |c: usize| {
                let d2 = (dirs[c] - departure).length_squared() + eps;
                let w = 1.0 / d2;
                wsum += w;
                tsum += w as f64 * old[c];
            };
            accumulate(i);
            for &j in &neighbors[i] {
                accumulate(j as usize);
            }
            world.mantle.temp_k[i] = tsum / wsum as f64;
        }
    }
}

impl Stage for MantleConvection {
    fn name(&self) -> &'static str {
        "MantleConvection"
    }

    fn tick(&self, world: &mut World, dt_myr: f64, _rng: &mut StageRng) {
        Self::derive_velocity(world);
        Self::advect_temperature(world, dt_myr);
    }
}

/// Unit tangent at `from` pointing toward `to` on the unit sphere (the component of
/// `to − from` orthogonal to `from`). Zero if the two coincide.
pub(crate) fn tangent_toward(from: Vec3, to: Vec3) -> Vec3 {
    let d = to - from;
    (d - from * d.dot(from)).normalize_or_zero()
}

/// Least-squares estimate of the tangent-plane gradient of a scalar at `pi` on the
/// unit sphere, from `(neighbour_dir, Δvalue)` samples (`Δ = value_j − value_i`). It
/// fits a plane to the neighbour differences (inverse-distance weighted), so a smooth
/// field yields a smooth gradient **regardless of how the mesh is arranged** — the
/// fix for the grid print-through a raw neighbour sum suffers (variable neighbour
/// count / spacing at pentagons and shard edges leaks into the sum). Returns a vector
/// in the tangent plane at `pi`; zero if the neighbours are degenerate.
pub(crate) fn tangent_gradient(pi: Vec3, samples: impl Iterator<Item = (Vec3, f32)>) -> Vec3 {
    // A stable orthonormal tangent basis at pi.
    let seed = if pi.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let e1 = (seed - pi * seed.dot(pi)).normalize_or_zero();
    let e2 = pi.cross(e1);
    // Normal equations for  min_g  Σ w (g·d − Δ)²  over the 2-D tangent coords d.
    let (mut a11, mut a12, mut a22) = (0.0f32, 0.0, 0.0);
    let (mut b1, mut b2) = (0.0f32, 0.0);
    for (pj, dv) in samples {
        let off = pj - pi;
        let d1 = off.dot(e1);
        let d2 = off.dot(e2);
        let w = 1.0 / (off.length_squared() + 1e-9);
        a11 += w * d1 * d1;
        a12 += w * d1 * d2;
        a22 += w * d2 * d2;
        b1 += w * d1 * dv;
        b2 += w * d2 * dv;
    }
    let det = a11 * a22 - a12 * a12;
    if det.abs() < 1e-12 {
        return Vec3::ZERO;
    }
    let g1 = (a22 * b1 - a12 * b2) / det;
    let g2 = (a11 * b2 - a12 * b1) / det;
    g1 * e1 + g2 * e2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;
    use crate::config::content_data_dir;
    use crate::planet::{PlanetState, World};
    use crate::scheduler::Scheduler;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldgrid::icosphere;

    fn world(freq: u32, seed: u64) -> World {
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let b = Budget::from_dir(&dir, &t).expect("budget");
        World::seed(icosphere(freq), b, &t, seed)
    }

    #[test]
    fn radiogenic_heat_was_higher_when_young() {
        // The early planet ran hotter — it must fall out of the decay curve, not a
        // constant. Same U/K, later time → less power.
        let young = radiogenic_power_w(1e18, 1e21, 0.0);
        let old = radiogenic_power_w(1e18, 1e21, 4500.0);
        assert!(young > old, "radiogenic power must decline as isotopes decay");
        assert!(young > 2.0 * old, "several-fold hotter young — U-235/K-40 were abundant");
    }

    #[test]
    fn the_planet_differentiates() {
        // THE M1 milestone gate: a metallic core separates from the bulk seed, and
        // it is iron-dominated. Nobody set the core mass — it emerges from the
        // partition coefficients.
        let mut w = world(6, 3);
        let mut sched = Scheduler::new(
            vec![
                Box::new(RadiogenicDecay),
                Box::new(CoreFormation),
                Box::new(MantleConvection),
            ],
            3,
        );
        for _ in 0..120 {
            sched.step(&mut w, 1.0, None); // the audit runs each tick — conservation held throughout
        }
        let core = w.reservoirs.core.total();
        let planet = w.budget.total();
        let frac = core / planet;
        assert!(
            (0.25..0.40).contains(&frac),
            "core should be ~a third of the planet, got {:.1}%",
            frac * 100.0,
        );
        // Iron dominates the core (real core is ~85% Fe) — if Fe stayed in the
        // mantle, differentiation is broken.
        let fe_in_core = w.reservoirs.core.amount(26);
        assert!(fe_in_core / core > 0.80, "core should be iron-dominated");
        // And the mantle is depleted in iron relative to the bulk seed.
        let mantle_fe_frac = w.mantle.element_mass(26) / w.mantle.total_mass();
        let bulk_fe_frac = w.budget.accreted(26) / planet;
        assert!(mantle_fe_frac < bulk_fe_frac, "the mantle lost iron to the core");
    }

    /// A hash of the ENTIRE simulated state — the temperature field, every core
    /// element, and the differentiation array. The right object for §11 "same seed
    /// → identical output hash" (a single scalar would miss a reordering in a field
    /// only convection touches).
    fn world_hash(w: &World) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for &t in &w.mantle.temp_k {
            t.to_bits().hash(&mut h);
        }
        for &d in &w.mantle.differentiation {
            d.to_bits().hash(&mut h);
        }
        for (e, m) in w.reservoirs.core.iter() {
            e.hash(&mut h);
            m.to_bits().hash(&mut h);
        }
        h.finish()
    }

    #[test]
    fn the_full_interior_run_is_deterministic() {
        // §11: same seed → identical world. Run the FULL interior pipeline — crucially
        // including MantleConvection, the stage that sums floats over neighbour lists
        // (the real fixed-order hazard) — twice, and compare a hash of the whole
        // world, not just one scalar.
        let run = |seed: u64| {
            let mut w = world(5, seed);
            let mut s = Scheduler::new(crate::interior_stages(), seed);
            for _ in 0..40 {
                s.step(&mut w, 1.0, None);
            }
            world_hash(&w)
        };
        assert_eq!(run(11), run(11), "same seed → identical world hash");
        assert_ne!(run(11), run(12), "different seed → a different world");
    }

    #[test]
    fn velocity_gradient_ignores_the_grid() {
        // Grid-ghost regression. Impose a perfectly smooth linear field T = k·dir;
        // its gradient is known analytically (k minus its radial part). The
        // least-squares tangent gradient must align with that analytic direction at
        // EVERY cell — pentagons and shard edges included — so the icosphere leaves
        // no fingerprint in the flow. A raw neighbour sum fails this at the seams.
        let w = world(8, 1);
        let k = Vec3::new(0.3, 0.9, -0.2);
        let dirs = &w.grid.dirs;
        let mut worst = 1.0f32;
        let mut checked = 0;
        for i in 0..dirs.len() {
            let pi = dirs[i];
            let grad = tangent_gradient(
                pi,
                w.grid.neighbors[i].iter().map(|&j| (dirs[j as usize], k.dot(dirs[j as usize]) - k.dot(pi))),
            );
            let analytic = k - pi * k.dot(pi);
            // Near the k-axis the tangential gradient vanishes — direction is ill-defined.
            if analytic.length() < 0.2 || grad.length() < 1e-6 {
                continue;
            }
            worst = worst.min(grad.normalize().dot(analytic.normalize()));
            checked += 1;
        }
        assert!(checked > 100, "sampled a real spread of cells ({checked})");
        assert!(worst > 0.9, "the gradient follows the field, not the grid (worst cos {worst:.3})");
    }

    /// RMS of each cell's departure from its neighbour mean — the roughness the
    /// noise-field bug inflates.
    fn roughness(w: &World) -> f64 {
        let t = &w.mantle.temp_k;
        let mut acc = 0.0;
        for i in 0..t.len() {
            let nb = &w.grid.neighbors[i];
            let mean: f64 = nb.iter().map(|&j| t[j as usize]).sum::<f64>() / nb.len().max(1) as f64;
            acc += (t[i] - mean).powi(2);
        }
        (acc / t.len() as f64).sqrt()
    }

    #[test]
    fn temperature_stays_smooth_through_convection() {
        // The §6.1 regression test (the M1 analogue of relief_stays_smooth_through_
        // plate_drift): advect the perturbed thermal field many ticks with ONLY
        // convection (no heating), and the field must NOT roughen into a checkerboard.
        // A scatter/flux would blow this up 30–50×; the semi-Lagrangian resample
        // keeps it bounded (numerical diffusion even smooths it).
        let mut w = world(8, 5);
        let conv = MantleConvection;
        let mut rng = StageRng::new(1);
        let before = roughness(&w);
        for _ in 0..40 {
            conv.tick(&mut w, 1.0, &mut rng);
        }
        let after = roughness(&w);
        assert!(
            after < 2.0 * before,
            "convection roughened the field ({before:.2} → {after:.2}) — the resample is scattering",
        );
    }

    #[test]
    fn convection_conserves_mass_and_leaves_planetstate_sane() {
        let mut w = world(6, 2);
        let conv = MantleConvection;
        let mut rng = StageRng::new(1);
        let before = w.mantle.total_mass();
        for _ in 0..10 {
            conv.tick(&mut w, 1.0, &mut rng);
        }
        // Convection moves heat, not mass.
        assert_eq!(w.mantle.total_mass(), before);
        let s = PlanetState::sample(&w);
        assert!(s.mean_mantle_temp_k > SURFACE_K);
    }
}
