//! Hex-budget constants — a body's **planet-scale macro-voxel** resolution (spec §8).
//!
//! The icosphere `freq` sets a body's hex-cell count (`cells = 10·freq² + 2`). Because the
//! ~49.6-mi tile is fixed, the cell count tracks surface area and so `freq ∝ radius`: a
//! body's hex resolution is the *real* per-tile count for its size — **accurate data**, not
//! a render preference. Anchored at two reference worlds:
//!
//! - **Mercury ≈ `freq 48`**, **Earth ≈ `freq 100`** (`hex_freq_for_radius`).
//!
//! **Gas giants are the exception** — pinned at the Mercury count [`HEX_FREQ_GIANT`] = `48`
//! **regardless of their (huge) size**: a giant is a pure-gas simulation with no fine surface
//! to resolve, so it is kept rough (rendered large but coarse). Only solid worlds scale their
//! `freq` up with radius.
//!
//! These are the **data** budget. A multi-body *viewer* may render at a coarser LOD (a stride
//! over the hex data) — a separate concern from the stored count here.

/// A gas giant's pinned hex frequency — the Mercury count, fixed regardless of the giant's
/// real (large) size. The "solid ball of air" stays rough (spec §7/§8).
pub const HEX_FREQ_GIANT: u32 = 48;

/// Earth's physical radius (AU) — the upper anchor of the radius→freq line.
const R_EARTH_AU: f64 = 4.26e-5;
/// Earth's anchor hex frequency.
const FREQ_EARTH: f64 = 100.0;
/// Mercury's physical radius (AU) — the lower anchor (~0.38 R⊕).
const R_MERCURY_AU: f64 = 1.63e-5;
/// Mercury's anchor hex frequency.
const FREQ_MERCURY: f64 = 48.0;
/// Floor so a small (but non-belt) world still resolves; ceiling so a super-Earth doesn't run
/// the cell count away. (The ceiling is a data bound; pick a viewer LOD separately.)
const FREQ_MIN: u32 = 12;
const FREQ_MAX: u32 = 100;

/// The hex frequency for a **solid** world of physical radius `radius_au` (AU): the line
/// through the Mercury (`48`) and Earth (`100`) anchors, clamped to `[FREQ_MIN, FREQ_MAX]`.
/// Gas giants do **not** use this — they are pinned at [`HEX_FREQ_GIANT`].
pub fn hex_freq_for_radius(radius_au: f64) -> u32 {
    let slope = (FREQ_EARTH - FREQ_MERCURY) / (R_EARTH_AU - R_MERCURY_AU);
    let freq = FREQ_MERCURY + slope * (radius_au - R_MERCURY_AU);
    (freq.round() as i64).clamp(FREQ_MIN as i64, FREQ_MAX as i64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_land_on_their_reference_frequencies() {
        assert_eq!(hex_freq_for_radius(R_EARTH_AU), 100, "Earth → 100");
        assert_eq!(hex_freq_for_radius(R_MERCURY_AU), 48, "Mercury → 48");
    }

    #[test]
    fn freq_grows_with_radius_and_clamps() {
        // Bigger world → finer hex, monotonically.
        assert!(hex_freq_for_radius(3.0e-5) > hex_freq_for_radius(2.0e-5));
        // A super-Earth is bounded by the ceiling, not run away.
        assert_eq!(hex_freq_for_radius(8.0e-5), FREQ_MAX);
        // A near-zero radius stays positive and bounded; a degenerate (negative) radius floors.
        assert!(hex_freq_for_radius(1.0e-7) >= FREQ_MIN);
        assert_eq!(hex_freq_for_radius(-1.0), FREQ_MIN);
    }

    #[test]
    fn giants_are_pinned_at_the_mercury_count() {
        assert_eq!(HEX_FREQ_GIANT, 48);
        // A giant is huge but stays at 48; an equally-large *solid* world would scale up.
        assert!(hex_freq_for_radius(8.0e-5) > HEX_FREQ_GIANT);
    }
}
