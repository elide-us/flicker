//! flicker-solarbirth — the **birth of the Prism system**, as a cinematic.
//!
//! Recovered from the old emergent-formation `examples/flicker-solarsystem` by
//! keeping only its two good ideas and throwing the rest away:
//! - the **cinematic camera flight path** — now authored data (`flights/intro.flight`)
//!   played by the **`flicker-flight`** service: opens outside a dust cloud, below
//!   the plane, then rises and glides in to frame Home, and gently coasts;
//! - the **dust-cloud clearing** — the volumetric disk dissipates inside-out and
//!   carves annular gaps at each planet's orbit, in sync with the flight (`scene.rs`).
//!
//! Gone: the random seed, the N-body giant-impact sim, the collision/material
//! ledger, and the habitability export. The system is now **fixed** — the Prism
//! canonical roster (`system.rs`): the sun at the origin, eight planets in the
//! ruled order (Chaos · Fire · Home · Earth · Light · Air · Water · Death), and
//! Home's moon. It always resolves to the same system; the "clearing" is cosmetic.
//!
//! A **flicker-shell client**: START launches the `Sim` scene, Esc opens the pause
//! menu. Controls: drag rotate · wheel zoom · Space replay the fly-in · Esc pause.

mod camera;
mod scene;
mod system;

use anyhow::Result;

use scene::Sim;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // The shell owns the front-end (splash → menu → settings/pause) + the run
    // loop; START launches our fixed-system cinematic scene.
    flicker_shell::run(flicker_shell::ShellConfig {
        game_scene: Box::new(|| Box::new(Sim::new())),
        settings_dir: Some(env!("CARGO_MANIFEST_DIR").into()),
        game_label: None,
    })
}
