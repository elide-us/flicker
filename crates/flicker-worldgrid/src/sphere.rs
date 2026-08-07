//! Slice 3 — the full icosahedral hex sphere.
//!
//! Subdivide all twenty icosahedron faces to frequency `freq`, weld into one
//! closed geodesic mesh, and dualise ([`crate::mesh`]). The result is the global
//! grid: every cell a hexagon except the **twelve pentagons** on the twelve
//! icosahedron vertices, spread evenly over the sphere (the Goldberg/ISEA
//! connectivity locked in docs/hex-sphere-handoff.md §1).
//!
//! Cells carry their **shard** (one of the 20 triangular faces) and a stable
//! [`CellId`] packing that face with the cell's Morton key within it; cells are
//! emitted in `(shard, Morton)` order for cache locality on the per-key scan.
//!
//! Positions come from the **equal-area ISEA map** ([`crate::isea`], Slice 3b),
//! so cells carry the same area wherever they sit — which is what lets the
//! ledgers store absolute mass and the tectonic conveyor treat one hex step as
//! one distance. Adjacency, shards and ids are independent of the projection and
//! were settled in Slice 3.

use glam::Vec3;

use crate::mesh::{cell_corners, dual, icosahedron, subdivide, Weld};

/// A stable global cell id: `(face << 48) | morton(i, j)` of the cell's canonical
/// lattice site. Unique per cell at a given frequency; the final bit layout is
/// pinned at Slice 4 when it meets the ledger key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellId(pub u64);

impl CellId {
    /// The shard (icosahedron face, 0..20) this id lives on.
    pub fn shard(self) -> u8 {
        (self.0 >> 48) as u8
    }
}

/// The global icosahedral hex grid. The first four fields are exactly what
/// world-gen's `EpochCtx { dirs, neighbors, .. }` consumes; the rest place each
/// cell in its shard and give it a stable id.
#[derive(Clone, Debug)]
pub struct Sphere {
    /// Unit-sphere position of each cell (feeds `EpochCtx.dirs`).
    pub dirs: Vec<Vec3>,
    /// Adjacency (feeds `EpochCtx.neighbors`): 5 for the twelve pentagons, 6 for
    /// every hex. The sphere is closed, so there are no boundary cells.
    pub neighbors: Vec<Vec<u32>>,
    /// Per-cell area on the unit sphere (total ≈ 4π). Equal across hexes to
    /// within a few percent under the equal-area map — pentagons are genuinely
    /// smaller, fanning five triangles rather than six.
    pub area: Vec<f32>,
    /// `true` for exactly the twelve pentagon defects.
    pub is_pentagon: Vec<bool>,
    /// Shard (icosahedron face, 0..20) owning each cell.
    pub shard: Vec<u8>,
    /// Stable global id of each cell (also encodes the shard).
    pub id: Vec<CellId>,
    /// The subdivision frequency this grid was built at.
    pub freq: u32,
}

impl Sphere {
    /// Number of cells (`10·freq² + 2`).
    pub fn len(&self) -> usize {
        self.dirs.len()
    }

    /// Whether the grid has no cells (it never does).
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty()
    }
}

/// Build the global icosahedral hex grid at the given subdivision `freq`
/// (clamped to ≥ 1). `freq` sets the cell size: `10·freq² + 2` cells, always
/// exactly 12 of them pentagons.
pub fn icosphere(freq: u32) -> Sphere {
    build(freq, false).0
}

/// As [`icosphere`], plus each cell's ordered boundary polygon (corner
/// positions) for rendering. Built in one pass so callers needn't rebuild.
pub fn icosphere_with_outlines(freq: u32) -> (Sphere, Vec<Vec<Vec3>>) {
    let (sphere, outlines) = build(freq, true);
    (sphere, outlines)
}

/// Spread the low bits of `x` out to every other bit (for a 2-D Morton code).
fn part1by1(mut x: u64) -> u64 {
    x &= 0xffff_ffff;
    x = (x | (x << 16)) & 0x0000_FFFF_0000_FFFF;
    x = (x | (x << 8)) & 0x00FF_00FF_00FF_00FF;
    x = (x | (x << 4)) & 0x0F0F_0F0F_0F0F_0F0F;
    x = (x | (x << 2)) & 0x3333_3333_3333_3333;
    x = (x | (x << 1)) & 0x5555_5555_5555_5555;
    x
}

/// Z-order (Morton) interleave of a face-local lattice site.
fn morton(i: u32, j: u32) -> u64 {
    part1by1(i as u64) | (part1by1(j as u64) << 1)
}

fn build(freq: u32, want_outlines: bool) -> (Sphere, Vec<Vec<Vec3>>) {
    let m = freq.max(1);
    let (iv, faces) = icosahedron();

    let mut weld = Weld::default();
    let mut tris = Vec::new();
    for (f, face) in faces.iter().enumerate() {
        subdivide(
            iv[face[0]],
            iv[face[1]],
            iv[face[2]],
            f as u8,
            m,
            &mut weld,
            &mut tris,
        );
    }
    let mesh = weld.into_mesh(tris);
    let d = dual(&mesh);
    let outlines_raw = if want_outlines {
        cell_corners(&mesh, &d.interior)
    } else {
        Vec::new()
    };

    let n = mesh.verts.len();
    // Canonical key per cell: shard in the high bits, Morton of the lattice site
    // in the low bits. Sorting by it groups each shard and gives intra-shard
    // locality + a deterministic id.
    let key: Vec<u64> = (0..n)
        .map(|v| {
            let (f, i, j) = mesh.prov[v];
            ((f as u64) << 48) | morton(i, j)
        })
        .collect();

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&v| key[v]);
    let mut old_to_new = vec![0u32; n];
    for (new, &old) in order.iter().enumerate() {
        old_to_new[old] = new as u32;
    }

    let sphere = Sphere {
        dirs: order.iter().map(|&v| mesh.verts[v]).collect(),
        neighbors: order
            .iter()
            .map(|&v| {
                let mut x: Vec<u32> = d.neighbors[v]
                    .iter()
                    .map(|&u| old_to_new[u as usize])
                    .collect();
                x.sort_unstable();
                x
            })
            .collect(),
        area: order.iter().map(|&v| d.area[v]).collect(),
        is_pentagon: order.iter().map(|&v| d.is_pentagon[v]).collect(),
        shard: order.iter().map(|&v| mesh.prov[v].0).collect(),
        id: order.iter().map(|&v| CellId(key[v])).collect(),
        freq: m,
    };
    let outlines = if want_outlines {
        order.iter().map(|&v| outlines_raw[v].clone()).collect()
    } else {
        Vec::new()
    };
    (sphere, outlines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_twelve_pentagons_rest_hexes() {
        for freq in [2, 4, 8] {
            let s = icosphere(freq);
            let pents = s.is_pentagon.iter().filter(|&&b| b).count();
            assert_eq!(pents, 12, "freq {freq}: a hex-sphere has exactly 12 pentagons");
            for i in 0..s.len() {
                let deg = s.neighbors[i].len();
                let want = if s.is_pentagon[i] { 5 } else { 6 };
                assert_eq!(deg, want, "freq {freq}: cell {i} degree {deg}, want {want}");
            }
        }
    }

    #[test]
    fn cell_count_matches_goldberg_formula() {
        for freq in [1, 3, 6, 10] {
            let s = icosphere(freq);
            assert_eq!(s.len(), (10 * freq * freq + 2) as usize, "freq {freq} cell count");
        }
    }

    #[test]
    fn euler_characteristic_holds() {
        // Dual is a closed polyhedron: V - E + F = 2. Here V = cells, each edge
        // shared by two cells, each face a mesh triangle (a cell corner).
        let s = icosphere(6);
        let v = s.len();
        let e: usize = s.neighbors.iter().map(|n| n.len()).sum::<usize>() / 2;
        // F: every degree contributes one triangle corner; faces = 2E/3.
        assert_eq!(2 * e % 3, 0);
        let f = 2 * e / 3;
        assert_eq!(v as i64 - e as i64 + f as i64, 2, "Euler V-E+F");
    }

    #[test]
    fn adjacency_symmetric_and_in_range() {
        let s = icosphere(5);
        let n = s.len() as u32;
        for i in 0..s.len() {
            for &j in &s.neighbors[i] {
                assert!(j < n && j as usize != i);
                assert!(s.neighbors[j as usize].contains(&(i as u32)));
            }
        }
    }

    #[test]
    fn all_twenty_shards_populated_and_ids_unique() {
        let s = icosphere(6);
        let mut seen = std::collections::HashSet::new();
        for id in &s.id {
            assert!(seen.insert(*id), "duplicate cell id {:?}", id);
        }
        let mut counts = [0usize; 20];
        for &sh in &s.shard {
            assert!((sh as usize) < 20);
            counts[sh as usize] += 1;
        }
        assert!(counts.iter().all(|&c| c > 0), "an empty shard: {counts:?}");
        // Cells are emitted grouped by shard (the id is monotonic non-decreasing).
        assert!(s.id.windows(2).all(|w| w[0] <= w[1]), "ids not in scan order");
    }

    #[test]
    fn directions_unit_and_total_area_is_a_sphere() {
        let s = icosphere(8);
        for d in &s.dirs {
            assert!((d.length() - 1.0).abs() < 1e-4);
        }
        let total: f32 = s.area.iter().sum();
        // Flat barycentric-dual areas slightly under-count the curved sphere
        // (4π ≈ 12.566); within ~5% at this frequency.
        assert!((total - 4.0 * std::f32::consts::PI).abs() < 0.7, "total area {total}");
    }

    #[test]
    fn hexes_are_equal_area() {
        // The point of the ISEA slice, measured on the real grid. The cheap
        // projection this replaced spread hex areas ~1.8× and worsened with
        // resolution; the equal-area map holds them within a few percent. What
        // remains is Snyder's own vertex-ray crease (see `crate::isea`), which
        // makes the cells sitting on a crease slightly small.
        for freq in [8, 16] {
            let s = icosphere(freq);
            let (mut lo, mut hi) = (f32::MAX, 0.0f32);
            for (i, &a) in s.area.iter().enumerate() {
                if s.is_pentagon[i] {
                    continue; // a pentagon is intrinsically smaller — 5 fans, not 6
                }
                lo = lo.min(a);
                hi = hi.max(a);
            }
            assert!(hi / lo < 1.06, "freq {freq}: hex area spread {}", hi / lo);
        }
    }

    #[test]
    fn outlines_cover_every_cell_and_close_the_ring() {
        let (s, outlines) = icosphere_with_outlines(4);
        assert_eq!(outlines.len(), s.len());
        for (i, outline) in outlines.iter().enumerate() {
            let want = if s.is_pentagon[i] { 5 } else { 6 };
            assert_eq!(outline.len(), want, "cell {i} corner count");
            for c in outline {
                assert!((c.length() - 1.0).abs() < 1e-4, "corner off the sphere");
            }
        }
    }
}

