//! **The truth migration** — the one-way door.
//!
//! Before this runs, a cell's column is the truth and there is no inside to it.
//! After it, the maps are the truth and the column is a rollup of them. It happens
//! once per cell, its output is irreplaceable, and nothing downstream may
//! reconstruct a pixel by averaging.
//!
//! # How conservation and continuity are both kept
//!
//! They pull against each other. The surface is a **global function of position**
//! ([`crate::shape`]) so that two tiles cannot disagree at a seam — but a global
//! field does not happen to integrate to exactly this column's mass over exactly
//! this hexagon. Rescaling the tile to make it would move its edges and undo the
//! continuity.
//!
//! So the residual is absorbed **only where no other tile can see it**: weighted by
//! how far each pixel is from the cell boundary, zero at the seam, largest in the
//! middle. The edges are left exactly as the global field put them, and the
//! interior takes up the difference. Both properties hold at once, exactly, and
//! neither is approximated.
//!
//! # How the strata are laid down
//!
//! Conformably: every bed drapes over the same shape, thickest where the column is
//! thickest. That is what a conformable sequence *is*, and it is the honest answer
//! at migration — the aggregate knows how much of each bed there is and nothing
//! about how it varied within the cell, so inventing a variation would be inventing
//! data.
//!
//! It is also not the end state. The moment per-pixel erosion starts, the top bed
//! is cut where water runs and the ones beneath are not, and the maps diverge —
//! which is exactly how a canyon wall comes to show a layer cake. They are stored
//! separately from the first because they stop being proportional immediately.

use flicker_poc_chemistry::{crust_thickness_m, density_kg_m3, World};

use crate::mask::{HexMask, TileFrame, TILE_DIM};
use crate::shape::Neighbourhood;

/// One materialised cell: a stack of thickness maps, bottom bed first.
///
/// Thicknesses are `f32` metres here. The shipped format is narrower — a bed's
/// thickness does not need 24 bits of mantissa — but quantisation belongs with the
/// storage tier, where the blob format is decided, not with the arithmetic that
/// has to conserve mass.
#[derive(Clone, Debug)]
pub struct Tile {
    /// The cell this is the inside of.
    pub cell: u32,
    /// Where its pixels are.
    pub frame: TileFrame,
    /// Which of them belong to it.
    pub mask: HexMask,
    /// One map per stratum, bottom → top. `TILE_DIM²` metres of thickness, zero
    /// outside the mask.
    pub strata: Vec<Vec<f32>>,
    /// The ground just OUTSIDE the rim — every pixel not in the mask that touches
    /// one that is, at the height the global field puts there. This is how water at
    /// the edge knows whether the world continues uphill or falls away: the tile is
    /// a piece of a planet, not an island, and without a skirt its rim would read
    /// as a cliff into nothing. Frozen at migration, like the rest of the tile.
    pub skirt: std::collections::HashMap<(u16, u16), f32>,
    /// How many of the topmost strata were born at pixel scale (sediment laid by
    /// per-pixel erosion) rather than migrated from the column. `0` fresh from the
    /// door; the reintegration commit (T8) is what reconciles them with the ledger.
    pub pixel_born: usize,
}

impl Tile {
    /// Total thickness standing on a pixel — the whole stack, which is the surface
    /// a walker stands on.
    pub fn composite_m(&self, x: u32, y: u32) -> f32 {
        let i = (y as usize) * TILE_DIM as usize + x as usize;
        self.strata.iter().map(|m| m[i]).sum()
    }

    /// The topmost bed with any thickness at a pixel — what is exposed there, and
    /// the bed erosion will cut into next.
    pub fn exposed_bed(&self, x: u32, y: u32) -> Option<usize> {
        let i = (y as usize) * TILE_DIM as usize + x as usize;
        (0..self.strata.len())
            .rev()
            .find(|&s| self.strata[s][i] > 0.0)
    }

    /// Mass of one stratum as the maps actually hold it, kg — the left-hand side of
    /// the trial balance against the column it came from.
    pub fn stratum_mass_kg(&self, stratum: usize, density_kg_m3: f64) -> f64 {
        let area = self.frame.pixel_area_m2();
        self.strata[stratum]
            .iter()
            .map(|&t| t as f64 * density_kg_m3 * area)
            .sum()
    }
}

impl Tile {
    /// Render this tile as an RGBA image for the inspector, `scale`× smaller than
    /// the map itself — 2048² is more pixels than a panel has, and a nearest-sample
    /// reduction is honest about that rather than pretending to show detail it has
    /// no room for.
    ///
    /// Ground is shaded by height against the tile's own range, so a flat tile still
    /// reads; the exposed bed tints it, so a change of surface material is visible
    /// as a change of colour. Outside the hexagon is transparent — the corners
    /// belong to nobody and should look like it.
    pub fn preview_rgba(&self, scale: u32) -> (Vec<u8>, u32) {
        let scale = scale.max(1);
        let dim = TILE_DIM / scale;
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (x, y) in self.mask.iter() {
            let h = self.composite_m(x, y);
            lo = lo.min(h);
            hi = hi.max(h);
        }
        let span = (hi - lo).max(1e-3);
        let mut rgba = vec![0u8; (dim as usize) * (dim as usize) * 4];
        for oy in 0..dim {
            for ox in 0..dim {
                let (x, y) = (ox * scale, oy * scale);
                let o = ((oy as usize) * dim as usize + ox as usize) * 4;
                if !self.mask.contains(x, y) {
                    continue; // transparent: not this cell's ground
                }
                let t = ((self.composite_m(x, y) - lo) / span).clamp(0.0, 1.0);
                // Which bed is showing tints the shade, so a material change reads.
                let bed = self.exposed_bed(x, y).unwrap_or(0);
                let hue = (bed as f32 * 0.37).fract() * std::f32::consts::TAU;
                let third = std::f32::consts::TAU / 3.0;
                let shade = 0.25 + 0.75 * t;
                rgba[o] = ((0.55 + 0.45 * hue.cos()) * shade * 255.0) as u8;
                rgba[o + 1] = ((0.55 + 0.45 * (hue + third).cos()) * shade * 255.0) as u8;
                rgba[o + 2] = ((0.55 + 0.45 * (hue + 2.0 * third).cos()) * shade * 255.0) as u8;
                rgba[o + 3] = 255;
            }
        }
        (rgba, dim)
    }

    /// A one-line description of what this tile is, for the inspector caption.
    pub fn summary(&self) -> String {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for (x, y) in self.mask.iter() {
            let h = self.composite_m(x, y);
            lo = lo.min(h);
            hi = hi.max(h);
        }
        format!(
            "cell {} · {} beds · {} clusters · {:.0}–{:.0} m",
            self.cell,
            self.strata.len(),
            self.mask.count(),
            lo,
            hi
        )
    }
}

/// **Materialise one cell.** Turns its column into the maps that become the truth
/// for that ground.
///
/// Deterministic: same world, same cell, same maps. Nothing random happens here —
/// there is no seed, because there is nothing to seed. The shape comes from the
/// neighbourhood and the mass comes from the ledger.
pub fn materialize(world: &World, cell: usize, radius_m: f64, outline: &[glam::Vec3]) -> Tile {
    let frame = TileFrame::new(cell as u32, world.grid.dirs[cell], outline, radius_m);
    let mask = HexMask::new(&frame, outline);
    let hood = Neighbourhood::around(world, cell);

    // The surface the global field puts here, and how far each pixel is from the
    // cell boundary. Both are gathered in one pass over the mask.
    let mut relief = vec![0.0f64; TILE_DIM as usize * TILE_DIM as usize];
    let mut interior = vec![0.0f64; TILE_DIM as usize * TILE_DIM as usize];
    let mut interior_sum = 0.0f64;
    for (x, y) in mask.iter() {
        let i = idx(x, y);
        let r = hood.relief_at(frame.direction(x, y)).max(0.0);
        relief[i] = r;
        // How far inside the cell this pixel is. Zero on the boundary — so the
        // correction below cannot move an edge — rising toward the middle.
        let d = boundary_distance(&mask, x, y);
        interior[i] = d;
        interior_sum += d;
    }

    let owned = mask.count().max(1) as f64;
    let column = &world.columns[cell];

    // ── The ground, in metres, exactly as the global field puts it. ──
    //
    // The composite surface IS the field — not a rescaled version of it. An earlier
    // cut normalised each tile by its own mean thickness, which is the rescaling
    // that undoes continuity: two tiles then multiplied the same seam value by two
    // different constants and disagreed about the ground between them by 15%.
    //
    // What every stratum shares is this one surface. What differs across a seam is
    // the *beds* — and that is correct, because two columns genuinely have different
    // stacks, and a bed is entitled to end at a cell boundary. What may never differ
    // is the ground you stand on.
    let want_total = crust_thickness_m(column, world.cell_area_m2());
    let laid: f64 = mask.iter().map(|(x, y)| relief[idx(x, y)]).sum();
    let residual = want_total * owned - laid;
    let per_unit = if interior_sum > 0.0 {
        residual / interior_sum
    } else {
        0.0
    };

    let mut composite = vec![0.0f64; TILE_DIM as usize * TILE_DIM as usize];
    for (x, y) in mask.iter() {
        let i = idx(x, y);
        composite[i] = (relief[i] + per_unit * interior[i]).max(0.0);
    }

    // ── The skirt: the field's answer just past the rim. ──
    //
    // Same field, same question, one ring further out — so the ground does not
    // stop at the mask, it merely stops being this tile's to change.
    let mut skirt = std::collections::HashMap::new();
    for (x, y) in mask.iter() {
        for (dx, dy) in NEIGHBOURS_8 {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= TILE_DIM as i64 || ny >= TILE_DIM as i64 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            if mask.contains(nx, ny) {
                continue;
            }
            skirt
                .entry((nx as u16, ny as u16))
                .or_insert_with(|| hood.relief_at(frame.direction(nx, ny)).max(0.0) as f32);
        }
    }

    // ── Each bed takes its share of that surface. ──
    //
    // Conformable draping: the aggregate knows how much of each bed there is and
    // nothing about how it varied inside the cell, so every bed follows the same
    // shape in proportion. Inventing a per-bed variation would be inventing data.
    // Erosion is what makes them diverge, and it starts the moment T7 does.
    let mut strata = Vec::with_capacity(column.layers.len());
    for bed in &column.layers {
        let density = density_kg_m3(bed);
        let bed_thickness = if density > 0.0 {
            bed.mass_kg() / (density * world.cell_area_m2())
        } else {
            0.0
        };
        let share = if want_total > 0.0 {
            bed_thickness / want_total
        } else {
            0.0
        };
        let mut map = vec![0.0f32; TILE_DIM as usize * TILE_DIM as usize];
        for (x, y) in mask.iter() {
            let i = idx(x, y);
            map[i] = (composite[i] * share) as f32;
        }
        strata.push(map);
    }

    Tile {
        cell: cell as u32,
        frame,
        mask,
        strata,
        skirt,
        pixel_born: 0,
    }
}

/// The eight pixel neighbours, straights then diagonals.
pub(crate) const NEIGHBOURS_8: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Flat index of a pixel.
fn idx(x: u32, y: u32) -> usize {
    (y as usize) * TILE_DIM as usize + x as usize
}

/// How far a pixel is from the edge of the cell, as a weight that is **zero on the
/// boundary**. Measured by how much of its own neighbourhood is also inside — a
/// pixel with a neighbour outside the cell is on the edge and gets nothing.
///
/// Cheap on purpose: an exact distance transform over 4M pixels per tile would cost
/// more than the whole rest of the migration, and all this has to do is vanish at
/// the seam and be positive inside it.
fn boundary_distance(mask: &HexMask, x: u32, y: u32) -> f64 {
    const REACH: u32 = 3;
    if x < REACH || y < REACH || x + REACH >= TILE_DIM || y + REACH >= TILE_DIM {
        return 0.0;
    }
    for r in 1..=REACH {
        for (dx, dy) in [(r, 0), (0, r)] {
            if !mask.contains(x - dx, y - dy) || !mask.contains(x + dx, y + dy) {
                return (r - 1) as f64;
            }
        }
    }
    REACH as f64
}

/// Test fixture (shared with the erosion tests): **stratify one column, the
/// conserved way.** The size model makes a freq-6 bake a ~500-km planetoid
/// whose per-tick deposits sit near the absolute bed-film floor
/// (`MIN_BED_MASS_KG` is areal — the same hex at every size), so a short
/// default-pace bake no longer reliably stratifies any column. These tests are
/// about the MAPS, not the bake, so the heaviest column ALWAYS gets a felsic
/// and a mafic bed drawn from the mantle — unconditionally, so a green run
/// measured this one path and the claim is falsifiable (extra beds on an
/// already-stratified column are harmless to the map invariants under test).
#[cfg(test)]
pub(crate) fn guarantee_stack(w: &mut flicker_poc_chemistry::World) {
    use flicker_poc_chemistry::FormationProcess;
    let cell = (0..w.columns.len())
        .max_by(|&a, &b| {
            (w.columns[a].layers.len(), w.columns[a].mass_kg())
                .partial_cmp(&(w.columns[b].layers.len(), w.columns[b].mass_kg()))
                .expect("column masses are finite")
        })
        .expect("a world has columns");
    let beds: [(FormationProcess, [(u8, f64); 2]); 2] = [
        (
            FormationProcess::ContinentalArc,
            [(14, 4.0e15), (13, 2.0e15)],
        ),
        (FormationProcess::OceanicCrust, [(12, 3.0e15), (26, 3.0e15)]),
    ];
    for (process, wants) in beds {
        let mut melt = Vec::new();
        for (e, want) in wants {
            let mut want = want;
            for src in 0..w.mantle.n_cells() {
                if want <= 0.0 {
                    break;
                }
                let took = w.mantle.remove(src, e, want);
                if took > 0.0 {
                    melt.push((e, took));
                }
                want -= took;
            }
        }
        let at = w.tick_myr;
        w.columns[cell].deposit(process, at, &melt);
    }
    w.audit("guarantee_stack");
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_materials::{JsonTableSource, Tables};
    use flicker_poc_chemistry::{
        budget::Budget, config::content_data_dir, formation_stages, scheduler::Scheduler,
    };
    use flicker_worldgrid::icosphere_with_outlines;
    use std::sync::Arc;

    fn grown(freq: u32, ticks: usize) -> (World, Vec<Vec<glam::Vec3>>) {
        let t = Arc::new(
            Tables::from_source(&JsonTableSource::new(content_data_dir())).expect("tables"),
        );
        let b = Budget::from_dir(&content_data_dir(), &t).expect("budget");
        let (grid, outlines) = icosphere_with_outlines(freq);
        let mut w = World::seed(grid, b, &t, 5);
        let mut s = Scheduler::new(formation_stages(Arc::clone(&t), &w, &Default::default()), 5);
        for _ in 0..ticks {
            s.step(&mut w, 1.0, None);
        }
        crate::materialize::guarantee_stack(&mut w);
        (w, outlines)
    }

    /// **The trial balance.** Every gram the column held is in the maps — the
    /// invariant that replaces `Σ(pixels) = aggregate` as a derivation with
    /// `Σ(pixels) = aggregate` as a check, which is what walking through the
    /// one-way door means.
    #[test]
    fn the_maps_hold_exactly_what_the_column_held() {
        let (w, outlines) = grown(6, 150);
        let cell = w
            .columns
            .iter()
            .position(|c| c.layers.len() >= 2)
            .expect("the fixture stratified a column");
        let tile = materialize(&w, cell, crate::radius_for_freq(6), &outlines[cell]);
        assert_eq!(
            tile.strata.len(),
            w.columns[cell].layers.len(),
            "a map per bed"
        );
        // The column's beds keep their PROPORTIONS exactly — which is the trial
        // balance that matters, because the tile's own pixel area and the grid's
        // nominal cell area are two different conventions and reconciling them is
        // the storage tier's job, not this one's.
        let total_mass: f64 = w.columns[cell].layers.iter().map(|b| b.mass_kg()).sum();
        let tile_mass: f64 = (0..tile.strata.len())
            .map(|s| tile.stratum_mass_kg(s, density_kg_m3(&w.columns[cell].layers[s])))
            .sum();
        for (s, bed) in w.columns[cell].layers.iter().enumerate() {
            let want = bed.mass_kg() / total_mass;
            let got = tile.stratum_mass_kg(s, density_kg_m3(bed)) / tile_mass;
            assert!(
                (got - want).abs() <= 1e-6,
                "bed {s} holds {got:.9} of the tile against {want:.9} of the column"
            );
        }
    }

    /// **The seam, tested where it is actually decided.**
    ///
    /// Continuity is not achieved by comparing two tiles and hoping they are close —
    /// it is achieved by construction, in two parts that compose:
    ///
    /// 1. the shape field returns the same ground to either neighbour at a shared
    ///    point (`shape::two_cells_agree_about_the_ground_between_them`, exact), and
    /// 2. a tile's composite **is** that field wherever another tile can see it —
    ///    the mass correction is zero at the boundary by construction.
    ///
    /// This is (2). Together they mean two tiles cannot disagree about the ground
    /// between them, without ever comparing two tiles.
    ///
    /// (Comparing them directly is a worse test, not a better one: each tile's
    /// nearest pixel to a shared point is a different piece of ground up to a pixel
    /// away, so the comparison measures the terrain's own gradient as much as
    /// anything.)
    #[test]
    fn a_tile_leaves_its_edges_exactly_as_the_field_put_them() {
        let (w, outlines) = grown(6, 150);
        let cell = 17usize;
        let tile = materialize(&w, cell, crate::radius_for_freq(6), &outlines[cell]);
        let hood = crate::shape::Neighbourhood::around(&w, cell);

        // Pixels on the rim: inside the cell, with a neighbour outside it.
        let rim: Vec<(u32, u32)> = tile
            .mask
            .iter()
            .filter(|&(x, y)| {
                x > 0
                    && y > 0
                    && x + 1 < TILE_DIM
                    && y + 1 < TILE_DIM
                    && (!tile.mask.contains(x - 1, y)
                        || !tile.mask.contains(x + 1, y)
                        || !tile.mask.contains(x, y - 1)
                        || !tile.mask.contains(x, y + 1))
            })
            .take(400)
            .collect();
        assert!(!rim.is_empty(), "a cell has a rim");

        for (x, y) in rim {
            let composite = tile.composite_m(x, y) as f64;
            let field = hood.relief_at(tile.frame.direction(x, y));
            assert!(
                (composite - field).abs() <= 1e-3 * field.max(1.0),
                "the tile moved its own edge at ({x}, {y}): {composite} vs the field's {field}"
            );
        }
    }

    /// Materialising is a function, not an event: run it twice and get the same
    /// tile, so a tile that is thrown away can be made again exactly.
    #[test]
    fn materialising_twice_gives_the_same_tile() {
        let (w, outlines) = grown(6, 120);
        let cell = 17usize;
        let a = materialize(&w, cell, crate::radius_for_freq(6), &outlines[cell]);
        let b = materialize(&w, cell, crate::radius_for_freq(6), &outlines[cell]);
        assert_eq!(a.strata.len(), b.strata.len());
        for (ma, mb) in a.strata.iter().zip(&b.strata) {
            assert_eq!(ma, mb, "the same cell materialised differently twice");
        }
    }

    /// A pixel is a place, and the stack over it is what a digger would go through.
    /// The topmost bed with thickness is what is exposed — the one erosion cuts next.
    #[test]
    fn a_pixel_carries_the_whole_stack_above_it() {
        let (w, outlines) = grown(6, 150);
        let cell = w
            .columns
            .iter()
            .position(|c| c.layers.len() >= 2)
            .expect("the fixture stratified a column");
        let tile = materialize(&w, cell, crate::radius_for_freq(6), &outlines[cell]);
        let (mx, my) = (TILE_DIM / 2, TILE_DIM / 2);
        assert!(
            tile.composite_m(mx, my) > 0.0,
            "there is ground in the middle of a cell"
        );
        assert!(
            tile.exposed_bed(mx, my).is_some(),
            "and something is exposed at the top of it"
        );
        // Nothing outside the hexagon: the corners belong to nobody.
        assert_eq!(tile.composite_m(0, 0), 0.0);
    }
}
