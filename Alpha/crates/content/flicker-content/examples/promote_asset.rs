//! Headless PROMOTE — the same on-disk effect as the Quartermaster bench's `promote_selected`:
//! move an asset folder from `staging/<rel>/<name>` to the mirrored `package/<rel>/<name>` and
//! append the package manifest row through the canonical [`flicker_content::manifest::append`]
//! primitive. The README blesses a promotion as exactly this — a plain byte move + one manifest
//! row. (History/undo is the bench's concern and is not reproduced here.)
//!
//!   cargo run -p flicker-content --example promote_asset -- <rel> <class> <name>...
//!   e.g. cargo run -p flicker-content --example promote_asset -- \
//!          props/environment prop Grass-Tall Grass-Medium Grass-Short
//!
//! Safe by construction: a missing staging source or an already-occupied package target aborts
//! before any move, so a mis-resolved content root cannot damage the tree.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flicker_content::manifest::{append, ManifestEntry};

/// Move every file under `src` into `dst`, preserving sub-structure (staging & package share one
/// filesystem, so `rename` is a cheap byte move).
fn move_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let p = entry?.path();
        let target = dst.join(p.file_name().expect("dir entry has a name"));
        if p.is_dir() {
            move_tree(&p, &target)?;
        } else {
            std::fs::rename(&p, &target)
                .with_context(|| format!("moving {} -> {}", p.display(), target.display()))?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: promote_asset <rel> <class> <name>...");
        std::process::exit(2);
    }
    let rel = &args[1];
    let class = &args[2];

    let staging: PathBuf = flicker_content::roots().staging().to_path_buf();
    let package: PathBuf = flicker_content::roots().package().to_path_buf();
    let manifest = package.join("manifest.json");
    println!(
        "staging={} package={}",
        staging.display(),
        package.display()
    );

    for name in &args[3..] {
        let src = staging.join(rel).join(name);
        let dst = package.join(rel).join(name);
        if !src.is_dir() {
            bail!("no staging asset at {}", src.display());
        }
        if dst.exists() {
            bail!(
                "package target already exists at {} (already promoted?)",
                dst.display()
            );
        }
        move_tree(&src, &dst)?;
        std::fs::remove_dir_all(&src)
            .with_context(|| format!("removing emptied staging {}", src.display()))?;
        append(
            &manifest,
            ManifestEntry {
                name: name.clone(),
                class: class.clone(),
                path: format!("package/{rel}/{name}"),
                promoted_from: format!("staging/{rel}/{name}"),
            },
        )?;
        println!("promoted {name}: {} -> {}", src.display(), dst.display());
    }
    Ok(())
}
