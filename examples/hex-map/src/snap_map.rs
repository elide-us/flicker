//! The **snap map** — the click-to-select fold-in. When a tile is selected, its
//! neighbours snap flat into its tangent plane against the edges they share,
//! same-map and across the equator alike, so you can read the local
//! neighbourhood off one tile.
//!
//! Today this lays the **1-ring** fold (the selected tile + its ≤6 neighbours).
//! Slice 7 grows it into an **N-ring "horizon"** walk — repeatedly stepping the
//! topology adjacency outward — for the navigator; this module is its home.

use flicker::render::{Mat4, Vec3};

use crate::topology::Topology;
use crate::{edge_normal, left_center, HexInst, HEX_SPACING};

/// The fold-in layout for a selected tile: its neighbours placed against its
/// edges. `slots` pairs each neighbour's number with its centre in the selected
/// tile's plane; `flip`/`left` are the selected tile's orientation and map (so
/// neighbours draw with it). `center` is the selected tile's own number (the
/// anchor).
pub struct SnapMap {
    pub center: u32,
    pub left: bool,
    pub flip: bool,
    pub slots: Vec<(u32, Vec3)>,
}

impl SnapMap {
    /// Lay tile `s`'s neighbours flat into its tangent plane, aligned to its
    /// edges. Same-map neighbours (`within`) land on the exact edge their logical
    /// offset points along (flip-corrected for the left map); across-equator
    /// twins (the σ-zipper, [`Topology::equator_partners`]) fill the remaining
    /// outward edges by world-direction match. `m_right`/`m_left` are the live
    /// map transforms, so the cross-edge placement tracks the maps' rolls.
    pub fn build(
        hexes: &[HexInst],
        within: &[Vec<u32>],
        rings: usize,
        s: u32,
        m_right: &Mat4,
        m_left: &Mat4,
    ) -> SnapMap {
        let here = &hexes[s as usize];
        let xform = if here.left { *m_left } else { *m_right };
        let tile_center = |inst: &HexInst| {
            if inst.left {
                m_left.transform_point3(inst.center)
            } else {
                m_right.transform_point3(inst.center)
            }
        };
        // Edge direction in the flat frame, mirrored when the map is flipped, so
        // the slot lines up with the tile's *drawn* edge.
        let flip_dir = |v: Vec3| {
            if here.flip {
                Vec3::new(-v.x, v.y, v.z)
            } else {
                v
            }
        };
        let slot_pre = |e: usize| here.center + flip_dir(edge_normal(e)) * HEX_SPACING;
        let slots_pre: [Vec3; 6] = std::array::from_fn(slot_pre);
        let w_s = xform.transform_point3(here.center);

        let within_s = &within[s as usize];
        let cross = Topology::new(rings).equator_partners(s);
        let mut used = [false; 6];
        let mut out: Vec<(u32, Vec3)> = Vec::with_capacity(6);

        // Same-map neighbours → the edge their logical offset points along.
        for &n in within_s {
            let o = (hexes[n as usize].logical - here.logical).normalize_or_zero();
            let e = (0..6)
                .filter(|&e| !used[e])
                .max_by(|&a, &b| o.dot(edge_normal(a)).total_cmp(&o.dot(edge_normal(b))))
                .unwrap_or(0);
            used[e] = true;
            out.push((n, slots_pre[e]));
        }
        // Across-equator twins → the nearest remaining (outward) edge, by world
        // direction from the selected tile.
        for &n in cross.iter().filter(|n| !within_s.contains(n)) {
            let dir_n = (tile_center(&hexes[n as usize]) - w_s).normalize_or_zero();
            let e = (0..6)
                .filter(|&e| !used[e])
                .max_by(|&a, &b| {
                    let da = (xform.transform_point3(slots_pre[a]) - w_s).normalize_or_zero();
                    let db = (xform.transform_point3(slots_pre[b]) - w_s).normalize_or_zero();
                    dir_n.dot(da).total_cmp(&dir_n.dot(db))
                })
                .unwrap_or(0);
            used[e] = true;
            out.push((n, slots_pre[e]));
        }
        SnapMap { center: s, left: here.left, flip: here.flip, slots: out }
    }
}

/// A flat local layout of the tiles around `center` for the navigator's horizon
/// panel: `center` at the origin, each neighbour on the edge it shares, in the
/// standard (unflipped) hex frame `(edge_normal(e)·HEX_SPACING)`. Same-map
/// neighbours take the edge their logical offset points along; across-equator
/// twins fill the remaining edges. Returns `(tile_number, local_offset)`.
///
/// Radius 1 today (centre + its ≤6 snapped neighbours) — the N-ring outward walk
/// across the σ-zipper seams lands here next, so the navigator can show a broader
/// horizon and you can watch the toroidal continuity hold.
pub fn horizon(hexes: &[HexInst], within: &[Vec<u32>], rings: usize, center: u32) -> Vec<(u32, Vec3)> {
    let here = &hexes[center as usize];
    let mut used = [false; 6];
    let mut out: Vec<(u32, Vec3)> = vec![(center, Vec3::ZERO)];

    // Same-map neighbours → the edge their logical offset points along.
    for &n in &within[center as usize] {
        let o = (hexes[n as usize].logical - here.logical).normalize_or_zero();
        let e = (0..6)
            .filter(|&e| !used[e])
            .max_by(|&a, &b| o.dot(edge_normal(a)).total_cmp(&o.dot(edge_normal(b))))
            .unwrap_or(0);
        used[e] = true;
        out.push((n, edge_normal(e) * HEX_SPACING));
    }
    // Across-equator twins → the free edge pointing most *outward* (toward the
    // equator, away from this map's pole). That's the edge you actually walk into
    // when leaving the map, so crossing happens where you'd expect — no walls on
    // the outward edges, only at the genuine pentagon-defect corner.
    let map_center = if here.left { left_center() } else { Vec3::ZERO };
    let outward = (here.logical - map_center).normalize_or_zero();
    for n in Topology::new(rings).equator_partners(center) {
        let e = (0..6)
            .filter(|&e| !used[e])
            .max_by(|&a, &b| edge_normal(a).dot(outward).total_cmp(&edge_normal(b).dot(outward)));
        if let Some(e) = e {
            used[e] = true;
            out.push((n, edge_normal(e) * HEX_SPACING));
        }
    }
    out
}
