//! flicker-sol2 — a viewer for **supernova ejecta → star-system formation**.
//!
//! A star forms from its local cloud. The supernova that seeded that cloud cast material outward;
//! in this (deliberately simple) model each element settles at a characteristic *distance* set by
//! its **atomic weight** — heavier elements fall short, lighter ones reach far.
//!
//! **Phase 1 (Distribution):** a 2D top-down projection of that cloud — the star at the origin,
//! xyz arrows, one translucent colour ring per Prism element at its cast distance, clumpy and
//! differentially sheared (`flicker-system`), with overdensity **dots** marking where matter
//! concentrates. **Phase 2 (Collapse):** the cloud ignites into a planetary system that accretes
//! into planets, moons and rings, with the habitable world highlighted.
//!
//! The app is scene-driven (Logo splash → Menu → Sim, with Pause / Settings overlays). The sim's
//! every control + readout lives in a Lua HUD (`scripts/sim_ui.lua` + `ui_elements.json`): a
//! bottom-right control panel and a top-right stats overlay. Drag the dials; Esc opens Pause.

mod draw;
mod scene;
mod shell;
mod well;

use anyhow::Result;
use flicker::app::run;
use flicker::scene::SceneManager;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // Logo splash → Menu; Start → the Stage-1 simulation; Esc in-sim → Pause overlay.
    run(SceneManager::new(Box::new(shell::Logo::new())))?;
    Ok(())
}
