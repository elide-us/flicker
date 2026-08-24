//! The island bake pipeline, end to end without touching disk: every cell of
//! the Prism Test Room's 3×3 field contours from the shared island dome
//! (`HeightField::island`, which samples `heightmap::island_height`) to a
//! non-empty LOD-0 cluster that round-trips through the on-disk bake format.
//! This is the primitive→contour→bake path the `bake_island` bin runs.

use flicker_voxel::{
    contour, heightmap::island_height, BakedCluster, ClusterId, HeightField, Material,
};

/// Cluster count per axis — the 3×3 Prism field (`FIELD_DIM` in pocclusters).
const FIELD_DIM: u16 = 3;

#[test]
fn every_island_cell_contours_and_round_trips() {
    let material = Material::new(23, 23, 0); // Gravel
    for x in 0..FIELD_DIM {
        for z in 0..FIELD_DIM {
            let id = ClusterId::new(0, x, 0, z);
            let field = HeightField::island(id.world_offset());
            let cluster = contour(&field, material, id);
            assert!(
                cluster.override_count() > 0,
                "cell ({x},0,{z}) should contour to a non-empty surface"
            );

            // The bytes the bin writes are recoverable bit-for-bit.
            let baked = BakedCluster::from_cluster(id, cluster);
            let bytes = baked.to_disk_bytes().expect("bake serializes");
            let back = BakedCluster::from_bytes(&bytes).expect("bake round-trips");
            assert_eq!(back.id.bits(), id.bits());
            assert_eq!(back.cluster.default_material().raw(), material.raw());
        }
    }
}

#[test]
fn the_island_is_a_dome_in_the_expected_band() {
    // The contract a low sea level turns into an island: dry center peak,
    // field-edge flank flooded under a ~120 waterline, flat seabed at the
    // corner. (The height function is defined in flicker-primitive; pinned here
    // too so the voxel-side pipeline asserts the same shape it bakes.)
    let center = island_height(384.0, 384.0);
    let flank = island_height(384.0, 20.0);
    let corner = island_height(0.0, 0.0);
    assert!(center > 150.0, "center should be a dry peak, got {center}");
    assert!(flank <= 100.0, "field-edge flank should flood, got {flank}");
    assert!(
        (corner - 96.0).abs() < 4.0,
        "corner should be flat seabed, got {corner}"
    );
}
