//! Invariant world constants (spec §2). **The tile span is the canon**: a hex IS
//! a 2048² heightmap at 128 ft/px — ≈49.65 mi flat-to-flat — and that is not a
//! quality setting. The planet has to fit the grid, so a coarser grid is a
//! **smaller planet**, never a planet with bigger hexes (the T6 freq↔radius
//! pinning; the 2026-08-06 two-models incident retired the fixed-Earth radius
//! that used to live here). Every absolute in this crate is therefore a
//! **reference value at [`PLANET_FREQ`]**, and a world born on any other grid
//! derives its own radius, mass, budgets and gravity through [`size_scale`].

use std::path::PathBuf;

/// One hex flat-to-flat, m: 2048 px × 128 ft/px × 0.3048 m/ft ≈ 79.9 km
/// (49.65 mi). **The one constant the whole scale chain hangs from** — the
/// pixel tier (`flicker-worldtile`) re-exports this rather than deriving its
/// own copy.
pub const TILE_SPAN_M: f64 = 2048.0 * 128.0 * 0.3048;

/// A regular hexagon `s` across the flats covers `√3/2` of its bounding square.
const SQRT3_OVER_2: f64 = 0.866_025_403_784_438_6;

/// Area of one cell, m² — **the same at every frequency**, because the span is
/// fixed and the planet fits the grid. Equal-area landed (worldgrid Slice 3b,
/// the Snyder ISEA map), so this is the true area of every cell rather than an
/// average hiding a 1.75× spread — what lets the *areal* derivations
/// (thickness, overburden pressure, weathering flux, the sea-level solve) mean
/// the same thing everywhere.
///
/// The old `cell_area_m2(n) = 4πR²/n` with a fixed Earth radius made a coarse
/// grid a coarse PLANET; under the span law `4π·radius_for_cells(n)²/n` IS this
/// constant at every `n`, so the function collapsed into it. At [`PLANET_FREQ`]
/// the value moved by −0.104% against the retired Earth-radius derivation — the
/// gap between the two models the incident found live at once.
pub const CELL_AREA_M2: f64 = TILE_SPAN_M * TILE_SPAN_M * SQRT3_OVER_2;

/// The **reference** icosphere subdivision frequency — the Prism Earth. **Fixed
/// at 96** → `10·96² + 2 = 92_162` cells (92_150 hexes + 12 pentagons). Every
/// absolute in this module is stated at this frequency; [`size_scale`] carries
/// it to any other grid.
pub const PLANET_FREQ: u32 = 96;

/// Cell count at [`PLANET_FREQ`]: `10·freq² + 2`.
pub const PLANET_CELLS: usize = (10 * PLANET_FREQ * PLANET_FREQ + 2) as usize;

/// **Reference** planet mass (Earth, at [`PLANET_FREQ`]), kg. The accretion
/// seed sums to exactly this; a world born on `n` cells accretes
/// `× size_scale(n)³` of it (constant density — the R³ ruling, 2026-08-06).
pub const PLANET_MASS_KG: f64 = 5.972e24;

/// **Reference** surface gravity (the [`PLANET_FREQ`] planet's), m/s². A
/// world's own is `× size_scale` — `g = GM/R²` with `M ∝ s³` and `R ∝ s` — read
/// it through [`World::gravity_m_s2`](crate::planet::World::gravity_m_s2), so a
/// half-size world presses its stacks half as hard.
pub const GRAVITY_M_S2: f64 = 9.81;

/// **The planet radius a grid of `n_cells` implies, m.**
///
/// A tile is 2048 px at 128 ft and that is *not* a quality setting — a hex IS a
/// 2048² map. So the tile span is fixed and the planet has to fit the grid: a
/// coarser grid is a **smaller planet**, not a planet with bigger hexes. Solving
/// `4πR² / n = hexagon area` for R is the whole of it.
///
/// This is the freq↔radius pinning the topology spec asks for. Without it a coarse
/// test world silently has 50-mile tiles on an Earth-sized sphere, every tile falls
/// entirely inside its own cell, and nothing about the pixel tier means anything.
pub fn radius_for_cells(n_cells: usize) -> f64 {
    (CELL_AREA_M2 * n_cells.max(1) as f64 / (4.0 * std::f64::consts::PI)).sqrt()
}

/// [`radius_for_cells`] stated in grid frequency: `n = 10·freq² + 2`.
pub fn radius_for_freq(freq: u32) -> f64 {
    radius_for_cells((10 * freq.max(1) * freq.max(1) + 2) as usize)
}

/// **How big this grid's planet is against the reference**: `R / R_ref =
/// √(n_cells / PLANET_CELLS)` — exact, because `R ∝ √n` under the span law.
///
/// Exactly `1.0` at [`PLANET_CELLS`], so the shipping freq-96 world keeps its
/// reference mass, budgets and gravity bit-for-bit. Mass and the kg budget
/// levers ride `s³`; gravity rides `s`; the tick and the cell area don't ride
/// at all (both are consequences of the span, which never changes).
pub fn size_scale(n_cells: usize) -> f64 {
    (n_cells as f64 / PLANET_CELLS as f64).sqrt()
}

/// How fast plates move, cm/yr — the physical anchor of the tectonic timeline,
/// the same constant the worldengine tick-sim design derived its clock from
/// (Earth's plates run 2–10; 5 is the design's middle).
pub const PLATE_SPEED_CM_YR: f64 = 5.0;

/// One hex across, in centimetres — [`TILE_SPAN_M`] restated, never a second
/// derivation.
const HEX_CM: f64 = TILE_SPAN_M * 100.0;

/// Nominal formation tick, Myr — **the time for the ground to move one hex at
/// plate speed**, the design's iteration unit, DERIVED and never typed in:
/// `49.65 mi ÷ 5 cm/yr ≈ 1.6 My` (the `MY_PER_TICK` law the worldengine tick
/// sim stated). The hex span is the same on every size of planet, so the tick
/// survives the size model untouched. Still a microscopic fraction of any
/// planetary-scale chemical transformation — the ledgers move a little every
/// tick, and a planet takes its ~2,800 ticks of 4.5 billion years to bake,
/// never a lump sum.
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

    /// **The span law closes at every frequency**: `4πR²/n` recovers the one
    /// cell area exactly, and the reference grid is scale 1 EXACTLY — which is
    /// what keeps the shipping freq-96 world's mass, budgets and gravity
    /// bit-identical across the size-model unification.
    #[test]
    fn the_planet_fits_the_grid() {
        assert_eq!(size_scale(PLANET_CELLS), 1.0, "the reference grid is the reference planet");
        for freq in [4u32, 12, 24, 48, 96] {
            let n = (10 * freq * freq + 2) as usize;
            let r = radius_for_freq(freq);
            let area = 4.0 * std::f64::consts::PI * r * r / n as f64;
            assert!(
                ((area - CELL_AREA_M2) / CELL_AREA_M2).abs() < 1e-12,
                "freq {freq}: 4πR²/n = {area} vs the one cell area {CELL_AREA_M2}"
            );
        }
        // Half the frequency is (almost exactly) half the planet: √(23042/92162).
        let ratio = radius_for_freq(48) / radius_for_freq(96);
        assert!((ratio - 0.5).abs() < 1e-3, "freq 48 is a half-radius world: {ratio}");
    }

    /// The reference planet is Earth to within the two retired constants'
    /// disagreement: the span-derived radius sits 0.05% under the old
    /// `6_371_000` m, and the cell area moved −0.104% when the span derivation
    /// became the only one. Documented here so the gap is a measured fact, not
    /// a surprise in a bake.
    #[test]
    fn the_reference_planet_is_earth_sized() {
        let r = radius_for_freq(PLANET_FREQ);
        assert!(((r - 6_371_000.0) / 6_371_000.0).abs() < 1e-3, "≈ Earth radius: {r}");
        let old_area = 4.0 * std::f64::consts::PI * 6_371_000.0 * 6_371_000.0 / PLANET_CELLS as f64;
        let shift = (CELL_AREA_M2 - old_area) / old_area;
        assert!(shift.abs() < 1.2e-3, "cell area moved {shift:.2e} against the retired model");
    }
}
