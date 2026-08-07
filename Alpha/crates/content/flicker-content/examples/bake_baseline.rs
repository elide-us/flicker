//! CLI: (re)generate the AUTHORED baseline skeleton — `GolemBaseSkeleton` — into the
//! package characters tree. Run after editing the authored table in `baseline.rs`:
//!
//!   cargo run -p flicker-content --example bake_baseline
//!
//! Emits `Alpha/content/package/characters/GolemBaseSkeleton/GolemBaseSkeleton.json`
//! (gz at rest). The baseline lint tests are the acceptance gate — run them first.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let characters = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content/package/characters");
    let out = flicker_content::baseline::emit(&characters)?;
    let rig = flicker_content::baseline::golem_base_skeleton();
    println!(
        "baked {} — {} bones, A-pose {} cm stature",
        out.display(),
        rig.skeleton.bones.len(),
        flicker_content::baseline::STATURE
    );
    Ok(())
}
