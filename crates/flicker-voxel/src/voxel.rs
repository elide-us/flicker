use crate::{CornerVector, Material};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Voxel {
    corner: CornerVector,
    material: Material,
}

impl Voxel {
    pub const DEFAULT: Self = Self {
        corner: CornerVector::DEFAULT,
        material: Material::EMPTY,
    };

    #[must_use]
    pub const fn new(corner: CornerVector, material: Material) -> Self {
        Self { corner, material }
    }

    #[must_use]
    pub const fn corner(self) -> CornerVector {
        self.corner
    }

    #[must_use]
    pub const fn material(self) -> Material {
        self.material
    }
}

#[cfg(test)]
mod tests {
    use crate::{CornerVector, Material, Voxel};

    #[test]
    fn default_voxel_matches_constants() {
        assert_eq!(Voxel::default(), Voxel::DEFAULT);
        assert_eq!(Voxel::DEFAULT.corner(), CornerVector::DEFAULT);
        assert_eq!(Voxel::DEFAULT.material(), Material::EMPTY);
    }
}
