//! Commit an authored LIBRARY of material looks into a staging tree — the Commit button,
//! headless.
//!
//! ```text
//! cargo run -p flicker-materialbench --example commit_library -- <library> <staging_root> [size]
//! ```
//!
//! `<library>` is a file NAME in the content data dir — `factory_materials.json` (the
//! factory library: the looks the bench's preset select offers after New) or
//! `glow_materials.json` (the eight emissive glows, slots 248–255) — never a path: the
//! roots service says where content lives, so a name that could reach outside the data dir
//! is refused, loudly. Either file holds v2 material graphs, one per slot, read through
//! [`commit::graphs_in`] (each entry through `flicker_texture::load_material_json`, as a
//! staged bundle's document is read) — the same reader the bench's preset library uses.
//! Each is committed by the same [`commit::commit`] the bench's Commit step calls, weighed
//! by the SAME slot law the bench applies — the class of the material-index row its slot
//! binds — so the artifact layout, the at-rest forms, the refusals and the classification
//! are the real ones. It bakes at a rung the ladder OFFERS (`flicker_texture::size` — the
//! one definition of what sizes exist and which are offered; the very choices the bench's
//! Commit steps through), the 2K baseline by default: a `[size]` that is not one — not a
//! number, off the ladder, or a rung still gated — is refused with the ladder named, never
//! defaulted, as is an argument past it; and a commit that fails names the graph, its slot
//! and the staging root. Prints what each file classified as, which is what the
//! PackageBench's Type column will show, and the bundle's total bytes.
//!
//! The INPUT — the authored graphs and the material index — is read from the content tree's
//! `data/` (the roots service; read only). The OUTPUT goes under the staging root from
//! **argv\[2\]** and nowhere else, so it is safe to aim at a temp dir. Nothing here writes
//! `package/`: promoting a staged look is the PackageBench's review — Aaron's act.

use flicker_materialbench::commit;
use flicker_materials::{JsonTableSource, Tables};
use flicker_texture::size::{offered, BAKE_SIZES};
use flicker_texture::BAKE_DEFAULT;

const USAGE: &str = "usage: commit_library <library> <staging_root> [size]";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let library = args.next().ok_or(USAGE)?;
    // A NAME in the data dir, never a path: a separator (either spelling) or a bare `.` /
    // `..` could aim the read outside the content tree the roots service names.
    let named = std::path::Path::new(&library).file_name() == Some(library.as_ref());
    if !named || library.contains(['/', '\\']) {
        return Err(format!(
            "<library> {library:?} is not a file name in the content data dir ({USAGE})"
        )
        .into());
    }
    let root = std::path::PathBuf::from(args.next().ok_or(USAGE)?);
    // ONE check: the size is a rung the ladder offers, or it is refused — whatever kept it
    // from being one (not a number, off the ladder, gated), the refusal shows the ladder.
    let size = match args.next() {
        None => BAKE_DEFAULT,
        Some(s) => s
            .parse::<u32>()
            .ok()
            .and_then(|px| offered().find(|r| r.px == px))
            .map(|r| r.px)
            .ok_or_else(|| {
                let ladder = BAKE_SIZES
                    .iter()
                    .map(|r| {
                        let state = if r.enabled { "offered" } else { "gated" };
                        format!("{} ({}, {state})", r.px, r.label)
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[size] {s:?} is not an offered bake rung — the ladder: {ladder} ({USAGE})")
            })?,
    };
    if let Some(extra) = args.next() {
        return Err(format!("unexpected argument {extra:?} ({USAGE})").into());
    }

    let data = flicker_content::roots().data();
    let index = Tables::from_source(&JsonTableSource::new(&data))?;
    let path = data.join(&library);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let graphs = commit::graphs_in(&text).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut total = 0u64;
    for graph in &graphs {
        let class = graph.material.and_then(|slot| index.render_class(slot));
        let started = std::time::Instant::now();
        let out = commit::commit(graph, class, size, &root).map_err(|e| {
            format!(
                "{} (slot {:?}, {class:?}) did not commit under {}: {e}",
                graph.id,
                graph.material,
                root.display()
            )
        })?;
        println!(
            "slot {:>3}  {:<12}  {:?}  {}  ({} ms)",
            graph
                .material
                .map(|s| s.to_string())
                .unwrap_or_else(|| "—".into()),
            graph.name,
            class,
            out.dir.display(),
            started.elapsed().as_millis()
        );
        let mut bundle = 0u64;
        for f in &out.files {
            let name = f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let bytes = std::fs::metadata(f)
                .map_err(|e| format!("{}: {e}", f.display()))?
                .len();
            bundle += bytes;
            println!(
                "  {:<28} {:>9} B   {}",
                name,
                bytes,
                flicker_content::classify_package(f).id()
            );
        }
        println!("  {:<28} {:>9} B", "bundle", bundle);
        total += bundle;
    }
    println!(
        "{} graphs of {library} at {size}² → {} B under {}",
        graphs.len(),
        total,
        root.display()
    );
    Ok(())
}
