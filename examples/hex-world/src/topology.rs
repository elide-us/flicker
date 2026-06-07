//! HexWorld — the **flat neighbour graph** (per the Flat Neighbor Graph &
//! Celestial Orientation spec).
//!
//! The world data is a flat graph: a linear array of hexes, each carrying **6
//! explicit edge-neighbour refs**, baked as data and never recomputed from
//! geometry. **There is no sphere in the data.** [`HexMap::celestial_dir`] is the
//! one read-only sphere hook — consumed only by the celestial sim, never by the
//! data or render paths.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hemisphere {
    North,
    South,
}

impl Hemisphere {
    pub fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HexCoord {
    pub hemi: Hemisphere,
    pub ring: u32,
    pub pos: u32,
}

// Fixed edge order for the 6 baked refs, consumed by the flat render layout:
// W and E are the same-ring mates; NW/NE are inboard (toward the pole); SW/SE
// are outboard (away from the pole, or across the equator seam).
pub const EDGE_W: usize = 0;
pub const EDGE_NW: usize = 1;
pub const EDGE_NE: usize = 2;
pub const EDGE_E: usize = 3;
pub const EDGE_SE: usize = 4;
pub const EDGE_SW: usize = 5;

/// The world container: a flat array of hexes with baked adjacency.
pub struct HexMap {
    rings: u32,
    coords: Vec<HexCoord>,
    index_of: HashMap<HexCoord, u32>,
}

impl HexMap {
    /// Build a world with `rings` rings per hemisphere. Array order: north pole,
    /// north rings 1..=R, south rings R..=1 (the mirror), south pole (last).
    pub fn new(rings: u32) -> Self {
        assert!(rings >= 1);
        let mut coords = vec![HexCoord {
            hemi: Hemisphere::North,
            ring: 0,
            pos: 0,
        }];
        for k in 1..=rings {
            for p in 0..Self::ring_len(k) {
                coords.push(HexCoord {
                    hemi: Hemisphere::North,
                    ring: k,
                    pos: p,
                });
            }
        }
        for k in (1..=rings).rev() {
            for p in 0..Self::ring_len(k) {
                coords.push(HexCoord {
                    hemi: Hemisphere::South,
                    ring: k,
                    pos: p,
                });
            }
        }
        coords.push(HexCoord {
            hemi: Hemisphere::South,
            ring: 0,
            pos: 0,
        });
        let index_of = coords.iter().enumerate().map(|(i, &c)| (c, i as u32)).collect();
        Self {
            rings,
            coords,
            index_of,
        }
    }

    pub fn total(&self) -> u32 {
        self.coords.len() as u32
    }
    pub fn coord(&self, index: u32) -> HexCoord {
        self.coords[index as usize]
    }
    pub fn index(&self, coord: HexCoord) -> u32 {
        self.index_of[&coord]
    }
    pub fn ring_len(ring: u32) -> u32 {
        if ring == 0 {
            1
        } else {
            6 * ring
        }
    }

    /// The 6 baked edge-neighbour refs in `EDGE_*` order. Always 6; at a growth
    /// seam a ref may repeat — the graph just stores whatever §1.3 resolves.
    pub fn edge_refs(&self, index: u32) -> [u32; 6] {
        let c = self.coord(index);
        // Pole: its six edges are ring-1's hexes, in spiral order.
        if c.ring == 0 {
            return std::array::from_fn(|e| {
                self.index(HexCoord {
                    hemi: c.hemi,
                    ring: 1,
                    pos: e as u32,
                })
            });
        }
        let n = Self::ring_len(c.ring);
        let same_w = self.index(HexCoord {
            pos: (c.pos + n - 1) % n,
            ..c
        });
        let same_e = self.index(HexCoord {
            pos: (c.pos + 1) % n,
            ..c
        });
        let (nw, ne) = self.cross_ring(c, c.ring - 1);
        let (se, sw) = if c.ring == self.rings {
            self.equator_join(c)
        } else {
            self.cross_ring(c, c.ring + 1)
        };
        let mut r = [0u32; 6];
        r[EDGE_W] = same_w;
        r[EDGE_NW] = nw;
        r[EDGE_NE] = ne;
        r[EDGE_E] = same_e;
        r[EDGE_SE] = se;
        r[EDGE_SW] = sw;
        r
    }

    /// Two refs in `target` ring, found by proportional position mapping.
    fn cross_ring(&self, c: HexCoord, target: u32) -> (u32, u32) {
        if target == 0 {
            // ring 1's inboard pair is the single pole hex.
            let p = self.index(HexCoord {
                hemi: c.hemi,
                ring: 0,
                pos: 0,
            });
            return (p, p);
        }
        let from = Self::ring_len(c.ring) as f32;
        let to = Self::ring_len(target);
        let f = (c.pos as f32 + 0.5) / from * to as f32 - 0.5;
        let a = f.floor().rem_euclid(to as f32) as u32;
        let b = (a + 1) % to;
        (
            self.index(HexCoord {
                hemi: c.hemi,
                ring: target,
                pos: a,
            }),
            self.index(HexCoord {
                hemi: c.hemi,
                ring: target,
                pos: b,
            }),
        )
    }

    /// Equator seam: outboard refs come from the opposite hemisphere's
    /// equatorial ring (identical edge count) at a fixed half-hex offset.
    fn equator_join(&self, c: HexCoord) -> (u32, u32) {
        let r = self.rings;
        let n = Self::ring_len(r);
        let o = c.hemi.opposite();
        let a = c.pos % n;
        let b = (c.pos + n - 1) % n; // the fixed-convention half-step
        (
            self.index(HexCoord {
                hemi: o,
                ring: r,
                pos: a,
            }),
            self.index(HexCoord {
                hemi: o,
                ring: r,
                pos: b,
            }),
        )
    }

    /// **Read-only** celestial orientation: the hex's theoretical unit point on a
    /// sphere. Stubbed even-ish distribution (naive ring-radius → lat/lon). The
    /// *only* place the sphere appears, and only the celestial sim reads it.
    pub fn celestial_dir(&self, index: u32) -> [f32; 3] {
        let c = self.coord(index);
        let mag = 90.0 - (c.ring as f32) * (90.0 / (self.rings as f32 + 0.5));
        let lat = match c.hemi {
            Hemisphere::North => mag,
            Hemisphere::South => -mag,
        }
        .to_radians();
        let lon = if c.ring == 0 {
            0.0
        } else {
            ((c.pos as f32 + 0.5) / Self::ring_len(c.ring) as f32 * 360.0).to_radians()
        };
        let (clat, slat) = (lat.cos(), lat.sin());
        [clat * lon.cos(), slat, clat * lon.sin()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_matches_closed_form() {
        for r in 1..=6 {
            assert_eq!(HexMap::new(r).total(), 2 + 6 * r * (r + 1));
        }
    }

    #[test]
    fn index_coord_bijection() {
        let m = HexMap::new(5);
        for i in 0..m.total() {
            assert_eq!(m.index(m.coord(i)), i);
        }
    }

    #[test]
    fn every_hex_has_six_valid_refs() {
        for r in 1..=6 {
            let m = HexMap::new(r);
            for i in 0..m.total() {
                let refs = m.edge_refs(i);
                for &j in &refs {
                    assert!(j < m.total(), "ref {j} out of range for {i} (R={r})");
                }
            }
        }
    }

    #[test]
    fn pole_edges_are_ring_one() {
        let m = HexMap::new(4);
        let pole = m.index(HexCoord {
            hemi: Hemisphere::North,
            ring: 0,
            pos: 0,
        });
        for &j in &m.edge_refs(pole) {
            assert_eq!(m.coord(j).ring, 1);
        }
    }

    #[test]
    fn equator_outboard_crosses_hemispheres() {
        let r = 4;
        let m = HexMap::new(r);
        let eq = m.index(HexCoord {
            hemi: Hemisphere::North,
            ring: r,
            pos: 0,
        });
        let refs = m.edge_refs(eq);
        assert_eq!(m.coord(refs[EDGE_SE]).hemi, Hemisphere::South);
        assert_eq!(m.coord(refs[EDGE_SW]).hemi, Hemisphere::South);
    }

    #[test]
    fn celestial_is_unit() {
        let m = HexMap::new(4);
        for i in 0..m.total() {
            let d = m.celestial_dir(i);
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4);
        }
    }
}
