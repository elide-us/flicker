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
use std::path::{Path, PathBuf};

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

// ---------------------------------------------------------------------------
// File-level helpers — the gz-at-rest seam for package content.
//
// Package content (`Alpha/content/package/`) keeps every text file
// individually gzip-compressed at rest as `<name>.<ext>.gz`, so the package
// file is a store-only filesystem wrapper (index + concatenation, no
// container compression, full random access — `package.flk`, see
// [`crate::mount`]). Loaders address content by its LOGICAL path
// (`…/intro.flight`) and go through [`read_bytes`] / [`read_text`]. The
// resolution order is one precedence rule applied twice: the SHIPPED form
// wins, looser forms are the dev fallback —
//
//   mounted package entry (gz name first, then raw)  — installed builds
//   → filesystem `.gz` twin                          — the at-rest dev tree
//   → filesystem raw path                            — dev-loose files, fixtures
//
// Writers go through [`write_bytes`] / [`write_text`], which EMIT the gz
// form; the mount is never written (packing is an offline content-tool pass).
// ---------------------------------------------------------------------------

/// The at-rest twin of a logical content path: `<path>.gz` (the whole `.gz`
/// suffix appended, keeping the inner extension — `intro.flight` →
/// `intro.flight.gz`).
#[must_use]
pub fn gz_sibling(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".gz");
    PathBuf::from(os)
}

/// Whether `path` NAMES the gz form itself (its final extension is `gz`).
/// A naming test only — see [`is_gzipped`] for the content sniff.
#[must_use]
pub fn names_gz(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gz"))
}

/// Gz-transparent existence check for a logical content path: true when the
/// mounted package serves it, or the `.gz` twin or raw file is present. The
/// read-side companion of [`read_bytes`] — call this wherever a loader used
/// to call `path.exists()`.
#[must_use]
pub fn file_exists(path: &Path) -> bool {
    crate::mount::exists(path) || (!names_gz(path) && gz_sibling(path).is_file()) || path.is_file()
}

/// Read a content file by its LOGICAL path, transparently decompressing the
/// gz-at-rest form. Resolution order:
///
/// 1. The mounted package serves the path ([`crate::mount`], installed
///    builds) → its entry bytes, decoded by the same sniff as file bytes.
///    A present-but-unreadable entry is a loud error, never a fallback.
/// 2. `path` already names a `.gz` file → read it, decompress (sniffed).
/// 3. `<path>.gz` exists → read + decompress it (gz-FIRST: the at-rest form
///    always wins, so a stale raw twin can't shadow shipped content).
/// 4. Fall back to the raw `path` (dev-loose files, test fixtures). Raw
///    bytes that sniff as gzip are still decompressed, so a hand-renamed
///    file round-trips instead of parsing as garbage.
///
/// Errors are plain `io::Error`; a missing file surfaces as `NotFound` for
/// `path` itself (the logical name), matching what callers report.
pub fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    if let Some(raw) = crate::mount::read_raw(path)? {
        return decode_read(raw);
    }
    if names_gz(path) {
        return decode_read(std::fs::read(path)?);
    }
    match std::fs::read(gz_sibling(path)) {
        Ok(raw) => decode_read(raw),
        Err(e) if e.kind() == io::ErrorKind::NotFound => decode_read(std::fs::read(path)?),
        Err(e) => Err(e),
    }
}

/// [`read_bytes`], decoded as UTF-8 — the text-content read seam (JSON,
/// `.flight`, `.epoch`, …). Non-UTF-8 bytes surface as `InvalidData`.
pub fn read_text(path: &Path) -> io::Result<String> {
    String::from_utf8(read_bytes(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Read at most `max_bytes` of a content file's DECOMPRESSED form — the cheap
/// half of [`read_bytes`], for callers that only need the head of a file.
///
/// Decompression is streamed and stops as soon as the cap is reached, so this
/// costs the prefix rather than the whole file: an inspector can tell what a
/// 500 KB animation clip IS without inflating it. Same logical-path resolution
/// as [`read_bytes`] (gz twin first, raw fallback).
///
/// The result may end mid-token — it is a byte prefix, not a valid document.
/// Callers scan it; they must not parse it as complete.
pub fn read_bytes_prefix(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let raw = if let Some(raw) = crate::mount::read_raw(path)? {
        raw // at-rest bytes from the mounted package — same sniff below
    } else if names_gz(path) {
        std::fs::read(path)?
    } else {
        match std::fs::read(gz_sibling(path)) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::read(path)?,
            Err(e) => return Err(e),
        }
    };
    if !is_gzipped(&raw) {
        let end = raw.len().min(max_bytes);
        return Ok(raw[..end].to_vec());
    }
    let mut out = vec![0u8; max_bytes];
    let mut decoder = GzDecoder::new(&raw[..]);
    let mut filled = 0;
    while filled < max_bytes {
        match decoder.read(&mut out[filled..]) {
            Ok(0) => break, // stream ended before the cap
            Ok(n) => filled += n,
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }
    out.truncate(filled);
    Ok(out)
}

/// Decompress read bytes when they sniff as gzip, pass them through raw
/// otherwise. Decode failures surface as `InvalidData` io errors.
fn decode_read(raw: Vec<u8>) -> io::Result<Vec<u8>> {
    if is_gzipped(&raw) {
        decompress_gzip(&raw).map_err(|e| match e {
            CompressionError::Io(io) => io::Error::new(io::ErrorKind::InvalidData, io),
        })
    } else {
        Ok(raw)
    }
}

/// Write processed content at its LOGICAL path, EMITTING the gz-at-rest form.
/// Returns the path actually written:
///
/// - `path` names `.gz` already → compress and write exactly there.
/// - otherwise → write `<path>.gz` and remove a pre-existing raw `path`, so
///   a save can never leave a stale raw twin shadowed under the fresh gz
///   (package content at rest is ALWAYS gz — one-way discipline).
///
/// Parent directories are created as needed.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    if names_gz(path) {
        std::fs::write(path, compress_gzip(bytes))?;
        return Ok(path.to_path_buf());
    }
    let gz = gz_sibling(path);
    std::fs::write(&gz, compress_gzip(bytes))?;
    if path.is_file() {
        std::fs::remove_file(path)?;
    }
    Ok(gz)
}

/// [`write_bytes`] for text — the writer twin of [`read_text`].
pub fn write_text(path: &Path, text: &str) -> io::Result<PathBuf> {
    write_bytes(path, text.as_bytes())
}

/// One child of a listed content directory — see [`list_dir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// The child's full path (parent joined with its name). File names are the
    /// AT-REST names (`x.json.gz`) — exactly what a filesystem listing of the
    /// package tree shows, so existing name filters keep working.
    pub path: PathBuf,
    pub is_dir: bool,
}

/// List a content directory's children — the enumeration companion of
/// [`read_bytes`]: the union of the mounted package's children and the
/// filesystem's, mount first, each name once (a mounted entry shadows a loose
/// twin). Call this wherever a content loader used to call `fs::read_dir`.
/// `NotFound` only when NEITHER side knows the directory; sorted by name.
pub fn list_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    let mut seen: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
    let mounted = crate::mount::list(path);
    if let Some(children) = &mounted {
        for (name, is_dir) in children {
            seen.insert(name.clone(), *is_dir);
        }
    }
    match std::fs::read_dir(path) {
        Ok(rd) => {
            for entry in rd {
                let entry = entry?;
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                let is_dir = entry.file_type()?.is_dir();
                seen.entry(name).or_insert(is_dir);
            }
        }
        // The mount knowing the directory is enough — an installed build has
        // no loose package tree on disk at all.
        Err(_) if mounted.is_some() => {}
        Err(e) => return Err(e),
    }
    Ok(seen
        .into_iter()
        .map(|(name, is_dir)| DirEntry {
            path: path.join(name),
            is_dir,
        })
        .collect())
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
        let err =
            decompress_gzip(&[0x1F, 0x8B, 0x08, 0xFF]).expect_err("must reject truncated gzip");
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

    /// THE gz-first read seam: a fixture written via [`write_text`] lands as
    /// `<path>.gz` (raw twin removed), reads back by its LOGICAL path, wins
    /// over a raw twin planted beside it, and [`file_exists`] sees it.
    #[test]
    fn write_then_read_is_gz_first_at_the_logical_path() {
        let dir = std::env::temp_dir().join("flicker_core_gz_seam_test");
        let _ = std::fs::remove_dir_all(&dir);
        let logical = dir.join("sample.json");

        let written = write_text(&logical, r#"{"who":"gz"}"#).expect("write emits gz");
        assert_eq!(written, gz_sibling(&logical), "writer emits the .gz twin");
        assert!(!logical.exists(), "no raw twin left behind");
        assert!(
            file_exists(&logical),
            "logical path exists through the seam"
        );
        assert!(
            is_gzipped(&std::fs::read(&written).unwrap()),
            "at-rest bytes are really gzip"
        );
        assert_eq!(
            read_text(&logical).expect("gz-transparent read"),
            r#"{"who":"gz"}"#
        );

        // gz-FIRST: a raw twin planted beside the gz must NOT shadow it.
        std::fs::write(&logical, r#"{"who":"stale-raw"}"#).unwrap();
        assert_eq!(
            read_text(&logical).unwrap(),
            r#"{"who":"gz"}"#,
            "the .gz twin wins"
        );

        // Raw fallback: with no gz twin, a dev-loose raw file still reads.
        std::fs::remove_file(&written).unwrap();
        assert_eq!(read_text(&logical).unwrap(), r#"{"who":"stale-raw"}"#);

        // A literal .gz path reads directly too (directory-listing callers).
        write_text(&logical, r#"{"who":"gz2"}"#).unwrap();
        assert_eq!(
            read_text(&gz_sibling(&logical)).unwrap(),
            r#"{"who":"gz2"}"#
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_bytes_missing_file_is_not_found() {
        let ghost = std::env::temp_dir().join("flicker_core_gz_seam_test_ghost.json");
        let err = read_bytes(&ghost).expect_err("nothing on disk");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!file_exists(&ghost));
    }
}
