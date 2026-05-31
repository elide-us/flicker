//! Shared mesh types — the output contract of any contour algorithm
//! in this crate.
//!
//! [`CellMesh`] bundles a vertex buffer, an index buffer (16-bit or
//! 32-bit), and a [`MeshMetadata`] block describing the mesh's
//! bounds and counts. The name is historical (the original contour
//! algorithm walked 2×2×2 cells); the type itself is a neutral mesh
//! container.

/// One mesh vertex.
///
/// Position is in cluster-local coordinates (each axis in `[0, 256]`).
/// Normal is unit-length and points away from solid voxels. Material is
/// the packed 12/12/8 representation returned by
/// [`Material::raw`](crate::Material::raw).
///
/// The layout is `#[repr(C)]` and plain data — 28 bytes — derived `Pod`
/// and `Zeroable` so a downstream graphics crate can cast a vertex slice
/// to its own GPU vertex type (with the same field layout) without
/// copying. `flicker-render`'s `MeshVertex` is the intended target;
/// compile-time assertions in that crate verify the layouts match.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub material: u32,
}

/// Mesh index buffer; width is chosen automatically based on vertex count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Indices {
    /// Indices fit in 16 bits (vertex count `≤ 65536`).
    U16(Vec<u16>),
    /// Vertex count exceeds 16 bits; 32-bit indices needed.
    U32(Vec<u32>),
}

impl Indices {
    /// Total index count (always a multiple of 3).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Indices::U16(v) => v.len(),
            Indices::U32(v) => v.len(),
        }
    }

    /// `true` if the buffer holds no indices.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Number of triangles.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.len() / 3
    }

    /// `true` iff this buffer uses the compact 16-bit representation.
    #[must_use]
    pub fn is_u16(&self) -> bool {
        matches!(self, Indices::U16(_))
    }
}

/// Descriptive metadata about a contoured mesh.
///
/// Plain data — public fields, no encapsulation. `vertex_count` and
/// `triangle_count` are snapshots taken at construction time and mirror the
/// buffer lengths exactly.
///
/// For an empty mesh, both `bounds_min` and `bounds_max` are
/// `[0.0, 0.0, 0.0]` (the degenerate box).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MeshMetadata {
    /// LOD level the mesh was contoured at.
    pub lod: u32,
    /// Source cluster dimension in voxels (256 in the current pipeline).
    pub cluster_dim: u32,
    /// Vertex count. Mirrors `CellMesh::vertices().len()`.
    pub vertex_count: usize,
    /// Triangle count. Mirrors `CellMesh::indices().triangle_count()`.
    pub triangle_count: usize,
    /// Axis-aligned bounding box minimum in cluster-local coordinates.
    /// For an empty mesh, this is `[0.0, 0.0, 0.0]`.
    pub bounds_min: [f32; 3],
    /// Axis-aligned bounding box maximum in cluster-local coordinates.
    /// For an empty mesh, this is `[0.0, 0.0, 0.0]`.
    pub bounds_max: [f32; 3],
}

/// Output of the contour pass. A neutral CPU-side mesh container —
/// vertex buffer, index buffer, and descriptive metadata.
#[derive(Debug)]
pub struct CellMesh {
    vertices: Vec<Vertex>,
    indices: Indices,
    metadata: MeshMetadata,
}

impl CellMesh {
    /// All vertices in submission order.
    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// The index buffer.
    #[must_use]
    pub fn indices(&self) -> &Indices {
        &self.indices
    }

    /// Descriptive metadata — LOD, dimensions, counts, bounds.
    #[must_use]
    pub fn metadata(&self) -> &MeshMetadata {
        &self.metadata
    }

    /// `true` iff the mesh has no geometry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Construct a [`CellMesh`] from its parts. Crate-internal so
    /// contour algorithms can build the output shape without an
    /// algorithm-specific constructor.
    pub(crate) fn from_parts(
        vertices: Vec<Vertex>,
        indices: Indices,
        metadata: MeshMetadata,
    ) -> Self {
        Self {
            vertices,
            indices,
            metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_zero() -> MeshMetadata {
        MeshMetadata {
            lod: 0,
            cluster_dim: 256,
            vertex_count: 0,
            triangle_count: 0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
        }
    }

    #[test]
    fn indices_len_and_is_empty_round_trip() {
        let u16_empty = Indices::U16(Vec::new());
        assert_eq!(u16_empty.len(), 0);
        assert!(u16_empty.is_empty());
        assert_eq!(u16_empty.triangle_count(), 0);
        assert!(u16_empty.is_u16());

        let u32_one_tri = Indices::U32(vec![0, 1, 2]);
        assert_eq!(u32_one_tri.len(), 3);
        assert!(!u32_one_tri.is_empty());
        assert_eq!(u32_one_tri.triangle_count(), 1);
        assert!(!u32_one_tri.is_u16());
    }

    #[test]
    fn indices_triangle_count_floors_partial_triangle() {
        // Defensive: triangle_count is len/3, so 4 indices still report 1
        // triangle. Algorithms never emit a partial trailing triangle,
        // but the type's accessor remains well-defined.
        let four = Indices::U16(vec![0, 1, 2, 3]);
        assert_eq!(four.triangle_count(), 1);
    }

    #[test]
    fn empty_cell_mesh_accessors() {
        let m = CellMesh::from_parts(Vec::new(), Indices::U16(Vec::new()), meta_zero());
        assert!(m.is_empty());
        assert_eq!(m.vertices().len(), 0);
        assert_eq!(m.indices().triangle_count(), 0);
        assert_eq!(m.metadata().vertex_count, 0);
        assert_eq!(m.metadata().triangle_count, 0);
        assert_eq!(m.metadata().lod, 0);
        assert_eq!(m.metadata().cluster_dim, 256);
    }

    #[test]
    fn cell_mesh_round_trips_parts() {
        let vertex = Vertex {
            position: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
            material: 0xDEAD_BEEF,
        };
        let indices = Indices::U16(vec![0, 0, 0]);
        let metadata = MeshMetadata {
            lod: 2,
            cluster_dim: 256,
            vertex_count: 1,
            triangle_count: 1,
            bounds_min: [1.0, 2.0, 3.0],
            bounds_max: [1.0, 2.0, 3.0],
        };
        let m = CellMesh::from_parts(vec![vertex], indices.clone(), metadata);
        assert_eq!(m.vertices(), &[vertex]);
        assert_eq!(m.indices(), &indices);
        assert_eq!(*m.metadata(), metadata);
        assert!(!m.is_empty());
    }
}
