//! **The hex map, and nothing else.**
//!
//! One data structure: the tiles of a world, each identified by its own index.
//! No terrain, no materials, no elevation, no owner — those are later decisions
//! and every one of them is easier to add to a map that does not presume them.
//!
//! **The index IS the identity.** A tile's number is its position in the grid's
//! own arrays, which is what every neighbour lookup, every future per-tile
//! layer, and every save file will key on. Storing a parallel `Vec<u32>` of
//! indices would be the same fact written twice, and two copies of one fact is
//! how they drift.

use flicker_worldgrid::{icosphere_with_outlines, Sphere};
use glam::Vec3;

/// A tile's identity — its index in the map. A plain number on purpose: it is
/// the key everything else will hang off, so it stays the cheapest possible
/// thing to hold, compare and store.
pub type TileId = u32;

/// The smallest map the bench offers, in icosphere frequency — half the
/// standard world by diameter.
pub const MIN_FREQ: u32 = 48;
/// The largest — 1.25× the standard, around the ceiling of comfortable
/// human-scale gravity.
pub const MAX_FREQ: u32 = 120;
/// Where the bench opens (Aaron 2026-08-08): frequency 96 — 92,162 tiles, which
/// on an Earth-sized body makes each tile the standard ~49.65 mi across.
pub const DEFAULT_FREQ: u32 = clayengine::STANDARD_FREQ;

/// The planet size model — TILE_MI / EARTH_DIAMETER_MI / diameter_mi — moved
/// to `clayengine` (2026-08-28) when the world bake became a second consumer;
/// re-exported so the bench's readouts keep their one name for it.
pub use clayengine::{diameter_mi, TILE_MI};

/// **The map.** An equal-area hex tiling of a sphere: where each tile sits, who
/// it touches, and the outline that draws it.
pub struct HexMap {
    /// Icosphere frequency — the size dial. Tile COUNT is `10·f² + 2`.
    freq: u32,
    /// Tile centres and adjacency, from the shared grid.
    grid: Sphere,
    /// The corner ring of each tile, for drawing it.
    outlines: Vec<Vec<Vec3>>,
}

impl HexMap {
    /// Build the map at `freq`, clamped to the offered range.
    pub fn new(freq: u32) -> Self {
        let freq = freq.clamp(MIN_FREQ, MAX_FREQ);
        let (grid, outlines) = icosphere_with_outlines(freq);
        Self {
            freq,
            grid,
            outlines,
        }
    }

    /// The size dial this map was built at.
    pub fn freq(&self) -> u32 {
        self.freq
    }

    /// How many tiles the map has.
    pub fn len(&self) -> usize {
        self.grid.len()
    }

    /// Whether the map has no tiles (never true for a real frequency, but the
    /// lint asks and an honest answer is cheaper than an allow).
    pub fn is_empty(&self) -> bool {
        self.grid.len() == 0
    }

    /// Every tile, by index — the map's whole contents, since the index is all
    /// a tile is so far.
    pub fn tiles(&self) -> impl Iterator<Item = TileId> + '_ {
        0..self.len() as TileId
    }

    /// Which way `tile` faces from the centre of the world — its position on
    /// the unit sphere.
    pub fn direction(&self, tile: TileId) -> Vec3 {
        self.grid.dirs[tile as usize]
    }

    /// The tiles `tile` touches. Five for the twelve pentagons an icosphere
    /// cannot avoid, six for everything else.
    pub fn neighbours(&self, tile: TileId) -> &[u32] {
        &self.grid.neighbors[tile as usize]
    }

    /// The corner ring of `tile`, for drawing it.
    pub fn outline(&self, tile: TileId) -> &[Vec3] {
        &self.outlines[tile as usize]
    }

    /// The grid itself, for the mesh builder.
    pub fn grid(&self) -> &Sphere {
        &self.grid
    }

    /// Every outline, for the mesh builder.
    pub fn outlines(&self) -> &[Vec<Vec3>] {
        &self.outlines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clayengine::EARTH_DIAMETER_MI;

    /// The size dial covers exactly the offered range, and the tile count is the
    /// icosphere's own law — `10·f² + 2`.
    #[test]
    fn the_map_is_the_size_it_was_asked_for() {
        for freq in [MIN_FREQ, DEFAULT_FREQ, MAX_FREQ] {
            let map = HexMap::new(freq);
            assert_eq!(map.freq(), freq);
            assert_eq!(map.len(), (10 * freq * freq + 2) as usize, "freq {freq}");
        }
    }

    /// Out-of-range asks are clamped rather than refused: the dial has ends, and
    /// a bench that panics on a slider is no use.
    #[test]
    fn the_size_dial_has_ends() {
        assert_eq!(HexMap::new(1).freq(), MIN_FREQ);
        assert_eq!(HexMap::new(9_999).freq(), MAX_FREQ);
    }

    /// **The index IS the identity.** Every tile is its own position, once, with
    /// no gaps and no duplicates — the property every later per-tile layer will
    /// assume when it indexes by `TileId`.
    #[test]
    fn every_tile_is_its_own_index() {
        let map = HexMap::new(MIN_FREQ);
        let ids: Vec<TileId> = map.tiles().collect();
        assert_eq!(ids.len(), map.len());
        for (position, id) in ids.iter().enumerate() {
            assert_eq!(*id as usize, position);
        }
    }

    /// **Frequency IS planet size.** The tile stays the standard width, so the
    /// diameter scales linearly: the standard world at 96, half of it at 48,
    /// five quarters at 120 — the span the size dial offers.
    #[test]
    fn the_diameter_scales_linearly_with_frequency() {
        assert!((diameter_mi(DEFAULT_FREQ) - EARTH_DIAMETER_MI).abs() < 1e-9);
        assert!((diameter_mi(MIN_FREQ) - EARTH_DIAMETER_MI * 0.5).abs() < 1e-9);
        assert!((diameter_mi(MAX_FREQ) - EARTH_DIAMETER_MI * 1.25).abs() < 1e-9);
    }

    /// Adjacency is symmetric and hex — five neighbours only at the twelve
    /// pentagons an icosphere cannot avoid, six everywhere else. A map whose
    /// neighbours disagree with each other cannot support anything later.
    #[test]
    fn neighbours_are_mutual_and_hex() {
        let map = HexMap::new(MIN_FREQ);
        let mut pentagons = 0;
        for tile in map.tiles() {
            let n = map.neighbours(tile);
            match n.len() {
                5 => pentagons += 1,
                6 => {}
                other => panic!("tile {tile} has {other} neighbours"),
            }
            for &other in n {
                assert!(
                    map.neighbours(other).contains(&tile),
                    "tile {tile} touches {other} but not the other way round"
                );
            }
        }
        assert_eq!(pentagons, 12, "an icosphere has exactly twelve pentagons");
    }
}
