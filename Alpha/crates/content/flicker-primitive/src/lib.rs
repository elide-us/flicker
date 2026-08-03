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
//! Alongside the shapes sit the engine's **continuous scalar fields** — the
//! functional truths that anything can sample and agree on without
//! coordinating: [`heightmap`] (the world surface) and [`noise`] (the shared
//! seeded lattice, in 3D for the world-gen kernels and 2D-tileable for the
//! texture synthesizer).
//!
//! This crate depends only on the [`clayengine`] foundation — and the field
//! modules take plain scalars, so they add no math-crate dependency either. It
//! never depends on voxel storage, so storage and primitives stay peers — the
//! storage layer reads the same `CLUSTER_DIM` from `clayengine` that the
//! primitives do, and neither imports the other.

pub mod heightmap;
pub mod noise;
pub mod primitive;

pub use primitive::{
    Cone, Cube, Cylinder, FlatField, HalfCylinder, HalfSphere, HeightField, Hermite, Primitive,
    Scene, Sdf, Sphere,
};
