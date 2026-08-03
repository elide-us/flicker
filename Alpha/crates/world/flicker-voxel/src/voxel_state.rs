//! Per-voxel physics state, stored as a dense 2-bit-per-voxel field.
//!
//! Every voxel in a cluster has one of four states. The state drives
//! mesh-time sign-change detection (`Empty` = "nothing here", anything
//! else = "matter here") and feeds the map-layer water-cycle
//! simulation that aggregates material volume across clusters. The
//! state field is **dense** — `2 bits × 256³ = 4 MB` per cluster,
//! fixed regardless of content — because the alternative (sparse
//! per-voxel state overrides) burns ~8 M HashMap entries per cluster
//! for a typical heightfield's everything-below-surface band, while
//! the dense field collapses that to a flat 4 MB regardless of how
//! solid the cluster is.
//!
//! The corner data (QEF vertex placement) and material identity stay
//! **sparse** in `Cluster`'s `surface_overrides` map — only voxels
//! that actually participate in defining a surface vertex carry that
//! information. A typical heightfield cluster has ~65 k surface
//! overrides; an entirely hand-edited worst-case cluster could
//! approach 16 M. Both ends work; storage scales with how much
//! information is actually per-voxel meaningful.

use clayengine::CLUSTER_DIM;

use crate::local_coord::LocalCoord;

/// Per-voxel physics state. The 2-bit `repr` is the in-storage
/// encoding the [`StateField`] uses; `Empty` = 0 so the field's
/// `Default` matches "all empty" without an explicit fill.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum VoxelState {
    /// Air, vacuum, or any "nothing renderable here" voxel. The
    /// mesh's sign-change check reads this as "no matter".
    #[default]
    Empty = 0,
    /// Rock, ice, sand, any non-flowing matter. The renderer treats
    /// all `Solid`-tagged voxels uniformly when emitting a mesh; the
    /// distinction between rock and ice lives in the material
    /// identity, not the state.
    Solid = 1,
    /// Slow-moving liquid (lava is the canonical example). Drawn as
    /// matter by the renderer; participates in the water cycle with
    /// viscous-fluid dynamics.
    Viscous = 2,
    /// Fast-moving liquid (water). Drawn as matter by the renderer;
    /// participates in the water cycle with low-viscosity dynamics.
    Flowing = 3,
}

impl VoxelState {
    /// Decode a 2-bit value back to a `VoxelState`. Only the low 2
    /// bits of the input are inspected; higher bits are masked off.
    /// Total over all 4 codes, so this is infallible.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0 => Self::Empty,
            1 => Self::Solid,
            2 => Self::Viscous,
            _ => Self::Flowing,
        }
    }

    /// The 2-bit encoding consumed by [`StateField`].
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        self as u8
    }

    /// `true` for any state that the renderer treats as matter
    /// (everything except [`Empty`]). The mesh's `is_solid` predicate
    /// reduces to this — it doesn't care whether a voxel is rock or
    /// lava, only whether it's filled. Material identity drives
    /// shading, this drives geometry.
    #[inline]
    #[must_use]
    pub const fn is_filled(self) -> bool {
        !matches!(self, Self::Empty)
    }
}

/// Dense 2-bit-per-voxel state storage for a single cluster.
///
/// Layout: a packed `[u64; STATE_FIELD_WORDS]` (4 MB) where every
/// voxel's state occupies 2 contiguous bits. The voxel at cluster-
/// local `(x, y, z)` lives at linear index `voxel_index(x, y, z) =
/// (z * CLUSTER_DIM + y) * CLUSTER_DIM + x` (z-major, matching the
/// contour's and bake's canonical ordering); each `u64` packs 32
/// voxels. Default-initialised storage is all-zero → every voxel
/// reads as [`VoxelState::Empty`].
#[derive(Clone)]
pub struct StateField {
    words: Box<[u64; STATE_FIELD_WORDS]>,
}

/// Number of voxels per cluster — the dense field's row count
/// before packing.
const VOXELS_PER_CLUSTER: usize =
    (CLUSTER_DIM as usize) * (CLUSTER_DIM as usize) * (CLUSTER_DIM as usize);

/// Number of voxels packed into each `u64` (2 bits per voxel).
const VOXELS_PER_WORD: usize = 32;

/// Number of `u64` words in the dense field. Each word holds 32
/// voxels' worth of state; the total is `16 M / 32 = 524 288` words
/// = `4 MB`.
pub const STATE_FIELD_WORDS: usize = VOXELS_PER_CLUSTER / VOXELS_PER_WORD;

// Compile-time sanity: every voxel must map to exactly one
// `(word, slot)` pair with no remainder. If `CLUSTER_DIM³` ever
// stops being a multiple of `VOXELS_PER_WORD`, this fails and the
// indexing math has to be revisited.
const _: () = assert!(VOXELS_PER_CLUSTER % VOXELS_PER_WORD == 0);

impl StateField {
    /// All-`Empty` field. The most common starting point — `Cluster`
    /// builds this and the contour fills in the solidified voxels
    /// from the primitive.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            words: Box::new([0u64; STATE_FIELD_WORDS]),
        }
    }

    /// State at `coord`. Always succeeds — every in-range coord has
    /// an explicit 2-bit value, defaulting to `Empty` when nothing
    /// has been written.
    #[inline]
    #[must_use]
    pub fn get(&self, coord: LocalCoord) -> VoxelState {
        let (word_idx, shift) = location(coord);
        let bits = ((self.words[word_idx] >> shift) & 0b11) as u8;
        VoxelState::from_bits(bits)
    }

    /// Write `state` at `coord`, overwriting whatever was there.
    #[inline]
    pub fn set(&mut self, coord: LocalCoord, state: VoxelState) {
        let (word_idx, shift) = location(coord);
        let cleared = self.words[word_idx] & !(0b11u64 << shift);
        self.words[word_idx] = cleared | ((state.to_bits() as u64) << shift);
    }

    /// Borrow the raw packed words for byte-level access (bake
    /// serialisation, GPU upload). 4 MB, byte-exact.
    #[inline]
    #[must_use]
    pub fn as_words(&self) -> &[u64; STATE_FIELD_WORDS] {
        &self.words
    }

    /// Mutable view of the raw packed words. Used by `Cluster::uniform`
    /// to fill the field with a repeating 2-bit pattern in word-sized
    /// strides, and by future bulk-modification paths. Per-voxel writes
    /// should still go through [`Self::set`].
    #[inline]
    pub fn words_mut(&mut self) -> &mut [u64; STATE_FIELD_WORDS] {
        &mut self.words
    }

    /// Reconstruct from raw packed words — inverse of [`Self::as_words`].
    /// Used by the bake loader; the caller is responsible for trusting
    /// the input (no decoding required — every 64-bit pattern is a
    /// legal field).
    #[must_use]
    pub fn from_words(words: Box<[u64; STATE_FIELD_WORDS]>) -> Self {
        Self { words }
    }
}

impl Default for StateField {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for StateField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't enumerate 16 M voxels. Summarise non-Empty count
        // instead — the only useful field-wide summary anyone reads
        // in a debugger.
        let mut filled = 0_usize;
        for w in self.words.iter() {
            let mut w = *w;
            while w != 0 {
                // Each pair of bits that is non-zero counts as one
                // filled voxel. Pair-test by ORing the two bits.
                let pair_present = (w & 1) | ((w >> 1) & 1);
                filled += pair_present as usize;
                w >>= 2;
            }
        }
        f.debug_struct("StateField")
            .field("voxels", &VOXELS_PER_CLUSTER)
            .field("filled", &filled)
            .finish()
    }
}

/// `(word_index, bit_shift_within_word)` for a voxel coord. Z-major
/// linearisation matches `contour`'s iteration order so the dense
/// field is laid out cache-friendly for the contour's hot path.
#[inline]
fn location(coord: LocalCoord) -> (usize, u32) {
    let dim = CLUSTER_DIM as usize;
    let linear = (coord.z() as usize * dim + coord.y() as usize) * dim + coord.x() as usize;
    let word_idx = linear / VOXELS_PER_WORD;
    let slot = linear % VOXELS_PER_WORD;
    // 2 bits per slot, low slot in low bits.
    (word_idx, (slot * 2) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("in range")
    }

    #[test]
    fn empty_field_reads_empty_everywhere() {
        let f = StateField::empty();
        for &(x, y, z) in &[
            (0u32, 0u32, 0u32),
            (1, 1, 1),
            (128, 128, 128),
            (255, 255, 255),
            (0, 255, 0),
            (255, 0, 255),
        ] {
            assert_eq!(f.get(coord(x, y, z)), VoxelState::Empty);
        }
    }

    #[test]
    fn set_then_get_round_trips_all_four_states() {
        let mut f = StateField::empty();
        f.set(coord(10, 20, 30), VoxelState::Solid);
        f.set(coord(10, 20, 31), VoxelState::Viscous);
        f.set(coord(10, 20, 32), VoxelState::Flowing);
        f.set(coord(10, 20, 33), VoxelState::Empty);
        assert_eq!(f.get(coord(10, 20, 30)), VoxelState::Solid);
        assert_eq!(f.get(coord(10, 20, 31)), VoxelState::Viscous);
        assert_eq!(f.get(coord(10, 20, 32)), VoxelState::Flowing);
        assert_eq!(f.get(coord(10, 20, 33)), VoxelState::Empty);
    }

    #[test]
    fn writes_to_one_voxel_do_not_disturb_neighbours() {
        // Pack 32 voxels into one word, set the middle one to
        // Flowing, confirm the rest stay Empty. Catches off-by-one
        // shifts in the bit-pack math.
        let mut f = StateField::empty();
        // Voxels at (0,0,0)..(31,0,0) share word 0.
        for x in 0..32 {
            assert_eq!(f.get(coord(x, 0, 0)), VoxelState::Empty);
        }
        f.set(coord(15, 0, 0), VoxelState::Flowing);
        for x in 0..32 {
            let expected = if x == 15 {
                VoxelState::Flowing
            } else {
                VoxelState::Empty
            };
            assert_eq!(
                f.get(coord(x, 0, 0)),
                expected,
                "voxel ({x}, 0, 0) drifted"
            );
        }
    }

    #[test]
    fn overwrite_replaces_state() {
        let mut f = StateField::empty();
        let c = coord(42, 17, 89);
        f.set(c, VoxelState::Solid);
        assert_eq!(f.get(c), VoxelState::Solid);
        f.set(c, VoxelState::Viscous);
        assert_eq!(f.get(c), VoxelState::Viscous);
        f.set(c, VoxelState::Empty);
        assert_eq!(f.get(c), VoxelState::Empty);
    }

    #[test]
    fn extremal_coords_pack_into_the_right_word() {
        // The very last voxel `(255, 255, 255)` must land in the
        // very last word — a regression in `location()` would
        // either panic or write into the wrong word.
        let mut f = StateField::empty();
        let c = coord(255, 255, 255);
        f.set(c, VoxelState::Solid);
        assert_eq!(f.get(c), VoxelState::Solid);
        // And the first voxel still reads Empty.
        assert_eq!(f.get(coord(0, 0, 0)), VoxelState::Empty);
    }

    #[test]
    fn round_trip_through_word_buffer_preserves_state() {
        // Bake-loader path: round-trip an interesting pattern
        // through `as_words` → `from_words` and verify the field
        // compares equal at every probed voxel.
        let mut f = StateField::empty();
        f.set(coord(0, 0, 0), VoxelState::Solid);
        f.set(coord(1, 0, 0), VoxelState::Viscous);
        f.set(coord(2, 0, 0), VoxelState::Flowing);
        f.set(coord(255, 255, 255), VoxelState::Solid);
        f.set(coord(100, 100, 100), VoxelState::Flowing);
        // Allocate the buffer on the heap directly to avoid a 4 MB
        // stack copy (Box::new(*array) would copy through the stack).
        let mut heap_buf = vec![0u64; STATE_FIELD_WORDS].into_boxed_slice();
        heap_buf.copy_from_slice(f.as_words());
        let words: Box<[u64; STATE_FIELD_WORDS]> = heap_buf.try_into().unwrap();
        let loaded = StateField::from_words(words);
        for &(x, y, z, expected) in &[
            (0u32, 0u32, 0u32, VoxelState::Solid),
            (1, 0, 0, VoxelState::Viscous),
            (2, 0, 0, VoxelState::Flowing),
            (255, 255, 255, VoxelState::Solid),
            (100, 100, 100, VoxelState::Flowing),
            (50, 50, 50, VoxelState::Empty),
        ] {
            assert_eq!(loaded.get(coord(x, y, z)), expected);
        }
    }

    #[test]
    fn state_field_is_4_mb() {
        // Documenting the size at compile-time so a future change
        // to CLUSTER_DIM gets caught here before it surprises the
        // bake format.
        assert_eq!(STATE_FIELD_WORDS, 524_288);
        assert_eq!(STATE_FIELD_WORDS * 8, 4 * 1024 * 1024);
    }

    #[test]
    fn voxel_state_bit_round_trip() {
        for state in [
            VoxelState::Empty,
            VoxelState::Solid,
            VoxelState::Viscous,
            VoxelState::Flowing,
        ] {
            assert_eq!(VoxelState::from_bits(state.to_bits()), state);
        }
        // High bits are masked off — decoder is total.
        assert_eq!(VoxelState::from_bits(0xFF), VoxelState::Flowing);
        assert_eq!(VoxelState::from_bits(0xFE), VoxelState::Viscous);
        assert_eq!(VoxelState::from_bits(0xFD), VoxelState::Solid);
        assert_eq!(VoxelState::from_bits(0xFC), VoxelState::Empty);
    }

    #[test]
    fn is_filled_partitions_states_correctly() {
        assert!(!VoxelState::Empty.is_filled());
        assert!(VoxelState::Solid.is_filled());
        assert!(VoxelState::Viscous.is_filled());
        assert!(VoxelState::Flowing.is_filled());
    }
}
