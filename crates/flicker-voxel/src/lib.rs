//! flicker-voxel: voxel encoding and cluster storage.
//!
//! This crate is **pure data** — no graphics, no wgpu, no winit, no threads,
//! no persistence. It models the geometric and material identity of a voxel
//! grid and provides sparse storage for a single cluster. Later phases will
//! consume this data to produce meshes, LOD samples, and inter-cluster
//! addressing.
//!
//! # Domain model
//!
//! A **voxel** represents a 6-inch cube of space. Each voxel owns:
//! - a **corner vector** — a 3D offset stored as three bytes, one per axis,
//!   each in the range `[-0.5, 1.5]`. The default `(0.5, 0.5, 0.5)` points
//!   at the voxel's own center. Values outside `[0, 1]` allow a voxel's
//!   owned corner to extend into neighbor space, enabling overlapping
//!   geometry and inverted voxels in later phases.
//! - a **material** — a packed 32-bit identity with three subfields:
//!   12-bit primary index, 12-bit secondary index, 8-bit blend factor.
//!
//! A **cluster** is a 256×256×256 voxel volume — 128 feet on a side at
//! 6-inch voxels. Storage is sparse: a single base voxel value applies to
//! every cell, and only cells that differ from the base are stored
//! explicitly. Most clusters in a real world are mostly uniform (solid
//! rock, air, water), so the sparse representation is a large memory win.
//!
//! # Coordinate convention
//!
//! Right-handed, **Y-up**. Voxel `(0, 0, 0)` sits at the cluster's minimum
//! corner; voxel `(255, 255, 255)` at the maximum corner. This convention
//! is shared across the rest of the engine.

#![forbid(unsafe_code)]

mod cluster;
mod contour;
mod corner_vector;
mod local_coord;
mod material;
mod voxel;

pub use cluster::{Cluster, CLUSTER_DIM, VOXEL_COUNT};
pub use contour::{
    contour_cluster, contour_cluster_lod, contour_cluster_lod_with_neighbors, CellMesh, Indices,
    Lod, MeshMetadata, NeighborContext, Vertex,
};
pub use corner_vector::CornerVector;
pub use local_coord::LocalCoord;
pub use material::Material;
pub use voxel::Voxel;
