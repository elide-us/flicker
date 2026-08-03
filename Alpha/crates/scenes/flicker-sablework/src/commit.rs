//! Committing the bench's output into `staging/`.
//!
//! `source/ → [benches] → staging/ → [Content Manager promotes] → package/`
//!
//! Sablework is an ingest bench like Clayworks and Loomforge, so it writes here
//! and stops. Nothing it produces is visible to the running game until someone
//! reviews it in the Content Manager and promotes it — that split is the whole
//! point of the staging tier, and this module does not have a `package/` path in
//! it anywhere.
//!
//! # The two at-rest forms, and why they differ
//!
//! - **The recipe is TEXT**, so it goes through [`flicker_content::package`] and
//!   lands gz, exactly like every other staged text artifact. A promotion is then
//!   a plain byte move with no transcoding.
//! - **The maps are PNG**, and PNG is already compressed. Binary content stays
//!   raw in both trees (`GZIFY_EXTENSIONS` is `json`/`flight`/`epoch`, and
//!   `gzify_dir` skips images), which is why they are written with a plain file
//!   write. That is not a bypass of the gz seam — it is the no-gz half of the
//!   same rule, and gzipping them would make a promotion a transcode.
//!
//! # Layout mirrors `package/`
//!
//! `materials/<AssetName>/<AssetName>_<Map>.png` — so a promotion is the same
//! relative path under a different root, and the map suffixes are the content
//! standard's own role names (`GolemBase_Low_BaseColor.png` is already shaped
//! this way).

use std::io;
use std::path::{Path, PathBuf};

use flicker_texture::{bake, MapKind, TextureRecipe};

/// The sub-tree every material's appearance lands in, under whichever root it is
/// being written to. One name, so the staging and package sides cannot drift.
pub const MATERIALS_DIR: &str = "materials";

/// What a commit wrote.
pub struct Committed {
    /// The asset folder, under the staging root.
    pub dir: PathBuf,
    /// Every file written, in write order — the recipe last, so a reader that
    /// sees the recipe knows the maps beside it are complete.
    pub files: Vec<PathBuf>,
}

/// Fold an authored recipe name into the asset-naming standard:
/// **PascalCase-Hyphenated**, no spaces, no vendor punctuation, no path
/// separators.
///
/// A name reaches here from a text field, so it can contain anything; the
/// filesystem and the naming standard cannot. Anything unusable becomes a
/// hyphen, runs collapse, and an empty result falls back to the recipe id —
/// a file that lands SOMEWHERE recoverable beats a commit that silently does
/// nothing.
pub fn asset_name(name: &str, id: &str) -> String {
    let fold = |s: &str| -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_matches('-').to_string()
    };
    let folded = fold(name);
    if folded.is_empty() {
        let from_id = fold(id);
        if from_id.is_empty() {
            "Untitled".into()
        } else {
            from_id
        }
    } else {
        folded
    }
}

/// Bake `recipe` at `size` and write the artifact folder into `staging_root`.
///
/// `staging_root` is passed in rather than looked up so a test can drive this
/// against a temp tree; the scene asks `flicker_content::roots().staging()`.
pub fn commit(
    recipe: &TextureRecipe,
    size: u32,
    staging_root: &Path,
) -> io::Result<Committed> {
    let name = asset_name(&recipe.name, &recipe.id);
    let dir = staging_root.join(MATERIALS_DIR).join(&name);
    std::fs::create_dir_all(&dir)?;

    let set = bake(recipe, size);
    let mut files = Vec::with_capacity(MapKind::ALL.len() + 1);

    for kind in MapKind::ALL {
        let Some(map) = set.get(kind) else { continue };
        let path = dir.join(format!("{name}_{}.png", kind.role()));
        let png = encode_png(kind, &map.pixels, map.size).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{kind:?} PNG encode: {e}"))
        })?;
        // Raw, not gz — see the module docs.
        std::fs::write(&path, png)?;
        files.push(path);
    }

    // The recipe LAST: it is the artifact that says "these maps exist", so a
    // reader that finds it can trust the folder beside it is finished.
    let json = serde_json::to_string_pretty(recipe)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let recipe_path = dir.join(format!("{name}.texture.json"));
    files.push(flicker_content::package::write_text(&recipe_path, &json)?);

    Ok(Committed { dir, files })
}

/// Encode one map to PNG, in the **narrowest form that carries its meaning**.
///
/// The baker works in RGBA8 because that is what the GPU upload path takes, but
/// most of those channels hold nothing: a roughness map is one number repeated
/// three times under an alpha that is always 255. Writing that verbatim costs
/// roughly double the artifact for no information — ~19 MB per material at 2K
/// instead of ~10 — and every one of those bytes gets reviewed, promoted and
/// shipped.
///
/// So: the scalar maps go out as **L8** (their red channel, which is the value),
/// and the two colour maps as **RGB8** (the bake guarantees an opaque alpha, and
/// `every_baked_map_is_opaque` in `flicker-texture` is what lets this drop it).
/// A decoder widens back to RGBA8 on load; PNG carries its own channel count, so
/// nothing has to be told which form a file is in.
fn encode_png(kind: MapKind, rgba: &[u8], size: u32) -> Result<Vec<u8>, image::ImageError> {
    let (color, pixels) = match kind {
        // Colour: keep three channels, drop the constant alpha.
        MapKind::BaseColor | MapKind::Normal => (
            image::ColorType::Rgb8,
            rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>(),
        ),
        // Scalar: one channel is the whole map.
        _ => (
            image::ColorType::L8,
            rgba.chunks_exact(4).map(|p| p[0]).collect::<Vec<u8>>(),
        ),
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(&mut out, &pixels, size, size, color, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_content::PackageClass;
    use flicker_texture::presets;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sablework_commit_{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A commit writes the whole artifact folder in the layout `package/` uses, so
    /// a promotion is the same relative path under a different root.
    #[test]
    fn a_commit_writes_the_maps_and_the_recipe_in_package_layout() {
        let root = temp_root("layout");
        let recipe = presets::granite();
        let out = commit(&recipe, 32, &root).expect("commit writes");

        assert_eq!(out.dir, root.join("materials").join("Granite"));
        assert_eq!(out.files.len(), MapKind::ALL.len() + 1);
        for kind in MapKind::ALL {
            let png = out.dir.join(format!("Granite_{}.png", kind.role()));
            assert!(png.is_file(), "{kind:?} map missing at {}", png.display());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// PNGs land RAW and the recipe lands GZ — the at-rest split that makes a
    /// promotion a byte move instead of a transcode.
    #[test]
    fn maps_stay_raw_and_the_recipe_lands_gz() {
        let root = temp_root("atrest");
        let recipe = presets::basalt();
        let out = commit(&recipe, 32, &root).expect("commit writes");

        let png = out.dir.join("Basalt_BaseColor.png");
        assert!(png.is_file(), "the map is at its plain path");
        assert!(!out.dir.join("Basalt_BaseColor.png.gz").exists(), "a PNG must not be gzipped");
        let bytes = std::fs::read(&png).expect("read map");
        assert_eq!(&bytes[..4], b"\x89PNG", "the map is a real PNG, not a gz stream");

        let plain = out.dir.join("Basalt.texture.json");
        assert!(plain.with_extension("json.gz").is_file(), "the recipe is gz at rest");
        assert!(!plain.is_file(), "no stale raw twin beside the gz");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The committed recipe reads back byte-identical through the same seam the
    /// Content Manager will use — and rebuilds the same image, which is the whole
    /// reason the recipe is the artifact and the maps are its output.
    #[test]
    fn the_committed_recipe_round_trips_and_rebuilds() {
        let root = temp_root("roundtrip");
        let recipe = presets::sandstone();
        let out = commit(&recipe, 32, &root).expect("commit writes");

        let text =
            flicker_content::package::read_text(&out.dir.join("Sandstone.texture.json"))
                .expect("recipe reads back");
        let back: TextureRecipe = serde_json::from_str(&text).expect("recipe parses");
        assert_eq!(back, recipe);
        assert_eq!(bake(&back, 32), bake(&recipe, 32), "the recipe rebuilds its image");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Everything a commit writes must classify for the Content Manager's Type
    /// column — an artifact that lands as `Unknown` is one a reviewer cannot
    /// reason about.
    #[test]
    fn everything_committed_classifies() {
        let root = temp_root("classify");
        let out = commit(&presets::hematite(), 32, &root).expect("commit writes");
        for f in &out.files {
            let class = flicker_content::classify_package(f);
            assert_ne!(class, PackageClass::Unknown, "{} classified Unknown", f.display());
        }
        assert_eq!(
            flicker_content::classify_package(&out.dir.join("Hematite.texture.json")),
            PackageClass::TextureRecipe
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Nothing here may write into `package/`. Promotion is the Content Manager's
    /// job, and a bench that reached past staging would silently undo the review
    /// step the whole tier exists for.
    #[test]
    fn a_commit_touches_only_the_staging_root_it_was_given() {
        let root = temp_root("scope");
        let out = commit(&presets::granite(), 16, &root).expect("commit writes");
        for f in &out.files {
            assert!(f.starts_with(&root), "{} escaped the staging root", f.display());
        }
        assert!(!root.join("package").exists(), "a bench never writes into package/");
    }

    /// An authored name reaches here from a text field, so the folder name must be
    /// forced into the asset-naming standard rather than trusted.
    #[test]
    fn names_are_folded_into_the_asset_standard() {
        assert_eq!(asset_name("Granite", "granite"), "Granite");
        assert_eq!(asset_name("Red Sandstone", "x"), "Red-Sandstone");
        assert_eq!(asset_name("weird//name..here", "x"), "weird-name-here");
        // Nothing usable in the name: fall back to the id rather than write to a
        // path made of separators.
        assert_eq!(asset_name("///", "granite"), "granite");
        assert_eq!(asset_name("", ""), "Untitled");
        // No name can escape its folder.
        for wild in ["../../etc", "a/b", "..", "  "] {
            let n = asset_name(wild, "fallback");
            assert!(!n.contains('/') && !n.contains('.'), "{wild:?} folded to {n:?}");
        }
    }

    /// Each map ships in the narrowest form that carries its meaning, and must
    /// decode back to exactly the values the baker produced. Getting this wrong
    /// would be invisible in a file listing and wrong in every surface.
    #[test]
    fn maps_encode_narrow_and_decode_back_to_the_baked_values() {
        let root = temp_root("channels");
        let recipe = presets::granite();
        let out = commit(&recipe, 32, &root).expect("commit writes");
        let set = bake(&recipe, 32);

        for kind in MapKind::ALL {
            let path = out.dir.join(format!("Granite_{}.png", kind.role()));
            let img = image::open(&path).expect("map decodes");
            let expect_color = match kind {
                MapKind::BaseColor | MapKind::Normal => image::ColorType::Rgb8,
                _ => image::ColorType::L8,
            };
            assert_eq!(img.color(), expect_color, "{kind:?} shipped in the wrong form");

            // Widened back, every channel that carried meaning must be intact.
            let got = img.to_rgba8();
            let want = set.get(kind).unwrap();
            for (i, (g, w)) in got.chunks_exact(4).zip(want.pixels.chunks_exact(4)).enumerate() {
                assert_eq!(g[0], w[0], "{kind:?} texel {i} red changed");
                if expect_color == image::ColorType::Rgb8 {
                    assert_eq!((g[1], g[2]), (w[1], w[2]), "{kind:?} texel {i} colour changed");
                }
                assert_eq!(g[3], 255, "{kind:?} texel {i} lost its opaque alpha");
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
