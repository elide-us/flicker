//! Physical constants and the unit convention for the formation sim.
//!
//! Internal units are **AU** (length), **years** (time) and **solar masses**
//! (mass). In these units Newtonian gravity has `G = 4π²`, so a 1 AU circular
//! orbit around a 1 M☉ star takes exactly one year — the cleanest possible
//! bookkeeping for an orbital integrator. The simulation runs in `f64`
//! (`glam::DVec3`): positions reach tens of AU over many steps, and `f32` would
//! bleed away the precision the long integration needs. Rendering converts to
//! `f32` only at the very edge.

/// Gravitational constant in AU³ · M☉⁻¹ · yr⁻². With `M☉ = 1`, `G·M☉ = 4π²`.
pub const G: f64 = 4.0 * std::f64::consts::PI * std::f64::consts::PI;

/// Central star mass (M☉). Sun-like.
pub const M_STAR: f64 = 1.0;

/// One solar mass in grams — converts a body's mass to a volume via class density.
pub const M_SUN_G: f64 = 1.989e33;

/// One Earth mass in solar masses — display conversion (M☉ → M⊕).
pub const M_EARTH: f64 = 3.003e-6;

/// One AU in centimetres — converts a physical radius (cm) to AU.
pub const AU_CM: f64 = 1.495_978_707e13;

/// A solar-mass value expressed in Earth masses (for the HUD).
pub fn earth_masses(m_sun: f64) -> f64 {
    m_sun / M_EARTH
}
