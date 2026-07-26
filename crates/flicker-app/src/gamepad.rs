//! Platform gamepad input, read once per frame by the runner.
//!
//! macOS reads controllers through Apple's **GameController framework** ([`macos`]) — the
//! path Unreal and other engines use — because gilrs's raw-IOKit-HID backend enumerates
//! Xbox/PlayStation pads on macOS but never delivers their button/axis input. Every other
//! platform uses gilrs ([`gilrs_backend`]).
//!
//! Both back ends expose the same [`GamepadReader`] with `new()` + `poll(&mut InputState)`.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::GamepadReader;

#[cfg(not(target_os = "macos"))]
mod gilrs_backend;
#[cfg(not(target_os = "macos"))]
pub use gilrs_backend::GamepadReader;
