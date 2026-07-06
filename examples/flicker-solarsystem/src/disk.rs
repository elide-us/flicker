//! The protoplanetary disk — the analytic *starting condition* the formation sim
//! grows planets out of.
//!
//! Two grounded pieces, both in **AU**:
//! - A **solid surface density** `Σ(r)` (an MMSN-style `r^-3/2` power law with the
//!   **snow-line jump** — solids roughly quadruple past ~2.7 AU as water ice
//!   condenses, which is *why* the giant cores grow out there), and a
//!   **composition-by-radius** from the condensation sequence (refractory metal +
//!   silicate + carbon inner; water/volatile ice only past the snow line).
//! - **Embryo seeding**: tile the disk into Hill-spaced feeding zones and collapse
//!   each zone's integrated solids into one embryo of the local composition — the
//!   ~Moon-to-Earth-mass population the giant-impact phase starts from. Past the
//!   snow line, a massive-enough core captures an H/He envelope and seeds as a
//!   **giant** (the dominant scatterer once the gas is gone).
//!
//! A separate lightweight **background field** of blobs is kept purely for the
//! render (a decaying dust nebula); the physics runs off the bodies, not the blobs.

use std::f64::consts::TAU;

use glam::DVec3;

use crate::astro::{AU_CM, M_STAR, M_SUN_G};
use crate::body::{circular_speed, Body, BodyKind};
use crate::material::{Composition, MaterialClass};

/// Inner edge of the modelled disk (AU) — inside this the gas is too hot / the star
/// clears it.
pub const DISK_INNER: f64 = 0.3;
/// Outer edge of the modelled disk (AU). Kept to the dynamically active giant-forming
/// region (out to ~Saturn): a body past ~15 AU completes only a couple of orbits in the
/// compressed run, so it can't evolve/merge and would just linger as un-cleared clutter.
pub const DISK_OUTER: f64 = 15.0;
/// Snow line (AU) — water ice condenses beyond it; the solid-density jump that lets
/// giant cores grow.
pub const SNOW_LINE: f64 = 2.7;
/// Habitable zone (AU) for a Sun-like star — the *conservative* liquid-water annulus
/// (runaway-greenhouse inner edge to maximum-greenhouse outer edge, ~Kopparapu 2013).
pub const HZ_INNER: f64 = 0.95;
pub const HZ_OUTER: f64 = 1.37;

/// The minimum-mass solar nebula's solid surface density at 1 AU (g/cm², Hayashi 1981) —
/// a reference point, not what any single system uses.
pub const SIGMA_MMSN: f64 = 7.0;
/// Disk solid surface density at 1 AU (g/cm²) spans this range across systems, set by the
/// seeding supernova ([`Nebula`]) — roughly ~0.4–5× the MMSN. The MMSN is only the
/// *minimum* reconstruction for **our** system, not a floor for disks at large: observed
/// disks are frequently sub-MMSN, and those low-mass disks can't assemble a habitable-mass
/// world; the upper end is heavy enough to make the inner system giant-dominated. Where a
/// given disk lands — and whether it yields a habitable world — is emergent, not aimed at.
const SIGMA_MIN: f64 = 3.0;
const SIGMA_MAX: f64 = 36.0;
/// Metallicity (heavy-element enrichment) relative to solar spans this range. Higher
/// metallicity makes giant cores form more readily — the observed giant-planet/metallicity
/// correlation — by lowering the gas-capture core threshold (see [`Nebula::crit_core`]).
const Z_MIN: f64 = 0.5;
const Z_MAX: f64 = 2.2;
/// Multiplier on solids past the snow line (ice condensation).
const ICE_BOOST: f64 = 4.2;
/// Feeding-zone width in mutual Hill radii (oligarchic spacing) — the range each forming
/// embryo sweeps clean of solids. Wider → fewer, chunkier embryos, which interact more
/// strongly and merge more decisively (so the system settles to fewer final bodies).
/// At the upper end of the oligarchic literature (~5–10 R_Hill, Kokubo & Ida).
const SPACING_B: f64 = 10.0;
/// Core mass (M☉) past the snow line needed to capture a gas envelope — the canonical
/// **critical core mass ~10 M⊕** for runaway gas accretion (Mizuno 1980; Pollack et al.
/// 1996), at which the envelope can no longer stay in hydrostatic balance and gas runs in.
/// (Was 6 M⊕ — too low, so far too many marginal cores qualified.) Metallicity doesn't gate
/// this directly; it raises the *solid* density, so metal-rich disks grow bigger cores and
/// cross it more readily — the real correlation, working through core mass. How many cross it
/// is still emergent; the **gas budget** below — not a count cap — limits how many get gas.
const CRIT_CORE: f64 = 10.0 * crate::astro::M_EARTH;
/// Core mass (M☉) above which gas accretion runs away → a true gas giant (vs a modest-
/// envelope ice giant below it).
const RUNAWAY_CORE: f64 = 12.0 * crate::astro::M_EARTH;
/// Envelope mass as a multiple of the core: small for ice giants, large (runaway) for gas
/// giants. Each is jittered per body so giants aren't clones. Capped near a few Jupiters.
const ICE_GIANT_ENVELOPE: f64 = 1.5;
const GAS_GIANT_ENVELOPE: f64 = 22.0;
const ENVELOPE_CAP: f64 = 380.0 * crate::astro::M_EARTH;
/// Nebular gas-to-solid mass ratio (~solar; Hayashi 1981). The disk's H/He gas tracks the
/// **base** surface density (metallicity scales the dust/solids, not the gas).
const GAS_TO_SOLID: f64 = 100.0;
/// Fraction of the disk's gas that actually ends up in giant envelopes — the rest is accreted
/// by the star or photoevaporated before/while cores grow. Small and observationally
/// uncertain; this finite reservoir is *why giants are rare*. The dominant lever on giant
/// count — turn it down for fewer giants, up for more. (Replaces the old "promote every
/// past-snow-line core" rule that produced unphysical 6–12-giant systems.)
const GAS_CAPTURE_EFFICIENCY: f64 = 0.20;
/// Minimum envelope (M☉) for a core to count as a giant; once the reservoir drops below this
/// the remaining cores stay bare (failed cores / large rocky-icy bodies).
const MIN_GIANT_ENVELOPE: f64 = 1.0 * crate::astro::M_EARTH;
/// A small inner-disk water reservoir (hydrated silicates / water adsorbed on grains),
/// ramping up toward the snow line. A real and genuinely debated contributor to inner-
/// planet water — alongside later delivery from beyond the snow line — and uncertain in
/// magnitude; kept deliberately small.
const HYDRATION_FLOOR: f64 = 0.03;

/// The initial conditions a system is born with — set by the **supernova** that seeded
/// its molecular cloud. A single dial, `supernova_size` ∈ `[0, 1]`, scales how much
/// material and enrichment the cloud got: a bigger event means a heavier disk (more solid
/// surface density) and a more metal-rich one (giant cores form more easily). It's the
/// master source of system-to-system diversity — drawn fresh per system, and adjustable.
#[derive(Copy, Clone, Debug)]
pub struct Nebula {
    /// The seeding supernova's size, `0` (small/sparse) .. `1` (large/enriched).
    pub supernova_size: f64,
    /// Resulting solid surface density at 1 AU (g/cm²).
    pub sigma_1au: f64,
    /// Resulting metallicity relative to solar.
    pub metallicity: f64,
}

impl Nebula {
    /// Build the nebula for `seed` at the given `supernova_size` (clamped to `[0,1]`).
    /// Disk mass rises log-uniformly with supernova size; metallicity rises with it too
    /// but carries independent per-seed scatter (the enrichment of the specific pocket of
    /// cloud), so two equally-sized supernovae still seed chemically different disks.
    pub fn new(seed: u64, supernova_size: f64) -> Self {
        let s = supernova_size.clamp(0.0, 1.0);
        let sigma_1au = SIGMA_MIN * (SIGMA_MAX / SIGMA_MIN).powf(s);
        let mut rng = Rng::new(seed ^ 0x5E7A_11C0_0DEE_F00D);
        // Metallicity: ~60% from supernova size, ~40% independent scatter; log-spaced.
        let z_t = (0.6 * s + 0.4 * rng.f64()).clamp(0.0, 1.0);
        let metallicity = Z_MIN * (Z_MAX / Z_MIN).powf(z_t);
        Self {
            supernova_size: s,
            sigma_1au,
            metallicity,
        }
    }

    /// Solid surface density at 1 AU (g/cm²) actually available to build bodies — the base
    /// disk density scaled by metallicity (heavy-element / dust enrichment). This is how a
    /// metal-rich disk grows bigger cores and thus more giants.
    pub fn solid_sigma(&self) -> f64 {
        self.sigma_1au * self.metallicity
    }
}

/// A plausible random supernova size for `seed` — the per-system default before any manual
/// dialing. Independent of [`Nebula::new`]'s scatter stream.
pub fn random_supernova(seed: u64) -> f64 {
    Rng::new(seed ^ 0x0000_50FA_5152_E001).f64()
}

/// Solid surface density (g/cm²) at heliocentric radius `r` (AU) for a disk of 1-AU
/// density `sigma_1au`: an `r^-3/2` power law with the snow-line jump (smoothed over a
/// narrow band so it isn't a hard cliff).
pub fn solid_surface_density(r: f64, sigma_1au: f64) -> f64 {
    if r <= 0.0 {
        return 0.0;
    }
    let base = sigma_1au * r.powf(-1.5);
    // Ramp the ice boost on across a thin band at the snow line.
    let ramp = smoothstep(SNOW_LINE - 0.15, SNOW_LINE + 0.15, r);
    base * (1.0 + (ICE_BOOST - 1.0) * ramp)
}

/// Total solid mass (M☉) in the annulus `[r0, r1]` (AU), `∫ Σ(r)·2πr dr`, integrated
/// numerically so the snow-line ramp is captured.
pub fn annulus_solid_mass(r0: f64, r1: f64, sigma_1au: f64) -> f64 {
    if r1 <= r0 {
        return 0.0;
    }
    let steps = 64;
    let dr = (r1 - r0) / steps as f64;
    let mut sum_g = 0.0;
    for i in 0..steps {
        let r = r0 + (i as f64 + 0.5) * dr; // midpoint, AU
        let r_cm = r * AU_CM;
        let dr_cm = dr * AU_CM;
        sum_g += solid_surface_density(r, sigma_1au) * TAU * r_cm * dr_cm;
    }
    sum_g / M_SUN_G
}

/// Solid composition (class fractions, summing to 1) at radius `r` (AU) from the
/// condensation sequence. Metal/silicate/carbon are the refractory solids inside the
/// snow line; ice condenses (and quickly dominates the solids) beyond it. Gas is
/// *not* here — it's an envelope, added only to giants.
pub fn composition_fractions(r: f64) -> [f64; 5] {
    // Refractory split (used everywhere): metal richest inner, carbon peaks mid.
    let metal = (0.34 - 0.05 * r).clamp(0.08, 0.36);
    let carbon = (0.04 + 0.015 * r).clamp(0.04, 0.12);
    let silicate = (1.0 - metal - carbon).max(0.0);
    // Ice fraction of the *solids*: a trace hydration floor inside the snow line that
    // ramps up toward it, then the big jump as water ice condenses past it.
    let hydration = HYDRATION_FLOOR * smoothstep(0.7, SNOW_LINE, r);
    let ice_share = hydration.max(0.6 * smoothstep(SNOW_LINE - 0.15, SNOW_LINE + 0.8, r));

    let refractory = 1.0 - ice_share;
    let mut f = [0.0; 5];
    f[MaterialClass::Metal.index()] = metal * refractory;
    f[MaterialClass::Silicate.index()] = silicate * refractory;
    f[MaterialClass::Carbon.index()] = carbon * refractory;
    f[MaterialClass::Ice.index()] = ice_share;
    f[MaterialClass::Gas.index()] = 0.0;
    f
}

/// A [`Composition`] of total mass `mass` (M☉) distributed by the solid composition
/// at radius `r`.
pub fn composition_at(r: f64, mass: f64) -> Composition {
    let f = composition_fractions(r);
    let mut c = Composition::new();
    for &class in &MaterialClass::ALL {
        c.mass[class.index()] = f[class.index()] * mass;
    }
    c
}

/// Isolation-mass estimate (M☉) at `r` for surface density `sigma` (g/cm²) — the
/// mass a body reaches sweeping its `SPACING_B`-Hill-radius feeding zone. Solved from
/// `M_iso = 2π r · (b·R_Hill) · Σ` with `R_Hill = r(2M/3M☉)^{1/3}`.
fn isolation_mass(r: f64, sigma: f64) -> f64 {
    let r_cm = r * AU_CM;
    let sigma_term = sigma * TAU * r_cm * r_cm; // 2π r² Σ in g (per unit width factor)
    let hill_term = SPACING_B * (2.0 / (3.0 * M_STAR)).cbrt();
    // M^{2/3} = sigma_term/M_SUN_G · hill_term  ⇒ M = (...)^{3/2}.
    let m23 = (sigma_term / M_SUN_G) * hill_term;
    m23.max(0.0).powf(1.5)
}

/// Seed the disk's embryos (and any giants) for the given [`Nebula`]: tile
/// `[DISK_INNER, DISK_OUTER]` into Hill-spaced feeding zones, collapse each zone's solids
/// into one body of the local composition (placed at a random angle with a mild
/// eccentricity/inclination so the system self-stirs into crossing orbits), then let
/// whichever cores cross the gas-capture threshold become giants. Deterministic per `seed`.
pub fn seed_embryos(seed: u64, nebula: &Nebula) -> Vec<Body> {
    let sigma_1au = nebula.solid_sigma();
    let mut rng = Rng::new(seed ^ 0x501A_25ED_F00D_BEEF);
    let mut bodies = Vec::new();

    let mut r = DISK_INNER;
    while r < DISK_OUTER {
        let sigma = solid_surface_density(r, sigma_1au);
        let m_guess = isolation_mass(r, sigma).max(1.0e-9);
        let r_hill = r * (2.0 * m_guess / (3.0 * M_STAR)).cbrt();
        let width = (SPACING_B * r_hill).max(0.02);
        let r0 = r;
        let r1 = (r + width).min(DISK_OUTER);
        if r1 - r0 < 1.0e-4 {
            break;
        }
        let center = 0.5 * (r0 + r1);
        let solids = annulus_solid_mass(r0, r1, sigma_1au);
        if solids > 1.0e-10 {
            bodies.push(make_embryo(center, solids, &mut rng));
        }
        r = r1;
    }

    promote_giants(&mut bodies, nebula, &mut rng);
    bodies
}

/// Total nebular **gas** mass (M☉) in the modelled disk: the base surface-density power law
/// (no ice boost — gas doesn't condense) times the gas-to-solid ratio. This is the reservoir
/// giants draw their envelopes from.
fn disk_gas_mass(sigma_1au: f64) -> f64 {
    let steps = 128;
    let dr = (DISK_OUTER - DISK_INNER) / steps as f64;
    let mut sum_g = 0.0;
    for i in 0..steps {
        let r = DISK_INNER + (i as f64 + 0.5) * dr; // AU
        let sigma_gas = GAS_TO_SOLID * sigma_1au * r.powf(-1.5); // g/cm²
        sum_g += sigma_gas * TAU * (r * AU_CM) * (dr * AU_CM);
    }
    sum_g / M_SUN_G
}

/// Build one solid embryo at `center` (AU) holding `solids` (M☉) of local composition,
/// on a mildly eccentric/inclined orbit. (Giant envelopes are added later, to the few
/// most massive cores only — see [`promote_giants`].)
fn make_embryo(center: f64, solids: f64, rng: &mut Rng) -> Body {
    let comp = composition_at(center, solids);
    let mass = comp.total();

    // Orbit: random phase, in-plane tangential velocity with radial (e) and vertical (i)
    // perturbations. We seed the embryos **already dynamically excited** — at the
    // eccentricities (~0.1) a swarm reaches *entering* the giant-impact phase — because the
    // slow mutual stirring that pumps them there takes far more orbits than the compressed
    // run covers. Starting on crossing orbits, they collide and merge straight away, and
    // the eccentricities damp back down through those very collisions, as in reality.
    let theta = rng.f64() * TAU;
    let pos = DVec3::new(center * theta.cos(), 0.0, center * theta.sin());
    let radial = pos.normalize();
    let tangential = DVec3::new(-theta.sin(), 0.0, theta.cos());
    let vc = circular_speed(center, mass);
    let e = (rng.f64() - 0.5) * 0.24; // |e| ≲ 0.12
    let inc = (rng.f64() - 0.5) * 0.08;
    let vel = tangential * vc + radial * (e * vc) + DVec3::Y * (inc * vc);

    Body::new(pos, vel, comp, BodyKind::Protoplanet)
}

/// Promote cores to giants — **emergently**, gated by a finite gas budget. Past-snow-line
/// cores above the gas-capture threshold are candidates; the disk holds only so much gas
/// (`GAS_CAPTURE_EFFICIENCY` of its total), and the cores nearest the snow line reach
/// critical mass **first** — while the gas is still there — so they win it and become the
/// gas giants. Processed inner→outer, each draws its envelope from the reservoir until it's
/// spent; remaining cores stay bare (failed cores → ice-giant cores / large icy bodies, as
/// Uranus/Neptune did). So giant *count, kind, and mass* fall out of disk mass + metallicity
/// + how much gas there was to go around — a light disk makes none, a heavy one only a few.
fn promote_giants(bodies: &mut [Body], nebula: &Nebula, rng: &mut Rng) {
    let mut gas_budget = GAS_CAPTURE_EFFICIENCY * disk_gas_mass(nebula.sigma_1au);

    // Candidate cores (past the snow line, above the critical mass), inner→outer — the order
    // they reach critical mass and tap the still-present nebular gas.
    let mut idx: Vec<usize> = (0..bodies.len())
        .filter(|&i| bodies[i].helio_radius() > SNOW_LINE && bodies[i].mass > CRIT_CORE)
        .collect();
    idx.sort_by(|&a, &b| bodies[a].helio_radius().total_cmp(&bodies[b].helio_radius()));

    for i in idx {
        if gas_budget < MIN_GIANT_ENVELOPE {
            break; // the gas is gone — the remaining outer cores stay bare
        }
        let core = bodies[i].mass;
        let jitter = 0.5 + rng.f64(); // 0.5..1.5 — giants aren't clones
        let desired = if core > RUNAWAY_CORE {
            (GAS_GIANT_ENVELOPE * core * jitter).min(ENVELOPE_CAP)
        } else {
            ICE_GIANT_ENVELOPE * core * jitter
        };
        let envelope = desired.min(gas_budget); // only what the reservoir still holds
        gas_budget -= envelope;
        bodies[i].comp.add(&Composition::of(MaterialClass::Gas, envelope));
        bodies[i].sync_mass();
        bodies[i].kind = BodyKind::Giant;
    }
}

// --- helpers ----------------------------------------------------------------

/// A tiny soft-disc RGBA texture (radial alpha falloff) for blob/star billboards.
pub fn disc_texture() -> Vec<u8> {
    const S: usize = 16;
    let mut px = vec![0u8; S * S * 4];
    let c = (S as f32 - 1.0) * 0.5;
    for y in 0..S {
        for x in 0..S {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt() / c;
            let a = 1.0 - smoothstep_f32(0.55, 1.0, d);
            let i = (y * S + x) * 4;
            px[i] = 255;
            px[i + 1] = 255;
            px[i + 2] = 255;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}

/// A hollow soft-ring RGBA texture (annulus alpha, bright near the rim, clear inside) for
/// circling a body — used to mark viable starter worlds.
pub fn ring_texture() -> Vec<u8> {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4];
    let c = (S as f32 - 1.0) * 0.5;
    for y in 0..S {
        for x in 0..S {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt() / c; // 0 centre … 1 edge
            // Peak alpha in a band around d≈0.8, zero inside and at the very edge.
            let inner = smoothstep_f32(0.62, 0.80, d);
            let outer = 1.0 - smoothstep_f32(0.80, 0.98, d);
            let a = (inner * outer).clamp(0.0, 1.0);
            let i = (y * S + x) * 4;
            px[i] = 255;
            px[i + 1] = 255;
            px[i + 2] = 255;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smoothstep_f32(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// splitmix64 → `f64` in `[0, 1)`. Deterministic per seed.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astro::earth_masses;

    #[test]
    fn surface_density_jumps_across_the_snow_line() {
        let s = SIGMA_MMSN;
        let inside = solid_surface_density(SNOW_LINE - 0.5, s);
        let outside = solid_surface_density(SNOW_LINE + 0.5, s);
        // Past the snow line solids jump up despite the r^-3/2 falloff.
        assert!(outside > inside, "ice boost raises solids past the snow line");
        assert!(solid_surface_density(1.0, s) > 0.0);
    }

    #[test]
    fn supernova_sets_disk_mass_and_metallicity() {
        let small = Nebula::new(1, 0.1);
        let big = Nebula::new(1, 0.9);
        // A bigger supernova → heavier, more metal-rich disk → more solids to build cores.
        assert!(big.sigma_1au > small.sigma_1au, "bigger nova → heavier disk");
        assert!(big.metallicity > small.metallicity, "bigger nova → more metals");
        assert!(big.solid_sigma() > small.solid_sigma(), "metal-rich → more solids → more giants");
        // All within range; clamped; deterministic.
        assert!((SIGMA_MIN..=SIGMA_MAX).contains(&big.sigma_1au));
        assert!((Z_MIN..=Z_MAX).contains(&big.metallicity));
        assert_eq!(Nebula::new(1, 0.5).sigma_1au, Nebula::new(1, 0.5).sigma_1au);
        // Same nova size, different seeds → different metallicity (independent scatter).
        assert!((Nebula::new(1, 0.5).metallicity - Nebula::new(2, 0.5).metallicity).abs() > 1e-9);
    }

    #[test]
    fn composition_is_dry_inside_and_icy_outside() {
        let hot = composition_fractions(0.4); // close to the star
        let hz = composition_fractions(1.0); // habitable zone
        let outer = composition_fractions(5.0); // past the snow line
        let ice = MaterialClass::Ice.index();
        assert!(hot[ice] < 1e-6, "bone dry right by the star");
        // The HZ carries only *trace* water (hydrated silicates), far less than the
        // icy outer disk.
        assert!(hz[ice] > 0.0 && hz[ice] < 0.02, "HZ has trace water, got {}", hz[ice]);
        assert!(outer[ice] > 0.3, "ice dominates outer solids");
        assert!(hz[ice] < outer[ice]);
        // Fractions sum to 1 at every radius.
        for r in [0.4, 1.0, 2.7, 5.0, 20.0] {
            let s: f64 = composition_fractions(r).iter().sum();
            assert!((s - 1.0).abs() < 1e-9, "fractions sum to 1 at {r} AU");
        }
    }

    #[test]
    fn seeding_is_deterministic_and_sane() {
        let neb = Nebula::new(42, 0.6);
        let a = seed_embryos(42, &neb);
        let b = seed_embryos(42, &neb);
        assert_eq!(a.len(), b.len(), "same seed+nebula → same embryo count");
        assert!(a.len() >= 8, "a populated disk, got {}", a.len());
        for body in &a {
            let r = body.helio_radius();
            assert!(r >= DISK_INNER - 0.05 && r <= DISK_OUTER + 0.05, "embryo within disk");
            assert!(body.mass > 0.0, "embryo has mass");
            // Masses are planetary-scale, not absurd.
            assert!(earth_masses(body.mass) < 5000.0, "embryo not absurdly massive");
        }
    }

    #[test]
    fn giants_form_past_the_snow_line_only() {
        // A heavy, metal-rich disk so giants reliably form for the test.
        let bodies = seed_embryos(7, &Nebula::new(7, 0.95));
        let giants = bodies.iter().filter(|b| b.kind == BodyKind::Giant).count();
        assert!(giants > 0, "a heavy disk forms at least one giant");
        for b in &bodies {
            if b.kind == BodyKind::Giant {
                assert!(b.helio_radius() > SNOW_LINE, "giants only past the snow line");
                assert!(b.comp.get(MaterialClass::Gas) > 0.0, "giant has an envelope");
            }
        }
    }

    #[test]
    fn giant_count_emerges_from_disk_mass() {
        // A light, metal-poor disk should make far fewer giants than a heavy metal-rich one.
        let light = seed_embryos(3, &Nebula::new(3, 0.05));
        let heavy = seed_embryos(3, &Nebula::new(3, 0.98));
        let g = |v: &[Body]| v.iter().filter(|b| b.kind == BodyKind::Giant).count();
        assert!(g(&heavy) >= g(&light), "heavier/metal-richer disks make more giants");
    }

    #[test]
    fn gas_budget_keeps_giants_few() {
        // A very heavy, metal-rich disk has many past-snow-line cores above the critical
        // mass — but the finite gas reservoir only lets the innermost few become giants, not
        // all of them (the old ungated rule promoted every one → 6–12-giant systems).
        let neb = Nebula::new(11, 1.0);
        let bodies = seed_embryos(11, &neb);
        let giants = bodies.iter().filter(|b| b.kind == BodyKind::Giant).count();
        // Cores massive enough to have captured gas under the old rule (core = mass − envelope).
        let eligible = bodies
            .iter()
            .filter(|b| b.helio_radius() > SNOW_LINE && (b.mass - b.comp.get(MaterialClass::Gas)) > CRIT_CORE)
            .count();
        assert!(giants >= 1, "a heavy disk still forms a few giants, got {giants}");
        assert!(giants <= 5, "even a max disk stays in a realistic giant range, got {giants}");
        assert!(giants < eligible, "the gas budget gates giants: {giants} formed of {eligible} eligible cores");
    }
}
