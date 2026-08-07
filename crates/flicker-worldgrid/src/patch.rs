//! Slice 1 — the pentagon-centered patch.
//!
//! Construction: take the five icosahedron faces that meet at one vertex (a
//! pentagonal cap), geodesically subdivide each to frequency `m`, then take the
//! **dual** ([`crate::mesh`]). An interior vertex has six incident triangles, so
//! its dual cell is a hexagon — except the apex, where only five faces meet,
//! which dualises to the lone **pentagon**. Adjacency is correct by construction
//! and independent of where the points sit — which is why the equal-area ISEA map
//! ([`crate::isea`]) could later replace the placement rule without touching the
//! graph this slice built.

use glam::Vec3;
use std::collections::VecDeque;

use crate::mesh::{dual, subdivide, Weld};

/// A pentagon-centered hex patch: topology only (docs/hex-sphere-handoff.md §4).
///
/// All per-cell fields are indexed by cell id. Cells are ordered by ring outward
/// from the centre, so `center` is `0`. The first four fields are exactly what
/// world-gen's `EpochCtx { dirs, neighbors, .. }` consumes.
#[derive(Clone, Debug)]
pub struct Patch {
    /// Unit-sphere position of each cell (feeds `EpochCtx.dirs`).
    pub dirs: Vec<Vec3>,
    /// Adjacency (feeds `EpochCtx.neighbors`): length 5 for the centre pentagon,
    /// 6 for every interior hex, fewer for the construction-boundary fringe.
    pub neighbors: Vec<Vec<u32>>,
    /// Per-cell area on the unit sphere — equal among interior hexes under the
    /// equal-area map ([`crate::isea`]); the pentagon is genuinely smaller,
    /// because it fans five triangles rather than six.
    pub area: Vec<f32>,
    /// `true` only for the centre cell (the one degree-5 defect).
    pub is_pentagon: Vec<bool>,
    /// `false` for the outer fringe cells whose neighbourhood the cap does not
    /// fully resolve. The interior cells form the actual N-ring disc.
    pub interior: Vec<bool>,
    /// Ring distance from the centre (0 at the pentagon), via the adjacency graph.
    pub ring: Vec<u32>,
    /// The centre pentagon's cell id — always `0` after the ring ordering.
    pub center: u32,
}

impl Patch {
    /// Number of cells in the patch.
    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    /// Whether the patch has no cells (it never is, but clippy asks).
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

/// Build a pentagon-centered patch with `rings` complete interior rings of
/// hexagons around the central pentagon (clamped to at least 1). A construction
/// fringe of partially-resolved boundary cells surrounds the disc; those are
/// flagged `interior == false`.
pub fn pentagon_patch(rings: u32) -> Patch {
    let m = rings.max(1) + 1;

    // The cap: five faces meeting at the +Z apex. The icosahedron's upper ring
    // sits at polar angle arccos(1/√5); consecutive ring vertices are edges, so
    // `(apex, b_k, b_{k+1})` are genuine faces and `b_k–b_{k+1}` is the boundary.
    let apex = Vec3::Z;
    let cos_t = 1.0 / 5f32.sqrt();
    let sin_t = (1.0 - cos_t * cos_t).sqrt();
    let base: Vec<Vec3> = (0..5)
        .map(|k| {
            let phi = std::f32::consts::TAU * k as f32 / 5.0;
            Vec3::new(sin_t * phi.cos(), sin_t * phi.sin(), cos_t)
        })
        .collect();

    let mut weld = Weld::default();
    let mut tris = Vec::new();
    for k in 0..5u8 {
        subdivide(
            apex,
            base[k as usize],
            base[(k as usize + 1) % 5],
            k,
            m,
            &mut weld,
            &mut tris,
        );
    }
    let mesh = weld.into_mesh(tris);
    let d = dual(&mesh);
    order_from_center(mesh.verts, d)
}

/// Ring-distance from the centre pentagon, then relabel cells in `(ring, old id)`
/// order so the centre becomes id `0` and output is deterministic.
fn order_from_center(verts: Vec<Vec3>, d: crate::mesh::Dual) -> Patch {
    let n = verts.len();
    let center = (0..n)
        .find(|&i| d.is_pentagon[i])
        .expect("apex cap must contain exactly one pentagon");

    // BFS ring distance over the full adjacency.
    let mut ring = vec![u32::MAX; n];
    ring[center] = 0;
    let mut q = VecDeque::from([center]);
    while let Some(u) = q.pop_front() {
        for &v in &d.neighbors[u] {
            if ring[v as usize] == u32::MAX {
                ring[v as usize] = ring[u] + 1;
                q.push_back(v as usize);
            }
        }
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| (ring[i], i));
    let mut old_to_new = vec![0u32; n];
    for (new, &old) in order.iter().enumerate() {
        old_to_new[old] = new as u32;
    }

    let pick_bool = |src: &[bool]| -> Vec<bool> { order.iter().map(|&i| src[i]).collect() };
    Patch {
        dirs: order.iter().map(|&i| verts[i]).collect(),
        neighbors: order
            .iter()
            .map(|&i| {
                let mut v: Vec<u32> = d.neighbors[i]
                    .iter()
                    .map(|&u| old_to_new[u as usize])
                    .collect();
                v.sort_unstable();
                v
            })
            .collect(),
        area: order.iter().map(|&i| d.area[i]).collect(),
        is_pentagon: pick_bool(&d.is_pentagon),
        interior: pick_bool(&d.interior),
        ring: order.iter().map(|&i| ring[i]).collect(),
        center: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cells that are interior but not the pentagon — the true hexagons.
    fn interior_hexes(p: &Patch) -> impl Iterator<Item = usize> + '_ {
        (0..p.len()).filter(move |&i| p.interior[i] && !p.is_pentagon[i])
    }

    #[test]
    fn exactly_one_pentagon_at_the_center() {
        let p = pentagon_patch(3);
        let count = p.is_pentagon.iter().filter(|&&b| b).count();
        assert_eq!(count, 1, "a pentagon cap has exactly one defect");
        assert_eq!(p.center, 0);
        assert!(p.is_pentagon[0], "the centre cell is the pentagon");
        assert!(p.interior[0]);
        assert_eq!(p.ring[0], 0);
        assert_eq!(p.neighbors[0].len(), 5, "the pentagon has five neighbours");
    }

    #[test]
    fn interior_hexes_have_six_neighbors() {
        let p = pentagon_patch(3);
        let mut hexes = 0;
        for i in interior_hexes(&p) {
            assert_eq!(
                p.neighbors[i].len(),
                6,
                "interior hex {i} should have 6 neighbours, has {:?}",
                p.neighbors[i]
            );
            hexes += 1;
        }
        assert!(hexes >= 15, "expected a populated interior, got {hexes} hexes");
    }

    #[test]
    fn adjacency_is_symmetric_and_in_range() {
        let p = pentagon_patch(3);
        let n = p.len() as u32;
        for i in 0..p.len() {
            for &j in &p.neighbors[i] {
                assert!(j < n, "neighbour {j} of {i} out of range");
                assert_ne!(j as usize, i, "no self-loops");
                assert!(
                    p.neighbors[j as usize].contains(&(i as u32)),
                    "edge {i}->{j} is not mirrored"
                );
            }
        }
    }

    #[test]
    fn directions_are_unit_length() {
        let p = pentagon_patch(3);
        for d in &p.dirs {
            assert!((d.length() - 1.0).abs() < 1e-4, "dir off the sphere: {d:?}");
        }
    }

    #[test]
    fn areas_are_positive_and_roughly_uniform() {
        let p = pentagon_patch(4);
        let hex_areas: Vec<f32> = interior_hexes(&p).map(|i| p.area[i]).collect();
        assert!(!hex_areas.is_empty());
        let (mut lo, mut hi, mut sum) = (f32::MAX, 0.0f32, 0.0f32);
        for &a in &hex_areas {
            assert!(a > 0.0, "non-positive hex area");
            lo = lo.min(a);
            hi = hi.max(a);
            sum += a;
        }
        let mean = sum / hex_areas.len() as f32;
        // The pentagon is genuinely smaller than a hex (five wedges, not six).
        assert!(p.area[0] > 0.0 && p.area[0] < mean, "pentagon should be smaller");
        // The equal-area map holds these together; the residual is Snyder's
        // vertex-ray crease (see `crate::isea`), not the projection drifting.
        assert!(hi / lo < 1.10, "interior hex area spread too wide: {}", hi / lo);
    }

    #[test]
    fn patch_grows_with_rings_and_stays_single_defect() {
        let small = pentagon_patch(2);
        let big = pentagon_patch(5);
        assert!(big.len() > small.len());
        for p in [&small, &big] {
            assert_eq!(p.is_pentagon.iter().filter(|&&b| b).count(), 1);
            assert_eq!(p.neighbors[0].len(), 5);
        }
    }
}
