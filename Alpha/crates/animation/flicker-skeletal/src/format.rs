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
use serde::Deserialize;

// ─────────────────────────────── wire types (verbatim contract) ───────────────

#[derive(Debug, Deserialize)]
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
}

#[derive(Debug, Default, Deserialize)]
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

#[derive(Debug, Default, Deserialize)]
pub struct Skeleton {
    #[serde(default)]
    pub bones: Vec<BoneRaw>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Default, Deserialize)]
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
}

/// A contiguous run of `indices` sharing one material. Because the converter emits
/// a non-deduplicated vertex list with sequential indices, `[start, start+count)`
/// is equally a range into `indices` and into `vertices`.
#[derive(Debug, Clone, Deserialize)]
pub struct Submesh {
    pub material: usize,
    pub start: usize,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
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
    /// Flat RGB (0..1) used when `base_color` is empty (untextured props). Empty →
    /// neutral gray.
    #[serde(default)]
    pub color: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub struct Vertex {
    pub p: [f32; 3],
    pub n: [f32; 3],
    #[serde(default)]
    pub uv: [f32; 2],
    pub joints: [u32; 4],
    pub weights: [f32; 4],
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct Track {
    pub bone: String,
    pub keys: Vec<Keyframe>,
}

#[derive(Debug, Clone, Deserialize)]
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
    let mut files = Vec::new();
    collect_json_files(dir, &mut files)?;
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
            "no .json rig/clip assets found in {} — run the fbximport converter and \
             copy its output here",
            dir.display()
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
        for clip in &f.clips {
            let mut tracks = Vec::new();
            let mut unresolved = Vec::new();
            for tr in &clip.tracks {
                match name_to_index.get(tr.bone.as_str()) {
                    Some(&bi) => tracks.push(ResolvedTrack {
                        bone: bi,
                        keys: tr.keys.clone(),
                    }),
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
    })
}

/// Load a single mesh JSON as a static prop — geometry only (submeshes + materials).
/// Bones/clips are ignored; the prop is rendered rigid at an attach transform. In the
/// same source space (Z-up/cm) as the rig, so the rig's `world` matrix maps it too.
pub fn load_mesh(path: &Path) -> Result<Mesh> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading prop {}", path.display()))?;
    let file: RigFile = serde_json::from_str(&text)
        .with_context(|| format!("parsing prop {}", path.display()))?;
    Ok(file.mesh)
}
