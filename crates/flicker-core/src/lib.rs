//! flicker-core: math, time, input abstractions, and the fixed-step game loop.

pub mod compression;
pub mod input;

pub use compression::{
    compress_gzip, compress_gzip_with_level, decompress_gzip, is_gzipped, CompressionError,
};
pub use input::bindings::{
    AbstractControls, Action, AxisDirection, Bindings, ControlConfig, InputBinding, InputMap,
};
pub use input::{
    DeadzoneShape, GamepadAxis, GamepadButton, GamepadConfig, GamepadState, InputState, Key,
    MouseButton,
};

// Re-export the third-party `Compression` level type so callers don't
// need a separate `flate2` dep just to choose a level.
pub use flate2::Compression;
