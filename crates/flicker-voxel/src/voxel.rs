//! A single voxel: corner vector plus material.

use crate::corner_vector::CornerVector;
use crate::material::Material;

/// One voxel — a 6-inch cube of space — represented by its owned corner
/// vector and its packed material identity.
///
/// `Voxel::default()` is the empty voxel: default corner ([`CornerVector::DEFAULT`])
/// and empty material ([`Material::EMPTY`]). It is the conventional "no
/// voxel here" value and the base of an empty cluster.
///
/// The fields are kept private so the storage layout can evolve without
/// breaking callers; read them via [`Voxel::corner`] and [`Voxel::material`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Voxel {
    corner: CornerVector,
    material: Material,
}

impl Voxel {
    /// The empty voxel — default corner, empty material. Equivalent to
    /// `Voxel::default()`, exposed as a const for use in const contexts.
    pub const EMPTY: Self = Self {
        corner: CornerVector::DEFAULT,
        material: Material::EMPTY,
    };

    /// Convenience constructor.
    #[inline]
    #[must_use]
    pub const fn new(corner: CornerVector, material: Material) -> Self {
        Self { corner, material }
    }

    /// The owned corner vector.
    #[inline]
    #[must_use]
    pub const fn corner(self) -> CornerVector {
        self.corner
    }

    /// The packed material identity.
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
        let material = Material::new(1, 2, 3).unwrap();
        let v = Voxel::new(corner, material);
        assert_eq!(v.corner(), corner);
        assert_eq!(v.material(), material);
    }

    #[test]
    fn equality_requires_both_fields() {
        let c1 = CornerVector::from_bytes([1, 2, 3]);
        let c2 = CornerVector::from_bytes([1, 2, 4]);
        let m1 = Material::new(1, 0, 0).unwrap();
        let m2 = Material::new(2, 0, 0).unwrap();
        assert_eq!(Voxel::new(c1, m1), Voxel::new(c1, m1));
        assert_ne!(Voxel::new(c1, m1), Voxel::new(c2, m1));
        assert_ne!(Voxel::new(c1, m1), Voxel::new(c1, m2));
    }
}
