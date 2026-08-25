//! **The molten layer's first fact: where the heat comes up.**
//!
//! The mantle under the crust is not uniformly hot. It convects in a handful of
//! huge, slow cells; heat wells up along the boundaries where cells meet and
//! sinks in their interiors. Seen from above that is a BUBBLE MAP: large cool
//! bubbles (the cell interiors) rimmed by hot seams (the boundaries), hottest
//! where three cells meet — the points a deep-crust layer will later focus into
//! volcanoes.
//!
//! This module is that field, and nothing else: N random convection-cell seeds
//! on the sphere, a per-tile HEAT in `0..1` derived from how close a tile
//! stands to a boundary between cells, and a handful of HOT SPOTS — mantle
//! plumes that burn through wherever they are, seam or no seam (the Hawaiis
//! to the seams' ridges). It is DATA — the seams tab paints it
//! through the shared heat ramp ([`flicker_globe::temp_color`]) and the hex
//! stack reads a column's own value from it; neither meaning lives here.
//!
//! **Transformation, not outcome (rule 935269B7):** nothing here places a seam.
//! The seeds are random, the metric is geometry, and the seams are wherever the
//! seeds' boundaries fall. The editorial controls are counts and the re-roll —
//! how many cells, how many plumes, and which world — never a position.

use flicker::render::Vec3;

use crate::map::HexMap;

/// The fewest convection cells the dial offers — two hemispheres of cool with
/// one great seam between them.
pub const MIN_CELLS: u32 = 2;
/// The most — a busy mantle, seams everywhere.
pub const MAX_CELLS: u32 = 12;
/// Where the bench opens: enough cells that both the bubbles and the triple
/// points read at a glance.
pub const DEFAULT_CELLS: u32 = 6;

/// The fewest hot spots the dial offers — none: a pure seam field.
pub const MIN_SPOTS: u32 = 0;
/// The most — a plume-riddled mantle.
pub const MAX_SPOTS: u32 = 12;
/// Where the bench opens: a few plumes, so the map reads as seams AND spots.
pub const DEFAULT_SPOTS: u32 = 4;

/// A hot spot's angular radius. FIXED, not scaled by the cell count: a plume
/// is its own thing — it does not grow because the convection pattern
/// coarsened. About a dozen tiles across at the standard map size.
const SPOT_RADIUS: f32 = 0.07;
/// A spot's centre heat — white-hot on the shared ramp, hot enough that its
/// core clears the crust's breakthrough floor and vents.
const SPOT_PEAK: f32 = 0.92;
/// The spot stream's offset off the field's one roll, so the spots and the
/// cell seeds are INDEPENDENT draws of the same world: re-count the cells and
/// the spots stand still, and vice versa.
const SPOT_STREAM: u64 = 0x5851_F42D_4C95_7F2D;

/// How far from a boundary the heat glow reaches, as a fraction of a cell's own
/// characteristic angular radius (`√(4π/cells)/2`). Scale-free on purpose: two
/// huge cells get a broad seam, twelve small ones get tight seams, and the
/// bubbles stay bubbles at every count.
const SEAM_BAND: f32 = 0.45;

/// How the two boundary reads mix into one heat value: the seam line itself
/// carries this share, and the triple-junction read carries the rest — so an
/// ordinary seam tops out ORANGE on the shared ramp while the meeting points
/// push toward white-hot: the volcanic points of the bubble map.
const SEAM_WEIGHT: f32 = 0.62;

/// **The molten heat field.** N convection-cell seeds and the per-tile heat
/// their boundaries induce, over one [`HexMap`] tiling.
pub struct SeamField {
    /// How many convection cells were asked for, clamped to the offered range.
    cells: u32,
    /// How many hot spots, clamped likewise.
    spots: u32,
    /// The roll that placed the seeds — kept so the same world can be rebuilt
    /// at a new map size without moving its seams.
    seed: u64,
    /// The cell seeds: unit directions on the sphere.
    seeds: Vec<Vec3>,
    /// The hot-spot centres: unit directions, an independent stream of the
    /// same roll.
    spot_dirs: Vec<Vec3>,
    /// Per-tile heat, `0..1` — cool bubble interiors at 0, seams hot, triple
    /// junctions hotter, spot cores hottest. Indexed by `TileId` like every
    /// per-tile layer.
    heat: Vec<f32>,
}

impl SeamField {
    /// Roll a field of `cells` seeds and `spots` plumes with `seed` and derive
    /// the heat for every tile of `map`.
    pub fn new(map: &HexMap, cells: u32, spots: u32, seed: u64) -> Self {
        let mut field = Self {
            cells: cells.clamp(MIN_CELLS, MAX_CELLS),
            spots: spots.clamp(MIN_SPOTS, MAX_SPOTS),
            seed,
            seeds: Vec::new(),
            spot_dirs: Vec::new(),
            heat: Vec::new(),
        };
        field.rebuild(map);
        field
    }

    /// How many convection cells the field was rolled with.
    pub fn cells(&self) -> u32 {
        self.cells
    }

    /// How many hot spots.
    pub fn spots(&self) -> u32 {
        self.spots
    }

    /// The hot-spot centres — for a view that marks them, and for tests.
    pub fn spot_dirs(&self) -> &[Vec3] {
        &self.spot_dirs
    }

    /// The roll that placed the seeds.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// A tile's heat, `0..1`. Out-of-range asks read as cool rather than
    /// panicking — a viewer's question, and a hole is cold.
    pub fn heat(&self, tile: u32) -> f32 {
        self.heat.get(tile as usize).copied().unwrap_or(0.0)
    }

    /// Every tile's heat, for a shell's colour closure.
    pub fn heats(&self) -> &[f32] {
        &self.heat
    }

    /// Re-roll the seeds (a new random world) over the same map.
    pub fn randomize(&mut self, map: &HexMap) {
        self.seed = fastrand::u64(..);
        self.rebuild(map);
    }

    /// Change the cell count, keeping the roll — the first `n` seeds of the
    /// same sequence, so dialing up grows the same world rather than replacing
    /// it. A no-op at the current count.
    pub fn set_cells(&mut self, map: &HexMap, cells: u32) {
        let cells = cells.clamp(MIN_CELLS, MAX_CELLS);
        if cells == self.cells {
            return;
        }
        self.cells = cells;
        self.rebuild(map);
    }

    /// Change the spot count, keeping the roll — the same prefix law as the
    /// cells, on the spots' own stream. A no-op at the current count.
    pub fn set_spots(&mut self, map: &HexMap, spots: u32) {
        let spots = spots.clamp(MIN_SPOTS, MAX_SPOTS);
        if spots == self.spots {
            return;
        }
        self.spots = spots;
        self.rebuild(map);
    }

    /// The map was rebuilt (a new size) — derive the heat for the new tiling
    /// from the SAME seeds: the world's seams do not move when its map does.
    pub fn rebuild(&mut self, map: &HexMap) {
        let mut rng = fastrand::Rng::with_seed(self.seed);
        self.seeds = (0..self.cells)
            .map(|_| {
                // Uniform on the sphere: z uniform in −1..1, longitude uniform.
                let z = rng.f32() * 2.0 - 1.0;
                let a = rng.f32() * std::f32::consts::TAU;
                let r = (1.0 - z * z).max(0.0).sqrt();
                Vec3::new(r * a.cos(), z, r * a.sin())
            })
            .collect();

        // The hot spots ride their OWN stream of the same roll: independent of
        // the cell draws, so either count can change without moving the other.
        let mut spot_rng = fastrand::Rng::with_seed(self.seed.wrapping_add(SPOT_STREAM));
        self.spot_dirs = (0..self.spots)
            .map(|_| {
                let z = spot_rng.f32() * 2.0 - 1.0;
                let a = spot_rng.f32() * std::f32::consts::TAU;
                let r = (1.0 - z * z).max(0.0).sqrt();
                Vec3::new(r * a.cos(), z, r * a.sin())
            })
            .collect();

        // A cell's characteristic angular radius: N equal caps tile 4π sr.
        let cell_radius =
            (4.0 * std::f32::consts::PI / self.cells as f32).sqrt() * 0.5;
        let band = SEAM_BAND * cell_radius;

        let dirs = &map.grid().dirs;
        self.heat = dirs
            .iter()
            .map(|d| {
                // Angular distance to the three nearest seeds. The boundary
                // metric is their DIFFERENCES: on a seam the two nearest seeds
                // are equally far (d2−d1 → 0); at a triple junction the third
                // is too (d3−d1 → 0).
                let (mut d1, mut d2, mut d3) = (f32::MAX, f32::MAX, f32::MAX);
                for s in &self.seeds {
                    let a = d.dot(*s).clamp(-1.0, 1.0).acos();
                    if a < d1 {
                        (d1, d2, d3) = (a, d1, d2);
                    } else if a < d2 {
                        (d2, d3) = (a, d2);
                    } else if a < d3 {
                        d3 = a;
                    }
                }
                let seam = 1.0 - ((d2 - d1) / band).clamp(0.0, 1.0);
                let junction = if d3 < f32::MAX {
                    1.0 - ((d3 - d1) / band).clamp(0.0, 1.0)
                } else {
                    0.0 // two cells have no triple junction
                };
                let boundary = SEAM_WEIGHT * seam + (1.0 - SEAM_WEIGHT) * junction;
                // A plume burns wherever it is: a white-hot gaussian core that
                // falls off over SPOT_RADIUS. The tile reads the HOTTEST source
                // over it — heat sources do not stack past the hottest one.
                let plume = self
                    .spot_dirs
                    .iter()
                    .map(|s| {
                        let a = d.dot(*s).clamp(-1.0, 1.0).acos() / SPOT_RADIUS;
                        SPOT_PEAK * (-a * a).exp()
                    })
                    .fold(0.0f32, f32::max);
                boundary.max(plume)
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MIN_FREQ;

    /// **The field is the shape it claims.** One heat per tile, all inside
    /// `0..1`, the asked cell count clamped into the offered dial range.
    #[test]
    fn the_field_covers_the_map_inside_the_offered_range() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, 7);
        assert_eq!(field.heats().len(), map.len());
        assert!(field.heats().iter().all(|h| (0.0..=1.0).contains(h)));
        assert_eq!(SeamField::new(&map, 0, 0, 7).cells(), MIN_CELLS);
        assert_eq!(SeamField::new(&map, 99, 99, 7).cells(), MAX_CELLS);
        assert_eq!(SeamField::new(&map, 99, 99, 7).spots(), MAX_SPOTS);
        // Out-of-range reads are cool, not a panic.
        assert_eq!(field.heat(u32::MAX), 0.0);
    }

    /// **Bubbles of cool with edges of hot.** A tile standing at a seed (deep
    /// inside its cell) is cold; the hottest tile on the map stands near a
    /// boundary — and the map has BOTH in quantity: this is a bubble map, not
    /// a wash.
    #[test]
    fn interiors_are_cool_and_seams_are_hot() {
        let map = HexMap::new(MIN_FREQ);
        let field = SeamField::new(&map, DEFAULT_CELLS, 0, 42);
        let cold = field.heats().iter().filter(|h| **h < 0.1).count();
        let hot = field.heats().iter().filter(|h| **h > 0.5).count();
        assert!(
            cold > map.len() / 4,
            "the bubbles' interiors are cool: {cold}/{}",
            map.len()
        );
        assert!(hot > 0, "and the seams between them are hot");
        // The seam metric peaks where two cells actually meet: the hottest
        // tile's two nearest seeds are near-equidistant.
        let (hottest, _) = field
            .heats()
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .expect("tiles exist");
        let d = map.direction(hottest as u32);
        let mut dists: Vec<f32> = field
            .seeds
            .iter()
            .map(|s| d.dot(*s).clamp(-1.0, 1.0).acos())
            .collect();
        dists.sort_by(f32::total_cmp);
        assert!(
            dists[1] - dists[0] < 0.05,
            "the hottest tile stands on a boundary: Δ={}",
            dists[1] - dists[0]
        );
    }

    /// **The roll is the identity.** The same seed rebuilds the same field at
    /// any map size; a re-roll moves the seams; a cell-count change at the same
    /// roll KEEPS the shared prefix of seeds (dialing up grows the world).
    #[test]
    fn the_seed_is_the_world_and_rerolls_move_it() {
        let map = HexMap::new(MIN_FREQ);
        let a = SeamField::new(&map, 5, 3, 1234);
        let b = SeamField::new(&map, 5, 3, 1234);
        assert_eq!(a.heats(), b.heats(), "same roll, same world");

        let mut c = SeamField::new(&map, 5, 3, 1234);
        c.randomize(&map);
        assert_ne!(c.seed(), 1234, "a re-roll takes a new seed");
        assert_ne!(a.heats(), c.heats(), "and the seams moved");

        let mut d = SeamField::new(&map, 5, 3, 1234);
        d.set_cells(&map, 7);
        assert_eq!(d.cells(), 7);
        for (i, s) in a.seeds.iter().enumerate() {
            assert_eq!(*s, d.seeds[i], "seed {i} survives the dial");
        }
        // The spots are an INDEPENDENT stream of the same roll: the cells dial
        // does not move them, and their own dial keeps the shared prefix.
        assert_eq!(a.spot_dirs(), d.spot_dirs(), "cells dial leaves the spots");
        d.set_spots(&map, 6);
        assert_eq!(d.spots(), 6);
        assert_eq!(
            &d.spot_dirs()[..3],
            a.spot_dirs(),
            "the spots dial keeps the shared prefix"
        );
    }

    /// **A hot spot is a white-hot core, seam or no seam.** The tile nearest a
    /// plume's centre reads near the spot peak — hotter than any pure seam tile
    /// can reach — and a zero-spot field is exactly the pure seam field.
    #[test]
    fn spots_burn_white_hot_wherever_they_are() {
        let map = HexMap::new(MIN_FREQ);
        let none = SeamField::new(&map, DEFAULT_CELLS, 0, 9);
        let some = SeamField::new(&map, DEFAULT_CELLS, 4, 9);
        assert!(none.spot_dirs().is_empty());
        assert_eq!(some.spot_dirs().len(), 4);
        for centre in some.spot_dirs() {
            let (tile, _) = map
                .grid()
                .dirs
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.dot(*centre).total_cmp(&b.1.dot(*centre)))
                .expect("tiles exist");
            assert!(
                some.heat(tile as u32) > 0.85,
                "the plume's core tile burns white-hot: {}",
                some.heat(tile as u32)
            );
        }
        // The spot field only ADDS heat — nothing cools, and far from every
        // spot the two fields agree.
        for t in 0..map.len() as u32 {
            assert!(some.heat(t) >= none.heat(t) - 1e-6, "tile {t} cooled");
        }
    }
}
