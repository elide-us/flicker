//! flicker-godmode — **God Mode**, the world-simulation console (library).
//!
//! The maintainer's window onto a living planet: a Populous-style globe he can
//! orbit, recolour by any field the simulation exposes, step or play, reseed, and
//! eventually snapshot when he likes what he sees. It draws the world as a **stack
//! of concentric layer shells** — core, mantle, then each bed the chemistry grows
//! above it — so the planet's structure is the picture, not a texture painted on
//! one.
//!
//! The simulation lives in `flicker-poc-chemistry` and is **GPU-free**; it runs on
//! its own thread ([`sim_thread`]) and this scene never steps it inside a frame.
//! The scene sends commands and draws the latest published snapshot.
//!
//! A scene PACKAGE — library only, no binary (the standalone
//! `flicker-poc-chemistry` app moved into the unified `prism-alpha` launcher, the
//! same promotion `flicker-pocepochs` took): the launcher builds it via [`scene`]
//! and registers it as a roster entry.

mod globe_view;
mod route;
mod scene;
mod sim_thread;

pub use scene::GodModeScene;

/// Build the world-simulation console as a boxed `Scene` for the shell's scene
/// registry.
pub fn scene() -> Box<dyn flicker::scene::Scene> {
    Box::new(GodModeScene::new())
}

/// ⛔ QUARANTINED scene styles (five-line split, Aaron 2026-08-12): this dormant
/// bench's style blocks, vendored OUT of ui_theme.json — a scene's values belong
/// in its scene file, and these move into this bench's own `.scene.json` at its
/// migration. Do not grow this file.
pub(crate) fn scene_styles() -> serde_json::Value {
    serde_json::from_str(include_str!("../scene_styles.json")).expect("scene_styles.json parses")
}
