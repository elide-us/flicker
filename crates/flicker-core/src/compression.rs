//! Generic gzip compression — the workspace-wide helper.
//!
//! Used by [`flicker-voxel`](../flicker_voxel) for cluster bake files
//! and intended for wire packets, asset blobs, and any other byte
//! stream where size matters. Pure-Rust backend (`flate2` with
//! `rust_backend`) so there's no system `zlib` dependency.
//!
//! Two surfaces:
//!
//! - [`compress_gzip`] / [`decompress_gzip`] — buffer-in, buffer-out.
//!   Use these for everything that already has the full payload in
//!   memory (the common case: a finished bake, a fully-constructed
//!   wire packet, an asset loaded into a `Vec<u8>`).
//! - [`is_gzipped`] — a 2-byte magic-number sniff for content
//!   autodetection. Used by loaders that want to accept both gzipped
//!   and plain inputs without an explicit flag (cluster bakes do this
//!   so a developer can `gunzip` a file for inspection and feed it
//!   back without re-compressing).
//!
//! Streaming compression / decompression isn't exposed here yet — when
//! it earns its place (large network streams, on-the-fly file
//! processing) we'll re-export `flate2::{read::GzDecoder,
//! write::GzEncoder}` rather than rebuilding the wheel.

use std::io::{self, Read, Write};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use thiserror::Error;

/// gzip-compress `input` and return the compressed bytes. Uses the
/// default compression level (`Compression::default()` ≈ level 6) —
/// a balanced speed/ratio choice that suits our typical payload
/// (highly repetitive JSON / dense bit fields). Bump the level
/// explicitly via [`compress_gzip_with_level`] for shipping content
/// where size dominates.
pub fn compress_gzip(input: &[u8]) -> Vec<u8> {
    compress_gzip_with_level(input, Compression::default())
}

/// gzip-compress `input` at a caller-chosen [`Compression`] level.
/// Levels: `0` = no compression (just frames the bytes), `1` =
/// fastest / weakest, `9` = slowest / strongest, `default ≈ 6`.
pub fn compress_gzip_with_level(input: &[u8], level: Compression) -> Vec<u8> {
    // Preallocate to the input's order of magnitude — gzip rarely
    // grows past it on real-world data, and the rare case where it
    // does just costs one resize. Better than starting empty for our
    // 5–100 KB typical inputs.
    let mut encoder = GzEncoder::new(Vec::with_capacity(input.len() / 4), level);
    encoder
        .write_all(input)
        .expect("writing into a Vec cannot fail");
    encoder.finish().expect("writing into a Vec cannot fail")
}

/// Inverse of [`compress_gzip`]. Returns the original bytes, or
/// [`CompressionError::Io`] if the input isn't a valid gzip stream
/// (corrupt header, truncated body, bad CRC). The error wraps
/// `std::io::Error` so callers integrating with the wider IO error
/// space don't need a second decode branch.
pub fn decompress_gzip(input: &[u8]) -> Result<Vec<u8>, CompressionError> {
    let mut decoder = GzDecoder::new(input);
    // Decompressed output is typically larger than the input — pre-
    // size to roughly twice the input length. Final size from the
    // gzip trailer would be more precise, but the trailer is a
    // 4-byte mod-2³² value (unreliable for payloads ≥ 4 GB) and
    // peeking it would skip the CRC check; `Vec`'s amortised growth
    // handles the difference cheaply enough.
    let mut out = Vec::with_capacity(input.len() * 2);
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// `true` if `bytes` begins with the gzip magic number `1F 8B`.
/// Doesn't validate the rest of the header — only enough to decide
/// "try the gzip decoder" vs "trust this as already-uncompressed".
#[must_use]
pub fn is_gzipped(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && bytes[0] == 0x1F && bytes[1] == 0x8B
}

/// Errors surfaced from this module. Currently just wraps `io::Error`
/// — every failure mode is an IO-level rejection from `flate2`'s
/// decoder — but the dedicated type keeps room for future enums
/// (e.g. "input too large" guardrails) without breaking callers.
#[derive(Debug, Error)]
pub enum CompressionError {
    /// Underlying IO error from `flate2`'s decoder — typically
    /// "invalid gzip header", "unexpected EOF", or "CRC mismatch".
    #[error("gzip decode failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_input() {
        let compressed = compress_gzip(b"");
        // Gzip frames empty payloads with a header + trailer, so the
        // output is non-empty even on empty input.
        assert!(!compressed.is_empty());
        assert!(is_gzipped(&compressed));
        let decompressed = decompress_gzip(&compressed).expect("round trip");
        assert_eq!(decompressed, b"");
    }

    #[test]
    fn round_trip_small_payload_is_byte_equal() {
        let input = b"the quick brown fox jumps over the lazy dog";
        let compressed = compress_gzip(input);
        assert!(is_gzipped(&compressed));
        let decompressed = decompress_gzip(&compressed).expect("round trip");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_repetitive_payload_compresses_well() {
        // The bake's hot case: long runs of identical bytes. gzip
        // should crush this to a tiny fraction of the input — if it
        // doesn't, we picked the wrong compressor.
        let input = vec![0x42u8; 100_000];
        let compressed = compress_gzip(&input);
        assert!(
            compressed.len() < input.len() / 50,
            "100 KB of repeated bytes compressed to {} — should be far less than {} (1/50)",
            compressed.len(),
            input.len() / 50
        );
        let decompressed = decompress_gzip(&compressed).expect("round trip");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn round_trip_random_payload_does_not_blow_up() {
        // Worst case for gzip — pseudo-random bytes don't compress.
        // We expect the output to be within ~5% of the input (gzip
        // header + occasional unused-literal overhead). The point is
        // that we don't generate degenerate output.
        // Cheap PRNG (xorshift) — no test dep on `rand`.
        let mut state = 0xDEAD_BEEF_C0DE_BABE_u64;
        let input: Vec<u8> = (0..50_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let compressed = compress_gzip(&input);
        assert!(
            compressed.len() < input.len() + input.len() / 20 + 64,
            "random 50 KB compressed to {}, expected ≤ ~52.5 KB",
            compressed.len()
        );
        let decompressed = decompress_gzip(&compressed).expect("round trip");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn is_gzipped_detects_magic_bytes() {
        assert!(is_gzipped(&[0x1F, 0x8B]));
        assert!(is_gzipped(&[0x1F, 0x8B, 0x00, 0xFF]));
        assert!(!is_gzipped(&[0x1F]));
        assert!(!is_gzipped(&[]));
        assert!(!is_gzipped(b"{\"version\": 1}"));
        assert!(!is_gzipped(&[0x1F, 0x00]));
    }

    #[test]
    fn decompress_rejects_invalid_input() {
        // Bytes that pass `is_gzipped`'s magic check but aren't a
        // real gzip stream — the decoder should error rather than
        // hang or return garbage.
        let err = decompress_gzip(&[0x1F, 0x8B, 0x08, 0xFF])
            .expect_err("must reject truncated gzip");
        assert!(matches!(err, CompressionError::Io(_)));

        // Definitely-not-gzip plain text — the decoder rejects on
        // the magic check before reading further.
        let err = decompress_gzip(b"not actually gzip").expect_err("must reject non-gzip");
        assert!(matches!(err, CompressionError::Io(_)));
    }

    #[test]
    fn compression_level_0_still_round_trips() {
        let input = b"hello world";
        let compressed = compress_gzip_with_level(input, Compression::none());
        assert!(is_gzipped(&compressed));
        let decompressed = decompress_gzip(&compressed).unwrap();
        assert_eq!(decompressed, input);
    }

    #[test]
    fn compression_level_best_round_trips() {
        let input = vec![0x55u8; 10_000];
        let compressed = compress_gzip_with_level(&input, Compression::best());
        let decompressed = decompress_gzip(&compressed).unwrap();
        assert_eq!(decompressed, input);
    }
}
