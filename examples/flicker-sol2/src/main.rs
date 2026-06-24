//! flicker-sol2 — a viewer for a **supernova ejecta cloud** (the cast material distribution).
//!
//! A star forms from its local cloud. The supernova that seeded that cloud cast material
//! outward; in this (deliberately simple) model each element settles at a characteristic
//! *distance* governed only by its **atomic weight** — heavier elements are flung short,
//! lighter ones reach far.
//!
//! The scene draws that as a **2D top-down projection**: the star at the origin, xyz arrows,
//! and one translucent colour ring per Prism element at its cast distance. The cloud is
//! **clumpy and differentially sheared** (`src/cloud.rs`), and overdensity **dots** mark where
//! matter concentrates (`src/detect.rs`, toggle `B`). Dials shape the distribution (explosion
//! reach, atomic-weight falloff, gradient sharpness, clump strength).
//!
//! This is the material-distribution view ONLY. The formation simulation that grew bodies from
//! it was removed (it kept getting built wrong); when rebuilt, it must **derive from these
//! starting values** — nothing invented on the side.
//!
//! Controls: `[`/`]` explosion · ↑/↓ falloff · `,`/`.` gradient · `;`/`'` clump · ←/→ or hover
//! focus an element · wheel or `-`/`=` zoom · Space pause · N reclump · B dots · R reset · Esc.

mod cloud;
mod detect;
mod draw;
mod model;
mod scene;

use anyhow::Result;
use flicker::app::run;
use flicker::scene::SceneManager;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(scene::CloudView::new())))?;
    Ok(())
}
