//! CLI: retarget a directory of Motifect BVH clips onto a target `flicker.rig` skeleton, emitting
//! both variants under `<out_dir>/{In-Place,RootMotion}/`. The in-app port of `tools/retarget_bvh.py`.
//!
//!   cargo run -p flicker-content --example retarget_clips -- <bvh_dir> <skeleton.json> <out_dir>
//!
//! e.g. re-bake the locomotion library onto HumanBaseA's flat bind:
//!   cargo run -p flicker-content --example retarget_clips -- \
//!     "Alpha/content/source/Motifect/Motifect_locomotion_complete_v1_0/BVH" \
//!     Alpha/content/package/characters/HumanBaseA/HumanBaseA.json \
//!     Alpha/content/package/characters/HumanBaseA/clips/locomotion

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: retarget_clips <bvh_dir> <skeleton.json> <out_dir>");
        std::process::exit(2);
    }
    let (bvh_dir, skeleton, out_dir) = (Path::new(&args[1]), Path::new(&args[2]), Path::new(&args[3]));
    let mut ok = 0usize;
    let mut fail = 0usize;
    let mut entries: Vec<_> =
        std::fs::read_dir(bvh_dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|e| e == "bvh").unwrap_or(false)).collect();
    entries.sort();
    for p in &entries {
        match flicker_content::retarget::emit_variants(p, skeleton, out_dir) {
            Ok(_) => ok += 1,
            Err(e) => {
                fail += 1;
                eprintln!("  skip {}: {e}", p.file_name().unwrap().to_string_lossy());
            }
        }
    }
    println!("retargeted {ok} clip(s) ({fail} skipped) onto {} → {}", skeleton.display(), out_dir.display());
    Ok(())
}
