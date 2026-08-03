//! CLI wrapper around the in-app import pipeline — the same `import_folder` the editor calls.
//!
//!   cargo run -p flicker-content --example import_folder -- <source_dir> <out_dir> <AssetName> [reference.json]
//!
//! e.g. bake the human base into the characters tree:
//!   cargo run -p flicker-content --example import_folder -- \
//!     Alpha/content/source/PrismRaces/HumanBaseA_Low Alpha/content/package/characters/HumanBaseA HumanBaseA

use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: import_folder <source_dir> <out_dir> <AssetName> [reference.json]");
        std::process::exit(2);
    }
    let reference: PathBuf = args.get(4).map(PathBuf::from).unwrap_or_else(flicker_content::default_reference);
    let summary = flicker_content::import_folder(Path::new(&args[1]), Path::new(&args[2]), &args[3], &reference)?;
    println!(
        "baked {} — {} bones, {} tris; textures {:?} (from {})",
        summary.rig_path.display(),
        summary.bones,
        summary.tris,
        summary.textures,
        summary.source_fbx.display(),
    );
    Ok(())
}
