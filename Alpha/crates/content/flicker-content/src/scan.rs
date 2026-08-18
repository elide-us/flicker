//! Ingest — scan a folder and classify what's in it, then pick what to rig.
//!
//! The first pipeline stage: "point at a folder → determine what to rig." Classification here is
//! coarse (by extension + a name heuristic) because telling a mesh FBX from an animation FBX for
//! certain needs the FBX parser (the next slice) — but a Meshy export is explicit enough
//! (`..._Character_output.fbx` vs `..._Animation_Walking...fbx`) that the heuristic picks the right
//! riggable candidate today, and the parser will confirm/refine it.

use std::path::{Path, PathBuf};

/// What a file in a content folder IS, by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An FBX carrying a mesh + skeleton to rig (provisional until the parser confirms geometry).
    MeshFbx,
    /// An FBX carrying animation (motion curves), from which no rig is produced.
    AnimFbx,
    /// A BVH motion clip (already handled by `tools/retarget_bvh.py`).
    Bvh,
    /// A texture map (PNG / TGA / JPG / TIFF).
    Texture,
    /// An already-baked `flicker.rig` / asset JSON.
    Rig,
    /// A source→internal `manifest.json`.
    Manifest,
    /// Anything else (docs, .blend, .mtl, …).
    Other,
}

/// One classified file in a scanned folder.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
}

/// The result of scanning a content folder: every file classified, plus the indices of the riggable
/// candidates. If there is more than one candidate the caller MUST ask which to rig
/// ([`Scan::needs_selection`]) — the disambiguation guard.
#[derive(Debug, Clone)]
pub struct Scan {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    /// Indices into `entries` that are riggable ([`Kind::MeshFbx`]).
    pub riggable: Vec<usize>,
}

impl Scan {
    /// More than one riggable thing → the editor must ask which to load.
    pub fn needs_selection(&self) -> bool {
        self.riggable.len() > 1
    }

    /// The single riggable candidate, if unambiguous.
    pub fn sole_riggable(&self) -> Option<&Entry> {
        match self.riggable.as_slice() {
            [i] => Some(&self.entries[*i]),
            _ => None,
        }
    }

    /// All riggable candidates (what a selection prompt would list).
    pub fn candidates(&self) -> impl Iterator<Item = &Entry> {
        self.riggable.iter().map(move |&i| &self.entries[i])
    }

    /// Every entry of a given kind (e.g. the textures to bring along, the anim clips to retarget).
    pub fn of_kind(&self, k: Kind) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.kind == k)
    }
}

/// What a whole SOURCE FOLDER is — the Classify stage's question, one level up from the per-file
/// [`Kind`]. A folder of thirty PNGs and one FBX is still one asset; this says which asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetClass {
    /// A character: mesh + rig + textures. The only class the conform/bake path handles today.
    Skin,
    /// An attachable object — a weapon, a garment, set dressing.
    Prop,
    /// Motion only, to retarget onto the canonical rig; no rig is produced.
    Animation,
}

impl AssetClass {
    /// Stable id, for the editor's override control and for logs.
    pub fn id(self) -> &'static str {
        match self {
            AssetClass::Skin => "skin",
            AssetClass::Prop => "prop",
            AssetClass::Animation => "animation",
        }
    }
}

/// A prop's sub-type — only meaningful when the class is [`AssetClass::Prop`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    Weapon,
    Clothing,
    Environment,
    Accessory,
}

impl PropKind {
    pub fn id(self) -> &'static str {
        match self {
            PropKind::Weapon => "weapon",
            PropKind::Clothing => "clothing",
            PropKind::Environment => "environment",
            PropKind::Accessory => "accessory",
        }
    }
}

/// What [`classify_asset`] concluded, and **why**.
///
/// `confidence` is DERIVED — the share of the evidence weight that actually agreed, never a
/// decorative number. `evidence` carries the individual signals so the editor can show its
/// reasoning instead of an unexplained percentage.
#[derive(Debug, Clone)]
pub struct AssetReport {
    pub class: AssetClass,
    /// Best-guess sub-type; carried for any class so an override to Prop has a starting point.
    pub prop: PropKind,
    /// 0..1, derived from agreeing signals.
    pub confidence: f32,
    pub evidence: Vec<String>,
}

/// Bone count above which a skeleton is a character rather than a rigged prop. A Meshy biped
/// arrives at 24 bones pre-conform and 66 after; a prop carries a handful at most.
const BIPED_BONES: usize = 20;

/// Classify a scanned folder as a whole. `bones` is the parsed skeleton's bone count when Analyze
/// has run — by far the strongest signal, so the report sharpens once it is known rather than
/// pretending to certainty at scan time.
pub fn classify_asset(scan: &Scan, bones: Option<usize>) -> AssetReport {
    let meshes = scan.riggable.len();
    let anims = scan.of_kind(Kind::AnimFbx).count() + scan.of_kind(Kind::Bvh).count();
    let textures = scan.of_kind(Kind::Texture).count();

    // Each signal votes with a weight; confidence is the winner's share of the votes cast, so
    // agreeing evidence raises it and conflicting evidence lowers it on its own.
    let (mut skin, mut prop, mut animation) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut evidence = Vec::new();

    match bones {
        Some(n) if n >= BIPED_BONES => {
            skin += 3.0;
            evidence.push(format!("{n} bones — a biped skeleton"));
        }
        Some(0) => {
            prop += 3.0;
            evidence.push("no skeleton — a static mesh".into());
        }
        Some(n) => {
            prop += 2.0;
            evidence.push(format!("{n} bones — too few for a character"));
        }
        None => evidence.push("not parsed yet — folder shape only".into()),
    }

    if meshes == 0 && anims > 0 {
        animation += 3.0;
        evidence.push(format!("{anims} motion file(s), no mesh"));
    } else if meshes > 0 {
        evidence.push(format!("{meshes} riggable mesh(es)"));
        if anims > 0 {
            skin += 1.0;
            evidence.push(format!("{anims} companion animation(s)"));
        }
    }

    if textures >= 2 {
        skin += 1.0;
        prop += 0.5;
        evidence.push(format!("{textures} texture maps"));
    } else if textures == 0 && meshes > 0 {
        prop += 0.5;
        evidence.push("no textures".into());
    }

    // Name hints from the folder itself, so "…/Corset-Duster" reads as clothing before anything
    // is parsed.
    let stem = scan
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = prop_kind(&stem);
    if let Some(k) = kind {
        prop += 1.0;
        evidence.push(format!("folder name reads as {}", k.id()));
    }

    let (class, score) = [
        (AssetClass::Skin, skin),
        (AssetClass::Prop, prop),
        (AssetClass::Animation, animation),
    ]
    .into_iter()
    .fold((AssetClass::Skin, f32::MIN), |best, c| {
        if c.1 > best.1 {
            c
        } else {
            best
        }
    });

    let total = skin + prop + animation;
    let confidence = if total > 0.0 {
        (score / total).clamp(0.0, 1.0)
    } else {
        0.0
    };
    AssetReport {
        class,
        prop: kind.unwrap_or(PropKind::Accessory),
        confidence,
        evidence,
    }
}

/// Prop sub-type from a name. `None` when nothing matches, so the caller can tell "no evidence"
/// from "looks like an accessory".
fn prop_kind(name: &str) -> Option<PropKind> {
    const WEAPON: &[&str] = &[
        "sword", "axe", "bow", "gun", "blade", "staff", "dagger", "spear", "shield", "hammer",
        "mace", "rifle", "pistol", "wand",
    ];
    const CLOTHING: &[&str] = &[
        "shirt", "coat", "duster", "dress", "corset", "robe", "pants", "boot", "glove", "hat",
        "cloak", "armor", "armour", "helm", "skirt", "hood",
    ];
    const ENVIRONMENT: &[&str] = &[
        "rock", "tree", "wall", "door", "crate", "barrel", "table", "chair", "fence", "pillar",
        "ruin", "bridge",
    ];
    const ACCESSORY: &[&str] = &[
        "ring", "amulet", "necklace", "belt", "pouch", "bag", "earring", "hair",
    ];
    for (list, kind) in [
        (WEAPON, PropKind::Weapon),
        (CLOTHING, PropKind::Clothing),
        (ENVIRONMENT, PropKind::Environment),
        (ACCESSORY, PropKind::Accessory),
    ] {
        if list.iter().any(|h| name.contains(h)) {
            return Some(kind);
        }
    }
    None
}

/// Classify a single path by extension + name.
pub fn classify(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let fname = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "fbx" => {
            if looks_like_animation(&stem) {
                Kind::AnimFbx
            } else {
                Kind::MeshFbx
            }
        }
        "bvh" => Kind::Bvh,
        "tga" | "png" | "jpg" | "jpeg" | "tif" | "tiff" => Kind::Texture,
        "json" => {
            if fname == "manifest.json" {
                Kind::Manifest
            } else {
                Kind::Rig
            }
        }
        _ => Kind::Other,
    }
}

/// Name heuristic ONLY (the FBX parser is the real determinant, next slice): an FBX whose name reads
/// like a motion clip is an animation, not a mesh to rig. Meshy is explicit ("..._Animation_..."),
/// and this also catches bare locomotion/action verbs.
fn looks_like_animation(stem: &str) -> bool {
    const HINTS: &[&str] = &[
        "animation",
        "walk",
        "run",
        "jog",
        "idle",
        "jump",
        "crouch",
        "attack",
        "death",
        "dame",
        "hit",
        "roll",
        "climb",
        "turn",
        "strafe",
        "dodge",
        "sprint",
        "crawl",
        "vault",
        "land",
        "stomp",
        "sit",
        "stand",
        "dance",
        "wave",
    ];
    HINTS.iter().any(|h| stem.contains(h))
}

// ───────────────────────────────────────────────────────────────────────────
// PACKAGE-SIDE classification — what a PROCESSED file is.
//
// [`Kind`] above answers "what is this SOURCE file" by extension, for the ingest
// stage. This answers the other tree's question: given a file in `package/` or
// `staging/`, what kind of content IS it? That cannot be read off the name —
// 597 of ~600 files are generically `<name>.json.gz`, and rigs AND clips both
// declare `"format": "flicker.rig"`.
// ───────────────────────────────────────────────────────────────────────────

/// What a processed content file is — the Type column in the content browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageClass {
    /// A skinned asset: skeleton + mesh (`flicker.rig` carrying geometry).
    Rig,
    /// Motion only: `flicker.rig` carrying clips and an empty mesh.
    Clip,
    /// An animation state graph (`flicker.pack`) — the combat system's input.
    CombatPack,
    /// A retarget base pose (`flicker.rbp`).
    RetargetBasePose,
    /// A baked voxel cluster (no `format` field at all — its own shape).
    Bake,
    /// A camera cinematic (`.flight`).
    Flight,
    /// A captured world (`.epoch`).
    Epoch,
    /// A texture map.
    Texture,
    /// A Sablework texture RECIPE (`<name>.texture.json`) — the synthesizer
    /// settings a map set was baked from. Distinct from [`Self::Texture`]: the
    /// recipe is the authored artifact and the maps are its rebuildable output,
    /// so the browser must not present them as the same kind of thing.
    TextureRecipe,
    /// A font face.
    Font,
    /// A README / licence riding along in the tree.
    Doc,
    /// A directory.
    Folder,
    /// Present, readable, and none of the above — reported honestly rather than
    /// guessed at.
    Unknown,
}

impl PackageClass {
    /// Stable id, for style lookups and logs.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rig => "rig",
            Self::Clip => "clip",
            Self::CombatPack => "pack",
            Self::RetargetBasePose => "rbp",
            Self::Bake => "bake",
            Self::Flight => "flight",
            Self::Epoch => "epoch",
            Self::Texture => "texture",
            Self::TextureRecipe => "texture_recipe",
            Self::Font => "font",
            Self::Doc => "doc",
            Self::Folder => "folder",
            Self::Unknown => "unknown",
        }
    }
}

/// How much of a file's DECOMPRESSED head the sniff reads.
///
/// Empirically sufficient for every file in the tree: a clip's `"clips"` array
/// starts by ~52 KB (its mesh is empty), while a rig's populated mesh pushes
/// everything after it far beyond any prefix — which is itself the signal.
const SNIFF_BYTES: usize = 64 * 1024;

/// Classify a processed content file.
///
/// **Reads only the head of the file.** Fully parsing every entry to list one
/// folder is not affordable — a single animation clip is ~6 MB of JSON, and the
/// tree holds hundreds. Measured over the real tree: ~180× faster than parsing,
/// with identical results on all 600 files.
///
/// ## Why the sniff cannot key off `"format"` alone
///
/// The tree carries **two serialization layouts**, and content-addressing must
/// survive both:
///
/// - *field order* — `{"format": "flicker.rig", …}`, `format` first;
/// - *alphabetical order* — `{"clips":[…], …, "format": …}`, where `format`
///   lands at byte 5,881,063 of a 5.9 MB clip, far past any prefix.
///
/// So the discriminator is layout-agnostic: a **non-empty `clips` array visible
/// in the head means a clip** (its mesh must be empty, or the mesh's bulk would
/// have pushed `clips` out of the prefix), and a populated `mesh` means a rig.
///
/// ## Entangled bundles
///
/// A few raw Katanami exports carry a mesh AND a clip in one file (the README's
/// "entangled raw export"). Geometry wins, matching how the loader already
/// picks rig authority by mesh-vertex count.
pub fn classify_package(path: &Path) -> PackageClass {
    if path.is_dir() {
        return PackageClass::Folder;
    }
    // Strip the at-rest `.gz` so the LOGICAL extension drives the fast paths.
    let logical = path.to_string_lossy();
    let logical = logical
        .strip_suffix(".gz")
        .unwrap_or(&logical)
        .to_ascii_lowercase();

    // Cheap, unambiguous names first — no read at all.
    if logical.ends_with(".png")
        || logical.ends_with(".jpg")
        || logical.ends_with(".jpeg")
        || logical.ends_with(".tga")
        || logical.ends_with(".tif")
        || logical.ends_with(".tiff")
    {
        return PackageClass::Texture;
    }
    if logical.ends_with(".ttf") || logical.ends_with(".otf") {
        return PackageClass::Font;
    }
    if logical.ends_with(".md") || logical.ends_with(".txt") {
        return PackageClass::Doc;
    }
    if logical.ends_with(".flight") {
        return PackageClass::Flight;
    }
    if logical.ends_with(".epoch") {
        return PackageClass::Epoch;
    }
    if logical.ends_with(".pack.json") {
        return PackageClass::CombatPack;
    }
    if logical.ends_with(".rbp.json") {
        return PackageClass::RetargetBasePose;
    }
    if logical.ends_with(".texture.json") {
        return PackageClass::TextureRecipe;
    }
    if !logical.ends_with(".json") {
        return PackageClass::Unknown;
    }

    let Ok(head) = flicker_core::compression::read_bytes_prefix(path, SNIFF_BYTES) else {
        return PackageClass::Unknown;
    };
    classify_package_head(&head)
}

/// A recipe is named by its EXTENSION, not sniffed: `<name>.texture.json` is the
/// same cheap unambiguous path `.pack.json` and `.rbp.json` take, and it costs no
/// read at all. Pinned because the extension is a contract between the Sablework
/// bench (which writes it) and the Content Manager (which names it).
#[cfg(test)]
mod texture_recipe_class_tests {
    use super::*;

    #[test]
    fn a_recipe_is_classified_by_its_extension() {
        assert_eq!(
            classify_package(Path::new("staging/materials/Granite/Granite.texture.json")),
            PackageClass::TextureRecipe
        );
        // At rest it is gz, and the logical extension still drives the answer.
        assert_eq!(
            classify_package(Path::new(
                "staging/materials/Granite/Granite.texture.json.gz"
            )),
            PackageClass::TextureRecipe
        );
        // The maps it produced stay TEXTURES — the recipe is the authored thing,
        // the maps are its output, and the browser must tell them apart.
        assert_eq!(
            classify_package(Path::new("staging/materials/Granite/Granite_BaseColor.png")),
            PackageClass::Texture
        );
        // Ids are what a style lookup keys on, so they must not collide.
        assert_ne!(PackageClass::TextureRecipe.id(), PackageClass::Texture.id());
    }
}

/// The sniff itself, over an already-read head — split out so it is testable
/// without a file on disk.
pub fn classify_package_head(head: &[u8]) -> PackageClass {
    match declared_format(head) {
        Some(b"flicker.pack") => return PackageClass::CombatPack,
        Some(b"flicker.rbp") => return PackageClass::RetargetBasePose,
        // A cluster bake declares no `format` — its `lod` id is the shape.
        None if find_key(head, b"lod").is_some() => return PackageClass::Bake,
        _ => {}
    }
    if array_is_populated(head, b"clips") == Some(true) {
        return PackageClass::Clip;
    }
    if declared_format(head) == Some(b"flicker.rig") || find_key(head, b"mesh").is_some() {
        return PackageClass::Rig;
    }
    PackageClass::Unknown
}

/// Byte offset just past `"<key>"` in `head`, if present.
fn find_key(head: &[u8], key: &[u8]) -> Option<usize> {
    let mut needle = Vec::with_capacity(key.len() + 2);
    needle.push(b'"');
    needle.extend_from_slice(key);
    needle.push(b'"');
    head.windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + needle.len())
}

/// The `"format"` string value, when it is visible in the head.
fn declared_format(head: &[u8]) -> Option<&[u8]> {
    let after = find_key(head, b"format")?;
    let rest = &head[after..head.len().min(after + 48)];
    let open = rest.iter().position(|&b| b == b'"')?;
    let tail = &rest[open + 1..];
    let close = tail.iter().position(|&b| b == b'"')?;
    Some(&tail[..close])
}

/// `Some(true)` when `"<key>"` names an array with at least one element,
/// `Some(false)` when it is empty, `None` when it is not visible in the head.
fn array_is_populated(head: &[u8], key: &[u8]) -> Option<bool> {
    let after = find_key(head, key)?;
    let rest = trim_start(&head[after..]);
    let rest = if rest.first() == Some(&b':') {
        trim_start(&rest[1..])
    } else {
        rest
    };
    if rest.first() != Some(&b'[') {
        return None;
    }
    Some(trim_start(&rest[1..]).first() != Some(&b']'))
}

fn trim_start(b: &[u8]) -> &[u8] {
    let n = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    &b[n..]
}

/// Recursively scan a folder and classify every file under it. Riggable candidates are the
/// [`Kind::MeshFbx`] entries.
pub fn scan_folder(root: &Path) -> std::io::Result<Scan> {
    let mut entries = Vec::new();
    collect(root, &mut entries)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let riggable = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == Kind::MeshFbx)
        .map(|(i, _)| i)
        .collect();
    Ok(Scan {
        root: root.to_path_buf(),
        entries,
        riggable,
    })
}

fn collect(dir: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
    for de in std::fs::read_dir(dir)? {
        let path = de?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.is_file() {
            out.push(Entry {
                kind: classify(&path),
                path,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"").unwrap();
    }

    #[test]
    fn classify_by_extension_and_name() {
        // The real Meshy split: "Character_output" is the mesh, "Animation_Walking" is motion.
        assert_eq!(
            classify(Path::new("Female_Human_Base_Character_output.fbx")),
            Kind::MeshFbx
        );
        assert_eq!(
            classify(Path::new(
                "Female_Human_Base_Animation_Walking_frame_rate_60.fbx"
            )),
            Kind::AnimFbx
        );
        assert_eq!(classify(Path::new("walk_forward.bvh")), Kind::Bvh);
        assert_eq!(
            classify(Path::new("body_texture_0_normal.png")),
            Kind::Texture
        );
        assert_eq!(classify(Path::new("skin.TGA")), Kind::Texture); // case-insensitive
        assert_eq!(classify(Path::new("manifest.json")), Kind::Manifest);
        assert_eq!(classify(Path::new("PrismHumanBaseA.json")), Kind::Rig);
        assert_eq!(classify(Path::new("readme.txt")), Kind::Other);
    }

    #[test]
    fn scan_a_meshy_folder_finds_one_riggable() {
        let d = std::env::temp_dir().join("flicker_content_scan_single");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        touch(&d, "Female_Character_output.fbx"); // the mesh to rig
        touch(&d, "Female_Animation_Walking.fbx"); // an animation
        touch(&d, "body_texture_0.png");
        touch(&d, "body_texture_0_normal.png");
        touch(&d, "manifest.json");

        let scan = scan_folder(&d).unwrap();
        assert_eq!(scan.riggable.len(), 1, "exactly one Character mesh to rig");
        assert_eq!(scan.sole_riggable().unwrap().kind, Kind::MeshFbx);
        assert!(!scan.needs_selection());
        assert_eq!(scan.of_kind(Kind::Texture).count(), 2);
        assert_eq!(scan.of_kind(Kind::AnimFbx).count(), 1);

        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn multiple_meshes_trip_the_selection_guard_across_subdirs() {
        let d = std::env::temp_dir().join("flicker_content_scan_multi");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("set")).unwrap();
        touch(&d, "Top_Duster.fbx");
        touch(&d, "Hem_Pants.fbx");
        touch(&d.join("set"), "Foot_Boots.fbx"); // nested → recursion must find it

        let scan = scan_folder(&d).unwrap();
        assert_eq!(
            scan.riggable.len(),
            3,
            "three garment meshes, incl. the nested one"
        );
        assert!(scan.needs_selection(), "editor must prompt which to rig");
        assert!(scan.sole_riggable().is_none());

        let _ = fs::remove_dir_all(&d);
    }

    /// The bone count is the deciding signal: the SAME folder reads as a character with a biped
    /// skeleton and as a prop without one, so Classify sharpens after Analyze instead of guessing.
    #[test]
    fn bone_count_decides_skin_versus_prop() {
        let d = std::env::temp_dir().join("flicker_content_classify_bones");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        touch(&d, "Character_output.fbx");
        touch(&d, "body_texture_0.png");
        touch(&d, "body_texture_0_normal.png");
        let scan = scan_folder(&d).unwrap();

        let skin = classify_asset(&scan, Some(66));
        assert_eq!(skin.class, AssetClass::Skin);
        assert!(
            skin.confidence > 0.5,
            "agreeing signals carry it, got {}",
            skin.confidence
        );
        assert!(
            skin.evidence.iter().any(|e| e.contains("66 bones")),
            "{:?}",
            skin.evidence
        );

        let prop = classify_asset(&scan, Some(0));
        assert_eq!(
            prop.class,
            AssetClass::Prop,
            "no skeleton → not a character"
        );

        // Unparsed: the folder shape alone must NOT read as confident.
        let unknown = classify_asset(&scan, None);
        assert!(
            unknown.confidence < skin.confidence,
            "unparsed is less certain than parsed"
        );

        let _ = fs::remove_dir_all(&d);
    }

    /// Motion-only folders are Animation, and a garment folder's NAME carries its sub-type.
    #[test]
    fn motion_only_reads_as_animation_and_names_give_prop_subtype() {
        let d = std::env::temp_dir().join("flicker_content_classify_anim");
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        touch(&d, "Animation_Walking.fbx");
        touch(&d, "run_forward.bvh");
        let scan = scan_folder(&d).unwrap();
        assert_eq!(classify_asset(&scan, None).class, AssetClass::Animation);
        let _ = fs::remove_dir_all(&d);

        let g = std::env::temp_dir().join("Corset-Duster");
        let _ = fs::remove_dir_all(&g);
        fs::create_dir_all(&g).unwrap();
        touch(&g, "Top_Duster.fbx");
        let scan = scan_folder(&g).unwrap();
        let r = classify_asset(&scan, Some(2));
        assert_eq!(r.class, AssetClass::Prop);
        assert_eq!(r.prop, PropKind::Clothing, "'duster' in the folder name");
        let _ = fs::remove_dir_all(&g);
    }

    /// Guarded real-data check: the actual `PrismHumanBaseA` source folder classifies to exactly one
    /// riggable mesh (`..._Character_output.fbx`), plus the walking animation + texture PNGs. Skips
    /// when the content tree isn't present.
    #[test]
    fn scans_the_real_prismhumanbasea_source() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/source/PrismHumanBaseA");
        if !dir.exists() {
            eprintln!("skipping: {} not present", dir.display());
            return;
        }
        let scan = scan_folder(&dir).unwrap();
        assert_eq!(
            scan.riggable.len(),
            1,
            "one Character mesh to rig, got {:?}",
            scan.candidates()
                .map(|e| e.path.file_name())
                .collect::<Vec<_>>()
        );
        assert!(
            scan.sole_riggable()
                .unwrap()
                .path
                .to_string_lossy()
                .contains("Character_output"),
            "the riggable candidate is the character mesh"
        );
        assert!(
            scan.of_kind(Kind::AnimFbx).count() >= 1,
            "the walking animation classifies as anim"
        );
        assert!(scan.of_kind(Kind::Texture).count() >= 1, "textures present");
    }

    // ── package-side classification ──

    #[test]
    fn names_alone_settle_the_unambiguous_classes() {
        // The `.gz` at-rest suffix must not hide the logical extension.
        assert_eq!(
            classify_package(Path::new("a/Foo_BaseColor.png")),
            PackageClass::Texture
        );
        assert_eq!(
            classify_package(Path::new("a/Cinzel.ttf")),
            PackageClass::Font
        );
        assert_eq!(
            classify_package(Path::new("a/intro.flight.gz")),
            PackageClass::Flight
        );
        assert_eq!(
            classify_package(Path::new("a/earthlike.epoch.gz")),
            PackageClass::Epoch
        );
        assert_eq!(
            classify_package(Path::new("a/K.pack.json.gz")),
            PackageClass::CombatPack
        );
        assert_eq!(
            classify_package(Path::new("a/B.rbp.json.gz")),
            PackageClass::RetargetBasePose
        );
        assert_eq!(classify_package(Path::new("a/OFL.txt")), PackageClass::Doc);
        assert_eq!(
            classify_package(Path::new("a/thing.bin")),
            PackageClass::Unknown
        );
    }

    /// The tree carries TWO serialization layouts and the sniff must survive
    /// both — in the alphabetical one, `format` sits ~5.9 MB into the file,
    /// so it is invisible to any prefix and `clips` must carry the decision.
    #[test]
    fn the_sniff_is_layout_agnostic() {
        let field_order_clip =
            br#"{"format": "flicker.rig", "mesh": {"indices": []}, "clips": [{"name":"walk"}]}"#;
        assert_eq!(classify_package_head(field_order_clip), PackageClass::Clip);

        let alphabetical_clip = br#"{"clips":[{"duration_ticks":599,"name":"army_crawl"}],"#;
        assert_eq!(
            classify_package_head(alphabetical_clip),
            PackageClass::Clip,
            "no `format` in the head at all — the populated clips array decides"
        );

        // A rig: empty clips, geometry present.
        let rig = br#"{"clips": [], "format": "flicker.rig", "mesh": {"indices": [0,1,2]}}"#;
        assert_eq!(classify_package_head(rig), PackageClass::Rig);

        // A rig whose mesh pushed `clips` out of the prefix entirely.
        let truncated_rig =
            br#"{"format": "flicker.rig", "skeleton": {"bones": [1,2,3]}, "mesh": {"pos"#;
        assert_eq!(classify_package_head(truncated_rig), PackageClass::Rig);
    }

    #[test]
    fn packs_rbps_and_bakes_are_read_off_their_shape() {
        assert_eq!(
            classify_package_head(br#"{"format": "flicker.pack", "state_machine": {}}"#),
            PackageClass::CombatPack
        );
        assert_eq!(
            classify_package_head(br#"{"format": "flicker.rbp", "pose": []}"#),
            PackageClass::RetargetBasePose
        );
        // A cluster bake declares no `format` at all.
        assert_eq!(
            classify_package_head(br#"{"version":2,"id":{"lod":0,"x":0,"y":0,"z":0}}"#),
            PackageClass::Bake
        );
        assert_eq!(
            classify_package_head(br#"{"something": 1}"#),
            PackageClass::Unknown
        );
    }

    /// An entangled Katanami export carries a mesh AND a clip; geometry wins,
    /// matching how the loader already picks rig authority.
    #[test]
    fn an_entangled_bundle_classifies_by_its_geometry() {
        let entangled = br#"{"format": "flicker.rig", "mesh": {"indices": [0,1,2]}, "clips": []}"#;
        assert_eq!(classify_package_head(entangled), PackageClass::Rig);
    }

    /// Guarded real-data check: every processed file in the tree classifies to
    /// something known — no `Unknown` may appear in shipped content. Skips when
    /// the package tree is absent.
    #[test]
    fn every_real_package_file_classifies() {
        let pkg = crate::roots::roots().package();
        if !pkg.is_dir() {
            eprintln!("skipping: {} not present", pkg.display());
            return;
        }
        let mut entries = Vec::new();
        collect(&pkg, &mut entries).expect("scanning the package tree");
        let unknown: Vec<_> = entries
            .iter()
            .map(|e| (&e.path, classify_package(&e.path)))
            .filter(|(_, c)| *c == PackageClass::Unknown)
            .collect();
        assert!(
            unknown.is_empty(),
            "unclassifiable package content: {unknown:?}"
        );

        let clips = entries
            .iter()
            .filter(|e| classify_package(&e.path) == PackageClass::Clip)
            .count();
        let rigs = entries
            .iter()
            .filter(|e| classify_package(&e.path) == PackageClass::Rig)
            .count();
        assert!(
            clips > 100,
            "the retargeted clip library should dominate, got {clips}"
        );
        assert!(rigs > 0, "at least one skinned rig, got {rigs}");
    }
}
