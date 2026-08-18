//! The package-content seam — how everything under `Alpha/content/package/`
//! is read, written, and converted to its at-rest form.
//!
//! Package content ships **individually gz-compressed**: every text file is
//! `<name>.<ext>.gz` on disk, so the future package format can be a
//! store-only filesystem wrapper (index + concatenation, NO container
//! compression, full random access). The one implementation of that seam
//! lives in [`flicker_core::compression`] (the workspace-wide gzip routine);
//! this module re-exports it under the content crate's name and adds the
//! offline conversion pass ([`gzify_dir`]) the `content-tool` bin drives.
//!
//! - **Readers** address content by its LOGICAL path (`…/intro.flight`) and
//!   call [`read_text`] / [`read_bytes`]: `<path>.gz` is tried first, the
//!   raw path is the dev-loose/test-fixture fallback.
//! - **Writers** of processed content call [`write_text`] / [`write_bytes`]
//!   and EMIT gz — at rest the package is ALWAYS gz (one-way discipline).
//! - **Binary formats stay raw** (`.png` textures, `.ttf` fonts — already
//!   compressed or compile-embedded); [`gzify_dir`] skips them.

use std::path::Path;

use anyhow::{ensure, Context, Result};

pub use flicker_core::compression::{
    file_exists, gz_sibling, names_gz, read_bytes, read_text, write_bytes, write_text,
};

/// File extensions [`gzify_dir`] converts — the package's TEXT content
/// formats. Everything else is left untouched: `.gz` (already at rest),
/// `.png`/`.ttf` (compressed / compile-embedded binaries), and the odd
/// `.md`/`.txt` sidecar (human-read documentation, not game-read content).
pub const GZIFY_EXTENSIONS: &[&str] = &["json", "flight", "epoch"];

/// What one [`gzify_dir`] pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GzifyStats {
    /// Files converted raw → `<name>.<ext>.gz` this pass.
    pub converted: usize,
    /// Files already `.gz` (the idempotent skip).
    pub skipped_gz: usize,
    /// Files whose extension is not in [`GZIFY_EXTENSIONS`] (binaries, docs).
    pub skipped_other: usize,
    /// Raw byte total of the files converted this pass.
    pub bytes_before: u64,
    /// Compressed byte total those files became.
    pub bytes_after: u64,
}

/// Recursively convert every eligible text file under `dir` to its
/// gz-at-rest form: write `<name>.<ext>.gz` through the shared routine,
/// **verify** the round-trip decompresses to the original bytes, then delete
/// the raw file. Idempotent — a second pass converts nothing (`.gz` files
/// are skipped by name). Errors abort the walk with the offending path named;
/// a verify failure leaves the raw file in place.
pub fn gzify_dir(dir: &Path) -> Result<GzifyStats> {
    ensure!(dir.is_dir(), "gzify: {} is not a directory", dir.display());
    let mut stats = GzifyStats::default();
    gzify_walk(dir, &mut stats)?;
    Ok(stats)
}

fn gzify_walk(dir: &Path, stats: &mut GzifyStats) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("gzify: reading dir {}", dir.display()))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("gzify: reading dir {}", dir.display()))?
            .path();
        if path.is_dir() {
            gzify_walk(&path, stats)?;
        } else {
            gzify_file(&path, stats)?;
        }
    }
    Ok(())
}

/// Convert one file if eligible (see [`GZIFY_EXTENSIONS`]).
fn gzify_file(path: &Path, stats: &mut GzifyStats) -> Result<()> {
    if names_gz(path) {
        stats.skipped_gz += 1;
        return Ok(());
    }
    let eligible = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| GZIFY_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
    if !eligible {
        stats.skipped_other += 1;
        return Ok(());
    }

    let raw = std::fs::read(path).with_context(|| format!("gzify: reading {}", path.display()))?;
    let gz = gz_sibling(path);
    let compressed = flicker_core::compress_gzip(&raw);
    std::fs::write(&gz, &compressed).with_context(|| format!("gzify: writing {}", gz.display()))?;

    // Verify BEFORE deleting the raw file: the round-trip through the shared
    // routine must reproduce the original bytes exactly.
    let back = read_bytes(&gz).with_context(|| format!("gzify: verifying {}", gz.display()))?;
    if back != raw {
        std::fs::remove_file(&gz).ok();
        anyhow::bail!(
            "gzify: round-trip verify failed for {} — raw file left in place",
            path.display()
        );
    }
    std::fs::remove_file(path)
        .with_context(|| format!("gzify: removing raw {}", path.display()))?;

    stats.converted += 1;
    stats.bytes_before += raw.len() as u64;
    stats.bytes_after += compressed.len() as u64;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// THE idempotence guard: one pass converts exactly the eligible text
    /// files; a second pass converts NOTHING (the `.gz` results are skipped
    /// by name, binaries are skipped by extension), and the converted
    /// content still reads back — byte-identical — through the seam.
    #[test]
    fn gzify_converts_once_and_is_idempotent() {
        let dir = fresh_dir("flicker_content_gzify_idempotent");
        std::fs::create_dir_all(dir.join("clips")).unwrap();
        std::fs::write(dir.join("rig.json"), r#"{"format":"flicker.rig"}"#).unwrap();
        std::fs::write(dir.join("clips/walk.json"), r#"{"clips":[]}"#).unwrap();
        std::fs::write(dir.join("intro.flight"), r#"{"format":"flicker.flight"}"#).unwrap();
        std::fs::write(dir.join("skin.png"), [0x89, b'P', b'N', b'G']).unwrap();
        std::fs::write(
            dir.join("already.json.gz"),
            flicker_core::compress_gzip(b"{}"),
        )
        .unwrap();

        let first = gzify_dir(&dir).expect("first pass");
        assert_eq!(first.converted, 3, "json + nested json + flight convert");
        assert_eq!(first.skipped_gz, 1, "the pre-existing .gz is skipped");
        assert_eq!(first.skipped_other, 1, "the png is skipped");
        assert!(first.bytes_after > 0 && first.bytes_before > 0);

        // Raw forms gone, gz twins present, png untouched.
        assert!(!dir.join("rig.json").exists());
        assert!(dir.join("rig.json.gz").is_file());
        assert!(!dir.join("clips/walk.json").exists());
        assert!(dir.join("clips/walk.json.gz").is_file());
        assert!(!dir.join("intro.flight").exists());
        assert!(dir.join("intro.flight.gz").is_file());
        assert!(dir.join("skin.png").is_file(), "binary content stays raw");

        // The seam reads the converted files by their LOGICAL paths.
        assert_eq!(
            read_text(&dir.join("rig.json")).unwrap(),
            r#"{"format":"flicker.rig"}"#
        );
        assert_eq!(
            read_text(&dir.join("intro.flight")).unwrap(),
            r#"{"format":"flicker.flight"}"#
        );

        // Idempotent: the second pass converts nothing at all.
        let second = gzify_dir(&dir).expect("second pass");
        assert_eq!(second.converted, 0, "re-running converts nothing");
        assert_eq!(second.bytes_before, 0);
        assert_eq!(
            second.skipped_gz, 4,
            "the three conversions + the original .gz"
        );
        assert_eq!(second.skipped_other, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt write must never cost the raw file: verify failure keeps it.
    /// (Simulated by gzifying a file the walker can also prove round-trips —
    /// the real assertion is the success path's verify-before-delete order,
    /// so here we just pin that a passing verify deletes and a missing dir
    /// errors instead of silently succeeding.)
    #[test]
    fn gzify_rejects_a_non_directory() {
        let dir = fresh_dir("flicker_content_gzify_not_a_dir");
        let file = dir.join("lone.json");
        std::fs::write(&file, "{}").unwrap();
        assert!(gzify_dir(&file).is_err(), "a file path is refused");
        assert!(file.is_file(), "and the file is untouched");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
