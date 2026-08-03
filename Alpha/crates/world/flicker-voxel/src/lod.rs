//! Level-of-detail: the sample stride at which a cluster is read.
//!
//! [`Lod`] is a fundamental cluster-sampling quantity. It is independent of
//! cross-cluster neighbours, of contouring, and of meshing — a LOD is just an
//! integer whose only derived quantities are `stride = 2^level` and
//! `sample_dim = 256 >> level`. It lives in its own module so that data
//! producers ([`crate::contour`], [`crate::derive_lod`]) and consumers
//! ([`crate::mesh`], the nav surface, [`crate::neighbor`]) can name the type
//! without taking a dependency on one another. (Historically `Lod` lived in
//! `neighbor`, which forced the neighbour-oblivious contour path to depend on
//! the neighbour module purely to name this type.)

use clayengine::{CLUSTER_DIM, MAX_LOD};

/// Level of detail.
///
/// LOD `L` reads every `2^L`-th voxel along each axis when contouring.
/// Valid range is `0..=8`:
///
/// | level | stride | sample_dim | cell_dim |
/// | ----: | -----: | ---------: | -------: |
/// |   0   |    1   |    256     |   255    |
/// |   1   |    2   |    128     |   127    |
/// |   2   |    4   |     64     |    63    |
/// |   3   |    8   |     32     |    31    |
/// |   4   |   16   |     16     |    15    |
/// |   5   |   32   |      8     |     7    |
/// |   6   |   64   |      4     |     3    |
/// |   7   |  128   |      2     |     1    |
/// |   8   |  256   |      1     |     0    |
///
/// At LOD 8 the cluster is represented by a single sample vector
/// (`sample_dim = 1`, no interior cell); its surface quad is formed in
/// the normal manner through the field function — from this cluster's
/// vector plus the neighbours' vectors read across the seams. LOD 9
/// would sample nothing (`sample_dim = 0`), so 8 is the floor.
///
/// The sample sets are nested: positions sampled at LOD `L+1` are a strict
/// subset of positions sampled at LOD `L`, by construction. This is what
/// keeps inter-LOD geometry consistent across levels.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Lod(u8);

impl Lod {
    /// LOD 0 — full resolution.
    pub const ZERO: Lod = Lod(0);

    /// Maximum allowed LOD — [`clayengine::MAX_LOD`] (= `log2(CLUSTER_DIM)`
    /// = 8). At this LOD the cluster is represented by a single vector
    /// (`sample_dim = 1`, no interior cell); its quad is formed in the
    /// normal manner through the field function, from this cluster's vector
    /// plus the neighbours' across the seams. One step beyond
    /// (`sample_dim = 0`) would sample nothing, so this is the floor.
    pub const MAX: Lod = Lod(MAX_LOD);

    /// Construct an LOD from a level. Returns `None` if `level > 8`.
    #[inline]
    #[must_use]
    pub const fn new(level: u8) -> Option<Lod> {
        if level > Self::MAX.0 {
            None
        } else {
            Some(Lod(level))
        }
    }

    /// The LOD level (`0..=8`).
    #[inline]
    #[must_use]
    pub const fn level(self) -> u8 {
        self.0
    }

    /// The voxel stride at this LOD (`2^level`). Always a power of two in
    /// `[1, 256]`.
    #[inline]
    #[must_use]
    pub const fn stride(self) -> u32 {
        1u32 << self.0
    }

    /// The effective sample dimension at this LOD: `256 / stride`. Always
    /// in `[1, 256]`.
    #[inline]
    #[must_use]
    pub const fn sample_dim(self) -> u32 {
        CLUSTER_DIM >> self.0
    }

    /// The effective cell dimension at this LOD: `sample_dim - 1`. Always
    /// in `[0, 255]` (0 at LOD 8, where the cluster is a single vector
    /// with no interior cell).
    #[inline]
    #[must_use]
    pub const fn cell_dim(self) -> u32 {
        (CLUSTER_DIM >> self.0) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_new_accepts_0_through_8() {
        for level in 0..=8u8 {
            assert!(
                Lod::new(level).is_some(),
                "Lod::new({level}) should be Some"
            );
        }
        assert!(Lod::new(9).is_none());
        assert!(Lod::new(255).is_none());
    }

    #[test]
    fn lod_zero_constants() {
        assert_eq!(Lod::ZERO.level(), 0);
        assert_eq!(Lod::ZERO.stride(), 1);
        assert_eq!(Lod::ZERO.sample_dim(), 256);
        assert_eq!(Lod::ZERO.cell_dim(), 255);
    }

    #[test]
    fn lod_max_constants() {
        assert_eq!(Lod::MAX.level(), 8);
        assert_eq!(Lod::MAX.stride(), 256);
        assert_eq!(Lod::MAX.sample_dim(), 1);
        assert_eq!(Lod::MAX.cell_dim(), 0);
    }

    #[test]
    fn lod_dim_relationships_at_every_level() {
        for level in 0..=8u8 {
            let lod = Lod::new(level).unwrap();
            assert_eq!(lod.stride(), 1u32 << level, "stride at LOD {level}");
            assert_eq!(
                lod.sample_dim(),
                256 / lod.stride(),
                "sample_dim at LOD {level}"
            );
            assert_eq!(
                lod.cell_dim(),
                lod.sample_dim() - 1,
                "cell_dim at LOD {level}"
            );
        }
    }
}
