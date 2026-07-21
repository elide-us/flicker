//! flicker-poc-chemistry — the chemistry-first world sim, as a flicker-shell app.
//!
//! A thin shell client: intro splash → menu → *the chemistry scene* → pause /
//! settings. The **simulation runs on its own thread** ([`sim_thread`], spec §11 —
//! detached from the renderer); the scene sends it commands and draws the latest
//! snapshot, never stepping the sim inside a frame. The sim itself lives in the
//! crate library (`lib.rs`) and needs no renderer.
//!
//! `cargo run -p flicker-poc-chemistry`.

mod camera;
mod globe;
mod scene;
mod sim_thread;

use anyhow::Result;

use scene::ChemScene;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // The shell owns the front-end + run loop; START launches the chemistry scene.
    // `settings_dir` keeps this app's per-user settings.json in its own root.
    flicker_shell::run(flicker_shell::ShellConfig::single(
        Some(env!("CARGO_MANIFEST_DIR").into()),
        None,
        || Box::new(ChemScene::new()),
    ))
}
