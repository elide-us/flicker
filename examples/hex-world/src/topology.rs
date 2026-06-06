//! Hex-planet topology — the "virtual sphere" addressing and adjacency model.
//!
//! This is the experiment's real payload. Everything visual is a viewer for it.
//!
//! # The model
//!
//! A planet is a polar-symmetric stack of latitude rings. `R` (`rings`) is the
//! number of rings from a pole *to the equator*; it is the single planet-size
//! knob. The two hemispheres are mirror images joined directly at the equator —
//! there is **no shared equator band** (the two outermost rings are distinct and
//! touch across the fold). That choice is what makes the minimal planet 14 hexes,
//! not 8:
//!
//! ```text
//! total = 2 + 6·R·(R+1)
//!   R=1 -> 14   (minimum)
//!   R=2 -> 38
//!   R=3 -> 74   (test floor, 30° per latitude ring)
//!   R=4 -> 122
//! ```
//!
//! Each ring subdivides latitude; each ring further out subdivides longitude:
//! - ring 0 is a single cap hex (a pole),
//! - ring `k` (1..=R) holds `6k` hexes, each spanning the longitude sector
//!   `[p/(6k), (p+1)/(6k))` of the full circle.
//!
//! Latitude is uniform at `90°/R` per ring (pole = ±90°, equator = 0°).
//!
//! # Adjacency
//!
//! This is a latitude/longitude grid, not a strict hexagonal lattice — "weirdly
//! shaped, and it doesn't have to be perfect." Neighbours are:
//! - the two in-ring cells (`pos ± 1`, wrapping the ring),
//! - every cell in the next ring toward the pole whose longitude sector overlaps,
//! - every cell in the next ring toward the equator whose longitude sector
//!   overlaps — and at the equator (`ring == R`) that "next ring" is the *other
//!   hemisphere's* ring R (the fold).
//!
//! Because the two equator rings have equal counts and aligned sectors, the fold
//! is identity in longitude: `North(R, p)` ↔ `South(R, p)`. There is no chirality
//! flip to get wrong here — but [`Planet::neighbors`] is still validated by the
//! adjacency-symmetry test, which would catch one if the model grew it.

// The adjacency API (`neighbors`, `index`, `opposite`, …) is consumed by the
// stage-3 streaming cache and is fully exercised by the tests below; the
// stage-2 binary doesn't call all of it yet, so silence dead-code until then.
#![allow(dead_code)]

use std::collections::HashMap;

/// Smallest legal planet: R=1, 14 hexes.
pub const MIN_RINGS: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hemisphere {
    North,
    South,
}

impl Hemisphere {
    pub fn opposite(self) -> Self {
        match self {
            Hemisphere::North => Hemisphere::South,
            Hemisphere::South => Hemisphere::North,
        }
    }
}

/// A hex addressed by hemisphere / ring / position-in-ring.
///
/// `ring == 0` is the hemisphere's pole and always has `pos == 0`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HexCoord {
    pub hemi: Hemisphere,
    pub ring: u32,
    pub pos: u32,
}

/// A whole planet: the ring count plus the canonical index↔coord mapping.
///
/// Array order runs pole-to-pole with the fold in the middle:
/// `North pole, North ring 1..=R, South ring R..=1, South pole`. So the two
/// equator rings are adjacent in the array and the south pole is the last index
/// — matching the spec's "south pole is the last element."
pub struct Planet {
    rings: u32,
    coords: Vec<HexCoord>,
    index_of: HashMap<HexCoord, u32>,
}

impl Planet {
    /// Build a planet with `rings` (>= [`MIN_RINGS`]) rings per hemisphere.
    pub fn new(rings: u32) -> Self {
        assert!(rings >= MIN_RINGS, "a planet needs at least {MIN_RINGS} ring");
        let coords = build_coords(rings);
        let index_of = coords
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, i as u32))
            .collect();
        Self {
            rings,
            coords,
            index_of,
        }
    }

    pub fn rings(&self) -> u32 {
        self.rings
    }

    /// `2 + 6·R·(R+1)`.
    pub fn total_hexes(&self) -> u32 {
        self.coords.len() as u32
    }

    /// Hexes in `ring`: 1 at the pole, `6·ring` otherwise.
    pub fn ring_len(ring: u32) -> u32 {
        if ring == 0 {
            1
        } else {
            6 * ring
        }
    }

    pub fn coord(&self, index: u32) -> HexCoord {
        self.coords[index as usize]
    }

    pub fn index(&self, coord: HexCoord) -> u32 {
        self.index_of[&coord]
    }

    /// Array indices of every hex adjacent to `index`.
    pub fn neighbors(&self, index: u32) -> Vec<u32> {
        self.neighbor_coords(self.coord(index))
            .into_iter()
            .map(|c| self.index(c))
            .collect()
    }

    /// Adjacency in coordinate space (see module docs for the rules).
    pub fn neighbor_coords(&self, c: HexCoord) -> Vec<HexCoord> {
        let r = self.rings;
        let mut out = Vec::with_capacity(6);

        // Pole: bordered by the whole of ring 1 in the same hemisphere.
        if c.ring == 0 {
            for p in 0..Self::ring_len(1) {
                out.push(HexCoord {
                    hemi: c.hemi,
                    ring: 1,
                    pos: p,
                });
            }
            return out;
        }

        let n = Self::ring_len(c.ring);
        // In-ring (the ring is a cycle, so east eventually wraps home).
        out.push(HexCoord {
            hemi: c.hemi,
            ring: c.ring,
            pos: (c.pos + 1) % n,
        });
        out.push(HexCoord {
            hemi: c.hemi,
            ring: c.ring,
            pos: (c.pos + n - 1) % n,
        });

        // Toward the pole.
        if c.ring - 1 == 0 {
            out.push(HexCoord {
                hemi: c.hemi,
                ring: 0,
                pos: 0,
            });
        } else {
            let k2 = c.ring - 1;
            for q in 0..Self::ring_len(k2) {
                if sectors_overlap(c.ring, c.pos, k2, q) {
                    out.push(HexCoord {
                        hemi: c.hemi,
                        ring: k2,
                        pos: q,
                    });
                }
            }
        }

        // Toward the equator — or across the fold at the equator itself.
        if c.ring < r {
            let k2 = c.ring + 1;
            for q in 0..Self::ring_len(k2) {
                if sectors_overlap(c.ring, c.pos, k2, q) {
                    out.push(HexCoord {
                        hemi: c.hemi,
                        ring: k2,
                        pos: q,
                    });
                }
            }
        } else {
            // ring == R: glue to the other hemisphere's equator ring.
            for q in 0..n {
                if sectors_overlap(c.ring, c.pos, r, q) {
                    out.push(HexCoord {
                        hemi: c.hemi.opposite(),
                        ring: r,
                        pos: q,
                    });
                }
            }
        }

        out
    }

    /// Latitude of a ring in degrees: +90° at a pole down toward 0° at the
    /// equator. Spacing is `90°/(R+0.5)`, **not** `90°/R`: that lands the two
    /// equator rings at ±half-a-step so they *straddle* the equator (meeting at
    /// lat 0) instead of both sitting on lat 0 and drawing over each other.
    pub fn latitude_deg(&self, c: HexCoord) -> f32 {
        let mag = 90.0 - (c.ring as f32) * (90.0 / (self.rings as f32 + 0.5));
        match c.hemi {
            Hemisphere::North => mag,
            Hemisphere::South => -mag,
        }
    }

    /// Longitude of a hex's center in degrees `[0, 360)`. `None` for the poles
    /// (a cap has no single longitude).
    ///
    /// The southern hemisphere is rotated half an equator-cell so its teeth
    /// **interlock** with the north across the fold instead of meeting
    /// point-to-point (which left a zigzag seam). The offset is constant per
    /// hemisphere, so it changes nothing about the south's internal meshing —
    /// only the equator junction, where it lands exactly half a cell.
    pub fn longitude_center_deg(&self, c: HexCoord) -> Option<f32> {
        if c.ring == 0 {
            None
        } else {
            let n = Self::ring_len(c.ring) as f32;
            let base = ((c.pos as f32) + 0.5) / n * 360.0;
            // Equator cell = 360/(6R); half = 30/R degrees.
            let offset = match c.hemi {
                Hemisphere::North => 0.0,
                Hemisphere::South => 30.0 / self.rings as f32,
            };
            Some((base + offset).rem_euclid(360.0))
        }
    }

    /// Hex center as a unit vector on the virtual sphere (Y up, north pole at
    /// +Y). This is the "fake the curve out of a flat data set" placement hook
    /// the god-view will use; the data stays flat, only the angle is known.
    pub fn unit_position(&self, c: HexCoord) -> [f32; 3] {
        let lat = self.latitude_deg(c).to_radians();
        let lon = self.longitude_center_deg(c).unwrap_or(0.0).to_radians();
        let (clat, slat) = (lat.cos(), lat.sin());
        [clat * lon.cos(), slat, clat * lon.sin()]
    }
}

/// Do the longitude sectors of `(k1,p1)` and `(k2,p2)` share interior (an edge,
/// not just a corner)? Pure integer test — no float boundary ambiguity.
///
/// Sector `(k,p) = [p/(6k), (p+1)/(6k))`. The common factor 6 cancels, leaving:
/// overlap ⇔ `p1·k2 < (p2+1)·k1` AND `p2·k1 < (p1+1)·k2`.
fn sectors_overlap(k1: u32, p1: u32, k2: u32, p2: u32) -> bool {
    let (k1, p1, k2, p2) = (k1 as u64, p1 as u64, k2 as u64, p2 as u64);
    p1 * k2 < (p2 + 1) * k1 && p2 * k1 < (p1 + 1) * k2
}

/// Canonical array order: N pole, N ring 1..=R, S ring R..=1, S pole.
fn build_coords(rings: u32) -> Vec<HexCoord> {
    let mut v = Vec::new();
    v.push(HexCoord {
        hemi: Hemisphere::North,
        ring: 0,
        pos: 0,
    });
    for k in 1..=rings {
        for p in 0..Planet::ring_len(k) {
            v.push(HexCoord {
                hemi: Hemisphere::North,
                ring: k,
                pos: p,
            });
        }
    }
    for k in (1..=rings).rev() {
        for p in 0..Planet::ring_len(k) {
            v.push(HexCoord {
                hemi: Hemisphere::South,
                ring: k,
                pos: p,
            });
        }
    }
    v.push(HexCoord {
        hemi: Hemisphere::South,
        ring: 0,
        pos: 0,
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn expected_total(r: u32) -> u32 {
        2 + 6 * r * (r + 1)
    }

    #[test]
    fn total_matches_closed_form_and_your_numbers() {
        // The numbers you called out by hand must fall straight out of the model.
        assert_eq!(Planet::new(1).total_hexes(), 14, "minimum planet");
        assert_eq!(Planet::new(3).total_hexes(), 74, "test floor");
        for r in 1..=8 {
            assert_eq!(Planet::new(r).total_hexes(), expected_total(r), "R={r}");
        }
    }

    #[test]
    fn index_coord_is_a_bijection() {
        for r in 1..=6 {
            let p = Planet::new(r);
            let mut seen = HashSet::new();
            for i in 0..p.total_hexes() {
                let c = p.coord(i);
                assert_eq!(p.index(c), i, "round-trip i={i} R={r}");
                assert!(seen.insert(c), "duplicate coord at i={i} R={r}");
            }
        }
    }

    #[test]
    fn poles_are_first_and_last_and_cap_the_array() {
        for r in 1..=6 {
            let p = Planet::new(r);
            assert_eq!(
                p.coord(0),
                HexCoord {
                    hemi: Hemisphere::North,
                    ring: 0,
                    pos: 0
                }
            );
            assert_eq!(
                p.coord(p.total_hexes() - 1),
                HexCoord {
                    hemi: Hemisphere::South,
                    ring: 0,
                    pos: 0
                }
            );
        }
    }

    #[test]
    fn ring_lengths_are_six_k() {
        let p = Planet::new(4);
        for i in 0..p.total_hexes() {
            let c = p.coord(i);
            let expect = if c.ring == 0 { 1 } else { 6 * c.ring };
            // pos must be in range for its ring
            assert!(c.pos < expect, "pos {} out of range for ring {}", c.pos, c.ring);
        }
    }

    #[test]
    fn adjacency_is_symmetric() {
        // THE fold guard: if any seam (equator fold included) were wired one-way,
        // some j would list i without i listing j. Holds for every planet size.
        for r in 1..=6 {
            let p = Planet::new(r);
            for i in 0..p.total_hexes() {
                for j in p.neighbors(i) {
                    assert!(
                        p.neighbors(j).contains(&i),
                        "asymmetry: {i} -> {j} but not back (R={r}, {:?} -> {:?})",
                        p.coord(i),
                        p.coord(j),
                    );
                }
            }
        }
    }

    #[test]
    fn neighbors_are_valid_unique_and_not_self() {
        for r in 1..=6 {
            let p = Planet::new(r);
            for i in 0..p.total_hexes() {
                let ns = p.neighbors(i);
                let set: HashSet<u32> = ns.iter().copied().collect();
                assert_eq!(set.len(), ns.len(), "dup neighbor of {i} (R={r})");
                assert!(!set.contains(&i), "{i} neighbors itself (R={r})");
                for &j in &ns {
                    assert!(j < p.total_hexes(), "neighbor {j} out of range (R={r})");
                }
            }
        }
    }

    #[test]
    fn pole_borders_all_of_ring_one() {
        for r in 1..=6 {
            let p = Planet::new(r);
            let npole = p.index(HexCoord {
                hemi: Hemisphere::North,
                ring: 0,
                pos: 0,
            });
            let ns = p.neighbors(npole);
            assert_eq!(ns.len(), 6, "pole should touch 6 hexes (R={r})");
            for &j in &ns {
                let c = p.coord(j);
                assert_eq!(c.ring, 1, "pole neighbor not in ring 1 (R={r})");
                assert_eq!(c.hemi, Hemisphere::North);
            }
        }
    }

    #[test]
    fn equator_fold_is_identity_in_longitude() {
        // North(R, p) must border South(R, p) — same longitude across the fold.
        for r in 1..=6 {
            let p = Planet::new(r);
            for pos in 0..Planet::ring_len(r) {
                let north = p.index(HexCoord {
                    hemi: Hemisphere::North,
                    ring: r,
                    pos,
                });
                let south = p.index(HexCoord {
                    hemi: Hemisphere::South,
                    ring: r,
                    pos,
                });
                assert!(
                    p.neighbors(north).contains(&south),
                    "fold N({r},{pos}) !-> S({r},{pos}) (R={r})",
                );
            }
        }
    }

    #[test]
    fn equator_rings_straddle_the_line() {
        // Regression: the two equator rings must NOT share a latitude. They used
        // to both land on 0° and render on top of each other (south over north).
        for r in 1..=6 {
            let p = Planet::new(r);
            let north = p.latitude_deg(HexCoord {
                hemi: Hemisphere::North,
                ring: r,
                pos: 0,
            });
            let south = p.latitude_deg(HexCoord {
                hemi: Hemisphere::South,
                ring: r,
                pos: 0,
            });
            assert!(north > 0.0, "north equator ring above the line (R={r})");
            assert!(south < 0.0, "south equator ring below the line (R={r})");
            assert!((north + south).abs() < 1e-4, "rings must mirror (R={r})");
        }
    }

    #[test]
    fn south_leads_north_by_half_an_equator_cell() {
        // The equator teeth interlock only if the south ring is offset half a
        // cell from the north at the same `pos`.
        let r = 4;
        let p = Planet::new(r);
        let north = p
            .longitude_center_deg(HexCoord {
                hemi: Hemisphere::North,
                ring: r,
                pos: 0,
            })
            .unwrap();
        let south = p
            .longitude_center_deg(HexCoord {
                hemi: Hemisphere::South,
                ring: r,
                pos: 0,
            })
            .unwrap();
        let cell = 360.0 / (6.0 * r as f32);
        let diff = (south - north).rem_euclid(360.0);
        assert!(
            (diff - cell / 2.0).abs() < 1e-3,
            "south should lead north by half a cell ({}°), got {diff}°",
            cell / 2.0
        );
    }

    #[test]
    fn an_in_ring_walk_circles_back() {
        // Flying east along a ring returns to where you started.
        let start = HexCoord {
            hemi: Hemisphere::North,
            ring: 2,
            pos: 0,
        };
        let n = Planet::ring_len(2);
        let mut cur = start;
        for _ in 0..n {
            // east neighbour = same ring, pos+1
            cur = HexCoord {
                pos: (cur.pos + 1) % n,
                ..cur
            };
        }
        assert_eq!(cur, start, "ring did not close");
    }
}
