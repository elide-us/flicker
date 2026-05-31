//! Stub importable primitives — fake surfaces we contour with QEF
//! before a real primitive importer/parser exists.
//!
//! A "primitive" is a *source*: something the contour pass queries for
//! the surface, distinct from the baked voxel data the contour
//! produces. The pipeline is: **primitive → contour to voxel (QEF) →
//! cluster from voxel data.** This module fakes the simplest primitive
//! — a flat field — so the QEF contour has a trivial, known-correct
//! input: a horizontal plane at the cluster's half-height that should
//! contour to a flat surface with every vertex's normal pointing
//! straight up.

use crate::cluster::CLUSTER_DIM;

/// Surface height of the flat stub field, in voxel units: the cluster's
/// vertical midpoint (`CLUSTER_DIM / 2` = 128 for a 256³ cluster), i.e.
/// normalized height 0.5.
pub const FLAT_HEIGHT: f32 = CLUSTER_DIM as f32 / 2.0;

/// Faked surface query for the flat "solid grey field" primitive.
///
/// Returns the same height everywhere, so contouring it yields a flat
/// horizontal plane at [`FLAT_HEIGHT`]. The implied surface normal is
/// `(0, 1, 0)` (the gradient of a constant field is zero). The material
/// to assign when contouring is a uniform grey — there is no slope or
/// material variation to express.
#[must_use]
pub fn flat_surface_height(_world_x: f32, _world_z: f32) -> f32 {
    FLAT_HEIGHT
}
