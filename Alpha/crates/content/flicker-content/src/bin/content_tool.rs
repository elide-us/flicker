//! content-tool — offline maintenance for the runtime package tree
//! (`Alpha/content/package/`).
//!
//! ```text
//! content-tool gzify <dir>                    convert eligible text content to gz-at-rest
//! content-tool pack <package-dir> <out-file>  pack the tree into the store-only package.flk
//! content-tool verify <flk> [<package-dir>]   CRC-read every entry; with a tree, byte-compare
//! ```
//!
//! `gzify` recursively converts every eligible text file (`.json`,
//! `.flight`, `.epoch`) to `<name>.<ext>.gz` through the ONE shared routine
//! (`flicker_core::compression` via [`flicker_content::package`]), verifying
//! each round-trip before deleting the raw file. Already-`.gz` files and
//! binary formats (`.png`, `.ttf`, …) are skipped, so re-running converts
//! nothing.
//!
//! `pack` walks the tree as ground truth (the promotion manifest is packed as
//! ordinary content, never consulted as an index), writes Stored zip entries in
//! sorted order with fixed timestamps — deterministic, byte-identical repacks —
//! verifies every entry against the tree, then renames into place. `verify`
//! re-runs that check on an existing package file.

use std::path::Path;
use std::process::ExitCode;

use flicker_content::{pack, package};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gzify") => match args.get(1) {
            Some(dir) => gzify(Path::new(dir)),
            None => usage("gzify needs a directory"),
        },
        Some("pack") => match (args.get(1), args.get(2)) {
            (Some(dir), Some(out)) => pack_cmd(Path::new(dir), Path::new(out)),
            _ => usage("pack needs <package-dir> <out-file>"),
        },
        Some("verify") => match args.get(1) {
            Some(flk) => verify_cmd(Path::new(flk), args.get(2).map(Path::new)),
            None => usage("verify needs <package.flk> [<package-dir>]"),
        },
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

fn pack_cmd(dir: &Path, out: &Path) -> ExitCode {
    match pack::pack_dir(dir, out) {
        Ok(s) => {
            println!(
                "pack {} -> {}: {} files, {} content bytes (stored, verified)",
                dir.display(),
                out.display(),
                s.files,
                s.bytes
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("content-tool pack failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn verify_cmd(flk: &Path, tree: Option<&Path>) -> ExitCode {
    match pack::verify_pack(flk, tree) {
        Ok(s) => {
            println!(
                "verify {}: {} entries, {} bytes OK{}",
                flk.display(),
                s.files,
                s.bytes,
                if tree.is_some() {
                    " (byte-compared against the tree)"
                } else {
                    ""
                }
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("content-tool verify failed: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn usage(why: &str) -> ExitCode {
    eprintln!("content-tool: {why}");
    eprintln!("usage: content-tool gzify <dir>");
    eprintln!("       content-tool pack <package-dir> <out-file>");
    eprintln!("       content-tool verify <flk> [<package-dir>]");
    ExitCode::FAILURE
}
