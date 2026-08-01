//! flicker-pocepochs — the **forward-regenerative epoch simulation** (library).
//!
//! (Replaces the old single-hex retained layer-map POC; its water-cycle nucleus
//! `layers.rs` is re-homed in `flicker_worldgen::water`.) The in-game
//! [`WorldScene`] drives a `flicker_worldengine` simulation: it scrubs the
//! epochs, tweaks levers, and reseeds, regenerating forward on demand —
//! Epoch-1 seed viewing + Epoch-2 molten convection today, with the icosphere
//! globe render and the walker HUD (readout + life-supporting gauges).
//!
//! A scene PACKAGE — library only, no binary (the standalone app moved into the
//! unified `prism-alpha` launcher): the launcher builds it via [`scene`] and
//! registers it as a Developer-mode roster entry.

mod camera;
mod globe;
mod route;
mod scene;

pub use scene::WorldScene;

/// Build the epoch-simulation viewer as a boxed `Scene` for the shell's scene
/// registry.
pub fn scene() -> Box<dyn flicker::scene::Scene> {
    Box::new(WorldScene::new())
}
