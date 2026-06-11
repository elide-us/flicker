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

/// Drawn-frame edge indices, `a`–`f` clockwise from the NW edge — the same
/// indexing as `edge_normal` and the a–f labels painted on every hex. A tile's
/// six edges carry fixed *fold roles*: `a`/`f` fold **inward** (toward the
/// pole), `c`/`d` fold **outward** (toward the equator), and `b`/`e` are the
/// **same-ring** tangential joins. The record flip is the involution
/// σ = (a↔c)(d↔f), `b`/`e` fixed — so across the equator a drawn edge `x` meshes
/// the south's drawn `σ(x)`, which is exactly "same edge in each tile's own
/// (local) frame" once the south's upside-down face is accounted for.
pub const A: usize = 0;
pub const C: usize = 2;
pub const D: usize = 3;
pub const F: usize = 5;

/// One edge-to-edge join from a fold zipper: the lower (giving) tile's edge
/// `lo_edge` meshes the upper (receiving) tile's edge `hi_edge`. Edges are
/// drawn-frame (a–f); this is the unit the fence tables are written in
/// (e.g. `60c-84a` is `Join { lo: 60, lo_edge: C, hi: 84, hi_edge: A }`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Join {
    pub lo: u32,
    pub lo_edge: usize,
    pub hi: u32,
    pub hi_edge: usize,
}

/// The fold zipper between a lower side `lo` (giving ring) and an upper side
/// `hi` (receiving ring), both as tile numbers already in matched mesh order.
/// One template covers every crease:
/// * **primary** `lo[i].c — hi[i].a`,
/// * **bridge** `hi[i].f — lo[i+1].d` within a hemisphere, or `hi[i].d —
///   lo[i+1].f` across the equator — the only thing the record flip changes is
///   which end of the σ-pair `{d,f}` the bridge sits on (`cross`).
///
/// The bridge ties `hi[i]` to the *next* lower tile, so one upper tile is shared
/// by two lower tiles — at a side boundary those lie in different fences, which
/// is the corner where an upper tile bridges two fences. Acyclic: the last
/// lower tile gets no bridge (it belongs to the neighbouring fence). Reproduces
/// the hand-authored fence excerpts exactly (see tests).
///
/// The reusable crease template — [`Topology::equator_cross`] is its cyclic
/// equator instance; the inboard/outboard per-ring folds will feed it their
/// side sequences next.
#[allow(dead_code)]
pub fn fold(lo: &[u32], hi: &[u32], cross: bool) -> Vec<Join> {
    let pairs = lo.len().min(hi.len());
    let mut joins = Vec::with_capacity(2 * pairs);
    let (bridge_hi, bridge_lo) = if cross { (D, F) } else { (F, D) };
    for i in 0..pairs {
        joins.push(Join { lo: lo[i], lo_edge: C, hi: hi[i], hi_edge: A });
        if let Some(&next) = lo.get(i + 1) {
            joins.push(Join { lo: next, lo_edge: bridge_lo, hi: hi[i], hi_edge: bridge_hi });
        }
    }
    joins
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

    /// First tile number of the north equator (outer) ring. The south equator is
    /// exactly `6·rings` higher (`per_map` away), so a north equator tile at ring
    /// index `i` and the south tile at the same index are partners.
    fn north_equator_start(&self) -> u32 {
        let r = self.rings as u32;
        1 + 3 * r * (r - 1)
    }

    /// The full equator stitch as a cyclic σ-zipper between the two outer rings,
    /// driven purely by tile number — **no proximity**. The north ring (lower)
    /// and south ring (upper) each hold `6·rings` tiles; north tile at ring index
    /// `i` joins `c → south[i]` (primary) and `f → south[i+1]` (bridge, modulo
    /// the ring). Returns `Join`s with `lo` north, `hi` south. Empty for the
    /// degenerate zero-ring map.
    #[allow(dead_code)] // consumed by tests / available to the mesh + HUD work.
    pub fn equator_cross(&self) -> Vec<Join> {
        let ring = 6 * self.rings as u32;
        if ring == 0 {
            return Vec::new();
        }
        let n0 = self.north_equator_start();
        let s0 = self.per_map; // == n0 + ring
        let mut joins = Vec::with_capacity(2 * ring as usize);
        for i in 0..ring {
            joins.push(Join { lo: n0 + i, lo_edge: C, hi: s0 + i, hi_edge: A });
            joins.push(Join { lo: n0 + i, lo_edge: F, hi: s0 + (i + 1) % ring, hi_edge: D });
        }
        joins
    }

    /// The across-equator neighbours of tile `n` (the two south twins of a north
    /// equator tile, or the two north twins of a south one), or empty when `n`
    /// is not an equator tile. The deterministic replacement for the old
    /// nearest-tile fill: a north tile reaches `south[i]` (its `c` edge) and
    /// `south[i+1]` (its `f` edge); a south tile reaches `north[j]` (`a`) and
    /// `north[j-1]` (`d`). Reciprocal by construction.
    pub fn equator_partners(&self, n: u32) -> Vec<u32> {
        if self.ring_class(n) != RingClass::Equator {
            return Vec::new();
        }
        let ring = 6 * self.rings as u32;
        let n0 = self.north_equator_start();
        let s0 = self.per_map;
        if self.is_south(n) {
            let j = n - s0;
            vec![n0 + j, n0 + (j + ring - 1) % ring]
        } else {
            let i = n - n0;
            vec![s0 + i, s0 + (i + 1) % ring]
        }
    }

    /// The across-equator twin reached from equator tile `n` over a specific
    /// drawn edge, or `None` if `edge` is not one of that tile's cross edges
    /// (the inward/tangential edges fold within the hemisphere — logical
    /// adjacency, not this). North tiles cross on `c` (→ `south[i]`) and `f`
    /// (→ `south[i+1]`); south tiles cross on `a` (→ `north[j]`) and `d`
    /// (→ `north[j-1]`). This is the corner disambiguation rule: when a sub-tile
    /// position is nearest one of these edges, this names the single twin to
    /// render — so the shared corner never needs the whole fan resolved.
    #[allow(dead_code)] // the render/travel wedge hookup consumes this next.
    pub fn equator_edge_partner(&self, n: u32, edge: usize) -> Option<u32> {
        if self.ring_class(n) != RingClass::Equator {
            return None;
        }
        let ring = 6 * self.rings as u32;
        let n0 = self.north_equator_start();
        let s0 = self.per_map;
        match (self.is_south(n), edge) {
            (false, C) => Some(s0 + (n - n0)),
            (false, F) => Some(s0 + (n - n0 + 1) % ring),
            (true, A) => Some(n0 + (n - s0)),
            (true, D) => Some(n0 + (n - s0 + ring - 1) % ring),
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

    /// Shorthand to read fence excerpts the way they were authored: `j(60, C)`
    /// is "tile 60's edge c".
    fn join(lo: u32, lo_edge: usize, hi: u32, hi_edge: usize) -> Join {
        Join { lo, lo_edge, hi, hi_edge }
    }

    #[test]
    fn fold_reproduces_the_equator_fence() {
        // The hand-authored cross-map excerpt (north 57‑60 ↔ south 81‑84):
        //   60c-84a 84d-59f 59c-83a 83d-58f 58c-82a 82d-57f
        let lo = [60, 59, 58, 57];
        let hi = [84, 83, 82];
        assert_eq!(
            fold(&lo, &hi, true),
            vec![
                join(60, C, 84, A),
                join(59, F, 84, D), // 84d-59f
                join(59, C, 83, A),
                join(58, F, 83, D), // 83d-58f
                join(58, C, 82, A),
                join(57, F, 82, D), // 82d-57f
            ],
        );
    }

    #[test]
    fn fold_reproduces_the_north_east_inter_ring_fence() {
        // The within-hemisphere ring-3 → ring-4 excerpt (north only):
        //   27c-45a 45f-26d 26c-44a 44f-25d 25c-43a
        let lo = [27, 26, 25];
        let hi = [45, 44, 43];
        assert_eq!(
            fold(&lo, &hi, false),
            vec![
                join(27, C, 45, A),
                join(26, D, 45, F), // 45f-26d
                join(26, C, 44, A),
                join(25, D, 44, F), // 44f-25d
                join(25, C, 43, A),
            ],
        );
    }

    #[test]
    fn equator_cross_matches_the_fence_and_is_reciprocal() {
        let t = Topology::new(4); // 122 tiles, the live default
        let cross = t.equator_cross();
        // The four primary + four bridge joins the excerpt named all appear.
        for want in [
            join(60, C, 84, A),
            join(59, F, 84, D),
            join(59, C, 83, A),
            join(58, F, 83, D),
            join(58, C, 82, A),
            join(57, F, 82, D),
        ] {
            assert!(cross.contains(&want), "missing {want:?}");
        }
        // Every equator tile of either map has exactly two cross twins, and the
        // relation is mutual.
        for n in 0..2 * t.per_map {
            let p = t.equator_partners(n);
            if t.ring_class(n) == RingClass::Equator {
                assert_eq!(p.len(), 2, "equator tile {n}");
                for q in p {
                    assert!(t.equator_partners(q).contains(&n), "{n}↔{q} not mutual");
                    assert_ne!(t.is_south(q), t.is_south(n), "{n}→{q} stayed on one map");
                }
            } else {
                assert!(p.is_empty(), "non-equator tile {n} got cross twins {p:?}");
            }
        }
    }

    #[test]
    fn equator_edge_partner_names_the_fence_twins() {
        let t = Topology::new(4);
        // Straight off the excerpt: 60c → 84, 59f → 84, 84a → 60, 84d → 59.
        assert_eq!(t.equator_edge_partner(60, C), Some(84));
        assert_eq!(t.equator_edge_partner(59, F), Some(84));
        assert_eq!(t.equator_edge_partner(84, A), Some(60));
        assert_eq!(t.equator_edge_partner(84, D), Some(59));
        // Non-cross edges fold within the hemisphere — not this rule's business.
        for e in [B_, D] {
            assert_eq!(t.equator_edge_partner(60, e), None, "north non-cross edge {e}");
        }
        // The two cross edges of every equator tile recover exactly its twins.
        for n in 0..2 * t.per_map {
            if t.ring_class(n) != RingClass::Equator {
                continue;
            }
            let mut twins: Vec<u32> = (0..6)
                .filter_map(|e| t.equator_edge_partner(n, e))
                .collect();
            twins.sort_unstable();
            let mut want = t.equator_partners(n);
            want.sort_unstable();
            assert_eq!(twins, want, "tile {n} edge twins ≠ partners");
        }
    }

    // `b` and `e` (the same-ring tangential edges) — named only to assert they
    // are never cross edges.
    const B_: usize = 1;

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
