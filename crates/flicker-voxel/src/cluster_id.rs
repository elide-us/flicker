//! Packed `ClusterId` — the world-space vocabulary for cluster
//! addressing.
//!
//! `ClusterId` is a single packed `u32` identifying *which* cluster
//! at *which* LOD. Multi-cluster operations (per-cluster meshes, the
//! cluster map's keying, world-coordinate generation) route through
//! this type rather than through raw `(x, y, z)` tuples.
//!
//! # Bit layout (LOCKED)
//!
//! `u32` packed `[LOD: 4][x: 10][y: 8][z: 10]` from high to low:
//!
//! | field | bits | range          |
//! |-------|------|----------------|
//! | LOD   |  4   | `0..=15`       |
//! | x     | 10   | `0..=1023`     |
//! | y     |  8   | `0..=255`      |
//! | z     | 10   | `0..=1023`     |
//!
//! Total `4 + 10 + 8 + 10 = 32` bits — exactly one `u32`.
//!
//! # LOD semantics (locked, only LOD 0 exercised in this step)
//!
//! LOD `0..=7` are intra-cluster — the `(x, y, z)` addresses a single
//! physical cluster; LOD selects which sample-stride representation
//! of that same cluster. LOD `8..=15` are inter-cluster aggregation
//! (one address represents a 2^(LOD−7) cube of clusters).
//!
//! This step (multi-cluster, all LOD 0) only constructs LOD-0 ids
//! with `y = 0`, but the bit-pack is locked for all 32 bits so future
//! LOD work plugs in without breaking the substrate.

use std::fmt;

use crate::cluster::CLUSTER_DIM;

/// Cluster address: `(LOD, x, y, z)` packed into a single `u32`.
///
/// See the [module-level docs](self) for the bit layout and LOD
/// semantics.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct ClusterId(u32);

impl ClusterId {
    /// Largest legal LOD value (15 — exhausts the 4-bit field).
    pub const MAX_LOD: u8 = 0xF;
    /// Largest legal `x` (1023 — exhausts the 10-bit field).
    pub const MAX_X: u16 = 0x3FF;
    /// Largest legal `y` (255 — exhausts the 8-bit field).
    pub const MAX_Y: u16 = 0xFF;
    /// Largest legal `z` (1023 — exhausts the 10-bit field).
    pub const MAX_Z: u16 = 0x3FF;

    const LOD_SHIFT: u32 = 28;
    const X_SHIFT: u32 = 18;
    const Y_SHIFT: u32 = 10;

    const LOD_MASK: u32 = 0xF;
    const X_MASK: u32 = 0x3FF;
    const Y_MASK: u32 = 0xFF;
    const Z_MASK: u32 = 0x3FF;

    /// Construct a `ClusterId` from its components. Panics if any
    /// component exceeds its field width.
    #[inline]
    #[must_use]
    pub const fn new(lod: u8, x: u16, y: u16, z: u16) -> Self {
        assert!(lod <= Self::MAX_LOD, "ClusterId: lod overflows 4-bit field");
        assert!(x <= Self::MAX_X, "ClusterId: x overflows 10-bit field");
        assert!(y <= Self::MAX_Y, "ClusterId: y overflows 8-bit field");
        assert!(z <= Self::MAX_Z, "ClusterId: z overflows 10-bit field");
        let packed = ((lod as u32) << Self::LOD_SHIFT)
            | ((x as u32) << Self::X_SHIFT)
            | ((y as u32) << Self::Y_SHIFT)
            | (z as u32);
        Self(packed)
    }

    /// The raw packed `u32`. Useful for serialization, atomic-cell
    /// storage, and hashing into per-cluster seeds.
    #[inline]
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Reconstruct a `ClusterId` from raw bits — inverse of
    /// [`Self::bits`]. Does not validate; intended for round-tripping
    /// through a known-valid serialization.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// LOD field (`0..=15`).
    #[inline]
    #[must_use]
    pub const fn lod(self) -> u8 {
        ((self.0 >> Self::LOD_SHIFT) & Self::LOD_MASK) as u8
    }

    /// X cell index (`0..=1023`).
    #[inline]
    #[must_use]
    pub const fn x(self) -> u16 {
        ((self.0 >> Self::X_SHIFT) & Self::X_MASK) as u16
    }

    /// Y cell index (`0..=255`).
    #[inline]
    #[must_use]
    pub const fn y(self) -> u16 {
        ((self.0 >> Self::Y_SHIFT) & Self::Y_MASK) as u16
    }

    /// Z cell index (`0..=1023`).
    #[inline]
    #[must_use]
    pub const fn z(self) -> u16 {
        (self.0 & Self::Z_MASK) as u16
    }

    /// World-space origin of this cluster's `(0, 0, 0)` local corner,
    /// in voxel units: `[x, y, z] * CLUSTER_DIM`.
    ///
    /// At LOD 0 this is the straightforward translation a cluster's
    /// mesh draw matrix uses. Higher-LOD semantics are deferred to
    /// later steps — they may want different offset behavior (e.g.
    /// inter-cluster aggregation at LOD ≥ 8 spans multiple clusters'
    /// worth of voxels), but for LOD 0 the result is well-defined.
    #[inline]
    #[must_use]
    pub fn world_offset(self) -> [f32; 3] {
        [
            self.x() as f32 * CLUSTER_DIM as f32,
            self.y() as f32 * CLUSTER_DIM as f32,
            self.z() as f32 * CLUSTER_DIM as f32,
        ]
    }
}

impl fmt::Debug for ClusterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ClusterId {{ lod: {}, x: {}, y: {}, z: {}, bits: 0x{:08X} }}",
            self.lod(),
            self.x(),
            self.y(),
            self.z(),
            self.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_components_across_full_range() {
        // Probe every field at its endpoints and a few middling values.
        let probes = [
            (0u8, 0u16, 0u16, 0u16),
            (15, 1023, 255, 1023),
            (0, 1023, 0, 0),
            (0, 0, 255, 0),
            (0, 0, 0, 1023),
            (7, 512, 128, 768),
            (3, 1, 2, 3),
        ];
        for (lod, x, y, z) in probes {
            let id = ClusterId::new(lod, x, y, z);
            assert_eq!(
                id.lod(),
                lod,
                "lod round-trip failed for {:?}",
                (lod, x, y, z)
            );
            assert_eq!(id.x(), x, "x round-trip failed for {:?}", (lod, x, y, z));
            assert_eq!(id.y(), y, "y round-trip failed for {:?}", (lod, x, y, z));
            assert_eq!(id.z(), z, "z round-trip failed for {:?}", (lod, x, y, z));
        }
    }

    #[test]
    fn bit_layout_isolation() {
        // Each field's bits must not bleed into another field.
        // Set lod=15, leave others zero — only lod bits should be set.
        let lod_only = ClusterId::new(15, 0, 0, 0);
        assert_eq!(lod_only.bits(), 0xF0000000);

        let x_only = ClusterId::new(0, 1023, 0, 0);
        assert_eq!(x_only.bits(), 0x3FF << 18);

        let y_only = ClusterId::new(0, 0, 255, 0);
        assert_eq!(y_only.bits(), 0xFF << 10);

        let z_only = ClusterId::new(0, 0, 0, 1023);
        assert_eq!(z_only.bits(), 0x3FF);

        // All-ones sanity: every legal max packs into 0xFFFF_FFFF.
        let all_max = ClusterId::new(15, 1023, 255, 1023);
        assert_eq!(all_max.bits(), 0xFFFF_FFFF);
    }

    #[test]
    fn from_bits_inverts_bits() {
        let id = ClusterId::new(11, 700, 42, 200);
        let again = ClusterId::from_bits(id.bits());
        assert_eq!(id, again);
        assert_eq!(again.lod(), 11);
        assert_eq!(again.x(), 700);
        assert_eq!(again.y(), 42);
        assert_eq!(again.z(), 200);
    }

    #[test]
    fn world_offset_is_field_times_cluster_dim() {
        let id = ClusterId::new(0, 2, 0, 3);
        let off = id.world_offset();
        assert_eq!(
            off,
            [2.0 * CLUSTER_DIM as f32, 0.0, 3.0 * CLUSTER_DIM as f32]
        );

        let id_y = ClusterId::new(0, 0, 1, 0);
        let off_y = id_y.world_offset();
        assert_eq!(off_y, [0.0, CLUSTER_DIM as f32, 0.0]);

        let id_zero = ClusterId::new(0, 0, 0, 0);
        assert_eq!(id_zero.world_offset(), [0.0; 3]);
    }

    #[test]
    #[should_panic(expected = "lod")]
    fn new_panics_when_lod_overflows() {
        let _ = ClusterId::new(16, 0, 0, 0);
    }

    #[test]
    #[should_panic(expected = "x")]
    fn new_panics_when_x_overflows() {
        let _ = ClusterId::new(0, 1024, 0, 0);
    }

    #[test]
    #[should_panic(expected = "y")]
    fn new_panics_when_y_overflows() {
        let _ = ClusterId::new(0, 0, 256, 0);
    }

    #[test]
    #[should_panic(expected = "z")]
    fn new_panics_when_z_overflows() {
        let _ = ClusterId::new(0, 0, 0, 1024);
    }
}
