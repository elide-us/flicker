//! **Epoch 5 — mineralization & ore-vein formation** (world-gen spec, Epoch 5).
//!
//! Hydrothermal fluids circulate along the planet's **fault plumbing** — the
//! active plate boundaries Epoch 3 drew — and precipitate dissolved metal where
//! they cool, concentrating into **veins**. Two outputs:
//!
//! - A per-hex **`hydrothermal`** signature `0..1`: how mineralizing this hex is,
//!   from its boundary activity (convergent belts strongest, then transform
//!   faults and rifts), its volcanism, and nearby fluid (water proximity).
//! - **Veins**: each is a chain of hexes traced from a hot source *along the
//!   fault* (greedily toward the highest-signature neighbour), carrying one metal
//!   with a concentration that peaks at the source and tapers to the tips. The
//!   metal is one Epoch 2 **sank out of the crust** (iron and the heavier metals
//!   are still in the bulk `composition`); the vein brings it back **up into the
//!   crust** along the path. Mostly iron — the abundant one — with the occasional
//!   copper/silver/gold vein where the roll digs deeper into the heavy tail.
//!
//! This is the stage that breaks the crust's compositional uniformity: a hex on a
//! rich iron vein now reads iron at the surface, shifting both its dominant
//! element and its hardness — the differential the erosion sim later carves. The
//! per-hex `vein_element` / `vein_strength` are what the field sampler reads to
//! place the sub-hex filaments; the cross-hex path structure (spec's `veins`
//! list) is a later refinement — the per-hex membership is enough to render and
//! erode against.

use crate::pipeline::{EpochCtx, EpochTransform};
use crate::state::{Boundary, HexState, LifeStage};

/// Epoch 5 parameters.
pub struct Epoch5 {
    /// Hydrothermal drive from an active boundary: convergent gets the full
    /// value (scaled by orogeny), transform / divergent a fraction.
    pub boundary_drive: f32,
    /// Hydrothermal drive contributed by a hex's volcanism (`× volcanic`).
    pub volcanic_drive: f32,
    /// Hydrothermal boost when fluid is at hand (this hex or a neighbour is
    /// submerged) — fluids are what carry and drop the metal.
    pub water_drive: f32,
    /// A hex sources a vein only if its signature reaches this.
    pub vein_threshold: f32,
    /// Vein length (hexes) at a full-strength source; shorter sources run
    /// shorter veins. Major veins approach this, minor ones a few hexes.
    pub max_vein_len: usize,
    /// Cap on how many veins to trace (scales the cost on big worlds).
    pub max_veins: usize,
    /// Density (g/cm³) at/above which an element counts as a vein **metal** — the
    /// heavy fraction Epoch 2 differentiated out of the crust. Matches Epoch 2's
    /// `crust_density_max` so the candidates are exactly what sank.
    pub metal_density_min: f64,
    /// Metal mass deposited into the crust at a vein's core, as a fraction of the
    /// hex's crust total (scaled by the local concentration along the path).
    pub deposit_fraction: f64,
    /// Life potential (prebiotic × vent energy) above which precursors cross into
    /// prokaryotic **microbial** life. Lower → life takes hold more readily.
    pub microbial_threshold: f32,
    /// How strongly a hydrothermal vent boosts the precursor→life transition (the
    /// classic origin-of-life cradle): potential = `prebiotic × (1 + boost × hydro)`.
    pub vent_life_boost: f32,
}

impl Default for Epoch5 {
    fn default() -> Self {
        Self {
            boundary_drive: 0.7,
            volcanic_drive: 0.5,
            water_drive: 0.3,
            vein_threshold: 0.35,
            max_vein_len: 12,
            max_veins: 64,
            metal_density_min: 3.5,
            deposit_fraction: 0.35,
            microbial_threshold: 0.12,
            vent_life_boost: 1.0,
        }
    }
}

/// splitmix64 → `[0, 1)`, the deterministic per-vein / per-seed roll (same idiom
/// as Epoch 3).
fn rand01(z: u64) -> f64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

impl EpochTransform for Epoch5 {
    fn epoch(&self) -> u8 {
        5
    }

    fn apply(&self, ctx: &EpochCtx, prev: &[HexState]) -> Vec<HexState> {
        let n = prev.len();
        if n == 0 {
            return Vec::new();
        }
        let mut out = prev.to_vec();

        // 1. Per-hex hydrothermal signature: boundary plumbing + volcanism +
        //    fluid proximity. This is the "where can metal precipitate" field.
        let mut hydro = vec![0.0f32; n];
        for (i, s) in prev.iter().enumerate() {
            let boundary = match s.boundary {
                Boundary::Convergent => self.boundary_drive * (0.4 + 0.6 * s.orogeny),
                Boundary::Transform => self.boundary_drive * 0.7,
                Boundary::Divergent => self.boundary_drive * 0.5,
                Boundary::Interior => 0.0,
            };
            let volcanic = self.volcanic_drive * s.volcanic;
            let wet = s.water_depth > 0.0
                || ctx.neighbors[i].iter().any(|&nb| prev[nb as usize].water_depth > 0.0);
            let water = if wet { self.water_drive } else { 0.0 };
            hydro[i] = (boundary + volcanic + water).clamp(0.0, 1.0);
        }

        // 2. Vein sources: the hottest hexes, strongest first (deterministic).
        //    Skip a candidate that already sits on a traced vein so veins spread
        //    along the fault network rather than stacking on one hot spot.
        let mut sources: Vec<usize> = (0..n).filter(|&i| hydro[i] >= self.vein_threshold).collect();
        sources.sort_by(|&a, &b| {
            hydro[b].partial_cmp(&hydro[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
        });

        let mut on_vein = vec![false; n];
        let mut veins = 0usize;
        for &src in &sources {
            if veins >= self.max_veins {
                break;
            }
            if on_vein[src] {
                continue;
            }
            // Length scales with the source's heat; min 2 so a vein is a chain.
            let len = (2.0 + hydro[src] * self.max_vein_len as f32).round() as usize;
            let path = self.trace(src, &hydro, ctx, &on_vein, len);
            if path.len() < 2 {
                continue;
            }
            // Pick this vein's metal from what's locally in the bulk column.
            let Some(metal) = self.pick_metal(&prev[src], ctx, veins as u64) else {
                continue;
            };
            // Deposit along the path: concentration peaks at the source, tapers
            // to the tip. Bring the metal up into the crust (surface composition).
            let last = (path.len() - 1).max(1) as f32;
            for (step, &h) in path.iter().enumerate() {
                let conc = 1.0 - 0.85 * (step as f32 / last); // 1.0 .. 0.15
                let crust_total = out[h].crust.total();
                if crust_total > 0.0 {
                    let add = self.deposit_fraction * crust_total * conc as f64;
                    out[h].crust.add(metal, add);
                }
                // Strongest membership wins where veins cross.
                if conc >= out[h].vein_strength {
                    out[h].vein_element = Some(metal);
                    out[h].vein_strength = conc;
                }
                on_vein[h] = true;
            }
            veins += 1;
        }

        for (i, s) in out.iter_mut().enumerate() {
            s.hydrothermal = hydro[i];
            // Microbial life: Epoch 4's precursors cross into prokaryotic life
            // where energy pushes them over the edge — hydrothermal vents are the
            // classic cradle, so they multiply the odds. (High `prebiotic` already
            // implies liquid water + organics from Epoch 4, so no extra water test.)
            let potential = (s.prebiotic * (1.0 + self.vent_life_boost * hydro[i])).min(1.0);
            if potential >= self.microbial_threshold {
                s.life_stage = LifeStage::Microbial;
                s.biomass = potential;
            }
        }
        out
    }
}

impl Epoch5 {
    /// Trace a vein from `src` of at most `len` hexes, stepping each time to the
    /// neighbour with the highest hydrothermal signature (following the fault),
    /// never revisiting a hex and never crossing another vein.
    fn trace(
        &self,
        src: usize,
        hydro: &[f32],
        ctx: &EpochCtx,
        on_vein: &[bool],
        len: usize,
    ) -> Vec<usize> {
        let mut path = vec![src];
        let mut cur = src;
        while path.len() < len {
            let next = ctx.neighbors[cur]
                .iter()
                .map(|&nb| nb as usize)
                .filter(|&nb| !path.contains(&nb) && !on_vein[nb] && hydro[nb] > 0.0)
                .max_by(|&a, &b| {
                    hydro[a].partial_cmp(&hydro[b]).unwrap_or(std::cmp::Ordering::Equal).then(b.cmp(&a))
                });
            match next {
                Some(nb) => {
                    path.push(nb);
                    cur = nb;
                }
                None => break,
            }
        }
        path
    }

    /// Choose the vein's metal from the source hex's **bulk** composition (where
    /// the heavy metals Epoch 2 sank still live). Candidates are the dense
    /// elements, ranked by abundance; the roll usually lands on the most abundant
    /// (iron) but `r²` weighting digs into the tail often enough for the
    /// occasional copper/silver/gold vein. `salt` varies it per vein.
    fn pick_metal(&self, src: &HexState, ctx: &EpochCtx, salt: u64) -> Option<u8> {
        let mut metals: Vec<(u8, f64)> = src
            .composition
            .iter()
            .filter(|&(el, _)| {
                ctx.tables
                    .element_by_number(el)
                    .is_some_and(|e| e.density_g_cm3 as f64 >= self.metal_density_min)
            })
            .collect();
        if metals.is_empty() {
            return None;
        }
        metals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
        let r = rand01(ctx.seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let rank = (r * r * metals.len() as f64) as usize; // bias toward the top
        Some(metals[rank.min(metals.len() - 1)].0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldstate::Composition;
    use glam::Vec3;

    use crate::epoch1::{Epoch1, Epoch1Params};
    use crate::epoch2::Epoch2;
    use crate::epoch3::Epoch3;
    use crate::epoch4::Epoch4;

    const FE: u8 = 26;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/materials");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo data/materials loads")
    }

    /// A ring of `n` hexes around the equator — a minimal connected world.
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

    /// Run Epochs 1→2→3 so Epoch 5 has real boundaries / volcanism / bulk metals.
    fn through_epoch3<'a>(t: &'a Tables, dirs: &'a [Vec3], neighbors: &'a [Vec<u32>]) -> (EpochCtx<'a>, Vec<HexState>) {
        let e1 = Epoch1::new(t, Epoch1Params::default(), 7);
        let ctx = EpochCtx { tables: t, dirs, neighbors, seed: 7 };
        let seed: Vec<HexState> = dirs.iter().map(|&d| HexState::new(e1.seed_hex(d))).collect();
        let e2 = Epoch2::default().apply(&ctx, &seed);
        let e3 = Epoch3::default().apply(&ctx, &e2);
        (ctx, e3)
    }

    #[test]
    fn hydrothermal_concentrates_on_boundaries() {
        let t = tables();
        let (dirs, neighbors) = ring(30);
        let (ctx, e3) = through_epoch3(&t, &dirs, &neighbors);
        let out = Epoch5::default().apply(&ctx, &e3);

        assert!(out.iter().all(|s| (0.0..=1.0).contains(&s.hydrothermal)));
        // Interiors are inert; boundaries are where the signature lives.
        for s in &out {
            if s.boundary == Boundary::Interior && s.volcanic == 0.0 {
                assert_eq!(s.hydrothermal, 0.0, "an inert interior should not be hydrothermal");
            }
        }
        assert!(out.iter().any(|s| s.hydrothermal > 0.0), "no hydrothermal activity anywhere");
    }

    #[test]
    fn veins_form_and_carry_metal_into_the_crust() {
        let t = tables();
        let (dirs, neighbors) = ring(30);
        let (ctx, e3) = through_epoch3(&t, &dirs, &neighbors);
        let out = Epoch5::default().apply(&ctx, &e3);

        let vein_hexes: Vec<&HexState> = out.iter().filter(|s| s.vein_element.is_some()).collect();
        assert!(!vein_hexes.is_empty(), "no veins formed");
        for s in &vein_hexes {
            assert!(s.vein_strength > 0.0);
            let metal = s.vein_element.unwrap();
            // The vein deposited metal into the crust that wasn't there before.
            let before = e3.iter().find(|p| p.composition == s.composition).map(|p| p.crust.amount(metal));
            if let Some(before) = before {
                assert!(s.crust.amount(metal) >= before, "vein removed metal instead of adding it");
            }
            assert!(s.crust.amount(metal) > 0.0, "vein hex carries no metal in its crust");
        }
    }

    #[test]
    fn a_vein_runs_along_more_than_one_hex() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, e3) = through_epoch3(&t, &dirs, &neighbors);
        let out = Epoch5::default().apply(&ctx, &e3);
        // A traced vein deposits a full-strength core (1.0) and tapering members
        // (< 1.0) — so a vein that formed leaves more than one membership hex.
        let cores = out.iter().filter(|s| s.vein_strength >= 0.999).count();
        let members = out.iter().filter(|s| s.vein_strength > 0.0).count();
        assert!(cores >= 1, "no vein core");
        assert!(members > cores, "veins never extended past their source hex");
    }

    #[test]
    fn deterministic_for_a_seed() {
        let t = tables();
        let (dirs, neighbors) = ring(24);
        let (ctx, e3) = through_epoch3(&t, &dirs, &neighbors);
        let a = Epoch5::default().apply(&ctx, &e3);
        let b = Epoch5::default().apply(&ctx, &e3);
        assert_eq!(a, b);
    }

    /// Run Epochs 1→4 so Epoch 5 has real hydrothermal vents **and** prebiotic
    /// precursors (Epoch 4) to grow microbial life from.
    fn through_epoch4<'a>(
        t: &'a Tables,
        dirs: &'a [Vec3],
        neighbors: &'a [Vec<u32>],
    ) -> (EpochCtx<'a>, Vec<HexState>) {
        let (ctx, e3) = through_epoch3(t, dirs, neighbors);
        let e4 = Epoch4::default().apply(&ctx, &e3);
        (ctx, e4)
    }

    #[test]
    fn microbial_life_emerges_from_precursors() {
        let t = tables();
        let (dirs, neighbors) = ring(30);
        let (ctx, e4) = through_epoch4(&t, &dirs, &neighbors);
        assert!(e4.iter().any(|s| s.prebiotic > 0.0), "precondition: Epoch 4 made precursors");

        // A low threshold so any precursor cradle crosses into life — tests the
        // mechanism without depending on exact magnitudes.
        let out = Epoch5 { microbial_threshold: 0.01, ..Epoch5::default() }.apply(&ctx, &e4);
        assert!(
            out.iter().any(|s| s.life_stage == LifeStage::Microbial && s.biomass > 0.0),
            "no microbial life emerged at the precursor cradles"
        );
        for s in &out {
            assert!(s.biomass.is_finite() && (0.0..=1.0).contains(&s.biomass));
            // No precursors ⇒ no life, no standing biomass.
            if s.prebiotic <= 0.0 {
                assert!(s.life_stage < LifeStage::Microbial, "life with no precursors");
                assert_eq!(s.biomass, 0.0);
            }
        }
    }

    #[test]
    fn pick_metal_returns_a_dense_element() {
        let t = tables();
        let dirs = [Vec3::X];
        let neighbors = [vec![]];
        let ctx = EpochCtx { tables: &t, dirs: &dirs, neighbors: &neighbors, seed: 1 };
        // A column with iron present.
        let s = HexState::new(Composition::from_iter([(14, 8000.0), (FE, 2000.0)]));
        let e5 = Epoch5::default();
        let metal = e5.pick_metal(&s, &ctx, 0).expect("a dense metal is present");
        let density = t.element_by_number(metal).unwrap().density_g_cm3 as f64;
        assert!(density >= e5.metal_density_min, "picked a light element as ore metal");
    }
}
