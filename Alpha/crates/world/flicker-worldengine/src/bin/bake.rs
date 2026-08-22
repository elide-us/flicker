//! `bake` — a headless world-bake tool: generate a planet at the Earth-like
//! defaults and write it as a `.epoch` capture. No GPU, no window.
//!
//! ```text
//! cargo run -p flicker-worldengine --bin bake                 # default freq/seed → Alpha/content/package/epochs/earthlike.epoch
//! cargo run -p flicker-worldengine --bin bake -- 12 42 out.epoch
//! ```
//! Args (all optional, positional): `freq seed out_path`.

use flicker_worldengine::WorldEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let freq: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let seed: Option<u64> = args.next().and_then(|s| s.parse().ok());
    let out = args.next().unwrap_or_else(|| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../Alpha/content/package/epochs/earthlike.epoch.gz"
        )
        .to_string()
    });

    let mut engine = WorldEngine::from_repo()?;
    engine.set_freq(freq);
    if let Some(s) = seed {
        engine.set_seed(s);
    }

    let file = engine.capture(format!(
        "Earth-like sample world — freq {freq}, seed {:#x}. Regenerate with `cargo run -p flicker-worldengine --bin bake`.",
        engine.config().seed
    ));
    file.save(&out)?;

    // A short human summary of what was baked.
    let last = file.snapshots.last().expect("nine snapshots");
    println!(
        "baked {} cells × {} epochs → {out}\n  bulk mass conserved: {:.3}\n  plates: {}  watersheds: {}",
        last.len(),
        file.snapshots.len(),
        last.provenance.conserved_mass,
        last.plates.len(),
        last.watersheds.len(),
    );
    Ok(())
}
