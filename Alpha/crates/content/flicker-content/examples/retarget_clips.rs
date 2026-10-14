//! CLI: retarget a directory tree of clip sources — Motifect BVH files, or `flicker.rig` clip
//! documents authored on another skeleton (the recovered Katanami library) — onto a target
//! `flicker.rig` skeleton, emitting both variants under `<out_dir>/{In-Place,RootMotion}/`.
//! The in-app port of `tools/retarget_bvh.py`.
//!
//!   cargo run -p flicker-content --example retarget_clips -- <source_dir> <skeleton.json> <out_dir>
//!
//! Sources are found recursively (`.bvh`, `.json`, `.json.gz`) and baked in sorted path order, so
//! a source library laid out as `clips/{In-Place,RootMotion}/…` lets its RootMotion clip win the
//! stem the two trees share. e.g. bake the Katanami library onto the canon:
//!   cargo run -p flicker-content --example retarget_clips -- \
//!     ../PrismContentSource/Katanami/clips \
//!     Alpha/content/package/skeletons/Humanoid/Humanoid.json \
//!     Alpha/content/package/retarget/clips/katanami

use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            collect(&p, out)?;
        } else if p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".bvh") || n.ends_with(".json") || n.ends_with(".json.gz"))
        {
            out.push(p);
        }
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: retarget_clips <source_dir> <skeleton.json> <out_dir>");
        std::process::exit(2);
    }
    let (source_dir, skeleton, out_dir) = (
        Path::new(&args[1]),
        Path::new(&args[2]),
        Path::new(&args[3]),
    );
    let mut entries = Vec::new();
    collect(source_dir, &mut entries)?;
    entries.sort();
    let mut ok = 0usize;
    let mut fail = 0usize;
    for p in &entries {
        match flicker_content::retarget::emit_variants(p, skeleton, out_dir) {
            Ok(_) => ok += 1,
            Err(e) => {
                fail += 1;
                eprintln!("  skip {}: {e}", p.file_name().unwrap().to_string_lossy());
            }
        }
    }
    println!(
        "retargeted {ok} clip(s) ({fail} skipped) onto {} → {}",
        skeleton.display(),
        out_dir.display()
    );
    Ok(())
}
