//! flicker-solarsystem — a POC for **solar-system formation**.
//!
//! A real(ish) formation *simulation*, not just a cinematic disk: embryos are seeded
//! from the protoplanetary disk's solid surface density + condensation sequence
//! (`disk.rs`), then a heliocentric N-body integration (`sim.rs`) runs the
//! giant-impact phase — bodies orbit, gravitationally scatter, and **collide**:
//! merge, hit-and-run, erode, or shatter (`collide.rs`), tracking composition through
//! every event (`material.rs`, strictly within Prism's limited periodic table). The
//! run is recorded and played back under an orbit camera; a habitability verdict
//! (`habitability.rs`) flags the handful of survivors that could be worlds — most are
//! not (wrong mass/orbit/dryness, gas giants, stripped iron cores). A selected
//! survivor's bulk composition is the element-abundance vector that feeds Epoch 1.
//!
//! Each system is born from a **supernova** of a drawn size (`disk.rs::Nebula`) that sets
//! its disk mass and metallicity — the master source of diversity: light/metal-poor disks
//! stay barren, heavy/metal-rich ones turn giant-dominated. How many giants form (and of
//! what kind/mass), whether any worlds are habitable, and what leftover **belt**
//! planetesimals survive are all *emergent*, never targeted.
//!
//! Time/units are AU·yr·M☉ (`astro.rs`). Caveat: collision radii are inflated to make
//! the giant-impact phase tractable, so the displayed "Myr" is a scaled cosmetic
//! label, not a faithful clock.
//!
//! Controls: drag orbit · wheel zoom · Space play/pause · ←/→ scrub · ↑/↓ select a
//! survivor · `[`/`]` dial the supernova size (re-forms this seed's disk) · R reseed
//! (new random system) · Esc quit.

mod astro;
mod body;
mod camera;
mod collide;
mod disk;
mod habitability;
mod material;
mod planet;
mod scene;
mod sim;
mod worldglobe;

use anyhow::Result;
use flicker::app::run;
use flicker::scene::SceneManager;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(scene::SolarSystem::new())))?;
    Ok(())
}
