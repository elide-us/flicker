//! **Zonal advection** — a gas giant's *real* material transport (its `Stratification` stage).
//!
//! Differential rotation carries the equator faster than the poles; that shear transports each
//! cell's material downstream (eastward) into a neighbour, **conserving total mass** via
//! [`HexWorld::transfer`]. Run iterated on the MYR clock, the shear stretches the initial
//! composition into zonal bands and swirls — **the swirl emerges from the transport**, it is not
//! painted on (memory `gas-giant-is-liquefied-air`). Chromophores (S, C) ride the gas as passive
//! tracers, so the tan belts are wherever the flow carries them. This is pass-based and slow — a
//! handful of steps that bake into the final state — never a per-frame fluid PDE.

use glam::Vec3;

use super::StepCtx;
use crate::model::world::HexWorld;

/// Equatorial rotation rate of the transport profile — the overall speed. A tuning knob, not a
/// measured constant (the MYR step size folds in via [`ADVECT_RATE`]).
const OMEGA_EQ: f32 = 1.0;
/// Differential-rotation shear: the rate falls to `OMEGA_EQ·(1 − SHEAR)` at the poles, so equator
/// and poles turn at different speeds — the shear that stretches blobs into bands.
const SHEAR: f32 = 0.6;
/// Folds the MYR step size into a per-step cell fraction (transported = speed·dt·`ADVECT_RATE`).
const ADVECT_RATE: f32 = 0.25;
/// Courant cap: never carry more than this fraction of a cell downstream in one step (stability).
const MAX_FRAC: f32 = 0.5;
/// Below this tangential length a cell is on the spin axis — no well-defined zonal direction.
const POLE_EPS: f32 = 1e-4;

/// Advect `world` one step of `dt_myr` by zonal differential rotation, returning the next grid.
///
/// Each non-polar cell sends a Courant-limited fraction of **every** element to its eastward
/// neighbour (passive transport — composition rides along), the fraction scaling with the local
/// zonal speed `ω(lat)·cos(lat)` (fastest at the equator). Amounts are read from the *original*
/// grid so a step never cascades through itself, and a cell only loses mass via its own single
/// outflow — so `next[i] ≥ orig[i]` until that outflow and the [`transfer`](HexWorld::transfer) is
/// **exact** (no clamping loss). Hence `total()` is invariant (proven in the tests). Pure, so the
/// worker can run it off-thread on the last completed grid.
///
/// `ctx.dirs` and `ctx.neighbors` must be parallel to `world`'s cells.
pub fn advect_zonal(world: &HexWorld, dt_myr: f64, ctx: &StepCtx) -> HexWorld {
    debug_assert_eq!(
        ctx.dirs.len(),
        world.cell_count(),
        "dirs must match the grid"
    );
    debug_assert_eq!(
        ctx.neighbors.len(),
        world.cell_count(),
        "neighbors must match the grid"
    );

    let mut next = world.clone();
    let axis = Vec3::Y; // spin axis
    let dt = dt_myr as f32;

    for i in 0..world.cell_count() {
        let dir = ctx.dirs[i];
        // Eastward tangent at this cell (zero on the spin axis).
        let east = axis.cross(dir);
        let east_len = east.length();
        if east_len < POLE_EPS {
            continue;
        }
        let east = east / east_len;

        // Differential rotation: linear zonal speed = ω(lat)·cos(lat), fastest at the equator.
        let sinlat = dir.y.clamp(-1.0, 1.0);
        let coslat = (1.0 - sinlat * sinlat).max(0.0).sqrt();
        let omega = OMEGA_EQ * (1.0 - SHEAR * sinlat * sinlat);
        let speed = omega * coslat;

        // The downstream neighbour: the one whose offset best aligns with the eastward flow.
        let mut downstream = None;
        let mut best = 0.0;
        for &nj in &ctx.neighbors[i] {
            let off = (ctx.dirs[nj as usize] - dir).normalize_or_zero();
            let aligned = off.dot(east);
            if aligned > best {
                best = aligned;
                downstream = Some(nj as usize);
            }
        }
        let Some(j) = downstream else { continue };

        let frac = (speed * dt * ADVECT_RATE).clamp(0.0, MAX_FRAC);
        if frac <= 0.0 {
            continue;
        }
        // Move that fraction of every element (chromophores ride the gas). Amounts come from the
        // original grid; see the conservation argument above.
        for (el, amount) in world.cells()[i].iter() {
            next.transfer(i, j, el, amount * frac as f64);
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_worldstate::Composition;

    const O: flicker_materials::ElementId = 8;
    const FE: flicker_materials::ElementId = 26;

    /// A ring of `n` cells at latitude `asin(y)`, each neighbouring its two ring-adjacent cells.
    /// Cells are ordered so the eastward tangent (`Y × dir`) of cell `k` points at cell `k+1`.
    fn ring(y: f32, n: usize) -> (Vec<Vec3>, Vec<Vec<u32>>) {
        let r = (1.0 - y * y).max(0.0).sqrt();
        let dirs = (0..n)
            .map(|k| {
                let a = -(k as f32) / n as f32 * std::f32::consts::TAU;
                Vec3::new(r * a.cos(), y, r * a.sin()).normalize()
            })
            .collect();
        let neighbors = (0..n)
            .map(|k| vec![((k + n - 1) % n) as u32, ((k + 1) % n) as u32])
            .collect();
        (dirs, neighbors)
    }

    #[test]
    fn advection_conserves_total_mass_over_many_steps() {
        let (dirs, neighbors) = ring(0.0, 6);
        let ctx = StepCtx {
            dirs: &dirs,
            neighbors: &neighbors,
        };
        let mut cells = vec![Composition::new(); 6];
        cells[0].add(O, 100.0);
        cells[2].add(FE, 40.0);
        let mut w = HexWorld::from_cells(1, cells);
        let before = w.total();
        for _ in 0..50 {
            w = advect_zonal(&w, 1.0, &ctx);
        }
        assert!(
            (w.total() - before).abs() < 1e-9,
            "advection conserves total mass: {} vs {before}",
            w.total()
        );
        // It actually moved: the material is no longer all in the seed cells.
        assert!(
            w.cell(0).unwrap().amount(O) < 100.0,
            "material left the source cell"
        );
    }

    #[test]
    fn material_moves_downstream_eastward_not_upstream() {
        let (dirs, neighbors) = ring(0.0, 4);
        let ctx = StepCtx {
            dirs: &dirs,
            neighbors: &neighbors,
        };
        let mut cells = vec![Composition::new(); 4];
        cells[0].add(O, 100.0);
        let next = advect_zonal(&HexWorld::from_cells(1, cells), 1.0, &ctx);
        assert!(
            next.cell(0).unwrap().amount(O) < 100.0,
            "source cell sheds material"
        );
        assert!(
            next.cell(1).unwrap().amount(O) > 0.0,
            "the eastward (downstream) neighbour gains it"
        );
        assert_eq!(
            next.cell(3).unwrap().amount(O),
            0.0,
            "the upstream neighbour gains nothing"
        );
    }

    #[test]
    fn equator_shears_faster_than_high_latitude() {
        let (de, ne) = ring(0.0, 6);
        let (dp, np) = ring(0.8, 6);
        let mut ce = vec![Composition::new(); 6];
        ce[0].add(O, 100.0);
        let mut cp = vec![Composition::new(); 6];
        cp[0].add(O, 100.0);
        let we = advect_zonal(
            &HexWorld::from_cells(1, ce),
            1.0,
            &StepCtx {
                dirs: &de,
                neighbors: &ne,
            },
        );
        let wp = advect_zonal(
            &HexWorld::from_cells(1, cp),
            1.0,
            &StepCtx {
                dirs: &dp,
                neighbors: &np,
            },
        );
        let moved_e = 100.0 - we.cell(0).unwrap().amount(O);
        let moved_p = 100.0 - wp.cell(0).unwrap().amount(O);
        assert!(
            moved_e > moved_p,
            "equator ({moved_e}) shears faster than high latitude ({moved_p})"
        );
        assert!(moved_p > 0.0, "high latitude still advects, just slower");
    }

    #[test]
    fn poles_do_not_advect() {
        // A cell on the spin axis has no zonal direction → it sheds nothing, however long the step.
        let dirs = vec![Vec3::Y, Vec3::new(0.0, 0.999, 0.0447).normalize()];
        let neighbors = vec![vec![1u32], vec![0u32]];
        let ctx = StepCtx {
            dirs: &dirs,
            neighbors: &neighbors,
        };
        let mut cells = vec![Composition::new(); 2];
        cells[0].add(O, 100.0);
        let next = advect_zonal(&HexWorld::from_cells(1, cells), 10.0, &ctx);
        assert_eq!(
            next.cell(0).unwrap().amount(O),
            100.0,
            "the polar cell sheds nothing"
        );
    }
}
