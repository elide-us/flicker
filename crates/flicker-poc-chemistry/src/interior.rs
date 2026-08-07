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
pub struct RadiogenicDecay {
    /// Multiplier on radiogenic heating — the heat from inside, as a dial.
    pub heat: f64,
}

impl Default for RadiogenicDecay {
    /// The physics as written.
    fn default() -> Self {
        Self { heat: 1.0 }
    }
}

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
            // The heat dial scales the source, not the result: a hotter world stays
            // molten longer and differentiates sooner, and everything downstream of
            // that follows on its own.
            let d_radio = self.heat * power / (cell_mass * SPECIFIC_HEAT) * dt_sec;
            let d_cool = COOLING_PER_MYR * (m.temp_k[c] - SURFACE_K) * dt_myr;
            m.temp_k[c] = (m.temp_k[c] + d_radio - d_cool).max(SURFACE_K);
        }
    }
}

// ── Core formation — iron sinks to the core (the M1 output) ──────────────────

/// Temperature above which metal can percolate and settle — differentiation only
/// proceeds while the cell is this molten (a **chemistry gate**, not a clock).
pub(crate) const FE_SEGREGATION_K: f64 = 1800.0;
/// Fraction of a molten cell's remaining core-destined metal that drains per Myr —
/// a molten cell fully differentiates in ~50 Myr (core formation is geologically
/// fast).
const DIFFERENTIATION_RATE: f64 = 0.02;

/// Siderophile partition: `(atomic number, fraction that ends in the core once
/// fully differentiated)`. These are **metal–silicate partition coefficients**,
/// not a target: the resulting core mass (~32% of the planet, iron-dominated)
/// *emerges* from them — nobody wrote "the core is a third of the planet". Iron
/// keeps ~15% behind as mantle FeO; the lithophiles (O, Si, Mg, Ca, Al, K, U…) are
/// absent here and stay in the mantle entirely.
const SIDEROPHILES: &[(ElementId, f64)] = &[
    (26, 0.85), // Fe
    (28, 0.92), // Ni
    (27, 0.90), // Co
    (16, 0.97), // S — the core's light element. Metal–silicate D_S runs ~50–100
    //             at magma-ocean conditions, and a third-of-a-planet of metal at
    //             that D dissolves ~97% of the sulfur and carries it down —
    //             Earth's core holds ≳95% of the planet's S while the mantle
    //             keeps a few hundred ppm. At the old 0.55, half the S inventory
    //             (S is ~97% of the whole volatile budget) reached the sky as
    //             SO₂ instead (defect 7E01115B).
    (29, 0.50), // Cu
    (15, 0.40), // P
    (24, 0.10), // Cr — mildly siderophile
    (78, 0.98), // Pt — highly siderophile
    (79, 0.98), // Au
    (47, 0.85), // Ag
];

/// Core-partition fraction of `element` — the φ of its [`SIDEROPHILES`] row, 0
/// for the lithophiles.
fn partition_frac(element: ElementId) -> f64 {
    SIDEROPHILES.iter().find(|&&(e, _)| e == element).map_or(0.0, |&(_, phi)| phi)
}

/// Mass of `element` in `cell` that is **dissolved in the cell's not-yet-sunk
/// metal**: `φ · seed-share · (1 − d)` — the load the remaining metal will carry
/// down as it drains. Metal–silicate partition equilibrium is fast (melt and
/// metal rain touch everywhere); the *sinking* is what takes tens of Myr. So
/// this mass is spoken for by the core from the moment the cell is molten, and
/// only the rest is anyone else's to take.
///
/// [`Outgassing`](crate::atmosphere::Outgassing) subtracts it from a driver's
/// availability, which is what makes core formation and degassing genuinely
/// **compete** for sulfur — the one element that is both a major volatile and
/// the core's light element — the way they do on a real planet, where most
/// sulfur sinks with the iron long before it can degas (defect 7E01115B). Zero
/// for lithophiles and for a fully differentiated cell (d = 1): the residue is
/// free.
pub fn metal_bound_mass(world: &World, cell: usize, element: ElementId) -> f64 {
    let phi = partition_frac(element);
    if phi <= 0.0 {
        return 0.0;
    }
    let seed_share = world.budget.accreted(element) / world.mantle.n_cells().max(1) as f64;
    phi * seed_share * (1.0 - world.mantle.differentiation[cell])
}

/// **CoreFormation** — while a cell is molten, siderophile metals sink out of the
/// mantle into the (global, well-mixed) core, toward their partition targets. Mass
/// moves mantle → core, conserved. Gated on temperature, never the tick number.
///
/// The drain is an **incremental claim**: the metal that actually sinks this tick
/// (`Δd` of the cell's core-destined share) carries `budget/N · φ · Δd` of every
/// siderophile down with it, clamped by what the cell still holds. The claim is
/// sized to the homogeneous seed share and **never re-derived from the cell's
/// current inventory** — so a competitor that removes mantle mass (outgassing
/// stripping sulfur, crust freezing, an eruption's melt) spends its *own* pool,
/// not the core's. The old drain-to-a-target formula re-baselined against what
/// was left and silently yielded the core's share to whoever moved first; sulfur
/// was the case that mattered (defect 7E01115B — see [`metal_bound_mass`], the
/// degassing side of the same competition). Deterministic: cells and elements are
/// visited in a fixed order (float addition into the core is not associative,
/// §11).
pub struct CoreFormation;

impl Stage for CoreFormation {
    fn name(&self) -> &'static str {
        "CoreFormation"
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
            let d0 = world.mantle.differentiation[c];
            let d = (d0 + DIFFERENTIATION_RATE * hot * dt_myr).min(1.0);
            world.mantle.differentiation[c] = d;
            // The metal share that actually sank this tick claims its dissolved
            // load — and only that. What another process already took is that
            // process's win, not a head start on the core's target.
            let sunk = d - d0;
            if sunk <= 0.0 {
                continue;
            }
            for &(e, phi) in SIDEROPHILES {
                let claim = world.budget.accreted(e) / n_f * phi * sunk;
                let taken = world.mantle.remove(c, e, claim);
                if taken > 0.0 {
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
/// How much of a **flat-weighted** ring mean is blended into each resampled
/// value — the stabilising numerical diffusion, made explicit. The old
/// inverse-distance kernel supplied diffusion implicitly, but its weights took
/// the ring's geometry, so the diffusion was anisotropic along the lattice axes
/// and etched the shard edges into the field. Equal weights carry no geometry —
/// a pentagon averages its five exactly as a hex averages its six — so this
/// kills cell-scale roughness (the §6.1 guarantee) without preferring any
/// lattice direction.
const RESAMPLE_DIFFUSION: f64 = 0.1;

/// **MantleConvection** — derive a surface velocity field from the temperature
/// gradient (hot upwellings spread, cold downwellings converge), then advect the
/// temperature field by the **semi-Lagrangian resample** of §6.1: every cell looks
/// *upstream* and evaluates the temperature it drifted from by the least-squares
/// linear fit, clamped to its ring's own range. Ring-bounded values neither
/// scatter nor pile, so the field stays smooth — no noise-field blow-up (the bug
/// that cost the old code four sessions) — and a fit has no kernel shape to
/// print the lattice through (see [`Self::advect_temperature`]). Changes
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
    ///
    /// The departure value is evaluated by the **least-squares linear fit**
    /// ([`tangent_gradient`]) — the same frame-consistent operator the velocity
    /// derivation trusts — and then clamped to the ring's own value range. The
    /// clamp preserves the property the old inverse-distance kernel was chosen
    /// for (a resample bounded by what the ring holds neither scatters nor
    /// piles, the §6.1 no-blow-up guarantee). What the kernel could NOT do is
    /// stay blind to the ring's shape: its weights took the lattice's geometry,
    /// so its numerical diffusion was anisotropic along the lattice axes, and
    /// over hundreds of ticks it ETCHED the shard edges into the temperature
    /// field — measured as a shard-edge ΔT excess growing to ~7% and a 1.6×
    /// strain excess in the derived flow, which is what locked the conveyor's
    /// first plate boundaries onto the pentagon-to-pentagon lines (the R5b
    /// grid-ghost class, returned through the resample; see
    /// `convection_strain_ignores_the_shard_edges`). A fit evaluation has no
    /// kernel shape to imprint: exact for linear fields from any ring, seams
    /// and pentagons included.
    fn advect_temperature(world: &mut World, dt_myr: f64) {
        let old = world.mantle.temp_k.clone();
        let dirs = &world.grid.dirs;
        let neighbors = &world.grid.neighbors;
        for i in 0..old.len() {
            let pi = dirs[i];
            // Local spacing → the CFL clamp.
            let mut spacing = 0.0f32;
            for &j in &neighbors[i] {
                spacing += (dirs[j as usize] - pi).length();
            }
            spacing /= neighbors[i].len().max(1) as f32;
            let mut disp = world.mantle.velocity[i] * dt_myr as f32;
            let max_disp = CFL_FRACTION * spacing;
            if disp.length() > max_disp {
                disp = disp.normalize_or_zero() * max_disp;
            }
            // Departure point: where this parcel drifted FROM (upstream).
            let departure = (pi - disp).normalize_or_zero();
            // Linear-fit evaluation of the old field at the departure point,
            // bounded by the ring's own range.
            let ti = old[i];
            let grad = tangent_gradient(
                pi,
                neighbors[i].iter().map(|&j| (dirs[j as usize], (old[j as usize] - ti) as f32)),
            );
            let (mut lo, mut hi) = (ti, ti);
            let mut ring_mean = ti;
            for &j in &neighbors[i] {
                let t = old[j as usize];
                lo = lo.min(t);
                hi = hi.max(t);
                ring_mean += t;
            }
            ring_mean /= (neighbors[i].len() + 1) as f64;
            // Bound the fit HARD by the ring's own range — the bound is what
            // keeps compounding overshoot from becoming the §6.1 noise-field
            // blow-up (measured: even a 0.5-range slack roughens exactly like
            // no clamp at all) — then blend in the flat-mean diffusion that
            // keeps the field SMOOTHING over time rather than sharpening.
            let value = (ti + grad.dot(departure - pi) as f64).clamp(lo, hi);
            world.mantle.temp_k[i] = value + RESAMPLE_DIFFUSION * (ring_mean - value);
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
                Box::new(RadiogenicDecay::default()),
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

    /// **The sulfur competition (defect 7E01115B).** Run core formation and
    /// outgassing TOGETHER on the magma-ocean seed: the metal must carry the
    /// sulfur down, not watch the sky strip it. Before the fix this world vented
    /// ~half its S inventory as SO₂ (3.4e8 kg/m² of it — Earth's whole
    /// atmosphere is ~1e4) while the core, formula re-baselining against the
    /// loss, kept less than half its own partition share.
    #[test]
    fn sulfur_sinks_with_the_iron_not_into_the_sky() {
        let mut w = world(4, 7);
        let dir = content_data_dir();
        let t = Tables::from_source(&JsonTableSource::new(&dir)).expect("tables");
        let mut sched = Scheduler::new(
            vec![
                Box::new(RadiogenicDecay::default()),
                Box::new(CoreFormation),
                Box::new(MantleConvection),
                Box::new(crate::atmosphere::Outgassing::new(
                    &t,
                    crate::atmosphere::DEFAULT_OUTGAS_RATE,
                )),
            ],
            7,
        );
        // Through the whole full-rate degassing era (the mean mantle cools past
        // the 3400 K SO₂ floor well inside this window) — audited every tick.
        for _ in 0..200 {
            sched.step(&mut w, 1.0, None);
        }
        let s_total = w.budget.accreted(16);
        let s_core = w.reservoirs.core.amount(16);
        let s_air = w.reservoirs.atmosphere.contents.amount(16);
        assert!(
            s_core / s_total > 0.90,
            "the core carries its sulfur share down: got {:.1}%",
            100.0 * s_core / s_total,
        );
        assert!(
            s_air / s_total < 0.05,
            "the sky gets the residue, not the inventory: got {:.1}%",
            100.0 * s_air / s_total,
        );
        // And the exhale is a real distillation burst, not a sulfur flood: the
        // carbon sky outweighs the sulfur one.
        let air = &w.reservoirs.atmosphere.species;
        assert!(
            air.amount(crate::atmosphere::SULFUR_DIOXIDE)
                < air.amount(crate::atmosphere::CARBON_DIOXIDE),
            "SO₂ ({:.2e} kg) must not dominate CO₂ ({:.2e} kg)",
            air.amount(crate::atmosphere::SULFUR_DIOXIDE),
            air.amount(crate::atmosphere::CARBON_DIOXIDE),
        );
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


    /// Grid-ghost regression at the FIELD level (the pentagon-seam incident):
    /// evolve temperature by convection alone and compare the raw link strain
    /// `|Δv|/gap` across shard edges against interior links. The old
    /// inverse-distance resample kernel took its weights from the ring's
    /// geometry, so its numerical diffusion was anisotropic along the lattice
    /// axes — the shard-edge strain excess grew to ~1.6× by 120 Myr, the
    /// conveyor's yield gate broke along the pentagon-to-pentagon lines, and
    /// the early arc record traced the icosahedron (Aaron's eyeball, 2026-08-05).
    /// The fit-evaluated, ring-clamped, flat-diffused resample holds this near
    /// one; the analytic floor for an honestly sheared smooth field is ~1.15
    /// (link directions cluster at the seams, and real shear is anisotropic).
    /// The lattice must stay invisible in the flow — R5b: no physical field may
    /// correlate with the grid.
    #[test]
    fn convection_strain_ignores_the_shard_edges() {
        let mut w = world(48, 42);
        let mut sched = Scheduler::new(
            vec![Box::new(RadiogenicDecay::default()), Box::new(MantleConvection)],
            42,
        );
        for _ in 0..120 {
            sched.step(&mut w, 1.0, None);
        }
        let vel = &w.mantle.velocity;
        let (mut sx, mut nx, mut si, mut ni) = (0.0f64, 0u64, 0.0f64, 0u64);
        for i in 0..w.mantle.n_cells() {
            for &j in &w.grid.neighbors[i] {
                let j = j as usize;
                if j <= i {
                    continue;
                }
                let gap = (w.grid.dirs[i] - w.grid.dirs[j]).length().max(1e-9);
                let s = ((vel[i] - vel[j]).length() / gap) as f64;
                if w.grid.shard[i] != w.grid.shard[j] {
                    sx += s;
                    nx += 1;
                } else {
                    si += s;
                    ni += 1;
                }
            }
        }
        let ratio = (sx / nx.max(1) as f64) / (si / ni.max(1) as f64).max(1e-300);
        assert!(
            ratio < 1.35,
            "shard-edge strain excess regressed: {ratio:.3} — the resample (or the \
             velocity derivation) is printing the lattice into the flow again",
        );
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
