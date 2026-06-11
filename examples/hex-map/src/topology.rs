//! Flat hex-map topology: which ring of its hemisphere a tile sits in, and the
//! framework for the (ring-dependent) rules by which a tile joins its
//! neighbours.
//!
//! The two maps are numbered as one continuous spiral over the sphere: the
//! north (right) map runs **centre-outward** from the north pole (tile 0) to its
//! equator, then the south (left) map runs **equator-inward** to the south pole.
//! So within either map the tile number increases by ring, and the **outermost
//! ring of each map is the equator**, where the two hemispheres stitch together.
//!
//! Neighbour joins are deliberately **not uniform** — the rule a tile uses
//! depends on its [`RingClass`] (see [`JoinKind`]):
//!   * **same-ring** — the walk around one ring of a map,
//!   * **inboard / outboard** — to the adjacent rings (toward pole / equator),
//!   * **equator cross** — the north↔south stitch, which follows its own
//!     longitude/letter zipper (derived separately from the in-map joins),
//!   * **pole** — the ring-0 centre, with fewer than six neighbours.
//!
//! This module currently provides the ring classification and the equator
//! **fence** grouping used by the visualization; the join-rule bodies are added
//! here as they are specified.

/// Where a tile sits in its hemisphere's ring stack.
#[allow(dead_code)] // `Inner`/`Pole` payloads are read once the join rules land.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RingClass {
    /// Ring 0 — the map centre (a hemisphere pole).
    Pole,
    /// An interior ring `1..rings`.
    Inner(usize),
    /// The outermost ring (`== rings`) — the equator, where the maps stitch.
    Equator,
}

/// The kinds of neighbour join on the flat map. Which apply to a tile depends on
/// its [`RingClass`]; the equator and pole cases are special. The rule bodies
/// are filled in as they are specified — this enumerates the cases they cover.
#[allow(dead_code)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum JoinKind {
    /// Same ring of the same map — the ring walk.
    SameRing,
    /// One ring further in (toward the pole).
    Inboard,
    /// One ring further out (toward the equator).
    Outboard,
    /// The equator cross-map stitch (north equator ↔ south equator) — its own
    /// special derivation, distinct from the in-map joins.
    EquatorCross,
    /// The pole centre tile — fewer than six neighbours.
    Pole,
}

/// Tile-number layout of the two maps for a given ring count.
#[derive(Copy, Clone)]
pub struct Topology {
    /// Rings per map (the centre tile excluded).
    pub rings: usize,
    /// Tile count of one map (centre + `rings` rings). The north map occupies
    /// `[0, per_map)`, the south `[per_map, 2·per_map)`.
    pub per_map: u32,
}

impl Topology {
    pub fn new(rings: usize) -> Self {
        Self { rings, per_map: (1 + 3 * rings * (rings + 1)) as u32 }
    }

    /// Whether tile `n` is on the south (left, record-flipped) map.
    pub fn is_south(&self, n: u32) -> bool {
        n >= self.per_map
    }

    /// Ring class of tile `n` within its own map.
    pub fn ring_class(&self, n: u32) -> RingClass {
        let local = n % self.per_map; // index within this tile's own map
        let equator_count = 6 * self.rings as u32;
        if self.is_south(n) {
            // South map is numbered outer-ring-inward: equator first, pole last.
            if local < equator_count {
                RingClass::Equator
            } else if local == self.per_map - 1 {
                RingClass::Pole
            } else {
                let mut start = equator_count;
                for k in (1..self.rings).rev() {
                    let count = 6 * k as u32;
                    if local < start + count {
                        return RingClass::Inner(k);
                    }
                    start += count;
                }
                RingClass::Pole
            }
        } else {
            // North map is numbered centre-outward: pole 0, equator outermost.
            if local == 0 {
                return RingClass::Pole;
            }
            // Ring k occupies [1 + 3(k-1)k, 1 + 3k(k+1)).
            for k in 1..=self.rings {
                let end = 1 + 3 * k as u32 * (k as u32 + 1);
                if local < end {
                    return if k == self.rings { RingClass::Equator } else { RingClass::Inner(k) };
                }
            }
            RingClass::Equator
        }
    }

    /// The ring index `k`, side index, and the side's tile-number range for tile
    /// `n` — the `k`-tile chunk (one of the ring's six sides) it belongs to,
    /// taken in spiral-number order from the ring's lowest index. `None` for a
    /// pole (ring 0). Generalises the equator fences to every ring: a ring's
    /// side size equals its ring index, so the chunk interval scales with `k`.
    pub fn ring_side(&self, n: u32) -> Option<(usize, usize, std::ops::Range<u32>)> {
        if self.ring_class(n) == RingClass::Pole {
            return None;
        }
        let local = n % self.per_map;
        let map_base = if self.is_south(n) { self.per_map } else { 0 };
        let (k, ring_local_start) = if self.is_south(n) {
            // South numbers outer-ring-inward: ring `rings` first, then inward.
            let mut start = 0u32;
            let mut kk = self.rings;
            loop {
                let count = 6 * kk as u32;
                if local < start + count {
                    break (kk, start);
                }
                start += count;
                kk -= 1;
            }
        } else {
            // North numbers centre-outward: ring k at [1+3(k-1)k, 1+3k(k+1)).
            let mut kk = 1usize;
            loop {
                let s = 1 + 3 * (kk as u32 - 1) * kk as u32;
                let e = 1 + 3 * kk as u32 * (kk as u32 + 1);
                if local < e {
                    break (kk, s);
                }
                kk += 1;
            }
        };
        let side = ((local - ring_local_start) / k as u32) as usize;
        let lo = map_base + ring_local_start + side as u32 * k as u32;
        Some((k, side, lo..lo + k as u32))
    }

    /// If `n` is an equator tile, its **fence**: the equator is the outermost
    /// ring, so a fence is a ring side of `rings` tiles. Returns
    /// `(fence_index 0..6, tile-number range)`, or `None` off the equator.
    /// (The equator's own accessor — kept for the special cross-join rules.)
    #[allow(dead_code)]
    pub fn equator_fence(&self, n: u32) -> Option<(usize, std::ops::Range<u32>)> {
        match self.ring_side(n) {
            Some((k, side, range)) if k == self.rings => Some((side, range)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equator_fences_chunk_by_ring_size_from_lowest() {
        // rings 3: north equator 19..=36 → six fences of three, lowest first;
        // south equator 37..=54 → the same, continuing the sequence.
        let t = Topology::new(3);
        assert_eq!(t.per_map, 37);
        assert_eq!(t.equator_fence(19), Some((0, 19..22)));
        assert_eq!(t.equator_fence(20), Some((0, 19..22)));
        assert_eq!(t.equator_fence(21), Some((0, 19..22)));
        assert_eq!(t.equator_fence(22), Some((1, 22..25)));
        assert_eq!(t.equator_fence(36), Some((5, 34..37)));
        assert_eq!(t.equator_fence(37), Some((0, 37..40))); // south, next in sequence
        assert_eq!(t.equator_fence(52), Some((5, 52..55)));
        // Interior + pole tiles are not on any fence.
        assert_eq!(t.equator_fence(0), None); // north pole
        assert_eq!(t.equator_fence(7), None); // north inner ring
        assert_eq!(t.equator_fence(73), None); // south pole

        assert_eq!(t.ring_class(0), RingClass::Pole);
        assert_eq!(t.ring_class(7), RingClass::Inner(2));
        assert_eq!(t.ring_class(19), RingClass::Equator);
        assert_eq!(t.ring_class(37), RingClass::Equator);
        assert_eq!(t.ring_class(73), RingClass::Pole);
    }

    #[test]
    fn ring_sides_chunk_by_ring_index() {
        let t = Topology::new(3);
        // Side size scales with the ring: ring 1 → singletons, ring 2 → pairs,
        // ring 3 (equator) → triples; chunked from each ring's lowest number.
        assert_eq!(t.ring_side(1), Some((1, 0, 1..2)));
        assert_eq!(t.ring_side(6), Some((1, 5, 6..7)));
        assert_eq!(t.ring_side(7), Some((2, 0, 7..9)));
        assert_eq!(t.ring_side(9), Some((2, 1, 9..11)));
        assert_eq!(t.ring_side(19), Some((3, 0, 19..22)));
        assert_eq!(t.ring_side(37), Some((3, 0, 37..40))); // south equator
        assert_eq!(t.ring_side(55), Some((2, 0, 55..57))); // south inner ring 2
        assert_eq!(t.ring_side(0), None); // north pole
        assert_eq!(t.ring_side(73), None); // south pole
    }

    #[test]
    fn fences_scale_with_ring_count() {
        for rings in 1..=5 {
            let t = Topology::new(rings);
            // Every equator tile lands in exactly one fence of `rings` tiles,
            // and there are six fences per map.
            for &south in &[false, true] {
                let start = if south { t.per_map } else { 1 + 3 * (rings as u32 - 1) * rings as u32 };
                for f in 0..6u32 {
                    for j in 0..rings as u32 {
                        let n = start + f * rings as u32 + j;
                        assert_eq!(t.equator_fence(n), Some((f as usize, start + f * rings as u32..start + (f + 1) * rings as u32)));
                    }
                }
            }
        }
    }
}
