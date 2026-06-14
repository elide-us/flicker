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
mod color;
mod globe;
mod scene;
mod settings;
mod shell;
mod world;

use anyhow::Result;
use flicker::app::run;
use flicker::scene::SceneManager;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    settings::load_global(); // load settings.json into the live settings
    // Logo splash → Menu; Start → Loading → World, Esc in-world → Pause overlay.
    run(SceneManager::new(Box::new(shell::Logo::new())))?;
    Ok(())
}
