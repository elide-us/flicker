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
//! Then each hex is classified into a **biome** from its temperature and
//! **precipitation** (both Epoch 4) plus its elevation above the sea, and the
//! **life thread** advances onto the land (fungus → flora), shedding the dead
//! `organics` that time-gated preservation banks as coal/oil/chalk `deposits`.
//! This is *macro* erosion at ~50-mi hex scale — deliberately coarse; the per-pass
//! water cycle refines it into real river valleys and smoothed relief.

use flicker_materials::PhysicalState;
use flicker_worldstate::Composition;

use crate::noise::fbm;
use crate::pipeline::{EpochCtx, EpochTransform};
use crate::state::{Biome, HexState, LifeStage};

/// Carbon — coal (on land) / oil (under sea) where organics escape decomposition.
const CARBON: u8 = 6;
/// Calcium — the carbonate (with carbon) that builds chalk / limestone.
const CALCIUM: u8 = 20;
/// Water depth (normalized) beyond which a submerged shelf is too deep for
/// carbonate plankton to build chalk — only the sunlit shelves accumulate it.
const CHALK_DEPTH: f32 = 0.6;

/// A drainage basin — the spec's cross-hex `watersheds` record: the hexes that all
/// drain (by steepest descent) to one terminal `outlet` sink. Reconstructed from
/// the per-hex [`HexState::watershed`] field by [`watersheds`].
#[derive(Clone, Debug, PartialEq)]
pub struct Watershed {
    /// Dense basin id (`0..count`).
    pub id: u32,
    /// The terminal sink hex everything in the basin drains to (a coastal/abyssal
    /// or endorheic low point).
    pub outlet: u32,
    /// Hex indices belonging to this basin.
    pub members: Vec<u32>,
}

/// Group a finished Epoch-6 layer into its drainage basins by the per-hex
/// [`HexState::watershed`] sink id (the cross-hex `watersheds` structure).
pub fn watersheds(layer: &[HexState]) -> Vec<Watershed> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (i, s) in layer.iter().enumerate() {
        groups.entry(s.watershed).or_default().push(i as u32);
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(id, (outlet, members))| Watershed { id: id as u32, outlet, members })
        .collect()
}

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
    /// Growth potential (warmth × moisture) above which land bears **flora**
    /// (forest) — `LifeStage::Floral`. Lower → greener planet.
    pub floral_threshold: f32,
    /// Growth potential above which land bears **fungus / sparse vegetation** —
    /// `LifeStage::Fungal` (below the flora threshold).
    pub fungal_threshold: f32,
    /// Dead-organic-matter (peat) shed per unit standing biomass per iteration —
    /// the coal/oil precursor that accumulates over the epoch's cycles.
    pub organics_rate: f32,
    /// How late wood-decomposers ("termites") arrived, `0..1` — the **fraction of
    /// dead organics preserved** as buried carbon (coal on land, oil under sea).
    /// `1` = they never came (a coal-rich planet, the Carboniferous window wide
    /// open); `0` = always present (organics all recycled, no coal).
    pub decomposer_onset: f32,
    /// How early carbonate-shelled plankton evolved, `0..1` — `1 - onset` is the
    /// fraction of the era they spent raining CaCO₃ onto warm shallow shelves
    /// (chalk). `0` = present the whole era (maximal chalk); `1` = never (none).
    pub carbonate_onset: f32,
    /// Scale of chalk accumulation in a perfect warm shallow carbonate sea.
    pub carbonate_rate: f32,
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
            floral_threshold: 0.45,
            fungal_threshold: 0.15,
            organics_rate: 0.05,
            decomposer_onset: 0.6,
            carbonate_onset: 0.3,
            carbonate_rate: 0.5,
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

        // Drainage basins: on the settled terrain, every hex drains by steepest
        // descent to a terminal sink (a local minimum). Follow the chains with path
        // compression so each hex records the sink it ends at — its watershed id.
        let down: Vec<Option<usize>> = (0..n).map(|i| lowest_neighbor(i, &elev, ctx)).collect();
        let mut sink = vec![u32::MAX; n];
        for start in 0..n {
            if sink[start] != u32::MAX {
                continue;
            }
            let mut path = vec![start];
            let mut cur = start;
            loop {
                match down[cur] {
                    Some(d) if sink[d] != u32::MAX => {
                        let s = sink[d];
                        for &p in &path {
                            sink[p] = s;
                        }
                        break;
                    }
                    Some(d) => {
                        path.push(d);
                        cur = d;
                    }
                    None => {
                        let s = cur as u32;
                        for &p in &path {
                            sink[p] = s;
                        }
                        break;
                    }
                }
            }
        }

        (0..n)
            .map(|i| {
                let mut s = prev[i].clone();
                s.elevation = elev[i];
                s.watershed = sink[i];
                s.flow = flow[i];
                s.sediment = sediment[i];
                s.water_depth = (sea - elev[i]).max(0.0);
                let above = (elev[i] - sea).max(0.0);
                // Moisture = Epoch 4's precipitation (the single rainfall truth),
                // plus a regional wobble so biome belts aren't perfectly zonal.
                let wobble = 0.15 * (fbm(ctx.dirs[i] * 3.0, 2, 0x0B10_E5A1, ctx.seed) as f32 - 0.5);
                let m = (s.precipitation + wobble).clamp(0.0, 1.0);
                s.biome = if s.water_depth > 0.0 {
                    Biome::Ocean
                } else {
                    classify(s.temperature, above, m, self.alpine_height)
                };

                // Life thread: on temperate-wet land, fungus then flora establish,
                // growing biomass and shedding dead `organics` (the coal precursor)
                // over the epoch's cycles. Advance-only (`max`) so ocean keeps its
                // Epoch-5 microbial life and stages never regress.
                let warmth = ((s.temperature + 8.0) / 30.0).clamp(0.0, 1.0);
                let growth = warmth * m;
                let supported = if s.water_depth > 0.0 || above > self.alpine_height {
                    LifeStage::Barren // ocean keeps microbial via max; alpine is bare
                } else if growth >= self.floral_threshold {
                    LifeStage::Floral
                } else if growth >= self.fungal_threshold {
                    LifeStage::Fungal
                } else {
                    LifeStage::Barren
                };
                let land_biomass = match supported {
                    LifeStage::Floral => growth,
                    LifeStage::Fungal => 0.4 * growth,
                    _ => 0.0,
                };
                s.life_stage = s.life_stage.max(supported);
                s.biomass = s.biomass.max(land_biomass);
                s.organics = s.biomass * self.organics_rate * self.iterations.max(1) as f32;

                // Preservation into the distinct `deposits` ledger. Coal/oil: the
                // fraction of dead organics that escaped late decomposers buries as
                // carbon (`decomposer_onset` = how late they came; land C reads as
                // coal, submerged C as oil).
                let mut deposits = Composition::new();
                let coal = s.organics * self.decomposer_onset;
                if coal > 0.0 {
                    deposits.add(CARBON, coal as f64);
                }
                // Chalk: carbonate-shelled plankton (present for `1 - carbonate_onset`
                // of the era) rain CaCO₃ onto warm shallow sea floors.
                if s.water_depth > 0.0 && s.life_stage >= LifeStage::Microbial {
                    let shallow = (1.0 - s.water_depth / CHALK_DEPTH).clamp(0.0, 1.0);
                    let chalk = (1.0 - self.carbonate_onset) * shallow * warmth * self.carbonate_rate;
                    if chalk > 0.0 {
                        deposits.add(CALCIUM, chalk as f64);
                        deposits.add(CARBON, chalk as f64 * 0.5); // carbonate C rides with the Ca
                    }
                }
                s.deposits = deposits;
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

    #[test]
    fn flora_and_organics_establish_on_temperate_wet_land() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let out = Epoch6::default().apply(&ctx, &e5);

        // Land life took hold (fungus or flora) and shed dead organics.
        assert!(
            out.iter().any(|s| s.life_stage >= LifeStage::Fungal),
            "no fungus/flora established on land"
        );
        assert!(out.iter().any(|s| s.organics > 0.0), "no dead organics accumulated");
        // The thread only advances — ocean keeps its Epoch-5 microbial life.
        for (before, after) in e5.iter().zip(&out) {
            assert!(after.life_stage >= before.life_stage, "life stage regressed");
            assert!(after.biomass.is_finite() && after.organics.is_finite());
        }

        // The Floral branch genuinely fires given lush thresholds.
        let lush = Epoch6 { floral_threshold: 0.0, fungal_threshold: 0.0, ..Epoch6::default() }
            .apply(&ctx, &e5);
        assert!(
            lush.iter().any(|s| s.life_stage == LifeStage::Floral && s.biomass > 0.0),
            "the Floral path never produced forest"
        );
    }

    #[test]
    fn drainage_partitions_into_watersheds() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let out = Epoch6::default().apply(&ctx, &e5);

        let basins = watersheds(&out);
        // Every hex lands in exactly one basin, draining to that basin's outlet.
        assert!(!basins.is_empty(), "no watersheds formed");
        let total: usize = basins.iter().map(|b| b.members.len()).sum();
        assert_eq!(total, out.len(), "watershed membership doesn't cover every hex once");
        for b in &basins {
            assert!(b.members.contains(&b.outlet), "a basin's outlet isn't one of its hexes");
            // The outlet is a true sink: no neighbour is strictly lower.
            let o = b.outlet as usize;
            for &nb in &ctx.neighbors[o] {
                assert!(out[nb as usize].elevation >= out[o].elevation, "outlet drains further downhill");
            }
        }
    }

    #[test]
    fn time_gates_preserve_coal_and_chalk() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e5) = through_epoch5(&t, &dirs, &neighbors);
        let carbon = |w: &[HexState]| w.iter().map(|s| s.deposits.amount(CARBON)).sum::<f64>();

        // Late decomposers (onset 1) preserve far more buried carbon than
        // ever-present ones (onset 0) — the Carboniferous coal window. Carbonate
        // is held equal, so the difference is the organics-derived coal.
        let coal_world = Epoch6 { decomposer_onset: 1.0, ..Epoch6::default() }.apply(&ctx, &e5);
        let rot_world = Epoch6 { decomposer_onset: 0.0, ..Epoch6::default() }.apply(&ctx, &e5);
        assert!(carbon(&coal_world) > carbon(&rot_world), "late decomposers should preserve more coal");

        // Chalk (calcium carbonate) forms in warm shallow seas once carbonate life
        // is present for the era.
        let chalky = Epoch6 { carbonate_onset: 0.0, ..Epoch6::default() }.apply(&ctx, &e5);
        assert!(
            chalky.iter().any(|s| s.deposits.amount(CALCIUM) > 0.0),
            "no chalk deposited in the warm shallow seas"
        );
        for s in &chalky {
            assert!(s.deposits.total().is_finite());
        }
    }
}
