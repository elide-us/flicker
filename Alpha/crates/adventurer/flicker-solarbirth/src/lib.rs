//! flicker-solarbirth — the **birth of the Prism system**, as a cinematic (library).
//!
//! The `Sim` scene (fixed Prism roster + authored `flicker-flight` fly-in + dust-cloud
//! clearing) plus its camera/system modules. A minigame/cinematic PACKAGE — library
//! only, no binary; the `prism-alpha` launcher builds it via [`scene`] and registers it
//! as a roster entry.
//!
//! Controls: drag rotate · wheel zoom · Space replay the fly-in · Esc pause.

mod camera;
mod route;
mod scene;
mod system;

pub use scene::Sim;

/// Build the solar-birth cinematic as a boxed `Scene` — the CLIENT BEHAVIOUR the
/// roster registers; the manifest resolves `solarbirth.scene.json` and hands its
/// def here.
pub fn scene(def: &flicker::ui::SceneDef) -> Box<dyn flicker::scene::Scene> {
    Box::new(Sim::new(def))
}
