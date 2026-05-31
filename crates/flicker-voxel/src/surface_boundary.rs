//! Surface-boundary predicate and centroid-wireframe segments.
//!
//! A voxel **participates in the surface** iff at least one of its six
//! axis-aligned neighbors has a different solid/non-solid
//! classification. This is the same condition under which the contour
//! pass emits a quad touching that voxel's owned corner.
//!
//! The two surfaces this module exposes:
//!
//! - [`is_surface_boundary_voxel`] — the per-voxel predicate. Used as
//!   the keep-decision for sparse-octree storage (a non-boundary voxel
//!   is collapsible without affecting the mesh) and as the gate for
//!   the centroid-wireframe view.
//! - [`surface_boundary_segments`] — walks the cluster and yields one
//!   `(centroid, owned_join)` pair per surface-boundary voxel, in
//!   cluster-local voxel-unit coordinates. The example uses this to
//!   draw the centroid wireframe — a diagnostic overlay where each
//!   line traces from a voxel's center to its owned `+++` corner. The
//!   line cluster on one side of a cluster seam should meet the
//!   cluster on the other side continuously, because both sides
//!   sample the same global heightmap at the same world coordinates.
//!
//! # No cross-cluster gates
//!
//! The Pass-D code in history also exposed a `voxel_owns_boundary_wall`
//! gate that picked which side of an inter-cluster cliff emitted its
//! vertical face. That gate is **deliberately not part of the new
//! architecture** — the contour pass operates on one cluster and knows
//! nothing about its neighbors, so there is no cross-cluster boundary
//! wall to attribute. Cross-cluster correctness comes from upstream
//! (both sides sample the same heightmap), not from a seam-time gate.
//! Do not re-introduce that function here.
//!
//! This module operates at **LOD 0 only**. Strided sampling lived in
//! the Pass-D file but is not needed until LOD work resumes in a
//! later step.

use crate::cluster::{Cluster, CLUSTER_DIM};
use crate::local_coord::LocalCoord;
use crate::material::Material;
use crate::voxel::Voxel;

/// One `(centroid, owned_join)` pair in cluster-local coordinates, in
/// voxel units (`0..CLUSTER_DIM` per axis). The two `[f32; 3]`s are the
/// endpoints of the centroid-wireframe line segment for one
/// surface-boundary voxel.
pub type SurfaceBoundarySegment = ([f32; 3], [f32; 3]);

/// `true` iff two voxels' solid/non-solid classifications differ. The
/// trigger for both face emission and the keep-predicate below;
/// exposed here so callers describing the same comparison can name it.
#[inline]
#[must_use]
pub const fn classification_differs(self_solid: bool, neighbor_solid: bool) -> bool {
    self_solid != neighbor_solid
}

#[inline]
fn is_voxel_solid(v: Voxel) -> bool {
    v.material() != Material::EMPTY
}

/// `true` iff the voxel at `coord` participates in the surface — i.e.
/// at least one of its six axis-aligned neighbors has a different
/// classification. Out-of-cluster neighbor reads fall back to the
/// cluster's base voxel, matching the no-neighbor-context behavior of
/// the single-cluster contour pass.
#[must_use]
pub fn is_surface_boundary_voxel(cluster: &Cluster, coord: LocalCoord) -> bool {
    let base_solid = is_voxel_solid(cluster.base());
    let dim_i32 = CLUSTER_DIM as i32;

    let solid_at = |x: i32, y: i32, z: i32| -> bool {
        if (0..dim_i32).contains(&x) && (0..dim_i32).contains(&y) && (0..dim_i32).contains(&z) {
            is_voxel_solid(
                cluster.get(LocalCoord::new(x as u32, y as u32, z as u32).expect("in bounds")),
            )
        } else {
            base_solid
        }
    };

    let x = coord.x() as i32;
    let y = coord.y() as i32;
    let z = coord.z() as i32;
    let self_solid = solid_at(x, y, z);
    classification_differs(self_solid, solid_at(x - 1, y, z))
        || classification_differs(self_solid, solid_at(x + 1, y, z))
        || classification_differs(self_solid, solid_at(x, y - 1, z))
        || classification_differs(self_solid, solid_at(x, y + 1, z))
        || classification_differs(self_solid, solid_at(x, y, z - 1))
        || classification_differs(self_solid, solid_at(x, y, z + 1))
}

/// Walk every voxel in `cluster` and yield a `(centroid, owned_join)`
/// segment for each voxel that [`is_surface_boundary_voxel`] flags.
///
/// - `centroid` is the voxel's center: `(x + 0.5, y + 0.5, z + 0.5)`
///   in cluster-local voxel units.
/// - `owned_join` is the voxel's owned `+++` corner with its
///   `CornerVector` applied: `(x + cv_x, y + cv_y, z + cv_z)`.
///
/// Returned in `(z, y, x)` iteration order so calls with the same
/// cluster produce byte-identical output.
///
/// # Performance
///
/// Pre-classifies every voxel in one `O(CLUSTER_DIM³)` scan, then
/// performs six array reads per boundary check. With `CLUSTER_DIM =
/// 256` the scan is ~16 M classifications — a few hundred ms in debug,
/// well under 100 ms in release. Acceptable for one-shot diagnostic
/// overlay use; not designed for per-frame regeneration.
#[must_use]
pub fn surface_boundary_segments(cluster: &Cluster) -> Vec<SurfaceBoundarySegment> {
    let dim = CLUSTER_DIM as usize;
    let row_stride = dim;
    let slab_stride = dim * dim;
    let voxel_idx =
        |x: usize, y: usize, z: usize| -> usize { x + y * row_stride + z * slab_stride };

    // Pre-classify every voxel; per-voxel neighbor check then becomes
    // six array reads. Matches the contour pass's pre-classification
    // strategy and keeps the inner loop tight.
    let mut is_solid = vec![false; dim * dim * dim];
    for z in 0..CLUSTER_DIM {
        for y in 0..CLUSTER_DIM {
            for x in 0..CLUSTER_DIM {
                let v = cluster.get(LocalCoord::new(x, y, z).expect("in bounds"));
                is_solid[voxel_idx(x as usize, y as usize, z as usize)] = is_voxel_solid(v);
            }
        }
    }
    let base_solid = is_voxel_solid(cluster.base());
    let dim_i32 = CLUSTER_DIM as i32;
    let lookup = |x: i32, y: i32, z: i32| -> bool {
        if (0..dim_i32).contains(&x) && (0..dim_i32).contains(&y) && (0..dim_i32).contains(&z) {
            is_solid[voxel_idx(x as usize, y as usize, z as usize)]
        } else {
            base_solid
        }
    };

    let mut segments = Vec::new();
    for z in 0..dim_i32 {
        for y in 0..dim_i32 {
            for x in 0..dim_i32 {
                let self_solid = lookup(x, y, z);
                let boundary = classification_differs(self_solid, lookup(x - 1, y, z))
                    || classification_differs(self_solid, lookup(x + 1, y, z))
                    || classification_differs(self_solid, lookup(x, y - 1, z))
                    || classification_differs(self_solid, lookup(x, y + 1, z))
                    || classification_differs(self_solid, lookup(x, y, z - 1))
                    || classification_differs(self_solid, lookup(x, y, z + 1));
                if !boundary {
                    continue;
                }
                let voxel =
                    cluster.get(LocalCoord::new(x as u32, y as u32, z as u32).expect("in bounds"));
                let [dx, dy, dz] = voxel.corner().to_components();
                let centroid = [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5];
                let owned_join = [x as f32 + dx, y as f32 + dy, z as f32 + dz];
                segments.push((centroid, owned_join));
            }
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corner_vector::CornerVector;

    fn solid_voxel() -> Voxel {
        Voxel::new(CornerVector::DEFAULT, Material::new(1, 0, 0).unwrap())
    }

    fn coord(x: u32, y: u32, z: u32) -> LocalCoord {
        LocalCoord::new(x, y, z).expect("in-range")
    }

    /// Solid half-space `y < 128`. The boundary plane is between
    /// `y = 127` (solid) and `y = 128` (empty).
    fn build_plane_cluster() -> Cluster {
        let mut c = Cluster::empty();
        let v = solid_voxel();
        for z in 0..CLUSTER_DIM {
            for y in 0..128u32 {
                for x in 0..CLUSTER_DIM {
                    c.set(coord(x, y, z), v);
                }
            }
        }
        c
    }

    #[test]
    fn empty_cluster_has_no_boundary_segments() {
        let c = Cluster::empty();
        assert!(surface_boundary_segments(&c).is_empty());
    }

    #[test]
    fn classification_differs_is_xor() {
        assert!(classification_differs(true, false));
        assert!(classification_differs(false, true));
        assert!(!classification_differs(true, true));
        assert!(!classification_differs(false, false));
    }

    #[test]
    fn plane_marks_only_the_two_surface_rows() {
        let c = build_plane_cluster();
        // Interior solid: every neighbour also solid — not a boundary.
        assert!(!is_surface_boundary_voxel(&c, coord(128, 64, 128)));
        // Interior empty: every neighbour also empty — not a boundary.
        assert!(!is_surface_boundary_voxel(&c, coord(128, 192, 128)));
        // Just below the plane: +y neighbour is empty — boundary.
        assert!(is_surface_boundary_voxel(&c, coord(128, 127, 128)));
        // Just above the plane: -y neighbour is solid — boundary.
        assert!(is_surface_boundary_voxel(&c, coord(128, 128, 128)));
    }

    #[test]
    fn segments_centroid_is_voxel_center() {
        let c = build_plane_cluster();
        let segments = surface_boundary_segments(&c);
        // Look up the segment for voxel (128, 127, 128) by its centroid.
        // The default corner-vector encoding `128` decodes to ~0.5039
        // (quantization on a 256-bucket axis), so use a tolerance that
        // accommodates that.
        let plane_seg = segments
            .iter()
            .find(|(centroid, _)| {
                (centroid[0] - 128.5).abs() < 0.01
                    && (centroid[1] - 127.5).abs() < 0.01
                    && (centroid[2] - 128.5).abs() < 0.01
            })
            .expect("voxel (128, 127, 128) is surface-boundary and should appear");
        let (_, join) = plane_seg;
        assert!((join[0] - (128.0 + 0.5)).abs() < 0.01);
        assert!((join[1] - (127.0 + 0.5)).abs() < 0.01);
        assert!((join[2] - (128.0 + 0.5)).abs() < 0.01);
    }

    #[test]
    fn segments_owned_join_tracks_corner_vector() {
        // A single solid voxel at (10, 20, 30) with a non-default
        // corner vector. Its owned-join endpoint should reflect that
        // corner vector, not the default.
        let mut c = Cluster::empty();
        let cv = CornerVector::from_components(0.25, 0.75, -0.1);
        let v = Voxel::new(cv, Material::new(1, 0, 0).unwrap());
        c.set(coord(10, 20, 30), v);

        let [dx, dy, dz] = cv.to_components();
        let segments = surface_boundary_segments(&c);
        let seg = segments
            .iter()
            .find(|(centroid, _)| {
                (centroid[0] - 10.5).abs() < 0.01
                    && (centroid[1] - 20.5).abs() < 0.01
                    && (centroid[2] - 30.5).abs() < 0.01
            })
            .expect("the solid voxel should be a boundary");
        let (_, join) = seg;
        assert!((join[0] - (10.0 + dx)).abs() < 0.005);
        assert!((join[1] - (20.0 + dy)).abs() < 0.005);
        assert!((join[2] - (30.0 + dz)).abs() < 0.005);
    }

    #[test]
    fn out_of_range_predicate_is_safe_via_clamp() {
        // is_surface_boundary_voxel won't actually be called with
        // out-of-range coords (LocalCoord rejects them), but the inner
        // out-of-range neighbor lookup must not panic. Check a
        // boundary-cell coord where neighbor reads go out of bounds.
        let c = build_plane_cluster();
        // (0, 0, 0): -x, -y, -z neighbors all go out of bounds.
        let _ = is_surface_boundary_voxel(&c, coord(0, 0, 0));
        let _ = is_surface_boundary_voxel(&c, coord(255, 255, 255));
    }

    #[test]
    fn segments_are_deterministic() {
        let c = build_plane_cluster();
        let a = surface_boundary_segments(&c);
        let b = surface_boundary_segments(&c);
        assert_eq!(a, b);
    }

    #[test]
    fn segments_non_empty_for_heightmap_terrain() {
        use crate::generators::heightmap_terrain;
        let c = heightmap_terrain(0x42, Material::new(7, 7, 7).unwrap());
        let segs = surface_boundary_segments(&c);
        assert!(
            !segs.is_empty(),
            "heightmap terrain should produce boundary segments"
        );
        // Order of magnitude check: the surface is roughly 256² ≈ 65K
        // top voxels plus their below-neighbors and the +/-Y neighbors,
        // so segment count should be at least ~50K.
        assert!(
            segs.len() > 50_000,
            "expected substantial segment count; got {}",
            segs.len()
        );
    }
}
