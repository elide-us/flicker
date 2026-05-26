//! Validated cluster-local coordinate.
//!
//! A [`LocalCoord`] addresses a single voxel within a 256³ cluster. Each
//! axis is in `0..256`, fitting exactly in a `u8` — we store as three bytes
//! and validate on construction so that all `LocalCoord` values are by
//! definition in bounds for any cluster.
//!
//! Construction takes `u32` per axis so that out-of-range inputs can be
//! rejected cleanly via `Option`. Accessors widen back to `u32` for
//! ergonomics in caller arithmetic.

use crate::cluster::CLUSTER_DIM;

/// A validated `(x, y, z)` coordinate inside a 256³ cluster.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocalCoord {
    x: u8,
    y: u8,
    z: u8,
}

impl LocalCoord {
    /// Construct a coordinate. Returns `None` if any axis is `>= 256`.
    #[inline]
    #[must_use]
    pub fn new(x: u32, y: u32, z: u32) -> Option<Self> {
        if x >= CLUSTER_DIM || y >= CLUSTER_DIM || z >= CLUSTER_DIM {
            return None;
        }
        Some(Self {
            x: x as u8,
            y: y as u8,
            z: z as u8,
        })
    }

    /// The X axis (`0..256`).
    #[inline]
    #[must_use]
    pub const fn x(&self) -> u32 {
        self.x as u32
    }

    /// The Y axis (`0..256`).
    #[inline]
    #[must_use]
    pub const fn y(&self) -> u32 {
        self.y as u32
    }

    /// The Z axis (`0..256`).
    #[inline]
    #[must_use]
    pub const fn z(&self) -> u32 {
        self.z as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_range_constructs() {
        let c = LocalCoord::new(0, 0, 0).unwrap();
        assert_eq!((c.x(), c.y(), c.z()), (0, 0, 0));
        let c = LocalCoord::new(255, 255, 255).unwrap();
        assert_eq!((c.x(), c.y(), c.z()), (255, 255, 255));
        let c = LocalCoord::new(42, 137, 200).unwrap();
        assert_eq!((c.x(), c.y(), c.z()), (42, 137, 200));
    }

    #[test]
    fn out_of_range_rejects() {
        assert!(LocalCoord::new(256, 0, 0).is_none());
        assert!(LocalCoord::new(0, 256, 0).is_none());
        assert!(LocalCoord::new(0, 0, 256).is_none());
        assert!(LocalCoord::new(u32::MAX, 0, 0).is_none());
        assert!(LocalCoord::new(0, 1000, 0).is_none());
    }

    #[test]
    fn boundary_255_is_valid() {
        assert!(LocalCoord::new(255, 255, 255).is_some());
    }

    #[test]
    fn equality_and_hash() {
        use std::collections::HashSet;
        let a = LocalCoord::new(1, 2, 3).unwrap();
        let b = LocalCoord::new(1, 2, 3).unwrap();
        let c = LocalCoord::new(3, 2, 1).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }
}
