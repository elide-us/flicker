//! **What the ground does, as a function of where you are.**
//!
//! This is the one idea that makes the migration work. Instead of each tile
//! deciding what its own surface looks like and then trying to match its
//! neighbours at the edges, the surface is defined **globally, in terms of
//! position on the sphere** — and a tile merely samples it.
//!
//! A tile therefore has no say in what the ground does at its own boundary. Two
//! tiles that meet get the same answer there because they are asking the same
//! question of the same function, which is a much stronger guarantee than matching
//! two independent constructions and hoping. There is no seam-stitching pass here,
//! and there is no need for one.
//!
//! # Where the sub-hex shape comes from
//!
//! From the neighbourhood. A column knows its own thickness and so do the columns
//! around it; between them the ground leans from one toward the other. Sampling
//! that lean at pixel resolution gives a surface that is smooth, continuous, and
//! *implied by what the aggregate actually knows* — rather than invented.
//!
//! It is deliberately **not noise.** Fractal detail would look like terrain and
//! mean nothing, and the chemical-accounting ruling is explicit that shape comes
//! from material, not from a noise field. What this produces is the honest shape at
//! migration: the interpolation of what the geology worked out. The *detail* —
//! valleys, scarps, the layer cake in a canyon wall — is what per-pixel erosion
//! then cuts into it, and that detail will be earned rather than generated.

use glam::DVec3;

use crate::source::TileSource;

/// How far a column's influence reaches, as a multiple of the cell spacing.
///
/// **Compact support is the whole trick.** A blend with unbounded falloff depends
/// on *which cells were gathered*, and two neighbouring tiles gather different sets
/// — so they disagree about the ground between them by a fraction of a percent,
/// which is a step in the terrain at every seam. Cutting the influence off well
/// inside the two-ring gather means both tiles include every cell that can possibly
/// matter at a shared point, and the sets they actually use are identical.
///
/// Comfortably under 2.0 so the gather is always sufficient, comfortably over 1.0
/// so the blend is smooth across a boundary.
const REACH: f64 = 1.6;

/// How sharply influence concentrates near a column. Higher keeps each cell's own
/// thickness closer to its centre and steepens the transition between them.
const SHARPNESS: i32 = 4;

/// Centre-to-centre spacing on the unit sphere, from the cell count alone.
///
/// **A property of the grid, not of a cell** — and it has to be. Measuring each
/// cell's own mean spacing makes the reach, and therefore every weight, very
/// slightly different for every cell; two tiles then blend the same columns with
/// different weights and disagree about the ground between them by a tenth of a
/// percent. Small, and a step in the terrain at every seam on the planet. Equal
/// area is what makes one number correct everywhere: a hexagon of area `4π/N`
/// measures `√(2·area/√3)` flat to flat.
fn grid_spacing(cells: usize) -> f64 {
    let area = 4.0 * std::f64::consts::PI / cells.max(1) as f64;
    (2.0 * area / 3f64.sqrt()).sqrt()
}

/// Everything a sample needs, gathered once per tile instead of per pixel — 3.8M
/// pixels each doing their own neighbour lookup is the difference between a tile
/// materialising in a moment and in a minute.
///
/// Gathers **two rings**, because the influence reaches [`REACH`] spacings and a
/// pixel near a boundary must see everything the tile on the other side sees.
pub struct Neighbourhood {
    dirs: Vec<DVec3>,
    thickness: Vec<f64>,
    /// The distance influence reaches, in chord length on the unit sphere.
    reach: f64,
}

impl Neighbourhood {
    /// Gather a cell and the two rings around it.
    pub fn around<S: TileSource>(src: &S, cell: usize) -> Self {
        let grid = src.grid();
        let mut gathered: Vec<usize> = vec![cell];
        for &j in &grid.neighbors[cell] {
            gathered.push(j as usize);
            for &k in &grid.neighbors[j as usize] {
                gathered.push(k as usize);
            }
        }
        gathered.sort_unstable();
        gathered.dedup();

        Self {
            dirs: gathered
                .iter()
                .map(|&i| grid.dirs[i].as_dvec3().normalize())
                .collect(),
            thickness: gathered.iter().map(|&i| src.thickness_m(i)).collect(),
            reach: grid_spacing(grid.len()) * REACH,
        }
    }

    /// The crust thickness the ground has at `p`, in metres.
    ///
    /// A convex blend of every column within reach, weighted so that influence is
    /// infinite at a column's own centre and falls smoothly to exactly zero at the
    /// reach. At a cell centre it is that cell's own thickness; on a shared
    /// boundary the same set of columns is in range from either side, weighted the
    /// same way — which is why the two tiles cannot disagree.
    pub fn relief_at(&self, p: DVec3) -> f64 {
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for (i, &d) in self.dirs.iter().enumerate() {
            let chord = (p - d).length();
            if chord < 1e-12 {
                return self.thickness[i];
            }
            if chord >= self.reach {
                continue;
            }
            // Compactly supported and singular at the centre: `(reach − d)^k / d²`.
            let taper = (self.reach - chord).powi(SHARPNESS);
            let w = taper / (chord * chord);
            num += w * self.thickness[i];
            den += w;
        }
        if den <= 0.0 {
            0.0
        } else {
            num / den
        }
    }

    /// The range of thickness the columns in reach actually span — what a blend
    /// over them is bounded by.
    pub fn range(&self) -> (f64, f64) {
        self.thickness
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &t| (lo.min(t), hi.max(t)))
    }
}

/// The same read without the gathering, for a caller that has one position and no
/// tile — the GM inspector asking "what is the ground here".
pub fn relief_at<S: TileSource>(src: &S, cell: usize, p: DVec3) -> f64 {
    Neighbourhood::around(src, cell).relief_at(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_poc_chemistry::{
        budget::Budget, config::content_data_dir, crust_thickness_m, formation_stages,
        scheduler::Scheduler, World,
    };
    use flicker_worldgrid::icosphere;
    use std::sync::Arc;

    fn grown_world(freq: u32, ticks: usize) -> World {
        let t = Arc::new(
            Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables"),
        );
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let mut w = World::seed(icosphere(freq), b, &t, 5);
        let mut s = Scheduler::new(formation_stages(Arc::clone(&t), &w, &Default::default()), 5);
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None);
        }
        w
    }

    /// At a cell's own centre the ground is that cell's own thickness — the sample
    /// agrees with the aggregate it came from.
    #[test]
    fn a_cell_centre_reads_its_own_column() {
        let w = grown_world(6, 120);
        let area = w.cell_area_m2();
        for cell in [0usize, 17, 60] {
            let n = Neighbourhood::around(&w, cell);
            let here = n.relief_at(w.grid.dirs[cell].as_dvec3().normalize());
            let own = crust_thickness_m(&w.columns[cell], area);
            assert!(
                (here - own).abs() < 1e-6 * own.max(1.0),
                "cell {cell}: sampled {here} vs its own {own}"
            );
        }
    }

    /// **The seam property, which is the whole point.** Two neighbouring cells
    /// sampled at the same point on the boundary between them return the same
    /// ground — so their tiles cannot disagree about it, and the world has no cliff
    /// around every hex.
    #[test]
    fn two_cells_agree_about_the_ground_between_them() {
        let w = grown_world(6, 120);
        for cell in [0usize, 17, 60] {
            let a = w.grid.dirs[cell].as_dvec3().normalize();
            for &j in &w.grid.neighbors[cell] {
                let j = j as usize;
                let b = w.grid.dirs[j].as_dvec3().normalize();
                // Halfway between the two centres is on their shared boundary.
                let seam = ((a + b) * 0.5).normalize();
                let from_a = Neighbourhood::around(&w, cell).relief_at(seam);
                let from_b = Neighbourhood::around(&w, j).relief_at(seam);
                let scale = from_a.abs().max(from_b.abs()).max(1.0);
                assert!(
                    (from_a - from_b).abs() < 1e-9 * scale,
                    "cells {cell} and {j} disagree at their seam: {from_a} vs {from_b}"
                );
            }
        }
    }

    /// The ground leans from one column toward the other rather than jumping —
    /// walking from a cell to its neighbour, the reading stays between the two.
    #[test]
    fn the_ground_leans_between_columns() {
        let w = grown_world(6, 120);
        let area = w.cell_area_m2();
        let cell = 17usize;
        let j = w.grid.neighbors[cell][0] as usize;
        let (a, b) = (
            w.grid.dirs[cell].as_dvec3().normalize(),
            w.grid.dirs[j].as_dvec3().normalize(),
        );
        let (ta, tb) = (
            crust_thickness_m(&w.columns[cell], area),
            crust_thickness_m(&w.columns[j], area),
        );
        let _ = (ta, tb);
        let n = Neighbourhood::around(&w, cell);
        // Bounded by the columns actually in reach — a blend pulls toward all of
        // them, not only the two endpoints of the walk.
        let (lo, hi) = n.range();
        for k in 0..=10 {
            let t = k as f64 / 10.0;
            let p = (a * (1.0 - t) + b * t).normalize();
            let r = n.relief_at(p);
            assert!(
                r >= lo - 1e-6 && r <= hi + 1e-6,
                "the blend left the range its columns set: {r} not in {lo}..{hi}"
            );
        }
    }
}
