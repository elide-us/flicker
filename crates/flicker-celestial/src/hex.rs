//! Hex-budget — a body's **planet-scale macro-voxel** resolution (spec §8).
//!
//! The icosphere `freq` sets a body's hex-cell count (`cells = 10·freq² + 2`). Because the
//! ~49.6-mi tile is fixed, the cell count tracks surface area and so **`freq ∝ radius`**: a
//! body's hex resolution is the *real* per-tile count for its size — **accurate data**, not a
//! render preference. Anchored at **Earth = `freq 100`**; everything else scales with radius
//! (e.g. Mercury, ~0.38 R⊕, lands near `freq 38`).
//!
//! **Gas giants** use **half** the count for their size ([`hex_freq_for_giant`]): their hexes are
//! ~twice normal size, kept deliberately coarse — a giant is a ball of air we never render as a
//! detailed world (its final swirl is drawn once at pegging). Derived from size like everything
//! else, **not a static pin**.
//!
//! These are the **data** budget. A multi-body *viewer* may render at a coarser LOD — a separate
//! concern from the stored count here.

/// Earth's physical radius (AU) — the anchor of the radius→freq line.
const R_EARTH_AU: f64 = 4.26e-5;
/// Earth's anchor hex frequency.
const FREQ_EARTH: f64 = 100.0;
/// Floor so a small (but non-belt) world still resolves; ceiling so a super-Earth doesn't run the
/// cell count away. (The ceiling is a data bound; pick a viewer LOD separately.)
const FREQ_MIN: u32 = 12;
const FREQ_MAX: u32 = 100;

/// The hex frequency for a **solid** world of physical radius `radius_au` (AU): strictly
/// **proportional to radius** (cell count ∝ surface area ∝ R², and `cells = 10·freq²+2`, so
/// `freq ∝ R`), anchored at Earth = 100 and clamped to `[FREQ_MIN, FREQ_MAX]`. The body's size sets
/// its hex count — that's the whole point: bigger world → more, finer tiles.
pub fn hex_freq_for_radius(radius_au: f64) -> u32 {
    let freq = FREQ_EARTH * radius_au / R_EARTH_AU;
    (freq.round() as i64).clamp(FREQ_MIN as i64, FREQ_MAX as i64) as u32
}

/// The hex frequency for a **gas giant** of physical radius `radius_au`: **half** a solid world's
/// count at the same size — its hexes are ~twice normal size, kept deliberately coarse (a giant is
/// a ball of air, never rendered as a detailed world). Derived from size, not a static pin.
pub fn hex_freq_for_giant(radius_au: f64) -> u32 {
    (hex_freq_for_radius(radius_au) / 2).max(FREQ_MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    const R_MERCURY_AU: f64 = 1.63e-5; // ~0.38 R⊕

    #[test]
    fn freq_is_proportional_to_radius() {
        assert_eq!(hex_freq_for_radius(R_EARTH_AU), 100, "Earth → 100");
        // Mercury (~0.38 R⊕) scales to ~38 — derived from size, not a hand-set anchor.
        assert_eq!(hex_freq_for_radius(R_MERCURY_AU), 38, "Mercury scales to ~38");
        // The freq ratio equals the radius ratio (true proportionality).
        let big = hex_freq_for_radius(3.0e-5) as f64;
        let small = hex_freq_for_radius(1.5e-5) as f64;
        assert!((big / small - 2.0).abs() < 0.06, "doubling radius ~doubles freq");
    }

    #[test]
    fn freq_grows_with_radius_and_clamps() {
        assert!(hex_freq_for_radius(3.0e-5) > hex_freq_for_radius(2.0e-5));
        assert_eq!(hex_freq_for_radius(8.0e-5), FREQ_MAX);
        assert!(hex_freq_for_radius(1.0e-7) >= FREQ_MIN);
        assert_eq!(hex_freq_for_radius(-1.0), FREQ_MIN);
    }

    #[test]
    fn a_giant_uses_half_a_solid_world_s_tiles() {
        // Same size → half the freq: coarse, derived from size (not a static 48).
        let r = 5.0e-4; // a giant-sized radius
        assert_eq!(hex_freq_for_giant(r), (hex_freq_for_radius(r) / 2).max(FREQ_MIN));
        assert!(hex_freq_for_giant(r) < hex_freq_for_radius(r), "a giant is coarser than a same-size solid");
    }
}
