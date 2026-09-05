//! `bake_island` — a headless bake tool: contour the Prism Test Room's 3×3
//! cluster field and write the nine LOD-0 bakes. No GPU, no window — the
//! data-generation half of the pipeline. Two terrain sets, one tool:
//!
//! ```text
//! cargo run -p flicker-voxel --bin bake_island          # island dome → bakes_island/
//! cargo run -p flicker-voxel --bin bake_island -- wave  # wave field  → bakes/
//! ```
//!
//! The default bakes the procedural ISLAND dome ([`flicker_voxel::heightmap::
//! island_height`], built as a [`flicker_voxel::HeightField::island`]) into
//! `Alpha/content/package/bakes_island/cluster_{x}_0_{z}.json.gz` (the
//! gz-at-rest form emitted by [`flicker_core::compression::write_bytes`]),
//! which the pocclusters loader reads on startup. `wave` bakes the seeded
//! wave field ([`flicker_voxel::HeightField::new`] under `FLICKER_SEED`,
//! default seed otherwise) into `bakes/`, which the csgtest loader reads.
//! Each cell samples the SAME global surface function at its own world
//! offset, so cross-cluster seams are continuous for free.

use std::path::PathBuf;

use flicker_voxel::{
    contour,
    heightmap::{island_height, seed_from_env},
    BakedCluster, ClusterId, HeightField, Material,
};

/// Cluster count per axis — the 3×3 Prism Test Room field (`FIELD_DIM` in
/// flicker-pocclusters).
const FIELD_DIM: u16 = 3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `wave` bakes the seeded wave field into `bakes/`; the default bakes
    // the island dome into `bakes_island/`.
    let wave = std::env::args().nth(1).as_deref() == Some("wave");
    let subdir = if wave { "bakes" } else { "bakes_island" };

    // Same directories the scene loaders resolve through the content-roots
    // service (`roots().package().join(subdir)`). Written relative to
    // this crate so the tool works from any cwd, mirroring the worldengine
    // `bake` bin.
    let out_dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../content/package"
    ))
    .join(subdir);

    // Id 23 = Gravel in the material catalog — one material for the whole
    // field, matching the scenes' live-contour fallbacks.
    let material = Material::new(23, 23, 0);

    let seed = seed_from_env();
    let center = 0.5 * f32::from(FIELD_DIM) * flicker_voxel::CLUSTER_DIM as f32;
    if wave {
        println!("wave field: seed={seed:#x} → {}", out_dir.display());
    } else {
        // A short diagnostic of the terrain the dome bakes, so a human (and
        // step 2, choosing the sea level) can read the numbers without
        // inflating a bake.
        println!(
            "island dome: center=({center},{center}) h={:.2}  flank({center},20) h={:.2}  corner(0,0) h={:.2}",
            island_height(center, center),
            island_height(center, 20.0),
            island_height(0.0, 0.0),
        );
    }

    for x in 0..FIELD_DIM {
        for z in 0..FIELD_DIM {
            let id = ClusterId::new(0, x, 0, z);
            let field = if wave {
                HeightField::new(seed, id.world_offset())
            } else {
                HeightField::island(id.world_offset())
            };
            let cluster = contour(&field, material, id);
            let overrides = cluster.override_count();
            let baked = BakedCluster::from_cluster(id, cluster);
            // `write_bytes` gzips its input and appends `.gz`, so hand it the
            // uncompressed JSON at the logical `.json` path — NOT the already-
            // gzipped `to_disk_bytes()`, which would double-compress.
            let logical = out_dir.join(format!("cluster_{x}_0_{z}.json"));
            let written =
                flicker_core::compression::write_bytes(&logical, baked.to_json()?.as_bytes())?;
            println!(
                "baked cluster ({x},0,{z}) — {overrides} surface overrides → {}",
                written.display()
            );
        }
    }
    Ok(())
}
