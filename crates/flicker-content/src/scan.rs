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
    .fold((AssetClass::Skin, f32::MIN), |best, c| if c.1 > best.1 { c } else { best });

    let total = skin + prop + animation;
    let confidence = if total > 0.0 { (score / total).clamp(0.0, 1.0) } else { 0.0 };
    AssetReport { class, prop: kind.unwrap_or(PropKind::Accessory), confidence, evidence }
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
    const ACCESSORY: &[&str] =
        &["ring", "amulet", "necklace", "belt", "pouch", "bag", "earring", "hair"];
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
        "animation", "walk", "run", "jog", "idle", "jump", "crouch", "attack", "death", "dame",
        "hit", "roll", "climb", "turn", "strafe", "dodge", "sprint", "crawl", "vault", "land",
        "stomp", "sit", "stand", "dance", "wave",
    ];
    HINTS.iter().any(|h| stem.contains(h))
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
    Ok(Scan { root: root.to_path_buf(), entries, riggable })
}

fn collect(dir: &Path, out: &mut Vec<Entry>) -> std::io::Result<()> {
    for de in std::fs::read_dir(dir)? {
        let path = de?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else if path.is_file() {
            out.push(Entry { kind: classify(&path), path });
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
        assert_eq!(classify(Path::new("Female_Human_Base_Character_output.fbx")), Kind::MeshFbx);
        assert_eq!(classify(Path::new("Female_Human_Base_Animation_Walking_frame_rate_60.fbx")), Kind::AnimFbx);
        assert_eq!(classify(Path::new("walk_forward.bvh")), Kind::Bvh);
        assert_eq!(classify(Path::new("body_texture_0_normal.png")), Kind::Texture);
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
        assert_eq!(scan.riggable.len(), 3, "three garment meshes, incl. the nested one");
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
        assert!(skin.confidence > 0.5, "agreeing signals carry it, got {}", skin.confidence);
        assert!(skin.evidence.iter().any(|e| e.contains("66 bones")), "{:?}", skin.evidence);

        let prop = classify_asset(&scan, Some(0));
        assert_eq!(prop.class, AssetClass::Prop, "no skeleton → not a character");

        // Unparsed: the folder shape alone must NOT read as confident.
        let unknown = classify_asset(&scan, None);
        assert!(unknown.confidence < skin.confidence, "unparsed is less certain than parsed");

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
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../Alpha/content/source/PrismHumanBaseA");
        if !dir.exists() {
            eprintln!("skipping: {} not present", dir.display());
            return;
        }
        let scan = scan_folder(&dir).unwrap();
        assert_eq!(
            scan.riggable.len(),
            1,
            "one Character mesh to rig, got {:?}",
            scan.candidates().map(|e| e.path.file_name()).collect::<Vec<_>>()
        );
        assert!(
            scan.sole_riggable().unwrap().path.to_string_lossy().contains("Character_output"),
            "the riggable candidate is the character mesh"
        );
        assert!(scan.of_kind(Kind::AnimFbx).count() >= 1, "the walking animation classifies as anim");
        assert!(scan.of_kind(Kind::Texture).count() >= 1, "textures present");
    }
}
