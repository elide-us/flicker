//! THE MATERIAL BUNDLE — the promotable unit of plan 30FE7F58 phase P3.
//!
//! A bundle is one material's *appearance*: the authored recipe plus the seven
//! baked maps it produced, living together under `<root>/materials/<id:03>/`
//! and keyed by the catalog's SLOT ID (`flicker_materials::slot_dir`), never by
//! the authored display name. Keying by the slot is what makes a rename a pure
//! data edit — the folder never moves, because the folder is the id.
//!
//! The staging side and the package side are the SAME relative path under a
//! different root, so promoting a bundle stays what a promotion has always been
//! here: a byte move plus one manifest row. That is why every entry point in
//! this module takes the root as an argument rather than asking the roots
//! service — the writer (Sablework's commit, against the staging root) and the
//! reader (the Quartermaster and, later, the engine, against the package root)
//! spell one path through one seam.
//!
//! What this module deliberately does NOT do yet: it hands back PATHS, it does
//! not decode maps and it does not know the role vocabulary. Nothing in the
//! engine reads `package/materials/` until the material-texture loader lands
//! (P5), and a typed map set here would be API ahead of its first consumer.
//! The map set's arity and role names have ONE owner, `flicker_texture::MapKind`
//! (a pure CPU leaf crate), and a reader that needs them takes that crate
//! directly rather than through a second spelling here.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flicker_materials::MaterialId;

use crate::{browse, package};

/// The sub-tree every material's appearance lands in, under whichever root it
/// is being written to. One name, so the staging and package sides cannot
/// drift.
pub const MATERIALS_DIR: &str = "materials";

/// The folder a slot's bundle occupies under `root` — the ONE join the writer
/// and the reader both spell, so neither can invent a second layout.
#[must_use]
pub fn bundle_dir(root: &Path, slot: MaterialId) -> PathBuf {
    root.join(MATERIALS_DIR)
        .join(flicker_materials::slot_dir(slot))
}

/// One material's bundle as it sits on disk.
pub struct MaterialBundle {
    /// The slot this bundle defines.
    pub slot: MaterialId,
    /// The bundle folder ([`bundle_dir`]).
    pub dir: PathBuf,
    /// The recipe's LOGICAL path, `<id:03>.texture.json` — at rest it is the
    /// `.gz` twin, which is exactly the detail [`MaterialBundle::recipe_text`]
    /// exists to keep out of callers.
    pub recipe: PathBuf,
    /// The baked maps, physical paths, sorted.
    pub maps: Vec<PathBuf>,
}

/// The bundle `root` holds for `slot`, or `None` when it holds none.
///
/// **A slot is DEFINED (plan 30FE7F58 P3) iff `bundle_at(package_root, slot)`
/// is `Some`.** That is the whole definition — there is no second predicate
/// spelling it, and the deferred 256-slot materials tab builds its own table by
/// looping this.
///
/// The RECIPE is what says a folder is finished: the commit writes it last,
/// after all seven maps, so a folder with maps but no recipe is a bake caught
/// half-written and is correctly not a bundle. Absence is checked gz-transparently
/// through [`package::file_exists`], because at rest the recipe is `.gz`.
#[must_use]
pub fn bundle_at(root: &Path, slot: MaterialId) -> Option<MaterialBundle> {
    let dir = bundle_dir(root, slot);
    let recipe = dir.join(format!(
        "{}.texture.json",
        flicker_materials::slot_dir(slot)
    ));
    if !package::file_exists(&recipe) {
        return None;
    }
    let maps = browse::files_under(&dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    Some(MaterialBundle {
        slot,
        dir,
        recipe,
        maps,
    })
}

impl MaterialBundle {
    /// The recipe's text, read through the gz-at-rest seam by its LOGICAL path.
    ///
    /// Here rather than at every call site so the `.gz` twin stays a fact of
    /// this seam and not of the Quartermaster, which only wants to parse it.
    pub fn recipe_text(&self) -> Result<String> {
        package::read_text(&self.recipe)
            .with_context(|| format!("reading material recipe {}", self.recipe.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seven maps a bake always emits (`flicker_texture::MapKind::ALL`),
    /// spelled as strings — the bundle seam knows paths, not the role vocabulary.
    const MAPS: [&str; 7] = [
        "BaseColor",
        "Normal",
        "Roughness",
        "Metallic",
        "AO",
        "Height",
        "Emit",
    ];

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Unique per-process (pid) and per-call (atomic counter) so two concurrent
    /// `cargo test` processes never share a fixed dir and stomp each other's
    /// fixtures — the failure mode that makes this suite look broken when it
    /// is only being run twice at once.
    fn scratch(name: &str) -> Scratch {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let d = Scratch(std::env::temp_dir().join(format!(
            "flicker_bundle_{name}_{}_{seq}",
            std::process::id()
        )));
        let _ = std::fs::remove_dir_all(&d.0);
        std::fs::create_dir_all(&d.0).unwrap();
        d
    }

    /// Write the maps of `stem` into `dir` (raw `.png` bytes — binaries never gz).
    fn write_maps(dir: &Path, stem: &str) {
        std::fs::create_dir_all(dir).unwrap();
        for m in MAPS {
            std::fs::write(
                dir.join(format!("{stem}_{m}.png")),
                [0x89, b'P', b'N', b'G'],
            )
            .unwrap();
        }
    }

    #[test]
    fn a_bundle_resolves_by_slot_with_its_recipe_and_every_map() {
        let d = scratch("resolves");
        let dir = bundle_dir(&d.0, 10);
        write_maps(&dir, "010");
        package::write_text(&dir.join("010.texture.json"), r#"{"id":"granite"}"#).unwrap();

        let b = bundle_at(&d.0, 10).expect("slot 10 is defined");
        assert_eq!(b.slot, 10);
        assert_eq!(b.dir, d.0.join("materials").join("010"));
        assert_eq!(b.recipe, dir.join("010.texture.json"));
        assert_eq!(b.maps.len(), 7, "all seven baked maps: {:?}", b.maps);
        let mut sorted = b.maps.clone();
        sorted.sort();
        assert_eq!(b.maps, sorted, "maps come back sorted");
        // The recipe is not mistaken for a map, and the gz twin is not either.
        assert!(b.maps.iter().all(|p| p.extension().unwrap() == "png"));
    }

    #[test]
    fn maps_without_a_recipe_are_not_a_bundle() {
        let d = scratch("no_recipe");
        write_maps(&bundle_dir(&d.0, 10), "010");
        assert!(
            bundle_at(&d.0, 10).is_none(),
            "the recipe is written LAST — without it the folder is a half-finished bake, \
             and the P3 DEFINED predicate must say so"
        );
    }

    #[test]
    fn the_recipe_resolves_and_reads_through_the_gz_seam() {
        let d = scratch("gz");
        let dir = bundle_dir(&d.0, 47);
        write_maps(&dir, "047");
        let logical = dir.join("047.texture.json");
        let written = package::write_text(&logical, r#"{"id":"basalt","seed":7}"#).unwrap();

        assert_eq!(written, dir.join("047.texture.json.gz"), "at rest it is gz");
        assert!(!logical.is_file(), "no raw twin beside the gz");
        let b = bundle_at(&d.0, 47).expect("gz recipe is found through file_exists");
        assert_eq!(b.recipe, logical, "the bundle carries the LOGICAL path");
        assert_eq!(b.recipe_text().unwrap(), r#"{"id":"basalt","seed":7}"#);
    }

    #[test]
    fn a_legacy_name_keyed_folder_is_not_a_slot_bundle() {
        let d = scratch("legacy");
        // Exactly the shape of the one bundle staged before P3: keyed by name.
        let legacy = d.0.join("materials").join("Granite");
        write_maps(&legacy, "Granite");
        package::write_text(&legacy.join("Granite.texture.json"), "{}").unwrap();

        assert!(
            bundle_at(&d.0, 10).is_none(),
            "a name-keyed folder defines no slot — it is visible to a reviewer, never coerced"
        );
        assert_eq!(
            bundle_dir(&d.0, 10),
            d.0.join("materials").join("010"),
            "and the path the writer will use is the id one"
        );
    }
}
