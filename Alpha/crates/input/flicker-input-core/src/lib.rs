//! `flicker-input-core` — the pure input MODEL + resolution.
//!
//! The leaf crate the whole input stack stands on. It knows nothing of any
//! platform (no `winit`/`gilrs`/GameController) and spawns no threads; it holds
//! only the model and the deterministic edge resolver. `flicker-input-router`
//! (event bus) and `flicker-input-device` (platform sources + the 120 Hz analog
//! sampler) build on top of it; it depends on nothing of ours.
//!
//! The model here was **moved** out of `flicker-core::input` (which no longer
//! exists) and the `flicker-controllertester` `signals.rs` mockup was
//! **promoted** into it (spec §3). The migration is COMPLETE: the transitional
//! `flicker-core::input` re-export bridge is deleted, and consumers import this
//! crate directly (the `flicker` umbrella additionally re-exports it as
//! `flicker::input_core`, though no consumer currently routes through it).
//!
//! # Module map (spec §3.4)
//!
//! - [`device`] — `Key`, `MouseButton`, `GamepadButton`, `GamepadAxis`,
//!   `AxisDirection`, `DeadzoneShape` (+`Display`); each control enum carries
//!   its canonical `ALL` catalog and a `token()` stringtable stem (the
//!   input-side half of the signal/input catalog, S2).
//! - [`snapshot`] — `GamepadConfig`, `GamepadState`, `apply_deadzone`,
//!   `InputState` (with the analog latch), and [`InputEdge`] — the ordered
//!   per-frame transition log that keeps a press from being lost on a long frame.
//! - [`signal`] — [`ActionSignal`] (+`Display`, +`label`) and its catalog
//!   metadata [`SignalGroup`] / [`RebindScope`] (+`group`, +`rebind_scope`,
//!   +`token`, +`rebindable` — the derived settings surface's row source).
//! - [`binding`] — [`InputBinding`] (+`is_down`), [`InputMap`], and the §3.5
//!   descriptors.
//! - [`context`] — [`InputContext`] (open newtype + name registry),
//!   [`ContextualBindings`] (+`signal_held`, +serde by name), and the profile model
//!   [`InputProfile`] / [`ContextBindings`] / [`SignalBinding`].
//! - [`resolve`] — [`EventKind`], [`Fired`], [`Resolver`].
//! - [`analog`] — [`AnalogFrame`], [`AnalogCache`], [`AbstractControls`].
//! - [`rebind`] — [`RebindCapture`], [`capture_input`] (both ride the device
//!   enums' `ALL` catalogs).

pub mod analog;
pub mod binding;
/// The chord layer — hold a modifier and the controls under it mean editor verbs.
pub mod chord;
pub mod context;
pub mod device;
pub mod rebind;
pub mod resolve;
pub mod signal;
pub mod snapshot;

// ── Flat public surface (crate root) ──
pub use analog::{AbstractControls, AnalogCache, AnalogFrame};
pub use binding::{Activation, BindingDescriptor, InputBinding, InputMap, MouseAxis};
pub use chord::{editor_chords, ChordLayer};
pub use context::{ContextBindings, ContextualBindings, InputContext, InputProfile, SignalBinding};
pub use device::{AxisDirection, DeadzoneShape, GamepadAxis, GamepadButton, Key, MouseButton};
pub use rebind::{capture_input, RebindCapture};
pub use resolve::{EventKind, Fired, Resolver, TickTime};
pub use signal::{ActionSignal, RebindScope, SignalGroup};
pub use snapshot::{apply_deadzone, GamepadConfig, GamepadState, InputEdge, InputState};
