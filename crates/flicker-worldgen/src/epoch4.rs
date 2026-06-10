//! **Epoch 4 — hydrosphere formation** (world-gen spec).
//!
//! Water condenses and fills the low ground: a **bathtub fill** sets a global
//! sea level so a target fraction of the surface is submerged, and each hex
//! records its water depth. Surface **temperature** follows latitude (warm
//! equator, cold poles) minus an elevation lapse. This is the stage that hands
//! the planet to the water cycle — oceans now sit on real terrain with a real
//! hardness field to erode.
//!
//! The *coastlines* are composition-driven (they fall out of the terrain the
//! earlier epochs built); the water *amount* (`ocean_fraction`) is the planet's
//! water endowment — a knob here. Tying it to the composition's H/O budget, plus
//! atmosphere-from-outgassing and precipitation, are Epoch-4 refinements.

use std::cmp::Ordering;

use crate::pipeline::{EpochCtx, EpochTransform};
use crate::state::HexState;

/// Epoch 4 parameters.
pub struct Epoch4 {
    /// Fraction of the surface submerged — the sea level is the bathtub level
    /// that floods this fraction of the hexes (by elevation).
    pub ocean_fraction: f32,
    /// Surface temperature at the equator / poles (°C-ish).
    pub equator_temp: f32,
    pub pole_temp: f32,
    /// Temperature drop per unit of land elevation above sea level.
    pub lapse: f32,
}

impl Default for Epoch4 {
    fn default() -> Self {
        Self {
            ocean_fraction: 0.6,
            equator_temp: 28.0,
            pole_temp: -25.0,
            lapse: 40.0,
        }
    }
}

impl EpochTransform for Epoch4 {
    fn epoch(&self) -> u8 {
        4
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        if prev.is_empty() {
            return Vec::new();
        }
        // Bathtub fill: sea level = the ocean-fraction percentile of elevations.
        let mut elevs: Vec<f32> = prev.iter().map(|s| s.elevation).collect();
        elevs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        let idx = ((self.ocean_fraction.clamp(0.0, 1.0) * (elevs.len() - 1) as f32).round() as usize)
            .min(elevs.len() - 1);
        let sea_level = elevs[idx];

        prev.iter()
            .enumerate()
            .map(|(i, s)| {
                let mut s = s.clone();
                s.sea_level = sea_level;
                s.water_depth = (sea_level - s.elevation).max(0.0);
                // Temperature: latitude band, then land cools with height; ocean
                // stays near the sea-level base.
                let poleness = ctx.dirs[i].y.abs();
                let base = self.equator_temp + (self.pole_temp - self.equator_temp) * poleness;
                let land_height = (s.elevation - sea_level).max(0.0);
                s.temperature = base - self.lapse * land_height;
                s
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldstate::Composition;
    use glam::Vec3;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    #[test]
    fn ocean_fills_the_low_hexes() {
        let n = 10;
        let t = tables();
        let dirs: Vec<Vec3> = (0..n)
            .map(|i| Vec3::new(0.0, i as f32 / n as f32, 1.0).normalize())
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n).map(|_| vec![]).collect();
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        // Elevations spread evenly from -1 (deep) to +1 (peak).
        let prev: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::new());
                s.elevation = -1.0 + 2.0 * i as f32 / (n - 1) as f32;
                s
            })
            .collect();
        let out = Epoch4 { ocean_fraction: 0.5, ..Epoch4::default() }.apply(&ctx, &prev);
        // Roughly half submerged; the lowest hex is underwater, the highest dry.
        let submerged = out.iter().filter(|s| s.water_depth > 0.0).count();
        assert!((3..=7).contains(&submerged), "submerged {submerged}, expected ~half");
        assert!(out[0].water_depth > 0.0, "deepest hex should be ocean");
        assert_eq!(out[n - 1].water_depth, 0.0, "highest hex should be dry");
        assert!(out.iter().all(|s| s.temperature.is_finite()));
    }

    #[test]
    fn equator_is_warmer_than_the_poles() {
        let t = tables();
        let dirs = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)]; // equator, pole
        let neighbors = [vec![], vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        let flat = HexState::new(Composition::new()); // elevation 0 both
        let out = Epoch4::default().apply(&ctx, &[flat.clone(), flat]);
        assert!(out[0].temperature > out[1].temperature, "equator should beat pole");
    }
}
