//! flicker-pocepochs — the **application for the forward-regenerative epoch
//! simulation**.
//!
//! (Replaces the old single-hex retained layer-map POC; its water-cycle nucleus
//! `layers.rs` is re-homed in `flicker_worldgen::water`.) A thin [`flicker_shell`]
//! client (intro splash → menu → *this scene* → pause/settings) whose in-game
//! [`WorldScene`] drives a [`flicker_worldengine::WorldEngine`]: it scrubs the nine
//! epochs, tweaks levers, and reseeds, regenerating forward on demand. This slice
//! sets up the app + the engine binding + an orbit camera + a stats HUD; the
//! icosphere globe render and the consolidated timeline UI (the `flicker-widgets`
//! toolkit) land on top.
//!
//! `cargo run -p flicker-pocepochs`.

mod camera;
mod globe;
mod scene;

use anyhow::Result;

use scene::WorldScene;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // The shell owns the front-end + the run loop; START launches the world scene.
    // `settings_dir` puts this app's per-user settings.json in its own project root.
    flicker_shell::run(flicker_shell::ShellConfig {
        game_scene: Box::new(|| Box::new(WorldScene::new())),
        settings_dir: Some(env!("CARGO_MANIFEST_DIR").into()),
        game_label: None,
    })
}
