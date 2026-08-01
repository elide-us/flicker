//! flicker-core: shared low-level utilities (gzip compression helpers).
//!
//! The input model no longer lives here — it moved to the `flicker-input-core`
//! crate (spec §2 / §3). Reach input types via `flicker_input_core` directly, or
//! through the umbrella as `flicker::input_core::…`.

pub mod compression;

pub use compression::{
    compress_gzip, compress_gzip_with_level, decompress_gzip, is_gzipped, CompressionError,
};

// Re-export the third-party `Compression` level type so callers don't
// need a separate `flate2` dep just to choose a level.
pub use flate2::Compression;
