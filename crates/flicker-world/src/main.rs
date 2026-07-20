//! flicker-world — the interactive world viewer (name not final).
//!
//! Consolidates the application architecture from `examples/hex-world` and
//! `examples/hex-map` onto the icosahedral grid (`flicker-worldgrid`): a
//! scene-driven app (loading → world) that renders the whole planet as a single
//! sphere mesh, each cell coloured by a world-gen epoch field, under an orbit
//! camera.
//!
//! This is the first slice — the runnable viewer. The Lua UI, logo art, and the
//! richer app shell (menus, settings) are brought in next; the HUD here is plain
//! engine text for now. See `docs/flicker-world-handoff.md`.

mod camera;
mod celestial;
mod color;
mod globe;
mod scene;
mod world;

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // Shared flicker-shell owns the front end (logo → menu → settings → pause +
    // the Prism UI theme, with display persistence). Start → Loading → World.
    flicker_shell::run(flicker_shell::ShellConfig {
        game_scene: Box::new(|| Box::new(scene::Loading::new(world::DEFAULT_FREQ, world::DEFAULT_SEED))),
        settings_dir: Some(env!("CARGO_MANIFEST_DIR").into()),
        game_label: None,
    })
}
