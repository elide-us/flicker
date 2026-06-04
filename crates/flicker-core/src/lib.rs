//! flicker-core: math, time, input abstractions, and the fixed-step game loop.

pub mod compression;
pub mod input;

pub use compression::{
    compress_gzip, compress_gzip_with_level, decompress_gzip, is_gzipped, CompressionError,
};
pub use input::bindings::{Action, Bindings, ControlConfig};
pub use input::{InputState, Key};

// Re-export the third-party `Compression` level type so callers don't
// need a separate `flate2` dep just to choose a level.
pub use flate2::Compression;
