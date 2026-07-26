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
use glam::{Mat4, Vec3};
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

/// One dangly region (a bell sleeve, a skirt hem …) — a fan of chains hung from one bone,
/// plus the region's vertices bound along those chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClothRegion {
    pub name: String,
    /// Body bone the chains hang from (drives their anchor point + home direction).
    pub anchor_bone: String,
    #[serde(default)]
    pub params: ClothParams,
    pub chains: Vec<ClothChain>,
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
        Self { gravity: [0.0, 0.0, -600.0], stiffness: 0.015, damping: 0.9, iterations: 8, max_dt: 1.0 / 30.0 }
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

/// Recursively collect every `*.json` path under `dir`.
fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading assets dir {}", dir.display()))?
    {
        let path = entry?.path();
        if path.is_dir() {
            collect_json_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

/// Whether any component of `path` equals `name` (e.g. `clips`, `RootMotion`).
fn path_has_component(path: &Path, name: &str) -> bool {
    path.components().any(|c| c.as_os_str().to_str() == Some(name))
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

/// Like [`load_dir`] but gathers rig/clip JSON from SEVERAL directories — for a base
/// body that borrows another character's clip library (e.g. `base_human_female` playing
/// the Katanami animation set, which resolves by shared bone names). The rig authority is
/// still the single file with the most mesh vertices across all the dirs, so the base
/// body (dense mesh) wins over the clip files (skeleton-only).
pub fn load_dirs(dirs: &[&Path]) -> Result<Model> {
    let mut files = Vec::new();
    for dir in dirs {
        collect_json_files(dir, &mut files)?;
    }
    let mut parsed: Vec<(PathBuf, RigFile)> = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let file: RigFile = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        parsed.push((path, file));
    }
    if parsed.is_empty() {
        anyhow::bail!(
            "no .json rig/clip assets found in {:?} — run the exporter and copy its output here",
            dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>()
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

    let bones: Vec<Bone> = rig_file
        .skeleton
        .bones
        .iter()
        .map(|b| Bone {
            name: b.name.clone(),
            parent: b.parent,
            local: mat4_from_contract(&b.local),
            inverse_bind: mat4_from_contract(&b.inverse_bind),
        })
        .collect();
    if bones.is_empty() {
        anyhow::bail!("rig skeleton has no bones");
    }

    let name_to_index: HashMap<&str, usize> = bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();

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
        // Rest translations from THIS clip file's OWN skeleton — the space the clip's
        // translations were authored in. Retarget rebases each bone's animated translation off
        // this onto the target rig's rest (see `ResolvedTrack::source_rest`).
        let src_rest: HashMap<&str, [f32; 3]> = f
            .skeleton
            .bones
            .iter()
            .map(|b| (b.name.as_str(), [b.local[12], b.local[13], b.local[14]]))
            .collect();
        for clip in &f.clips {
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
                        tracks.push(ResolvedTrack {
                            bone: bi,
                            keys: tr.keys.clone(),
                            source_rest,
                        });
                    }
                    None => unresolved.push(tr.bone.clone()),
                }
            }
            let name = if root_motion {
                format!("RM/{}", clip.name)
            } else {
                clip.name.clone()
            };
            clips.push(ResolvedClip {
                name,
                tick_rate_hz: clip.tick_rate_hz,
                duration_ticks: clip.duration_ticks,
                tracks,
                unresolved,
            });
        }
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
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading prop {}", path.display()))?;
    let file: RigFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing prop {}", path.display()))?;
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
    let text = std::fs::read_to_string(path)
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
        assert_eq!(f.attach.scale, [1.0, 1.0, 1.0], "omitted attach.scale defaults to unit");
        assert_eq!(f.collision.volumes.len(), 2);
        assert!(matches!(f.collision.volumes[0].role, CollisionRole::Physics));
        assert!(matches!(f.collision.volumes[0].shape,
            CollisionShape::Capsule { radius, .. } if radius == 8.0));
        assert!(matches!(f.collision.volumes[1].role, CollisionRole::Hitbox));
        assert!(matches!(f.collision.volumes[1].shape, CollisionShape::Box { .. }));
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
        assert_eq!(f.attach.rotate, [0.0, 0.0, 0.0, 1.0], "absent attach rotate = identity");
        assert_eq!(f.attach.scale, [1.0, 1.0, 1.0], "absent attach scale = unit");
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
        assert_eq!(attach.scale, [1.0, 1.0, 1.0], "omitted attach.scale defaults to unit");
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
    /// added face group (`jaw`, `eye_l`, `eye_r` under `head`) → 66 bones, and no stray
    /// asset under the character dir breaks the recursive rig parse. `#[ignore]`d because
    /// it reads the ~33 MB canonical rig; run explicitly with
    /// `cargo test -p flicker-skeletal -- --ignored`.
    #[test]
    #[ignore]
    fn loads_canonical_base_a_with_face_group() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/characters/PrismHumanBaseA");
        if !dir.exists() {
            eprintln!("skipping: canonical content not present at {}", dir.display());
            return;
        }
        let model = load_dir(&dir).expect("canonical base-A rig should load");
        assert_eq!(model.bones.len(), 66, "base-A + face group must be 66 bones");
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
}
