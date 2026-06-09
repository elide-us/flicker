//! HexWorld — the **flat neighbour graph**, with formal edge-neighbour finding.
//!
//! Each hemisphere is a **hexagon-of-hexagons** of radius `R` (ring `k` holds
//! `6k` hexes, total `1 + 3R(R+1)` per hemisphere). That is precisely the shape
//! whose adjacency **cube coordinates** describe exactly: every hex maps to a
//! cube `(x, y, z)` with `x + y + z = 0`, its ring is the cube distance from the
//! pole, and its six edge-neighbours are the six cube-adjacent cells. Intra-
//! hemisphere adjacency is therefore exact and **symmetric by construction** at
//! any size — no proportional-mapping approximations.
//!
//! The two hemispheres are joined at the equator (ring `R`) by a **symmetric
//! fold**: north `(R, p)` pairs with south `(R, p)` and `(R, p−1)` (the half-hex
//! interlock), defined as an undirected edge set so both directions agree.
//!
//! Result: [`HexMap::neighbours`] returns 5–6 distinct neighbours per hex (6 in
//! the interior, 6 on equator edges, 5 at the six equator corners — the
//! intentional defect of folding a sphere flat), with **zero asymmetry, zero
//! duplicates, zero self-references** for any `R`. That is the property a
//! halo-exchange simulation needs: if A pulls from B, B pushes to A.
//!
//! ## Scale (the practical max)
//!
//! `total(R) = 2 + 6R(R+1)`. The addressing is the ceiling: a `u32` index tops
//! out near `R ≈ 26,700` (~4.3 billion hexes) — at the spec's ~49.6 mi/hex
//! (2048 clusters × 128 ft) that great-circle of ~110k hexes is a star-sized
//! world (~220× Earth's circumference); an *Earth-sized* planet is only
//! ~`R = 125` (~95k hexes). `u64` is effectively unbounded. This implementation also precomputes a cube→index map
//! (O(N) memory), which caps the *in-RAM* graph at ~tens of millions of hexes
//! (a few GB) — ample for any test world. Planet scale would drop the map for
//! analytic cube↔pos arithmetic (O(1) memory, O(1) per neighbour); past that the
//! real wall is **sim compute per cycle** (the sweep is O(N)), not addressing.

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

/// Cube coordinate of a hex within its hemisphere (`x + y + z == 0`).
type Cube = (i32, i32, i32);

/// The six cube neighbour directions, in cyclic order (Red Blob convention).
const CUBE_DIRS: [Cube; 6] = [
    (1, -1, 0),
    (1, 0, -1),
    (0, 1, -1),
    (-1, 1, 0),
    (-1, 0, 1),
    (0, -1, 1),
];

#[inline]
fn cube_add(a: Cube, b: Cube) -> Cube {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

/// Cube distance from the pole = the hex's ring.
#[inline]
fn cube_ring(c: Cube) -> i32 {
    (c.0.abs() + c.1.abs() + c.2.abs()) / 2
}

/// Spiral `(ring, pos)` → cube: walk the hexagonal ring at radius `ring`,
/// starting one corner out along direction 4 and stepping around the six sides.
/// This is the forward map; [`HexMap`] inverts it with a precomputed table.
fn spiral_to_cube(ring: u32, pos: u32) -> Cube {
    if ring == 0 {
        return (0, 0, 0);
    }
    let k = ring as i32;
    let mut c = (CUBE_DIRS[4].0 * k, CUBE_DIRS[4].1 * k, CUBE_DIRS[4].2 * k);
    let mut p = 0;
    for dir in CUBE_DIRS {
        for _ in 0..ring {
            if p == pos {
                return c;
            }
            c = cube_add(c, dir);
            p += 1;
        }
    }
    c // unreachable for pos < 6*ring
}

/// The world container: a flat array of hexes with exact cube-based adjacency.
pub struct HexMap {
    rings: u32,
    coords: Vec<HexCoord>,
    index_of: HashMap<HexCoord, u32>,
    cubes: Vec<Cube>,
    cube_index: HashMap<(Hemisphere, Cube), u32>,
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
        let mut cubes = Vec::with_capacity(coords.len());
        let mut cube_index = HashMap::with_capacity(coords.len());
        for (i, c) in coords.iter().enumerate() {
            let cube = spiral_to_cube(c.ring, c.pos);
            cubes.push(cube);
            cube_index.insert((c.hemi, cube), i as u32);
        }
        Self {
            rings,
            coords,
            index_of,
            cubes,
            cube_index,
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
    pub fn cube(&self, index: u32) -> Cube {
        self.cubes[index as usize]
    }
    pub fn ring_len(ring: u32) -> u32 {
        if ring == 0 {
            1
        } else {
            6 * ring
        }
    }

    /// The hex's **edge-neighbours** — 5 or 6 distinct indices, symmetric for
    /// every hex at any size. Intra-hemisphere neighbours come from cube
    /// adjacency (this handles the pole and every interior ring uniformly);
    /// equator hexes additionally fold to the opposite hemisphere.
    pub fn neighbours(&self, index: u32) -> Vec<u32> {
        let c = self.coord(index);
        let cube = self.cube(index);
        let r = self.rings as i32;
        let mut out = Vec::with_capacity(6);

        // Same-hemisphere neighbours: cube-adjacent cells still inside the
        // hexagon. Off-perimeter steps (ring R+1) are dropped — the equator
        // fold below replaces them.
        for dir in CUBE_DIRS {
            let nc = cube_add(cube, dir);
            if cube_ring(nc) <= r {
                if let Some(&j) = self.cube_index.get(&(c.hemi, nc)) {
                    out.push(j);
                }
            }
        }

        // Equator fold (undirected, so it is symmetric): north (R,p) ↔ south
        // (R,p) and (R,p−1); the mirror for south.
        if c.ring == self.rings {
            let n = Self::ring_len(self.rings);
            let (a, b) = match c.hemi {
                Hemisphere::North => (c.pos % n, (c.pos + n - 1) % n),
                Hemisphere::South => (c.pos % n, (c.pos + 1) % n),
            };
            let o = c.hemi.opposite();
            out.push(self.index(HexCoord { hemi: o, ring: self.rings, pos: a }));
            out.push(self.index(HexCoord { hemi: o, ring: self.rings, pos: b }));
        }
        out
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
    fn index_coord_and_cube_bijections() {
        let m = HexMap::new(6);
        for i in 0..m.total() {
            assert_eq!(m.index(m.coord(i)), i);
            // cube round-trips back to the same index within its hemisphere.
            assert_eq!(m.cube_index[&(m.coord(i).hemi, m.cube(i))], i);
        }
    }

    /// The load-bearing reliability property, across a wide range of sizes:
    /// every adjacency is mutual, with no duplicate or self references.
    #[test]
    fn neighbours_are_symmetric_clean_at_every_size() {
        for r in 1..=30 {
            let m = HexMap::new(r);
            for i in 0..m.total() {
                let ns = m.neighbours(i);
                assert!(ns.len() == 5 || ns.len() == 6, "R={r}: deg {} at {i}", ns.len());
                for (k, &j) in ns.iter().enumerate() {
                    assert_ne!(j, i, "R={r}: self-ref at {i}");
                    assert!(!ns[..k].contains(&j), "R={r}: dup {j} at {i}");
                    assert!(j < m.total());
                    assert!(m.neighbours(j).contains(&i), "R={r}: {i}->{j} not mutual");
                }
            }
        }
    }

    #[test]
    fn interior_hexes_have_six_neighbours() {
        let m = HexMap::new(5);
        for i in 0..m.total() {
            let c = m.coord(i);
            if c.ring < m.rings {
                // pole and every non-equator ring: a full six.
                assert_eq!(m.neighbours(i).len(), 6, "ring {} pos {}", c.ring, c.pos);
            }
        }
    }

    #[test]
    fn pole_neighbours_are_ring_one() {
        let m = HexMap::new(4);
        let pole = m.index(HexCoord { hemi: Hemisphere::North, ring: 0, pos: 0 });
        let ns = m.neighbours(pole);
        assert_eq!(ns.len(), 6);
        for j in ns {
            assert_eq!(m.coord(j).ring, 1);
        }
    }

    #[test]
    fn equator_hexes_fold_across_hemispheres() {
        let r = 4;
        let m = HexMap::new(r);
        let eq = m.index(HexCoord { hemi: Hemisphere::North, ring: r, pos: 0 });
        let crosses = m.neighbours(eq).into_iter().filter(|&j| m.coord(j).hemi == Hemisphere::South).count();
        assert_eq!(crosses, 2, "equator hex should fold to exactly two south hexes");
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
