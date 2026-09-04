//! `bake_world` — a headless bake tool: turn a committed Populous planet
//! (a v2 `.epoch`) into a region of the WORLD CLUSTER MAP — the gameplay
//! heightmap + material planes, atlas-addressed (`WorldClusterId`
//! coordinates), the LOD-8 heightmap-dot tier the sparse ladder samples.
//!
//! ```text
//! cargo run -p flicker-worldtile --bin bake_world -- <planet.epoch[.gz]> \
//!     [--center <hex>] [--rings <n>] [--relief-m <m-per-unit>] \
//!     [--trim-deg <deg>] [--out <dir>]
//! ```
//!
//! Defaults: the highest-ground hex as centre, 1 ring, 2000 m per ledger
//! unit, 10° cap trim. Output lands BESIDE the epoch (or under `--out`):
//!
//! ```text
//! <out>/<epoch-stem>/region_c{center}_r{rings}/
//!   index.json          — frame, rect, id layout, levers, per-hex audit
//!   height.f32le.gz     — f32 LE ground metres, row-major over the rect
//!   material.u8.gz      — exposed-material codes (source registry)
//! ```
//!
//! The bench committed the epoch into `staging/worlds/`, so by default the
//! baked region stays in staging too — the ingest contract: benches and
//! their tools write to staging and STOP; the Content Manager promotes.

use std::path::PathBuf;

use flicker_core::compression;
use flicker_worldengine::PlanetEpoch;
use flicker_worldtile::{bake::region_rings, bake_region, AtlasFrame, PlanetSource, TileSource};

#[derive(serde::Serialize)]
struct Index {
    format: &'static str,
    version: u32,
    /// The ratified WorldClusterId layout, stated where the data lives so a
    /// reader needs no other source: `[LOD:4][y:12][x:24][z:24]`.
    id_layout: &'static str,
    epoch: String,
    freq: u32,
    seed: String,
    /// Atlas frame.
    atlas_width: u32,
    atlas_height: u32,
    trim_deg: f64,
    cluster_m: f64,
    /// The rect these planes cover, in atlas cluster coordinates.
    x0: u32,
    z0: u32,
    width: u32,
    height: u32,
    /// Levers, recorded so the choice is data.
    relief_m_per_unit: f64,
    sea_level_m: f64,
    /// Material registry for the `material.u8` plane.
    materials: Vec<(u8, &'static str)>,
    /// Per-region-hex audit: (hex id, owned clusters, conserved thickness m).
    hexes: Vec<(u32, u64, f64)>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let epoch_path = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("usage: bake_world <planet.epoch[.gz]> [--center N] [--rings N] [--relief-m M] [--trim-deg D] [--out DIR]")?
        .clone();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let rings: u32 = flag("--rings").map(|v| v.parse()).transpose()?.unwrap_or(1);
    let relief_m: f64 = flag("--relief-m")
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(2_000.0);
    let trim_deg: f64 = flag("--trim-deg")
        .map(|v| v.parse())
        .transpose()?
        .unwrap_or(10.0);

    let epoch = PlanetEpoch::load(&epoch_path)?;
    let recipe = epoch.recipe;
    println!(
        "planet: freq {} seed {:#018x} · {} ticks · loading…",
        recipe.freq, recipe.seed, epoch.era.ticks
    );
    let src = PlanetSource::new(epoch, relief_m);

    // Default centre: the tallest ground on the planet — the hex a first
    // look most wants to see.
    let center: u32 = match flag("--center") {
        Some(v) => v.parse()?,
        None => (0..src.grid().len() as u32)
            .max_by(|&a, &b| {
                src.thickness_m(a as usize)
                    .total_cmp(&src.thickness_m(b as usize))
            })
            .expect("a planet has hexes"),
    };

    let frame = AtlasFrame::new(recipe.freq, trim_deg);
    let region = region_rings(&src, center, rings);
    println!(
        "atlas: {}×{} clusters · region: {} hexes around {center} · baking…",
        frame.width,
        frame.height,
        region.len()
    );
    let bake = bake_region(&src, &frame, &region);

    // ── Write the artifact beside the epoch (or under --out). ──
    let epoch_file = PathBuf::from(&epoch_path);
    let stem = epoch_file
        .file_name()
        .map(|n| {
            n.to_string_lossy()
                .trim_end_matches(".gz")
                .trim_end_matches(".epoch")
                .to_string()
        })
        .unwrap_or_else(|| "planet".into());
    let out_root = flag("--out")
        .map(PathBuf::from)
        .or_else(|| epoch_file.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = out_root.join(&stem).join(format!("region_c{center}_r{rings}"));
    std::fs::create_dir_all(&dir)?;

    let heights_le: Vec<u8> = bake
        .heights
        .iter()
        .flat_map(|h| h.to_le_bytes())
        .collect();
    std::fs::write(
        dir.join("height.f32le.gz"),
        compression::compress_gzip(&heights_le),
    )?;
    std::fs::write(
        dir.join("material.u8.gz"),
        compression::compress_gzip(&bake.materials),
    )?;

    let index = Index {
        format: "flicker.worldmap.region",
        version: 1,
        id_layout: "[LOD:4][y:12][x:24][z:24]",
        epoch: epoch_file.display().to_string(),
        freq: recipe.freq,
        seed: format!("{:#018x}", recipe.seed),
        atlas_width: frame.width,
        atlas_height: frame.height,
        trim_deg,
        cluster_m: clayengine::cluster_span_m(),
        x0: bake.x0,
        z0: bake.z0,
        width: bake.width,
        height: bake.height,
        relief_m_per_unit: relief_m,
        sea_level_m: bake.sea_level_m,
        materials: vec![
            (flicker_worldtile::source::MAT_BASE, "base crust"),
            (flicker_worldtile::source::MAT_STRATUM, "stratum"),
            (flicker_worldtile::source::MAT_VOLCANIC, "volcanic"),
            (flicker_worldtile::source::MAT_ROCK, "loose rock"),
            (flicker_worldtile::source::MAT_SEDIMENT, "sediment"),
            (
                flicker_worldtile::source::MAT_VEIN_BASE,
                "vein strata from here: 16 + vein kind index",
            ),
        ],
        hexes: bake.hexes.clone(),
    };
    std::fs::write(dir.join("index.json"), serde_json::to_vec_pretty(&index)?)?;

    println!(
        "baked {}×{} clusters ({} hexes) · sea {:.0} m · → {}",
        bake.width,
        bake.height,
        bake.hexes.len(),
        bake.sea_level_m,
        dir.display()
    );
    Ok(())
}
