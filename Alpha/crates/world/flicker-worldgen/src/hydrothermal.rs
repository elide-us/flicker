//! **Epoch-5 hydrothermal percolation** — the iterative reaction-transport that
//! grows ore veins, replacing Epoch 5's one-shot greedy path *trace*.
//!
//! Epoch 5 computes the per-hex **`hydrothermal`** field (fault plumbing +
//! volcanism + fluid proximity — the permeability/heat of the crust). This module
//! then runs the *fluid* on that field: hot **source** hexes leach dissolved metal
//! into a circulating fluid; the fluid **diffuses along the fault network**
//! (preferentially into high-permeability neighbours, so it hugs the plate
//! boundaries rather than seeping into inert interior); and a fraction
//! **precipitates** every step where it flows. Run for many steps, the precipitate
//! accumulates into **branching veins that taper away from their sources** — the
//! structure *emerges* from the transport (the manifesto's "simulate, don't
//! paint"), not from a hand-walked greedy line.
//!
//! Each vein's **metal** comes from an emergent **province**: a nearest-source BFS
//! over the graph tags every reached hex with the metal of the source whose region
//! it lies in, so iron / copper / gold sort into distinct territories instead of
//! one metal per traced string.
//!
//! **Conservation.** This produces only the per-hex *concentration field*
//! (`vein_strength` + a `vein_element` marker) — it fabricates **no** mass. The
//! actual ore MASS is formed conservedly downstream by the compound former, which
//! reads `vein_strength` / `hydrothermal` to concentrate the harvestable ore
//! *compounds* (Native Gold, Chalcopyrite, …) out of each cell's **free elements**,
//! capped so the element mass locked in compounds never exceeds the ledger. The
//! bulk `composition` is never touched here. The transport itself is conservative
//! (double-buffered symmetric flux).

use std::collections::VecDeque;

use flicker_materials::{ElementId, Tables};
use flicker_worldstate::Composition;

use crate::pipeline::EpochCtx;
use crate::state::HexState;

/// Metal leached into the fluid per step at a source, scaled by its heat.
const LEACH: f32 = 0.5;
/// Fraction of a hex's fluid that diffuses to neighbours per step.
const SPREAD: f32 = 0.5;
/// Fraction of a hex's fluid that precipitates (fluid → vein) per step.
const PRECIP: f32 = 0.25;
/// Floor on a neighbour's diffusion weight, so fluid can seep one ring into
/// cooler rock (a vein's fringe) while still strongly favouring the fault.
const PERM_FLOOR: f32 = 0.05;
/// Minimum normalised concentration (fraction of the peak precipitate) for a hex
/// to count as a **vein** — the faint diffusion halo that reaches every cell is
/// not ore, so it gets no `vein_element` and no crust enrichment. Keeps veins
/// selective (concentrated near the fault) and leaves non-vein rock for the
/// node guarantee to place trace-element nodes on.
const VEIN_MIN_CONC: f32 = 0.25;

/// Tunables for the percolation (the rest are the module `const`s above).
pub struct HydrothermalParams {
    /// A hex sources fluid only if its `hydrothermal` signature reaches this.
    pub vein_threshold: f32,
    /// Density (g/cm³) at/above which a bulk element counts as a vein **metal**
    /// (matches Epoch 2's `crust_density_max` — exactly what sank out of the crust).
    pub metal_density_min: f64,
    /// Cap on how many sources drive the fluid (scales cost on big worlds).
    pub max_sources: usize,
}

impl Default for HydrothermalParams {
    fn default() -> Self {
        Self {
            vein_threshold: 0.35,
            metal_density_min: 3.5,
            max_sources: 64,
        }
    }
}

/// splitmix64 → `[0, 1)` — the deterministic per-source metal roll (same idiom as
/// Epoch 3 / Epoch 5).
fn rand01(z: u64) -> f64 {
    let mut z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Pick a source's vein metal from its **bulk** composition (where the heavy
/// metals Epoch 2 sank still live): dense elements ranked by abundance, the roll
/// `r²`-biased toward the top (usually iron, occasionally the copper/silver/gold
/// tail). `salt` varies it per source.
fn source_metal(
    comp: &Composition,
    tables: &Tables,
    density_min: f64,
    seed: u64,
    salt: u64,
) -> Option<ElementId> {
    let mut metals: Vec<(ElementId, f64)> = comp
        .iter()
        .filter(|&(el, _)| {
            tables
                .element_by_number(el)
                .is_some_and(|e| e.density_g_cm3 as f64 >= density_min)
        })
        .collect();
    if metals.is_empty() {
        return None;
    }
    metals.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    let r = rand01(seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let rank = (r * r * metals.len() as f64) as usize; // bias toward the abundant top
    Some(metals[rank.min(metals.len() - 1)].0)
}

/// Grow ore veins by hydrothermal percolation, in place. Reads each cell's
/// `hydrothermal` signature (set by [`crate::Epoch5`]), diffuses + precipitates a
/// dissolved-metal fluid for `steps` passes, and writes the `vein_element` /
/// `vein_strength` **concentration field** (which the compound former then turns
/// into conserved ore compounds). Fabricates no mass; `composition` untouched.
pub fn run_hydrothermal_veins(
    cells: &mut [HexState],
    ctx: &EpochCtx,
    params: &HydrothermalParams,
    steps: u32,
) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    let nbr = ctx.neighbors;
    let hydro: Vec<f32> = cells.iter().map(|c| c.hydrothermal).collect();

    // Sources: the hottest hexes, strongest first (deterministic), capped. Each is
    // assigned the metal it leaches, drawn from its own bulk column.
    let mut sources: Vec<usize> = (0..n)
        .filter(|&i| hydro[i] >= params.vein_threshold)
        .collect();
    sources.sort_by(|&a, &b| {
        hydro[b]
            .partial_cmp(&hydro[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    sources.truncate(params.max_sources);
    let sourced: Vec<(usize, ElementId)> = sources
        .iter()
        .filter_map(|&s| {
            source_metal(
                &cells[s].composition,
                ctx.tables,
                params.metal_density_min,
                ctx.seed,
                s as u64,
            )
            .map(|m| (s, m))
        })
        .collect();
    if sourced.is_empty() {
        return;
    }

    // Provinces: nearest-source BFS tags every reached hex with the metal of the
    // source region it belongs to — so a precipitating hex knows which metal drops.
    let carrier = province_carriers(n, nbr, &sourced);

    // Reaction-transport: leach → diffuse (permeability-weighted, conservative) →
    // precipitate, for `steps` passes.
    let mut fluid = vec![0.0f32; n];
    let mut vein = vec![0.0f32; n];
    for _ in 0..steps.max(1) {
        for &(s, _) in &sourced {
            fluid[s] += LEACH * hydro[s];
        }
        // Diffuse: each cell sheds SPREAD of its fluid to neighbours in proportion
        // to their permeability (double-buffered so a parcel can't cascade through
        // itself in one step; symmetric flux ⇒ conservative).
        let mut next = fluid.clone();
        for i in 0..n {
            if fluid[i] <= 0.0 {
                continue;
            }
            let wsum: f32 = nbr[i]
                .iter()
                .map(|&nb| hydro[nb as usize].max(PERM_FLOOR))
                .sum();
            if wsum <= 0.0 {
                continue;
            }
            let outflow = SPREAD * fluid[i];
            next[i] -= outflow;
            for &nb in &nbr[i] {
                let w = hydro[nb as usize].max(PERM_FLOOR) / wsum;
                next[nb as usize] += outflow * w;
            }
        }
        fluid = next;
        for i in 0..n {
            let p = PRECIP * fluid[i];
            fluid[i] -= p;
            vein[i] += p;
        }
    }

    // Express the precipitate: normalise to a 0..1 concentration for `vein_strength`
    // and lift the metal up into the (derived) crust in that proportion.
    let vmax = vein.iter().cloned().fold(0.0f32, f32::max);
    if vmax <= 0.0 {
        return;
    }
    for i in 0..n {
        let conc = vein[i] / vmax; // 0..1 emergent concentration
        if conc < VEIN_MIN_CONC {
            continue; // faint diffusion halo — not an ore vein
        }
        let Some(metal) = carrier[i] else { continue };
        // Mark the concentration field only — the ORE MASS is formed conservedly
        // into the compound ledger by the former (which reads `vein_strength`);
        // nothing is fabricated here. Strongest membership wins where provinces meet.
        if conc >= cells[i].vein_strength {
            cells[i].vein_element = Some(metal);
            cells[i].vein_strength = conc;
        }
    }
}

/// Multi-source BFS: label each hex with the metal of the nearest source (graph
/// distance), ties broken by source order — the emergent ore provinces.
fn province_carriers(
    n: usize,
    nbr: &[Vec<u32>],
    sourced: &[(usize, ElementId)],
) -> Vec<Option<ElementId>> {
    let mut carrier = vec![None; n];
    let mut dist = vec![u32::MAX; n];
    let mut queue = VecDeque::new();
    for &(s, metal) in sourced {
        if dist[s] != 0 {
            dist[s] = 0;
            carrier[s] = Some(metal);
            queue.push_back(s);
        }
    }
    while let Some(i) = queue.pop_front() {
        let d = dist[i] + 1;
        for &nb in &nbr[i] {
            let nb = nb as usize;
            if d < dist[nb] {
                dist[nb] = d;
                carrier[nb] = carrier[i];
                queue.push_back(nb);
            }
        }
    }
    carrier
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use glam::Vec3;

    use crate::epoch1::{Epoch1, Epoch1Params};
    use crate::epoch2::Epoch2;
    use crate::epoch3::Epoch3;
    use crate::pipeline::EpochTransform;
    use crate::Epoch5;

    fn tables() -> Tables {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/data"
        );
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
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

    /// Epochs 1→3 then Epoch 5's analytic base with the greedy trace OFF
    /// (`max_veins = 0`), so we get the `hydrothermal` field alone for the module
    /// to run on — exactly how the engine drives it.
    fn hydro_base<'a>(
        t: &'a Tables,
        dirs: &'a [Vec3],
        neighbors: &'a [Vec<u32>],
    ) -> (EpochCtx<'a>, Vec<HexState>) {
        let e1 = Epoch1::new(t, Epoch1Params::default(), 7);
        let ctx = EpochCtx {
            tables: t,
            dirs,
            neighbors,
            seed: 7,
        };
        let seed: Vec<HexState> = dirs
            .iter()
            .map(|&d| HexState::new(e1.seed_hex(d)))
            .collect();
        let e2 = Epoch2::default().apply(&ctx, &seed);
        let e3 = Epoch3::default().apply(&ctx, &e2);
        let base = Epoch5 {
            max_veins: 0,
            ..Epoch5::default()
        }
        .apply(&ctx, &e3);
        (ctx, base)
    }

    #[test]
    fn percolation_grows_a_selective_vein_field() {
        let t = tables();
        let (dirs, neighbors) = ring(30);
        let (ctx, base) = hydro_base(&t, &dirs, &neighbors);
        // Precondition: the base has a hydrothermal field but no veins yet.
        assert!(
            base.iter().any(|s| s.hydrothermal > 0.0),
            "no hydrothermal field to run on"
        );
        assert!(
            base.iter().all(|s| s.vein_element.is_none()),
            "greedy veins should be off"
        );

        let mut cells = base.clone();
        run_hydrothermal_veins(&mut cells, &ctx, &HydrothermalParams::default(), 24);

        let vein_hexes: Vec<&HexState> =
            cells.iter().filter(|s| s.vein_element.is_some()).collect();
        assert!(!vein_hexes.is_empty(), "no veins precipitated");
        // Selective, not a global wash — the faint halo is excluded.
        assert!(
            vein_hexes.len() < cells.len(),
            "every cell became a vein (not selective)"
        );
        for s in &vein_hexes {
            assert!((0.0..=1.0).contains(&s.vein_strength) && s.vein_strength > 0.0);
            // The concentration field marks a dense ore metal (its province).
            let metal = s.vein_element.unwrap();
            let dense = t.element_by_number(metal).unwrap().density_g_cm3 as f64;
            assert!(
                dense >= HydrothermalParams::default().metal_density_min,
                "vein metal is light"
            );
        }
        // Conserved: percolation fabricates no crust mass (the former does the ore).
        let crust_before: f64 = base.iter().map(|s| s.crust.total()).sum();
        let crust_after: f64 = cells.iter().map(|s| s.crust.total()).sum();
        assert!(
            (crust_after - crust_before).abs() < 1e-6,
            "percolation must not fabricate crust"
        );
    }

    #[test]
    fn a_vein_spreads_past_its_source_into_a_chain() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, base) = hydro_base(&t, &dirs, &neighbors);
        let mut cells = base;
        run_hydrothermal_veins(&mut cells, &ctx, &HydrothermalParams::default(), 24);
        let cores = cells.iter().filter(|s| s.vein_strength >= 0.999).count();
        let members = cells.iter().filter(|s| s.vein_strength > 0.0).count();
        assert!(cores >= 1, "no vein core precipitated");
        assert!(
            members > cores,
            "the vein never spread past its source (no chain emerged)"
        );
    }

    #[test]
    fn the_bulk_element_ledger_is_untouched() {
        let t = tables();
        let (dirs, neighbors) = ring(30);
        let (ctx, base) = hydro_base(&t, &dirs, &neighbors);
        let before: f64 = base.iter().map(|s| s.composition.total()).sum();
        let mut cells = base;
        run_hydrothermal_veins(&mut cells, &ctx, &HydrothermalParams::default(), 24);
        let after: f64 = cells.iter().map(|s| s.composition.total()).sum();
        assert!(
            (after - before).abs() < 1e-6,
            "percolation must not touch the bulk ledger"
        );
    }

    #[test]
    fn longer_percolation_spreads_the_veins_wider() {
        let t = tables();
        let (dirs, neighbors) = ring(40);
        let (ctx, base) = hydro_base(&t, &dirs, &neighbors);
        let vein_cells = |c: &[HexState]| c.iter().filter(|s| s.vein_strength > 0.0).count();
        let mut brief = base.clone();
        run_hydrothermal_veins(&mut brief, &ctx, &HydrothermalParams::default(), 2);
        let mut mature = base;
        run_hydrothermal_veins(&mut mature, &ctx, &HydrothermalParams::default(), 40);
        assert!(vein_cells(&brief) >= 1, "a brief run still seeds a vein");
        assert!(
            vein_cells(&mature) >= vein_cells(&brief),
            "longer circulation should spread the veins at least as wide ({} vs {})",
            vein_cells(&mature),
            vein_cells(&brief)
        );
    }

    #[test]
    fn deterministic_for_a_seed() {
        let t = tables();
        let (dirs, neighbors) = ring(24);
        let (ctx, base) = hydro_base(&t, &dirs, &neighbors);
        let mut a = base.clone();
        let mut b = base;
        run_hydrothermal_veins(&mut a, &ctx, &HydrothermalParams::default(), 16);
        run_hydrothermal_veins(&mut b, &ctx, &HydrothermalParams::default(), 16);
        assert_eq!(a, b);
    }
}
