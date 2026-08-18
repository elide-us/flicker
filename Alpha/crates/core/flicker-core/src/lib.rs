//! flicker-core: shared low-level utilities (gzip compression helpers).
//!
//! The input model no longer lives here — it moved to the `flicker-input-core`
//! crate (spec §2 / §3). Reach input types via `flicker_input_core` directly, or
//! through the umbrella as `flicker::input_core::…`.

pub mod compression;
pub mod mount;
pub mod roots;

pub use compression::{
    compress_gzip, compress_gzip_with_level, decompress_gzip, is_gzipped, CompressionError,
};
// The file-level gz-at-rest seam (read_bytes/read_text/write_bytes/write_text/
// file_exists/gz_sibling/names_gz) is deliberately NOT re-exported at the root —
// those names are too generic there. Reach it as `flicker_core::compression::…`
// (or through `flicker_content::package`, the content crate's named seam).

// Re-export the third-party `Compression` level type so callers don't
// need a separate `flate2` dep just to choose a level.
pub use flate2::Compression;
