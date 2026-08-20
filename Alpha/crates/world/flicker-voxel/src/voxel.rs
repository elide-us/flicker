//! A single voxel — physics state, corner vector, and material identity
//! bundled into the value `Cluster` returns from `get`.
//!
//! State is the dense-per-voxel field (every voxel always has a state, even
//! deep inside a uniform solid mass). Corner and material are the *sparse
//! surface data* — only meaningful at voxels that actually participate in
//! a rendered surface vertex. The cluster's `get` synthesises one of these
//! by reading the dense state field and, when there's a surface override,
//! pulling the corner + material from the sparse map; for solid voxels
//! without an override (the rock mass below the surface), it falls back to
//! the cluster's default material and a default corner.

use crate::corner_vector::CornerVector;
use crate::material::Material;
use crate::voxel_state::VoxelState;

/// One voxel — a 6-inch cube of space — represented by its physics state,
/// owned corner vector, and packed material identity.
///
/// `Voxel::default()` is the empty voxel: `Empty` state, default corner,
/// empty material. The conventional "no voxel here" value.
///
/// Fields are private so the storage layout can evolve without breaking
/// callers; read them via [`Voxel::state`], [`Voxel::corner`], and
/// [`Voxel::material`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Voxel {
    state: VoxelState,
    corner: CornerVector,
    material: Material,
}

impl Voxel {
    /// The empty voxel — `Empty` state, default corner, empty material.
    /// Equivalent to `Voxel::default()`, exposed as a const for use in
    /// const contexts.
    pub const EMPTY: Self = Self {
        state: VoxelState::Empty,
        corner: CornerVector::DEFAULT,
        material: Material::EMPTY,
    };

    /// Convenience constructor.
    #[inline]
    #[must_use]
    pub const fn new(state: VoxelState, corner: CornerVector, material: Material) -> Self {
        Self {
            state,
            corner,
            material,
        }
    }

    /// The physics state — drives the renderer's "is this matter"
    /// predicate and the map-layer water-cycle simulation.
    #[inline]
    #[must_use]
    pub const fn state(self) -> VoxelState {
        self.state
    }

    /// The owned corner vector — the QEF vertex offset, meaningful at
    /// surface-participating voxels.
    #[inline]
    #[must_use]
    pub const fn corner(self) -> CornerVector {
        self.corner
    }

    /// The packed material identity — what kind of matter this is.
    /// Drives renderer shading; ignored by the geometry pipeline.
    #[inline]
    #[must_use]
    pub const fn material(self) -> Material {
        self.material
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let v = Voxel::default();
        assert_eq!(v.state(), VoxelState::Empty);
        assert_eq!(v.corner(), CornerVector::DEFAULT);
        assert_eq!(v.material(), Material::EMPTY);
    }

    #[test]
    fn empty_constant_equals_default() {
        assert_eq!(Voxel::EMPTY, Voxel::default());
    }

    #[test]
    fn fields_round_trip_via_new() {
        let corner = CornerVector::from_bytes([10, 20, 30]);
        let material = Material::new(1, 2, 3);
        let v = Voxel::new(VoxelState::Solid, corner, material);
        assert_eq!(v.state(), VoxelState::Solid);
        assert_eq!(v.corner(), corner);
        assert_eq!(v.material(), material);
    }

    #[test]
    fn equality_requires_all_three_fields() {
        let c1 = CornerVector::from_bytes([1, 2, 3]);
        let c2 = CornerVector::from_bytes([1, 2, 4]);
        let m1 = Material::new(1, 0, 0);
        let m2 = Material::new(2, 0, 0);
        let s = VoxelState::Solid;
        let v = VoxelState::Viscous;
        assert_eq!(Voxel::new(s, c1, m1), Voxel::new(s, c1, m1));
        assert_ne!(Voxel::new(s, c1, m1), Voxel::new(s, c2, m1));
        assert_ne!(Voxel::new(s, c1, m1), Voxel::new(s, c1, m2));
        assert_ne!(Voxel::new(s, c1, m1), Voxel::new(v, c1, m1));
    }
}
