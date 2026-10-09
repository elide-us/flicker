//! CLI: (re)generate a PATTERN's reference skeleton into `package/skeletons/<Pattern>/` — by default `Humanoid`, the canon
//! canon — into the package characters tree. Run after editing the authored tables in
//! `baseline.rs`, or to emit a RECIPE's skeleton as the retarget target for baking a clip
//! library onto that recipe's rest:
//!
//!   cargo run -p flicker-content --example bake_baseline
//!   cargo run -p flicker-content --example bake_baseline -- --pattern Quadruped
//!
//! Emits `Alpha/content/package/skeletons/<Pattern>/<Pattern>.json` (gz at rest) and, for a
//! creature pattern (Quadruped · Bird · Bat), the pattern's DEFAULT CONTROLLER beside it —
//! `<Pattern>.pack.json`, every state a generated gait (`baseline::default_pack`, G5). The
//! baseline lint tests are the acceptance gate — run them first.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let skeletons =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/package/skeletons");
    let args: Vec<String> = std::env::args().collect();
    let pattern = args
        .iter()
        .position(|a| a == "--pattern")
        .and_then(|i| args.get(i + 1))
        .map(|name| {
            flicker_content::baseline::Pattern::from_name(name)
                .ok_or_else(|| anyhow::anyhow!("unknown skeleton pattern `{name}`"))
        })
        .transpose()?
        .unwrap_or(flicker_content::baseline::Pattern::Humanoid);
    let out = flicker_content::baseline::emit_pattern(
        &skeletons,
        pattern,
        flicker_content::baseline::STATURE,
    )?;
    let bones = flicker_content::baseline::compose(
        &flicker_content::baseline::reference_recipe(pattern),
        flicker_content::baseline::STATURE,
    )?
    .len();
    println!(
        "baked {} — {} bones at {} cm stature",
        out.display(),
        bones,
        flicker_content::baseline::STATURE
    );
    if let Some(pack) = flicker_content::baseline::default_pack(pattern) {
        println!(
            "wrote the pattern's default controller {} — {} generated states",
            out.with_file_name(format!("{}.pack.json", pattern.name()))
                .display(),
            pack.state_machine.states.len()
        );
    }
    Ok(())
}
