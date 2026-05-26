//! Single-cluster dual contouring at full resolution (LOD 0).
//!
//! Phase 2 uses a simplified dual contouring variant tailored to this crate's
//! corner-vector voxel encoding:
//! - Emit one vertex per grid point `(x, y, z)` in `1..=255` where the 8
//!   surrounding voxels are mixed solid/empty.
//! - Vertex position is the centroid of those 8 voxels' owned-corner positions.
//! - Emit quads (as two triangles) for sign-changing voxel edges along each axis,
//!   connecting the four dual vertices around that edge.
//!
//! This keeps topology close to dual contouring while avoiding QEF solving for
//! now. Future phases can replace centroid placement with QEF/Hermite fitting
//! (for sharper features) without changing the mesh data model.

use crate::{Cluster, LocalCoord, Material, Voxel, CLUSTER_DIM};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Indices {
    U16(Vec<u16>),
    U32(Vec<u32>),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MeshMetadata {
    pub cluster_dim: u32,
    pub lod_level: u8,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellMesh {
    vertices: Vec<Vertex>,
    indices: Indices,
    metadata: MeshMetadata,
}

impl CellMesh {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Indices::U16(Vec::new()),
            metadata: MeshMetadata {
                cluster_dim: CLUSTER_DIM,
                lod_level: 0,
                vertex_count: 0,
                triangle_count: 0,
                aabb_min: [0.0; 3],
                aabb_max: [0.0; 3],
            },
        }
    }

    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }
    #[must_use]
    pub fn indices(&self) -> &Indices {
        &self.indices
    }
    #[must_use]
    pub fn metadata(&self) -> &MeshMetadata {
        &self.metadata
    }
}

#[must_use]
pub fn contour_cluster(cluster: &Cluster) -> CellMesh {
    if is_uniform_solidness(cluster) {
        return CellMesh::empty();
    }

    let n = CLUSTER_DIM as usize;
    let mut vmap = vec![u32::MAX; n * n * n];
    let mut vertices = Vec::new();
    let mut aabb_min = [f32::INFINITY; 3];
    let mut aabb_max = [f32::NEG_INFINITY; 3];

    for z in 1..CLUSTER_DIM {
        for y in 1..CLUSTER_DIM {
            for x in 1..CLUSTER_DIM {
                let mut cells = [Voxel::EMPTY; 8];
                let mut idx = 0;
                let mut solid = 0;
                for dz in 0..=1 {
                    for dy in 0..=1 {
                        for dx in 0..=1 {
                            let c = coord(x - dx, y - dy, z - dz);
                            let v = cluster.get(c);
                            if is_solid(v.material()) {
                                solid += 1;
                            }
                            cells[idx] = v;
                            idx += 1;
                        }
                    }
                }
                if solid == 0 || solid == 8 {
                    continue;
                }

                let position = centroid_owned_corners(cells, x, y, z);
                let normal = estimate_normal(cluster, x, y, z);
                let material = pick_material(cells, position, x, y, z).raw();
                let vid = vertices.len() as u32;
                vertices.push(Vertex {
                    position,
                    normal,
                    material,
                });
                vmap[grid_i(x, y, z)] = vid;
                for a in 0..3 {
                    aabb_min[a] = aabb_min[a].min(position[a]);
                    aabb_max[a] = aabb_max[a].max(position[a]);
                }
            }
        }
    }

    if vertices.is_empty() {
        return CellMesh::empty();
    }

    let mut idx32 = Vec::new();
    for z in 0..CLUSTER_DIM {
        for y in 0..CLUSTER_DIM {
            for x in 0..CLUSTER_DIM {
                if x + 1 < CLUSTER_DIM {
                    emit_edge_quad(cluster, &vmap, &mut idx32, (x, y, z), (1, 0, 0));
                }
                if y + 1 < CLUSTER_DIM {
                    emit_edge_quad(cluster, &vmap, &mut idx32, (x, y, z), (0, 1, 0));
                }
                if z + 1 < CLUSTER_DIM {
                    emit_edge_quad(cluster, &vmap, &mut idx32, (x, y, z), (0, 0, 1));
                }
            }
        }
    }

    let indices = if vertices.len() <= u16::MAX as usize {
        Indices::U16(idx32.iter().map(|&i| i as u16).collect())
    } else {
        Indices::U32(idx32)
    };
    let triangle_count = match &indices {
        Indices::U16(v) => v.len() / 3,
        Indices::U32(v) => v.len() / 3,
    };

    CellMesh {
        metadata: MeshMetadata {
            cluster_dim: CLUSTER_DIM,
            lod_level: 0,
            vertex_count: vertices.len(),
            triangle_count,
            aabb_min,
            aabb_max,
        },
        vertices,
        indices,
    }
}

fn emit_edge_quad(
    cluster: &Cluster,
    vmap: &[u32],
    out: &mut Vec<u32>,
    p: (u32, u32, u32),
    d: (u32, u32, u32),
) {
    let (x, y, z) = p;
    let (dx, dy, dz) = d;
    let a = cluster.get(coord(x, y, z));
    let b = cluster.get(coord(x + dx, y + dy, z + dz));
    let sa = is_solid(a.material());
    let sb = is_solid(b.material());
    if sa == sb {
        return;
    }

    let verts = if dx == 1 {
        [
            (x + 1, y, z),
            (x + 1, y + 1, z),
            (x + 1, y + 1, z + 1),
            (x + 1, y, z + 1),
        ]
    } else if dy == 1 {
        [
            (x, y + 1, z),
            (x, y + 1, z + 1),
            (x + 1, y + 1, z + 1),
            (x + 1, y + 1, z),
        ]
    } else {
        [
            (x, y, z + 1),
            (x + 1, y, z + 1),
            (x + 1, y + 1, z + 1),
            (x, y + 1, z + 1),
        ]
    };

    let mut quad = [0u32; 4];
    for (i, (vx, vy, vz)) in verts.into_iter().enumerate() {
        if vx == 0
            || vy == 0
            || vz == 0
            || vx >= CLUSTER_DIM
            || vy >= CLUSTER_DIM
            || vz >= CLUSTER_DIM
        {
            return;
        }
        let vi = vmap[grid_i(vx, vy, vz)];
        if vi == u32::MAX {
            return;
        }
        quad[i] = vi;
    }

    let winding = sa && !sb;
    if winding {
        out.extend_from_slice(&[quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
    } else {
        out.extend_from_slice(&[quad[0], quad[2], quad[1], quad[0], quad[3], quad[2]]);
    }
}

fn centroid_owned_corners(cells: [Voxel; 8], x: u32, y: u32, z: u32) -> [f32; 3] {
    let mut p = [0.0; 3];
    let mut i = 0;
    for dz in 0..=1 {
        for dy in 0..=1 {
            for dx in 0..=1 {
                let c = cells[i].corner().to_components();
                p[0] += (x - dx) as f32 + c[0];
                p[1] += (y - dy) as f32 + c[1];
                p[2] += (z - dz) as f32 + c[2];
                i += 1;
            }
        }
    }
    [p[0] / 8.0, p[1] / 8.0, p[2] / 8.0]
}

fn pick_material(cells: [Voxel; 8], position: [f32; 3], x: u32, y: u32, z: u32) -> Material {
    let mut best = Material::EMPTY;
    let mut best_d2 = f32::INFINITY;
    let mut i = 0;
    for dz in 0..=1 {
        for dy in 0..=1 {
            for dx in 0..=1 {
                let v = cells[i];
                if is_solid(v.material()) {
                    let c = v.corner().to_components();
                    let px = (x - dx) as f32 + c[0];
                    let py = (y - dy) as f32 + c[1];
                    let pz = (z - dz) as f32 + c[2];
                    let d2 = (px - position[0]).powi(2)
                        + (py - position[1]).powi(2)
                        + (pz - position[2]).powi(2);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best = v.material();
                    }
                }
                i += 1;
            }
        }
    }
    best
}

fn estimate_normal(cluster: &Cluster, x: u32, y: u32, z: u32) -> [f32; 3] {
    let fx1 = sample_field(cluster, x as i32 + 1, y as i32, z as i32);
    let fx0 = sample_field(cluster, x as i32 - 1, y as i32, z as i32);
    let fy1 = sample_field(cluster, x as i32, y as i32 + 1, z as i32);
    let fy0 = sample_field(cluster, x as i32, y as i32 - 1, z as i32);
    let fz1 = sample_field(cluster, x as i32, y as i32, z as i32 + 1);
    let fz0 = sample_field(cluster, x as i32, y as i32, z as i32 - 1);
    let mut n = [fx0 - fx1, fy0 - fy1, fz0 - fz1];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len > 0.0 {
        n[0] /= len;
        n[1] /= len;
        n[2] /= len;
    } else {
        n = [0.0, 1.0, 0.0];
    }
    n
}

fn sample_field(cluster: &Cluster, x: i32, y: i32, z: i32) -> f32 {
    if x < 0
        || y < 0
        || z < 0
        || x >= CLUSTER_DIM as i32
        || y >= CLUSTER_DIM as i32
        || z >= CLUSTER_DIM as i32
    {
        return 1.0;
    }
    let s = is_solid(cluster.get(coord(x as u32, y as u32, z as u32)).material());
    if s {
        -1.0
    } else {
        1.0
    }
}

fn is_uniform_solidness(cluster: &Cluster) -> bool {
    let base = is_solid(cluster.base().material());
    cluster
        .overrides()
        .all(|(_, v)| is_solid(v.material()) == base)
}

#[inline]
fn is_solid(m: Material) -> bool {
    m != Material::EMPTY
}
fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
    LocalCoord::new(x, y, z).expect("in range")
}
fn grid_i(x: u32, y: u32, z: u32) -> usize {
    (z as usize * CLUSTER_DIM as usize * CLUSTER_DIM as usize)
        + (y as usize * CLUSTER_DIM as usize)
        + x as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CornerVector;
    use std::collections::HashMap;

    fn mat(id: u16) -> Material {
        Material::new(id, 0, 0).unwrap()
    }
    fn set(c: &mut Cluster, x: u32, y: u32, z: u32, m: Material) {
        c.set(coord(x, y, z), Voxel::new(CornerVector::DEFAULT, m));
    }

    fn index_len(mesh: &CellMesh) -> usize {
        match mesh.indices() {
            Indices::U16(v) => v.len(),
            Indices::U32(v) => v.len(),
        }
    }

    #[test]
    fn uniform_clusters_produce_empty_mesh() {
        let m = contour_cluster(&Cluster::empty());
        assert_eq!(m.vertices().len(), 0);
        assert_eq!(index_len(&m), 0);

        let full = Cluster::uniform(Voxel::new(CornerVector::DEFAULT, mat(1)));
        let m2 = contour_cluster(&full);
        assert_eq!(m2.vertices().len(), 0);
        assert_eq!(index_len(&m2), 0);
    }

    #[test]
    fn deterministic_for_same_cluster() {
        let mut c = Cluster::empty();
        set(&mut c, 10, 10, 10, mat(2));
        let a = contour_cluster(&c);
        let b = contour_cluster(&c);
        assert_eq!(a, b);
    }

    #[test]
    fn single_voxel_is_closed() {
        let mut c = Cluster::empty();
        set(&mut c, 64, 64, 64, mat(1));
        let mesh = contour_cluster(&c);
        assert!(mesh.vertices().len() >= 8);
        assert!(index_len(&mesh) >= 36);

        let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
        let idx: Vec<u32> = match mesh.indices() {
            Indices::U16(v) => v.iter().map(|&x| x as u32).collect(),
            Indices::U32(v) => v.clone(),
        };
        for tri in idx.chunks_exact(3) {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let e = if a < b { (a, b) } else { (b, a) };
                *edges.entry(e).or_default() += 1;
            }
        }
        assert!(edges.values().all(|&v| v == 2));
    }

    #[test]
    fn corner_vector_influences_vertex_positions() {
        let mut c1 = Cluster::empty();
        let mut c2 = Cluster::empty();
        set(&mut c1, 30, 30, 30, mat(1));
        c2.set(
            coord(30, 30, 30),
            Voxel::new(CornerVector::from_components(1.5, 0.5, 0.5), mat(1)),
        );
        let a = contour_cluster(&c1);
        let b = contour_cluster(&c2);
        let ax =
            a.vertices().iter().map(|v| v.position[0]).sum::<f32>() / a.vertices().len() as f32;
        let bx =
            b.vertices().iter().map(|v| v.position[0]).sum::<f32>() / b.vertices().len() as f32;
        assert!(bx > ax);
    }
}
