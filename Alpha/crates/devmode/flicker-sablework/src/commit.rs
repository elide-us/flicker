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
//! `materials/<id:03>/<id:03>_<Map>.png` — the folder and the file stems are the
//! bound material's SLOT ID, zero-padded (plan 30FE7F58 P3). A promotion is then
//! the same relative path under a different root, and the map suffixes are the
//! content standard's own role names (`GolemBase_Low_BaseColor.png` is already
//! shaped this way).
//!
//! The display name is NOT in the path: it lives in the recipe and in
//! `materials.json`, so renaming a material is a pure data edit and never moves
//! a file. The one join both sides spell is [`flicker_content::bundle_dir`] —
//! this module calls it with the STAGING root, and still has no `package/` path
//! in it anywhere.

use std::io;
use std::path::{Path, PathBuf};

use flicker_texture::{bake, MapKind, TextureRecipe};

/// Why a commit was REFUSED before it baked or wrote anything.
///
/// Each refusal is named by a stringtable TOKEN, because the bench publishes
/// what it is told straight into `commit_status`: a Rust string here would be
/// English on every locale, past the STRINGS gate (which scans source
/// literals, not the copy a `String` carries at runtime). The specifics —
/// which recipe, which slot — go to the log, not the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `recipe.material` is `None`: an id-keyed bundle has no folder to land in.
    Unbound,
    /// The bound slot is Air (0) or inside the reserved exotic-emissive block
    /// (`flicker_materials::RESERVED_EXOTIC_FIRST..=255`, ruling BC95B08F D5).
    SlotClosed,
}

impl Refusal {
    /// The stringtable token the bench shows for this refusal.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Refusal::Unbound => "$sw_commit_unbound",
            Refusal::SlotClosed => "$sw_commit_slot_closed",
        }
    }
}

/// What can stop a commit: a refusal (nothing baked, nothing written) or an
/// I/O failure while writing the bundle.
#[derive(Debug)]
pub enum CommitError {
    Refused(Refusal),
    Io(io::Error),
}

impl From<io::Error> for CommitError {
    fn from(e: io::Error) -> Self {
        CommitError::Io(e)
    }
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The token, so a log line reads exactly what the screen shows.
            CommitError::Refused(why) => f.write_str(why.token()),
            CommitError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CommitError {}

/// What a commit wrote.
///
/// `Debug` so a refusal test can say what a commit wrongly produced instead of
/// failing with an opaque type.
#[derive(Debug)]
pub struct Committed {
    /// The asset folder, under the staging root.
    pub dir: PathBuf,
    /// Every file written, in write order — the recipe last, so a reader that
    /// sees the recipe knows the maps beside it are complete.
    pub files: Vec<PathBuf>,
}

/// Bake `recipe` at `size` and write the artifact folder into `staging_root`.
///
/// `staging_root` is passed in rather than looked up so a test can drive this
/// against a temp tree; the scene asks `flicker_content::roots().staging()`.
///
/// # Refusals
///
/// The bundle is keyed by the recipe's bound SLOT, so a recipe that names no
/// slot has no correct folder to land in and one bound to a slot that may not
/// hold a material has no legitimate one. Both come back as
/// [`CommitError::Refused`] BEFORE the bake runs — a rejected commit must not
/// cost the ~360 ms a baseline bake does — and each carries a stringtable
/// token, so what the bench shows is localized copy, never English from here.
pub fn commit(
    recipe: &TextureRecipe,
    size: u32,
    staging_root: &Path,
) -> Result<Committed, CommitError> {
    let Some(slot) = recipe.material else {
        return Err(CommitError::Refused(Refusal::Unbound));
    };
    // Slot 0 is Air and the top of the range is the reserved exotic-emissive
    // block; the boundary lives in `flicker_materials::slot_is_writable`, never
    // as a literal here.
    if !flicker_materials::slot_is_writable(slot) {
        return Err(CommitError::Refused(Refusal::SlotClosed));
    }

    // The ONE join the writer and the reader both spell.
    let dir = flicker_content::bundle_dir(staging_root, slot);
    std::fs::create_dir_all(&dir)?;
    // Every stem in the bundle is the slot, so a re-commit of the same slot
    // REPLACES it file for file rather than accumulating a second folder.
    let seg = flicker_materials::slot_dir(slot);

    let set = bake(recipe, size);
    let mut files = Vec::with_capacity(MapKind::ALL.len() + 1);

    for kind in MapKind::ALL {
        let Some(map) = set.get(kind) else { continue };
        let path = dir.join(format!("{seg}_{}.png", kind.role()));
        let png = encode_png(kind, &map.pixels, map.size).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{kind:?} PNG encode: {e}"),
            )
        })?;
        // Raw, not gz — see the module docs.
        std::fs::write(&path, png)?;
        files.push(path);
    }

    // The recipe LAST: it is the artifact that says "these maps exist", so a
    // reader that finds it can trust the folder beside it is finished.
    let json = serde_json::to_string_pretty(recipe)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let recipe_path = dir.join(format!("{seg}.texture.json"));
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
/// and the three-channel maps as **RGB8** (the bake guarantees an opaque alpha, and
/// `every_baked_map_is_opaque` in `flicker-texture` is what lets this drop it).
/// A decoder widens back to RGBA8 on load; PNG carries its own channel count, so
/// nothing has to be told which form a file is in.
fn encode_png(kind: MapKind, rgba: &[u8], size: u32) -> Result<Vec<u8>, image::ImageError> {
    // Three-channel maps are the COLOURS (`is_color`) plus `Normal`, which is a
    // vector rather than a colour but still needs all three components. Everything
    // else is one number replicated, and shipping THOSE as RGB8 wastes two thirds
    // of the file — while shipping a colour as L8 destroys its hue, which is how
    // an `Emit` map would have lost the whole point of being a colour.
    let (color, pixels) = match kind {
        _ if kind.is_color() || kind == MapKind::Normal => (
            image::ColorType::Rgb8,
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect::<Vec<u8>>(),
        ),
        // Scalar: one channel is the whole map.
        _ => (
            image::ColorType::L8,
            rgba.as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[0])
                .collect::<Vec<u8>>(),
        ),
    };
    let mut out = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut out,
        &pixels,
        size,
        size,
        color,
        image::ImageFormat::Png,
    )?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_content::PackageClass;
    use flicker_texture::presets;

    /// Unique per process (pid) and per call (atomic counter), so two concurrent
    /// `cargo test` runs never share a fixed dir and stomp each other's fixtures
    /// — the failure mode that makes a suite look broken when it is only being
    /// run twice at once (trap D373B875).
    fn temp_root(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "sablework_commit_{tag}_{}_{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// The slot `recipe` is bound to. The CATALOG is the source of which slot a
    /// patch owns, so a test reads it off the recipe instead of restating it.
    fn slot_of(recipe: &TextureRecipe) -> flicker_materials::MaterialId {
        recipe
            .material
            .expect("every factory patch is bound to a slot")
    }

    /// A commit writes the whole artifact folder in the layout `package/` uses, so
    /// a promotion is the same relative path under a different root — and that
    /// path is composed by the seam both sides share.
    #[test]
    fn a_commit_writes_the_maps_and_the_recipe_in_package_layout() {
        let root = temp_root("layout");
        let recipe = presets::granite();
        let slot = slot_of(&recipe);
        let out = commit(&recipe, 32, &root).expect("commit writes");

        assert_eq!(out.dir, flicker_content::bundle_dir(&root, slot));
        // And what that composes to, spelled out once: the zero-padded slot id.
        assert_eq!(out.dir, root.join("materials").join("010"));
        assert_eq!(out.files.len(), MapKind::ALL.len() + 1);
        for kind in MapKind::ALL {
            let png = out.dir.join(format!("010_{}.png", kind.role()));
            assert!(png.is_file(), "{kind:?} map missing at {}", png.display());
        }
        assert!(
            out.dir.join("010.texture.json.gz").is_file(),
            "the recipe stem is the slot too"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A recipe bound to nothing has no correct folder to land in — an id-keyed
    /// bundle needs a slot, and guessing a name is the failure this removes. It
    /// must be refused BEFORE the bake, having written nothing at all.
    #[test]
    fn an_unbound_recipe_is_refused_before_it_costs_a_bake() {
        let root = temp_root("unbound");
        let recipe = flicker_texture::random(0x5EED);
        assert!(recipe.material.is_none(), "a rolled recipe is scratch");

        let why = match commit(&recipe, 32, &root) {
            Err(CommitError::Refused(why)) => why,
            other => panic!("an unbound recipe is a REFUSAL, not an I/O failure: {other:?}"),
        };
        assert_eq!(why, Refusal::Unbound);
        assert!(
            why.token().starts_with('$'),
            "the refusal reaches the screen as a stringtable token: {}",
            why.token()
        );
        assert!(
            !root.exists(),
            "a refused commit writes nothing — not even the folder"
        );
    }

    /// Air and the reserved exotic-emissive block are not authorable slots, so a
    /// recipe bound to one is refused as loudly as an unbound one.
    #[test]
    fn air_and_the_reserved_block_are_refused() {
        let root = temp_root("reserved");
        for slot in [0, flicker_materials::RESERVED_EXOTIC_FIRST] {
            let mut recipe = presets::granite();
            recipe.material = Some(slot);
            let why = match commit(&recipe, 32, &root) {
                Err(CommitError::Refused(why)) => why,
                other => panic!("slot {slot} is a REFUSAL, not an I/O failure: {other:?}"),
            };
            assert_eq!(why, Refusal::SlotClosed, "slot {slot}");
            assert!(why.token().starts_with('$'), "slot {slot}: {}", why.token());
            assert!(
                !root.exists(),
                "slot {slot} wrote something it should have refused"
            );
        }
    }

    /// The slot IS the identity, so re-committing one replaces its bundle in
    /// place. Under the old name keying an edit produced a second folder; here
    /// the folder must still hold exactly the seven maps and one recipe, and the
    /// recipe must be the NEW one (D-OVERWRITE — an announced behaviour change).
    #[test]
    fn a_second_commit_replaces_the_bundle_rather_than_accumulating() {
        let root = temp_root("overwrite");
        let first = presets::granite();
        let slot = slot_of(&first);
        let out = commit(&first, 32, &root).expect("first commit writes");

        let mut second = first.clone();
        second.seed ^= 0xABCD_1234;
        second.name = "Granite Rework".into();
        let again = commit(&second, 32, &root).expect("second commit writes");
        assert_eq!(again.dir, out.dir, "the same slot is the same folder");

        let mut pngs = 0;
        let mut recipes = 0;
        for entry in std::fs::read_dir(&again.dir).expect("bundle folder reads") {
            let p = entry.expect("dir entry").path();
            match p.extension().and_then(|e| e.to_str()) {
                Some("png") => pngs += 1,
                Some("gz") => recipes += 1,
                other => panic!("unexpected file {p:?} ({other:?})"),
            }
        }
        assert_eq!(pngs, MapKind::ALL.len(), "seven maps, not fourteen");
        assert_eq!(recipes, 1, "one recipe");

        let text = flicker_content::package::read_text(
            &flicker_content::bundle_dir(&root, slot).join("010.texture.json"),
        )
        .expect("recipe reads back");
        let back: TextureRecipe = serde_json::from_str(&text).expect("recipe parses");
        assert_eq!(back, second, "the bundle carries the SECOND recipe");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// PNGs land RAW and the recipe lands GZ — the at-rest split that makes a
    /// promotion a byte move instead of a transcode.
    #[test]
    fn maps_stay_raw_and_the_recipe_lands_gz() {
        let root = temp_root("atrest");
        let recipe = presets::basalt();
        assert_eq!(slot_of(&recipe), 11, "basalt is slot 11");
        let out = commit(&recipe, 32, &root).expect("commit writes");

        let png = out.dir.join("011_BaseColor.png");
        assert!(png.is_file(), "the map is at its plain path");
        assert!(
            !out.dir.join("011_BaseColor.png.gz").exists(),
            "a PNG must not be gzipped"
        );
        let bytes = std::fs::read(&png).expect("read map");
        assert_eq!(
            &bytes[..4],
            b"\x89PNG",
            "the map is a real PNG, not a gz stream"
        );

        let plain = out.dir.join("011.texture.json");
        assert!(
            plain.with_extension("json.gz").is_file(),
            "the recipe is gz at rest"
        );
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
        assert_eq!(slot_of(&recipe), 12, "sandstone is slot 12");
        let out = commit(&recipe, 32, &root).expect("commit writes");

        let text = flicker_content::package::read_text(&out.dir.join("012.texture.json"))
            .expect("recipe reads back");
        let back: TextureRecipe = serde_json::from_str(&text).expect("recipe parses");
        assert_eq!(back, recipe);
        assert_eq!(
            bake(&back, 32),
            bake(&recipe, 32),
            "the recipe rebuilds its image"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Everything a commit writes must classify for the Content Manager's Type
    /// column — an artifact that lands as `Unknown` is one a reviewer cannot
    /// reason about.
    #[test]
    fn everything_committed_classifies() {
        let root = temp_root("classify");
        let recipe = presets::hematite();
        assert_eq!(slot_of(&recipe), 40, "hematite is slot 40");
        let out = commit(&recipe, 32, &root).expect("commit writes");
        for f in &out.files {
            let class = flicker_content::classify_package(f);
            assert_ne!(
                class,
                PackageClass::Unknown,
                "{} classified Unknown",
                f.display()
            );
        }
        assert_eq!(
            flicker_content::classify_package(&out.dir.join("040.texture.json")),
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
            assert!(
                f.starts_with(&root),
                "{} escaped the staging root",
                f.display()
            );
        }
        assert!(
            !root.join("package").exists(),
            "a bench never writes into package/"
        );
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
            let path = out.dir.join(format!("010_{}.png", kind.role()));
            let img = image::open(&path).expect("map decodes");
            // COLOUR maps (base colour, emit) and `Normal` keep three channels; a
            // colour shipped as L8 would lose its hue entirely.
            let expect_color = if kind.is_color() || kind == MapKind::Normal {
                image::ColorType::Rgb8
            } else {
                image::ColorType::L8
            };
            assert_eq!(
                img.color(),
                expect_color,
                "{kind:?} shipped in the wrong form"
            );

            // Widened back, every channel that carried meaning must be intact.
            let got = img.to_rgba8();
            let want = set.get(kind).unwrap();
            for (i, (g, w)) in got
                .as_chunks::<4>()
                .0
                .iter()
                .zip(want.pixels.as_chunks::<4>().0.iter())
                .enumerate()
            {
                assert_eq!(g[0], w[0], "{kind:?} texel {i} red changed");
                if expect_color == image::ColorType::Rgb8 {
                    assert_eq!(
                        (g[1], g[2]),
                        (w[1], w[2]),
                        "{kind:?} texel {i} colour changed"
                    );
                }
                assert_eq!(g[3], 255, "{kind:?} texel {i} lost its opaque alpha");
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
