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
mod well;

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    // Shared flicker-shell owns the front end: Logo splash → Menu → Sim (Start),
    // with Pause / Settings overlays. Esc in-sim → Pause.
    flicker_shell::run(flicker_shell::ShellConfig::single(
        Some(env!("CARGO_MANIFEST_DIR").into()),
        None,
        || Box::new(scene::Sim::new()),
    ))
}
