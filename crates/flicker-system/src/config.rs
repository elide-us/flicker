//! The simulation's **input contract** — every lever, in one place.
//!
//! [`SystemConfig`] is what the UI (or a constraint search, or a test) fills in and hands to a
//! [`System`](crate::System): the Phase-1 distribution dials, the Phase-2 sampling, and a
//! [`Tuning`] block holding the collapse physics / body-typing / playability levers. The defaults
//! reproduce the confirmed Sol-like regime, so `SystemConfig::default()` is the canonical system.

use crate::mass::MassParams;
use crate::model::CastParams;

/// The default cloud seed (the confirmed-good system used throughout development).
pub const DEFAULT_SEED: u32 = 0xC10D_5EED;
/// Default motes sampled per element when igniting the collapse — the count of "potential
/// bodies" Stage 2 starts with (per element, drawn from the clump CDF; total ≈ this × element
/// count). Halved from 24 → 12 (2026-06-24): not every potential needs selecting for the
/// collapse. Conservation-safe — each parcel just carries proportionally more of its element's
/// tonnage. Lower = fewer/chunkier seeds; raise for a finer disk.
pub const DEFAULT_MOTES_PER_EL: usize = 12;
/// Default cloud clump strength (matches the confirmed distribution view).
pub const DEFAULT_CLUMP: f32 = 0.6;

/// Every lever the simulation exposes, as one struct. Phase-1 (distribution) + Phase-2 (collapse).
#[derive(Clone, Copy, Debug)]
pub struct SystemConfig {
    /// Cast model — how far each element is flung (`explosion` reach, atomic-weight `falloff`).
    pub cast: CastParams,
    /// Conserved mass layer — total cloud `tonnage` and `metallicity` (gas-to-metal balance).
    pub mass: MassParams,
    /// Cloud clump strength (0 → smooth rings, higher → lumpier overdensities / hot spots).
    pub clump: f32,
    /// Reproducible cloud seed — reseeding yields a genuinely different system.
    pub seed: u32,
    /// Motes sampled per element at ignition (collapse resolution / body count).
    pub motes_per_el: usize,
    /// Collapse physics, body-typing, and playability levers.
    pub tuning: Tuning,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            cast: CastParams::default(),
            mass: MassParams::default(),
            clump: DEFAULT_CLUMP,
            seed: DEFAULT_SEED,
            motes_per_el: DEFAULT_MOTES_PER_EL,
            tuning: Tuning::default(),
        }
    }
}

/// The collapse's physical levers — gravity / drag / accretion / satellite / typing / playability.
/// Defaults reproduce the confirmed Sol-like regime; these are the dials the user adjusts visually.
/// (Physical *constants* — `G`, the g/cc→M☉/AU³ conversion, the integration substep — are NOT
/// here; they are not design levers.)
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Plummer softening (AU) — finite gravity at tiny separations.
    pub softening: f32,
    /// Accretion radius coefficient: `reach = radius_k · m^(1/3)` (AU) — contact/draw scale.
    pub radius_k: f32,
    /// Fraction of the cloud's gas (H/He) extracted into the pinned central star (the "cheat").
    pub star_gas_frac: f32,
    /// Disk parcels orbit the star at this fraction of circular speed at ignition.
    pub disk_spin: f32,
    /// Gas-drag rate (per year) while the gas is present.
    pub drag: f32,
    /// Fading-drag target (fraction of circular) — sub-circular drives inward migration/accretion.
    pub drag_target_frac: f32,
    /// Gas-dispersal timescale (years) — drag fades as `exp(-t / gas_tau)`.
    pub gas_tau: f32,
    /// Perpetual drag floor (per year) — keeps settled orbits circular long after the gas is gone.
    pub drag_floor: f32,
    /// Bulk densities by material class (g/cc) for the physical (collision / Roche) radius.
    pub rho_gas_gcc: f32,
    pub rho_ice_gcc: f32,
    pub rho_rock_gcc: f32,
    /// Fraction of a planet's Hill radius within which a bound body is retained as its satellite.
    pub hill_frac: f32,
    /// Tidal-disruption ("Roche") scale as a fraction of the host's accretion radius (rings).
    pub tidal_frac: f32,
    /// Emergent body-type mass thresholds (solar masses).
    pub star_mass: f32,
    pub giant_mass: f32,
    pub planet_mass: f32,
    /// Playable-world gate: mass band (M☉) + habitable-zone edges (× √luminosity AU).
    pub playable_mass_min: f32,
    pub playable_mass_max: f32,
    pub hz_inner_frac: f32,
    pub hz_outer_frac: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        // Earth mass in solar masses, for the playable mass band (0.3–5 M⊕).
        const EARTH_MASS: f32 = 3.003e-6;
        Self {
            softening: 0.05,
            radius_k: 0.8,
            star_gas_frac: 0.98,
            disk_spin: 1.0,
            drag: 0.6,
            drag_target_frac: 0.95,
            gas_tau: 35.0,
            drag_floor: 0.02,
            rho_gas_gcc: 1.0,
            rho_ice_gcc: 1.6,
            rho_rock_gcc: 4.0,
            hill_frac: 0.5,
            tidal_frac: 0.1,
            star_mass: 0.08,     // ~hydrogen-fusion limit
            giant_mass: 9.0e-5,  // ≈ 30 M⊕
            planet_mass: 1.5e-7, // ≈ 0.05 M⊕
            playable_mass_min: 0.3 * EARTH_MASS,
            playable_mass_max: 5.0 * EARTH_MASS,
            hz_inner_frac: 0.95,
            hz_outer_frac: 1.37,
        }
    }
}
