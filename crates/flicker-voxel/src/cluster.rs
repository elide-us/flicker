//! Sparse cluster storage for local voxels.
//!
//! Coordinates are right-handed with Y-up. `(0, 0, 0)` is the minimum corner of the cluster.
//! Cluster storage is sparse: a single base voxel applies everywhere and only differing cells
//! are stored as overrides.

use std::collections::HashMap;

use crate::Voxel;

pub const CLUSTER_DIMENSION: u16 = 256;
pub const CLUSTER_VOXEL_COUNT: usize =
    CLUSTER_DIMENSION as usize * CLUSTER_DIMENSION as usize * CLUSTER_DIMENSION as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalCoord {
    x: u8,
    y: u8,
    z: u8,
}

impl LocalCoord {
    #[must_use]
    pub fn new(x: u16, y: u16, z: u16) -> Option<Self> {
        if x >= CLUSTER_DIMENSION || y >= CLUSTER_DIMENSION || z >= CLUSTER_DIMENSION {
            return None;
        }

        Some(Self {
            x: x as u8,
            y: y as u8,
            z: z as u8,
        })
    }

    #[must_use]
    pub const fn x(self) -> u8 {
        self.x
    }

    #[must_use]
    pub const fn y(self) -> u8 {
        self.y
    }

    #[must_use]
    pub const fn z(self) -> u8 {
        self.z
    }
}

#[derive(Debug, Default)]
pub struct Cluster {
    base: Voxel,
    // TODO(elideus): Replace HashMap sparse overrides with an octree using small dense leaf
    // chunks (8^3 or 16^3). HashMap is correct for Phase 1 but has poor locality for dense
    // region scans that LOD sampling and contouring will require.
    overrides: HashMap<LocalCoord, Voxel>,
}

impl Cluster {
    #[must_use]
    pub fn empty() -> Self {
        Self::uniform(Voxel::default())
    }

    #[must_use]
    pub fn uniform(base: Voxel) -> Self {
        Self {
            base,
            overrides: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn base(&self) -> Voxel {
        self.base
    }

    #[must_use]
    pub fn get(&self, coord: LocalCoord) -> Voxel {
        self.overrides.get(&coord).copied().unwrap_or(self.base)
    }

    pub fn set(&mut self, coord: LocalCoord, voxel: Voxel) {
        if voxel == self.base {
            self.overrides.remove(&coord);
        } else {
            self.overrides.insert(coord, voxel);
        }
    }

    #[must_use]
    pub fn override_count(&self) -> usize {
        self.overrides.len()
    }

    #[must_use]
    pub fn is_uniform(&self) -> bool {
        self.overrides.is_empty()
    }

    pub fn overrides(&self) -> impl Iterator<Item = (LocalCoord, Voxel)> + '_ {
        self.overrides.iter().map(|(coord, voxel)| (*coord, *voxel))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::{Cluster, CornerVector, LocalCoord, Material, Voxel};

    fn coord(x: u16, y: u16, z: u16) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("valid coord")
    }

    #[test]
    fn empty_cluster_reads_default_everywhere() {
        let cluster = Cluster::empty();
        assert_eq!(cluster.override_count(), 0);
        assert!(cluster.is_uniform());

        for point in [coord(0, 0, 0), coord(255, 255, 255), coord(42, 9, 180)] {
            assert_eq!(cluster.get(point), Voxel::default());
        }
    }

    #[test]
    fn uniform_cluster_reads_custom_base_everywhere() {
        let base = Voxel::new(
            CornerVector::from_components(1.0, 0.25, -0.4),
            Material::new(12, 34, 56).expect("valid material"),
        );
        let cluster = Cluster::uniform(base);

        assert_eq!(cluster.override_count(), 0);
        assert_eq!(cluster.base(), base);
        assert_eq!(cluster.get(coord(1, 2, 3)), base);
        assert_eq!(cluster.get(coord(255, 0, 1)), base);
    }

    #[test]
    fn set_override_adds_and_reads_back() {
        let mut cluster = Cluster::empty();
        let special = Voxel::new(
            CornerVector::from_bytes(0, 128, 255),
            Material::new(7, 42, 128).expect("valid material"),
        );

        let target = coord(10, 20, 30);
        cluster.set(target, special);
        assert_eq!(cluster.override_count(), 1);
        assert_eq!(cluster.get(target), special);
        assert_eq!(cluster.get(coord(11, 20, 30)), Voxel::default());
    }

    #[test]
    fn setting_override_back_to_base_removes_it() {
        let mut cluster = Cluster::empty();
        let target = coord(2, 3, 4);
        let non_base = Voxel::new(
            CornerVector::from_components(-0.5, 1.5, 0.5),
            Material::new(1, 2, 3).expect("valid material"),
        );

        cluster.set(target, non_base);
        assert_eq!(cluster.override_count(), 1);
        cluster.set(target, Voxel::default());

        assert_eq!(cluster.override_count(), 0);
        assert!(cluster.is_uniform());
    }

    #[test]
    fn setting_base_value_does_not_allocate_override() {
        let base = Voxel::new(
            CornerVector::from_bytes(1, 2, 3),
            Material::new(4, 5, 6).expect("valid material"),
        );
        let mut cluster = Cluster::uniform(base);

        cluster.set(coord(99, 88, 77), base);
        assert_eq!(cluster.override_count(), 0);
    }

    #[test]
    fn local_coord_rejects_out_of_range() {
        assert!(LocalCoord::new(256, 0, 0).is_none());
        assert!(LocalCoord::new(0, 256, 0).is_none());
        assert!(LocalCoord::new(0, 0, 256).is_none());
        assert!(LocalCoord::new(255, 255, 255).is_some());
    }

    #[test]
    fn override_iterator_matches_exact_overrides() {
        let mut cluster = Cluster::empty();
        let first = coord(1, 2, 3);
        let second = coord(4, 5, 6);
        let voxel_a = Voxel::new(
            CornerVector::from_bytes(10, 20, 30),
            Material::new(100, 200, 10).expect("valid material"),
        );
        let voxel_b = Voxel::new(
            CornerVector::from_bytes(11, 21, 31),
            Material::new(101, 201, 11).expect("valid material"),
        );
        cluster.set(first, voxel_a);
        cluster.set(second, voxel_b);

        let expected: HashSet<_> = [(first, voxel_a), (second, voxel_b)].into_iter().collect();
        let actual: HashSet<_> = cluster.overrides().collect();

        assert_eq!(actual, expected);
    }

    #[test]
    fn realistic_density_sphere_of_overrides() {
        let mut cluster = Cluster::empty();
        let center = 128_i32;
        let radius = 64_i32;
        let radius_sq = radius * radius;
        let solid = Voxel::new(
            CornerVector::DEFAULT,
            Material::new(3, 9, 255).expect("valid material"),
        );

        for z in 0..256_i32 {
            for y in 0..256_i32 {
                for x in 0..256_i32 {
                    let dx = x - center;
                    let dy = y - center;
                    let dz = z - center;
                    if dx * dx + dy * dy + dz * dz <= radius_sq {
                        cluster.set(coord(x as u16, y as u16, z as u16), solid);
                    }
                }
            }
        }

        let count = cluster.override_count();
        assert!(
            count > 1_000_000,
            "expected dense sphere count, got {count}"
        );
        assert!(
            count < 1_200_000,
            "sphere should not exceed bounds too much, got {count}"
        );
        assert_eq!(cluster.get(coord(128, 128, 128)), solid);
        assert_eq!(cluster.get(coord(64, 128, 128)), solid);
        assert_eq!(cluster.get(coord(10, 10, 10)), Voxel::default());
    }

    #[test]
    fn boundary_coordinates_work_for_overrides() {
        let mut cluster = Cluster::empty();
        let edge = coord(255, 255, 255);
        let voxel = Voxel::new(
            CornerVector::from_components(1.5, -0.5, 1.5),
            Material::new(12, 24, 128).expect("valid material"),
        );

        cluster.set(edge, voxel);
        assert_eq!(cluster.get(edge), voxel);
        assert_eq!(cluster.override_count(), 1);
    }
}
