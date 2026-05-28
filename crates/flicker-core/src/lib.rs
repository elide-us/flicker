//! flicker-core: math, time, input abstractions, and the fixed-step game loop.

pub mod input;

pub use input::bindings::{Action, Bindings, ControlConfig};
pub use input::{InputState, Key};
