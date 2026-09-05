//! flicker-shell — the reusable **game front-end shell** service.
//!
//! A flicker client is a game scene plus a front-end: an intro splash, a main
//! menu, a settings screen, and a pause overlay, all sharing one gothic UI
//! theme and display/settings persistence. That front-end is identical across
//! games, so it lives here as a service instead of being copied into every
//! client. A client supplies only its in-game scene and calls [`run`]:
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     // Single-scene client: one launch button. A multi-scene host fills
//!     // `ShellConfig.scenes` with several `SceneEntry`s instead.
//!     flicker_shell::run(flicker_shell::ShellConfig::single(
//!         Some(env!("CARGO_MANIFEST_DIR").into()),
//!         None, // or Some("…") to name the menu's launch button
//!         || Box::new(MyGameScene::new()),
//!     ))
//! }
//! ```
//!
//! [`run`] owns the whole flow — display restore → logo → menu → *your scene* →
//! pause/settings — and the winit run loop. The in-game scene reaches back for
//! exactly two things: [`Theme`] (to draw its loading widget and to hand to the
//! [`PauseScene`] it pushes) and [`take_pending_input`] (to pick up input
//! settings changes made in the pause menu). Everything else — the shell
//! scripts, the `ui_theme.json` layout, and the publisher/engine logos — is
//! embedded in the crate, so a new client inherits the front-end with no copied
//! files.

mod display;
mod shell;
mod theme;

pub use display::user_settings_dir;
pub use shell::{
    builtin_behaviours, current_world_map, input_controls, input_profile, modal_host_of,
    param_driven_modals, publish_signal_bindings, run, stage_prompt, stage_prompt_closed,
    take_pending_input, ModalConflict, ModalOption, ModalParams, ModalProgress, ModalText,
    PauseScene, SceneEntry, SceneFactory, SceneInfo, SharedModal, ShellConfig, MODAL_CONFLICT,
    REALM_ADVENTURER, REALM_DEVELOPER, REALM_DM, REALM_GAMEMASTER, STAGE_APPLY, STAGE_KEEP,
    STAGE_REVERT,
};
pub use theme::Theme;
