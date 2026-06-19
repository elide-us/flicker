//! Physical constants and the unit convention for the celestial model.
//!
//! Orbital mechanics run in the **AU · year · solar-mass** system the formation
//! sim established: in these units Newtonian gravity has `G = 4π²`, so a 1 AU
//! circular orbit around a 1 M☉ star takes exactly one year — the cleanest
//! bookkeeping for an integrator, and `f64` throughout (positions reach tens of
//! AU; `f32` would bleed the precision a long integration needs).
//!
//! The central star's mass is **not** a constant here — it is a property of a
//! [`System`](crate::System)'s root body (different systems have different stars).
//! What stays fixed are the unit conversions: the gravitational constant, and the
//! mass/length scales used to turn a composition into a physical radius, density,
//! gravity, and pressure.

/// Gravitational constant in AU³ · M☉⁻¹ · yr⁻². With `M☉ = 1`, `G·M☉ = 4π²`.
/// This is the constant the orbital math ([`Body::orbital_elements`] etc.) uses.
///
/// [`Body::orbital_elements`]: crate::Body::orbital_elements
pub const G: f64 = 4.0 * std::f64::consts::PI * std::f64::consts::PI;

/// One solar mass in **grams** — turns a body's mass (M☉) into the CGS mass the
/// volume/density derivation needs (element densities are g/cm³).
pub const M_SUN_G: f64 = 1.989e33;

/// One solar mass in **kilograms** — for the SI surface gravity / pressure
/// derivations.
pub const M_SUN_KG: f64 = 1.989e30;

/// One Earth mass in solar masses — display conversion (M☉ → M⊕).
pub const M_EARTH: f64 = 3.003e-6;

/// One AU in **centimetres** — converts a physical radius (cm, from a CGS volume)
/// to AU.
pub const AU_CM: f64 = 1.495_978_707e13;

/// One AU in **metres** — for the SI surface gravity / pressure derivations.
pub const AU_M: f64 = 1.495_978_707e11;

/// Gravitational constant in SI (m³ · kg⁻¹ · s⁻²) — for surface gravity (m/s²) and
/// central pressure (Pa). Distinct from [`G`], which is in the AU/yr/M☉ system the
/// orbital math uses.
pub const G_SI: f64 = 6.674_30e-11;

/// Earth's surface gravity (m/s²) — a reference for reporting a body's gravity
/// relative to Earth.
pub const EARTH_GRAVITY_SI: f64 = 9.80665;

/// A solar-mass value expressed in Earth masses (for the HUD / labels).
pub fn earth_masses(m_sun: f64) -> f64 {
    m_sun / M_EARTH
}
