//! **Epoch-3 iterative tectonics** — the emergent hotspot chains.
//!
//! Epoch 3's plate partition + isostasy + boundary deformation are computed once
//! (`Epoch3::apply`); this adds the part that *has* to be time-stepped to read
//! right: **a plate drifting over a fixed mantle plume**. Each step deposits uplift
//! at whatever cell currently sits over each hotspot (the plume forcing material up
//! *now*), then **advects that built relief downstream along the plate's drift** —
//! so as the plate moves, the fresh volcano stays over the plume while the older
//! rock is carried away, and a **chain trails out in the drift direction**. The
//! island chain is not drawn; it *emerges* from depositing + carrying material over
//! many steps (the manifesto's "simulate, don't paint").
//!
//! This moves the *relief* (shape), not the conserved element ledger — elevation is
//! a derived quantity here, so there is no mass to leak.

use glam::Vec3;

use crate::pipeline::EpochCtx;
use crate::state::HexState;

/// Fraction of a cell's hotspot relief carried to its down-drift neighbour per step
/// — sets how fast the chain trails away from the plume.
const TRAIL_ADVECT: f32 = 0.35;

/// Evenly-distributed fixed mantle plumes on the unit sphere (Fibonacci lattice,
/// rotated by the seed so reseeding moves them).
fn place_hotspots(count: usize, seed: u64) -> Vec<Vec3> {
    if count == 0 {
        return Vec::new();
    }
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let off = ((seed & 0xffff) as f32 / 65535.0) * std::f32::consts::TAU;
    (0..count)
        .map(|k| {
            let i = k as f32 + 0.5;
            let y = 1.0 - 2.0 * i / count as f32;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let theta = golden * i + off;
            Vec3::new(r * theta.cos(), y, r * theta.sin())
        })
        .collect()
}

/// Build the hotspot chains in place: deposit at each plume and advect the relief
/// along the drift for `steps` passes, then fold it into `elevation`/`volcanic`.
///
/// `flow[i]` = cell `i`'s per-cell convection drift direction (the curved plate motion);
/// `uplift` is the target peak height at an active plume. `plate_of` is unused now that
/// the drift is per-cell, kept for call-site symmetry with the other tectonic passes.
pub fn run_tectonic_hotspots(
    cells: &mut [HexState],
    ctx: &EpochCtx,
    _plate_of: &[usize],
    flow: &[Vec3],
    hotspots: usize,
    steps: u32,
    uplift: f32,
) {
    let n = cells.len();
    if hotspots == 0 || n == 0 {
        return;
    }
    let dirs = ctx.dirs;
    let nbr = ctx.neighbors;
    let spots = place_hotspots(hotspots, ctx.seed);

    // The cell sitting over each plume (fixed grid + fixed plume ⇒ compute once).
    let spot_cells: Vec<usize> = spots
        .iter()
        .map(|&spot| {
            (0..n)
                .max_by(|&a, &b| dirs[a].dot(spot).total_cmp(&dirs[b].dot(spot)))
                .unwrap_or(0)
        })
        .collect();

    // Each cell's drift direction in its own tangent plane (the local convection flow).
    let drift_dir: Vec<Vec3> = (0..n)
        .map(|i| {
            let d = flow.get(i).copied().unwrap_or(Vec3::ZERO);
            (d - d.dot(dirs[i]) * dirs[i]).normalize_or_zero()
        })
        .collect();

    // Steady-state peak ≈ deposit / TRAIL_ADVECT, so deposit this to peak at `uplift`.
    let deposit = uplift * TRAIL_ADVECT;
    let mut relief = vec![0.0f32; n];

    for _ in 0..steps.max(1) {
        for &c in &spot_cells {
            relief[c] += deposit;
        }
        // Carry each cell's relief to its down-drift neighbour (read old, write new).
        let mut next = relief.clone();
        for i in 0..n {
            if relief[i] <= 0.0 {
                continue;
            }
            let dd = drift_dir[i];
            if dd.length_squared() < 1e-8 {
                continue;
            }
            let mut best = None;
            let mut best_align = 0.0f32;
            for &nj in &nbr[i] {
                let off = (dirs[nj as usize] - dirs[i]).normalize_or_zero();
                let a = off.dot(dd);
                if a > best_align {
                    best_align = a;
                    best = Some(nj as usize);
                }
            }
            if let Some(j) = best {
                let moved = TRAIL_ADVECT * relief[i];
                next[i] -= moved;
                next[j] += moved;
            }
        }
        relief = next;
    }

    for (i, c) in cells.iter_mut().enumerate() {
        if relief[i] > 0.0 {
            c.elevation = (c.elevation + relief[i]).clamp(-1.0, 1.5);
            c.volcanic = (c.volcanic + relief[i]).clamp(0.0, 1.0);
        }
    }
}

/// Oceanic convergence builds only a damped island arc; buoyant continental crust
/// lifts the full arc (matches Epoch 3's one-shot `OCEANIC_OROGENY`).
const OROGENY_OCEANIC: f32 = 0.25;
/// Volcanic-**arc** uplift rate per step on the *overriding* plate at a subduction
/// margin. Modest — the bulk convergent relief (continental collision belts) comes
/// from the isostatic crust-pile (Airy) now, so this is only the subduction arc and
/// must not restack a second belt on top of the piled crust.
const OROGENY_RATE: f32 = 0.3;
/// Fraction of a hex's accumulated uplift that spreads to its neighbours per step —
/// how a mountain belt **widens inland** over deep time (a range, not a ridge line).
const OROGENY_DIFFUSE: f32 = 0.12;
/// Isostatic ceiling on accumulated uplift (a crustal root can only float so high).
const OROGENY_MAX: f32 = 1.0;

/// Buoyancy gap (per-plate mean rank) above which the lighter plate **overrides** and
/// the heavier one **subducts** — below it the two collide symmetrically (continental
/// collision, both pile up). Sets when a trench+arc forms vs a collision belt.
const SUBDUCT_MARGIN: f32 = 0.18;
/// Trench-deepening rate per step on the subducting (down-going) plate at a
/// convergent margin — the deepest valleys on the planet.
const TRENCH_RATE: f32 = 0.35;
/// Rift-deepening rate per step at a divergent margin (a spreading valley).
const RIFT_RATE: f32 = 0.25;
/// Ceiling on accumulated trench/rift depth (before the `rift_scale` multiply).
const DEPRESS_MAX: f32 = 2.0;

/// Strongest plate boundary at hex `i`: the signed closing rate from the per-cell
/// **`flow`** (`+` = the crust moves **together** → compression; `-` = **apart** → a
/// rift) AND the plate on the far side of that edge (so the caller can compare
/// buoyancies and decide who subducts). Only inter-plate edges count — subduction is
/// between two *different* plates. Physically `(v_i − v_nb)·across > 0` ⇒ i advances.
fn strongest_boundary(i: usize, ctx: &EpochCtx, plate_of: &[usize], flow: &[Vec3]) -> (f32, usize) {
    let p = plate_of[i];
    let vi = flow.get(i).copied().unwrap_or(Vec3::ZERO);
    let d = ctx.dirs[i];
    let mut strongest = 0.0f32;
    let mut other = p;
    for &nb in &ctx.neighbors[i] {
        let q = plate_of[nb as usize];
        if q == p {
            continue;
        }
        let across = (ctx.dirs[nb as usize] - d).normalize_or_zero();
        let closing = (vi - flow.get(nb as usize).copied().unwrap_or(Vec3::ZERO)).dot(across);
        if closing.abs() > strongest.abs() {
            strongest = closing;
            other = q;
        }
    }
    (strongest, other)
}

/// **Epoch-3 deep-time subduction deformation** — the iterative tectonic *asymmetry*
/// that the isostatic crust-pile can't express. Over `steps` passes (each a slice of
/// the ~1.5 BY the plates drift), each convergent margin resolves **asymmetrically
/// from the material**:
/// - **Subduction** — where one plate is clearly lighter (higher mean buoyancy) than
///   the other, the heavier one dives: its boundary hexes deepen into a **trench**
///   (the planet's deepest valleys) while the lighter overrider grows a volcanic
///   **arc** (offset inland by the diffusion).
/// - **Collision** — where the two plates are of similar buoyancy (two continents),
///   neither subducts and this pass adds **no belt**: that relief is the piled,
///   collided crust floating up via the isostatic `crust_pile` (Airy) in
///   [`Epoch3::apply`](crate::Epoch3::apply). Building a belt here too would
///   double-count the same convergence.
///
/// Arc uplift **accumulates and diffuses inland** (grows tall AND wide, then
/// saturates); trenches and **rift** valleys (divergent margins) accumulate as sharp
/// depressions. `buoyancy[i]` is the per-hex continentality rank (Epoch 3
/// `buoyancy_ranks`); `uplift_scale` / `rift_scale` are Epoch 3's `mountain_uplift` /
/// `rift_drop`.
///
/// Moves derived *relief* (elevation/orogeny/volcanism), not the conserved element
/// ledger — so there is no mass to leak.
#[allow(clippy::too_many_arguments)]
pub fn run_orogeny(
    cells: &mut [HexState],
    ctx: &EpochCtx,
    plate_of: &[usize],
    flow: &[Vec3],
    buoyancy: &[f32],
    uplift_scale: f32,
    rift_scale: f32,
    threshold: f32,
    steps: u32,
) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    // Per-plate mean buoyancy — decides which plate overrides and which subducts.
    let np = plate_of.iter().copied().max().map(|m| m + 1).unwrap_or(0);
    let mut bsum = vec![0.0f32; np];
    let mut bcnt = vec![0u32; np];
    for (i, &p) in plate_of.iter().enumerate() {
        bsum[p] += buoyancy.get(i).copied().unwrap_or(0.0);
        bcnt[p] += 1;
    }
    let plate_buoy: Vec<f32> = (0..np)
        .map(|p| {
            if bcnt[p] > 0 {
                bsum[p] / bcnt[p] as f32
            } else {
                0.0
            }
        })
        .collect();

    // The (time-independent) strongest boundary at each hex: signed closing + the
    // plate across it.
    let bnd: Vec<(f32, usize)> = (0..n)
        .map(|i| strongest_boundary(i, ctx, plate_of, flow))
        .collect();

    // Two accumulating fields over deep time: arc/collision uplift (diffuses inland
    // → broad ranges) and trench/rift depression (sharp valleys, no diffusion).
    let mut uplift = vec![0.0f32; n];
    let mut depress = vec![0.0f32; n];
    for _ in 0..steps.max(1) {
        // Diffuse the uplift (widen existing ranges), double-buffered.
        let mut next = uplift.clone();
        for i in 0..n {
            if uplift[i] <= 0.0 || ctx.neighbors[i].is_empty() {
                continue;
            }
            let out = OROGENY_DIFFUSE * uplift[i];
            next[i] -= out;
            let share = out / ctx.neighbors[i].len() as f32;
            for &nb in &ctx.neighbors[i] {
                next[nb as usize] += share;
            }
        }
        // Sources, resolved per boundary from the material buoyancy contrast.
        for i in 0..n {
            let (c, q) = bnd[i];
            let p = plate_of[i];
            if c > threshold {
                let mine = plate_buoy.get(p).copied().unwrap_or(0.0);
                let theirs = plate_buoy.get(q).copied().unwrap_or(0.0);
                if mine + SUBDUCT_MARGIN < theirs {
                    // I'm the heavier plate → I subduct → a trench opens here.
                    depress[i] = (depress[i] + TRENCH_RATE * (c - threshold)).min(DEPRESS_MAX);
                } else if theirs + SUBDUCT_MARGIN < mine {
                    // I'm the lighter plate → I override → a volcanic arc lifts here.
                    // (Symmetric continental collision hits neither branch: no belt —
                    // that relief is the piled crust via isostasy, not a second source.)
                    let cont = OROGENY_OCEANIC
                        + (1.0 - OROGENY_OCEANIC) * buoyancy.get(i).copied().unwrap_or(0.0);
                    next[i] += OROGENY_RATE * (c - threshold) * cont;
                }
            } else if c < -threshold {
                // Divergent margin → a rift valley deepens.
                depress[i] = (depress[i] + RIFT_RATE * (-c - threshold)).min(DEPRESS_MAX);
            }
            next[i] = next[i].min(OROGENY_MAX);
        }
        uplift = next;
    }

    // Express the accumulated deformation as relief: arcs/belts up, trenches/rifts down.
    for i in 0..n {
        let mut elev = cells[i].elevation;
        if uplift[i] > 0.0 {
            elev += uplift_scale * uplift[i];
            cells[i].orogeny = uplift[i].min(1.0);
            cells[i].volcanic = (cells[i].volcanic + 0.4 * uplift[i]).clamp(0.0, 1.0);
        }
        if depress[i] > 0.0 {
            elev -= rift_scale * depress[i];
        }
        cells[i].elevation = elev.clamp(-1.0, 1.5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_worldstate::Composition;

    fn tables() -> Tables {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/data");
        Tables::from_source(&JsonTableSource::new(dir)).expect("repo Alpha/content/data loads")
    }

    /// An equatorial ring drifting eastward: the plume cell rises highest and the
    /// relief trails to its down-drift neighbours (a chain), not upstream.
    #[test]
    fn a_drifting_plate_grows_a_chain_downstream_of_the_plume() {
        let t = tables();
        let n = 24;
        let dirs: Vec<Vec3> = (0..n)
            .map(|k| {
                let a = -(k as f32) / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n)
            .map(|k| vec![((k + n - 1) % n) as u32, ((k + 1) % n) as u32])
            .collect();
        let ctx = EpochCtx {
            tables: &t,
            dirs: &dirs,
            neighbors: &neighbors,
            seed: 0,
        };
        let mut cells: Vec<HexState> = (0..n)
            .map(|_| HexState::new(Composition::from_iter([(8u8, 1.0)])))
            .collect();
        // One plate; a per-cell eastward flow (Y × dir points to k+1 at each cell).
        let plate_of = vec![0usize; n];
        let flow: Vec<Vec3> = (0..n).map(|k| Vec3::Y.cross(dirs[k])).collect();

        run_tectonic_hotspots(&mut cells, &ctx, &plate_of, &flow, 1, 30, 0.8);

        // Something rose (a chain formed).
        let peak = cells.iter().map(|c| c.elevation).fold(f32::MIN, f32::max);
        assert!(peak > 0.05, "no hotspot uplift formed (peak {peak})");
        // The relief spread beyond a single cell (a trail, not a spike).
        let raised = cells.iter().filter(|c| c.elevation > 0.01).count();
        assert!(
            raised >= 3,
            "hotspot made a spike, not a chain ({raised} cells raised)"
        );
    }

    /// The load-bearing physics check: at a subduction margin (a buoyancy contrast)
    /// the **overriding** plate lifts a volcanic **arc** (compression → uplift), and
    /// deep time accumulates more relief. This pins the sign convention — arcs at
    /// convergence, not at rifts. (A *symmetric* collision builds no belt here — that
    /// relief is the isostatic crust-pile in `Epoch3::apply`, tested there.)
    #[test]
    fn a_subduction_arc_grows_over_deep_time() {
        let t = tables();
        let n = 24;
        let dirs: Vec<Vec3> = (0..n)
            .map(|k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n)
            .map(|k| vec![((k + n - 1) % n) as u32, ((k + 1) % n) as u32])
            .collect();
        let ctx = EpochCtx {
            tables: &t,
            dirs: &dirs,
            neighbors: &neighbors,
            seed: 0,
        };
        // Plate 0 = hexes 0..12, plate 1 = 12..24; motions chosen so the two close
        // on the boundary near hex 12 (both drive material into the seam).
        let plate_of: Vec<usize> = (0..n).map(|k| usize::from(k >= n / 2)).collect();
        let motion = [Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 1.0)];
        // Per-cell flow = each cell's plate motion (the two close on the boundary).
        let flow: Vec<Vec3> = plate_of.iter().map(|&p| motion[p]).collect();
        // Buoyancy contrast: plate 1 (light) overrides plate 0 (heavy) → an arc lifts
        // on plate 1 (and plate 0 digs a trench).
        let buoyancy: Vec<f32> = (0..n).map(|k| if k < n / 2 { 0.1 } else { 0.9 }).collect();
        let base: Vec<HexState> = dirs
            .iter()
            .map(|_| HexState::new(Composition::from_iter([(8u8, 1.0)])))
            .collect();

        let mut brief = base.clone();
        let mut mature = base;
        run_orogeny(
            &mut brief, &ctx, &plate_of, &flow, &buoyancy, 0.6, 0.25, 0.15, 4,
        );
        run_orogeny(
            &mut mature,
            &ctx,
            &plate_of,
            &flow,
            &buoyancy,
            0.6,
            0.25,
            0.15,
            40,
        );

        let peak = |c: &[HexState]| c.iter().map(|s| s.elevation).fold(f32::MIN, f32::max);
        let relief = |c: &[HexState]| c.iter().map(|s| s.elevation.max(0.0) as f64).sum::<f64>();
        // Compression lifted an arc where the light plate overrides.
        assert!(
            peak(&brief) > 0.0,
            "subduction built no arc (sign inverted?)"
        );
        // Deep time accumulates more total relief (arc piles up + widens inland).
        assert!(
            relief(&mature) > relief(&brief),
            "deep time didn't build more relief ({} vs {})",
            relief(&mature),
            relief(&brief)
        );
    }

    /// Subduction asymmetry: where a heavy (low-buoyancy) plate converges on a light
    /// one, the heavy plate dives into a **trench** and the light plate lifts an
    /// **arc** — the trench + arc signature (and the trench is a deep valley).
    #[test]
    fn subduction_digs_a_trench_and_lifts_an_arc() {
        let t = tables();
        let n = 24;
        let dirs: Vec<Vec3> = (0..n)
            .map(|k| {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), 0.0, a.sin())
            })
            .collect();
        let neighbors: Vec<Vec<u32>> = (0..n)
            .map(|k| vec![((k + n - 1) % n) as u32, ((k + 1) % n) as u32])
            .collect();
        let ctx = EpochCtx {
            tables: &t,
            dirs: &dirs,
            neighbors: &neighbors,
            seed: 0,
        };
        let plate_of: Vec<usize> = (0..n).map(|k| usize::from(k >= n / 2)).collect();
        // Converge on the boundary near hex 12 (per-cell flow = each cell's plate motion).
        let motion = [Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 1.0)];
        let flow: Vec<Vec3> = plate_of.iter().map(|&p| motion[p]).collect();
        // Plate 0 = heavy oceanic (low buoyancy); plate 1 = light continental (high).
        let buoyancy: Vec<f32> = (0..n).map(|k| if k < n / 2 { 0.1 } else { 0.9 }).collect();
        let mut cells: Vec<HexState> = dirs
            .iter()
            .map(|_| HexState::new(Composition::from_iter([(8u8, 1.0)])))
            .collect();

        run_orogeny(
            &mut cells, &ctx, &plate_of, &flow, &buoyancy, 0.6, 0.4, 0.15, 30,
        );

        let ocean_min = cells[..n / 2]
            .iter()
            .map(|s| s.elevation)
            .fold(f32::MAX, f32::min);
        let cont_max = cells[n / 2..]
            .iter()
            .map(|s| s.elevation)
            .fold(f32::MIN, f32::max);
        assert!(
            ocean_min < 0.0,
            "no trench opened on the subducting oceanic plate ({ocean_min})"
        );
        assert!(
            cont_max > 0.0,
            "no arc rose on the overriding continental plate ({cont_max})"
        );
    }
}
