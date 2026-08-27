//! CLI wrapper around the in-app PROP bake — the same `parse_fbx` + `write_prop` the Clayworks
//! bench Commit calls for a boneless static prop. The character analog is `import_folder`.
//!
//!   cargo run -p flicker-content --example import_prop -- <source.fbx> <out_dir> <AssetName>
//!
//! e.g. bake a grass tuft into the props tree (lands as <out_dir>/<AssetName>.json.gz):
//!   cargo run -p flicker-content --example import_prop -- \
//!     Alpha/content/source/Environment/Grass.fbx Alpha/content/staging/props/Grass-Medium Grass-Medium
//!
//! Geometry is emitted RAW in the engine's space (`parse_fbx` normalises to Z-up centimetres); the
//! socket `Fit` is left default (an environment prop mounts to no bone).
//!
//! POC flat colour: untextured Synty foliage carries no PNG maps, so its flat colour would be lost
//! through the stock prop bake (placeholder "steel"). We read the FBX material's base colour and
//! pass it as `flat_color`. This is TEMPORARY and conflicts with the materials-unification project
//! (see `bake_prop`); it exists only so these POC environment props keep their look.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: import_prop <source.fbx> <out_dir> <AssetName>");
        std::process::exit(2);
    }
    let fbx = Path::new(&args[1]);
    let out_dir = Path::new(&args[2]);
    let name = &args[3];
    std::fs::create_dir_all(out_dir)?;
    let out = out_dir.join(format!("{name}.json"));

    let model = flicker_content::parse_fbx(fbx)?;
    let flat_color = flicker_content::first_material_color(fbx);
    flicker_content::write_prop(
        &model,
        fbx,
        name,
        &out,
        &flicker_content::Fit::default(),
        flat_color,
    )?;

    // Report bounding size in cm so the caller can eyeball the real-world scale.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &model.vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(v.p[i]);
            hi[i] = hi[i].max(v.p[i]);
        }
    }
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    println!(
        "baked {} — {} verts, size {:.1} x {:.1} x {:.1} cm, flat_color {:?} (from {})",
        out.display(),
        model.vertices.len(),
        size[0],
        size[1],
        size[2],
        flat_color,
        fbx.display(),
    );
    Ok(())
}
