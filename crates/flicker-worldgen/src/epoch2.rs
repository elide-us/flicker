//! **Epoch 2 — differentiation & crust formation** (world-gen spec).
//!
//! The molten planet sorts by density: light elements (the silicate formers —
//! O, Si, Al, Ca, Na, K, C) rise to form the **crust**; heavy elements (iron and
//! the metals) sink toward the core/mantle. The surface composition becomes the
//! light crust — so a hex that Epoch 1 left iron-rich loses that iron from its
//! surface here. Thinner crust (and equatorial hexes, which cool slower) reads as
//! more **volcanic**. Per-hex — no neighbours needed.

use flicker_worldstate::Composition;

use crate::pipeline::{EpochCtx, EpochTransform, NOMINAL_DURATION};
use crate::state::HexState;

/// Cycles of differentiation at which the density sort is complete (heavy elements
/// have fully drained from the crust) — the [shared nominal][NOMINAL_DURATION], so
/// the default molten era is fully differentiated (today's baseline) and shorter
/// eras leave heavies stranded in the crust.
const FULL_DIFFERENTIATION: u32 = NOMINAL_DURATION;

/// Epoch 2 parameters.
pub struct Epoch2 {
    /// Elements at or below this density (g/cm³) rise into the crust; denser ones
    /// sink. `~3.5` keeps the silicate formers up (O, Si ≈ 2.3, Al ≈ 2.7, Ca, Na,
    /// K, C) and drops iron (7.9) and the heavy metals.
    pub crust_density_max: f64,
    /// How much polar cooling thickens the crust: poles cool faster → thicker
    /// crust → less volcanism. `0` = no latitude effect.
    pub polar_thickening: f32,
    /// **Molten-era duration** on the shared world clock — how many cycles the
    /// planet stays molten and differentiating. The density sort is *progressive*:
    /// a `settle` fraction (`duration / FULL_DIFFERENTIATION`, capped at 1) of each
    /// heavy element drains out of the crust, so a short molten era leaves iron and
    /// the metals stranded at the surface (a young, undifferentiated, iron-rich
    /// crust) while a long one drains them fully. At the default it is fully settled
    /// — today's output unchanged. The first epoch added to the shared timeline
    /// (Epoch 1 is the snapshot the clock starts from, so it has no duration).
    pub duration: u32,
}

impl Default for Epoch2 {
    fn default() -> Self {
        Self {
            crust_density_max: 3.5,
            polar_thickening: 0.3,
            duration: FULL_DIFFERENTIATION,
        }
    }
}

impl EpochTransform for Epoch2 {
    fn epoch(&self) -> u8 {
        2
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        prev.iter()
            .enumerate()
            .map(|(i, prev)| {
                let mut s = prev.clone();
                // Split the bulk composition by element density: light → crust. The
                // sort is progressive in time — only a `settle` fraction of each
                // heavy element has drained out by the end of the molten era, so a
                // short era leaves heavies stranded in the crust.
                let settle =
                    (self.duration as f32 / FULL_DIFFERENTIATION as f32).min(1.0) as f64;
                let mut crust = Composition::new();
                for (el, amount) in s.composition.iter() {
                    let density = ctx
                        .tables
                        .element_by_number(el)
                        .map(|e| e.density_g_cm3 as f64)
                        .unwrap_or(f64::INFINITY);
                    if density <= self.crust_density_max {
                        crust.add(el, amount); // light former — stays up regardless of time
                    } else {
                        let retained = amount * (1.0 - settle); // heavy — only the un-settled part stays
                        if retained > 0.0 {
                            crust.add(el, retained);
                        }
                    }
                }
                let total = s.composition.total();
                s.crust_fraction = if total > 0.0 { crust.total() / total } else { 0.0 };
                s.crust = crust;
                // Crust thickness: more crust + polar cooling → thicker → less
                // volcanic. `poleness` is 0 at the equator, 1 at a pole.
                let poleness = ctx.dirs[i].y.abs();
                let lat_factor = 1.0 - self.polar_thickening + self.polar_thickening * poleness;
                let thickness = s.crust_fraction as f32 * lat_factor;
                s.volcanic = (1.0 - thickness).clamp(0.0, 1.0);
                s
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{ElementId, JsonTableSource, Tables};
    use glam::Vec3;

    use crate::pipeline::EpochCtx;

    const SI: ElementId = 14;
    const FE: ElementId = 26;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    #[test]
    fn heavy_elements_sink_out_of_the_crust() {
        let t = tables();
        // Half silicon (light, 2.33), half iron (heavy, 7.87).
        let comp = Composition::from_iter([(SI, 5000.0), (FE, 5000.0)]);
        let dirs = [Vec3::Y];
        let neighbors = [vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let out = Epoch2::default().apply(&ctx, &[HexState::new(comp)]);
        let s = &out[0];
        // Silicon stayed in the crust; iron sank out.
        assert_eq!(s.crust.amount(SI), 5000.0);
        assert_eq!(s.crust.amount(FE), 0.0);
        assert!((s.crust_fraction - 0.5).abs() < 1e-9);
        // The bulk composition is untouched (iron is still in the column, below).
        assert_eq!(s.composition.amount(FE), 5000.0);
        // surface() now reports the crust (silicon), not the bulk.
        assert_eq!(s.surface().dominant(), Some(SI));
    }

    #[test]
    fn a_shorter_molten_era_leaves_more_iron_in_the_crust() {
        let t = tables();
        let comp = Composition::from_iter([(SI, 5000.0), (FE, 5000.0)]);
        let dirs = [Vec3::Y];
        let neighbors = [vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let brief = Epoch2 { duration: 1, ..Epoch2::default() }.apply(&ctx, &[HexState::new(comp.clone())]);
        let full = Epoch2 { duration: NOMINAL_DURATION, ..Epoch2::default() }.apply(&ctx, &[HexState::new(comp)]);
        // A short molten era strands iron at the surface; a full one drains it.
        assert!(
            brief[0].crust.amount(FE) > full[0].crust.amount(FE),
            "less differentiation time should leave more iron in the crust"
        );
        assert_eq!(full[0].crust.amount(FE), 0.0, "the default (full) era drains the iron");
        assert!(brief[0].crust.amount(FE) > 0.0, "a brief era keeps some iron up");
    }

    #[test]
    fn volcanic_is_in_range_and_finite() {
        let t = tables();
        let comp = Composition::from_iter([(SI, 1000.0), (FE, 100.0)]);
        let dirs = [Vec3::new(1.0, 0.0, 0.0)]; // equator
        let neighbors = [vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let out = Epoch2::default().apply(&ctx, &[HexState::new(comp)]);
        assert!((0.0..=1.0).contains(&out[0].volcanic));
    }
}
