//! flicker-pocepochs — the **forward-regenerative epoch simulation** (library).
//!
//! (Replaces the old single-hex retained layer-map POC; its water-cycle nucleus
//! `layers.rs` is re-homed in `flicker_worldgen::water`.) The in-game
//! [`WorldScene`] drives a `flicker_worldengine` simulation: it scrubs the
//! epochs, tweaks levers, and reseeds, regenerating forward on demand —
//! Epoch-1 seed viewing + Epoch-2 molten convection today, with the icosphere
//! globe render and the walker HUD (readout + transport + life-supporting
//! gauges), authored as the scene pair `pocepochs.scene.json` + `pocepochs.lua`.
//!
//! The planet itself is not this crate's: the mesh stack, the authored stage,
//! the offscreen target and the orbit camera are one
//! [`flicker_globe::GlobeWorld`], shared with God Mode and Populous. All this
//! bench owns of the picture is [`appearance`] — how a simulated column becomes
//! a colour.
//!
//! A scene PACKAGE — library only, no binary: the launcher's roster entry is the
//! CLIENT BEHAVIOUR that plays `pocepochs.scene.json`, built via [`scene`].

mod appearance;
mod scene;

pub use scene::WorldScene;

use flicker::ui::SceneDef;

/// Build the epoch-simulation viewer as a boxed `Scene` — the CLIENT BEHAVIOUR
/// the roster registers; the manifest resolves `pocepochs.scene.json` and hands
/// its def here.
pub fn scene(def: &SceneDef) -> Box<dyn flicker::scene::Scene> {
    Box::new(WorldScene::new(def))
}
