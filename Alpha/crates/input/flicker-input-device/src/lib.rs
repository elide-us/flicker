//! `flicker-input-device` — platform input sources + the 120 Hz analog channel.
//!
//! The only input crate that touches the OS/hardware. It translates `winit`
//! window events and gamepad state (Apple GameController on macOS, gilrs
//! elsewhere) into the pure model owned by [`flicker_input_core`]: it fills the
//! discrete [`InputState`] snapshot (held-state only — edge classification is the
//! core `Resolver`, never here) and pushes the volatile 120 Hz
//! [`AnalogCache`](flicker_input_core::AnalogCache).
//!
//! `flicker-app` keeps the winit event *loop*, the window, the renderer, the `dt`
//! clock, and `App::update/render`; it forwards `&WindowEvent`s here and drains
//! these sources into its snapshot each frame. This crate depends on
//! `flicker-input-core` (types) and `winit`; it must never depend on
//! `flicker-app`. See spec §5 / §6.
//!
//! # Surface (spec §6.2 / §6.3)
//!
//! - [`DiscreteSource`] — the "drain my accumulated held-state into the snapshot"
//!   contract, implemented by both sources.
//! - [`WindowSource`] — keyboard + mouse. Owns the winit→model translation and
//!   the typed-text / backspace seam (ruling `4B15929B`).
//! - [`GamepadSource`] — the active pad (single player, slot 0). Doubles as the
//!   analog hub: it owns the shared platform backend and the 120 Hz
//!   [`AnalogCache`], writes held **buttons** into the snapshot, and fills the
//!   analog channel via [`GamepadSource::tick_analog`].
//! - [`DeviceCaps`] — which controls the active pad exposes (for settings UI to
//!   gray out absent ones; spec §6.4).

mod gamepad;
mod window;

use flicker_input_core::InputState;

pub use gamepad::{DeviceCaps, GamepadSource};
pub use window::WindowSource;

/// A platform input source that accumulates held-state and flushes it into the
/// shared per-frame [`InputState`] snapshot on demand.
///
/// Both [`WindowSource`] and [`GamepadSource`] implement this. Sources write
/// **held-state only**; per-frame edge classification (Press/Release/Hold/Chord)
/// is the core `Resolver`'s job, not the device's (spec §6.1 / §6.2).
pub trait DiscreteSource {
    /// Flush this source's accumulated state into `out`.
    fn drain_into(&mut self, out: &mut InputState);
}
