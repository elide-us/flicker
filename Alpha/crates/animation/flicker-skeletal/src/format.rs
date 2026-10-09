//! `flicker.rig` format contract — serde types + loader.
//!
//! The shared seam with the C++ `fbximport` converter (Part 1). The converter
//! emits data in **source space** (Z-up, centimetres, `applied_transform: "none"`);
//! this loader records that and the viewer normalises to the engine's Y-up/metre
//! space via a single world matrix ([`Model::world`]). Clip tracks are keyed by
//! bone **name**; the loader resolves each to a skeleton index (never assumes clip
//! bone order == skeleton order).
//!
//! Several contract fields (uv/weights/joints, texture list, inverse-bind) are
//! parsed now but only consumed by Slice 2 (CPU skinning) — hence the module-wide
//! dead-code allowance.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

// ─────────────────────────────── wire types (verbatim contract) ───────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct RigFile {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub skeleton: Skeleton,
    #[serde(default)]
    pub mesh: Mesh,
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// How this asset mounts onto a skeleton (folded-in `fits.json`). Populated for props /
    /// garments; empty for a character. serde-default keeps older files loading.
    #[serde(default)]
    pub attach: Attach,
    /// The CHARACTER's authored attach POINTS — where props mount onto THIS rig (grip /
    /// holster / scabbard / belt …), the Clayworks Attach stage's output. The inverse of
    /// [`RigFile::attach`]: that is how a piece hangs on a body, these are where a body
    /// offers to hang pieces. Empty for props / garments / clips; serde-default keeps
    /// older files loading.
    #[serde(default)]
    pub attach_points: Vec<AttachPoint>,
    /// Collision volumes (physics / hitbox / attach roles) carried by this asset. Schema-only
    /// today (the `mechanics` cluster consumes it later). serde-default keeps older files loading.
    #[serde(default)]
    pub collision: Collision,
    /// Play clips as ROTATION-ONLY on this rig: keep each bone's own rest translation
    /// (its bone offset/length) instead of the clip's baked source-skeleton offsets.
    /// Set for rigs RETARGETED from a differently-proportioned authoring skeleton (e.g.
    /// a Meshy body driven by the Katanami clip library). Default false = play the
    /// clip's full local TRS, i.e. the authoring character itself.
    #[serde(default)]
    pub retarget: bool,
    /// What this body's skeleton is MADE OF — the modular skeleton recipe it was composed from
    /// (2026-09-07), so packs, the retarget bake and every bench know the family. Absent on
    /// older rigs and on every rig conformed to the humanoid canon before recipes: read as
    /// [`SkeletonRecipe::humanoid`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skeleton_recipe: Option<SkeletonRecipe>,
}

// ───────────────────────── the skeleton RECIPE (the modular skeleton contract) ─────────────

/// SKELETON RECIPE — what a body is MADE OF (Aaron 2026-09-07, the modular skeleton system).
/// A tree of module instances: one root TRUNK carrying any number of shoulder pairs (arms),
/// hip pairs (legs), tails and MOUNTED child trunks, each child carrying the same. The
/// content crate's `baseline::compose` turns a recipe into the authored rest skeleton (names,
/// parents, positions) exactly the way it authors the humanoid canon; nothing bounds the tree
/// — seven chained trunks with fourteen arm pairs is a valid recipe.
///
/// NAMING: the FIRST instance of every module kind keeps the canon names (`pelvis`,
/// `clavicle_l`, `thigh_r` …), so the humanoid recipe reproduces the packaged `Humanoid` reference and biped
/// clips resolve by name on every biped recipe; every further instance takes a numbered
/// per-kind prefix (`trunk2_pelvis`, `arm14_hand_l`, `leg2_thigh_l`, `tail2_01`, `head2_jaw`),
/// counted in the recipe's depth-first order. The side suffix `_l`/`_r` is always last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkeletonRecipe {
    pub trunk: TrunkSpec,
    /// The shipped PRESET this recipe came from (`package/skeletons/<Pattern>/<Name>.recipe.json`),
    /// when it did — the picker's display name. Informational: what is load-bearing for CLIPS is
    /// the recipe's [`Pattern`] ([`SkeletonRecipe::pattern`]), which keys the reference rig and
    /// the `<library>@<Pattern>` bakes [`load_dirs`] picks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

impl SkeletonRecipe {
    /// The humanoid canon: one biped trunk, a head, one humanoid arm pair, one plantigrade
    /// leg pair, no tail — the packaged `Humanoid` reference (root + 66).
    pub fn humanoid() -> Self {
        SkeletonRecipe {
            trunk: TrunkSpec::default(),
            preset: None,
        }
    }

    /// The recipe's animation basis — see [`pattern_of`].
    pub fn pattern(&self) -> Pattern {
        pattern_of(self)
    }
}

/// A skeleton PATTERN — the animation BASIS a body animates on, and the ONE thing a clip library
/// is baked per (Aaron 2026-09-08: "the only difference between these skeletons is the legs").
/// It is the root trunk's LEG module: everything else a recipe says — head, arms, tails, mounts,
/// the heel's exact height — rides in the body's rig and needs no bake of its own (arm directions
/// are re-aimed at load, tails and heads carry no library motion, heel height is carried by the
/// rest-rebase). One reference rig per pattern lives at `package/skeletons/<Pattern>/<Pattern>.json`
/// with the recipes that compose to it beside it, and a library re-baked on that rest lives at
/// `package/retarget/clips/<library>@<Pattern>/`; the humanoid's libraries carry no suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pattern {
    /// Plantigrade legs — the humanoid canon and its variants (headless, tailed, hanging arms).
    Humanoid,
    /// Heel-raised legs on the humanoid's bones — cat and dog peoples, heeled shoes, the golem's heel.
    Digitigrade,
    /// The long-foot toe walker — lizardman, faun.
    ToeWalker,
    /// A hooved BIPED (a minotaur) — arrives with the unguligrade leg module (P3b).
    Unguligrade,
    /// A QUADRUPED TRUNK — the spine lies along the body, the four legs are the basis. No biped
    /// clip applies here; its gaits are their own content (P3). The orientation decides it
    /// whatever the leg modules are.
    Quadruped,
    /// A BIRD — bird wings on a biped trunk over bird legs (P4): flap, glide, perch, hop are its
    /// own basis. Decided by the first shoulder pair being bird wings.
    Bird,
    /// A BAT — membrane wings on a biped trunk (P4): a flier that hangs and crawls. Decided by the
    /// first shoulder pair being bat wings.
    Bat,
}

impl Pattern {
    pub const ALL: [Pattern; 7] = [
        Self::Humanoid,
        Self::Digitigrade,
        Self::ToeWalker,
        Self::Unguligrade,
        Self::Quadruped,
        Self::Bird,
        Self::Bat,
    ];

    /// The ONE spelling — the folder under `package/skeletons/`, the reference rig's name, and
    /// the `@<Pattern>` suffix of a library bake.
    pub fn name(self) -> &'static str {
        match self {
            Self::Humanoid => "Humanoid",
            Self::Digitigrade => "Digitigrade",
            Self::ToeWalker => "ToeWalker",
            Self::Unguligrade => "Unguligrade",
            Self::Quadruped => "Quadruped",
            Self::Bird => "Bird",
            Self::Bat => "Bat",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name))
    }

    /// The leg module a pattern's reference rig is composed with — the canonical knobs (the
    /// Digitigrade heel at 0.15 of stature; the toe walker measured off Aaron's LizardBaseA).
    pub fn leg(self) -> LegKind {
        match self {
            Self::Humanoid => LegKind::Plantigrade,
            Self::Digitigrade => LegKind::Digitigrade { heel: 0.15 },
            Self::ToeWalker => LegKind::ToeWalker {
                heel: 0.15,
                setback: 0.075,
                toe: 0.0,
                thigh: 0.19,
                calf: 0.2,
            },
            Self::Unguligrade | Self::Quadruped => LegKind::Unguligrade,
            Self::Bird => LegKind::Bird { heel: 0.25 },
            Self::Bat => LegKind::Plantigrade,
        }
    }

    /// The shoulder module a pattern's reference rig is composed with — the humanoid arm, the
    /// lizardman's hanging arm, the quadruped's foreleg, the bird's and the bat's wings.
    pub fn arm(self) -> ArmKind {
        match self {
            Self::Humanoid | Self::Digitigrade | Self::Unguligrade => ArmKind::Humanoid,
            Self::ToeWalker => ArmKind::Hanging,
            Self::Quadruped => ArmKind::Ungulate,
            Self::Bird => ArmKind::Bird,
            Self::Bat => ArmKind::Bat,
        }
    }
}

/// The pattern a rig on disk animates on: its embedded `skeleton_recipe`'s pattern, or the humanoid
/// basis for a rig that carries none (or cannot be read). What a stage or bench keys a body's
/// default controller pack and clip libraries on without parsing the whole rig into a document.
pub fn rig_pattern(path: &Path) -> Pattern {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        skeleton_recipe: Option<SkeletonRecipe>,
    }
    // Gz-transparent (flicker-core::compression): package content is gz at rest.
    flicker_core::compression::read_text(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Head>(&text).ok())
        .and_then(|head| head.skeleton_recipe.map(|r| r.pattern()))
        .unwrap_or(Pattern::Humanoid)
}

/// The pattern a recipe animates on: a QUADRUPED trunk is its own basis whatever its legs; a biped
/// follows its root trunk's FIRST leg module (a legless trunk, or one with no recipe at all, is a
/// humanoid basis).
pub fn pattern_of(recipe: &SkeletonRecipe) -> Pattern {
    if recipe.trunk.orientation == Orientation::Quadruped {
        return Pattern::Quadruped;
    }
    // A flier's basis is its wings, whatever it stands on.
    match recipe.trunk.arms.first() {
        Some(ArmKind::Bird) => return Pattern::Bird,
        Some(ArmKind::Bat) => return Pattern::Bat,
        _ => {}
    }
    match recipe.trunk.legs.first() {
        None | Some(LegKind::Plantigrade) => Pattern::Humanoid,
        Some(LegKind::Digitigrade { .. }) => Pattern::Digitigrade,
        Some(LegKind::ToeWalker { .. }) => Pattern::ToeWalker,
        Some(LegKind::Unguligrade) => Pattern::Unguligrade,
        Some(LegKind::Bird { .. }) => Pattern::Bird,
    }
}

/// One trunk (pelvis → spine → neck) and everything hung from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrunkSpec {
    pub orientation: Orientation,
    /// A head on the neck (off for the headless monsters).
    pub head: bool,
    /// A PROBOSCIS: a chain of this many articulated bones off the head's front — an elephant's
    /// trunk, the one appendage that is DRIVEN rather than hung (ruling 7881216F: *"trunk should
    /// curl and reach"*). 0 for none; composes nothing without a head.
    #[serde(default)]
    pub proboscis: u8,
    /// Shoulder pairs, in order: pair 1 at the canon shoulder, later pairs stepped back and up.
    pub arms: Vec<ArmKind>,
    /// Hip pairs, in order: pair 1 at the canon hips, later pairs stepped back along the trunk.
    pub legs: Vec<LegKind>,
    /// Tails off the pelvis rear, in order.
    pub tails: Vec<TailKind>,
    /// Child trunks and where on THIS trunk they mount.
    pub mounts: Vec<Mount>,
    /// This trunk's authored rest relative to the recipe's stature knob (a smaller front torso
    /// on a centaur is `< 1.0`). The root trunk of a humanoid is `1.0`.
    pub scale: f32,
    /// A QUADRUPED trunk's body length — pelvis to the shoulder socket — as a fraction of the
    /// stature knob (the spec's second knob, P3). A biped's spine is plumb and ignores it.
    #[serde(default = "TrunkSpec::default_length")]
    pub length: f32,
}

impl TrunkSpec {
    /// A horse's hip-to-shoulder is about two thirds of its stature knob.
    pub const DEFAULT_LENGTH: f32 = 0.65;

    fn default_length() -> f32 {
        Self::DEFAULT_LENGTH
    }
}

impl Default for TrunkSpec {
    fn default() -> Self {
        TrunkSpec {
            orientation: Orientation::Biped,
            head: true,
            proboscis: 0,
            arms: vec![ArmKind::Humanoid],
            legs: vec![LegKind::Plantigrade],
            tails: Vec::new(),
            mounts: Vec::new(),
            scale: 1.0,
            length: Self::DEFAULT_LENGTH,
        }
    }
}

/// How a trunk's spine lies: plumb (the biped) or along the body (the quadruped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Orientation {
    #[default]
    Biped,
    Quadruped,
}

/// The articulation of one shoulder pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArmKind {
    /// clavicle → upperarm (+twist) → lowerarm (+twist) → hand → five three-bone fingers + the
    /// Weapon grip socket — the canon arm.
    Humanoid,
    /// The canon arm chain HANGING at the sides (Aaron's lizardman, measured 2026-09-07): the
    /// shoulder out and a little back of the socket, the upper arm 20° out and 8° back, the
    /// forearm 17° out and 5° forward, the hand and fingers straight down — shorter than the
    /// canon (upper arm 0.161, forearm 0.124 of stature). Same bones as `Humanoid`.
    Hanging,
    /// The bat wing (P4): the arm chain to the hand, a clawed `wing_thumb`, then four elongated
    /// membrane digits `wing_digit_1..4_01..03` fanning from the wrist, the leading digit along
    /// the span and the trailing one back along the body.
    Bat,
    /// The bird wing (P4): clavicle → upperarm → lowerarm → hand → `wing_tip`, with two
    /// feather-group bones — `wing_feathers_01` (secondaries, off the forearm) and
    /// `wing_feathers_02` (primaries, off the hand) — for fold/spread. Authored SPREAD WIDE,
    /// the pose the Meshy birds ship in.
    Bird,
    /// The quadruped foreleg (P3b): scapula (`clavicle`) → shoulder (`upperarm`) → elbow
    /// (`lowerarm`) → carpus (`hand`) → fetlock (`foredigit`) → the ground contact (`forehoof`,
    /// the hoof or a paw's toe — the joint an IK foot-plant drives). No twists, no fingers.
    Ungulate,
}

/// The articulation of one hip pair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LegKind {
    /// thigh (+twist) → calf (+twist) → foot → ball, sole flat — the canon leg.
    Plantigrade,
    /// The same bones with the heel raised: `foot` pitched steeply, `ball` on the ground — cats,
    /// dogs, catman, werewolf, lizardman, heeled shoes. `heel` = the heel's height as a fraction
    /// of stature.
    Digitigrade { heel: f32 },
    /// A LONG-FOOT TOE WALKER (Aaron 2026-09-07: lizardman, faun, the cat peoples): the heel
    /// raised to `heel` of stature and set BACK `setback` behind the hip line, a shorter thigh
    /// and shin (`thigh`, `calf`, lengths as fractions of stature) meeting at a mildly bent
    /// knee, and a long metatarsus running down-forward to a ball at `toe` (forward is
    /// negative; 0 puts it under the hip). Measured off the hand-rigged LizardBaseA:
    /// heel 0.15, setback 0.075, toe 0.0, thigh 0.19, calf 0.20.
    ToeWalker {
        heel: f32,
        setback: f32,
        toe: f32,
        thigh: f32,
        calf: f32,
    },
    /// thigh → calf (stifle) → foot (hock) → ball (fetlock) → hoof on the ground (P3b): the
    /// hind leg of a hoofed quadruped, authored to the quadruped trunk's height. No twists.
    Unguligrade,
    /// A BIRD's leg (P4): a short thigh inside the body, the knee forward, the visible
    /// backward "knee" is the ankle (`foot`) raised to `heel` of stature, the tarsometatarsus
    /// down to the toe base (`ball`) on the ground, then three forward toes (`toe_01..03`) and
    /// the hind toe (`hallux`) — the joints a perch IK curls round a branch. No twists.
    Bird { heel: f32 },
}

/// A tail off the pelvis rear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TailKind {
    /// One bone.
    Short,
    /// One bone plus a chain of `bones` physics-driven hair bones.
    ShortHair { bones: u8 },
    /// `bones` articulated bones (lizard, gator, monkey).
    Long { bones: u8 },
}

/// A child trunk mounted on a socket of its parent trunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mount {
    pub socket: Socket,
    pub trunk: TrunkSpec,
}

/// Where on a trunk a child trunk mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Socket {
    /// The front-top of the trunk, above the first shoulder pair (a centaur's rider trunk).
    Withers,
    /// Behind the pelvis.
    Rear,
    /// On spine bone `1..=3`.
    Spine(u8),
    /// THE NECK ROOT of a HEADLESS trunk (Aaron's ruling, 2026-09-12): the parent composes its
    /// chain only to `spine_03` — NO `neck_01` / `neck_02` — and the mounted trunk's own `pelvis`
    /// stands where `neck_01` would have, parented to `spine_03`. That pelvis IS the SUB-PELVIS
    /// the upper torso bends on, exactly as the quadruped pelvis bends for the four-legged body:
    /// the centaur whose human torso rises where the animal's neck would be. A `Neck` mount on a
    /// trunk with `head: true`, or a second one on the same trunk, fails to compose (use
    /// [`Socket::Withers`] for a rider ON the back, which keeps the animal's neck and head).
    Neck,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub fbx_version: String,
    #[serde(default)]
    pub source_axis: String,
    #[serde(default)]
    pub source_unit: String,
    #[serde(default)]
    pub applied_transform: String,
    #[serde(default)]
    pub textures: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Skeleton {
    #[serde(default)]
    pub bones: Vec<BoneRaw>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoneRaw {
    pub name: String,
    pub parent: i32,
    pub local: [f32; 16],
    #[serde(default = "identity16")]
    pub inverse_bind: [f32; 16],
}

fn identity16() -> [f32; 16] {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn quat_identity() -> [f32; 4] {
    [0.0, 0.0, 0.0, 1.0]
}

fn one3() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}

fn one_f32() -> f32 {
    1.0
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Mesh {
    #[serde(default)]
    pub vertices: Vec<Vertex>,
    #[serde(default)]
    pub indices: Vec<u32>,
    /// Index ranges grouped by material (one draw per submesh). Empty for older
    /// rig JSON → the whole mesh is treated as a single untextured submesh.
    #[serde(default)]
    pub submeshes: Vec<Submesh>,
    #[serde(default)]
    pub materials: Vec<Material>,
    /// Secondary-motion cloth: which verts swing on which jiggle chains
    /// (`tools/skin_outfit.py --build-cloth`). Empty/absent → the mesh is fully rigid.
    #[serde(default)]
    pub cloth: Cloth,
    /// Facial identity morph targets — the character-creator "create-a-face" system.
    /// Sparse per-vertex position deltas from the bind face, blended BEFORE skinning
    /// (`skin::skin_morphed`). Empty/absent → no morphs (serde-default keeps old files loading).
    #[serde(default)]
    pub morphs: Vec<Morph>,
}

/// A contiguous run of `indices` sharing one material. Because the converter emits
/// a non-deduplicated vertex list with sequential indices, `[start, start+count)`
/// is equally a range into `indices` and into `vertices`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submesh {
    pub material: usize,
    pub start: usize,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Material {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slot: String,
    /// Albedo PNG basename, or empty → render this submesh flat. sRGB colour data.
    #[serde(default)]
    pub base_color: String,
    /// Tangent-space normal-map PNG basename, or empty. LINEAR data.
    #[serde(default)]
    pub normal: String,
    /// Roughness PNG basename, or empty. LINEAR data (R channel).
    #[serde(default)]
    pub roughness: String,
    /// Metalness PNG basename, or empty. LINEAR data (R channel).
    #[serde(default)]
    pub metalness: String,
    /// Ambient-occlusion PNG basename, or empty. LINEAR data (R channel).
    #[serde(default)]
    pub ao: String,
    /// Emissive PNG basename, or empty → non-emissive. sRGB colour data (self-illumination).
    /// The content standard's `Emit` map (`Alpha/content/README.md`).
    #[serde(default)]
    pub emit: String,
    /// Packed occlusion-roughness-metalness PNG basename (R=occlusion, G=roughness, B=metalness),
    /// or empty. LINEAR data. The content standard's canonical `ORM` map; when present a renderer
    /// prefers it over the separate `ao`/`roughness`/`metalness` basenames above.
    #[serde(default)]
    pub orm: String,
    /// Flat RGB (0..1) used when `base_color` is empty (untextured props). Empty →
    /// neutral gray.
    #[serde(default)]
    pub color: Vec<f32>,
}

/// Secondary-motion cloth data for a garment: which vertices swing on which jiggle
/// chains. Emitted by `tools/skin_outfit.py --build-cloth`, consumed by [`crate::cloth`].
/// All positions are in BIND (source/rig) space, the same space as the mesh vertices.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Cloth {
    #[serde(default)]
    pub regions: Vec<ClothRegion>,
}

/// What a region IS — the semantics the tagger sets and the bake reads (spec 0A81088E), and
/// since ruling 82EDC071 THE SOLVER IT RUNS ON: a `Cloth` region is a SHEET over its own
/// triangles (`cloth::Sheet`; its `chains`/`binds` are the comb a strand reader would lay and
/// the sheet reads only the vertices they name), every other tag a comb of jiggle chains.
/// `Cloth` is the default, so a region from an older rig keeps exactly the meaning it had.
/// UNTAGGED vertices are the body: there is deliberately no `Body` variant.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegionTag {
    #[default]
    Cloth,
    Hair,
    Mane,
    Tail,
    Pendant,
    /// A part of the body's own flesh with no bone of its own — a horn, an antler, an ear, a
    /// tusk — as the skin bind found it (ruling 7881216F): rigid with the bone it grows from
    /// at `chain_count` 0, swung on its chains otherwise.
    Appendage,
}

/// A region with no `chain_count` in its JSON predates the field and carries the single hang
/// the old emitter always laid.
fn one() -> u32 {
    1
}

/// One dangly region (a bell sleeve, a skirt hem …) — a fan of chains hung from one bone,
/// plus the region's vertices bound along those chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClothRegion {
    pub name: String,
    /// Body bone the chains hang from (drives their anchor point + home direction).
    pub anchor_bone: String,
    /// Semantics — tag only. The physics knobs are `chain_count` + `params`.
    #[serde(default)]
    pub tag: RegionTag,
    /// AUTHORED membership: the vertices that ARE this region (the tagger's selection, or the
    /// auto-split's component). `binds` is the DERIVED chain attachment rebuilt at bake — two
    /// concepts, two fields.
    #[serde(default)]
    pub verts: Vec<u32>,
    /// How many chains the comb lays across the region's attachment edge. 0 = no chains: the
    /// region skins RIGIDLY to `anchor_bone` (a pendant's mount, a stiff panel).
    #[serde(default = "one")]
    pub chain_count: u32,
    #[serde(default)]
    pub params: ClothParams,
    #[serde(default)]
    pub chains: Vec<ClothChain>,
    #[serde(default)]
    pub binds: Vec<ClothBind>,
}

/// A single jiggle chain: a straight hang from `anchor` along `dir`, `segments` links of
/// `seg_len` each. Same construction args as [`crate::jiggle::JiggleChain::new`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClothChain {
    pub anchor: [f32; 3],
    pub dir: [f32; 3],
    pub seg_len: f32,
    pub segments: u32,
}

/// A vertex's attachment: which region chain (`c`) it follows and where along it —
/// segment `k`, fraction `f` in `0..1` along that segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClothBind {
    pub v: u32,
    pub c: u32,
    pub k: u32,
    pub f: f32,
}

/// Per-region jiggle dials (mirrors [`crate::jiggle::JiggleParams`], as plain arrays for
/// the wire). Defaults suit a light garment in cm / z-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClothParams {
    pub gravity: [f32; 3],
    pub stiffness: f32,
    pub damping: f32,
    pub iterations: u32,
    pub max_dt: f32,
}

impl Default for ClothParams {
    fn default() -> Self {
        Self {
            gravity: [0.0, 0.0, -600.0],
            stiffness: 0.015,
            damping: 0.9,
            iterations: 8,
            max_dt: 1.0 / 30.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vertex {
    pub p: [f32; 3],
    pub n: [f32; 3],
    #[serde(default)]
    pub uv: [f32; 2],
    pub joints: [u32; 4],
    pub weights: [f32; 4],
}

/// A facial identity morph target: a named, sparse set of per-vertex position deltas from
/// the bind face (only affected verts are listed). Blended by a player-driven weight in the
/// create-a-face UI; static per character at runtime. DNA-forward — a future DNA/RigLogic
/// import maps its identity morphs onto ours by `name`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Morph {
    pub name: String,
    #[serde(default)]
    pub deltas: Vec<MorphDelta>,
}

/// One vertex's contribution to a `Morph`: index into `Mesh::vertices` (`v`) plus the position
/// delta (`d`), added to the bind position scaled by the morph's blend weight.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MorphDelta {
    pub v: u32,
    pub d: [f32; 3],
}

/// How a prop / garment mounts onto a skeleton — the self-describing fold of the `fits.json`
/// sidecar (one recorded placement per asset). A rigid prop (weapon, sheath) carries this so the
/// import editor can snap it to a socket; a fitted garment carries its placement the same way.
///
/// `socket` names the bone/slot it hangs from (accepts the `slot` key from legacy `fits.json`);
/// `offset` is in WORLD axes at rest (x lateral, y depth, z up — NOT bone axes); `rotate` is a
/// quaternion `[x,y,z,w]`; `scale` × `uniform` size the RAW (unscaled) vendor mesh. The socket's
/// upright correction is re-derived from the rig at load, never stored here. serde-default: an
/// absent section means the asset has no recorded fit (the import editor infers a default).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attach {
    #[serde(default, alias = "slot")]
    pub socket: String,
    #[serde(default)]
    pub offset: [f32; 3],
    #[serde(default = "quat_identity")]
    pub rotate: [f32; 4],
    #[serde(default = "one3")]
    pub scale: [f32; 3],
    #[serde(default = "one_f32")]
    pub uniform: f32,
}

/// One authored attach POINT on a character rig — see [`RigFile::attach_points`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttachPoint {
    /// The point's stable id (`hand_r`, `holster_l`, `belt`, …) — what gameplay binds to.
    #[serde(default)]
    pub id: String,
    /// The CANONICAL bone the point rides.
    #[serde(default)]
    pub bone: String,
    /// Authored offset from that bone's frame, in model units.
    #[serde(default)]
    pub offset: [f32; 3],
}

// Hand-written so an ABSENT `attach` block (RigFile's `#[serde(default)]`) yields the SAME
// sensible values as a present-but-partial one — identity rotation, unit scale — instead of the
// derived all-zeros (which would give a zero quaternion and zero scale). Mirrors `ClothParams`.
impl Default for Attach {
    fn default() -> Self {
        Self {
            socket: String::new(),
            offset: [0.0, 0.0, 0.0],
            rotate: quat_identity(),
            scale: one3(),
            uniform: one_f32(),
        }
    }
}

/// Collision volumes carried by an asset — the golden-spec three-role model. SCHEMA ONLY here:
/// the primitive geometry + overlap-query runtime + capsule-authoring live in the `mechanics`
/// cluster (a later slice). serde-default lets assets start carrying volumes before the runtime
/// consumes them, and keeps every existing file (which has none) loading.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Collision {
    #[serde(default)]
    pub volumes: Vec<CollisionVolume>,
}

/// One collision primitive, parented to a bone (so it follows the pose). `bone` resolves by NAME
/// like every clip track (share-by-name); the `shape` carries the primitive + its dimensions in
/// the bone's local frame (Z-up / cm); `role` tags what the volume is for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollisionVolume {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bone: String,
    pub shape: CollisionShape,
    #[serde(default)]
    pub role: CollisionRole,
}

/// The primitive geometry of a [`CollisionVolume`], internally tagged by `kind`
/// (`"sphere"` / `"capsule"` / `"box"`). Capsule = segment `a`..`b` + `radius`; box = oriented
/// half-extents about `center`; sphere = `center` + `radius`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollisionShape {
    Sphere {
        center: [f32; 3],
        radius: f32,
    },
    Capsule {
        a: [f32; 3],
        b: [f32; 3],
        radius: f32,
    },
    Box {
        center: [f32; 3],
        half_extents: [f32; 3],
        #[serde(default = "quat_identity")]
        rotation: [f32; 4],
    },
}

/// What a [`CollisionVolume`] is for (the golden-spec three roles). serde-default = `Physics`
/// (the persistent occupy-the-world / damageable-hurtbox volume).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionRole {
    /// Persistent, always-on: item-vs-world occupancy and the damageable hurtbox.
    #[default]
    Physics,
    /// Transient combat box, switched on/off by a TAE `HitboxActive` tick-window.
    Hitbox,
    /// An attachment point where a prop/weapon mounts (a socket, possibly a bone).
    Attach,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    #[serde(default = "default_tick_rate")]
    pub tick_rate_hz: u32,
    #[serde(default)]
    pub duration_ticks: u32,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

fn default_tick_rate() -> u32 {
    60
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Track {
    pub bone: String,
    pub keys: Vec<Keyframe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyframe {
    #[serde(default)]
    pub t: u32,
    #[serde(rename = "T")]
    pub translation: [f32; 3],
    #[serde(rename = "R")]
    pub rotation: [f32; 4],
    #[serde(rename = "S")]
    pub scale: [f32; 3],
}

// ─────────────────────────────── engine form ──────────────────────────────────

/// A skeleton bone in engine form (matrices decoded from row-major to glam).
pub struct Bone {
    pub name: String,
    pub parent: i32,
    /// Rest local transform (used when a clip has no track for this bone).
    pub local: Mat4,
    /// Bind-offset matrix (skinning; Slice 2).
    pub inverse_bind: Mat4,
}

/// A clip track resolved to a skeleton bone index.
pub struct ResolvedTrack {
    pub bone: usize,
    pub keys: Vec<Keyframe>,
    /// The SOURCE skeleton's rest translation for this bone (from the clip file's own
    /// skeleton — the space the clip was authored in). Retarget playback keeps
    /// `target_rest + (clip_T - source_rest)`: zero for constant-offset bones (limbs, where
    /// `clip_T == source_rest`, so the rig's own proportions are preserved), but the real
    /// animated delta for a bone that translates — the pelvis's hip sway/bob — rebased onto
    /// this rig's own hip height. Falls back to the target bone's rest translation (delta 0)
    /// when the source skeleton lacks the bone.
    pub source_rest: [f32; 3],
}

/// An animation clip with its tracks resolved against the rig skeleton.
pub struct ResolvedClip {
    pub name: String,
    pub tick_rate_hz: u32,
    pub duration_ticks: u32,
    pub tracks: Vec<ResolvedTrack>,
    /// Track bone names that did NOT match any skeleton bone (validation signal).
    pub unresolved: Vec<String>,
}

/// The assembled, ready-to-play model: rig skeleton + mesh + resolved clips, plus
/// the source→engine-space `world` matrix and an orbit-framing radius.
pub struct Model {
    pub bones: Vec<Bone>,
    pub clips: Vec<ResolvedClip>,
    pub mesh: Mesh,
    pub source: Source,
    /// Source space (Z-up/cm) → engine space (Y-up/m), centred on the origin so the
    /// orbit camera (which looks at ZERO) frames the model.
    pub world: Mat4,
    /// Bounding radius of the rest pose in engine space — camera framing.
    pub orbit_radius: f32,
    /// Rotation-only clip playback for a retargeted rig (see [`RigFile::retarget`]).
    pub retarget: bool,
    /// The rig file's mount record (folded-in `fits.json`); default for a character with none.
    pub attach: Attach,
    /// The rig file's collision volumes (schema-only until the `mechanics` runtime lands).
    pub collision: Collision,
}

/// Decode a contract matrix (16 floats) into a glam `Mat4`.
///
/// The converter emits FBX-native matrices: row-major storage in **row-vector**
/// convention (a point transforms as `p * M`, so translation lives in the LAST ROW,
/// `m[12..15]`). glam is column-vector (`M * p`, translation in the last column), and
/// the column-vector form is the transpose — which is exactly what `from_cols_array`
/// yields when it reads these row-major floats as columns. So do NOT add
/// `.transpose()` here: that would move translation to the wrong place and the
/// bind/inverse-bind matrices would explode skinning. (The clip `T/R/S` keys are
/// decomposed values, not matrices, so they're unaffected.) See
/// docs/flicker-animation-handoff.md.
fn mat4_from_contract(m: &[f32; 16]) -> Mat4 {
    Mat4::from_cols_array(m)
}

/// Recursively collect every `*.json` path under `dir` — loose or in its
/// gz-at-rest form (`*.json.gz`, how package content ships). Enumerates
/// through the seam's [`flicker_core::compression::list_dir`], so a mounted
/// `package.flk` serves the same listing an on-disk tree would.
fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in flicker_core::compression::list_dir(dir)
        .with_context(|| format!("reading assets dir {}", dir.display()))?
    {
        if entry.is_dir {
            collect_json_files(&entry.path, out)?;
        } else if is_json_asset(&entry.path) {
            out.push(entry.path);
        }
    }
    Ok(())
}

/// A rig/clip asset file: `*.json`, loose or gz-at-rest (`*.json.gz`).
fn is_json_asset(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".json") || n.ends_with(".json.gz"))
}

/// Whether any component of `path` equals `name` (e.g. `clips`, `RootMotion`).
fn path_has_component(path: &Path, name: &str) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_str() == Some(name))
}

/// Recursively load every `*.json` under `dir`, pick the rig (the file carrying the
/// mesh), and resolve all clip tracks against its skeleton.
///
/// Clips are taken from the structured library under `dir/clips/` (mirroring the
/// converter's `In-Place/` + `RootMotion/` taxonomy). When that tree is present, flat
/// top-level clip files are treated as legacy duplicates and skipped. **RootMotion
/// clips are namespaced `RM/<stem>`** so they don't collide with the In-Place clip of
/// the same stem (`Run_nonWeapon`/`Walk_nonWeapon`/`Run_Weapon` exist in both trees);
/// In-Place clips keep their bare stem so a pack references the default (in-place)
/// locomotion by plain name. Falls back to loading top-level clip files when no
/// `clips/` subtree exists (a legacy flat layout).
pub fn load_dir(dir: &Path) -> Result<Model> {
    load_dirs(&[dir])
}

/// One rig file's embedded skeleton decoded to the runtime [`Bone`] set (contract-space
/// matrices converted). The shared seam between [`load_dirs`] and in-memory consumers —
/// the Clayworks clip preview builds a playable rig straight from the retargeter's
/// output value, no disk round-trip.
pub fn rig_bones(file: &RigFile) -> Vec<Bone> {
    file.skeleton
        .bones
        .iter()
        .map(|b| Bone {
            name: b.name.clone(),
            parent: b.parent,
            local: mat4_from_contract(&b.local),
            inverse_bind: mat4_from_contract(&b.inverse_bind),
        })
        .collect()
}

/// Resolve one file's clips against `bones`: each track's bone NAME becomes a skeleton
/// index, carrying the SOURCE skeleton's rest translation (the space the clip's
/// translations were authored in) for the retarget rebase — see
/// [`ResolvedTrack::source_rest`]. `rm_namespace` prefixes clip names `RM/…`, the
/// structured clip library's RootMotion convention; an in-memory consumer whose
/// variant identity lives elsewhere passes `false`.
/// The bone a LIMB bone's rest DIRECTION runs to — the pairs the retarget transfers by direction
/// (thigh→calf, calf→foot, upper arm→forearm, forearm→hand, hand→middle finger, the finger
/// phalanges), and the pairs the load-time limb rebase re-aims. The foot chain is deliberately
/// absent: feet transfer by DELTA and keep the body's own stance (a lizardman's raised heel).
pub fn limb_direction_child(name: &str) -> Option<String> {
    let (base, side) = name.rsplit_once('_')?;
    if side != "l" && side != "r" {
        return None;
    }
    let child = match base {
        "thigh" => "calf".to_string(),
        "calf" => "foot".to_string(),
        "upperarm" => "lowerarm".to_string(),
        "lowerarm" => "hand".to_string(),
        "hand" => "middle_01".to_string(),
        _ => {
            let (finger, seg) = base.rsplit_once('_')?;
            if !matches!(finger, "thumb" | "index" | "middle" | "ring" | "pinky") {
                return None;
            }
            match seg {
                "01" => format!("{finger}_02"),
                "02" => format!("{finger}_03"),
                _ => return None,
            }
        }
    };
    Some(format!("{child}_{side}"))
}

/// THE LOAD-TIME LIMB REBASE (Aaron 2026-09-07: "Is there a root cause for this that we can
/// solve since it seems to happen to every model?"). A clip's limb rotations turn the LIBRARY
/// skeleton's rest bone direction to the source's, so on a body whose limb rests at another
/// angle — every Meshy A-pose is its own — the same rotation lands the limb off by that
/// difference: the arms crossed behind the back on every body. Per body bone: ONE fixed
/// rotation `q` from the body's rest bone direction to the library's (identity where either
/// skeleton lacks the pair, or the bone is not a limb), and every key is conjugated
/// `inv(q_parent) · r · q`, so the body's limbs point where the library's do under every clip,
/// whatever the fit, with the body's own lengths (the translation rebase is unchanged). Torso,
/// head and feet keep `q = identity` and their delta semantics — a bone whose parent is a limb
/// still takes `inv(q_parent)`, so its world rotation is the library's exactly.
fn limb_rebase(file: &RigFile, bones: &[Bone]) -> Vec<Quat> {
    let rest_positions = |bones: &[Bone]| -> HashMap<String, Vec3> {
        let locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
        crate::pose::global_transforms(bones, &locals)
            .into_iter()
            .zip(bones)
            .map(|(g, b)| (b.name.clone(), g.w_axis.truncate()))
            .collect()
    };
    let body = rest_positions(bones);
    let library = rest_positions(&rig_bones(file));
    bones
        .iter()
        .map(|b| {
            let Some(child) = limb_direction_child(&b.name) else {
                return Quat::IDENTITY;
            };
            let (Some(&b0), Some(&b1), Some(&l0), Some(&l1)) = (
                body.get(&b.name),
                body.get(&child),
                library.get(&b.name),
                library.get(&child),
            ) else {
                return Quat::IDENTITY;
            };
            match ((b1 - b0).try_normalize(), (l1 - l0).try_normalize()) {
                (Some(d_body), Some(d_lib)) => Quat::from_rotation_arc(d_body, d_lib),
                _ => Quat::IDENTITY,
            }
        })
        .collect()
}

pub fn resolve_clips(file: &RigFile, bones: &[Bone], rm_namespace: bool) -> Vec<ResolvedClip> {
    let name_to_index: HashMap<&str, usize> = bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let q = limb_rebase(file, bones);
    let src_rest: HashMap<&str, [f32; 3]> = file
        .skeleton
        .bones
        .iter()
        .map(|b| (b.name.as_str(), [b.local[12], b.local[13], b.local[14]]))
        .collect();
    let mut out = Vec::new();
    for clip in &file.clips {
        let mut tracks = Vec::new();
        let mut unresolved = Vec::new();
        for tr in &clip.tracks {
            match name_to_index.get(tr.bone.as_str()) {
                Some(&bi) => {
                    // Fall back to the target bone's own rest translation → delta 0.
                    let w = bones[bi].local.w_axis;
                    let source_rest = src_rest
                        .get(tr.bone.as_str())
                        .copied()
                        .unwrap_or([w.x, w.y, w.z]);
                    let q_parent_inv = usize::try_from(bones[bi].parent)
                        .ok()
                        .and_then(|p| q.get(p))
                        .map_or(Quat::IDENTITY, |qp| qp.inverse());
                    let q_bone = q[bi];
                    let keys = tr
                        .keys
                        .iter()
                        .map(|k| {
                            let r = Quat::from_array(k.rotation);
                            Keyframe {
                                rotation: (q_parent_inv * r * q_bone).normalize().to_array(),
                                ..k.clone()
                            }
                        })
                        .collect();
                    tracks.push(ResolvedTrack {
                        bone: bi,
                        keys,
                        source_rest,
                    });
                }
                None => unresolved.push(tr.bone.clone()),
            }
        }
        let name = if rm_namespace {
            format!("RM/{}", clip.name)
        } else {
            clip.name.clone()
        };
        out.push(ResolvedClip {
            name,
            tick_rate_hz: clip.tick_rate_hz,
            duration_ticks: clip.duration_ticks,
            tracks,
            unresolved,
        });
    }
    out
}

/// Like [`load_dir`] but gathers rig/clip JSON from SEVERAL directories — for a base
/// body that borrows another character's clip library (e.g. `base_human_female` playing
/// the Katanami animation set, which resolves by shared bone names). The rig authority is
/// still the single file with the most mesh vertices across all the dirs, so the base
/// body (dense mesh) wins over the clip files (skeleton-only).
pub fn load_dirs(dirs: &[&Path]) -> Result<Model> {
    let read = |path: &Path| -> Result<RigFile> {
        // Gz-transparent read (flicker-core::compression) — package content is
        // gz at rest; loose dev files read the same way.
        let text = flicker_core::compression::read_text(path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    };
    let mut per_dir: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect_json_files(dir, &mut files)?;
        per_dir.push((dir.to_path_buf(), files));
    }
    // The BODY's own rig — the promoted convention `<Name>/<Name>.json` — parses FIRST, once:
    // its skeleton recipe's PATTERN (the leg module) is the basis its clips were baked for, and
    // a clip LIBRARY passed for a non-humanoid body loads the sibling bake `<library>@<Pattern>`
    // (the same clips re-baked onto that pattern's rest) when one exists. A library without
    // that bake loads as passed, and says so. A humanoid body takes the library as passed.
    let mut parsed: Vec<(PathBuf, RigFile)> = Vec::new();
    let mut preset: Option<String> = None;
    for (dir, files) in &per_dir {
        let stem = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let own = files.iter().find(|f| {
            f.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == format!("{stem}.json") || n == format!("{stem}.json.gz"))
        });
        if let Some(path) = own {
            let file = read(path)?;
            if let Some(p) = file
                .skeleton_recipe
                .as_ref()
                .map(SkeletonRecipe::pattern)
                .filter(|p| *p != Pattern::Humanoid)
            {
                preset = Some(p.name().to_string());
            }
            parsed.push((path.clone(), file));
        }
    }
    if let Some(p) = &preset {
        for (dir, files) in per_dir.iter_mut() {
            let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.contains('@') || !files.iter().any(|f| path_has_component(f, "clips")) {
                continue; // a body folder, or already a preset bake
            }
            let sibling = dir.with_file_name(format!("{name}@{p}"));
            if sibling.is_dir() {
                let mut swapped = Vec::new();
                collect_json_files(&sibling, &mut swapped)?;
                eprintln!(
                    "flicker-skeletal: clips: pattern `{p}` bake {}",
                    sibling.display()
                );
                *dir = sibling;
                *files = swapped;
            } else {
                eprintln!(
                    "flicker-skeletal: clips: no `{p}` bake beside {} — loading the humanoid bake",
                    dir.display()
                );
            }
        }
    }
    for (_, files) in &per_dir {
        for path in files {
            if parsed.iter().any(|(p, _)| p == path) {
                continue;
            }
            parsed.push((path.clone(), read(path)?));
        }
    }
    if parsed.is_empty() {
        anyhow::bail!(
            "no .json rig/clip assets found in {:?} — run the exporter and copy its output here",
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
        );
    }

    // Rig authority = the file with the most mesh vertices (the mesh FBX); tie-break
    // by bone count. Clip files carry a redundant skeleton copy which we ignore.
    let rig_idx = parsed
        .iter()
        .enumerate()
        .max_by_key(|(_, (_, f))| (f.mesh.vertices.len(), f.skeleton.bones.len()))
        .map(|(i, _)| i)
        .expect("parsed is non-empty");
    let (_rig_name, rig_file) = parsed.remove(rig_idx);

    let bones = rig_bones(&rig_file);
    if bones.is_empty() {
        anyhow::bail!("rig skeleton has no bones");
    }

    // Clips come from the structured `clips/` library; resolve each track's bone NAME
    // to a skeleton index against the rig. When that tree exists, skip the flat
    // top-level files (legacy duplicates). RootMotion clips are namespaced `RM/…`.
    let has_clip_tree = parsed.iter().any(|(p, _)| path_has_component(p, "clips"));
    let mut clips: Vec<ResolvedClip> = Vec::new();
    for (path, f) in &parsed {
        if has_clip_tree && !path_has_component(path, "clips") {
            continue;
        }
        let root_motion = path_has_component(path, "RootMotion");
        clips.extend(resolve_clips(f, &bones, root_motion));
    }
    // Stable, predictable clip order for the cycle control.
    clips.sort_by(|a, b| a.name.cmp(&b.name));

    // Framing: transform the rest-pose joint positions into engine space, fit an
    // orbit sphere, and centre the model on the origin.
    let scale_factor = if rig_file.source.source_unit.eq_ignore_ascii_case("cm") {
        0.01
    } else {
        1.0
    };
    let rot = if rig_file.source.source_axis.eq_ignore_ascii_case("Z_up") {
        Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
    } else {
        Mat4::IDENTITY
    };
    let orient = rot * Mat4::from_scale(Vec3::splat(scale_factor));

    let rest_locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
    let rest_globals = crate::pose::global_transforms(&bones, &rest_locals);
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for g in &rest_globals {
        let p = orient.transform_point3(g.w_axis.truncate());
        min = min.min(p);
        max = max.max(p);
    }
    let center = (min + max) * 0.5;
    let radius = ((max - min).length() * 0.5).max(0.25);
    let world = Mat4::from_translation(-center) * orient;

    Ok(Model {
        bones,
        clips,
        mesh: rig_file.mesh,
        source: rig_file.source,
        world,
        orbit_radius: radius,
        retarget: rig_file.retarget,
        attach: rig_file.attach,
        collision: rig_file.collision,
    })
}

/// Load a single mesh JSON as a static prop — geometry only (submeshes + materials).
/// Bones/clips are ignored; the prop is rendered rigid at an attach transform. In the
/// same source space (Z-up/cm) as the rig, so the rig's `world` matrix maps it too.
pub fn load_mesh(path: &Path) -> Result<Mesh> {
    Ok(load_mesh_with_attach(path)?.0)
}

/// Like [`load_mesh`] but also returns the prop's folded-in `attach` mount record (default when
/// the file carries none). The fit editor uses this to place a prop at its recorded socket/offset
/// straight from the one self-describing file — no `fits.json` sidecar — without re-parsing it.
pub fn load_mesh_with_attach(path: &Path) -> Result<(Mesh, Attach)> {
    let text = flicker_core::compression::read_text(path)
        .with_context(|| format!("reading prop {}", path.display()))?;
    let file: RigFile =
        serde_json::from_str(&text).with_context(|| format!("parsing prop {}", path.display()))?;
    Ok((file.mesh, file.attach))
}

/// Remap a mesh's joint indices from an outfit's OWN (reduced) bone list into a base
/// skeleton's index space, matching by bone NAME. This is what lets a partial-bone
/// outfit — exported with only the bones it weights + their ancestor chain — be skinned
/// directly with the base skeleton's pose palette. Every joint index (including the
/// 0-padded, zero-weight slots) is rewritten; an influence whose bone name is absent
/// from the base collapses to the root (index 0) with a warning.
fn remap_outfit_joints(mesh: &mut Mesh, outfit_names: &[String], base: &[Bone]) {
    let base_index: HashMap<&str, usize> = base
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let remap: Vec<u32> = outfit_names
        .iter()
        .map(|n| match base_index.get(n.as_str()) {
            Some(&i) => i as u32,
            None => {
                eprintln!("flicker-skeletal: outfit bone '{n}' not in base skeleton; influence pinned to root");
                0
            }
        })
        .collect();
    let nb = remap.len() as u32;
    for v in &mut mesh.vertices {
        for k in 0..4 {
            let j = v.joints[k];
            v.joints[k] = if j < nb { remap[j as usize] } else { 0 };
        }
    }
}

/// Load an OUTFIT rig file: a partial skinned mesh that shares another (base) skeleton
/// and is drawn over a base body. Its `skeleton.bones` is a REDUCED list and its vertex
/// `joints` index into THAT list; this remaps them into `base`'s index space by bone
/// NAME so the returned mesh skins directly with the base pose palette. A file with no
/// skeleton block is returned unchanged (legacy: joints already index the base — e.g.
/// an outfit exported against the full skeleton, where the remap is the identity anyway).
pub fn load_outfit(path: &Path, base: &[Bone]) -> Result<Mesh> {
    Ok(load_outfit_with_attach(path, base)?.0)
}

/// Like [`load_outfit`] but also returns the garment's folded-in `attach` mount record (default
/// when the file carries none) — the fit editor reads a piece's placement from its one
/// self-describing file instead of the shared `fits.json` sidecar.
pub fn load_outfit_with_attach(path: &Path, base: &[Bone]) -> Result<(Mesh, Attach)> {
    let text = flicker_core::compression::read_text(path)
        .with_context(|| format!("reading outfit {}", path.display()))?;
    let file: RigFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing outfit {}", path.display()))?;
    let outfit_names: Vec<String> = file.skeleton.bones.iter().map(|b| b.name.clone()).collect();
    let mut mesh = file.mesh;
    if !outfit_names.is_empty() {
        remap_outfit_joints(&mut mesh, &outfit_names, base);
    }
    Ok((mesh, file.attach))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE ADDITIVE CONTRACT (spec 0A81088E, rule 7C46FAC4): a rig written before the region
    /// tagger existed carries no `tag`, no `verts`, no `chain_count` — and still loads, with the
    /// meaning it always had: one hang of cloth off its anchor bone.
    #[test]
    fn a_rig_from_before_the_tagger_still_loads_its_cloth() {
        let json = r#"{"regions":[{"name":"hem","anchor_bone":"pelvis",
            "chains":[{"anchor":[0,0,90],"dir":[0,0,-1],"seg_len":4.0,"segments":5}],
            "binds":[{"v":7,"c":0,"k":2,"f":0.25}]}]}"#;
        let cloth: Cloth = serde_json::from_str(json).expect("old cloth deserialises");
        let r = &cloth.regions[0];
        assert_eq!(r.tag, RegionTag::Cloth);
        assert!(r.verts.is_empty());
        assert_eq!(r.chain_count, 1, "an old region is a single hang");
        assert_eq!(r.chains.len(), 1);
        assert_eq!(r.binds.len(), 1);
    }

    /// The new fields survive the wire — a tagged, combed region reads back exactly as written.
    #[test]
    fn a_rig_file_round_trips_a_tagged_region() {
        let mut rig = RigFile {
            format: "flicker.rig".to_string(),
            version: 1,
            source: Source::default(),
            skeleton: Skeleton { bones: Vec::new() },
            mesh: Mesh::default(),
            clips: Vec::new(),
            attach: Attach::default(),
            attach_points: Vec::new(),
            collision: Collision::default(),
            retarget: true,
            skeleton_recipe: None,
        };
        rig.mesh.cloth.regions.push(ClothRegion {
            name: "mane_01".to_string(),
            anchor_bone: "neck_01".to_string(),
            tag: RegionTag::Mane,
            verts: vec![3, 4, 5],
            chain_count: 4,
            params: ClothParams::default(),
            chains: vec![ClothChain {
                anchor: [1.0, 2.0, 3.0],
                dir: [0.0, 0.0, -1.0],
                seg_len: 2.5,
                segments: 5,
            }],
            binds: vec![ClothBind {
                v: 4,
                c: 0,
                k: 1,
                f: 0.5,
            }],
        });
        let text = serde_json::to_string(&rig).expect("serialises");
        let back: RigFile = serde_json::from_str(&text).expect("deserialises");
        let r = &back.mesh.cloth.regions[0];
        assert_eq!(r.tag, RegionTag::Mane);
        assert_eq!(r.verts, vec![3, 4, 5]);
        assert_eq!(r.chain_count, 4);
        assert_eq!(r.binds[0].f, 0.5);
    }

    /// PATTERN-AWARE LIBRARIES (the modular skeleton, 2026-09-08): a body whose recipe animates
    /// on a non-humanoid PATTERN loads the sibling `<library>@<Pattern>` bake in place of the
    /// library it was passed — whatever preset name the recipe carries; a humanoid body (with or
    /// without a recipe) loads the library as passed.
    #[test]
    fn a_body_with_a_pattern_loads_the_libraries_baked_for_it() {
        let root = std::env::temp_dir().join("flicker_format_preset_swap");
        let _ = std::fs::remove_dir_all(&root);
        let id16 = "[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]";
        let bones = format!(
            r#"[{{"name":"root","parent":-1,"local":{id16},"inverse_bind":{id16}}},{{"name":"pelvis","parent":0,"local":{id16},"inverse_bind":{id16}}}]"#
        );
        let write = |rel: &str, body: String| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        };
        let clip = |name: &str| {
            format!(
                r#"{{"format":"flicker.rig","version":1,"skeleton":{{"bones":{bones}}},"mesh":{{"vertices":[],"indices":[]}},"clips":[{{"name":"{name}","tick_rate_hz":60,"duration_ticks":2,"tracks":[]}}]}}"#
            )
        };
        let mesh = r#"{"vertices":[{"p":[0,0,0],"n":[0,0,1],"uv":[0,0],"joints":[1,0,0,0],"weights":[1,0,0,0]}],"indices":[0]}"#;
        write(
            "Tailed/Tailed.json",
            format!(
                r#"{{"format":"flicker.rig","version":1,"skeleton":{{"bones":{bones}}},"mesh":{mesh},"skeleton_recipe":{{"trunk":{{"legs":[{{"Digitigrade":{{"heel":0.14}}}}],"tails":[{{"ShortHair":{{"bones":6}}}}]}},"preset":"Catman"}}}}"#
            ),
        );
        write(
            "Plain/Plain.json",
            format!(
                r#"{{"format":"flicker.rig","version":1,"skeleton":{{"bones":{bones}}},"mesh":{mesh}}}"#
            ),
        );
        write("retarget/clips/lib/In-Place/walk.json", clip("walk"));
        write(
            "retarget/clips/lib@Digitigrade/In-Place/walk_tailed.json",
            clip("walk_tailed"),
        );

        let lib = root.join("retarget/clips/lib");
        let tailed = load_dirs(&[&root.join("Tailed"), &lib]).expect("the tailed body loads");
        let names: Vec<&str> = tailed.clips.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["walk_tailed"],
            "the PATTERN's bake replaced the library (the Catman preset has none of its own)"
        );
        let plain = load_dirs(&[&root.join("Plain"), &lib]).expect("the plain body loads");
        let names: Vec<&str> = plain.clips.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["walk"], "no preset: the library loads as passed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// THE PATTERN IS THE FIRST LEG: a recipe's basis follows its root trunk's leg module and
    /// nothing else — a tail, hanging arms or a missing head change no pattern; no legs is a
    /// humanoid basis; every pattern has one spelling that round-trips by name.
    #[test]
    fn the_pattern_is_the_first_leg_module() {
        let mut r = SkeletonRecipe::humanoid();
        assert_eq!(r.pattern(), Pattern::Humanoid);
        r.trunk.head = false;
        r.trunk.arms = vec![ArmKind::Hanging];
        r.trunk.tails = vec![TailKind::Long { bones: 8 }];
        assert_eq!(
            r.pattern(),
            Pattern::Humanoid,
            "head, arms and tails change no basis"
        );
        r.trunk.legs = vec![LegKind::Digitigrade { heel: 0.06 }];
        assert_eq!(
            r.pattern(),
            Pattern::Digitigrade,
            "any heel height is the one basis"
        );
        r.trunk.legs = vec![Pattern::ToeWalker.leg()];
        assert_eq!(r.pattern(), Pattern::ToeWalker);
        r.trunk.legs = vec![LegKind::Unguligrade, LegKind::Plantigrade];
        assert_eq!(
            r.pattern(),
            Pattern::Unguligrade,
            "the FIRST leg pair decides"
        );
        r.trunk.legs.clear();
        assert_eq!(
            r.pattern(),
            Pattern::Humanoid,
            "no legs is a humanoid basis"
        );
        r.trunk.orientation = Orientation::Quadruped;
        r.trunk.legs = vec![LegKind::Digitigrade { heel: 0.2 }];
        assert_eq!(
            r.pattern(),
            Pattern::Quadruped,
            "a quadruped trunk is its own basis whatever its legs"
        );
        assert!(
            (TrunkSpec::default().length - TrunkSpec::DEFAULT_LENGTH).abs() < 1e-6,
            "the body-length knob defaults"
        );
        r.trunk.orientation = Orientation::Biped;
        r.trunk.legs = vec![LegKind::Bird { heel: 0.25 }];
        r.trunk.arms = vec![ArmKind::Humanoid];
        assert_eq!(r.pattern(), Pattern::Bird, "bird legs are a bird basis");
        r.trunk.legs = vec![LegKind::Plantigrade];
        r.trunk.arms = vec![ArmKind::Bird];
        assert_eq!(
            r.pattern(),
            Pattern::Bird,
            "bird wings are a bird basis whatever the legs"
        );
        r.trunk.arms = vec![ArmKind::Bat];
        assert_eq!(r.pattern(), Pattern::Bat);
        for p in Pattern::ALL {
            assert_eq!(Pattern::from_name(p.name()), Some(p));
            if matches!(p, Pattern::Quadruped | Pattern::Bat) {
                continue; // decided by the orientation / the wings, not the leg
            }
            assert_eq!(
                pattern_of(&SkeletonRecipe {
                    trunk: TrunkSpec {
                        legs: vec![p.leg()],
                        ..TrunkSpec::default()
                    },
                    preset: None
                }),
                p
            );
        }
        assert_eq!(
            Pattern::from_name("Lizardman"),
            None,
            "a preset is not a pattern"
        );
    }

    fn bone(name: &str, parent: i32) -> Bone {
        Bone {
            name: name.to_string(),
            parent,
            local: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        }
    }

    /// The outfit's reduced bone list is in a DIFFERENT order/subset than the base;
    /// `remap_outfit_joints` must rewrite every joint index by NAME into base space.
    #[test]
    fn outfit_joints_remap_by_name() {
        let base = vec![bone("root", -1), bone("spine", 0), bone("arm", 1)];
        // Reduced outfit skeleton: only two bones, listed arm-first.
        let outfit_names = vec!["arm".to_string(), "spine".to_string()];
        let mut mesh = Mesh::default();
        mesh.vertices.push(Vertex {
            p: [0.0, 0.0, 0.0],
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            // outfit-local: joint 0 = arm, joint 1 = spine, then 0-padding.
            joints: [0, 1, 0, 0],
            weights: [0.5, 0.5, 0.0, 0.0],
        });
        remap_outfit_joints(&mut mesh, &outfit_names, &base);
        // arm→2, spine→1; the padded slots (outfit joint 0 = arm) also map to 2.
        assert_eq!(mesh.vertices[0].joints, [2, 1, 2, 2]);
    }

    /// WS-C C-α: the new self-describing sections round-trip through serde — Material `emit`/`orm`,
    /// the `attach` mount (with the legacy `slot` alias folding into `socket`), and `collision`
    /// volumes with their tagged shape + role.
    #[test]
    fn self_describing_sections_deserialize() {
        let json = r#"{
            "format": "flicker.rig", "version": 2,
            "mesh": { "materials": [ { "name": "body", "base_color": "Body_BaseColor.png",
                "emit": "Body_Emit.png", "orm": "Body_ORM.png" } ] },
            "attach": { "slot": "lhand", "offset": [1.0, 2.0, 3.0], "rotate": [0.0, 0.0, 0.0, 1.0],
                "uniform": 37.5 },
            "collision": { "volumes": [
                { "name": "pelvis_hull", "bone": "pelvis",
                  "shape": { "kind": "capsule", "a": [0.0,0.0,0.0], "b": [0.0,0.0,10.0], "radius": 8.0 },
                  "role": "physics" },
                { "name": "blade_edge", "bone": "Weapon_R",
                  "shape": { "kind": "box", "center": [0.0,0.0,0.0], "half_extents": [1.0,1.0,30.0] },
                  "role": "hitbox" }
            ] }
        }"#;
        let f: RigFile = serde_json::from_str(json).expect("self-describing rig parses");
        let m = &f.mesh.materials[0];
        assert_eq!(m.emit, "Body_Emit.png");
        assert_eq!(m.orm, "Body_ORM.png");
        // `slot` alias folds into `socket`; the omitted `scale` still defaults to unit.
        assert_eq!(f.attach.socket, "lhand");
        assert_eq!(f.attach.offset, [1.0, 2.0, 3.0]);
        assert_eq!(f.attach.uniform, 37.5);
        assert_eq!(
            f.attach.scale,
            [1.0, 1.0, 1.0],
            "omitted attach.scale defaults to unit"
        );
        assert_eq!(f.collision.volumes.len(), 2);
        assert!(matches!(
            f.collision.volumes[0].role,
            CollisionRole::Physics
        ));
        assert!(matches!(f.collision.volumes[0].shape,
            CollisionShape::Capsule { radius, .. } if radius == 8.0));
        assert!(matches!(f.collision.volumes[1].role, CollisionRole::Hitbox));
        assert!(matches!(
            f.collision.volumes[1].shape,
            CollisionShape::Box { .. }
        ));
    }

    /// WS-C C-α backward-compat: a file that predates the self-describing sections still loads —
    /// every new field is serde-default (emit/orm empty, attach identity/unit, collision empty),
    /// including when the whole `attach`/`collision` blocks are absent.
    #[test]
    fn legacy_rig_without_new_sections_defaults() {
        let json = r#"{
            "format": "flicker.rig", "version": 1,
            "mesh": { "materials": [ { "name": "body", "base_color": "Body_BaseColor.png",
                "roughness": "Body_Roughness.png" } ] }
        }"#;
        let f: RigFile = serde_json::from_str(json).expect("legacy rig parses");
        let m = &f.mesh.materials[0];
        assert_eq!(m.base_color, "Body_BaseColor.png");
        assert_eq!(m.emit, "", "emit defaults empty");
        assert_eq!(m.orm, "", "orm defaults empty");
        // Absent `attach` block must default to identity/unit (not the derived all-zeros).
        assert_eq!(f.attach.socket, "");
        assert_eq!(
            f.attach.rotate,
            [0.0, 0.0, 0.0, 1.0],
            "absent attach rotate = identity"
        );
        assert_eq!(
            f.attach.scale,
            [1.0, 1.0, 1.0],
            "absent attach scale = unit"
        );
        assert_eq!(f.attach.uniform, 1.0, "absent attach uniform = one");
        assert!(f.collision.volumes.is_empty(), "collision defaults empty");
    }

    /// WS-C C-γ: `load_mesh_with_attach` surfaces a prop's inline `attach` mount straight from its
    /// one self-describing file (the folded `fits.json`) — `slot` alias, unit defaults for omitted
    /// fields.
    #[test]
    fn load_mesh_with_attach_reads_inline_attach() {
        let path = std::env::temp_dir().join("flicker_skeletal_ws_c_attach_roundtrip.json");
        std::fs::write(
            &path,
            r#"{ "format": "flicker.rig", "attach": { "slot": "lhand", "uniform": 37.5,
                "offset": [1.0, 2.0, 3.0] }, "mesh": { "vertices": [] } }"#,
        )
        .unwrap();
        let (mesh, attach) = load_mesh_with_attach(&path).expect("prop with inline attach loads");
        assert!(mesh.vertices.is_empty());
        assert_eq!(attach.socket, "lhand", "slot folds into socket");
        assert_eq!(attach.uniform, 37.5);
        assert_eq!(attach.offset, [1.0, 2.0, 3.0]);
        assert_eq!(
            attach.scale,
            [1.0, 1.0, 1.0],
            "omitted attach.scale defaults to unit"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A bone name the base doesn't have collapses to root (0), not out of bounds.
    #[test]
    fn outfit_unknown_bone_pins_to_root() {
        let base = vec![bone("root", -1), bone("spine", 0)];
        let outfit_names = vec!["spine".to_string(), "ghost".to_string()];
        let mut mesh = Mesh::default();
        mesh.vertices.push(Vertex {
            p: [0.0, 0.0, 0.0],
            n: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
            joints: [1, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        });
        remap_outfit_joints(&mut mesh, &outfit_names, &base);
        // outfit joint 1 = "ghost" (absent) → 0; joint 0 = "spine" → 1.
        assert_eq!(mesh.vertices[0].joints, [0, 1, 1, 1]);
    }

    /// D.1 regression: the canonical base-A rig loads through the real loader with the
    /// added face group (`jaw`, `eye_l`, `eye_r` under `head`) → the 67-bone canon, and no stray
    /// asset under the character dir breaks the recursive rig parse. `#[ignore]`d as a
    /// real-content read; run explicitly with `cargo test -p flicker-skeletal -- --ignored`.
    /// (Repointed 2026-08-04: the reference is the GOLEM — PrismHumanBaseA was swept.)
    #[test]
    #[ignore]
    fn loads_canonical_base_a_with_face_group() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/package/characters/GolemBase_Low");
        if !dir.exists() {
            eprintln!(
                "skipping: canonical content not present at {}",
                dir.display()
            );
            return;
        }
        let model = load_dir(&dir).expect("canonical reference rig should load");
        assert_eq!(
            model.bones.len(),
            67,
            "the reference must carry the 67-bone canon (source: baseline::TOPOLOGY)"
        );
        let head = model
            .bones
            .iter()
            .position(|b| b.name == "head")
            .expect("head bone present") as i32;
        for name in ["jaw", "eye_l", "eye_r"] {
            let b = model
                .bones
                .iter()
                .find(|b| b.name == name)
                .unwrap_or_else(|| panic!("missing face bone {name}"));
            assert_eq!(b.parent, head, "{name} must be a child of head");
        }
    }

    /// THE LOAD-TIME LIMB REBASE: a body whose upper arm rests 24° out plays a library baked on
    /// a 47° A-pose with its arm pointing exactly where the library's arm points under the
    /// clip — the body's own lengths, the library's directions — and a body identical to the
    /// library plays the keys untouched.
    #[test]
    fn a_narrow_a_pose_plays_a_library_arm_where_the_library_arm_points() {
        use crate::pose::{global_transforms, sample_local_poses};
        let arm = |angle_deg: f32, upper: f32, fore: f32| -> Vec<BoneRaw> {
            let (s, c) = angle_deg.to_radians().sin_cos();
            let dir = Vec3::new(s, 0.0, -c);
            let t = |v: Vec3| Mat4::from_translation(v).to_cols_array();
            vec![
                BoneRaw {
                    name: "root".into(),
                    parent: -1,
                    local: t(Vec3::ZERO),
                    inverse_bind: identity16(),
                },
                BoneRaw {
                    name: "spine_03".into(),
                    parent: 0,
                    local: t(Vec3::new(0.0, 0.0, 120.0)),
                    inverse_bind: identity16(),
                },
                BoneRaw {
                    name: "upperarm_l".into(),
                    parent: 1,
                    local: t(Vec3::new(20.0, 0.0, 20.0)),
                    inverse_bind: identity16(),
                },
                BoneRaw {
                    name: "lowerarm_l".into(),
                    parent: 2,
                    local: t(dir * upper),
                    inverse_bind: identity16(),
                },
                BoneRaw {
                    name: "hand_l".into(),
                    parent: 3,
                    local: t(dir * fore),
                    inverse_bind: identity16(),
                },
                BoneRaw {
                    name: "middle_01_l".into(),
                    parent: 4,
                    local: t(dir * 8.0),
                    inverse_bind: identity16(),
                },
            ]
        };
        // The library: a 47° A-pose. The clip swings the upper arm 30° about Y and bends the
        // forearm 40° about X.
        let library: RigFile = serde_json::from_value(serde_json::json!({
            "format": "flicker.rig", "version": 1,
            "skeleton": { "bones": arm(47.0, 32.0, 25.0) },
            "clips": [{ "name": "swing", "tick_rate_hz": 60, "duration_ticks": 1, "tracks": [
                { "bone": "upperarm_l", "keys": [{ "t": 0, "T": [20.0, 0.0, 20.0], "R": Quat::from_rotation_y(0.5).to_array(), "S": [1.0, 1.0, 1.0] }] },
                { "bone": "lowerarm_l", "keys": [{ "t": 0, "T": (Vec3::new(47f32.to_radians().sin(), 0.0, -(47f32.to_radians().cos())) * 32.0).to_array(), "R": Quat::from_rotation_x(0.7).to_array(), "S": [1.0, 1.0, 1.0] }] }
            ] }]
        }))
        .expect("a library rig parses");
        let lib_bones = rig_bones(&library);
        let body_bones = rig_bones(&RigFile {
            skeleton: Skeleton {
                bones: arm(24.0, 28.0, 22.0),
            },
            ..serde_json::from_value(serde_json::json!({ "format": "flicker.rig", "version": 1 }))
                .expect("an empty rig parses")
        });
        let pose = |bones: &[Bone]| {
            let clip = resolve_clips(&library, bones, false)
                .pop()
                .expect("resolves");
            global_transforms(bones, &sample_local_poses(bones, &clip, 0, true))
        };
        let (gl, gb) = (pose(&lib_bones), pose(&body_bones));
        let dir = |g: &[Mat4], a: usize, b: usize| {
            (g[b].w_axis.truncate() - g[a].w_axis.truncate()).normalize()
        };
        for (a, b, what) in [(2, 3, "upper arm"), (3, 4, "forearm"), (4, 5, "hand")] {
            let (dl, db) = (dir(&gl, a, b), dir(&gb, a, b));
            assert!(
                dl.dot(db) > 0.9999,
                "{what}: the body points where the library points: {dl} vs {db}"
            );
        }
        let len = |g: &[Mat4], a: usize, b: usize| {
            g[b].w_axis.truncate().distance(g[a].w_axis.truncate())
        };
        assert!(
            (len(&gb, 2, 3) - 28.0).abs() < 1e-3 && (len(&gb, 3, 4) - 22.0).abs() < 1e-3,
            "with the body's own lengths"
        );
        // A body identical to the library: every key untouched.
        let same = resolve_clips(&library, &lib_bones, false).pop().unwrap();
        for (tr, src) in same.tracks.iter().zip(&library.clips[0].tracks) {
            for (k, s) in tr.keys.iter().zip(&src.keys) {
                let (a, b) = (Quat::from_array(k.rotation), Quat::from_array(s.rotation));
                assert!(
                    a.angle_between(b) < 1e-5,
                    "an identical body plays the keys as baked"
                );
            }
        }
    }
}
