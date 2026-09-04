//! Render-time LOD derivation: build a coarse-LOD view of a cluster from
//! its LOD-0 source-of-truth data — **without re-contouring**.
//!
//! Per the source-of-truth invariant (`docs/architecture.md`), the LOD-0
//! cluster *is the data*. Coarser LODs for rendering are *derived* from it,
//! never re-contoured: contouring (primitive evaluation + QEF solve) is a
//! bake/edit-time activity, and the dual vertices it produced already live
//! in the cluster at LOD 0.
//!
//! There is **no per-LOD logic** here. A LOD is just an integer; the only
//! quantity it contributes is `stride = 2^LOD` (and `sample_dim =
//! 256 >> LOD`). One code path serves every level.
//!
//! # What it does
//!
//! - **State** is copied verbatim — voxel solidity is LOD-independent, so
//!   the coarse view shares the LOD-0 dense [`StateField`]. The mesher
//!   reads it at the coarse stride to decide cell activeness, exactly as it
//!   would for a contoured-at-LOD cluster.
//! - **Corners** are condensed: every LOD-0 dual vertex is binned into the
//!   coarse cell (`stride³` voxels) that **owns** it (by its storing
//!   voxel's coord — the decoded position may reach outside); each coarse cell's
//!   vertex is the average of the LOD-0 vertices in its footprint. That
//!   average is re-encoded cell-relative to the coarse cell and written
//!   across the footprint — byte-shaped exactly as [`crate::contour`] would
//!   write it at this LOD, so [`crate::mesh`] consumes the result unchanged.
//!
//! Averaging is the v1 condensation; pure sub-sampling (take one footprint
//! vertex) is an alternative quality lever and would slot in at the same
//! point with no other change.

use std::collections::HashMap;

use clayengine::CLUSTER_DIM;

use crate::cluster::Cluster;
use crate::corner_vector::CornerVector;
use crate::local_coord::LocalCoord;
use crate::lod::Lod;
use crate::voxel::Voxel;

/// Derive a `target`-LOD view of `source` (which must hold LOD-0 data).
///
/// The returned cluster carries `source`'s dense state verbatim and one
/// dual vertex per active coarse cell, aggregated from the LOD-0 corners in
/// that cell's `stride³` footprint. No primitive is evaluated and no QEF is
/// solved — this is a pure read of data already present at LOD 0.
///
/// At `target == Lod::ZERO` the result is an independent copy of `source`
/// (`Cluster` is intentionally non-`Clone`, so callers that already hold the
/// LOD-0 cluster should mesh it directly rather than copy through here).
#[must_use]
pub fn derive_lod(source: &Cluster, target: Lod) -> Cluster {
    let stride = target.stride() as usize;
    let voxel_dim = CLUSTER_DIM as usize;

    // Solidity is LOD-independent — the coarse view shares the LOD-0 dense
    // state field verbatim. The mesher samples it at the coarse stride.
    let mut coarse = Cluster::empty();
    coarse.replace_state_field(source.state_field().clone());
    coarse.set_default_material(source.default_material());

    if stride == 1 {
        // LOD-0 target: the source already is the data. Copy its corners
        // through so the result is a standalone equal cluster.
        for (coord, voxel) in source.overrides() {
            coarse.set(coord, voxel);
        }
        return coarse;
    }

    // Bin every LOD-0 dual vertex into its coarse cell. Key = coarse-cell
    // sample index; value = (sum of absolute LOD-0 vertex positions, count).
    // Absolute position is `voxel + corner` because LOD-0 corners are encoded
    // at stride 1 (cell-units == voxel-units).
    let mut accum: HashMap<(usize, usize, usize), ([f32; 3], u32)> = HashMap::new();
    for (coord, voxel) in source.overrides() {
        let corner = voxel.corner();
        if corner == CornerVector::DEFAULT {
            continue;
        }
        let [dx, dy, dz] = corner.to_components();
        let abs = [
            coord.x() as f32 + dx,
            coord.y() as f32 + dy,
            coord.z() as f32 + dz,
        ];
        let cell = (
            coord.x() as usize / stride,
            coord.y() as usize / stride,
            coord.z() as usize / stride,
        );
        let entry = accum.entry(cell).or_insert(([0.0; 3], 0));
        entry.0[0] += abs[0];
        entry.0[1] += abs[1];
        entry.0[2] += abs[2];
        entry.1 += 1;
    }

    // For each active coarse cell, encode the averaged vertex cell-relative
    // to the coarse stride and expand it across the footprint (∩ cluster
    // bounds) so cross-LOD seam reads at any footprint voxel resolve to it —
    // mirrors `contour`'s footprint expansion.
    let stride_f = stride as f32;
    for ((ci, cj, ck), (sum, count)) in &accum {
        let n = *count as f32;
        let cell_min = [
            (ci * stride) as f32,
            (cj * stride) as f32,
            (ck * stride) as f32,
        ];
        let corner = CornerVector::from_components(
            (sum[0] / n - cell_min[0]) / stride_f,
            (sum[1] / n - cell_min[1]) / stride_f,
            (sum[2] / n - cell_min[2]) / stride_f,
        );

        let base = [ci * stride, cj * stride, ck * stride];
        for dz in 0..stride {
            let vz = base[2] + dz;
            if vz >= voxel_dim {
                break;
            }
            for dy in 0..stride {
                let vy = base[1] + dy;
                if vy >= voxel_dim {
                    break;
                }
                for dx in 0..stride {
                    let vx = base[0] + dx;
                    if vx >= voxel_dim {
                        break;
                    }
                    let coord = LocalCoord::new(vx as u32, vy as u32, vz as u32).expect("in range");
                    let existing = coarse.get(coord);
                    coarse.set(
                        coord,
                        Voxel::new(existing.state(), corner, existing.material()),
                    );
                }
            }
        }
    }

    coarse
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster_id::ClusterId;
    use crate::contour::contour;
    use crate::material::Material;
    use crate::mesh::mesh;
    use crate::neighbor::NeighborContext;
    use crate::voxel_state::VoxelState;
    use flicker_primitive::FlatField;

    fn grey() -> Material {
        Material::new(1, 1, 0)
    }

    fn lod0_flat() -> Cluster {
        contour(&FlatField::at_half(), grey(), ClusterId::new(0, 0, 0, 0))
    }

    /// State is LOD-independent: the derived coarse cluster reports the same
    /// per-voxel solidity as the LOD-0 source at every voxel.
    #[test]
    fn derived_state_matches_source_everywhere() {
        let src = lod0_flat();
        let coarse = derive_lod(&src, Lod::new(2).unwrap());
        for &(x, y, z) in &[
            (0u32, 0u32, 0u32),
            (100, 120, 30),
            (255, 255, 255),
            (10, 200, 250),
        ] {
            let c = LocalCoord::new(x, y, z).unwrap();
            assert_eq!(
                coarse.get(c).state(),
                src.get(c).state(),
                "state drift at ({x},{y},{z})"
            );
        }
    }

    /// The derived coarse vertex lands on the surface plane: a flat field at
    /// y=128, derived to LOD 2, must place its coarse cells' vertices at
    /// world y≈128 (decoded `cell_min + corner·stride`).
    #[test]
    fn derived_corner_lands_on_surface_plane() {
        let src = lod0_flat();
        let stride = 4_i32; // LOD 2
        let coarse = derive_lod(&src, Lod::new(2).unwrap());
        // The coarse cell straddling y=128 has min-corner sample 31 → voxel
        // y=124. Its stored corner decodes to y ≈ 128.
        let c = LocalCoord::new(0, 124, 0).unwrap();
        let v = coarse.get(c);
        assert_ne!(
            v.corner(),
            CornerVector::DEFAULT,
            "coarse surface cell must carry a derived vertex"
        );
        let [_dx, dy, _dz] = v.corner().to_components();
        let world_y = 124.0 + dy * stride as f32;
        assert!(
            (world_y - 128.0).abs() < 1.0,
            "derived coarse vertex y={world_y} should sit on the plane"
        );
    }

    /// End-to-end: meshing the *derived* coarse cluster yields the same
    /// surface (same triangle count, same kind of geometry) as meshing a
    /// cluster *contoured directly at that LOD* — proving derive_lod is a
    /// drop-in for the re-contour path the mesher used to depend on.
    #[test]
    fn derived_mesh_matches_recontoured_mesh_triangle_count() {
        let recontoured = contour(&FlatField::at_half(), grey(), ClusterId::new(2, 0, 0, 0));
        let derived = derive_lod(&lod0_flat(), Lod::new(2).unwrap());
        let none = NeighborContext::none();
        let lod2 = Lod::new(2).unwrap();
        let m_recontoured = mesh(&recontoured, &none, lod2);
        let m_derived = mesh(&derived, &none, lod2);
        assert!(!m_derived.is_empty(), "derived mesh should be non-empty");
        assert_eq!(
            m_derived.triangle_count(),
            m_recontoured.triangle_count(),
            "derived LOD-2 mesh should have the same triangle count as the re-contoured one"
        );
    }

    /// A uniform (no-override) cluster derives to an empty-surface coarse
    /// cluster: state carries over, no spurious corners.
    #[test]
    fn uniform_source_derives_no_corners() {
        let mut src = Cluster::empty();
        // Fill the lower half solid via state only (no overrides).
        for z in 0..CLUSTER_DIM {
            for y in 0..128u32 {
                for x in 0..CLUSTER_DIM {
                    src.set_state(LocalCoord::new(x, y, z).unwrap(), VoxelState::Solid);
                }
            }
        }
        let coarse = derive_lod(&src, Lod::new(3).unwrap());
        assert_eq!(
            coarse.override_count(),
            0,
            "no LOD-0 corners → no coarse corners"
        );
        // State still present.
        assert_eq!(
            coarse.get(LocalCoord::new(0, 0, 0).unwrap()).state(),
            VoxelState::Solid
        );
    }
}
