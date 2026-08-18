//! The protoplanetary disk — the **analytic field** a system is born with, and the
//! source the conserved [`Cloud`](crate::model::Cloud) is materialised from.
//!
//! Ported from the formation-sim POC (`examples/flicker-solarsystem/src/disk.rs`),
//! retargeted onto the celestial model's [`ClassComposition`]. Two grounded pieces,
//! both in **AU**:
//!
//! - a **solid surface density** `Σ(r)` (an MMSN-style `r^-3/2` power law with the
//!   **snow-line jump** — solids roughly quadruple past ~2.7 AU as water ice
//!   condenses, which is why giant cores grow out there); and
//! - a **composition-by-radius** from the condensation sequence (refractory
//!   metal + silicate + carbon inner; water/volatile ice past the snow line).
//!
//! The disk model is for a **Sun-like** star (the favourable, modelled case): masses
//! are in solar masses with `M☉ = 1`. This module is *pure analytic* — it is the
//! statistical distribution; [`materialize_cloud`](super::materialize_cloud) turns it
//! into the discrete, conserved reservoir bodies actually draw from.

use std::f64::consts::TAU;

use crate::model::{ClassComposition, CondensationClass};
use crate::units::{AU_CM, M_SUN_G};

/// Inner edge of the modelled disk (AU) — inside this the gas is too hot / the star
/// clears it.
pub const DISK_INNER: f64 = 0.3;
/// Outer edge of the modelled disk (AU) — the dynamically active giant-forming region
/// (out to ~Saturn); bodies past it barely evolve in a compressed run.
pub const DISK_OUTER: f64 = 15.0;
/// Snow line (AU) — water ice condenses beyond it; the solid-density jump that lets
/// giant cores grow.
pub const SNOW_LINE: f64 = 2.7;

/// Disk solid surface density at 1 AU (g/cm²) spans this range across systems, set by
/// the seeding supernova ([`Nebula`]) — roughly ~0.4–5× the MMSN.
const SIGMA_MIN: f64 = 3.0;
const SIGMA_MAX: f64 = 36.0;
/// Metallicity (heavy-element enrichment) relative to solar spans this range — higher
/// metallicity grows bigger cores and thus more giants (the observed correlation).
const Z_MIN: f64 = 0.5;
const Z_MAX: f64 = 2.2;
/// Multiplier on solids past the snow line (ice condensation).
const ICE_BOOST: f64 = 4.2;
/// A small inner-disk water reservoir (hydrated silicates) ramping up toward the snow
/// line — a real, debated contributor to inner-planet water; kept deliberately small.
const HYDRATION_FLOOR: f64 = 0.03;
/// Nebular gas-to-solid mass ratio (~solar; Hayashi 1981) — the disk's H/He gas tracks
/// the *base* surface density (metallicity scales the dust/solids, not the gas).
const GAS_TO_SOLID: f64 = 100.0;

/// The initial conditions a system is born with — set by the **supernova** that seeded
/// its molecular cloud. A single dial `supernova_size` ∈ `[0, 1]` scales how much
/// material and enrichment the cloud got: a bigger event means a heavier, more
/// metal-rich disk. The master source of system-to-system diversity.
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
    /// but carries independent per-seed scatter (the enrichment of the specific pocket
    /// of cloud).
    pub fn new(seed: u64, supernova_size: f64) -> Self {
        let s = supernova_size.clamp(0.0, 1.0);
        let sigma_1au = SIGMA_MIN * (SIGMA_MAX / SIGMA_MIN).powf(s);
        let mut rng = Rng::new(seed ^ 0x5E7A_11C0_0DEE_F00D);
        let z_t = (0.6 * s + 0.4 * rng.f64()).clamp(0.0, 1.0);
        let metallicity = Z_MIN * (Z_MAX / Z_MIN).powf(z_t);
        Self {
            supernova_size: s,
            sigma_1au,
            metallicity,
        }
    }

    /// Solid surface density at 1 AU (g/cm²) actually available to build bodies — the
    /// base disk density scaled by metallicity. How a metal-rich disk grows bigger
    /// cores (and so more giants).
    pub fn solid_sigma(&self) -> f64 {
        self.sigma_1au * self.metallicity
    }

    /// Total nebular **gas** mass (M☉) in the modelled disk — the base power law (no ice
    /// boost; gas doesn't condense) × the gas-to-solid ratio. The reservoir giant
    /// envelopes draw from. Tracked **separately** from the solid [`Cloud`](crate::model::Cloud):
    /// gas is an envelope captured by massive cores, not a solid that condenses into
    /// every body. (Giant formation — the consumer of this — is a later slice.)
    pub fn disk_gas_mass(&self) -> f64 {
        let steps = 128;
        let dr = (DISK_OUTER - DISK_INNER) / steps as f64;
        let mut sum_g = 0.0;
        for i in 0..steps {
            let r = DISK_INNER + (i as f64 + 0.5) * dr;
            let sigma_gas = GAS_TO_SOLID * self.sigma_1au * r.powf(-1.5);
            sum_g += sigma_gas * TAU * (r * AU_CM) * (dr * AU_CM);
        }
        sum_g / M_SUN_G
    }
}

/// A plausible random supernova size for `seed` — the per-system default before any
/// manual dialing. Independent of [`Nebula::new`]'s scatter stream.
pub fn random_supernova(seed: u64) -> f64 {
    Rng::new(seed ^ 0x0000_50FA_5152_E001).f64()
}

/// Solid surface density (g/cm²) at heliocentric radius `r` (AU) for a disk of 1-AU
/// density `sigma_1au`: an `r^-3/2` power law with the snow-line jump (smoothed).
pub fn solid_surface_density(r: f64, sigma_1au: f64) -> f64 {
    if r <= 0.0 {
        return 0.0;
    }
    let base = sigma_1au * r.powf(-1.5);
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
        let r = r0 + (i as f64 + 0.5) * dr;
        let r_cm = r * AU_CM;
        let dr_cm = dr * AU_CM;
        sum_g += solid_surface_density(r, sigma_1au) * TAU * r_cm * dr_cm;
    }
    sum_g / M_SUN_G
}

/// Solid composition (class fractions, summing to 1) at radius `r` (AU) from the
/// condensation sequence. Metal/silicate/carbon are the refractory solids inside the
/// snow line; ice condenses (and quickly dominates) beyond it. Gas is **not** here — it
/// is an envelope, added only to giants. Indexed by [`CondensationClass::index`].
pub fn composition_fractions(r: f64) -> [f64; 5] {
    let metal = (0.34 - 0.05 * r).clamp(0.08, 0.36);
    let carbon = (0.04 + 0.015 * r).clamp(0.04, 0.12);
    let silicate = (1.0 - metal - carbon).max(0.0);
    let hydration = HYDRATION_FLOOR * smoothstep(0.7, SNOW_LINE, r);
    let ice_share = hydration.max(0.6 * smoothstep(SNOW_LINE - 0.15, SNOW_LINE + 0.8, r));

    let refractory = 1.0 - ice_share;
    let mut f = [0.0; 5];
    f[CondensationClass::Metal.index()] = metal * refractory;
    f[CondensationClass::Silicate.index()] = silicate * refractory;
    f[CondensationClass::Carbon.index()] = carbon * refractory;
    f[CondensationClass::Ice.index()] = ice_share;
    f[CondensationClass::Gas.index()] = 0.0;
    f
}

/// A [`ClassComposition`] of total mass `mass` (M☉) distributed by the solid
/// composition at radius `r` — the material a cloud ring at `r` holds.
pub fn class_composition_at(r: f64, mass: f64) -> ClassComposition {
    let f = composition_fractions(r);
    let mut c = ClassComposition::new();
    for &class in &CondensationClass::ALL {
        c.add_class(class, f[class.index()] * mass);
    }
    c
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// splitmix64 → `f64` in `[0, 1)`. Deterministic per seed — the formation RNG.
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

    #[test]
    fn surface_density_jumps_across_the_snow_line() {
        let s = 7.0;
        let inside = solid_surface_density(SNOW_LINE - 0.5, s);
        let outside = solid_surface_density(SNOW_LINE + 0.5, s);
        assert!(
            outside > inside,
            "ice boost raises solids past the snow line"
        );
        assert!(solid_surface_density(1.0, s) > 0.0);
    }

    #[test]
    fn supernova_sets_disk_mass_and_metallicity() {
        let small = Nebula::new(1, 0.1);
        let big = Nebula::new(1, 0.9);
        assert!(
            big.sigma_1au > small.sigma_1au,
            "bigger nova → heavier disk"
        );
        assert!(
            big.metallicity > small.metallicity,
            "bigger nova → more metals"
        );
        assert!(big.solid_sigma() > small.solid_sigma());
        assert!((SIGMA_MIN..=SIGMA_MAX).contains(&big.sigma_1au));
        assert!((Z_MIN..=Z_MAX).contains(&big.metallicity));
        // Deterministic; independent metallicity scatter per seed.
        assert_eq!(Nebula::new(1, 0.5).sigma_1au, Nebula::new(1, 0.5).sigma_1au);
        assert!((Nebula::new(1, 0.5).metallicity - Nebula::new(2, 0.5).metallicity).abs() > 1e-9);
    }

    #[test]
    fn composition_is_dry_inside_and_icy_outside() {
        let ice = CondensationClass::Ice.index();
        assert!(
            composition_fractions(0.4)[ice] < 1e-6,
            "bone dry by the star"
        );
        let hz = composition_fractions(1.0)[ice];
        assert!(hz > 0.0 && hz < 0.02, "HZ has only trace water, got {hz}");
        assert!(
            composition_fractions(5.0)[ice] > 0.3,
            "ice dominates outer solids"
        );
        for r in [0.4, 1.0, 2.7, 5.0, 14.0] {
            let s: f64 = composition_fractions(r).iter().sum();
            assert!((s - 1.0).abs() < 1e-9, "fractions sum to 1 at {r} AU");
        }
    }

    #[test]
    fn class_composition_at_carries_the_radius_mix() {
        // Past the snow line a ring's material is ice-dominant; its total is the mass.
        let c = class_composition_at(6.0, 10.0e-6);
        assert!(
            (c.total() - 10.0e-6).abs() < 1e-15,
            "conserves the ring mass"
        );
        assert_eq!(
            c.dominant(),
            Some(CondensationClass::Ice),
            "icy past the snow line"
        );
    }
}
