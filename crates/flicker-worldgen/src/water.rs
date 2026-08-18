//! **The Epoch-4 water cycle** — the conserved atmospheric/hydrological sweep the
//! hydrosphere runs after the static endowment is laid down. It re-homes the
//! `flicker-pocepochs` `layers.rs` conveyor onto the icosphere neighbour graph:
//! the essential physics (evaporate → advect vapour → condense/precipitate →
//! runoff → freeze/melt), written natively for variable-length hex adjacency
//! instead of a fixed 2-D grid, and driven on the geological batch cadence (one
//! sweep = one pass over the whole array, `sweeps` passes total).
//!
//! **Three conserved water phases** live on each [`HexState`]: `surface_water`
//! (oceans/lakes), `humidity` (the gaseous phase the wind carries), and `ice`. The
//! sweep only ever *moves* mass between the three and between neighbours — the
//! planet's total water `Σ(surface_water + humidity + ice)` is invariant (checked
//! in tests). It never touches the bulk element ledger, so composition conservation
//! is untouched too.
//!
//! **Gaseous erosion / emergent climate.** Evaporation loads vapour over warm
//! oceans; a prevailing wind advects it as an **edge flux** across the graph;
//! condensation rains it out where the air is cool or forced up over rising terrain
//! — so mountains catch rain on the windward side and cast a **dry rain-shadow**
//! downwind, emergently. The accumulated rain becomes the cell's `precipitation`
//! (the moisture field Epoch 6 erodes and grows biomes from), replacing the static
//! coast-proximity estimate.

use glam::Vec3;

use crate::pipeline::EpochCtx;
use crate::state::HexState;

/// Fraction of warm surface water that evaporates into vapour per sweep.
const EVAP_RATE: f32 = 0.25;
/// Fraction of a cell's vapour advected downwind per sweep.
const ADVECT_RATE: f32 = 0.6;
/// Base fraction of vapour that condenses per sweep (scaled by saturation).
const COND_RATE: f32 = 0.45;
/// Fraction of land surface water that runs off downhill per sweep.
const RUNOFF_RATE: f32 = 0.5;
/// Fraction of surface water that freezes (below 0°C) / ice that melts (above) per sweep.
const PHASE_RATE: f32 = 0.5;
/// Temperature (°C) of full evaporative warmth; also the scale for the cool-air
/// condensation ramp.
const WARM_TEMP: f32 = 30.0;

/// Run the conserved water cycle for `sweeps` passes over `cells`, in place. Seeds
/// the ocean surface water from each cell's Epoch-4 `water_depth`, iterates the
/// conveyor, and writes the emergent rain field back into `precipitation`.
///
/// `ctx` supplies the neighbour graph and the per-cell unit directions (for
/// latitude and the prevailing-wind tangent). Deterministic; no RNG.
pub fn run_water_cycle(cells: &mut [HexState], ctx: &EpochCtx, sweeps: u32) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    let dirs = ctx.dirs;
    let nbr = ctx.neighbors;

    // Seed the water phases: the ocean holds the Epoch-4 water column; air and ice
    // start empty and fill from the cycle.
    for c in cells.iter_mut() {
        c.surface_water = c.water_depth.max(0.0);
        c.humidity = 0.0;
        c.ice = 0.0;
    }

    // Prevailing-wind unit tangent (a zonal "westerly": eastward around the pole
    // axis), precomputed per cell — the direction vapour is pushed.
    let east: Vec<Vec3> = dirs
        .iter()
        .map(|&d| {
            let e = Vec3::Y.cross(d);
            e.normalize_or_zero()
        })
        .collect();

    // Accumulated rainfall per cell → the emergent precipitation field.
    let mut rain = vec![0.0f32; n];
    // Scratch deltas for the two conservative horizontal transports.
    let mut delta = vec![0.0f32; n];

    for _ in 0..sweeps.max(1) {
        // 1. Evaporate: warm surface water → vapour (in-cell, conserving).
        for c in cells.iter_mut() {
            let warmth = (c.temperature / WARM_TEMP).clamp(0.0, 1.0);
            let evap = (EVAP_RATE * warmth * c.surface_water).min(c.surface_water);
            c.surface_water -= evap;
            c.humidity += evap;
        }

        // 2. Advect vapour downwind as a conservative edge flux over the graph.
        delta.iter_mut().for_each(|d| *d = 0.0);
        for i in 0..n {
            let h = cells[i].humidity;
            if h <= 0.0 {
                continue;
            }
            let di = dirs[i];
            // Downwind weight per neighbour: how much its edge points along the wind.
            let mut weights = 0.0f32;
            let mut ws: [(usize, f32); 6] = [(0, 0.0); 6];
            let mut k = 0;
            for &jn in &nbr[i] {
                let j = jn as usize;
                let edge = dirs[j] - di;
                let tang = (edge - edge.dot(di) * di).normalize_or_zero();
                let w = tang.dot(east[i]).max(0.0);
                if w > 0.0 && k < 6 {
                    ws[k] = (j, w);
                    weights += w;
                    k += 1;
                }
            }
            if weights <= 0.0 {
                continue; // no downwind neighbour (e.g. at a pole) — vapour stays
            }
            let out = ADVECT_RATE * h;
            delta[i] -= out;
            for &(j, w) in ws.iter().take(k) {
                delta[j] += out * (w / weights);
            }
        }
        for i in 0..n {
            cells[i].humidity += delta[i];
        }

        // 3. Condense / precipitate: cool air and air forced up over terrain rains
        //    out (vapour → surface water, or ice when freezing). In-cell, conserving.
        for i in 0..n {
            let c = &mut cells[i];
            if c.humidity <= 0.0 {
                continue;
            }
            let cool = ((WARM_TEMP - c.temperature) / WARM_TEMP).clamp(0.0, 1.0);
            let oro = c.elevation.clamp(0.0, 1.0); // rising terrain wrings out vapour
            let sat = (0.25 + 0.55 * cool + 0.6 * oro).clamp(0.0, 1.0);
            let cond = COND_RATE * sat * c.humidity;
            c.humidity -= cond;
            if c.temperature < 0.0 {
                c.ice += cond;
            } else {
                c.surface_water += cond;
            }
            rain[i] += cond;
        }

        // 4. Runoff: land surface water flows to its lowest neighbour (toward the
        //    sea), a conservative edge flux; accumulate river flow.
        delta.iter_mut().for_each(|d| *d = 0.0);
        for i in 0..n {
            let w = cells[i].surface_water;
            if w <= 0.0 {
                continue;
            }
            // Steepest-descent neighbour by terrain elevation.
            let ei = cells[i].elevation;
            let mut low = None;
            let mut low_e = ei;
            for &jn in &nbr[i] {
                let j = jn as usize;
                if cells[j].elevation < low_e {
                    low_e = cells[j].elevation;
                    low = Some(j);
                }
            }
            if let Some(j) = low {
                let flow = RUNOFF_RATE * w;
                delta[i] -= flow;
                delta[j] += flow;
                cells[i].flow += flow;
            }
        }
        for i in 0..n {
            cells[i].surface_water += delta[i];
        }

        // 5. Freeze / melt (in-cell, conserving).
        for c in cells.iter_mut() {
            if c.temperature < 0.0 {
                let fr = PHASE_RATE * c.surface_water;
                c.surface_water -= fr;
                c.ice += fr;
            } else if c.ice > 0.0 {
                let ml = PHASE_RATE * c.ice;
                c.ice -= ml;
                c.surface_water += ml;
            }
        }
    }

    // Write the emergent rain field back as precipitation (normalised to 0..1). If
    // the world had no ocean to drive a cycle, leave Epoch-4's static estimate.
    let rain_max = rain.iter().copied().fold(0.0f32, f32::max);
    if rain_max > 0.0 {
        for i in 0..n {
            cells[i].precipitation = (rain[i] / rain_max).clamp(0.0, 1.0);
        }
    }
}

/// Total conserved water across the planet — `Σ(surface_water + humidity + ice)`.
/// Invariant across the sweep (the conservation witness the tests assert).
pub fn total_water(cells: &[HexState]) -> f64 {
    cells
        .iter()
        .map(|c| (c.surface_water + c.humidity + c.ice) as f64)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// A latitude ring of `n` cells with an ocean seed + a warm/cold gradient and a
    /// bump of terrain — enough to drive and check the cycle.
    fn ring_world(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>, Vec<HexState>) {
        use flicker_worldstate::Composition;
        let dirs: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        let cells: Vec<HexState> = (0..n)
            .map(|i| {
                let mut s = HexState::new(Composition::from_iter([(8u8, 1000.0)]));
                // Half ocean (depth), half land with a central mountain.
                if i < n / 2 {
                    s.water_depth = 5.0;
                    s.elevation = -0.5;
                } else {
                    s.elevation = if i == 3 * n / 4 { 0.9 } else { 0.1 };
                }
                s.temperature = 20.0 - 10.0 * (i as f32 / n as f32); // warm→cool
                s
            })
            .collect();
        (dirs, neighbors, cells)
    }

    #[test]
    fn water_is_conserved_across_the_sweep() {
        let t = tables();
        let (dirs, neighbors, mut cells) = ring_world(24);
        let ctx = EpochCtx {
            tables: &t,
            dirs: &dirs,
            neighbors: &neighbors,
            seed: 1,
        };
        // Seeding sets surface_water = water_depth, so measure the post-seed total.
        let seeded_total: f64 = cells.iter().map(|c| c.water_depth.max(0.0) as f64).sum();
        run_water_cycle(&mut cells, &ctx, 30);
        let after = total_water(&cells);
        assert!(
            (after - seeded_total).abs() <= 1e-3 * seeded_total.max(1.0),
            "water drifted: seeded {seeded_total}, after {after}"
        );
        for c in &cells {
            assert!(c.surface_water.is_finite() && c.humidity.is_finite() && c.ice.is_finite());
            assert!(c.surface_water >= -1e-4 && c.humidity >= -1e-4 && c.ice >= -1e-4);
        }
    }

    #[test]
    fn rain_reaches_land_and_varies() {
        let t = tables();
        let (dirs, neighbors, mut cells) = ring_world(24);
        let ctx = EpochCtx {
            tables: &t,
            dirs: &dirs,
            neighbors: &neighbors,
            seed: 1,
        };
        run_water_cycle(&mut cells, &ctx, 30);
        // Some land cell received rain, and the precipitation field is not uniform
        // (a rain shadow / gradient emerged).
        let land_rain = (12..24).any(|i| cells[i].precipitation > 0.0);
        assert!(land_rain, "no rain reached land");
        let lo = cells
            .iter()
            .map(|c| c.precipitation)
            .fold(f32::MAX, f32::min);
        let hi = cells
            .iter()
            .map(|c| c.precipitation)
            .fold(f32::MIN, f32::max);
        assert!(
            hi - lo > 0.05,
            "precipitation field is nearly uniform ({lo}..{hi})"
        );
    }
}
