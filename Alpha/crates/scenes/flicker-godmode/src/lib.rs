//! flicker-godmode — **God Mode**, the world-simulation console (library).
//!
//! The maintainer's window onto a living planet: a globe he can orbit, recolour
//! by any field the simulation exposes, step or play, reseed, and eventually
//! snapshot when he likes what he sees. It draws the world as a **stack of
//! concentric layer shells** — core, mantle, then each bed the chemistry grows
//! above it — so the planet's structure is the picture, not a texture painted on
//! one.
//!
//! The scene is a PAIR (five-line architecture): `godmode.scene.json` authors
//! the HUD tree + this bench's style blocks; `godmode.lua` picks every state
//! word, glyph and style path from the RAW model this behaviour publishes; the
//! Rust component kinds draw. The life-supporting gauge rows are REFILLED into
//! the authored `gm_hab_rows` container at construction from
//! `habitability::BANDS` — the observer's numbers, never authored copies.
//!
//! The simulation lives in `flicker-poc-chemistry` and is **GPU-free**; it runs
//! on its own thread ([`sim_thread`]) and this scene never steps it inside a
//! frame. The scene sends commands and draws the latest published snapshot.
//!
//! A scene PACKAGE — library only, no binary: the launcher's roster entry is
//! the CLIENT BEHAVIOUR that plays `godmode.scene.json`, built via [`scene`].

mod globe_view;
mod scene;
mod sim_thread;

pub use scene::GodModeScene;

use flicker::ui::SceneDef;

/// Build the world-simulation console as a boxed `Scene` — the CLIENT BEHAVIOUR
/// the roster registers; the manifest resolves `godmode.scene.json` and hands
/// its def here.
pub fn scene(def: &SceneDef) -> Box<dyn flicker::scene::Scene> {
    Box::new(GodModeScene::new(def))
}
