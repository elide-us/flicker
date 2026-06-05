//! flicker-primitive — the primitive (stampable shape) layer.
//!
//! A **primitive** is an implicit shape (an SDF / solidity oracle) sampled at
//! world coordinates. Primitives serve two consumers:
//!
//! - **World generation** — the contour pipeline turns a primitive into a
//!   cluster's voxel data (`primitive → contour → cluster data`).
//! - **The editor** — the player stamps a transformed primitive, additively
//!   or subtractively, into existing voxel data. (Transforms and the
//!   stamping CSG ops will land here as sibling modules; not built yet.)
//!
//! This crate depends only on the [`clayengine`] foundation. It never
//! depends on voxel storage, so storage and primitives stay peers — the
//! storage layer reads the same `CLUSTER_DIM` from `clayengine` that the
//! primitives do, and neither imports the other.

pub mod heightmap;
pub mod primitive;

pub use primitive::{
    Cone, Cube, Cylinder, FlatField, HalfCylinder, HalfSphere, HeightField, Hermite, Primitive,
    Scene, Sdf, Sphere,
};
