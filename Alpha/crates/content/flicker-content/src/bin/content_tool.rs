//! content-tool — offline maintenance for the runtime package tree
//! (`Alpha/content/package/`).
//!
//! ```text
//! content-tool gzify <dir>    convert eligible text content to gz-at-rest
//! ```
//!
//! `gzify` recursively converts every eligible text file (`.json`,
//! `.flight`, `.epoch`) to `<name>.<ext>.gz` through the ONE shared routine
//! (`flicker_core::compression` via [`flicker_content::package`]), verifying
//! each round-trip before deleting the raw file. Already-`.gz` files and
//! binary formats (`.png`, `.ttf`, …) are skipped, so re-running converts
//! nothing. Future subcommands (`pack` — concatenate the tree into the
//! store-only package file) slot into the same match below.

use std::path::Path;
use std::process::ExitCode;

use flicker_content::package;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gzify") => match args.get(1) {
            Some(dir) => gzify(Path::new(dir)),
            None => usage("gzify needs a directory"),
        },
        // `pack` lands here later: index + concatenate the gz files, store-only.
        Some(other) => usage(&format!("unknown subcommand '{other}'")),
        None => usage("no subcommand"),
    }
}

fn gzify(dir: &Path) -> ExitCode {
    match package::gzify_dir(dir) {
        Ok(s) => {
            let saved = s.bytes_before.saturating_sub(s.bytes_after);
            println!(
                "gzify {}: {} converted ({} already .gz, {} non-text skipped)",
                dir.display(),
                s.converted,
                s.skipped_gz,
                s.skipped_other
            );
            println!(
                "  bytes: {} -> {} (saved {}, {:.1}%)",
                s.bytes_before,
                s.bytes_after,
                saved,
                if s.bytes_before == 0 {
                    0.0
                } else {
                    100.0 * saved as f64 / s.bytes_before as f64
                }
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("content-tool gzify failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn usage(why: &str) -> ExitCode {
    eprintln!("content-tool: {why}");
    eprintln!("usage: content-tool gzify <dir>");
    ExitCode::FAILURE
}
