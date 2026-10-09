//! CLI wrapper around the in-app import pipeline — the same `import_folder` the editor calls.
//!
//!   cargo run -p flicker-content --example import_folder -- <source_dir> <out_dir> <AssetName> \
//!       [reference.json] [--stature <cm>] [--tris <count>] [--recipe <Preset>] [--square <side>] \
//!       [--mirror off|left|right] [--no-face] [--facing <quarters>]
//!
//! `--mirror` keeps one half of a LOPSIDED source and reflects it onto the other (697DEC55), in
//! PREP before the skeleton is fitted; a head bound off forward is un-turned at bake BY DEFAULT and
//! `--no-face` leaves it as posed (A79A6131);
//! `--facing` is the bench's Facing knob (69F4B20D) — quarter-turns about Z that lay a
//! side-profile body onto the rig's −Y forward, defaulting to the same suggestion the bench opens
//! on (`flicker_content::default_facing_quarters`).
//!
//! A source that arrives rigged takes the vendor-rig path as before. A RAW mesh (no skeleton —
//! the ultra Meshy generations) needs the Clayworks Prep numbers: `--stature` (defaults to the
//! canon's 170 cm) and `--tris` (the triangle target; omitted = keep the source count).
//! e.g. rig the ultra golem into staging:
//!   cargo run -p flicker-content --example import_folder -- \
//!     ../PrismContentSource/PrismRaces/GolemBaseV2/<unzipped folder> \
//!     Alpha/content/staging/characters/GolemBaseV2 GolemBaseV2 --stature 170 --tris 62000

use std::path::{Path, PathBuf};

use flicker_content::{RawMeshPrep, Side, StanceSource};

fn main() -> anyhow::Result<()> {
    // The pipeline stages FAIL LOUD through `tracing` (4BB12A75) — the trunk alignment says so
    // when it cannot measure a body and leaves the rest where it stood. Headless, that warning
    // went nowhere; here it reaches stderr. `RUST_LOG` overrides the default.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().collect();
    let mut positional: Vec<&str> = Vec::new();
    let mut stature: Option<f32> = None;
    let mut tris: Option<usize> = None;
    let mut recipe: Option<String> = None;
    // SQUARE FROM: which side the bake-time stance normaliser mirrors a mid-stride body from
    // (ruling FEFDA2B2) — auto (the planted limb of each pair), left or right.
    let mut square = StanceSource::Auto;
    // MIRROR FROM: which half of a LOPSIDED source to keep (697DEC55) — off by default, because a
    // half-body mirror destroys everything one-sided that is not a tagged region.
    let mut mirror: Option<Side> = None;
    // FACE FORWARD: a head bound off the body's forward is un-turned at bake BY DEFAULT — turned
    // heads are common across the generated sources, not only the birds' 45° (A79A6131).
    // `--no-face` is the opt-out.
    let mut no_face = false;
    // FACING: quarter-turns onto the rig's −Y forward; `None` = the shared default (69F4B20D).
    let mut facing: Option<u8> = None;
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--stature" => stature = it.next().and_then(|v| v.parse().ok()),
            "--tris" => tris = it.next().and_then(|v| v.parse().ok()),
            "--recipe" => recipe = it.next().cloned(),
            "--no-face" => no_face = true,
            "--facing" => facing = it.next().and_then(|v| v.parse().ok()),
            "--mirror" => {
                mirror = match it.next().map(String::as_str) {
                    Some("left") => Some(Side::Left),
                    Some("right") => Some(Side::Right),
                    Some("off") | None => None,
                    Some(other) => {
                        eprintln!("--mirror takes off | left | right, not `{other}`");
                        std::process::exit(2);
                    }
                }
            }
            "--square" => {
                square = match it.next().map(String::as_str) {
                    Some("left") => StanceSource::Left,
                    Some("right") => StanceSource::Right,
                    Some("auto") | None => StanceSource::Auto,
                    Some(other) => {
                        eprintln!("--square takes auto | left | right, not `{other}`");
                        std::process::exit(2);
                    }
                }
            }
            _ => positional.push(a),
        }
    }
    if positional.len() < 3 {
        eprintln!(
            "usage: import_folder <source_dir> <out_dir> <AssetName> [reference.json] [--stature <cm>] [--tris <count>] [--recipe <Preset>] [--square auto|left|right] [--mirror off|left|right] [--no-face] [--facing <quarters>]"
        );
        std::process::exit(2);
    }
    let reference: PathBuf = positional
        .get(3)
        .map(PathBuf::from)
        .unwrap_or_else(flicker_content::default_reference);
    // The skeleton preset by name (`package/skeletons/<Pattern>/<Name>.recipe.json`; Humanoid
    // without one) — the modular skeleton's pick for a raw mesh.
    let presets =
        flicker_content::baseline::load_presets(&flicker_content::baseline::skeletons_dir());
    let picked = match &recipe {
        None => flicker_skeletal::format::SkeletonRecipe::humanoid(),
        Some(name) => presets
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| p.recipe.clone())
            .unwrap_or_else(|| {
                let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
                eprintln!("unknown skeleton preset `{name}`; shipped: {names:?}");
                std::process::exit(2);
            }),
    };
    let asked = stature.is_some()
        || tris.is_some()
        || recipe.is_some()
        || mirror.is_some()
        || no_face
        || facing.is_some();
    let raw = asked.then(|| RawMeshPrep {
        stature_cm: stature.unwrap_or(flicker_content::baseline::STATURE),
        target_tris: tris,
        recipe: picked,
        stance_source: square,
        mirror_keep: mirror,
        face_forward: !no_face,
        facing_quarters: facing,
    });
    let summary = flicker_content::import_folder(
        Path::new(positional[0]),
        Path::new(positional[1]),
        positional[2],
        &reference,
        raw,
    )?;
    println!(
        "baked {} — {} bones, {} tris; textures {:?} (from {})",
        summary.rig_path.display(),
        summary.bones,
        summary.tris,
        summary.textures,
        summary.source_fbx.display(),
    );
    Ok(())
}
