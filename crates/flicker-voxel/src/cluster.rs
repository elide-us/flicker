//! Sparse 256³ voxel cluster.
//!
//! A [`Cluster`] stores a single 256×256×256 volume of voxels — 16,777,216
//! cells. Most real-world clusters are dominated by a single value (solid
//! rock, air, water), so storage is sparse: every cell starts as the
//! cluster's `base` voxel, and only cells that differ from `base` are
//! stored explicitly in a `HashMap` keyed by [`LocalCoord`].
//!
//! Reads are infallible — any in-range coordinate either resolves to its
//! recorded override or falls back to `base`. Writes that match the
//! current `base` value transparently remove the override, so `set` then
//! `set` again with the base reclaims memory.
//!
//! [`Cluster`] deliberately does **not** implement `Clone`. A cluster can
//! hold millions of overrides; accidental clones would be a real bug.
//! Later phases can add `Clone` if a concrete need surfaces.

use std::collections::hash_map;
use std::collections::HashMap;
use std::fmt;

use crate::local_coord::LocalCoord;
use crate::voxel::Voxel;

/// Side length of a cluster in voxels.
pub const CLUSTER_DIM: u32 = 256;

/// Total voxel count in a cluster (`CLUSTER_DIM³`).
pub const VOXEL_COUNT: usize = (CLUSTER_DIM as usize).pow(3);

/// A 256³ voxel volume with a base value and a sparse set of overrides.
pub struct Cluster {
    base: Voxel,
    // TODO(phase 3 or 5): replace the HashMap with an octree backed by
    // small dense leaf chunks (8³ or 16³). The HashMap is correct but has
    // poor locality for the dense-region scans that LOD sampling and dual
    // contouring will need; an octree with dense leaves gives both
    // sparse-region memory savings and dense-region cache friendliness.
    overrides: HashMap<LocalCoord, Voxel>,
}

impl Cluster {
    /// A cluster whose every cell reads as [`Voxel::default`] until
    /// explicitly overridden.
    #[must_use]
    pub fn empty() -> Self {
        Self::uniform(Voxel::default())
    }

    /// A cluster whose every cell reads as `base` until explicitly
    /// overridden. The `base` may itself have a non-default corner vector.
    #[must_use]
    pub fn uniform(base: Voxel) -> Self {
        Self {
            base,
            overrides: HashMap::new(),
        }
    }

    /// The base voxel — the value returned at any coordinate that has no
    /// explicit override.
    #[inline]
    #[must_use]
    pub fn base(&self) -> Voxel {
        self.base
    }

    /// Read the voxel at `coord`. Always succeeds: returns the override
    /// if one exists, otherwise the base.
    #[inline]
    #[must_use]
    pub fn get(&self, coord: LocalCoord) -> Voxel {
        self.overrides.get(&coord).copied().unwrap_or(self.base)
    }

    /// Write `voxel` at `coord`.
    ///
    /// If `voxel` equals the current base, any existing override at
    /// `coord` is removed (so set-then-unset reclaims memory). Otherwise
    /// the override is inserted or updated.
    pub fn set(&mut self, coord: LocalCoord, voxel: Voxel) {
        if voxel == self.base {
            self.overrides.remove(&coord);
        } else {
            self.overrides.insert(coord, voxel);
        }
    }

    /// Number of cells that currently differ from the base.
    #[inline]
    #[must_use]
    pub fn override_count(&self) -> usize {
        self.overrides.len()
    }

    /// `true` iff no cell differs from the base (the cluster is purely
    /// described by `base`).
    #[inline]
    #[must_use]
    pub fn is_uniform(&self) -> bool {
        self.overrides.is_empty()
    }

    /// Iterate every override as `(coordinate, voxel)`. Order is
    /// unspecified (the underlying `HashMap` does not preserve any).
    #[must_use]
    pub fn overrides(&self) -> Overrides<'_> {
        Overrides {
            inner: self.overrides.iter(),
        }
    }
}

impl fmt::Debug for Cluster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Don't dump every override (could be millions). Summarize instead.
        f.debug_struct("Cluster")
            .field("base", &self.base)
            .field("override_count", &self.overrides.len())
            .finish()
    }
}

/// Iterator over a cluster's overrides. See [`Cluster::overrides`].
pub struct Overrides<'a> {
    inner: hash_map::Iter<'a, LocalCoord, Voxel>,
}

impl Iterator for Overrides<'_> {
    type Item = (LocalCoord, Voxel);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| (*k, *v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Overrides<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_vector::CornerVector;
    use crate::material::Material;

    fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("in-range")
    }

    fn nonempty_material() -> Material {
        Material::new(7, 11, 200).unwrap()
    }

    #[test]
    fn cluster_dim_and_voxel_count_match_spec() {
        assert_eq!(CLUSTER_DIM, 256);
        assert_eq!(VOXEL_COUNT, 256 * 256 * 256);
    }

    #[test]
    fn empty_cluster_reads_default_everywhere() {
        let c = Cluster::empty();
        assert_eq!(c.base(), Voxel::default());
        assert_eq!(c.override_count(), 0);
        assert!(c.is_uniform());
        for &(x, y, z) in &[
            (0u32, 0u32, 0u32),
            (1, 1, 1),
            (128, 128, 128),
            (255, 255, 255),
            (0, 255, 0),
        ] {
            assert_eq!(c.get(coord(x, y, z)), Voxel::default());
        }
    }

    #[test]
    fn uniform_cluster_with_custom_base_reads_base_everywhere() {
        let base = Voxel::new(CornerVector::from_bytes([10, 20, 30]), nonempty_material());
        let c = Cluster::uniform(base);
        assert_eq!(c.base(), base);
        assert!(c.is_uniform());
        assert_eq!(c.override_count(), 0);
        for &(x, y, z) in &[(0u32, 0u32, 0u32), (42, 137, 200), (255, 255, 255)] {
            assert_eq!(c.get(coord(x, y, z)), base);
        }
    }

    #[test]
    fn single_override_increments_count_and_is_readable() {
        let mut c = Cluster::empty();
        let v = Voxel::new(CornerVector::DEFAULT, nonempty_material());
        let where_ = coord(5, 6, 7);
        c.set(where_, v);
        assert_eq!(c.override_count(), 1);
        assert!(!c.is_uniform());
        assert_eq!(c.get(where_), v);
        // Neighbors still read base.
        assert_eq!(c.get(coord(4, 6, 7)), Voxel::default());
        assert_eq!(c.get(coord(5, 7, 7)), Voxel::default());
        assert_eq!(c.get(coord(5, 6, 8)), Voxel::default());
    }

    #[test]
    fn override_back_to_base_removes_it() {
        let mut c = Cluster::empty();
        let v = Voxel::new(CornerVector::DEFAULT, nonempty_material());
        let where_ = coord(10, 20, 30);
        c.set(where_, v);
        assert_eq!(c.override_count(), 1);
        c.set(where_, Voxel::default());
        assert_eq!(c.override_count(), 0);
        assert!(c.is_uniform());
        assert_eq!(c.get(where_), Voxel::default());
    }

    #[test]
    fn setting_base_on_empty_cluster_does_not_allocate_override() {
        let mut c = Cluster::empty();
        // Writing the same value as base across many cells must stay sparse.
        for i in 0..100 {
            c.set(coord(i, 0, 0), Voxel::default());
        }
        assert_eq!(c.override_count(), 0);
        assert!(c.is_uniform());
    }

    #[test]
    fn setting_base_on_uniform_with_nondefault_base_stays_sparse() {
        let base = Voxel::new(CornerVector::from_bytes([42, 0, 0]), nonempty_material());
        let mut c = Cluster::uniform(base);
        for i in 0..50 {
            c.set(coord(i, i, i), base);
        }
        assert_eq!(c.override_count(), 0);
    }

    #[test]
    fn updating_an_existing_override_does_not_change_count() {
        let mut c = Cluster::empty();
        let v1 = Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap());
        let v2 = Voxel::new(CornerVector::DEFAULT, Material::new(2, 0, 0).unwrap());
        let where_ = coord(100, 100, 100);
        c.set(where_, v1);
        c.set(where_, v2);
        assert_eq!(c.override_count(), 1);
        assert_eq!(c.get(where_), v2);
    }

    #[test]
    fn overrides_iterator_visits_exactly_the_overrides() {
        let mut c = Cluster::empty();
        let v = Voxel::new(CornerVector::DEFAULT, nonempty_material());
        let pts = [coord(0, 0, 0), coord(1, 2, 3), coord(255, 255, 255)];
        for p in pts {
            c.set(p, v);
        }
        let collected: std::collections::HashMap<_, _> = c.overrides().collect();
        assert_eq!(collected.len(), 3);
        for p in pts {
            assert_eq!(collected.get(&p), Some(&v));
        }
    }

    #[test]
    fn cluster_boundary_coords_are_valid() {
        let mut c = Cluster::empty();
        let v = Voxel::new(CornerVector::DEFAULT, nonempty_material());
        let corners = [
            coord(0, 0, 0),
            coord(255, 0, 0),
            coord(0, 255, 0),
            coord(0, 0, 255),
            coord(255, 255, 0),
            coord(255, 0, 255),
            coord(0, 255, 255),
            coord(255, 255, 255),
        ];
        for p in corners {
            c.set(p, v);
        }
        assert_eq!(c.override_count(), corners.len());
        for p in corners {
            assert_eq!(c.get(p), v);
        }
    }

    #[test]
    fn sphere_of_overrides_realistic_density() {
        // Fill a sphere of radius 64 around the cluster center (128, 128, 128).
        // Exercises the sparse store under ~1.1M overrides — realistic memory
        // pressure rather than a toy case.
        let mut c = Cluster::empty();
        let v = Voxel::new(CornerVector::DEFAULT, nonempty_material());

        let center = 128i32;
        let r2 = 64i32 * 64i32;
        let mut expected = 0usize;

        for z in 0..256i32 {
            let dz = z - center;
            for y in 0..256i32 {
                let dy = y - center;
                for x in 0..256i32 {
                    let dx = x - center;
                    if dx * dx + dy * dy + dz * dz < r2 {
                        c.set(coord(x as u32, y as u32, z as u32), v);
                        expected += 1;
                    }
                }
            }
        }

        // Analytic sphere volume is (4/3)π r³ ≈ 1,098,066. The discrete
        // count differs slightly; check a generous band.
        assert_eq!(c.override_count(), expected);
        assert!(
            (1_050_000..1_150_000).contains(&expected),
            "expected ~1.1M voxels in r=64 sphere, got {expected}"
        );

        // Spot checks.
        assert_eq!(c.get(coord(128, 128, 128)), v); // center — inside
        assert_eq!(c.get(coord(0, 0, 0)), Voxel::default()); // far corner — base
        assert_eq!(c.get(coord(191, 128, 128)), v); // dx=63, inside
        assert_eq!(c.get(coord(192, 128, 128)), Voxel::default()); // dx=64, on
                                                                   // boundary; strict `<` excludes
        assert_eq!(c.get(coord(64, 128, 128)), Voxel::default()); // dx=-64, also excluded
        assert_eq!(c.get(coord(128, 128, 191)), v); // dz=63, inside
    }

    #[test]
    fn debug_does_not_dump_overrides() {
        let mut c = Cluster::empty();
        let v = Voxel::new(CornerVector::DEFAULT, nonempty_material());
        // 1000 distinct in-range coords spread across two axes.
        for i in 0..1000u32 {
            let x = i % 256;
            let y = i / 256;
            c.set(coord(x, y, 0), v);
        }
        assert_eq!(c.override_count(), 1000);
        let s = format!("{:?}", c);
        // The debug output should mention the count, not enumerate the entries.
        assert!(s.contains("override_count"));
        assert!(s.contains("1000"));
        // Length should be small regardless of override count.
        assert!(s.len() < 200, "Debug too verbose: {} bytes", s.len());
    }
}
