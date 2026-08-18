//! Commit every factory patch into a staging tree — the Commit button, headless.
//!
//! ```text
//! cargo run -p flicker-sablework --example commit_patches -- <staging_root> [size]
//! ```
//!
//! Same [`flicker_sablework::commit::commit`] the bench's Commit step calls, so
//! the artifact layout, the at-rest forms and the classification are the real
//! ones. Defaults to the 2K baseline. Prints what each file classified as, which
//! is what the Content Manager's Type column will show.

use flicker_sablework::commit;
use flicker_texture::{presets, BAKE_DEFAULT};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().ok_or("usage: <staging_root> [size]")?);
    let size: u32 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BAKE_DEFAULT);

    for recipe in presets::all() {
        let started = std::time::Instant::now();
        let out = commit::commit(&recipe, size, &root)?;
        println!(
            "{}  ({} ms)",
            out.dir.display(),
            started.elapsed().as_millis()
        );
        for f in &out.files {
            let name = f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let bytes = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            println!(
                "  {:<28} {:>9} B   {}",
                name,
                bytes,
                flicker_content::classify_package(f).id()
            );
        }
    }
    Ok(())
}
