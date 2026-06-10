//! **Epoch 6 — erosion, sedimentation & biomes** (world-gen spec, Epoch 6).
//!
//! The final hex-level transform: it takes the plate-built, mineralized terrain
//! and **weathers it into a starting landscape** for the runtime water cycle.
//! Three coupled steps, iterated over the hex graph:
//!
//! - **Drainage** — every hex flows to its lowest neighbour; processing from high
//!   to low accumulates `flow` (rainfall gathered downstream), so trunk valleys
//!   carry the most water. This is the flow field the Rivulet sim starts from.
//! - **Hydraulic erosion** — each land hex sheds material in proportion to its
//!   flow, its slope, and its **erodibility** (soft rock from the hardness field
//!   erodes fast; hard rock resists and stands as ridges). The shed mass is
//!   carried to the downhill neighbour as `sediment` — conserved, not destroyed —
//!   so highlands grade down and basins/coasts fill in.
//! - **Thermal creep** — slopes steeper than the talus angle relax toward it,
//!   rounding off the sharpest plate-built scarps.
//!
//! Then each hex is classified into a **biome** from its temperature (Epoch 4),
//! its elevation above the sea, and a moisture field diffused inland from the
//! oceans. This is *macro* erosion at ~50-mi hex scale — deliberately coarse; the
//! per-pass water cycle refines it into real river valleys and smoothed relief.

use flicker_materials::PhysicalState;
use flicker_worldstate::Composition;

use crate::noise::fbm;
use crate::pipeline::{EpochCtx, EpochTransform};
use crate::state::{Biome, HexState};

/// Epoch 6 parameters.
pub struct Epoch6 {
    /// Erosion–deposition passes over the hex graph.
    pub iterations: u32,
    /// Base rainfall delivered to every hex each pass (the flow-accumulation
    /// unit).
    pub rain: f32,
    /// Hydraulic erosion coefficient — overall strength of the carving.
    pub erosion_rate: f32,
    /// Exponent on flow accumulation: `> 1` concentrates carving into the
    /// high-flow trunks (river valleys), `< 1` spreads it out.
    pub flow_exp: f32,
    /// Slope (normalized elevation) above which thermal creep relaxes a pair of
    /// neighbours toward the talus angle.
    pub talus: f32,
    /// Fraction of the excess (above-talus) slope relaxed per pass.
    pub talus_rate: f32,
    /// Elevation **above sea level** beyond which a hex is alpine (bare rock /
    /// snow), regardless of its climate biome.
    pub alpine_height: f32,
    /// Passes diffusing ocean moisture inland (coast wet → interior dry).
    pub moisture_diffuse: u32,
}

impl Default for Epoch6 {
    fn default() -> Self {
        Self {
            iterations: 8,
            rain: 1.0,
            erosion_rate: 0.018,
            flow_exp: 0.8,
            talus: 0.10,
            talus_rate: 0.4,
            alpine_height: 0.5,
            moisture_diffuse: 3,
        }
    }
}

impl EpochTransform for Epoch6 {
    fn epoch(&self) -> u8 {
        6
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        let n = prev.len();
        if n == 0 {
            return Vec::new();
        }
        let sea = prev[0].sea_level;

        let mut elev: Vec<f32> = prev.iter().map(|s| s.elevation).collect();
        let mut sediment = vec![0.0f32; n];
        // Per-hex erodibility from the surface hardness: soft rock sheds fast.
        let erod: Vec<f32> = prev
            .iter()
            .map(|s| (1.0 - solid_hardness(ctx, s.surface()) / 10.0).clamp(0.2, 1.0))
            .collect();

        let mut flow = vec![self.rain; n];
        for _ in 0..self.iterations.max(1) {
            // Process hexes high → low so each is handled before its outflow.
            let mut order: Vec<usize> = (0..n).collect();
            order.sort_by(|&a, &b| {
                elev[b].partial_cmp(&elev[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
            });
            let down: Vec<Option<usize>> =
                (0..n).map(|i| lowest_neighbor(i, &elev, ctx)).collect();

            // Drainage: accumulate rainfall down the flow graph.
            flow = vec![self.rain; n];
            for &h in &order {
                if let Some(d) = down[h] {
                    flow[d] += flow[h];
                }
            }

            // Hydraulic erosion: shed material downhill (mass-conserving — what
            // leaves `h` lands on `d`). Underwater hexes only receive deposition.
            for &h in &order {
                if elev[h] <= sea {
                    continue;
                }
                if let Some(d) = down[h] {
                    let slope = (elev[h] - elev[d]).max(0.0);
                    let mut e = self.erosion_rate * flow[h].powf(self.flow_exp) * slope * erod[h];
                    e = e.min(slope * 0.5); // never overtake the neighbour
                    elev[h] -= e;
                    elev[d] += e;
                    sediment[h] = (sediment[h] - e).max(0.0);
                    sediment[d] += e;
                }
            }

            // Thermal creep: relax over-steep neighbour pairs toward the talus
            // angle (each undirected pair once, symmetric → conserving).
            let snap = elev.clone();
            for i in 0..n {
                for &nb in &ctx.neighbors[i] {
                    let nb = nb as usize;
                    if nb <= i {
                        continue;
                    }
                    let diff = snap[i] - snap[nb];
                    if diff.abs() > self.talus {
                        let mv = (diff.abs() - self.talus) * self.talus_rate * 0.5 * diff.signum();
                        elev[i] -= mv;
                        elev[nb] += mv;
                    }
                }
            }
            for e in elev.iter_mut() {
                *e = e.clamp(-1.0, 1.0);
            }
        }

        // Moisture: oceans are wet (1), diffuse inland so coasts stay moist and
        // interiors dry out.
        let mut moist = vec![0.0f32; n];
        for i in 0..n {
            if elev[i] <= sea {
                moist[i] = 1.0;
            }
        }
        for _ in 0..self.moisture_diffuse {
            let snap = moist.clone();
            for i in 0..n {
                let (mut sum, mut cnt) = (snap[i], 1.0f32);
                for &nb in &ctx.neighbors[i] {
                    sum += snap[nb as usize];
                    cnt += 1.0;
                }
                moist[i] = sum / cnt;
            }
        }

        (0..n)
            .map(|i| {
                let mut s = prev[i].clone();
                s.elevation = elev[i];
                s.flow = flow[i];
                s.sediment = sediment[i];
                s.water_depth = (sea - elev[i]).max(0.0);
                let above = (elev[i] - sea).max(0.0);
                // Regional moisture wobble so biome belts aren't perfectly zonal.
                let wobble = 0.15 * (fbm(ctx.dirs[i] * 3.0, 2, 0x0B10_E5A1, ctx.seed) as f32 - 0.5);
                let m = (moist[i] + wobble).clamp(0.0, 1.0);
                s.biome = if s.water_depth > 0.0 {
                    Biome::Ocean
                } else {
                    classify(s.temperature, above, m, self.alpine_height)
                };
                s
            })
            .collect()
    }
}

/// Composition-weighted hardness over the **solid** formers only (gases are
/// binders, not hardness-bearers) — the same basis the field sampler uses.
fn solid_hardness(ctx: &EpochCtx, comp: &Composition) -> f32 {
    let (mut h, mut w) = (0.0f64, 0.0f64);
    for (el, amount) in comp.iter() {
        if let Some(e) = ctx.tables.element_by_number(el) {
            if e.state != PhysicalState::Gas {
                h += amount * e.hardness as f64;
                w += amount;
            }
        }
    }
    if w > 0.0 {
        (h / w) as f32
    } else {
        0.0
    }
}

/// The strictly-lower neighbour with the least elevation (steepest descent), or
/// `None` at a local minimum (a basin / sink that collects sediment).
fn lowest_neighbor(i: usize, elev: &[f32], ctx: &EpochCtx) -> Option<usize> {
    let mut best = None;
    let mut best_elev = elev[i];
    for &nb in &ctx.neighbors[i] {
        let nb = nb as usize;
        if elev[nb] < best_elev {
            best_elev = elev[nb];
            best = Some(nb);
        }
    }
    best
}

/// Whittaker-style biome from temperature (°C-ish), elevation above sea, and
/// moisture (`0..1`). Ocean is decided by the caller from water depth.
fn classify(temp: f32, above_sea: f32, moisture: f32, alpine_height: f32) -> Biome {
    if above_sea > alpine_height {
        return Biome::Alpine;
    }
    if temp < -8.0 {
        Biome::Ice
    } else if temp < 2.0 {
        if moisture > 0.4 {
            Biome::Taiga
        } else {
            Biome::Tundra
        }
    } else if temp < 16.0 {
        if moisture > 0.4 {
            Biome::Forest
        } else {
            Biome::Grassland
        }
    } else if moisture > 0.6 {
        Biome::Rainforest
    } else if moisture > 0.3 {
        Biome::Savanna
    } else {
        Biome::Desert
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use glam::Vec3;

    use crate::epoch1::{Epoch1, Epoch1Params};
    use crate::epoch2::Epoch2;
    use crate::epoch3::Epoch3;
    use crate::epoch4::Epoch4;
    use crate::epoch5::Epoch5;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    fn ring(n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let dirs = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors = (0..n)
            .map(|i| vec![((i + 1) % n) as u32, ((i + n - 1) % n) as u32])
            .collect();
        (dirs, neighbors)
    }

    /// Run Epochs 1→5 so Epoch 6 erodes real plate terrain with real hardness.
    fn through_epoch5<'a>(
        t: &'a Tables,
        dirs: &'a [Vec3],
        neighbors: &'a [Vec<u32>],
    ) -> (EpochCtx<'a>, Vec<HexState>) {
        let e1 = Epoch1::new(t, Epoch1Params::default(), 7);
        let ctx = EpochCtx { tables: t, dirs, neighbors, seed: 7 };
        let seed: Vec<HexState> = dirs.iter().map(|&d| HexState::new(e1.seed_hex(d))).collect();
        let e2 = Epoch2::default().apply(&ctx, &seed);
        let e3 = Epoch3::default().apply(&ctx, &e2);
        let e4 = Epoch4::default().apply(&ctx, &e3);
        let e5 = Epoch5::default().apply(&ctx, &e4);
        (ctx, e5)
    }

    #[test]
    fn erosion_grades_the_terrain_and_conserves_mass() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let out = Epoch6::default().apply(&ctx, &e5);

        let span = |v: &[HexState]| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for s in v {
                lo = lo.min(s.elevation);
                hi = hi.max(s.elevation);
            }
            hi - lo
        };
        // Erosion grades the macro relief down (peaks shed, lows fill).
        assert!(span(&out) < span(&e5), "erosion didn't reduce the relief span");
        // Material is moved, not created: total elevation ~conserved.
        let sum = |v: &[HexState]| v.iter().map(|s| s.elevation as f64).sum::<f64>();
        assert!((sum(&out) - sum(&e5)).abs() < 0.5, "erosion didn't conserve mass");
        assert!(out.iter().all(|s| (-1.0..=1.0).contains(&s.elevation)));
    }

    #[test]
    fn drainage_accumulates_downstream() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let out = Epoch6::default().apply(&ctx, &e5);
        // Some hex gathered more than the base rainfall — flow accumulated.
        let max_flow = out.iter().map(|s| s.flow).fold(0.0f32, f32::max);
        assert!(max_flow > Epoch6::default().rain, "flow never accumulated downstream");
    }

    #[test]
    fn biomes_are_assigned_and_varied() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let out = Epoch6::default().apply(&ctx, &e5);
        // Submerged hexes read Ocean; land hexes read a land biome.
        for s in &out {
            if s.water_depth > 0.0 {
                assert_eq!(s.biome, Biome::Ocean);
            } else {
                assert_ne!(s.biome, Biome::Ocean, "dry hex left as Ocean");
            }
        }
        let distinct: std::collections::BTreeSet<Biome> = out.iter().map(|s| s.biome).collect();
        assert!(distinct.len() >= 2, "only one biome over the whole ring: {distinct:?}");
    }

    #[test]
    fn deterministic_for_a_seed() {
        let t = tables();
        let (dirs, neighbors) = ring(24);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let a = Epoch6::default().apply(&ctx, &e5);
        let b = Epoch6::default().apply(&ctx, &e5);
        assert_eq!(a, b);
    }
}
