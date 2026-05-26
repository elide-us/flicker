#![deny(unsafe_code)]
//! `flicker-voxel`: voxel encoding primitives and sparse cluster storage.
//!
//! Coordinate convention is right-handed with Y-up. Local voxel coordinate `(0, 0, 0)` sits
//! at the cluster's minimum corner.

pub mod cluster;
pub mod corner_vector;
pub mod material;
pub mod voxel;

pub use cluster::{Cluster, LocalCoord, CLUSTER_DIMENSION, CLUSTER_VOXEL_COUNT};
pub use corner_vector::CornerVector;
pub use material::Material;
pub use voxel::Voxel;
