//! **The tile source** — where a planet's aggregate truth comes from.
//!
//! The migration machinery (shape + materialize) asks a planet exactly two
//! questions per cell: *how thick is the whole solid stack here* (the relief
//! field), and *what beds is it made of, bottom to top* (the conformable
//! drape). [`TileSource`] is those two questions as a trait, so the same
//! machinery serves BOTH planets we have:
//!
//! - the retired chemistry sim's [`flicker_poc_chemistry::World`] (the T6
//!   lineage — its tests keep guarding the mechanics), and
//! - **the Populous bench's committed planet** ([`PlanetSource`] over a
//!   [`flicker_worldengine::PlanetEpoch`]) — the live generation-mode path
//!   (two-modes spec, 2026-08-28).
//!
//! One trait, one mechanism, no parallel implementation.

use flicker_poc_chemistry::{crust_thickness_m, density_kg_m3, World};
use flicker_worldengine::PlanetEpoch;
use flicker_worldgrid::{icosphere_with_outlines, Sphere};

/// One bed of a cell's stack: a material code and its thickness.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Bed {
    /// Material code — meaningful to the source's own registry; the baked
    /// index documents the mapping. For the chemistry world it is the
    /// stratum index; for a planet epoch it is a [`planet material
    /// code`](PlanetSource) below.
    pub material: u8,
    /// Thickness, metres.
    pub thickness_m: f64,
}

/// The two questions the migration asks of a planet.
pub trait TileSource {
    /// The hex grid the planet stands on.
    fn grid(&self) -> &Sphere;
    /// Total solid-stack thickness at `cell`, metres — the relief field.
    fn thickness_m(&self, cell: usize) -> f64;
    /// The stack at `cell`, bottom → top. Zero-thickness beds are omitted.
    fn beds_m(&self, cell: usize) -> Vec<Bed>;
}

impl TileSource for World {
    fn grid(&self) -> &Sphere {
        &self.grid
    }

    fn thickness_m(&self, cell: usize) -> f64 {
        crust_thickness_m(&self.columns[cell], self.cell_area_m2())
    }

    fn beds_m(&self, cell: usize) -> Vec<Bed> {
        let area = self.cell_area_m2();
        self.columns[cell]
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, bed)| {
                let density = density_kg_m3(bed);
                (density > 0.0).then(|| Bed {
                    material: i as u8,
                    thickness_m: bed.mass_kg() / (density * area),
                })
            })
            .filter(|b| b.thickness_m > 0.0)
            .collect()
    }
}

/// Planet-epoch material codes — the baked map's `material` byte. Stable,
/// documented in the bake index; a vein-bearing stratum carries
/// `VEIN_BASE + (vein - 1)` so the ore identity survives into the texture.
pub const MAT_BASE: u8 = 1;
pub const MAT_STRATUM: u8 = 2;
pub const MAT_VOLCANIC: u8 = 3;
pub const MAT_ROCK: u8 = 4;
pub const MAT_SEDIMENT: u8 = 5;
/// Vein-bearing stratum codes start here: `VEIN_BASE + vein_kind_index`.
pub const MAT_VEIN_BASE: u8 = 16;

/// **A committed Populous planet as a tile source.**
///
/// Wraps a validated [`PlanetEpoch`]: the grid is rebuilt from the recipe
/// (`icosphere(freq)` — the same deterministic roll the bench stood on), and
/// the ledger's per-hex solid stack becomes the beds. Ledger heights ride the
/// bench's tile-width units; `relief_scale_m` converts one unit to metres —
/// a bake LEVER (the bench's unit is its own visual convention, not a
/// physical claim), recorded in the bake index so the choice is data.
pub struct PlanetSource {
    epoch: PlanetEpoch,
    grid: Sphere,
    /// Each cell's hex outline on the unit sphere — the tile mask's geometry.
    outlines: Vec<Vec<glam::Vec3>>,
    /// Metres one ledger height-unit spans.
    relief_scale_m: f64,
    /// The sea's level in the same metres, solved once from the conserved
    /// water volume over the planet's own hypsometry (the bench's
    /// `resolve_sea`, replayed at bake scale).
    sea_level_m: f64,
}

impl PlanetSource {
    /// Stand the planet up from its epoch. `relief_scale_m` is the metres
    /// per ledger unit lever.
    pub fn new(epoch: PlanetEpoch, relief_scale_m: f64) -> Self {
        let (grid, outlines) = icosphere_with_outlines(epoch.recipe.freq);
        let mut src = Self {
            epoch,
            grid,
            outlines,
            relief_scale_m,
            sea_level_m: 0.0,
        };
        src.sea_level_m = src.solve_sea_level_m();
        src
    }

    /// The epoch this planet stands on.
    pub fn epoch(&self) -> &PlanetEpoch {
        &self.epoch
    }

    /// A cell's hex outline (unit-sphere corner loop).
    pub fn outline(&self, cell: usize) -> &[glam::Vec3] {
        &self.outlines[cell]
    }

    /// The planet's radius in metres — the canon size model at the recipe's
    /// frequency.
    pub fn radius_m(&self) -> f64 {
        clayengine::diameter_mi(self.epoch.recipe.freq) * clayengine::METERS_PER_MILE / 2.0
    }

    /// The solved sea level, metres above the datum the ledger heights ride.
    pub fn sea_level_m(&self) -> f64 {
        self.sea_level_m
    }

    /// The ledger's solid ground at a cell, in LEDGER units (pre-scale).
    fn ground_units(&self, cell: usize) -> f64 {
        let l = &self.epoch.ledger;
        f64::from(l.base[cell])
            + f64::from(l.l3_h[cell])
            + f64::from(l.l4_h[cell])
            + f64::from(l.rock[cell])
            + f64::from(l.sediment[cell])
    }

    /// The bench's sea solve, replayed over the file: find the level at
    /// which the standing water (conserved volume minus the share locked in
    /// ice, area-weighted exactly as the ledger weights it) fits the
    /// planet's hypsometry. Bisection — the fill volume is monotone in the
    /// level.
    fn solve_sea_level_m(&self) -> f64 {
        let n = self.grid.len();
        // Area weights relative to the mean cell — the ledger's own
        // convention for its conserved volume.
        let mean = self.grid.area.iter().map(|&a| f64::from(a)).sum::<f64>() / n as f64;
        let rel: Vec<f64> = self
            .grid
            .area
            .iter()
            .map(|&a| f64::from(a) / mean)
            .collect();
        let free_units = f64::from(self.epoch.era.water_volume) - f64::from(self.epoch.era.ice_locked);
        if free_units <= 0.0 {
            return 0.0;
        }
        let ground: Vec<f64> = (0..n).map(|i| self.ground_units(i)).collect();
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for &g in &ground {
            hi = hi.max(g);
        }
        hi += free_units; // a level above every column floods everything
        for _ in 0..64 {
            let mid = 0.5 * (lo + hi);
            let held: f64 = (0..n)
                .map(|i| rel[i] * (mid - ground[i]).max(0.0))
                .sum();
            if held < free_units {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi) * self.relief_scale_m
    }
}

impl TileSource for PlanetSource {
    fn grid(&self) -> &Sphere {
        &self.grid
    }

    fn thickness_m(&self, cell: usize) -> f64 {
        self.ground_units(cell) * self.relief_scale_m
    }

    fn beds_m(&self, cell: usize) -> Vec<Bed> {
        let l = &self.epoch.ledger;
        let s = self.relief_scale_m;
        // The stratum bed carries the ore identity where a vein stands.
        let stratum_mat = match l.vein[cell] {
            0 => MAT_STRATUM,
            v => MAT_VEIN_BASE.saturating_add(v - 1),
        };
        [
            (MAT_BASE, f64::from(l.base[cell])),
            (stratum_mat, f64::from(l.l3_h[cell])),
            (MAT_VOLCANIC, f64::from(l.l4_h[cell])),
            (MAT_ROCK, f64::from(l.rock[cell])),
            (MAT_SEDIMENT, f64::from(l.sediment[cell])),
        ]
        .into_iter()
        .filter(|&(_, t)| t > 0.0)
        .map(|(material, t)| Bed {
            material,
            thickness_m: t * s,
        })
        .collect()
    }
}
