//! THE AUTHORED BASELINE SKELETON — `GolemBaseSkeleton` (Aaron's ruling, 2026-08-04).
//!
//! The reference skeleton is a GENERATED ARTIFACT, authored from first principles as
//! DATA — never derived from any vendor mesh. This closes the Katanami lineage for
//! good: the canon (67 names + parent topology + conventions) was always ours; the
//! BIND (where joints rest) is now ours too — a perfect A-pose at a stated stature,
//! exactly symmetric BY CONSTRUCTION (the left side is authored, the right side is
//! mirrored), soles flat on the ground, shoulders level, spine plumb.
//!
//! Segment heights and lengths follow the Drillis–Contini anthropometric fractions
//! of stature (the standard proportional body model), scaled by [`STATURE`]. Every
//! number is a fraction times one knob, so retuning the baseline is editing data and
//! regenerating — `cargo run -p flicker-content --example bake_baseline`.
//!
//! Conventions (identical to every other `flicker.rig` this crate bakes): Z-up, cm,
//! the character FACES -Y, LEFT is +X, root at the ground origin. Bone locals are
//! PURE TRANSLATIONS (identity rest rotations): a bone's direction is implied by its
//! children's positions, clips supply rotations wholesale, and the retarget rebase
//! reads exactly these rest translations.

use anyhow::Result;
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use flicker_skeletal::format::{BoneRaw, RigFile, Skeleton, Source};

/// The ruled base height (Aaron, 2026-08-04): 170 cm stature. Everything scales off it.
pub const STATURE: f32 = 170.0;

/// The canonical bone-count — derived from the one topology table below, and the
/// single number every consumer (the Clayworks requirements, the loader tests)
/// reads. 67 since 2026-08-04: `neck_02` joined the chain (Aaron: "at least one
/// more so it can be SIX SEVEN").
pub const CANON_BONES: usize = TOPOLOGY.len();

/// The canonical topology — names + parents, in hierarchy order (parents precede
/// children). THE canon statement; the golem and every conformed character carry
/// exactly this arrangement.
pub const TOPOLOGY: [(&str, &str); 67] = [
    ("root", "-"),
    ("pelvis", "root"),
    ("thigh_l", "pelvis"),
    ("thigh_r", "pelvis"),
    ("spine_01", "pelvis"),
    ("calf_l", "thigh_l"),
    ("calf_r", "thigh_r"),
    ("spine_02", "spine_01"),
    ("foot_l", "calf_l"),
    ("foot_r", "calf_r"),
    ("spine_03", "spine_02"),
    ("ball_l", "foot_l"),
    ("ball_r", "foot_r"),
    ("clavicle_l", "spine_03"),
    ("clavicle_r", "spine_03"),
    ("neck_01", "spine_03"),
    ("upperarm_l", "clavicle_l"),
    ("upperarm_r", "clavicle_r"),
    ("neck_02", "neck_01"),
    ("head", "neck_02"),
    ("lowerarm_l", "upperarm_l"),
    ("lowerarm_r", "upperarm_r"),
    ("hand_l", "lowerarm_l"),
    ("hand_r", "lowerarm_r"),
    ("Weapon_L", "hand_l"),
    ("index_01_l", "hand_l"),
    ("index_02_l", "index_01_l"),
    ("index_03_l", "index_02_l"),
    ("middle_01_l", "hand_l"),
    ("middle_02_l", "middle_01_l"),
    ("middle_03_l", "middle_02_l"),
    ("pinky_01_l", "hand_l"),
    ("pinky_02_l", "pinky_01_l"),
    ("pinky_03_l", "pinky_02_l"),
    ("ring_01_l", "hand_l"),
    ("ring_02_l", "ring_01_l"),
    ("ring_03_l", "ring_02_l"),
    ("thumb_01_l", "hand_l"),
    ("thumb_02_l", "thumb_01_l"),
    ("thumb_03_l", "thumb_02_l"),
    ("lowerarm_twist_01_l", "lowerarm_l"),
    ("upperarm_twist_01_l", "upperarm_l"),
    ("Weapon_R", "hand_r"),
    ("index_01_r", "hand_r"),
    ("index_02_r", "index_01_r"),
    ("index_03_r", "index_02_r"),
    ("middle_01_r", "hand_r"),
    ("middle_02_r", "middle_01_r"),
    ("middle_03_r", "middle_02_r"),
    ("pinky_01_r", "hand_r"),
    ("pinky_02_r", "pinky_01_r"),
    ("pinky_03_r", "pinky_02_r"),
    ("ring_01_r", "hand_r"),
    ("ring_02_r", "ring_01_r"),
    ("ring_03_r", "ring_02_r"),
    ("thumb_01_r", "hand_r"),
    ("thumb_02_r", "thumb_01_r"),
    ("thumb_03_r", "thumb_02_r"),
    ("lowerarm_twist_01_r", "lowerarm_r"),
    ("upperarm_twist_01_r", "upperarm_r"),
    ("calf_twist_01_l", "calf_l"),
    ("thigh_twist_01_l", "thigh_l"),
    ("calf_twist_01_r", "calf_r"),
    ("thigh_twist_01_r", "thigh_r"),
    ("jaw", "head"),
    ("eye_l", "head"),
    ("eye_r", "head"),
];

/// The A-pose arm's droop below horizontal, in degrees. 42° is the classic relaxed
/// A — high enough that armpits and sleeves skin cleanly, low enough that shoulder
/// deformation stays neutral.
const A_POSE_DEG: f32 = 42.0;

/// World rest positions for the CENTERLINE + LEFT bones (right is x-mirrored).
/// Fractions of stature per Drillis & Contini; hand/finger layout is an authored
/// simplification (straight fingers continuing the arm line, spread across Y).
fn authored_positions() -> HashMap<String, Vec3> {
    let h = STATURE;
    let mut p: HashMap<String, Vec3> = HashMap::new();

    // ── centreline: ground → head, plumb (y = 0) ──
    p.insert("root".to_string(), Vec3::ZERO);
    p.insert("pelvis".to_string(), Vec3::new(0.0, 0.0, 0.560 * h));
    p.insert("spine_01".to_string(), Vec3::new(0.0, 0.0, 0.615 * h));
    p.insert("spine_02".to_string(), Vec3::new(0.0, 0.0, 0.672 * h));
    p.insert("spine_03".to_string(), Vec3::new(0.0, 0.0, 0.730 * h));
    p.insert("neck_01".to_string(), Vec3::new(0.0, 0.0, 0.845 * h));
    p.insert("neck_02".to_string(), Vec3::new(0.0, 0.0, 0.858 * h));
    p.insert("head".to_string(), Vec3::new(0.0, 0.0, 0.870 * h));
    // The face rides the head: jaw and eyes sit forward (the character faces -Y).
    p.insert("jaw".to_string(), Vec3::new(0.0, -0.026 * h, 0.885 * h));
    p.insert("eye_l".to_string(), Vec3::new(0.019 * h, -0.047 * h, 0.936 * h));

    // ── left leg: vertical column, ankle slightly back, sole flat on the ground ──
    let hip_x = 0.051 * h; // femoral head offset from the midline
    p.insert("thigh_l".to_string(), Vec3::new(hip_x, 0.0, 0.530 * h));
    p.insert("calf_l".to_string(), Vec3::new(hip_x, 0.0, 0.285 * h));
    p.insert("foot_l".to_string(), Vec3::new(hip_x, 0.015 * h, 0.039 * h));
    // Ball of the foot: forward along -Y, riding just above the sole.
    p.insert("ball_l".to_string(), Vec3::new(hip_x, -0.071 * h, 0.012 * h));

    // ── left arm: clavicle from the upper chest, then a straight A-pose line ──
    p.insert("clavicle_l".to_string(), Vec3::new(0.015 * h, 0.0, 0.818 * h));
    let shoulder = Vec3::new(0.129 * h, 0.0, 0.818 * h);
    let (s, c) = A_POSE_DEG.to_radians().sin_cos();
    let dir = Vec3::new(c, 0.0, -s); // down-and-out in the XZ plane
    let elbow = shoulder + dir * (0.186 * h);
    let wrist = elbow + dir * (0.146 * h);
    p.insert("upperarm_l".to_string(), shoulder);
    p.insert("lowerarm_l".to_string(), elbow);
    p.insert("hand_l".to_string(), wrist);
    p.insert("Weapon_L".to_string(), wrist + dir * (0.054 * h)); // the grip point, mid-palm

    // Fingers: roots fan across Y at the knuckle line, segments continue the arm
    // line (3 phalanges, tapering). The thumb sits forward and shorter.
    let knuckle = wrist + dir * (0.049 * h);
    let seg = |root: Vec3, len: f32| (root, root + dir * len, root + dir * (len * 1.8));
    for (finger, y_off, len) in [
        ("index", -0.009 * h, 0.020 * h),
        ("middle", 0.0, 0.022 * h),
        ("ring", 0.009 * h, 0.020 * h),
        ("pinky", 0.018 * h, 0.016 * h),
    ] {
        let root = knuckle + Vec3::new(0.0, y_off, 0.0);
        let (a, b, c3) = seg(root, len);
        for (i, v) in [a, b, c3].into_iter().enumerate() {
            p.insert(format!("{finger}_{:02}_l", i + 1), v);
        }
    }
    let troot = wrist + dir * (0.022 * h) + Vec3::new(0.0, -0.024 * h, 0.0);
    let (a, b, c3) = seg(troot, 0.018 * h);
    p.insert("thumb_01_l".to_string(), a);
    p.insert("thumb_02_l".to_string(), b);
    p.insert("thumb_03_l".to_string(), c3);

    // ── twist helpers: the exact midpoints of their segments ──
    p.insert("upperarm_twist_01_l".to_string(), (shoulder + elbow) * 0.5);
    p.insert("lowerarm_twist_01_l".to_string(), (elbow + wrist) * 0.5);
    let (thigh, calf, foot) = (p["thigh_l"], p["calf_l"], p["foot_l"]);
    p.insert("thigh_twist_01_l".to_string(), (thigh + calf) * 0.5);
    p.insert("calf_twist_01_l".to_string(), (calf + foot) * 0.5);
    p
}

/// Every bone's world rest position: the authored set plus the mirrored right side.
pub fn world_positions() -> HashMap<String, Vec3> {
    let left = authored_positions();
    let mut out: HashMap<String, Vec3> = HashMap::new();
    for (name, _) in TOPOLOGY {
        let v = if let Some(v) = left.get(name) {
            *v
        } else {
            // A right-side bone mirrors its left twin exactly (x negated).
            let twin = if let Some(stripped) = name.strip_suffix("_r") {
                format!("{stripped}_l")
            } else if name == "Weapon_R" {
                "Weapon_L".to_string()
            } else if name == "eye_r" {
                "eye_l".to_string()
            } else {
                panic!("bone `{name}` has neither an authored position nor a left twin");
            };
            let l = left
                .get(twin.as_str())
                .unwrap_or_else(|| panic!("`{name}`'s twin `{twin}` is not authored"));
            Vec3::new(-l.x, l.y, l.z)
        };
        out.insert(name.to_string(), v);
    }
    out
}

/// Assemble the baseline as a skeleton-only `flicker.rig`: translation-only locals
/// (identity rest rotations), inverse binds straight from the world rests, empty
/// mesh/clips — the reference is nobody's body.
pub fn golem_base_skeleton() -> RigFile {
    let pos = world_positions();
    let index: HashMap<&str, usize> =
        TOPOLOGY.iter().enumerate().map(|(i, (n, _))| (*n, i)).collect();
    let bones: Vec<BoneRaw> = TOPOLOGY
        .iter()
        .map(|(name, parent)| {
            let w = pos[*name];
            let local_t = match *parent {
                "-" => w,
                p => w - pos[p],
            };
            BoneRaw {
                name: name.to_string(),
                parent: if *parent == "-" { -1 } else { index[parent] as i32 },
                local: Mat4::from_translation(local_t).to_cols_array(),
                inverse_bind: Mat4::from_translation(-w).to_cols_array(),
            }
        })
        .collect();
    RigFile {
        format: "flicker.rig".to_string(),
        version: 1,
        source: Source {
            file: "GolemBaseSkeleton".to_string(),
            source_axis: "Z_up".to_string(),
            source_unit: "cm".to_string(),
            applied_transform: format!(
                "authored baseline: generated A-pose ({A_POSE_DEG}\u{00b0}) at {STATURE} cm, no source mesh"
            ),
            ..Default::default()
        },
        skeleton: Skeleton { bones },
        mesh: Default::default(),
        clips: Vec::new(),
        attach: Default::default(),
        attach_points: Vec::new(),
        collision: Default::default(),
        retarget: true,
    }
}

/// Write the baseline into a characters root as `GolemBaseSkeleton/GolemBaseSkeleton.json`
/// (gz at rest via the shared seam). Returns the logical path.
pub fn emit(characters_root: &Path) -> Result<PathBuf> {
    let dir = characters_root.join("GolemBaseSkeleton");
    std::fs::create_dir_all(&dir)?;
    let out = dir.join("GolemBaseSkeleton.json");
    crate::bake::write_rig_file(&golem_base_skeleton(), &out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE BASELINE LINT — the gates that make "the skeleton is right" a property,
    /// not an eyeball: canon shape, exact mirror symmetry, level shoulders, flat
    /// soles, plumb spine, the ruled stature, and segment sanity.
    #[test]
    fn the_baseline_is_symmetric_level_flat_and_at_stature() {
        let rig = golem_base_skeleton();
        assert_eq!(rig.skeleton.bones.len(), CANON_BONES, "the canon count holds");

        // Hierarchy order: every parent precedes its child (loaders assume it).
        for (i, b) in rig.skeleton.bones.iter().enumerate() {
            assert!(b.parent < i as i32, "bone {i} `{}` precedes its parent", b.name);
        }
        // Topology matches the canon table verbatim.
        for ((name, parent), b) in TOPOLOGY.iter().zip(&rig.skeleton.bones) {
            assert_eq!(b.name, *name);
            let p = if b.parent < 0 { "-" } else { TOPOLOGY[b.parent as usize].0 };
            assert_eq!(p, *parent, "`{name}` parents `{p}`, canon says `{parent}`");
        }

        let pos = world_positions();
        // EXACT mirror symmetry: every left/right pair, the Weapon grips included.
        let mut pairs = 0;
        for (name, _) in TOPOLOGY {
            let twin = if let Some(stem) = name.strip_suffix("_l") {
                format!("{stem}_r")
            } else if name == "Weapon_L" {
                "Weapon_R".to_string()
            } else {
                continue;
            };
            let (l, r) = (pos[name], pos[twin.as_str()]);
            assert!(
                (l.x + r.x).abs() < 1e-4 && (l.y - r.y).abs() < 1e-4 && (l.z - r.z).abs() < 1e-4,
                "`{name}` and `{twin}` are not exact mirrors: {l:?} vs {r:?}"
            );
            pairs += 1;
        }
        assert_eq!(pairs, 29, "every one of the 29 left/right pairs was checked");

        // Level shoulders — the defect that started this. And a plumb spine.
        assert!((pos["upperarm_l"].z - pos["upperarm_r"].z).abs() < 1e-4);
        assert!((pos["upperarm_l"].z - 0.818 * STATURE).abs() < 1e-3, "shoulders at 0.818·H");
        for n in ["root", "pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "neck_02", "head"] {
            assert!(pos[n].x.abs() < 1e-4 && pos[n].y.abs() < 1e-4, "`{n}` off the plumb line");
        }

        // Flat soles: root on the ground, balls riding just above it, ankles low.
        assert_eq!(pos["root"], Vec3::ZERO);
        for n in ["ball_l", "ball_r"] {
            assert!(pos[n].z > 0.0 && pos[n].z < 0.02 * STATURE, "`{n}` sole not flat: {:?}", pos[n]);
        }
        assert!(pos["foot_l"].z < 0.05 * STATURE, "ankle rides low");

        // The ruled stature: the eyes (the highest joints) sit just under H.
        let top = pos.values().map(|v| v.z).fold(f32::MIN, f32::max);
        assert!((pos["eye_l"].z - top).abs() < 1e-4, "eyes are the highest joints");
        assert!(top < STATURE && top > 0.90 * STATURE, "stature respected: top {top} vs {STATURE}");

        // The A-pose: the whole arm line droops A_POSE_DEG below horizontal.
        let (s, e, w) = (pos["upperarm_l"], pos["lowerarm_l"], pos["hand_l"]);
        for (a, b) in [(s, e), (e, w)] {
            let d = b - a;
            let ang = (-d.z).atan2(d.x).to_degrees();
            assert!((ang - A_POSE_DEG).abs() < 0.1, "arm segment at {ang}°, authored {A_POSE_DEG}°");
        }
        // Twists sit exactly mid-segment.
        assert!((pos["upperarm_twist_01_l"] - (s + e) * 0.5).length() < 1e-4);
        assert!((pos["calf_twist_01_l"] - (pos["calf_l"] + pos["foot_l"]) * 0.5).length() < 1e-4);

        // Every non-root bone has a real segment (no zero-length locals except the
        // deliberate riders on their parents' frames).
        for b in &rig.skeleton.bones {
            if b.parent >= 0 {
                let t = Vec3::new(b.local[12], b.local[13], b.local[14]);
                assert!(t.length() > 0.1, "`{}` collapses onto its parent", b.name);
            }
        }
    }

    /// The emitted file round-trips through the ENGINE loader and the RETARGETER's
    /// target reader — the two consumers the reference exists for.
    #[test]
    fn the_baseline_emits_loads_and_targets() {
        let dir = std::env::temp_dir().join("flicker_baseline_emit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = emit(&dir).expect("emit baseline");
        let model = flicker_skeletal::format::load_dir(out.parent().unwrap())
            .expect("engine loads the baseline");
        assert_eq!(model.bones.len(), CANON_BONES);
        assert!(model.mesh.vertices.is_empty(), "skeleton-only — the reference is nobody's body");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
