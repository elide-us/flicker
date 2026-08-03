//! Inspect what a RAW source FBX actually contains, BEFORE any rename/conform — the diagnostic
//! the guided importer needs when a new source's rig is unknown ("does this export even carry an
//! armature, and what are its bone names?").
//!
//!   cargo run -p flicker-content --example inspect_fbx -- <file.fbx | folder>

use std::path::{Path, PathBuf};

fn main() -> anyhow::Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: inspect_fbx <file.fbx | folder>");
        std::process::exit(2);
    });
    let path = PathBuf::from(&arg);
    let fbx = if path.is_dir() { first_fbx(&path)? } else { path };

    let m = flicker_content::parse_fbx(&fbx)?;
    let skinned = m.vertices.iter().filter(|v| v.weights.iter().any(|w| *w > 0.0)).count();
    println!("file      {}", fbx.display());
    println!("vertices  {}  ({} tris)", m.vertices.len(), m.vertices.len() / 3);
    println!("skinned   {skinned} vertices carry a non-zero weight");
    println!("bones     {}", m.bones.len());
    for (i, b) in m.bones.iter().enumerate() {
        println!("  [{i:>3}] parent {:>3}  {}", b.parent, b.name);
    }
    Ok(())
}

fn first_fbx(dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().map(|e| e.eq_ignore_ascii_case("fbx")).unwrap_or(false))
        .ok_or_else(|| anyhow::anyhow!("no .fbx in {}", dir.display()))
}
