//! The conserved **mass layer** — the cloud as real matter, not just a picture.
//!
//! The cast model (`model.rs`) says *where* each element lands; the cloud field
//! (`cloud.rs`) says how it *clumps*. Neither says *how much* of each element there is.
//! This module supplies that missing axis — the absolute per-element **tonnage** — and
//! it is fully **derived**, never a hand-authored table:
//!
//! - **Mass** (dial) — the total tonnage of the cloud (the supernova's origin mass).
//! - **Metallicity** (dial) — the fraction of that mass that is *metals* (everything
//!   heavier than helium; the Sun's is ≈ 0.014). Sets the gas-to-rock balance.
//! - **The shape** within those two totals is the measured **cosmic abundance curve** —
//!   the real output of stellar + supernova nucleosynthesis (a steep decline with atomic
//!   number, the **iron peak**, and the post-iron **cliff**). It is reference data, the
//!   same kind of real-world fact as `atomic_mass`, not an invented amount.
//!
//! Result: an exact, conserved per-element tonnage — `Σ tonnage == Mass` and
//! `Σ metals == Metallicity · Mass` to the float. This is the conserved reservoir a
//! future collapse step will move into bodies.

use crate::model::Ejecta;

/// One solar mass expressed in Earth masses — used only to print tonnages in a
/// human-readable unit (Earth ≈ 1 of these).
pub const EARTH_PER_SUN: f32 = 332_946.0;

/// The two dials that set the cloud's total tonnage and its gas-to-metal balance.
#[derive(Clone, Copy, Debug)]
pub struct MassParams {
    /// Total cloud tonnage, in solar masses — the supernova's origin mass.
    pub total: f32,
    /// Fraction of [`total`](Self::total) that is *metals* (atomic number > 2). The
    /// Sun's is ≈ 0.014; raising it makes a rockier, metal-richer system.
    pub metallicity: f32,
}

impl Default for MassParams {
    fn default() -> Self {
        // A roughly Sol-like cloud: about one star's worth of material at solar metallicity.
        Self {
            total: 1.0,
            metallicity: 0.014,
        }
    }
}

/// The conserved per-element tonnage, parallel to `Ejecta::elements` (index `i` is the
/// same element the rings and legend use).
#[derive(Clone, Debug)]
pub struct CloudMass {
    /// Absolute tonnage per element, in solar masses. Sums to `MassParams::total`.
    pub tonnage: Vec<f32>,
}

impl CloudMass {
    /// Derive the conserved tonnage from the element set and the two dials.
    pub fn derive(ej: &Ejecta, p: &MassParams) -> Self {
        // Raw relative mass per element = (relative count) × (atomic mass), where the
        // relative count is the measured cosmic abundance (dex → linear). H dominates.
        let raw: Vec<f32> = ej
            .elements
            .iter()
            .map(|e| 10f32.powf(cosmic_abundance_dex(e.number)) * e.atomic_mass)
            .collect();

        // Sum the gas group (H, He) and the metals (everything else) separately, then
        // rescale each group so the metals hold exactly `metallicity` of the total. The
        // *shape* inside each group stays the measured abundance curve.
        let (mut gas_raw, mut metal_raw) = (0.0f32, 0.0f32);
        for (i, e) in ej.elements.iter().enumerate() {
            if is_gas(e.number) {
                gas_raw += raw[i];
            } else {
                metal_raw += raw[i];
            }
        }
        let z = p.metallicity.clamp(0.0, 1.0);
        let gas_scale = if gas_raw > 0.0 {
            (1.0 - z) * p.total / gas_raw
        } else {
            0.0
        };
        let metal_scale = if metal_raw > 0.0 {
            z * p.total / metal_raw
        } else {
            0.0
        };

        let tonnage = ej
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| {
                raw[i]
                    * if is_gas(e.number) {
                        gas_scale
                    } else {
                        metal_scale
                    }
            })
            .collect();
        Self { tonnage }
    }

    /// Total tonnage (≈ `MassParams::total`).
    pub fn total(&self) -> f32 {
        self.tonnage.iter().sum()
    }

    /// Tonnage held in the metals (atomic number > 2).
    pub fn metals(&self, ej: &Ejecta) -> f32 {
        self.tonnage
            .iter()
            .zip(&ej.elements)
            .filter(|(_, e)| !is_gas(e.number))
            .map(|(t, _)| *t)
            .sum()
    }
}

/// Hydrogen and helium are the "gas"; everything heavier is a "metal" (astronomer's sense).
fn is_gas(number: u8) -> bool {
    number <= 2
}

/// Measured **cosmic abundance** of an element as `log10(N/N_H) + 12` (the standard
/// astronomical "dex" scale, hydrogen = 12). These are real solar-photosphere /
/// meteoritic values (Asplund/Lodders) — the integrated output of stellar + supernova
/// nucleosynthesis — for the 27 Prism elements. The curve carries its structure: the
/// steep decline with atomic number, the **iron peak** (Fe/Ni well above their
/// neighbours), and the post-iron **cliff** (U ≈ 12 dex below H). Unmapped → a trace floor.
fn cosmic_abundance_dex(number: u8) -> f32 {
    match number {
        1 => 12.00,  // H
        2 => 10.93,  // He
        6 => 8.43,   // C
        7 => 7.83,   // N
        8 => 8.69,   // O
        11 => 6.24,  // Na
        12 => 7.60,  // Mg
        13 => 6.45,  // Al
        14 => 7.51,  // Si
        15 => 5.41,  // P
        16 => 7.12,  // S
        17 => 5.50,  // Cl
        19 => 5.03,  // K
        20 => 6.34,  // Ca
        22 => 4.95,  // Ti
        24 => 5.64,  // Cr
        26 => 7.50,  // Fe — the iron peak
        27 => 4.99,  // Co
        28 => 6.22,  // Ni
        29 => 4.19,  // Cu
        30 => 4.56,  // Zn
        47 => 0.94,  // Ag
        50 => 2.04,  // Sn
        78 => 1.62,  // Pt
        79 => 0.91,  // Au
        82 => 1.75,  // Pb
        92 => -0.54, // U — a trace, ~12 dex below H
        _ => -2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{load_tables, Ejecta};

    fn ejecta() -> Ejecta {
        Ejecta::from_tables(&load_tables())
    }

    #[test]
    fn total_tonnage_matches_the_mass_dial() {
        let ej = ejecta();
        let m = CloudMass::derive(
            &ej,
            &MassParams {
                total: 3.0,
                metallicity: 0.02,
            },
        );
        assert!((m.total() - 3.0).abs() < 1e-3, "Σ tonnage == Mass");
    }

    #[test]
    fn metals_hold_exactly_the_metallicity_fraction() {
        let ej = ejecta();
        let m = CloudMass::derive(
            &ej,
            &MassParams {
                total: 1.0,
                metallicity: 0.05,
            },
        );
        assert!((m.metals(&ej) - 0.05).abs() < 1e-4, "Σ metals == Z·Mass");
    }

    #[test]
    fn hydrogen_dominates_and_uranium_is_a_trace() {
        let ej = ejecta();
        let m = CloudMass::derive(&ej, &MassParams::default());
        let idx = |s: &str| ej.elements.iter().position(|e| e.symbol == s).unwrap();
        assert!(m.tonnage[idx("H")] > m.tonnage[idx("U")], "H outweighs U");
        // Uranium is present but vanishingly small — the HZ-world uranium trace.
        assert!(m.tonnage[idx("U")] > 0.0, "U is present, not zero");
        assert!(
            m.tonnage[idx("U")] < 1e-6 * m.tonnage[idx("H")],
            "U is a deep trace vs H"
        );
    }

    #[test]
    fn iron_peak_outweighs_its_neighbours() {
        let ej = ejecta();
        let m = CloudMass::derive(&ej, &MassParams::default());
        let idx = |s: &str| ej.elements.iter().position(|e| e.symbol == s).unwrap();
        assert!(m.tonnage[idx("Fe")] > m.tonnage[idx("Cr")], "Fe above Cr");
        assert!(m.tonnage[idx("Fe")] > m.tonnage[idx("Co")], "Fe above Co");
        assert!(m.tonnage[idx("Fe")] > m.tonnage[idx("Ti")], "Fe above Ti");
    }

    #[test]
    fn raising_metallicity_shifts_mass_from_gas_to_metals() {
        let ej = ejecta();
        let lo = CloudMass::derive(
            &ej,
            &MassParams {
                total: 1.0,
                metallicity: 0.01,
            },
        );
        let hi = CloudMass::derive(
            &ej,
            &MassParams {
                total: 1.0,
                metallicity: 0.10,
            },
        );
        let idx = |s: &str| ej.elements.iter().position(|e| e.symbol == s).unwrap();
        assert!(hi.metals(&ej) > lo.metals(&ej), "more Z → more metals");
        assert!(
            hi.tonnage[idx("H")] < lo.tonnage[idx("H")],
            "more Z → less hydrogen"
        );
    }
}
