//! Shared mesh primitives: geodesic subdivision of triangles on the unit sphere,
//! welding of coincident points, and dualisation to a hex/pentagon grid. The
//! single-pentagon patch (Slice 1) and the full icosahedral sphere (Slice 3)
//! are both built from these.

use glam::Vec3;
use std::collections::{HashMap, HashSet};

use crate::isea::{edge_point, lattice_point, FaceFrame};

/// Quantisation scale for welding coincident subdivision points (shared edges,
/// shared vertices). Points live on the unit sphere; 1e-6 resolution welds
/// genuine duplicates without merging distinct cells at our frequencies.
const WELD: f64 = 1.0e6;

/// Where a welded vertex came from: the `(face, i, j)` lattice site that first
/// produced it. Faces are subdivided in index order, so a vertex shared between
/// faces keeps the lowest-numbered face — a deterministic canonical home that
/// drives shard assignment and the within-shard ordering key.
pub(crate) type Prov = (u8, u32, u32);

pub(crate) struct TriMesh {
    pub verts: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub prov: Vec<Prov>,
}

/// Welds coincident points into a shared vertex table by quantised position,
/// recording each vertex's first-seen provenance.
#[derive(Default)]
pub(crate) struct Weld {
    map: HashMap<[i64; 3], u32>,
    verts: Vec<Vec3>,
    prov: Vec<Prov>,
}

impl Weld {
    /// Project `p` to the unit sphere and return its (possibly existing) id.
    pub fn intern(&mut self, p: Vec3, prov: Prov) -> u32 {
        let n = p.normalize();
        let key = [
            (n.x as f64 * WELD).round() as i64,
            (n.y as f64 * WELD).round() as i64,
            (n.z as f64 * WELD).round() as i64,
        ];
        if let Some(&i) = self.map.get(&key) {
            return i;
        }
        let i = self.verts.len() as u32;
        self.verts.push(n);
        self.prov.push(prov);
        self.map.insert(key, i);
        i
    }

    pub fn into_mesh(self, tris: Vec<[u32; 3]>) -> TriMesh {
        TriMesh {
            verts: self.verts,
            tris,
            prov: self.prov,
        }
    }
}

/// Geodesic subdivision of triangle `(a, b, c)` to frequency `m`, tagged with
/// `face` for provenance: `m²` small triangles over the barycentric lattice,
/// each site placed by the **equal-area map** ([`crate::isea`]) and interned in
/// the weld table. Point `(i, j)` has barycentric `(i, j, m-i-j)`.
///
/// Sites are routed three ways, because a site's owner decides how it may be
/// placed: a **corner** is the face vertex itself, an **edge** site is shared
/// with the neighbouring face and so is placed from the edge alone
/// ([`crate::isea::edge_point`] — bit-identical from both sides, which is what
/// lets the weld fuse them), and only an **interior** site may use this face's
/// own frame.
pub(crate) fn subdivide(
    a: Vec3,
    b: Vec3,
    c: Vec3,
    face: u8,
    m: u32,
    weld: &mut Weld,
    tris: &mut Vec<[u32; 3]>,
) {
    let frame = FaceFrame::new(a, b, c);
    let mut local: HashMap<(u32, u32), u32> = HashMap::new();
    for i in 0..=m {
        for j in 0..=(m - i) {
            let k = m - i - j;
            let p = match (i == 0, j == 0, k == 0) {
                (false, true, true) => a,
                (true, false, true) => b,
                (true, true, false) => c,
                // `j` sites walk a→b, `k` sites walk a→c and b→c.
                (_, _, true) => edge_point(a, b, j, m),
                (_, true, _) => edge_point(a, c, k, m),
                (true, _, _) => edge_point(b, c, k, m),
                _ => lattice_point(&frame, i, j, k, m),
            };
            local.insert((i, j), weld.intern(p, (face, i, j)));
        }
    }
    let g = |i: u32, j: u32| local[&(i, j)];
    for i in 0..m {
        for j in 0..(m - i) {
            tris.push([g(i, j), g(i + 1, j), g(i, j + 1)]);
            if j < m - i - 1 {
                tris.push([g(i + 1, j), g(i + 1, j + 1), g(i, j + 1)]);
            }
        }
    }
}

pub(crate) fn edge_key(a: u32, b: u32) -> (u32, u32) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

pub(crate) fn tri_area(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    0.5 * (b - a).cross(c - a).length()
}

/// Per-vertex dual data: cells neighbour iff their vertices share an edge; a
/// cell is interior iff every incident edge is shared by two triangles (a closed
/// fan); a pentagon is an interior degree-5 cell. Area is the barycentric dual
/// (a third of each incident triangle).
pub(crate) struct Dual {
    pub neighbors: Vec<Vec<u32>>,
    pub area: Vec<f32>,
    pub is_pentagon: Vec<bool>,
    pub interior: Vec<bool>,
}

pub(crate) fn dual(mesh: &TriMesh) -> Dual {
    let n = mesh.verts.len();
    let mut nbr: Vec<HashSet<u32>> = vec![HashSet::new(); n];
    let mut edge_count: HashMap<(u32, u32), u32> = HashMap::new();
    let mut area = vec![0.0f32; n];

    for tri in &mesh.tris {
        let [a, b, c] = *tri;
        let third = tri_area(
            mesh.verts[a as usize],
            mesh.verts[b as usize],
            mesh.verts[c as usize],
        ) / 3.0;
        area[a as usize] += third;
        area[b as usize] += third;
        area[c as usize] += third;
        for (u, v) in [(a, b), (b, c), (c, a)] {
            nbr[u as usize].insert(v);
            nbr[v as usize].insert(u);
            *edge_count.entry(edge_key(u, v)).or_insert(0) += 1;
        }
    }

    let neighbors: Vec<Vec<u32>> = nbr
        .iter()
        .map(|s| {
            let mut v: Vec<u32> = s.iter().copied().collect();
            v.sort_unstable();
            v
        })
        .collect();
    let interior: Vec<bool> = (0..n)
        .map(|v| {
            neighbors[v]
                .iter()
                .all(|&u| edge_count[&edge_key(v as u32, u)] == 2)
        })
        .collect();
    let is_pentagon: Vec<bool> = (0..n)
        .map(|v| interior[v] && neighbors[v].len() == 5)
        .collect();

    Dual {
        neighbors,
        area,
        is_pentagon,
        interior,
    }
}

/// Ordered corner positions of each cell's dual polygon: the unit-projected
/// centroids of the triangles incident to each vertex, wound cyclically. Empty
/// for any boundary vertex (its fan is open). Used for rendering and, later, the
/// per-cell heightmap parameterisation.
pub(crate) fn cell_corners(mesh: &TriMesh, interior: &[bool]) -> Vec<Vec<Vec3>> {
    let n = mesh.verts.len();
    let mut incident: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (ti, tri) in mesh.tris.iter().enumerate() {
        for &v in tri {
            incident[v as usize].push(ti as u32);
        }
    }

    (0..n)
        .map(|v| {
            if !interior[v] {
                return Vec::new();
            }
            let c = mesh.verts[v];
            // A tangent basis at the cell centre to sort corners by angle.
            let seed = if c.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            let t = c.cross(seed).normalize();
            let b = c.cross(t);
            let mut corners: Vec<(f32, Vec3)> = incident[v]
                .iter()
                .map(|&ti| {
                    let [a, bb, cc] = mesh.tris[ti as usize];
                    let centroid = (mesh.verts[a as usize]
                        + mesh.verts[bb as usize]
                        + mesh.verts[cc as usize])
                        .normalize();
                    let ang = centroid.dot(b).atan2(centroid.dot(t));
                    (ang, centroid)
                })
                .collect();
            corners.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
            corners.into_iter().map(|(_, p)| p).collect()
        })
        .collect()
}

/// The 12 vertices and 20 triangular faces of a unit icosahedron (Kähler's
/// canonical layout). After dualisation the 12 vertices become the 12 pentagons.
pub(crate) fn icosahedron() -> (Vec<Vec3>, Vec<[usize; 3]>) {
    let phi = (1.0 + 5f32.sqrt()) / 2.0;
    let verts: Vec<Vec3> = [
        (-1.0, phi, 0.0),
        (1.0, phi, 0.0),
        (-1.0, -phi, 0.0),
        (1.0, -phi, 0.0),
        (0.0, -1.0, phi),
        (0.0, 1.0, phi),
        (0.0, -1.0, -phi),
        (0.0, 1.0, -phi),
        (phi, 0.0, -1.0),
        (phi, 0.0, 1.0),
        (-phi, 0.0, -1.0),
        (-phi, 0.0, 1.0),
    ]
    .into_iter()
    .map(|(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();

    let faces = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    (verts, faces)
}
