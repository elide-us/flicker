//! Sparse 256³ voxel cluster.
//!
//! A [`Cluster`] stores a single 256×256×256 volume of voxels — 16,777,216
//! cells — split across two storage layers:
//!
//! - **State** is *dense*: every voxel has a [`VoxelState`] (`Empty` /
//!   `Solid` / `Viscous` / `Flowing`), stored in a packed 2-bit-per-voxel
//!   [`StateField`] (4 MB fixed). This is the renderer's "is this matter?"
//!   oracle and the map-layer water-cycle simulation's input.
//! - **Surface data** is *sparse*: only voxels that actually participate in
//!   a rendered surface vertex carry an explicit `(CornerVector, Material)`
//!   pair in `surface_overrides`. A typical contoured heightfield cluster
//!   has ~65 k entries here. Voxels not present in this map have the
//!   default corner and the cluster's `default_material` (which is the
//!   bulk-fill material for whatever a `Solid` voxel without surface data
//!   is — granite, say).
//!
//! Reads are infallible — [`Cluster::get`] always returns a [`Voxel`],
//! reading state from the dense field and corner+material from the sparse
//! map (or falling back to defaults). Writes via [`Cluster::set`] split
//! across the two stores: the state bit always updates; the
//! `(corner, material)` pair is inserted into `surface_overrides` only
//! when it carries information beyond the cluster's defaults, and removed
//! from `surface_overrides` when it doesn't.
//!
//! This split is what makes the cluster cheap to bake even on giant solid
//! masses: a fully-buried heightfield cluster has ~65 k entries in the
//! sparse map and 4 MB of state bits — not 8 M HashMap entries marking
//! every solid voxel as "still solid".

use std::collections::hash_map;
use std::collections::HashMap;
use std::fmt;

use crate::corner_vector::CornerVector;
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::voxel::Voxel;
use crate::voxel_state::{StateField, VoxelState};

/// A 256³ voxel volume — dense state per voxel + sparse surface overrides.
pub struct Cluster {
    /// Dense 2-bit-per-voxel physics state. Always present, 4 MB fixed.
    state: StateField,
    /// Sparse surface data — `(corner, material)` for voxels that
    /// actually carry a meaningful surface vertex. Everything not in
    /// this map gets defaults at read time.
    ///
    /// Future: when the HashMap's locality starts hurting LOD striding /
    /// dense-region scans, swap for an octree backed by dense leaf
    /// chunks. The trait surface (`get`/`set`/iteration) absorbs that.
    surface_overrides: HashMap<LocalCoord, (CornerVector, Material)>,
    /// The bulk-fill material for `Solid` voxels that have no surface
    /// override — e.g. the rock mass underneath a heightfield. Returned
    /// from `get` for any solid voxel without an explicit entry in
    /// `surface_overrides`. Defaults to [`Material::EMPTY`] for an empty
    /// cluster; the contour pipeline sets it to the primitive's material
    /// when contouring a populated primitive.
    default_material: Material,
}

impl Cluster {
    /// An all-`Empty` cluster — no matter anywhere, no surface
    /// overrides, default material is [`Material::EMPTY`].
    #[must_use]
    pub fn empty() -> Self {
        Self {
            state: StateField::empty(),
            surface_overrides: HashMap::new(),
            default_material: Material::EMPTY,
        }
    }

    /// A cluster pre-filled to a uniform `base` voxel: every voxel's
    /// state is set to `base.state()`, and `base.material()` becomes the
    /// cluster's bulk-fill material. `base.corner()` is ignored —
    /// corners are sparse-only data.
    #[must_use]
    pub fn uniform(base: Voxel) -> Self {
        let mut state = StateField::empty();
        let bits = base.state().to_bits() as u64;
        if bits != 0 {
            // Repeating 2-bit pattern across the whole word. For
            // state=Solid (bits=1): 0x5555..., for Viscous (2):
            // 0xAAAA..., for Flowing (3): 0xFFFF.... Fills the dense
            // field in word-sized strides without per-voxel calls.
            let pattern = (0..32).fold(0u64, |acc, i| acc | (bits << (i * 2)));
            for word in state.words_mut().iter_mut() {
                *word = pattern;
            }
        }
        Self {
            state,
            surface_overrides: HashMap::new(),
            default_material: base.material(),
        }
    }

    /// The cluster's bulk-fill material — what `Solid` voxels without
    /// an explicit surface override report. Renderer-irrelevant for
    /// those voxels (they're below the surface and never drawn), but
    /// the value is preserved for completeness and round-tripping.
    #[inline]
    #[must_use]
    pub fn default_material(&self) -> Material {
        self.default_material
    }

    /// Set the cluster's bulk-fill material. Used by the contour
    /// pipeline so a contoured cluster correctly reports the
    /// primitive's material at its solid interior, and by bake load
    /// to restore the recorded value.
    pub fn set_default_material(&mut self, material: Material) {
        self.default_material = material;
    }

    /// Set the state bit at `coord` without touching the sparse
    /// surface map. The fast path for bulk physics-state writes —
    /// the contour's pass 1 (flipping solidity from a primitive) and
    /// the map-layer water cycle (updating viscous/flowing voxels as
    /// they advect) both go through here. Does **not** clear any
    /// existing `(corner, material)` override at `coord`; that's a
    /// separate intent and you go through `set` if you want it.
    #[inline]
    pub fn set_state(&mut self, coord: LocalCoord, state: VoxelState) {
        self.state.set(coord, state);
    }

    /// Read the voxel at `coord`. Always succeeds: state comes from
    /// the dense field; corner+material from the sparse surface
    /// override at `coord` if one exists, else from state-appropriate
    /// defaults (empty state → empty material, filled state → the
    /// cluster's default material; the default corner in both cases).
    #[inline]
    #[must_use]
    pub fn get(&self, coord: LocalCoord) -> Voxel {
        let state = self.state.get(coord);
        if let Some(&(corner, material)) = self.surface_overrides.get(&coord) {
            return Voxel::new(state, corner, material);
        }
        let material = match state {
            VoxelState::Empty => Material::EMPTY,
            _ => self.default_material,
        };
        Voxel::new(state, CornerVector::DEFAULT, material)
    }

    /// Write `voxel` at `coord`. Always updates the state bit; the
    /// `(corner, material)` pair goes into `surface_overrides` **only
    /// when it carries information beyond the cluster's defaults** for
    /// the voxel's state. Default corner + state-appropriate default
    /// material removes any existing entry (so set-then-unset stays
    /// sparse and reclaims memory).
    pub fn set(&mut self, coord: LocalCoord, voxel: Voxel) {
        self.state.set(coord, voxel.state());
        let state_default_material = match voxel.state() {
            VoxelState::Empty => Material::EMPTY,
            _ => self.default_material,
        };
        let is_default_surface_data =
            voxel.corner() == CornerVector::DEFAULT && voxel.material() == state_default_material;
        if is_default_surface_data {
            self.surface_overrides.remove(&coord);
        } else {
            self.surface_overrides
                .insert(coord, (voxel.corner(), voxel.material()));
        }
    }

    /// Number of voxels that carry an explicit surface override
    /// (non-default corner or non-default material for their state).
    /// Bulk state-only writes don't count here; this is the
    /// `(corner, material)` map's size.
    #[inline]
    #[must_use]
    pub fn override_count(&self) -> usize {
        self.surface_overrides.len()
    }

    /// `true` iff no voxel carries an explicit surface override. The
    /// state field may still be entirely non-empty (e.g. a uniform
    /// solid cluster has no overrides but plenty of state).
    #[inline]
    #[must_use]
    pub fn is_uniform(&self) -> bool {
        self.surface_overrides.is_empty()
    }

    /// Iterate every surface override as `(coordinate, voxel)`. The
    /// yielded `Voxel` carries its state pulled from the dense field
    /// so callers downstream don't have to re-`get`. Order is
    /// unspecified (the underlying `HashMap` doesn't preserve any).
    #[must_use]
    pub fn overrides(&self) -> Overrides<'_> {
        Overrides {
            state: &self.state,
            inner: self.surface_overrides.iter(),
        }
    }

    /// Borrow the dense state field directly. Used by the bake layer
    /// for serialisation and by future renderer paths that want
    /// word-level access.
    #[inline]
    #[must_use]
    pub fn state_field(&self) -> &StateField {
        &self.state
    }

    /// Replace the dense state field wholesale. Used by the bake
    /// loader; not intended for normal mutation (use `set` for that).
    pub fn replace_state_field(&mut self, state: StateField) {
        self.state = state;
    }
}

impl fmt::Debug for Cluster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Don't dump every override or every state bit. Summarise.
        f.debug_struct("Cluster")
            .field("default_material", &self.default_material)
            .field("surface_override_count", &self.surface_overrides.len())
            .field("state", &self.state)
            .finish()
    }
}

/// Iterator over a cluster's surface overrides. See
/// [`Cluster::overrides`]. Each item is the full `Voxel` (state pulled
/// from the dense field, corner+material from the sparse map).
pub struct Overrides<'a> {
    state: &'a StateField,
    inner: hash_map::Iter<'a, LocalCoord, (CornerVector, Material)>,
}

impl Iterator for Overrides<'_> {
    type Item = (LocalCoord, Voxel);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(&coord, &(corner, material))| {
            let state = self.state.get(coord);
            (coord, Voxel::new(state, corner, material))
        })
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
    use clayengine::{CLUSTER_DIM, VOXEL_COUNT};

    fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("in-range")
    }

    fn nonempty_material() -> Material {
        Material::new(7, 11, 200).unwrap()
    }

    fn solid_grey() -> Voxel {
        Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, nonempty_material())
    }

    #[test]
    fn cluster_dim_and_voxel_count_match_spec() {
        assert_eq!(CLUSTER_DIM, 256);
        assert_eq!(VOXEL_COUNT, 256 * 256 * 256);
    }

    #[test]
    fn empty_cluster_reads_default_everywhere() {
        let c = Cluster::empty();
        assert_eq!(c.default_material(), Material::EMPTY);
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
    fn uniform_solid_cluster_reads_solid_everywhere() {
        let c = Cluster::uniform(solid_grey());
        assert_eq!(c.default_material(), nonempty_material());
        assert!(c.is_uniform(), "no surface overrides yet");
        for &(x, y, z) in &[(0u32, 0u32, 0u32), (42, 137, 200), (255, 255, 255)] {
            let v = c.get(coord(x, y, z));
            assert_eq!(v.state(), VoxelState::Solid);
            assert_eq!(v.corner(), CornerVector::DEFAULT);
            assert_eq!(v.material(), nonempty_material());
        }
    }

    #[test]
    fn single_surface_override_increments_count_and_is_readable() {
        let mut c = Cluster::empty();
        let custom_corner = CornerVector::from_bytes([1, 2, 3]);
        let v = Voxel::new(VoxelState::Solid, custom_corner, nonempty_material());
        let where_ = coord(5, 6, 7);
        c.set(where_, v);
        assert_eq!(c.override_count(), 1);
        assert!(!c.is_uniform());
        let read = c.get(where_);
        assert_eq!(read.state(), VoxelState::Solid);
        assert_eq!(read.corner(), custom_corner);
        assert_eq!(read.material(), nonempty_material());
        // Neighbouring voxels still read default.
        assert_eq!(c.get(coord(4, 6, 7)), Voxel::default());
        assert_eq!(c.get(coord(5, 7, 7)), Voxel::default());
        assert_eq!(c.get(coord(5, 6, 8)), Voxel::default());
    }

    #[test]
    fn override_back_to_default_removes_the_sparse_entry() {
        // Writing a Voxel that carries no surface information beyond
        // what the dense field + default material already encode
        // should remove any existing entry — set-then-unset stays
        // sparse.
        let mut c = Cluster::empty();
        let v = Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, nonempty_material());
        let where_ = coord(10, 20, 30);
        c.set(where_, v);
        // Even though state changed (Empty → Solid), there's no
        // surface info to store — material matches the (empty)
        // default for an empty cluster's Empty default but state has
        // moved to Solid + EMPTY... actually with an empty cluster's
        // default_material = EMPTY, a Solid voxel with EMPTY material
        // *does* differ from state-default (Solid wants
        // default_material = EMPTY here, which matches), so this
        // shouldn't override.
        // But we set with `nonempty_material()`, which DOES differ
        // from the cluster's `default_material = EMPTY`. So an
        // override is stored.
        assert_eq!(c.override_count(), 1);
        // Now set the cluster's default material to match the value
        // we wrote, and re-set the voxel; the override should drop.
        c.set_default_material(nonempty_material());
        c.set(where_, v);
        assert_eq!(c.override_count(), 0);
        // The state bit is still Solid even though the override is
        // gone, and read returns the bulk-fill material.
        let read = c.get(where_);
        assert_eq!(read.state(), VoxelState::Solid);
        assert_eq!(read.material(), nonempty_material());
    }

    #[test]
    fn set_state_without_explicit_corner_stays_sparse() {
        // Writing a voxel with default corner and the cluster's
        // default material should never create a sparse entry —
        // there's no surface info to store.
        let mut c = Cluster::uniform(solid_grey());
        let v = Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, nonempty_material());
        for &(x, y, z) in &[(0u32, 0u32, 0u32), (42, 42, 42), (255, 255, 255)] {
            c.set(coord(x, y, z), v);
        }
        assert_eq!(c.override_count(), 0);
    }

    #[test]
    fn updating_an_existing_override_does_not_change_count() {
        let mut c = Cluster::empty();
        let v1 = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_bytes([1, 1, 1]),
            Material::new(1, 0, 0).unwrap(),
        );
        let v2 = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_bytes([2, 2, 2]),
            Material::new(2, 0, 0).unwrap(),
        );
        let where_ = coord(100, 100, 100);
        c.set(where_, v1);
        c.set(where_, v2);
        assert_eq!(c.override_count(), 1);
        assert_eq!(c.get(where_), v2);
    }

    #[test]
    fn overrides_iterator_visits_exactly_the_surface_overrides() {
        let mut c = Cluster::empty();
        let v = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_bytes([1, 2, 3]),
            nonempty_material(),
        );
        let pts = [coord(0, 0, 0), coord(1, 2, 3), coord(255, 255, 255)];
        for p in pts {
            c.set(p, v);
        }
        let collected: HashMap<_, _> = c.overrides().collect();
        assert_eq!(collected.len(), 3);
        for p in pts {
            assert_eq!(collected.get(&p), Some(&v));
        }
    }

    #[test]
    fn cluster_boundary_coords_are_valid() {
        let mut c = Cluster::empty();
        let v = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_bytes([1, 1, 1]),
            nonempty_material(),
        );
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
    fn sphere_of_solid_writes_costs_zero_sparse_entries() {
        // The new architecture's payoff in one test: filling a
        // r=64 sphere (~1.1 M voxels) with the cluster's bulk-fill
        // material costs zero sparse-map entries — only state bits
        // flip. Old HashMap design would have allocated ~1.1 M
        // entries here.
        let mut c = Cluster::uniform(solid_grey());
        let v = Voxel::new(VoxelState::Solid, CornerVector::DEFAULT, nonempty_material());
        let centre = 128i32;
        let r2 = 64i32 * 64i32;
        let mut count = 0;
        for z in 0..256i32 {
            let dz = z - centre;
            for y in 0..256i32 {
                let dy = y - centre;
                for x in 0..256i32 {
                    let dx = x - centre;
                    if dx * dx + dy * dy + dz * dz < r2 {
                        c.set(coord(x as u32, y as u32, z as u32), v);
                        count += 1;
                    }
                }
            }
        }
        assert!(count > 1_000_000, "sphere fill should touch >1M voxels");
        assert_eq!(
            c.override_count(),
            0,
            "uniform-material fills must not allocate sparse entries"
        );
    }

    #[test]
    fn debug_does_not_dump_overrides() {
        let mut c = Cluster::empty();
        let v = Voxel::new(
            VoxelState::Solid,
            CornerVector::from_bytes([1, 1, 1]),
            nonempty_material(),
        );
        for i in 0..1000u32 {
            let x = i % 256;
            let y = i / 256;
            c.set(coord(x, y, 0), v);
        }
        assert_eq!(c.override_count(), 1000);
        let s = format!("{:?}", c);
        assert!(s.contains("surface_override_count"));
        assert!(s.contains("1000"));
        // Length should be small regardless of override count.
        assert!(s.len() < 400, "Debug too verbose: {} bytes", s.len());
    }
}
