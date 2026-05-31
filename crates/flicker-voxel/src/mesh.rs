//! Per-cell dual contouring: turn a contoured [`Cluster`] into a flat-
//! shaded mesh.
//!
//! Pipeline stage: **[contour to voxel](crate::contour) → cluster → mesh**.
//! For every axis-aligned grid edge whose two endpoints differ in solidity,
//! the four cells sharing that edge contribute one quad. Each cell's vertex
//! is the position carried by its **min-corner voxel** — the owned-vertex
//! formula `p(x,y,z) = [x, y, z] + voxel.corner().to_components()`. Vertices
//! are duplicated per quad (four fresh ones each, all carrying the oriented
//! face normal) so the result is flat-shaded with crisp facets.
//!
//! Solidity is `voxel.material() != Material::EMPTY`. Coordinates outside
//! `[0, CLUSTER_DIM)` are treated as empty — there is no neighbor cluster
//! yet, so cluster-border faces are intentionally left open.
//!
//! No graphics dependency: the output [`ClusterVertex`] is field-for-field
//! convertible to `flicker-render`'s `MeshVertex`, but mapping happens in
//! the consumer (see `examples/voxel-cluster`).
//!
//! Scope: single cluster, LOD 0, flat-field-correct (see
//! `docs/voxel-mesh-regen.md` §9 for the per-cell-vs-per-solid-voxel
//! divergence that must be resolved before curved primitives go in).

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::local_coord::LocalCoord;
use crate::material::Material;

/// Number of cells along each axis: one fewer than voxels because a cell
/// spans two voxels per axis.
const CELL_DIM: i32 = CLUSTER_DIM as i32 - 1;

/// One flat-shaded mesh vertex in cluster-local voxel units. Field-for-
/// field convertible to `flicker-render`'s `MeshVertex` — the example does
/// the mapping so this crate stays graphics-free.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClusterVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// CPU mesh: per-quad-duplicated vertices plus a `u32` triangle-index list.
#[derive(Clone, Debug, Default)]
pub struct ClusterMesh {
    pub vertices: Vec<ClusterVertex>,
    pub indices: Vec<u32>,
}

impl ClusterMesh {
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    #[inline]
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Regenerate a renderable mesh from a contoured cluster via per-cell dual
/// contouring (see module docs).
#[must_use]
pub fn mesh(cluster: &Cluster) -> ClusterMesh {
    let mut out = ClusterMesh::default();
    let dim = CLUSTER_DIM as i32;

    for gz in 0..dim {
        for gy in 0..dim {
            for gx in 0..dim {
                let g = [gx, gy, gz];
                let s_g = is_solid(cluster, g);

                for axis_a in 0..3 {
                    let mut n = g;
                    n[axis_a] += 1;
                    if n[axis_a] >= dim {
                        continue;
                    }
                    let s_n = is_solid_in_range(cluster, n);
                    if s_g == s_n {
                        continue;
                    }

                    // Four cells sharing the edge, in perimeter order so
                    // the (v1-v0)×(v2-v0) cross product points +e_a.
                    let cells = [
                        cell_coord(g, axis_a, 0, 0),
                        cell_coord(g, axis_a, -1, 0),
                        cell_coord(g, axis_a, -1, -1),
                        cell_coord(g, axis_a, 0, -1),
                    ];
                    if cells.iter().any(|c| !cell_in_range(*c)) {
                        continue;
                    }

                    let mut expected_normal = [0.0_f32; 3];
                    expected_normal[axis_a] = if s_g { 1.0 } else { -1.0 };

                    let solid_endpoint = if s_g { g } else { n };
                    let solid_coord = LocalCoord::new(
                        solid_endpoint[0] as u32,
                        solid_endpoint[1] as u32,
                        solid_endpoint[2] as u32,
                    )
                    .expect("solid endpoint is in voxel range");
                    let material = cluster.get(solid_coord).material().raw();

                    let positions = [
                        cell_vertex(cluster, cells[0]),
                        cell_vertex(cluster, cells[1]),
                        cell_vertex(cluster, cells[2]),
                        cell_vertex(cluster, cells[3]),
                    ];

                    push_quad(&mut out, positions, expected_normal, material);
                }
            }
        }
    }

    out
}

/// Append one oriented quad (4 vertices, 6 indices). The quad's face
/// normal is the geometric normal of triangle `(v0,v1,v2)`; if it points
/// opposite to `expected_normal`, the vertex order is reversed so the
/// emitted face faces the solid→empty direction.
fn push_quad(
    out: &mut ClusterMesh,
    positions: [[f32; 3]; 4],
    expected_normal: [f32; 3],
    material: u32,
) {
    let v0 = positions[0];
    let v1 = positions[1];
    let v2 = positions[2];
    let v3 = positions[3];

    let raw_normal = cross(sub(v1, v0), sub(v2, v0));
    let mut face_normal = normalize(raw_normal);

    let ordered = if dot(face_normal, expected_normal) < 0.0 {
        face_normal = [-face_normal[0], -face_normal[1], -face_normal[2]];
        // Keep v0 in place, reverse the rest: both triangles flip sign.
        [v0, v3, v2, v1]
    } else {
        [v0, v1, v2, v3]
    };

    let base = out.vertices.len() as u32;
    for p in ordered {
        out.vertices.push(ClusterVertex {
            position: p,
            normal: face_normal,
            material,
        });
    }
    out.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// Min-corner coord of one of the four cells around an edge along
/// `axis_a` at grid point `g`. `db`, `dc` are signed offsets in the two
/// non-`a` axes, which are taken cyclically: `b = (a+1) % 3`,
/// `c = (a+2) % 3`. The cyclic choice makes `e_b × e_c = e_a`, which is
/// what gives the quad a `+e_a` geometric normal in perimeter order.
fn cell_coord(g: [i32; 3], axis_a: usize, db: i32, dc: i32) -> [i32; 3] {
    let b = (axis_a + 1) % 3;
    let c = (axis_a + 2) % 3;
    let mut out = g;
    out[b] += db;
    out[c] += dc;
    out
}

#[inline]
fn cell_in_range(cell: [i32; 3]) -> bool {
    cell[0] >= 0
        && cell[0] < CELL_DIM
        && cell[1] >= 0
        && cell[1] < CELL_DIM
        && cell[2] >= 0
        && cell[2] < CELL_DIM
}

/// Owned-vertex formula: cell min-corner voxel's stored corner offset,
/// added to that voxel's integer origin. Caller must have verified the
/// cell is in range.
fn cell_vertex(cluster: &Cluster, cell: [i32; 3]) -> [f32; 3] {
    let coord = LocalCoord::new(cell[0] as u32, cell[1] as u32, cell[2] as u32)
        .expect("cell coord in range");
    let voxel = cluster.get(coord);
    let [dx, dy, dz] = voxel.corner().to_components();
    [
        cell[0] as f32 + dx,
        cell[1] as f32 + dy,
        cell[2] as f32 + dz,
    ]
}

fn is_solid(cluster: &Cluster, g: [i32; 3]) -> bool {
    let dim = CLUSTER_DIM as i32;
    if g[0] < 0 || g[0] >= dim || g[1] < 0 || g[1] >= dim || g[2] < 0 || g[2] >= dim {
        return false;
    }
    is_solid_in_range(cluster, g)
}

#[inline]
fn is_solid_in_range(cluster: &Cluster, g: [i32; 3]) -> bool {
    let coord = LocalCoord::new(g[0] as u32, g[1] as u32, g[2] as u32)
        .expect("caller verified range");
    cluster.get(coord).material() != Material::EMPTY
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if l2 > 1e-20 {
        let inv = 1.0 / l2.sqrt();
        [v[0] * inv, v[1] * inv, v[2] * inv]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contour::contour;
    use crate::corner_vector::CornerVector;
    use crate::primitive::{Hermite, Primitive};
    use crate::voxel::Voxel;

    fn grey() -> Material {
        Material::new(1, 1, 0).expect("valid")
    }

    /// A small solid cube `[0, n)³` — same shape used in the contour
    /// tests, copied here to avoid an `pub(crate)` carve-out just for one
    /// test. Cheap enough to mesh without the full 16M-voxel fill.
    struct CubeField {
        n: i32,
    }

    impl Primitive for CubeField {
        fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
            (0..self.n).contains(&x) && (0..self.n).contains(&y) && (0..self.n).contains(&z)
        }

        fn edge_hermite(&self, a: [i32; 3], b: [i32; 3]) -> Hermite {
            let ca = [a[0] as f32 + 0.5, a[1] as f32 + 0.5, a[2] as f32 + 0.5];
            let cb = [b[0] as f32 + 0.5, b[1] as f32 + 0.5, b[2] as f32 + 0.5];
            Hermite {
                position: [
                    (ca[0] + cb[0]) * 0.5,
                    (ca[1] + cb[1]) * 0.5,
                    (ca[2] + cb[2]) * 0.5,
                ],
                normal: [cb[0] - ca[0], cb[1] - ca[1], cb[2] - ca[2]],
            }
        }
    }

    /// Geometric (un-normalized) normal of triangle `(p0, p1, p2)`.
    fn tri_normal(p0: [f32; 3], p1: [f32; 3], p2: [f32; 3]) -> [f32; 3] {
        cross(sub(p1, p0), sub(p2, p0))
    }

    #[test]
    fn push_quad_orients_against_expected_normal() {
        // Square in the y=0 plane, vertex order chosen so (v1-v0)×(v2-v0)
        // already points +Y.
        let positions = [
            [0.0_f32, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        ];

        let mut m = ClusterMesh::default();
        push_quad(&mut m, positions, [0.0, 1.0, 0.0], 42);

        assert_eq!(m.vertices.len(), 4);
        assert_eq!(m.indices.len(), 6);
        for v in &m.vertices {
            assert!((v.normal[0]).abs() < 1e-5);
            assert!((v.normal[1] - 1.0).abs() < 1e-5);
            assert!((v.normal[2]).abs() < 1e-5);
            assert_eq!(v.material, 42);
        }
        for tri in m.indices.chunks(3) {
            let p0 = m.vertices[tri[0] as usize].position;
            let p1 = m.vertices[tri[1] as usize].position;
            let p2 = m.vertices[tri[2] as usize].position;
            let n = tri_normal(p0, p1, p2);
            assert!(n[1] > 0.0, "triangle {:?} not facing +Y: {:?}", tri, n);
        }
    }

    #[test]
    fn push_quad_reverses_when_expected_normal_flipped() {
        let positions = [
            [0.0_f32, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        ];

        let mut m = ClusterMesh::default();
        push_quad(&mut m, positions, [0.0, -1.0, 0.0], 7);

        for v in &m.vertices {
            assert!((v.normal[1] + 1.0).abs() < 1e-5);
        }
        for tri in m.indices.chunks(3) {
            let p0 = m.vertices[tri[0] as usize].position;
            let p1 = m.vertices[tri[1] as usize].position;
            let p2 = m.vertices[tri[2] as usize].position;
            let n = tri_normal(p0, p1, p2);
            assert!(n[1] < 0.0, "triangle {:?} not facing -Y: {:?}", tri, n);
        }
    }

    #[test]
    fn small_cube_produces_sane_mesh() {
        let c = contour(&CubeField { n: 3 }, grey());
        let m = mesh(&c);

        assert!(!m.is_empty(), "cube should produce some quads");
        assert_eq!(m.vertices.len() % 4, 0);
        assert_eq!(m.indices.len() % 6, 0);
        for i in &m.indices {
            assert!((*i as usize) < m.vertices.len());
        }
        for v in &m.vertices {
            let len = (v.normal[0] * v.normal[0]
                + v.normal[1] * v.normal[1]
                + v.normal[2] * v.normal[2])
                .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-3,
                "normal not unit-length: {:?}",
                v.normal
            );
        }
    }

    #[test]
    fn flat_patch_emits_quad_on_the_plane_facing_up() {
        // 2×2 surface-voxel block at y=127 with the QEF-style corner
        // (0.5, 1.0, 0.5) — the contour's result for a half-height flat
        // field, replicated cheaply. The +Y edge at the patch's interior
        // grid point (2, 127, 2) has all four cells inside the block, so
        // every vertex of that quad lies on y=128.
        let mut cluster = Cluster::empty();
        let mat = grey();
        let corner = CornerVector::from_components(0.5, 1.0, 0.5);
        for x in 1..=2u32 {
            for z in 1..=2u32 {
                let c = LocalCoord::new(x, 127, z).expect("in range");
                cluster.set(c, Voxel::new(corner, mat));
            }
        }

        let m = mesh(&cluster);
        assert!(!m.is_empty());

        let mut found = false;
        for quad in m.vertices.chunks_exact(4) {
            let on_plane = quad.iter().all(|v| (v.position[1] - 128.0).abs() < 0.01);
            let faces_up = quad.iter().all(|v| {
                v.normal[0].abs() < 0.01
                    && (v.normal[1] - 1.0).abs() < 0.01
                    && v.normal[2].abs() < 0.01
            });
            if on_plane && faces_up {
                found = true;
                break;
            }
        }
        assert!(found, "no +Y face quad at y=128 in patch");
    }
}
