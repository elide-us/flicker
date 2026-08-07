//! Invariant world constants (spec §2). Planet size is **fixed at Earth** — there
//! is no size slider. A hex *is* a 2048² heightmap at 128 ft/px, so 49.65 mi is a
//! consequence of the cluster geometry, not a tuning knob; the two independent
//! derivations (Goldberg cell size vs heightmap span) agree only at freq 96.

use std::path::PathBuf;

/// Icosphere subdivision frequency. **Fixed at 96** → `10·96² + 2 = 92_162` cells
/// (92_150 hexes + 12 pentagons), a hex ≈ 49.67 mi across — matching the
/// independent 2048 px × 128 ft = 49.65 mi span. The two agree only here.
pub const PLANET_FREQ: u32 = 96;

/// Cell count at [`PLANET_FREQ`]: `10·freq² + 2`.
pub const PLANET_CELLS: usize = (10 * PLANET_FREQ * PLANET_FREQ + 2) as usize;

/// Total planet mass (Earth), kg. The accretion budget sums to exactly this.
pub const PLANET_MASS_KG: f64 = 5.972e24;

/// Planet radius, m.
pub const PLANET_RADIUS_M: f64 = 6_371_000.0;

/// Area of one cell, m² — **read from the grid you are actually running**, never
/// a constant.
///
/// Equal-area landed (worldgrid Slice 3b, the Snyder ISEA map), so `4πR²/N` is
/// now the true area of every cell rather than an average hiding a 1.75× spread.
/// That is what lets the *areal* derivations — thickness, overburden pressure,
/// weathering flux, the sea-level solve — mean the same thing everywhere.
///
/// It takes `n_cells` because a coarse grid is a coarse **planet**, not a smaller
/// one: the radius is fixed, so fewer cells means bigger cells. A single constant
/// pinned to [`PLANET_FREQ`] was right there and silently wrong everywhere else —
/// at freq 24 it made cells 16× too small, and every thickness, pressure and
/// elevation derived from them 16× too large. (Mass conservation never cared
/// either way: it is areal-independent, absolute masses.)
///
/// At [`PLANET_FREQ`] this is ≈ 5.534×10⁹ m², which is where the scale chain
/// closes: 2048² px × (128 ft)² per hex.
pub fn cell_area_m2(n_cells: usize) -> f64 {
    4.0 * std::f64::consts::PI * PLANET_RADIUS_M * PLANET_RADIUS_M / n_cells.max(1) as f64
}

/// How fast plates move, cm/yr — the physical anchor of the tectonic timeline,
/// the same constant the worldengine tick-sim design derived its clock from
/// (Earth's plates run 2–10; 5 is the design's middle).
pub const PLATE_SPEED_CM_YR: f64 = 5.0;

/// One hex across, in centimetres — a CONSEQUENCE of the tile geometry, derived
/// from it exactly: 2048 px × 128 ft/px × 30.48 cm/ft (≈ 49.65 mi), never a
/// typed-in number.
const HEX_CM: f64 = 2048.0 * 128.0 * 30.48;

/// Nominal formation tick, Myr — **the time for the ground to move one hex at
/// plate speed**, the design's iteration unit, DERIVED and never typed in:
/// `49.65 mi ÷ 5 cm/yr ≈ 1.6 My` (the `MY_PER_TICK` law the worldengine tick
/// sim stated). Still a microscopic fraction of any planetary-scale chemical
/// transformation — the ledgers move a little every tick, and a planet takes
/// its ~2,800 ticks of 4.5 billion years to bake, never a lump sum.
pub const NOMINAL_DT_MYR: f64 = HEX_CM / PLATE_SPEED_CM_YR / 1.0e6;

/// The repo content directory (`Alpha/content/data`), resolved relative to this
/// crate — mirrors `flicker-worldengine`'s `from_repo` seam.
pub fn content_data_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock is DERIVED, never typed in: one tick is the time for the
    /// ground to cover one hex (49.65 mi — itself a consequence of 2048 px ×
    /// 128 ft) at the design's plate speed. `49.65 mi ÷ 5 cm/yr ≈ 1.6 My`.
    /// Anyone retuning the pace moves the plate speed (a physical claim), not
    /// the tick (an arithmetic consequence).
    #[test]
    fn one_tick_is_one_hex_of_plate_motion() {
        let hex_cm = 2048.0 * 128.0 * 30.48; // px × ft/px × cm/ft — the tile geometry
        let expect_myr = hex_cm / PLATE_SPEED_CM_YR / 1.0e6;
        assert!(
            (NOMINAL_DT_MYR - expect_myr).abs() < 1e-6,
            "the tick drifted from the hex-crossing law: {NOMINAL_DT_MYR} vs {expect_myr}"
        );
        assert!((NOMINAL_DT_MYR - 1.6).abs() < 0.05, "≈1.6 My at 5 cm/yr: {NOMINAL_DT_MYR}");
    }
}
