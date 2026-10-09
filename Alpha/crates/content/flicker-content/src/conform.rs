//! Conform a parsed body to our canonical skeleton — the in-app port of
//! `rename_meshy_to_canonical.py::reorient_to_canonical`, referencing the AUTHORED
//! **GolemBaseSkeleton** baseline (2026-08-04; see [`crate::baseline`] — the Katanami-derived
//! reference lineage is retired, and the live Motifect clips are retarget-baked against the
//! same authored bind bodies conform to).
//!
//! `reorient` rebuilds each bone's rest frame in two steps: (1) base frame = the REFERENCE's world
//! orientation + THIS body's positions (fixes the Mixamo Y-down-bone vs UE X-down-bone convention);
//! (2) LIMB-ALIGN each limb bone (arms/hands/legs/feet) — rotate its frame by the minimal rotation taking
//! the reference's limb direction onto THIS body's, so its axis points down this body's own limb and
//! the shared clips' absolute rotations land where they should. Torso bones (pelvis/spine/clavicle/
//! neck/head) are NEVER limb-aligned (the pelvis→child tilt trap) — they keep the reference frame.
//! No `retarget_rot = t·s⁻¹` — absolute retarget only (03BBF8F4). Follow-on: infer + hip-width.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::path::Path;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};

use flicker_skeletal::format::{ArmKind, SkeletonRecipe, TrunkSpec};

use crate::fbx::{RawBone, RawModel};
use crate::flesh::{narrowings, Flesh};
use crate::shape::{Body, ShapeGraph};

struct RefBone {
    name: String,
    parent: i32,
    local: Mat4,
}

/// Load a flicker.rig skeleton as glam frames. `local[16]` is column-major storage of a
/// column-vector matrix → `Mat4::from_cols_array` with NO transpose (the flicker.rig contract).
fn load_reference_skeleton(path: &Path) -> Result<Vec<RefBone>> {
    // Gz-transparent read: the reference rig ships gz-at-rest in the package.
    let text = crate::package::read_text(path)
        .with_context(|| format!("reading reference {}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing reference {}", path.display()))?;
    let bones = v["skeleton"]["bones"]
        .as_array()
        .context("reference has no skeleton.bones")?;
    let mut out = Vec::with_capacity(bones.len());
    for b in bones {
        let name = b["name"].as_str().context("bone.name")?.to_string();
        let parent = b["parent"].as_i64().context("bone.parent")? as i32;
        let local = b["local"].as_array().context("bone.local")?;
        let mut m = [0.0f32; 16];
        for (i, f) in local.iter().enumerate().take(16) {
            m[i] = f.as_f64().unwrap_or(0.0) as f32;
        }
        out.push(RefBone {
            name,
            parent,
            local: Mat4::from_cols_array(&m),
        });
    }
    Ok(out)
}

/// FK: world matrix per bone. `global = parent_global * local` (glam column-vector).
fn fk(locals: &[Mat4], parents: &[i32]) -> Vec<Mat4> {
    let mut g = vec![Mat4::IDENTITY; locals.len()];
    for i in 0..locals.len() {
        g[i] = if parents[i] < 0 {
            locals[i]
        } else {
            g[parents[i] as usize] * locals[i]
        };
    }
    g
}

/// Limb bone → the child joint down the same chain that defines its direction. Torso bones are
/// absent — deliberately NOT limb-aligned. The HAND is a limb too (2026-08-21): left on the
/// canon's world orientation, a hand whose flesh does not continue the canon's arm line played
/// every clip's hand direction that far off — 37° on the golem's A-pose, "hands bent away from the
/// default angle". Its finger root (`middle_01`) is the joint a human annotates to say where the
/// hand points; see [`canonical_world_frames`] for a hand that has no fingers yet.
fn limb_child(name: &str) -> Option<&'static str> {
    Some(match name {
        "upperarm_l" => "lowerarm_l",
        "lowerarm_l" => "hand_l",
        "hand_l" => "middle_01_l",
        "upperarm_r" => "lowerarm_r",
        "lowerarm_r" => "hand_r",
        "hand_r" => "middle_01_r",
        "thigh_l" => "calf_l",
        "calf_l" => "foot_l",
        "foot_l" => "ball_l",
        "thigh_r" => "calf_r",
        "calf_r" => "foot_r",
        "foot_r" => "ball_r",
        _ => return None,
    })
}

/// A frame with the given orientation basis (columns of `basis`) and translation `pos`.
fn frame(basis: Mat4, pos: Vec3) -> Mat4 {
    Mat4::from_cols(
        basis.x_axis.truncate().extend(0.0),
        basis.y_axis.truncate().extend(0.0),
        basis.z_axis.truncate().extend(0.0),
        pos.extend(1.0),
    )
}

fn pos_of(m: Mat4) -> Vec3 {
    m.w_axis.truncate()
}

/// A frame whose basis is `basis` rotated by `q`, translation `pos` (limb-align keeps position).
fn rotated_frame(q: Quat, basis: Mat4, pos: Vec3) -> Mat4 {
    Mat4::from_cols(
        (q * basis.x_axis.truncate()).extend(0.0),
        (q * basis.y_axis.truncate()).extend(0.0),
        (q * basis.z_axis.truncate()).extend(0.0),
        pos.extend(1.0),
    )
}

/// This body's world rest frames, FK'd from the parsed local TRS.
pub(crate) fn model_world_frames(model: &RawModel) -> Vec<Mat4> {
    let locals: Vec<Mat4> = model
        .bones
        .iter()
        .map(|b| {
            Mat4::from_scale_rotation_translation(
                Vec3::from(b.scale),
                Quat::from_array(b.rotation),
                Vec3::from(b.translation),
            )
        })
        .collect();
    let parents: Vec<i32> = model.bones.iter().map(|b| b.parent).collect();
    fk(&locals, &parents)
}

/// Write world frames `w` back onto the bones: derive each bone's local (relative to its parent) +
/// `inverse_bind` (= `world.inverse()`). Only bones whose world frame changed shift; a child whose
/// world is unchanged simply gets a new local that absorbs its parent's shift.
pub(crate) fn write_world_frames(bones: &mut [RawBone], w: &[Mat4]) {
    for i in 0..bones.len() {
        let p = bones[i].parent;
        let local = if p < 0 {
            w[i]
        } else {
            w[p as usize].inverse() * w[i]
        };
        let (s, r, t) = local.to_scale_rotation_translation();
        bones[i].scale = s.to_array();
        bones[i].rotation = r.to_array();
        bones[i].translation = t.to_array();
        bones[i].inverse_bind = w[i].inverse().to_cols_array();
    }
}

/// A COMPOSED rig's frames are the IDENTITY at every bone — the pattern skeletons are emitted so,
/// and the clip libraries are baked against them — and a bake keeps them so: every bone's world
/// frame becomes a pure translation at its joint, its local and inverse bind rewritten to match,
/// the joint positions untouched (the rest skin stays the identity). A VENDOR rig's frames are the
/// conform's business ([`reorient_to_canonical`]); turning a composed rig's limb frames down its
/// limbs, as that does, left the lizardman's foot frame 44° off the frames its clips were baked
/// on, and every clip lifted its toes 30 cm (Aaron 2026-09-07).
pub fn straighten_frames(model: &mut RawModel) {
    let world: Vec<Mat4> = model_world_frames(model)
        .into_iter()
        .map(|w| Mat4::from_translation(w.w_axis.truncate()))
        .collect();
    write_world_frames(&mut model.bones, &world);
}

/// What `derive_hip_placement` moved: per side, `(current_x, target_x, widest_flesh)` in cm.
#[derive(Debug, Clone, Default)]
pub struct HipReport {
    pub left: Option<(f32, f32, f32)>,
    pub right: Option<(f32, f32, f32)>,
}

/// Correct the femoral heads' WIDTH from the body's own mesh — the in-app port of
/// `rename_meshy_to_canonical.py::derive_hip_placement`. Meshy plants the hips too medial and offers
/// no control (the user can place only the groin), so the pipeline MUST derive hip width. Runs BEFORE
/// [`reorient_to_canonical`] (which consumes rest positions).
///
/// Rule (handoff §4): the femoral head sits **50 % of the way from the midline to the WIDEST HIP**.
/// Width is measured from flesh owned by pelvis/thigh (weight ≥ 0.5), which excludes the A-posed arms
/// — a plain "widest vertex at hip height" reads ~40 cm because the HANDS are the widest thing there.
/// **WIDTH ONLY** — Meshy's bone LENGTHS are trusted (memory 03BBF8F4); only joint widths are not, so
/// y/z and every other bone keep their position and just the thigh x moves.
pub fn derive_hip_placement(model: &mut RawModel) -> HipReport {
    let idx: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    let (Some(&pelvis), Some(&thigh_l), Some(&thigh_r)) =
        (idx.get("pelvis"), idx.get("thigh_l"), idx.get("thigh_r"))
    else {
        return HipReport::default();
    };

    let mut w = model_world_frames(model);
    let mid = pos_of(w[pelvis]).x;

    // Furthest hip flesh from the midline on `side`, from pelvis/thigh-owned verts only (weight≥0.5).
    let widest = |thigh: usize, sign: f32| -> Option<f32> {
        model
            .vertices
            .iter()
            .filter(|v| {
                (0..4).any(|k| {
                    (v.joints[k] as usize == pelvis || v.joints[k] as usize == thigh)
                        && v.weights[k] >= 0.5
                })
            })
            .map(|v| sign * (v.p[0] - mid))
            .filter(|d| *d > 0.0)
            .fold(None, |acc: Option<f32>, d| {
                Some(acc.map_or(d, |a| a.max(d)))
            })
    };

    let mut report = HipReport::default();
    for (thigh, sign, is_left) in [(thigh_l, 1.0f32, true), (thigh_r, -1.0f32, false)] {
        let Some(width) = widest(thigh, sign) else {
            continue;
        };
        let cur = pos_of(w[thigh]).x;
        let tgt = mid + sign * 0.5 * width;
        // WIDTH only: x moves, y/z (and every other bone) untouched.
        let mut col = w[thigh].w_axis;
        col.x = tgt;
        w[thigh].w_axis = col;
        let entry = Some((cur, tgt, width));
        if is_left {
            report.left = entry
        } else {
            report.right = entry
        }
    }

    write_world_frames(&mut model.bones, &w);
    report
}

/// What `derive_shoulder_placement` moved: per side, `(current_x, target_x, widest_flesh)` in cm.
#[derive(Debug, Clone, Default)]
pub struct ShoulderReport {
    pub left: Option<(f32, f32, f32)>,
    pub right: Option<(f32, f32, f32)>,
}

/// Fraction of the way from the shoulder midline (`spine_03.x`) out to the WIDEST shoulder flesh at
/// which to plant the glenohumeral joint (`upperarm_l/r`). Meshy plants the shoulder slightly too
/// MEDIAL — the same weak joint placement it does at the hip (Aaron 2026-07-22: "find the shoulders
/// the same way we find the pelvis, and fix the meshy rig") — so a clip's arm rotation swings from
/// too narrow a shoulder. WIDTH ONLY, like [`derive_hip_placement`]. TUNABLE — an eyeball call
/// against the render (raise → wider shoulders). Reference points: HumanBaseA's raw Meshy shoulder
/// sits at ~0.59 of its widest deltoid flesh; the human-proportioned oracle at ~0.63. Set to 0.70
/// so the idle hand clears the hip flesh (edge x≈17.1) by ~2.9 cm — bone-only clearance at 0.62 is
/// +1.3 cm, but the hand MESH hangs inboard of the bone, so ~0.70 keeps palm/fingers out of the hip
/// (measured by the `idle_pose_shoulder_effect` harness). Dial back toward 0.62 if shoulders read broad.
const SHOULDER_FRACTION: f32 = 0.70;

/// Correct the shoulder joints' WIDTH from the body's own mesh — a mesh-derived joint placement like
/// [`derive_hip_placement`] (Meshy is weak at the shoulder as at the hip). Moves `upperarm_l/r` x to
/// `SHOULDER_FRACTION` of the way from the midline to the widest clavicle/upperarm flesh (weight ≥
/// 0.5); keeps y/z, and every other bone keeps its world position, so the child locals absorb the
/// shift. Runs BEFORE [`reorient_to_canonical`] (which consumes rest positions).
pub fn derive_shoulder_placement(model: &mut RawModel) -> ShoulderReport {
    derive_shoulder_placement_frac(model, SHOULDER_FRACTION)
}

/// [`derive_shoulder_placement`] with an explicit fraction — the tuning seam (the idle-pose harness
/// sweeps this to pick `SHOULDER_FRACTION` against measured hip clearance).
fn derive_shoulder_placement_frac(model: &mut RawModel, fraction: f32) -> ShoulderReport {
    let idx: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    let (Some(&spine), Some(&ua_l), Some(&ua_r), Some(&cl_l), Some(&cl_r)) = (
        idx.get("spine_03"),
        idx.get("upperarm_l"),
        idx.get("upperarm_r"),
        idx.get("clavicle_l"),
        idx.get("clavicle_r"),
    ) else {
        return ShoulderReport::default();
    };

    let mut w = model_world_frames(model);
    let mid = pos_of(w[spine]).x;

    // Furthest shoulder flesh from the midline on `side`, from clavicle/upperarm-owned verts only
    // (weight ≥ 0.5) — the deltoid outer edge, the shoulder's analogue of the "widest hip flesh".
    let widest = |uarm: usize, clav: usize, sign: f32| -> Option<f32> {
        model
            .vertices
            .iter()
            .filter(|v| {
                (0..4).any(|k| {
                    (v.joints[k] as usize == uarm || v.joints[k] as usize == clav)
                        && v.weights[k] >= 0.5
                })
            })
            .map(|v| sign * (v.p[0] - mid))
            .filter(|d| *d > 0.0)
            .fold(None, |acc: Option<f32>, d| {
                Some(acc.map_or(d, |a| a.max(d)))
            })
    };

    let mut report = ShoulderReport::default();
    for (uarm, clav, sign, is_left) in [(ua_l, cl_l, 1.0f32, true), (ua_r, cl_r, -1.0f32, false)] {
        let Some(width) = widest(uarm, clav, sign) else {
            continue;
        };
        let cur = pos_of(w[uarm]).x;
        let tgt = mid + sign * fraction * width;
        // WIDTH only: x moves, y/z (and every other bone) untouched.
        let mut col = w[uarm].w_axis;
        col.x = tgt;
        w[uarm].w_axis = col;
        let entry = Some((cur, tgt, width));
        if is_left {
            report.left = entry
        } else {
            report.right = entry
        }
    }

    write_world_frames(&mut model.bones, &w);
    report
}

/// What `derive_ankle_placement` moved: per side, `(old_z, new_z)` of the ankle (`foot_l`) in cm.
#[derive(Debug, Clone, Default)]
pub struct AnkleReport {
    pub left: Option<(f32, f32)>,
    pub right: Option<(f32, f32)>,
}

/// Fraction from the ball up to Meshy's ankle at which to place the true ankle pivot. Meshy plants
/// `foot_l` too HIGH up the shin (Aaron 2026-07-21: "3/4 of the way down the shin"), so a foot bend
/// rotates the heel around a pivot ~7 cm above it → the heel drives into the floor. Lowering the
/// pivot toward the ankle flattens the foot toward the ~90° ankle bend Aaron described. TUNABLE — the
/// exact fraction is an eyeball call; refine against the render.
const ANKLE_FRACTION: f32 = 0.45;

/// Correct the ANKLE (`foot_l/r`) HEIGHT from the body's own mesh — a mesh-derived joint placement
/// like [`derive_hip_placement`] (Meshy is weak at both joints). Lowers the ankle pivot from Meshy's
/// too-high placement toward the true ankle (a fraction up from the ball), keeping x/y; every other
/// bone (incl. `ball_l`) keeps its world position, so the child locals absorb the shift and the foot
/// bone flattens toward horizontal. Runs BEFORE reorient (which consumes rest positions).
pub fn derive_ankle_placement(model: &mut RawModel) -> AnkleReport {
    let idx: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    let mut w = model_world_frames(model);
    let mut report = AnkleReport::default();
    for (foot_n, ball_n, is_left) in [("foot_l", "ball_l", true), ("foot_r", "ball_r", false)] {
        let (Some(&foot), Some(&ball)) = (idx.get(foot_n), idx.get(ball_n)) else {
            continue;
        };
        let old_z = pos_of(w[foot]).z;
        let ball_z = pos_of(w[ball]).z;
        // Lower the ankle to a fraction up from the ball toward Meshy's (too-high) ankle.
        let new_z = ball_z + ANKLE_FRACTION * (old_z - ball_z);
        if new_z >= old_z {
            continue; // never raise it
        }
        let mut col = w[foot].w_axis;
        col.z = new_z; // HEIGHT only: z moves, x/y untouched
        w[foot].w_axis = col;
        let entry = Some((old_z, new_z));
        if is_left {
            report.left = entry
        } else {
            report.right = entry
        }
    }
    write_world_frames(&mut model.bones, &w);
    report
}

/// How the reorient went.
#[derive(Debug, Clone, Default)]
pub struct ConformReport {
    pub limbs_aligned: usize,
}

/// Reorient `model`'s bones to conform to the reference skeleton (PrismHumanBaseA). Writes each
/// bone's new local TRS + inverse_bind in place.
pub fn reorient_to_canonical(model: &mut RawModel, reference: &Path) -> Result<ConformReport> {
    let (t, report) = canonical_world_frames(model, reference)?;
    // Derive local + inverse_bind from the final frames T (absolute retarget; no retarget_rot).
    write_world_frames(&mut model.bones, &t);
    Ok(report)
}

/// The canonical world frames this body's bones take under conform's reorient — the reference's
/// world ORIENTATION carried on THIS body's own joint POSITIONS, with each limb frame turned to
/// point down this body's own limb — computed WITHOUT writing them back into the model.
///
/// [`reorient_to_canonical`] writes these (the standard path). An AS-PROVIDED import keeps the
/// vendor's core frames untouched, yet [`infer_canonical_bones`] still needs this canonical BASIS to
/// hang the twists / fingers / face bones on: composing a canonical-basis offset directly onto a
/// vendor frame (a differing bone-axis convention) throws the inferred bones off the mesh. Because
/// reorient preserves POSITIONS, this basis shares the vendor rig's joint positions, so bones placed
/// on it land exactly where the canonical path would.
fn canonical_world_frames(
    model: &RawModel,
    reference: &Path,
) -> Result<(Vec<Mat4>, ConformReport)> {
    // This body's world frames (built from the parsed local TRS).
    let fg = model_world_frames(model);
    let idx: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();

    // The reference's world frames.
    let refs = load_reference_skeleton(reference)?;
    let ref_locals: Vec<Mat4> = refs.iter().map(|b| b.local).collect();
    let ref_parents: Vec<i32> = refs.iter().map(|b| b.parent).collect();
    let cg = fk(&ref_locals, &ref_parents);
    let cidx: HashMap<String, usize> = refs
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();

    // 1. base frames: reference world ORIENTATION + THIS body's positions (else this body's own).
    let mut g0 = vec![Mat4::IDENTITY; model.bones.len()];
    for (i, b) in model.bones.iter().enumerate() {
        let basis = match cidx.get(&b.name) {
            Some(&ci) => cg[ci],
            None => fg[i],
        };
        g0[i] = frame(basis, pos_of(fg[i]));
    }

    // 2. limb-align each limb frame (orientation only; position fixed): reference limb dir v → this u.
    let mut t = g0.clone();
    let mut report = ConformReport::default();
    for (i, b) in model.bones.iter().enumerate() {
        let Some(ch) = limb_child(&b.name) else {
            continue;
        };
        let (Some(&ref_bone), Some(&ref_ch)) = (cidx.get(&b.name), cidx.get(ch)) else {
            continue;
        };
        // This body's limb direction: to its child joint — or, for a hand whose fingers are not
        // inferred yet, onward along its own forearm. The canon's hand is collinear with its
        // forearm, so that is the canon's own rule on this body; it also makes the fingers infer
        // along THIS arm instead of the canon's world direction.
        let u = match idx.get(ch) {
            Some(&this_ch) => pos_of(g0[this_ch]) - pos_of(g0[i]),
            None if matches!(b.name.as_str(), "hand_l" | "hand_r") && b.parent >= 0 => {
                pos_of(g0[i]) - pos_of(g0[b.parent as usize])
            }
            None => continue,
        }
        .normalize_or_zero();
        let v = (pos_of(cg[ref_ch]) - pos_of(cg[ref_bone])).normalize_or_zero();
        if u.length_squared() < 1e-10 || v.length_squared() < 1e-10 {
            continue;
        }
        let align = Quat::from_rotation_arc(v, u); // minimal rotation v → u
        t[i] = rotated_frame(align, g0[i], pos_of(g0[i]));
        report.limbs_aligned += 1;
    }

    Ok((t, report))
}

/// This body's `limb` bone length ÷ the reference's — trusting Meshy's LENGTHS (memory 03BBF8F4).
fn limb_length_ratio(
    limb: &str,
    idx: &HashMap<String, usize>,
    g: &[Mat4],
    cidx: &HashMap<String, usize>,
    cg: &[Mat4],
) -> f32 {
    let Some(ch) = limb_child(limb) else {
        return 1.0;
    };
    let (Some(&li), Some(&ci), Some(&rli), Some(&rci)) =
        (idx.get(limb), idx.get(ch), cidx.get(limb), cidx.get(ch))
    else {
        return 1.0;
    };
    let refl = (pos_of(cg[rli]) - pos_of(cg[rci])).length();
    if refl > 1e-9 {
        (pos_of(g[li]) - pos_of(g[ci])).length() / refl
    } else {
        1.0
    }
}

/// What `infer_canonical_bones` added.
#[derive(Debug, Clone, Default)]
pub struct InferReport {
    pub added: Vec<String>,
    /// Bones REPARENTED onto the canonical chain (world frame preserved) — a source rig that
    /// lacked a canonical link parented straight past it (Meshy's one-neck rig hangs `head`
    /// off `neck_01`), and without the splice the inferred link dangles as a leaf and the
    /// shared clips' rotation for it is silently lost on this body (the 2026-08-20 golem's
    /// measured head jut).
    pub spliced: Vec<String>,
    pub hand_scale_l: f32,
    pub hand_scale_r: f32,
}

/// Add the canonical bones Meshy never produces — the in-app port of
/// `rename_meshy_to_canonical.py::infer_canonical_bones`. Whatever the reference (PrismHumanBaseA)
/// has and this body lacks (30 fingers + 8 twists + 2 weapon sockets + jaw/eyes) is inferred from the
/// reference's own local offset, hung off THIS body's canonical parent frame and scaled by this
/// body's limb-length ratio.
///
/// The parent frame it hangs off is the CANONICAL basis (reference orientation on this body's joint
/// positions). In [`ConformMode::Canonical`] the model was already reoriented, so its own frames ARE
/// that basis. In [`ConformMode::AsProvided`] the core keeps the vendor frames, so the canonical
/// basis is computed separately ([`canonical_world_frames`]) — otherwise the reference-basis offsets
/// would be rotated by the vendor's own bone-axis convention and land the inferred bones off the mesh
/// (the "inferred bones translated off the mesh" symptom). Each bone's LOCAL is expressed against its
/// ACTUAL parent frame, so FK reproduces the same world frame in both modes; a twist still lands at
/// the same FRACTION along this body's limb.
/// SCALE: Meshy gives no hand length, so fingers + sockets are sized by the FOREARM ratio ("a
/// straight hand of standard proportion"). Inferred bones carry NO weights — they are appended after
/// the mesh's joint indices are baked, so no vertex references them; they resolve and rotate but
/// deform nothing until each body's hand mesh is weighted to them (the follow-on).
pub fn infer_canonical_bones(
    model: &mut RawModel,
    reference: &Path,
    mode: ConformMode,
) -> Result<InferReport> {
    let refs = load_reference_skeleton(reference)?;
    let cg = fk(
        &refs.iter().map(|b| b.local).collect::<Vec<_>>(),
        &refs.iter().map(|b| b.parent).collect::<Vec<_>>(),
    );
    let cidx: HashMap<String, usize> = refs
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();

    // Initial (post-reorient) canonical frames + name→index; both grow as bones are appended.
    let g0 = model_world_frames(model);
    let idx0: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();

    // Fingers + weapon sockets hang off the hand; Meshy has no hand length, so size them by the
    // forearm (lowerarm→hand is that limb).
    let hand_scale_l = limb_length_ratio("lowerarm_l", &idx0, &g0, &cidx, &cg);
    let hand_scale_r = limb_length_ratio("lowerarm_r", &idx0, &g0, &cidx, &cg);

    // gap = reference bones this rig lacks, in the reference's topological order (parents precede
    // children). `root` is skipped here — it is synthesized at bake — as is any bone whose parent is
    // absent from this rig.
    let gap: Vec<usize> = refs
        .iter()
        .enumerate()
        .filter(|(_, b)| !idx0.contains_key(&b.name) && b.parent >= 0)
        .map(|(i, _)| i)
        .collect();

    // Per-bone scale, following the parent lineage (same precedence as the Blender tool).
    let mut scale: HashMap<String, f32> = HashMap::new();
    for &gi in &gap {
        let b = &refs[gi];
        let pnm = refs[b.parent as usize].name.as_str();
        let s = if let Some(&ps) = scale.get(pnm) {
            ps // chain continuing off an inferred bone (finger _02/_03)
        } else if pnm == "hand_l" {
            hand_scale_l // fingers + weapon socket, left
        } else if pnm == "hand_r" {
            hand_scale_r // fingers + weapon socket, right
        } else if limb_child(pnm).is_some() {
            limb_length_ratio(pnm, &idx0, &g0, &cidx, &cg) // twists: fraction along their own limb
        } else {
            1.0 // jaw/eyes off the head, etc.
        };
        scale.insert(b.name.clone(), s);
    }

    // Append each inferred bone on the CANONICAL basis, with its LOCAL taken against the ACTUAL
    // parent frame so FK reproduces the same world frame whether the core was reoriented (canonical)
    // or kept as provided (vendor frames): W = basis_parent · scaled_local, local = actual_parent⁻¹ · W.
    let mut g = g0;
    let mut basis = match mode {
        // Canonical: the model was already reoriented, so its own frames ARE the canonical basis.
        ConformMode::Canonical => g.clone(),
        // As provided: the core keeps the vendor frames — compute the canonical basis to hang on.
        ConformMode::AsProvided => canonical_world_frames(model, reference)?.0,
    };
    let mut idx = idx0;
    let mut report = InferReport {
        added: Vec::new(),
        spliced: Vec::new(),
        hand_scale_l,
        hand_scale_r,
    };
    for &gi in &gap {
        let b = &refs[gi];
        let pnm = refs[b.parent as usize].name.as_str();
        let Some(&pidx) = idx.get(pnm) else { continue };
        let s = *scale.get(b.name.as_str()).unwrap_or(&1.0);
        // Reference local with its translation scaled onto this body's limb.
        let mut new_l = b.local;
        new_l.w_axis = (new_l.w_axis.truncate() * s).extend(1.0);
        let w = basis[pidx] * new_l;
        let local = g[pidx].inverse() * w;
        let (sc, r, t) = local.to_scale_rotation_translation();
        model.bones.push(RawBone {
            name: b.name.clone(),
            parent: pidx as i32,
            translation: t.to_array(),
            rotation: r.to_array(),
            scale: sc.to_array(),
            inverse_bind: w.inverse().to_cols_array(),
        });
        idx.insert(b.name.clone(), model.bones.len() - 1);
        g.push(w);
        basis.push(w);
        report.added.push(b.name.clone());
    }

    report.spliced = splice_canonical_chain(model, reference)?;
    Ok(report)
}

/// Repair the CHAIN of an already-canonical model: reparent every canonical bone onto its
/// canonical parent — PRESERVING its world frame, so the rest pose does not move — then
/// re-sort parents-before-children and remap every parent index and vertex joint.
///
/// THE SPLICE (2026-08-20): a source rig that LACKED a canonical link parented straight past
/// it — Meshy's one-neck rig hangs `head` off `neck_01`, so the inferred `neck_02` dangled as
/// a childless leaf and every shared clip's neck_02 rotation was silently LOST on that body
/// (the head composed one link short of the canonical chain — the measured golem head jut).
/// Runs at the end of [`infer_canonical_bones`], and STANDALONE over a reloaded staged rig,
/// whose baked file may carry the pre-fix chain: the human's fitted joints stay exactly where
/// they were put, and only the chain composition is repaired. Returns the reparented names.
pub fn splice_canonical_chain(model: &mut RawModel, reference: &Path) -> Result<Vec<String>> {
    let refs = load_reference_skeleton(reference)?;
    let cidx: HashMap<String, usize> = refs
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    let idx: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    let g = model_world_frames(model);
    let mut spliced = Vec::new();
    for i in 0..model.bones.len() {
        let name = model.bones[i].name.clone();
        let Some(&ci) = cidx.get(&name) else { continue };
        let cparent = refs[ci].parent;
        // The reference's `root` is synthesized at bake, so a root-parented canonical bone
        // (pelvis) stays a model root here.
        let want: i32 = if cparent < 0 {
            -1
        } else {
            let pname = &refs[cparent as usize].name;
            if pname == "root" {
                -1
            } else {
                match idx.get(pname.as_str()) {
                    Some(&p) => p as i32,
                    None => continue, // canonical parent absent — leave as-is
                }
            }
        };
        if want == model.bones[i].parent {
            continue;
        }
        let local = match usize::try_from(want) {
            Ok(p) => g[p].inverse() * g[i],
            Err(_) => g[i],
        };
        let (sc, r, t) = local.to_scale_rotation_translation();
        let b = &mut model.bones[i];
        b.parent = want;
        b.translation = t.to_array();
        b.rotation = r.to_array();
        b.scale = sc.to_array();
        spliced.push(name);
    }

    // The splice can point a bone at a parent stored LATER in the vec (`head` at an appended
    // `neck_02`), and every world-frame walk in the pipeline is a single forward pass that
    // requires parents to precede children. Re-sort — canonical bones in the reference's own
    // topological order, everything else after in its original relative order (its parents are
    // canonical or preceded it before, so the invariant holds) — and remap every parent index
    // and vertex joint through the permutation.
    if !spliced.is_empty() {
        let n = model.bones.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| {
            match cidx.get(&model.bones[i].name) {
                Some(&ci) => (0, ci, i), // canonical: reference topological order
                None => (1, 0, i),       // stragglers: after, original relative order
            }
        });
        let mut perm = vec![0usize; n]; // old index → new index
        for (new_i, &old_i) in order.iter().enumerate() {
            perm[old_i] = new_i;
        }
        let mut bones = Vec::with_capacity(n);
        for &old_i in &order {
            let mut b = model.bones[old_i].clone();
            b.parent = match usize::try_from(b.parent) {
                Ok(p) => perm[p] as i32,
                Err(_) => -1,
            };
            bones.push(b);
        }
        model.bones = bones;
        for v in &mut model.vertices {
            for j in &mut v.joints {
                *j = perm[*j as usize] as u32;
            }
        }
        debug_assert!(
            model
                .bones
                .iter()
                .enumerate()
                .all(|(i, b)| b.parent < i as i32),
            "the splice re-sort must leave parents before children"
        );
    }
    Ok(spliced)
}

/// The full canonical conform, in order: mesh-derived hip WIDTH → limb-align reorient → infer the
/// missing bones. This is the whole port of `rename_meshy_to_canonical.py`; after it, the model's
/// bone world frames reproduce the reference (PrismHumanBaseA.json) for a body cut from the same
/// source. The synthesized `root` bone is a bake concern and is not added here.
#[derive(Debug, Clone, Default)]
pub struct ConformOutput {
    pub hip: HipReport,
    pub shoulder: ShoulderReport,
    pub ankle: AnkleReport,
    pub reorient: ConformReport,
    pub infer: InferReport,
}

/// Run the full conform against `reference` (use [`default_reference`] for PrismHumanBaseA).
/// How [`conform_to_canonical`] treats the vendor's rig.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConformMode {
    /// Derive the joint widths, reorient every bone onto the canonical reference frame, then
    /// complete the bone set — the standard path that makes a vendor rig drive the shared clips.
    #[default]
    Canonical,
    /// Keep the vendor rig EXACTLY as provided: skip the hip/shoulder/ankle width derivation AND
    /// [`reorient_to_canonical`], so every vendor bone keeps its own position and rest frame. Only
    /// the bone set is completed ([`infer_canonical_bones`]) so the shared clips have targets — and
    /// the inferred fill-ins carry no weights, so the vendor's own bones alone drive the mesh. The
    /// diagnostic path: stage a vendor rig untouched to see whether it already animates cleanly
    /// against the shared clips, rather than assuming it needs the correction.
    AsProvided,
}

pub fn conform_to_canonical(
    model: &mut RawModel,
    reference: &Path,
    mode: ConformMode,
) -> Result<ConformOutput> {
    match mode {
        ConformMode::Canonical => {
            let hip = derive_hip_placement(model);
            let shoulder = derive_shoulder_placement(model);
            let ankle = derive_ankle_placement(model);
            let reorient = reorient_to_canonical(model, reference)?;
            let infer = infer_canonical_bones(model, reference, ConformMode::Canonical)?;
            Ok(ConformOutput {
                hip,
                shoulder,
                ankle,
                reorient,
                infer,
            })
        }
        // As provided: no derive passes, no reorient — every vendor bone keeps its position and
        // frame. Only the bone set is completed so the shared clips resolve their targets.
        ConformMode::AsProvided => Ok(ConformOutput {
            infer: infer_canonical_bones(model, reference, ConformMode::AsProvided)?,
            ..Default::default()
        }),
    }
}

/// The canonical reference rig — the **Humanoid** pattern reference (historically GolemBaseSkeleton), the AUTHORED baseline
/// (Aaron's ruling, 2026-08-04): a generated, skeleton-only A-pose at 170 cm — see
/// [`crate::baseline`]. The reference is nobody's body: characters (the golem
/// included) are conformed ONTO it, which retires the Katanami-derived bind lineage
/// for good. "Canonical" = the 66 names + parent topology + conventions (Z-up, cm,
/// root at feet) + THIS authored rest bind; body proportions stay per-rig by design.
pub fn default_reference() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content/package/skeletons/Humanoid/Humanoid.json")
}

/// RIG A RAW (skeleton-less) MESH — the boneless character path, ONE sequence shared by the
/// Clayworks conform stage and the headless import ([`crate::import_folder`]): size the mesh to
/// `stature_cm`, rough-fit the authored canon's limbs to it (torso untouched), then skin. The skin
/// is baked TWICE on purpose: a rough pass first, so the hip fit reads flesh by OWNERSHIP — a bare
/// Z-band at hip height catches the A-posed hands (they hang there), but the weight test excludes
/// them because they belong to the hand bones — then a re-skin from the fitted hips. The bind is
/// the authored canon by construction (uniform stature scale, no mesh-fitted torso; invariant
/// B51DE4CB). Run on the PREPPED geometry (collapsed + sized), never the raw source.
///
/// HANDS BACK THE FIT'S OWN [`FitReport`] — what matched what, and the [`Body`] the fit read. The
/// body is read ONCE here: both skin passes bind on it (a hip moved by ownership moves bones, never
/// the mesh), the import squares its stance on it, and the bench's rail reads the match the fit
/// acted on instead of thinning the mesh again to re-derive it (0F0208AC).
pub fn rig_raw_mesh(
    model: &mut RawModel,
    stature_cm: f32,
    recipe: &SkeletonRecipe,
) -> anyhow::Result<FitReport> {
    scale_mesh_to_stature(model, stature_cm);
    let fit = fit_baseline_to_mesh(model, stature_cm, recipe)?;
    crate::bake::bind(model, fit.body.as_ref());
    // Hips the leg fit measured off the leg tubes stand; otherwise the rough skin's weight
    // ownership widens them (an arm hanging against the hip owns the hip's outer flesh, and
    // ownership then pulled a fitted hip 7 cm inboard — the lizard, 2026-09-07).
    if !(fit.legs || fit.hand_placed) {
        let _ = derive_hip_placement(model);
        crate::bake::bind(model, fit.body.as_ref());
    }
    Ok(fit)
}

/// A mesh's own bounding box (cm) — `(MAX, MIN)` when it has no vertices, which every caller
/// guards against before it divides by an extent.
pub(crate) fn bbox(model: &RawModel) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &model.vertices {
        let p = Vec3::from_array(v.p);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (lo, hi)
}

/// What [`scale_mesh_to_stature`] did: the uniform factor and the source/target heights (cm).
#[derive(Debug, Clone, Copy, Default)]
pub struct ScaleReport {
    pub scale: f32,
    pub source_height: f32,
    pub stature: f32,
}

/// Uniformly resize a mesh so its bounding height (Z-up) equals `stature_cm`, ground it on the
/// floor (min-Z → 0) and plant it on the plumb line (bbox centre X/Y → 0) — the frame the
/// authored canon lives in. Raw Meshy meshes arrive with no meaningful scale, so this is what
/// makes a hi-res mesh and the stature-scaled canon co-located and rig-able.
///
/// Normals are untouched (a uniform positive scale preserves their direction). Idempotent up to
/// the measured height; a degenerate (flat) mesh is left alone.
pub fn scale_mesh_to_stature(model: &mut RawModel, stature_cm: f32) -> ScaleReport {
    if model.vertices.is_empty() || stature_cm <= 0.0 {
        return ScaleReport::default();
    }
    let (lo, hi) = bbox(model);
    let height = hi.z - lo.z;
    if height <= 1e-6 {
        return ScaleReport::default();
    }
    let s = stature_cm / height;
    let cx = (lo.x + hi.x) * 0.5;
    let cy = (lo.y + hi.y) * 0.5;
    for v in &mut model.vertices {
        v.p = [(v.p[0] - cx) * s, (v.p[1] - cy) * s, (v.p[2] - lo.z) * s];
    }
    // A body that already carries a SKELETON (a re-opened staged rig, Aaron 2026-09-07: "when
    // you change it on the prep screen it never picks it up") resizes as one thing: every
    // joint takes the same map the vertices took, and the binds follow, so the rest skin stays
    // the identity and the rig survives the resize.
    if !model.bones.is_empty() {
        let map = |p: Vec3| Vec3::new((p.x - cx) * s, (p.y - cy) * s, (p.z - lo.z) * s);
        let world: Vec<Mat4> = model_world_frames(model)
            .into_iter()
            .map(|w| {
                let mut m = w;
                m.w_axis = map(w.w_axis.truncate()).extend(1.0);
                m
            })
            .collect();
        write_world_frames(&mut model.bones, &world);
        for (b, w) in model.bones.iter_mut().zip(&world) {
            b.inverse_bind = w.inverse().to_cols_array();
        }
    }
    ScaleReport {
        scale: s,
        source_height: height,
        stature: stature_cm,
    }
}

/// THE MEASURED FACING of a raw source: the yaw about the vertical Z, in degrees, that lays the
/// BODY'S OWN long horizontal axis ([`crate::flesh::Core::axis_deg`]) onto the rig's −Y forward.
/// `0.0` when there is no body to measure, or when the body is not LYING DOWN — an upright mesh
/// (the humans: a torso is taller than it is long) is already facing the rig and must not turn.
///
/// THE ONE facing measurement (decision 69F4B20D), replacing the BOUNDING-BOX guess the bench
/// used to make privately — "a quarter-turn when the longest dimension is X". Swept over the
/// whole hoofed family (2026-09-21) that guess is wrong twice over:
/// - a bull's HORNS, an elk's ANTLERS and a ewe's WOOL are wider and taller than the animal
///   inside them, so the box says the body lies the wrong way, or does not lie down at all;
/// - SEVEN of the seventeen sources are yawed 30–55° off the axis to begin with (Meshy does not
///   promise a square side profile), and no whole number of quarter-turns squares those.
///
/// This is an AXIS, not a heading: it never decides which END is the head. A body that comes out
/// tail-first is the human's `quarters` (the bench's Turn 90°, `--facing`) to flip, exactly as
/// before.
pub fn measure_facing(flesh: &Flesh) -> f32 {
    let Some(core) = flesh.core() else {
        return 0.0;
    };
    // IS THE BODY LYING DOWN? A quadruped's barrel is far longer than it is tall (the Horse's is
    // 118 cm along by 47 high); a standing biped's trunk is the other way about (a shoulder span
    // under a torso's height). Only a lying body is turned, so the humanoids never move.
    if core.along <= core.hi.z - core.lo.z {
        return 0.0;
    }
    // Onto −Y: the smaller of the two turns that lands the axis on the Y line, so a body already
    // square stays put and one yawed a little is nudged, never spun half round.
    let yaw = 90.0 - core.axis_deg;
    if yaw > 90.0 {
        yaw - 180.0
    } else {
        yaw
    }
}

/// `q` quarter-turns about the vertical Z axis as one exact matrix ([`crate::quarter_turn`]
/// composed), so repeatedly turning never drifts the geometry off square.
pub fn z_quarter_turns(q: u8) -> Mat4 {
    let mut m = Mat4::IDENTITY;
    for _ in 0..(q % 4) {
        m = crate::fbx::quarter_turn(2) * m;
    }
    m
}

/// TURN A RAW MESH ONTO THE RIG — the ONE apply, shared by the bench's Prep and the headless
/// [`crate::import_folder`]: the measured `yaw_deg` ([`measure_facing`]) that squares the body
/// onto the rig's forward, and `quarters` × 90° of the human's own on top of it, as one rotation
/// about Z through [`crate::apply_orientation`] (verts + normals, and any root bones).
///
/// A square body the human has not turned (`yaw_deg == 0`, `quarters` a multiple of four) is not
/// touched at all, and the quarter-turns alone stay the EXACT integer matrix they always were, so
/// four of them still return a mesh bit-for-bit.
pub fn face_to_rig(model: &mut RawModel, yaw_deg: f32, quarters: u8) {
    let turned = !quarters.is_multiple_of(4);
    if !turned && yaw_deg == 0.0 {
        return;
    }
    let q = if turned {
        z_quarter_turns(quarters)
    } else {
        Mat4::IDENTITY
    };
    let r = if yaw_deg == 0.0 {
        q
    } else {
        Mat4::from_rotation_z(yaw_deg.to_radians()) * q
    };
    crate::fbx::apply_orientation(model, r);
}

/// Install the authored canonical skeleton onto a mesh that has NONE — the raw-mesh rig path
/// (Aaron 2026-08-22): the HUMANOID recipe through [`install_skeleton`].
pub fn install_baseline_skeleton(model: &mut RawModel, stature_cm: f32) {
    install_skeleton(model, &SkeletonRecipe::humanoid(), stature_cm)
        .expect("the humanoid recipe composes");
}

/// Install a COMPOSED recipe (the modular skeleton system, 2026-09-07) onto a mesh that has
/// no skeleton: `baseline::compose` at `stature_cm`, planted at the canonical origin (feet on
/// the ground, plumb), so the bind IS the recipe's authored rest by construction (invariant
/// BIND == AUTHORED CANON, generalised to the recipe) — no mesh-fitting here, no
/// `pose_mesh_to_canon` transport (that mesh-warp path was rolled back).
///
/// Emits the `RawModel` convention conform produces (`root` EXCLUDED — `bake_rig` synthesizes
/// it and shifts +1; the first trunk's `pelvis` has parent `-1`). Pair with a prior
/// [`scale_mesh_to_stature`] at the same stature so mesh and skeleton co-locate, then
/// [`crate::bake::bake_skin`] derives weights from these bones. A recipe with a variant that
/// is not authored yet is an error, never a guess.
pub fn install_skeleton(
    model: &mut RawModel,
    recipe: &SkeletonRecipe,
    stature_cm: f32,
) -> anyhow::Result<()> {
    let authored = crate::baseline::compose(recipe, stature_cm)?;
    let out_index: HashMap<&str, usize> = authored
        .iter()
        .filter(|b| b.name != "root")
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let world: HashMap<&str, Vec3> = authored
        .iter()
        .map(|b| (b.name.as_str(), b.position))
        .collect();
    model.bones = authored
        .iter()
        .filter(|b| b.name != "root")
        .map(|b| {
            let (parent_idx, parent_world) = match out_index.get(b.parent.as_str()) {
                Some(&pi) => (pi as i32, world[b.parent.as_str()]),
                None => (-1, Vec3::ZERO), // hung from the root: world == local
            };
            RawBone {
                name: b.name.clone(),
                parent: parent_idx,
                translation: (b.position - parent_world).to_array(),
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                inverse_bind: Mat4::from_translation(-b.position).to_cols_array(),
            }
        })
        .collect();
    Ok(())
}

/// ROUGH-FIT the installed canon to a raw mesh's own geometry — the raw-mesh rig starting template
/// (Aaron 2026-08-22, "rough auto-template + manual"). A rig-less mesh has no joint positions, so the
/// bare canon lands generic (stocky shoulders, a wide bird A-pose that fits nothing). This measures
/// the mesh and pulls the LIMB joints onto it, so the human's follow-up joint-drag is a nudge, not a
/// rebuild. It NEVER touches the torso chain (pelvis/spine/neck/head stay plumb) — posture is the
/// clip's job, and bending the body is the logged fundamental dead-end.
///
/// Fits the ARMS from four mesh landmarks per side: the shoulder (`SHOULDER_FRACTION` of the
/// shoulder-band flesh), the FINGERTIP (the outboard vertex farthest from that shoulder — on an
/// A-posed mesh the hand is the arm's far end, wherever it hangs), the ELBOW (the arm tube's
/// centreline at the canon's elbow fraction of the reach) and the WRIST (the tube's thinnest
/// slice on the way to the fingertip). Three chained 3-D similarities (rotation + scale) lay the
/// canon's shoulder→elbow, elbow→wrist and wrist→fingertip chains along the mesh's, so a bent
/// elbow and a hand that hangs forward are followed and every arm bone runs INSIDE the arm —
/// GolemBaseV2, 2026-09-07: on the straight shoulder→wrist line the forearm bone ran 4.8 cm from
/// the centre of a 3.5 cm-thick forearm, outside the skin, where the skin bake's normal test
/// rightly refused it and bound that forearm to the hand and fingers 12 cm away; before that the
/// wrist itself sat on the fingertip and the finger bones hung in the air past the hand.
/// A sparse mesh with no measurable elbow or wrist puts them at the canon's fractions of the reach.
///
/// Then the TAIL is TRACED and the LEGS are laid down their tubes, both off ONE voxel [`Flesh`]
/// field of the mesh built here and shared: the tail rides [`Flesh::trace`], and each leg's knee,
/// ankle and ball are the NARROWINGS of its tube ([`fit_leg`]) — never a bare Z-band at hip
/// height, which catches the A-posed HANDS and shoved the legs out to the wrists. Legs that find
/// no tube leave the composed rest and the caller fits the hips by weight OWNERSHIP with
/// [`derive_hip_placement`] after a rough skin instead ([`rig_raw_mesh`]).
/// Requires the mesh already scaled to `stature_cm` (grounded, x-centred) — call after
/// [`scale_mesh_to_stature`].
/// What [`fit_baseline_to_mesh`] measured off the mesh (the rest kept its composed rest).
#[derive(Debug, Default)]
pub struct FitReport {
    /// Both legs were laid down their leg tubes — the hips are the tubes' own centres.
    pub legs: bool,
    /// Every fit stayed OFF: the composed rest is the whole placement, for the human to move
    /// (a QUADRUPED trunk — the humanoid landmarks mean nothing on it; spec C658F114).
    pub hand_placed: bool,
    /// Where the trunk went — `None` when the mesh had no measurable core and the composed rest
    /// was left exactly where it stood.
    pub align: Option<AlignReport>,
    /// WHAT MATCHED WHAT (spec 04803E0C): which module took which piece of the mesh's shape, and
    /// which modules and which pieces found no partner. `None` when the mesh had no shape graph
    /// and the fit fell back to [`align_trunk`].
    pub shape: Option<ShapeMatch>,
    /// THE BODY THE FIT READ — this mesh's flesh and the graph thinned from it, the one read every
    /// later question about the mesh is asked of (the skin bind, the stance normaliser's feet).
    /// `None` only for a mesh too small to read at all.
    pub body: Option<Body>,
}

/// What [`align_trunk`] measured off the mesh's trunk, and where it put the composed rest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignReport {
    /// The body's symmetry plane (cm): the rig's midline went here.
    pub plane_x: f32,
    /// Where the first trunk's `pelvis` landed (world cm).
    pub pelvis: Vec3,
    /// A QUADRUPED's withers (`spine_03`); `None` on a plumb trunk.
    pub withers: Option<Vec3>,
    /// The stature the rest was composed at (cm). A biped keeps the TYPED value; a quadruped's
    /// is re-measured off its own withers, because a horse's typed height includes the head.
    pub stature: f32,
    /// A QUADRUPED's re-measured `TrunkSpec.length` knob; `None` on a plumb trunk.
    pub length: Option<f32>,
}

/// A quadruped's BELLY LINE is the lowest midline run along the body that spans at least this
/// much of the trunk core's own length — below it the midline crosses only the tail or nothing,
/// above it the run grows into the chest and the hindquarters.
const QUAD_BELLY: f32 = 0.6;
/// How far in front of the rump the `pelvis` sits, as a fraction of the stature — the croup is a
/// joint inside the hindquarters, not the skin at the back of them.
const PELVIS_MARGIN: f32 = 0.05;
/// The back line is read this far inboard of each end of the belly run: exactly at an end the
/// column is the body's own edge, one cell of flesh thick.
const END_INSET: f32 = 0.05;

/// ALIGN THE COMPOSED REST ONTO THE MESH'S OWN TRUNK — the first thing a fit does, before any
/// limb is touched and before the animals' early return (incident D81498B7: the rest "has never
/// been close" — a horse's withers floated above its back and its pelvis sat mid-belly, because
/// nothing measured the mesh's TRUNK before composing; [`scale_mesh_to_stature`] only makes the
/// bounding BOX match, and a horse's box height is its head, not its withers).
///
/// Everything is read off the body-masked [`Flesh`] (`build_body`, so a tail-hair swamp is not
/// measured as flesh) — never the bounding box:
/// - THE SYMMETRY PLANE is the trunk [`Core`]'s own x centre. The rig's midline goes there, so a
///   body modelled off-centre stops dragging the whole rig sideways.
/// - A QUADRUPED's CHEST (−Y) and RUMP (+Y) are the trunk core's own ends — the barrel is exactly
///   the span `TrunkSpec.length` measures, pelvis to shoulder socket. Its BELLY LINE is the lowest
///   height at which the midline column along the body carries one solid run spanning
///   [`QUAD_BELLY`] of that span, its midpoint inside the core (so a hanging tail is never
///   mistaken for a belly): that height says which run in a vertical column is the BODY. The
///   CROUP is then the top of the body's own column just inboard of the rump. The knobs come off
///   those two numbers: the stature is the croup over [`baseline::QUAD_WITHERS`] and
///   `TrunkSpec.length` is what is left between the chest and a pelvis [`PELVIS_MARGIN`] in front
///   of the rump. Re-composed at those knobs (the recipe's other modules untouched), the rest
///   needs only an X/Y shift — every height is already right, which is what keeps the hooves on
///   the ground (the leg modules are authored from a socket at a fixed fraction of the stature).
///
///   THE CROUP, NOT THE WITHERS, IS THE HEIGHT THAT CAN BE MEASURED. A quadruped's withers skin is
///   unreachable: the neck is contiguous with the back, so the top of the column at the chest end
///   is the crest of a raised neck (on the real Horse, 156 cm where the back is 122). Nothing
///   rises above the rump — the head is forward and the tail behind and below — and a level
///   topline (croup skin at the withers height) is the norm the whole family is bred and modelled
///   to, so dividing the croup by [`baseline::QUAD_WITHERS`] gives the stature AND drops the croup
///   JOINT the canon's 0.04·h under the skin, where a vertebra belongs.
/// - A BIPED's CROTCH is the lowest height at which the body's own middle column is solid and what
///   was TWO runs across the body (the two legs) has become ONE. The `pelvis` sits
///   [`baseline::BIPED_PELVIS_OVER_CROTCH`] of the stature above it. The stature STAYS the typed
///   value — a biped's height IS its height, and the humanoid canon is bit-exact at its own knob.
///
/// THE MESH IS NEVER RESCALED HERE: the typed height stays the animal's overall height and the
/// RIG adapts to the mesh. A mesh with no measurable core (tiny, hollow, or a quadruped with no
/// belly run) is LEFT EXACTLY AS COMPOSED with a warning — never moved somewhere invented.
/// THE CROTCH OF AN UPRIGHT BODY — the top of the GAP BETWEEN ITS LEGS, and the one height a
/// standing body's own shape states. Walking up the SYMMETRY PLANE, it is the lowest height at
/// which the body is solid ON the plane after having been hollow on it: below the crotch the
/// plane runs between the legs and is empty, above it the legs have merged into the trunk. Read
/// on the plane and nowhere else, because the plane is the one place an A-posed arm never reaches
/// — counting runs across the body instead put the elf's pelvis 53 cm high, on the first height
/// above its own hanging hands. The run must belong to the trunk's own span in Y (`y_lo`..`y_hi`),
/// so a tail hanging behind the legs is not a crotch, and there must be A PAIR OF LEGS UNDER IT.
///
/// `None` when the gap was never there — a body solid on the plane all the way down (a plain box,
/// a robe, a plinth) has no crotch, and a caller must leave its rest where it was composed rather
/// than drop it to the floor (4BB12A75).
///
/// ONE READER, TWO CALLERS (the shape fit and the [`align_trunk`] fallback), for the reason the
/// facing has one ([`measure_facing`], 3A61D440): a second copy is a second answer. The shape
/// graph cannot supply this itself — the thick flesh of a standing body runs on DOWN INSIDE its
/// thighs, so its core's bottom end and its hip pair's attachments both sit BELOW where the legs
/// part (measured on all seven promoted humanoids: the pelvis composed the canon's own distance
/// above either one lands 18 cm low, in the gap between the legs and outside the flesh).
fn crotch_on_the_plane(
    flesh: &Flesh,
    plane_x: f32,
    y_lo: f32,
    y_hi: f32,
    floor: f32,
    ceiling: f32,
) -> Option<f32> {
    let step = flesh.cell();
    let n = ((ceiling - floor) / step).ceil().max(0.0) as usize;
    let mut gap = false;
    (0..=n)
        .map(|i| floor + (i as f32 + 0.5) * step)
        .find_map(|z| {
            let body = flesh
                .runs(Vec3::new(plane_x, 0.0, z), 1)
                .into_iter()
                .find(|r| {
                    let mid = 0.5 * (r.0 + r.1);
                    mid >= y_lo && mid <= y_hi
                });
            gap |= body.is_none();
            let run = body.filter(|_| gap)?;
            // A PAIR OF LEGS UNDER IT: just below, the body must lie on BOTH sides of the plane with
            // the plane itself between them. One tube through the plane is a limb fixture, not a
            // standing body, and its own middle is not a crotch.
            let across = flesh.runs(Vec3::new(0.0, 0.5 * (run.0 + run.1), z - step), 0);
            (across.iter().any(|r| r.1 <= plane_x) && across.iter().any(|r| r.0 >= plane_x))
                .then_some(z)
        })
}

pub fn align_trunk(
    model: &mut RawModel,
    flesh: &Flesh,
    recipe: &SkeletonRecipe,
    stature_cm: f32,
) -> anyhow::Result<Option<AlignReport>> {
    let Some(core) = flesh.core() else {
        tracing::warn!("align_trunk: no trunk core in this mesh — the composed rest stands");
        return Ok(None);
    };
    // A "core" a cell or two thick is a SHEET or a point cloud, not a trunk: every reading below
    // would be voxel noise dressed up as anatomy. Leave the rest where it was composed.
    if core.radius < 2.0 * flesh.cell() {
        tracing::warn!(
            radius = core.radius,
            cell = flesh.cell(),
            "align_trunk: this mesh has no body to measure — the composed rest stands"
        );
        return Ok(None);
    }
    let (lo, hi) = bbox(model);
    let plane_x = core.plane_x;
    // The body walked a cell at a time from the ground up — the one height each rule below reads.
    let step = flesh.cell();
    let heights = || {
        let n = ((hi.z - lo.z) / step).ceil().max(0.0) as usize;
        (0..=n).map(move |i| lo.z + (i as f32 + 0.5) * step)
    };

    let quadruped = recipe.trunk.orientation == flicker_skeletal::format::Orientation::Quadruped;
    let (stature, length, target) = if quadruped {
        let rump_y = core.hi.y;
        let min = QUAD_BELLY * (rump_y - core.lo.y);
        let belly = heights().find_map(|z| {
            flesh
                .runs(Vec3::new(plane_x, 0.0, z), 1)
                .into_iter()
                .find(|r| {
                    let mid = 0.5 * (r.0 + r.1);
                    r.1 - r.0 >= min && mid >= core.lo.y && mid <= rump_y
                })
                .map(|r| (z, r))
        });
        let Some((belly_z, belly)) = belly else {
            tracing::warn!(
                "align_trunk: no belly run across this quadruped's core — the composed rest stands"
            );
            return Ok(None);
        };
        // THE CHEST IS THE GIRTH — the FRONT END OF THE BELLY RUN, not the front of the core.
        // The core is the flesh at least half as thick as the body's thickest, and on an animal
        // whose head and neck are as thick as its barrel (the Hippopotamus, the Rhinoceros, the
        // Pig, the Boar — 2026-09-21 sweep) that reaches all the way to the SNOUT: the core's
        // front ran 45 to 55 cm ahead of the shoulder and the withers were composed inside the
        // nose, outside the flesh. The belly run ends where the forelegs come down, which is the
        // girth — within a centimetre of the barrel's front on the Horse (−56.6 against −55.7),
        // and behind the head on every animal, because a head is not something you can walk a
        // midline run along at belly height. Never farther back than the core's front, so a short
        // belly run (the Elephant's columnar legs stand well inboard) cannot shorten the barrel.
        let chest_y = belly.0.max(core.lo.y);
        // The BACK over the rump: the top of the solid column that the CORE ITSELF runs through,
        // so a mane or an ear floating over the back is never read as the back. Sampled a little
        // inboard of the rump, where the column is the body and not its own last cell of skin.
        // (The belly line is the body's LOWEST run and rises towards the haunches — it is what
        // says the trunk is a barrel, not what picks the column out of a vertical stack.)
        let core_z = 0.5 * (core.lo.z + core.hi.z);
        let Some(croup_z) = flesh
            .runs(
                Vec3::new(plane_x, rump_y - END_INSET * (rump_y - core.lo.y), 0.0),
                2,
            )
            .into_iter()
            .find(|r| r.0 <= core_z && r.1 >= core_z)
            .map(|r| r.1)
        else {
            tracing::warn!("align_trunk: no back line over this quadruped's belly run");
            return Ok(None);
        };
        let stature = croup_z / crate::baseline::QUAD_WITHERS;
        anyhow::ensure!(
            stature > 1.0,
            "a back {croup_z:.1} cm off the ground is not a body to rig"
        );
        let pelvis_y = rump_y - PELVIS_MARGIN * stature;
        let length = ((pelvis_y - chest_y) / stature).clamp(0.2, 2.0);
        tracing::debug!(
            plane_x,
            belly_z,
            chest_y,
            rump_y,
            croup_z,
            stature,
            length,
            "align_trunk: quadruped trunk measured"
        );
        (stature, Some(length), Vec3::new(plane_x, pelvis_y, 0.0))
    } else {
        // THE CROTCH is the top of the GAP BETWEEN THE LEGS: walking up the SYMMETRY PLANE, the
        // lowest height at which the body itself is solid on it. Below the crotch that plane runs
        // between the legs and is empty; above it the legs have merged into the trunk. Read on
        // the plane and nowhere else, because the plane is the one place an A-posed arm never
        // reaches — counting runs across the body instead put the elf's pelvis 53 cm high, on the
        // first height above its own hanging hands. The run must belong to the CORE's own span,
        // so a tail hanging behind the legs is not a crotch, and the gap must actually have been
        // there: a body solid on the plane all the way down (a plain box, a robe, a plinth) has no
        // crotch to read and its rest stays where it was composed rather than dropping to the
        // floor (4BB12A75).
        let crotch = crotch_on_the_plane(flesh, plane_x, core.lo.y, core.hi.y, lo.z, hi.z);
        let Some(crotch_z) = crotch else {
            tracing::warn!("align_trunk: no crotch under this trunk — the composed rest stands");
            return Ok(None);
        };
        let z = crotch_z + crate::baseline::BIPED_PELVIS_OVER_CROTCH * stature_cm;
        tracing::debug!(
            plane_x,
            crotch_z,
            pelvis_z = z,
            "align_trunk: biped trunk measured"
        );
        (stature_cm, None, Vec3::new(plane_x, 0.0, z))
    };

    // Re-compose at the MEASURED knobs (a biped's are the typed ones, so its canon stays
    // bit-exact), then shift the whole rest onto the trunk: X and Y for a quadruped, X and Z
    // for a biped — the axis each rule actually measured.
    if quadruped {
        let mut fitted = recipe.clone();
        fitted.trunk.length = length.expect("a quadruped measured its length");
        install_skeleton(model, &fitted, stature)?;
    }
    let mut w = model_world_frames(model);
    let Some(pelvis) = model
        .bones
        .iter()
        .position(|b| b.name == "pelvis")
        .map(|i| pos_of(w[i]))
    else {
        return Ok(None); // a recipe with no pelvis has no trunk to align
    };
    let shift = Vec3::new(
        target.x - pelvis.x,
        if quadruped { target.y - pelvis.y } else { 0.0 },
        if quadruped { 0.0 } else { target.z - pelvis.z },
    );
    for m in &mut w {
        m.w_axis += shift.extend(0.0);
    }
    write_world_frames(&mut model.bones, &w);
    let at = |name: &str| {
        model
            .bones
            .iter()
            .position(|b| b.name == name)
            .map(|i| pos_of(w[i]))
    };
    Ok(Some(AlignReport {
        plane_x,
        pelvis: pelvis + shift,
        withers: quadruped.then(|| at("spine_03")).flatten(),
        stature,
        length,
    }))
}

/// FIT THE COMPOSED REST TO THIS MESH — one [`Flesh`] field, one [`ShapeGraph`] off it, and every
/// module of the recipe placed on the piece of that graph it MATCHED (spec 04803E0C, Aaron's law
/// 513E5F78). The same call for every pattern: a humanoid, a horse, a bird and a seven-trunk box
/// monster differ only in what their recipes ask for and what their shapes answer with.
///
/// What this replaced, and why (rule 98232A50 — a caller left on the old path is a defect):
/// `fit_leg` found a leg by a canon name and a height band, `arm_reach` found a hand by a Z-band
/// and an outboard-most fifth, `ArmProfile` sliced a tube it assumed ran shoulder-to-fingertip,
/// and `fit_tail` traced a tube from wherever the composed chain happened to start. All four read
/// ANATOMY. The matched limbs carry the same reads — the same [`narrowings`], the same ground
/// joint at the tube's end — on the segment the MATCH chose, which is why they work on a body
/// whose proportions the canon never anticipated.
pub fn fit_baseline_to_mesh(
    model: &mut RawModel,
    stature_cm: f32,
    recipe: &SkeletonRecipe,
) -> anyhow::Result<FitReport> {
    install_skeleton(model, recipe, stature_cm)?;
    if model.vertices.len() < 4 || stature_cm <= 0.0 {
        return Ok(FitReport::default());
    }
    // ONE voxel FLESH field for the whole fit (spec 76EB9552) and ONE shape graph read off it:
    // both are mesh-wide passes, never one per limb — and never one per stage either, so the read
    // goes back to the caller with the report.
    let body = Body::read(model);
    if let Some(graph) = &body.graph {
        if let Some((m, align)) = fit_to_graph(model, &body.flesh, graph, recipe, stature_cm)? {
            for warn in &m.warnings {
                tracing::warn!("fit: {warn}");
            }
            for spare in &m.spare {
                tracing::info!("fit: {spare}");
            }
            let legs = matches!(
                m.of(&crate::baseline::module_id("leg", "")),
                Some(Matched::Pair(_))
            );
            // The human still has joints to place whenever a module found no partner — that list
            // IS the rail's opening order ([`ShapeMatch::marker_order`]).
            let hand_placed = !m.unmatched.is_empty();
            return Ok(FitReport {
                legs,
                hand_placed,
                align: Some(align),
                shape: Some(m),
                body: Some(body),
            });
        }
    }
    // NO GRAPH, OR NO CORE IN IT — a tiny, hollow or sheet-thin mesh. The trunk alignment stays
    // as the fallback it now is: it still finds a symmetry plane and a crotch or a croup where
    // the graph found no structure at all, and where even that fails the composed rest stands
    // exactly where it was (4BB12A75).
    tracing::warn!("fit: no shape graph for this mesh — falling back to the trunk alignment");
    let align = align_trunk(model, &body.flesh, recipe, stature_cm)?;
    Ok(FitReport {
        align,
        hand_placed: true,
        body: Some(body),
        ..Default::default()
    })
}

/// The cumulative arc length at each point of a traced or profiled centreline (`0` at its first
/// point) — what lets a chain of bones sit at even, or proportional, distances along it.
fn arc_lengths(path: &[(Vec3, f32)]) -> Vec<f32> {
    let mut cum = vec![0.0_f32];
    for pair in path.windows(2) {
        cum.push(cum.last().copied().unwrap_or(0.0) + pair[0].0.distance(pair[1].0));
    }
    cum
}

/// The point `s` cm along a centreline (its `cum` from [`arc_lengths`]), clamped to its ends.
fn along_path(path: &[(Vec3, f32)], cum: &[f32], s: f32) -> Vec3 {
    if path.len() < 2 {
        return path.first().map_or(Vec3::ZERO, |p| p.0);
    }
    let seg = cum
        .iter()
        .rposition(|&d| d <= s)
        .unwrap_or(0)
        .min(path.len() - 2);
    let f = ((s - cum[seg]) / (cum[seg + 1] - cum[seg]).max(1e-3)).clamp(0.0, 1.0);
    path[seg].0.lerp(path[seg + 1].0, f)
}

/// FILL the joints a human has NOT placed, between the ones they have — the guided rig's fill
/// (spec 76EB9552). Every maximal run of UNFIXED points between two FIXED ones is laid along the
/// flesh's own centreline between those two, each unfixed point keeping its CURRENT arc-length
/// fraction of the run (the composed rest's proportions), then nudged onto the nearest NARROWING
/// within 8 % of the run's length: a joint belongs where the flesh narrows, and the rest
/// proportions only say which narrowing is which. Unfixed points before the first or after the
/// last fixed one have no run to lie along, so they are only pulled onto the flesh's local medial
/// point. Positions are world cm, in the mesh's own frame; `fixed` is read per point (a short
/// `fixed` leaves the rest unfixed).
pub fn fill_chain(flesh: &Flesh, points: &mut [Vec3], fixed: &[bool]) {
    let anchors: Vec<usize> = (0..points.len())
        .filter(|&i| fixed.get(i).copied().unwrap_or(false))
        .collect();
    // The re-centring reach is the limb's own thickness — bounded, so a chain inside a leg cannot
    // be captured by the body beside it.
    let reach = |p: Vec3| 1.5 * flesh.radius_at(p).max(2.0 * flesh.cell());
    for pair in anchors.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if b <= a + 1 {
            continue; // the two are neighbours: nothing unplaced between them
        }
        let (pa, pb) = (points[a], points[b]);
        // The re-centring reach is the limb's own thickness where it is anchored, never less than
        // an eighth of the run (an anchor dropped on the skin reads almost no radius) and never
        // more than a quarter of it — bounded, so a chain inside a leg cannot be captured by the
        // body beside it.
        let run = pa.distance(pb);
        let r = reach(pa).max(reach(pb)).clamp(run / 8.0, run / 4.0);
        let n = ((run / flesh.cell()).ceil() as usize).clamp(8, 128);
        let prof = flesh.profile(pa, pb, n, r);
        let cum = arc_lengths(&prof);
        let total = *cum.last().expect("arc_lengths is never empty");
        // The run's CURRENT proportions (the composed rest's), as arc length along itself.
        let rest: Vec<f32> =
            arc_lengths(&points[a..=b].iter().map(|&p| (p, 0.0)).collect::<Vec<_>>());
        let span = *rest.last().expect("arc_lengths is never empty");
        if total < 1e-3 || span < 1e-3 {
            continue;
        }
        let dips: Vec<f32> = narrowings(&prof, flesh.cell())
            .iter()
            .map(|&i| cum[i])
            .collect();
        for k in (a + 1)..b {
            let want = total * rest[k - a] / span;
            let s = dips
                .iter()
                .copied()
                .filter(|d| (d - want).abs() <= 0.08 * total)
                .min_by(|x, y| (x - want).abs().total_cmp(&(y - want).abs()))
                .unwrap_or(want);
            points[k] = along_path(&prof, &cum, s);
        }
    }
    // The loose ends (and every point, when nothing at all is fixed): the flesh's medial point.
    let first = anchors.first().copied().unwrap_or(points.len());
    let last = anchors.last().copied().unwrap_or(points.len());
    for k in (0..first).chain((last + 1)..points.len()) {
        points[k] = flesh.centre_near(points[k], reach(points[k]));
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// THE SHAPE MATCH (spec 04803E0C §3) — the recipe walked as the composer walks it and MATCHED,
// module by module, to what the mesh's own [`ShapeGraph`] found. This is what replaced
// `align_trunk`: a trunk is no longer measured by belly runs and croups, it is the body's largest
// CORE; a head is no longer a canon offset ahead of the withers, it is the core or tube AHEAD; a
// hip pair is no longer a height band, it is the pair of tubes nearest the core's rear end.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// WHAT ONE MODULE WAS MATCHED TO — an element of the graph, by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matched {
    /// A trunk on a core, or a head on the core ahead of one.
    Core(usize),
    /// A tail, or a head, on one limb.
    Limb(usize),
    /// A shoulder or hip pair on a symmetry pair of limbs.
    Pair(usize),
}

/// The whole match: which module took which piece of the mesh, which modules found nothing, and
/// which pieces of the mesh no module claimed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeMatch {
    /// `(module id, what it took)` — the id is [`crate::baseline::module_id`]'s, the same one
    /// [`crate::baseline::markers_by_module`] tags its prompts with.
    pub matched: Vec<(String, Matched)>,
    /// The modules nothing in the mesh answered to, in the recipe's own walk order. THE MARKERS
    /// RAIL PROMPTS THESE FIRST (rule 513E5F78: what does not match is what the human is asked
    /// for).
    pub unmatched: Vec<String>,
    /// `(module id, core)` — a HEAD nothing ahead of its trunk's front end answered to, placed on
    /// that core's FRONT CAP anyway: a skull that merged into the barrel leaves no tube ahead of
    /// the front end, and the body's own end is where it is. The module STAYS in
    /// [`ShapeMatch::unmatched`], so the rail still asks the human to eye it; what it no longer
    /// keeps is the composed rest a fraction of the stature out in front of the body.
    pub capped: Vec<(String, usize)>,
    /// The pieces of the mesh the recipe had no module for, said in the graph's own vocabulary
    /// ("a rear midline tube the recipe has no tail for") so the human can pick a richer preset.
    pub spare: Vec<String>,
    /// Anything the match wants said out loud — an orientation that disagrees above all.
    pub warnings: Vec<String>,
}

impl ShapeMatch {
    /// What `module` took, if anything.
    pub fn of(&self, module: &str) -> Option<Matched> {
        self.matched
            .iter()
            .find(|(m, _)| m == module)
            .map(|(_, w)| *w)
    }

    /// THE RAIL'S LIST FOR THIS BODY — the prompts of every module that matched NOTHING, in
    /// `markers_by_module`'s own walk order (Aaron 2026-09-21: *"the rail lists the UNMATCHED
    /// joints in that same walk order"*). A joint whose module found its partner in the mesh is
    /// PLACED, not prompted; a module nothing answered for is exactly the work left for the human
    /// (rule 513E5F78, rail 42BE646D). A body the graph could not read at all matches nothing and
    /// therefore prompts everything, which is the old behaviour and the right fallback.
    pub fn marker_order(&self, recipe: &SkeletonRecipe) -> Vec<String> {
        crate::baseline::markers_by_module(recipe)
            .into_iter()
            .filter(|(id, _)| self.unmatched.iter().any(|u| u == id))
            .map(|(_, m)| m)
            .collect()
    }

    /// One block for a log or a diagnostic.
    pub fn report(&self) -> String {
        let mut out = String::new();
        for (m, w) in &self.matched {
            out.push_str(&format!("  matched {m:<14} {w:?}\n"));
        }
        for u in &self.unmatched {
            out.push_str(&format!("  UNMATCHED {u}\n"));
        }
        for (m, c) in &self.capped {
            out.push_str(&format!(
                "  capped  {m:<14} Core({c}) — its front cap, still prompted\n"
            ));
        }
        for s in &self.spare {
            out.push_str(&format!("  spare {s}\n"));
        }
        for w in &self.warnings {
            out.push_str(&format!("  WARN {w}\n"));
        }
        out
    }
}

/// The walk's state: the graph, the composer's own instance counters, and what has been claimed.
struct Matcher<'a> {
    graph: &'a ShapeGraph,
    counts: HashMap<&'static str, usize>,
    core_used: Vec<bool>,
    limb_used: Vec<bool>,
    pair_used: Vec<bool>,
    out: ShapeMatch,
}

/// MATCH A RECIPE TO A MESH'S SHAPE — the recipe tree walked exactly as [`crate::baseline::compose`]
/// walks it, each module taking the piece of the [`ShapeGraph`] that answers to it:
///
/// - the ROOT TRUNK ↔ the largest core (they are sorted largest first). Its
///   [`flicker_skeletal::format::Orientation`] must agree with the core's own axis against gravity
///   — a disagreement is a loud warning and not a silent re-interpretation, because the fix is the
///   human's facing / quadruped knobs (3A61D440), not a guess here.
/// - HIP PAIRS ↔ the symmetry pairs nearest the core's REAR/BOTTOM end, in order.
/// - SHOULDER PAIRS ↔ the pairs nearest its FRONT/TOP end, in order. A wing module matches a
///   [`crate::shape::Limb::sheet`] pair exactly as an arm matches a tube pair — the recipe says
///   which module it is, the graph only says where.
/// - GRAVITY SEPARATES THEM (spec §2: legs are the tubes reaching the ground, arms and wings the
///   ones that do not): a module the recipe authors STANDING — every leg, and a foreleg whose
///   chain ends in a ground contact ([`ArmKind::Ungulate`]) — takes only a pair that reaches the
///   floor, so a pair of antlers ahead of the forelegs is never laid out as the shoulders.
/// - TAILS ↔ the unpaired limbs behind the middle of the core that do not stand on the floor,
///   the one reaching furthest past the rear end first.
/// - the HEAD ↔ what the NECK leads to, followed from the core's FRONT END wherever it bends
///   (Aaron 2026-09-28, A79A6131 — turned heads are common): the core CHAINED AHEAD of this one —
///   its link leaving this core's front/top half, the smallest such (so a rider's torso is never
///   mistaken for a skull, nor a knee as thick as half a slim body) — else the NECK TUBE that
///   leaves the front end ([`Matcher::neck`]), else the core's own FRONT CAP
///   ([`ShapeMatch::capped`]). **Detected, never a canon offset** — this is what puts a head
///   inside the flesh on the bodies whose skull is nowhere near where a fraction of the stature
///   would put it; [`fit_to_graph`] then lays the neck along the path it took.
/// - MOUNTS ↔ the next chained core, largest first, recursed into with the same rules.
///
/// Nothing is placed here: this says only WHAT TOOK WHAT, which is what the gates assert.
pub fn match_recipe(graph: &ShapeGraph, recipe: &SkeletonRecipe) -> ShapeMatch {
    let mut m = Matcher {
        graph,
        counts: HashMap::new(),
        core_used: vec![false; graph.cores.len()],
        limb_used: vec![false; graph.limbs.len()],
        pair_used: vec![false; graph.pairs.len()],
        out: ShapeMatch::default(),
    };
    m.trunk(&recipe.trunk, (!graph.cores.is_empty()).then_some(0));
    m.spares();
    m.out
}

impl Matcher<'_> {
    /// The cores chained to `core` through a link, smallest first, unclaimed only.
    fn chained(&self, core: usize) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .graph
            .links
            .iter()
            .filter_map(|l| match (l.cores[0] == core, l.cores[1] == core) {
                (true, _) => Some(l.cores[1]),
                (_, true) => Some(l.cores[0]),
                _ => None,
            })
            .filter(|&c| !self.core_used[c])
            .collect();
        out.sort_by(|&a, &b| {
            let size = |k: usize| self.graph.cores[k].arc * self.graph.cores[k].radius;
            size(a).total_cmp(&size(b))
        });
        out.dedup();
        out
    }

    /// Does the link to the chained core `c` leave `core` from its FRONT/TOP half? The head is the
    /// mass AHEAD of the front end (spec 04803E0C §3); a core chained below or behind is not one.
    fn ahead(&self, core: usize, c: usize) -> bool {
        self.graph.links.iter().any(|l| {
            (0..2).any(|i| {
                l.cores[i] == core
                    && l.cores[1 - i] == c
                    && self.graph.cores[core].t_of(l.ends[i]) >= 0.5
            })
        })
    }

    /// One trunk of the recipe on `core`, and everything it carries.
    fn trunk(&mut self, spec: &TrunkSpec, core: Option<usize>) {
        use flicker_skeletal::format::Orientation;
        // THE PREFIXES ARE CLAIMED IN THE COMPOSER'S ORDER (trunk, head, arms, legs, tails,
        // mounts) — that walk is what decides which instance is which, and the ids here have to
        // be the ones the bones and the rail carry.
        let p = crate::baseline::next_prefix(&mut self.counts, "trunk");
        let head = spec
            .head
            .then(|| crate::baseline::next_prefix(&mut self.counts, "head"));
        let proboscis = (spec.head && spec.proboscis > 0)
            .then(|| crate::baseline::next_prefix(&mut self.counts, "proboscis"));
        let arms: Vec<String> = spec
            .arms
            .iter()
            .map(|_| crate::baseline::next_prefix(&mut self.counts, "arm"))
            .collect();
        let legs: Vec<String> = spec
            .legs
            .iter()
            .map(|_| crate::baseline::next_prefix(&mut self.counts, "leg"))
            .collect();
        let tails: Vec<String> = spec
            .tails
            .iter()
            .map(|_| crate::baseline::next_prefix(&mut self.counts, "tail"))
            .collect();

        let trunk_id = crate::baseline::module_id("trunk", &p);
        let Some(core) = core else {
            // No core left for this trunk: it and everything on it keeps the composed rest.
            self.unmatched(&trunk_id);
            if let Some(h) = &head {
                self.unmatched(&crate::baseline::module_id("head", h));
            }
            if let Some(n) = &proboscis {
                self.unmatched(&crate::baseline::module_id("proboscis", n));
            }
            for k in arms.iter().chain(&legs) {
                self.unmatched(&crate::baseline::module_id("arm", k));
            }
            for t in &tails {
                self.unmatched(&crate::baseline::module_id("tail", t));
            }
            for mount in &spec.mounts {
                self.trunk(&mount.trunk, None);
            }
            return;
        };
        self.core_used[core] = true;
        self.out.matched.push((trunk_id, Matched::Core(core)));
        let upright = self.graph.cores[core].upright;
        if (spec.orientation == Orientation::Quadruped) == upright {
            self.out.warnings.push(format!(
                "trunk {p:?} is authored {:?} but its core {} — set the facing or the \
                 quadruped knob, nothing here will guess it",
                spec.orientation,
                if upright { "stands up" } else { "lies down" }
            ));
        }

        // ── THE HEAD, where the neck leaving the front end leads: the smallest core chained to
        // this one, else the neck tube, else — once the shoulders are known — the core's own
        // front cap.
        let mut cap = None;
        // THE PROBOSCIS lies along the tube that runs on past the head's mass — the one tube the
        // head module never follows — or, off a skull of its own, the longest midline tube that
        // hangs from it.
        let mut nose: Option<usize> = None;
        if let Some(h) = head {
            let id = crate::baseline::module_id("head", &h);
            match self
                .chained(core)
                .into_iter()
                .find(|&c| self.ahead(core, c))
            {
                Some(c) => {
                    self.core_used[c] = true;
                    self.out.matched.push((id, Matched::Core(c)));
                    if proboscis.is_some() {
                        nose = self.snout_of(c);
                    }
                }
                None => match self.neck(core) {
                    Some(l) => {
                        self.limb_used[l] = true;
                        self.out.matched.push((id, Matched::Limb(l)));
                        // WHAT RUNS ON PAST THE HEAD is never the head, and never followed — it
                        // is the proboscis's, when the recipe has one, else reported in the
                        // graph's words, like any tube nothing claimed.
                        if let Some(a) = NeckTube::of(self.graph, l).and_then(|t| t.appendage_arc())
                        {
                            if proboscis.is_some() {
                                nose = Some(l);
                            } else {
                                self.out.spare.push(format!(
                                    "a tube {a:.0} cm long running on past the head's mass at the \
                                     end of limb {l} (an appendage) the recipe has no module for"
                                ));
                            }
                        }
                    }
                    None => {
                        self.unmatched(&id);
                        cap = Some(id);
                    }
                },
            }
        }
        if let Some(n) = &proboscis {
            let id = crate::baseline::module_id("proboscis", n);
            match nose {
                Some(l) => {
                    self.limb_used[l] = true;
                    self.out.matched.push((id, Matched::Limb(l)));
                }
                None => self.unmatched(&id),
            }
        }

        // ── THE PAIRS: hips from the rear/bottom, shoulders from the front/top.
        for k in &legs {
            let id = crate::baseline::module_id("leg", k);
            match self.pair_from(core, false, true) {
                Some(i) => {
                    self.pair_used[i] = true;
                    self.out.matched.push((id, Matched::Pair(i)));
                }
                None => self.unmatched(&id),
            }
        }
        let mut shoulders: Option<f32> = None;
        for (k, kind) in arms.iter().zip(&spec.arms) {
            let id = crate::baseline::module_id("arm", k);
            match self.pair_from(core, true, *kind == ArmKind::Ungulate) {
                Some(i) => {
                    self.pair_used[i] = true;
                    self.out.matched.push((id, Matched::Pair(i)));
                    let t = self.graph.pairs[i].t;
                    shoulders = Some(shoulders.map_or(t, |s| s.max(t)));
                }
                None => self.unmatched(&id),
            }
        }

        // ── NO NECK LEAVES THE FRONT END: the skull merged into the barrel, and the head is the
        // core's FRONT CAP — the core beyond its last shoulder attachment (the front-most pair
        // that stands, when no shoulder module matched; the front-most pair of any kind when
        // none stands — a sitting beaver, a raptor whose one pair does not reach the floor).
        // Placed there and still prompted, and only where there IS a cap: at least one of the
        // core's own radii of it beyond that pair. A barrel that ends at its forelegs, or a lump
        // with no pair at all, has no head in it and keeps the composed rest.
        if let Some(id) = cap {
            let g = self.graph;
            let front_most = |stands: bool| {
                g.pairs
                    .iter()
                    .filter(|p| p.core == core)
                    .filter(|p| !stands || g.limbs[p.l].grounded || g.limbs[p.r].grounded)
                    .map(|p| p.t)
                    .reduce(f32::max)
            };
            let last = shoulders
                .or_else(|| front_most(true))
                .or_else(|| front_most(false));
            let c = &g.cores[core];
            if last.is_some_and(|t| (1.0 - t) * c.arc >= c.radius) {
                self.out.capped.push((id, core));
            }
        }

        // ── THE TAILS: unpaired limbs behind the middle that do not stand on the floor.
        for k in &tails {
            let id = crate::baseline::module_id("tail", k);
            match self.behind_the_rear(core) {
                Some(l) => {
                    self.limb_used[l] = true;
                    self.out.matched.push((id, Matched::Limb(l)));
                }
                None => self.unmatched(&id),
            }
        }

        // ── THE MOUNTS: the next chained core, biggest first (a rider's torso, not a skull).
        for mount in &spec.mounts {
            let next = self.chained(core).last().copied();
            self.trunk(&mount.trunk, next);
        }
    }

    /// The tube a proboscis lies along off a skull core of its own: the longest unclaimed MIDLINE
    /// tube hanging from `skull` that does not stand on the floor (an ear is sided; a trunk, a
    /// snout, a beak is on the midline — and only the longest of those is the trunk).
    fn snout_of(&self, skull: usize) -> Option<usize> {
        let g = self.graph;
        (0..g.limbs.len())
            .filter(|&l| {
                let limb = &g.limbs[l];
                !self.limb_used[l] && limb.core == skull && !limb.grounded && limb.side == 0.0
            })
            .max_by(|&a, &b| g.limbs[a].arc.total_cmp(&g.limbs[b].arc))
    }

    /// The nearest unclaimed pair on `core` to its front/top end (`front`) or its rear/bottom —
    /// and ONLY from that half of the core. A hip pair belongs at the rear/bottom and a shoulder
    /// pair at the front/top; letting a hip module take the only pair a body has when that pair
    /// is at the FRONT drags the hind legs onto the forelegs' tubes (measured on the real Horse,
    /// whose hind legs arrive tangled with its tail hair and never pair: its hind hooves ended
    /// 16 cm off the floor). No pair in its own half is an UNMATCHED module, which keeps the
    /// composed legs standing on the ground and puts the joints on the rail instead.
    ///
    /// A module that STANDS (`stands`) takes only a pair at least one of whose tubes ends on the
    /// floor — the spec's own gravity reading, applied to the recipe's side of the match.
    fn pair_from(&self, core: usize, front: bool, stands: bool) -> Option<usize> {
        let g = self.graph;
        let left: Vec<usize> = (0..g.pairs.len())
            .filter(|&i| !self.pair_used[i] && g.pairs[i].core == core)
            .filter(|&i| {
                !stands || g.limbs[g.pairs[i].l].grounded || g.limbs[g.pairs[i].r].grounded
            })
            .collect();
        // With TWO OR MORE pairs ON THIS CORE the ends decide between themselves — the rear-most
        // is the hips and the front-most the shoulders, however the core's span happens to fall,
        // and a pair already claimed by an earlier module is exactly the thing this one is
        // compared against. The half test is only for a core carrying ONE pair in total, where
        // there is nothing to compare with: that pair must be in the half its module belongs to,
        // or it is no match at all. (A core inflated by a head as thick as its barrel puts the
        // forelegs at t = 0.45, and a half test applied to them once the hips are claimed hands
        // the shoulders nothing and composes the withers into the snout.)
        let alone = self.graph.pairs.iter().filter(|p| p.core == core).count() < 2;
        left.into_iter()
            .filter(|&i| !alone || (self.graph.pairs[i].t >= 0.5) == front)
            .min_by(|&a, &b| {
                let d = |i: usize| {
                    let t = self.graph.pairs[i].t;
                    if front {
                        1.0 - t
                    } else {
                        t
                    }
                };
                d(a).total_cmp(&d(b))
            })
    }

    /// THE UNPAIRED LIMB BEHIND THE REAR of `core` — the spec's "tails ↔ the unpaired REAR tubes"
    /// (04803E0C §3). Unclaimed, in no [`crate::shape::Pair`], on the rear half of the core, and
    /// NOT standing on the floor — gravity is what tells a tail from a leg, and it is the one
    /// direction the world supplies.
    ///
    /// Which one, when several answer: the tube that reaches FURTHEST BEYOND the rear end,
    /// measured along the core's own axis. Not the longest (a raised hind leg out-reaches a tail),
    /// and not the one nearest the symmetry plane — THE MIDLINE TEST IS GONE. A tail is a tail
    /// whether it hangs straight back or curls to one side (the Panther's reads 30 cm off the
    /// plane).
    fn behind_the_rear(&self, core: usize) -> Option<usize> {
        let c = &self.graph.cores[core];
        let (end, axis) = (c.rear(), (c.rear() - c.front()).normalize_or_zero());
        let unpaired = self.graph.unpaired();
        unpaired
            .into_iter()
            .filter(|&i| {
                let l = &self.graph.limbs[i];
                !self.limb_used[i] && l.core == core && !l.grounded && l.t < 0.5
            })
            .max_by(|&a, &b| {
                let reach = |i: usize| (self.graph.limbs[i].tip() - end).dot(axis);
                reach(a).total_cmp(&reach(b))
            })
    }

    /// THE NECK — the unpaired tube that LEAVES `core`'s FRONT END and goes on out of it, whatever
    /// way it bends (Aaron 2026-09-28, A79A6131: *"their heads are turned to the side a bit"* — a
    /// head is found by FOLLOWING the neck, never by how far it reaches along the trunk's own
    /// axis). Four readings of the graph, none of them anatomy:
    /// - it is NOT GROUNDED — a path that reaches the floor is a limb, never a neck;
    /// - it LEAVES THE FRONT END: its attachment lies within one of the core's own radii of the
    ///   core's front/top end. A fringe hanging under the chest halfway along the barrel (the
    ///   ElkBull's mane, attached 3.5 radii behind the end) is not the neck, however far it hangs;
    /// - it GOES ON beyond that end: its tip lies past the front end along the core's axis, so a
    ///   horn sweeping back over the body is not a neck;
    /// - it is a LIMB and not a bump of the front cap: its run OUTSIDE the core's own ball
    ///   ([`leaves_core`]) is longer than [`crate::shape::SPUR_RADII`] of the core's thickness —
    ///   the graph's own limb-versus-bump reading. An ear, a crest or a tuft on a skull the trunk
    ///   core already holds goes nowhere the body is not.
    ///
    /// - it CARRIES A HEAD OUT OF THE TRUNK ([`NeckTube::carries_head`]): a tube that is already
    ///   an appendage where it leaves the trunk core's ball (the Rabbit's ears, rising off a skull
    ///   the trunk core holds) carries no head — that head is the front cap.
    ///
    /// Of several, the THICKEST (the median flesh of that run): a neck carries a head's flesh, an
    /// antler, an ear or a tuft leaving the same end does not — the MooseBull's face hangs from
    /// its front end beside a palmate antler that reaches three times as far.
    fn neck(&self, core: usize) -> Option<usize> {
        let g = self.graph;
        let c = &g.cores[core];
        let (front, axis) = (c.front(), (c.front() - c.rear()).normalize_or_zero());
        // The run of a candidate's lead outside the core's own ball, as (arc, median thickness).
        let run = |l: &crate::shape::Limb| -> Option<(f32, f32)> {
            let k = leaves_core(l, c.radius)?;
            let arc: f32 = l.lead[k..].windows(2).map(|s| s[0].distance(s[1])).sum();
            Some((arc, crate::shape::median(&l.lead_r[k..])))
        };
        g.unpaired()
            .into_iter()
            .filter(|&i| !self.limb_used[i])
            .filter_map(|i| {
                let l = &g.limbs[i];
                let leaves = l.core == core
                    && !l.grounded
                    && l.at.distance(front) <= c.radius
                    && (l.tip() - front).dot(axis) > 0.0;
                let (arc, thick) = run(l).filter(|_| leaves)?;
                (arc > crate::shape::SPUR_RADII * c.radius
                    && NeckTube::of(g, i).is_some_and(|t| t.carries_head()))
                .then_some((i, thick))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    fn unmatched(&mut self, id: &str) {
        self.out.unmatched.push(id.to_string());
    }

    /// Everything in the mesh no module claimed, said in the graph's own words.
    fn spares(&mut self) {
        let g = self.graph;
        for (i, c) in g.cores.iter().enumerate() {
            if !self.core_used[i] {
                self.out.spare.push(format!(
                    "a {} core {:.0} cm long at {:.0?} the recipe has no trunk for",
                    if c.upright { "standing" } else { "lying" },
                    c.arc,
                    c.rear().to_array()
                ));
            }
        }
        for (i, p) in g.pairs.iter().enumerate() {
            if !self.pair_used[i] {
                self.out.spare.push(format!(
                    "a {} pair {:.0} along core {} the recipe has no limb pair for",
                    if g.limbs[p.l].sheet { "SHEET" } else { "tube" },
                    p.t,
                    p.core
                ));
            }
        }
        let paired = g.unpaired();
        for i in paired {
            if !self.limb_used[i] {
                let l = &g.limbs[i];
                self.out.spare.push(format!(
                    "an unpaired {} {} tube {:.0} cm long at {:.2} along core {} the recipe has \
                     no module for",
                    if l.side == 0.0 { "midline" } else { "side" },
                    if l.grounded {
                        "ground-reaching"
                    } else {
                        "raised"
                    },
                    l.arc,
                    l.t,
                    l.core
                ));
            }
        }
    }
}

/// A joint no bend answered takes a LANDMARK ([`landmarks`]) within this much of its own STRETCH
/// — the run between the joints either side of it that landed — of its composed place
/// ([`chain_targets`]). The composed rest only says WHICH landmark is which joint's, and a
/// module's proportions are its own, not the body's; wider and a joint reaches past its
/// neighbour's landmark.
const LANDMARK_WINDOW: f32 = 0.2;

/// A joint takes its tube's ATTACHMENT when its composed span to the tip and the tube's own
/// agree to within this ratio ([`chain_targets`]). A tube that leaves its core mid-bone is a
/// third longer than the limb below that bone's far end (the biped and toe-walker fixtures:
/// 1.34, 1.35), and its knee is then a bend further down, not the attachment; one that leaves
/// between a stifle and a hock read 1.17 on the hoofed sources and took the hock a hand high.
const ATTACHMENT_FIT: f32 = 1.15;

/// A BENDING joint takes a BEND within this much of the whole path's length of its composed
/// place ([`chain_targets`]). Under a fifth of the path — a bone's length on a five-joint limb:
/// wider, and a fetlock reaches the hock's bend when the hock has taken a wobble above it.
const BEND_WINDOW: f32 = 0.15;

/// A BEND of a tube's lead path is read between the path's runs this many of the tube's own
/// (median) radii either side of it — the scale at which the direction is its bone's and not the
/// voxel path's step — and never over fewer than [`BEND_CELLS`] cells of the flesh.
const BEND_RADII: f32 = 2.5;
const BEND_CELLS: f32 = 8.0;

/// The path a bend is read off is averaged over this many cells of arc either side. A thinned
/// path steps a whole cell sideways as it runs, and on a limb two cells thick that step, read
/// over a four-cell reach, is a 28° "bend": the seventeen hoofed sources read 125 bends on 52
/// legs that way, 80 over an eight-cell reach and this average — the hocks and fetlocks kept.
const BEND_SMOOTH_CELLS: f32 = 2.0;

/// A BEND turns the path by at least this much (degrees) between those two runs. A voxel path's
/// own jitter turns it by up to half this over the same reach.
const BEND_DEGREES: f32 = 20.0;

/// A lying trunk's core ENDS behind its hip pair — the rump is a cap, and the pelvis is at its
/// end — when the pair's own station is within this many of the core's radii of that end
/// ([`fit_to_graph`]). Further than that the core runs on behind the hips: a tail that is trunk.
const PELVIS_CAP_RADII: f32 = 1.5;

/// A trunk's chain is laid down its core only when that core is a RUN and not a LUMP — at least
/// this many of its own radii long, AND at least two cells thick ([`align_trunk`]'s own test for
/// "this mesh has no body to measure"). A cloud with no shape thins to a blob whose "centreline"
/// is a couple of centimetres of nothing, and a spine laid down that is crushed into a point; the
/// composed rest is moved onto such a core as a PIECE, exactly as it always was (4BB12A75).
const TRUNK_RUN_RADII: f32 = 2.0;

/// A TRUNK'S OWN CHAIN — its `pelvis` and every `spine_NN` the composer laid out from the authored
/// [`flicker_skeletal::format::TrunkSpec::length`], in order. The NECK is deliberately not in it:
/// [`place_head`] runs the neck up to wherever the head was DETECTED, which is its own
/// measurement and not a point along the back.
fn spine_chain(model: &RawModel, prefix: &str) -> Vec<usize> {
    let mut out: Vec<usize> = bone_at(model, &format!("{prefix}pelvis"))
        .into_iter()
        .collect();
    for k in 1..100 {
        let Some(i) = bone_at(model, &format!("{prefix}spine_{k:02}")) else {
            break;
        };
        out.push(i);
    }
    out
}

/// THE RUN OF A CORE between two places along it, in that order (`t0` first) — the piece of the
/// body's own centreline a trunk's chain is laid down. `None` when the two land on the same point
/// and there is no run between them.
fn core_span(core: &crate::shape::Core, t0: f32, t1: f32) -> Option<(Vec<Vec3>, Vec<f32>)> {
    let n = core.path.len().min(core.radii.len());
    if n < 2 {
        return None;
    }
    let prof: Vec<(Vec3, f32)> = core.path[..n]
        .iter()
        .copied()
        .zip(core.radii[..n].iter().copied())
        .collect();
    let cum = arc_lengths(&prof);
    let total = cum.last().copied().unwrap_or(0.0);
    let idx = |t: f32| {
        let want = t.clamp(0.0, 1.0) * total;
        (0..n).min_by(|&a, &b| (cum[a] - want).abs().total_cmp(&(cum[b] - want).abs()))
    };
    let (a, b) = (idx(t0)?, idx(t1)?);
    let (lo, hi) = (a.min(b), a.max(b));
    if hi == lo {
        return None;
    }
    let mut path: Vec<Vec3> = prof[lo..=hi].iter().map(|s| s.0).collect();
    let mut radii: Vec<f32> = prof[lo..=hi].iter().map(|s| s.1).collect();
    if a > b {
        path.reverse();
        radii.reverse();
    }
    Some((path, radii))
}

/// THE LINE AN UPRIGHT CORE LEANS ON, for a biped's spine: from the chain's own first joint
/// (`chain[0]`, the pelvis the crotch placed — it does not move) along the core's own LEAN — the
/// chord of its centreline from the pelvis's place on it to its top — for the chain's composed
/// length, held on the body's symmetry plane (the pelvis's own x, where the fit put the rig's
/// midline). The spine keeps the canon's proportions and its straightness and only its lean comes
/// off the body: the whole run above the pelvis, because a binned centreline zig-zags a cell
/// either way and its lowest slices run down inside one thigh — read over the chain's own length
/// the plumb humans leaned up to 7 cm (HumanBaseA, LizardBaseA), over the whole core under 2.
/// `None` when the core does not rise from the pelvis.
fn upright_span(core: &crate::shape::Core, chain: &[Vec3]) -> Option<(Vec<Vec3>, Vec<f32>)> {
    let pelvis = *chain.first()?;
    let len: f32 = chain.windows(2).map(|s| s[0].distance(s[1])).sum();
    let lean = core.front() - core.at(core.t_of(pelvis));
    let dir = Vec3::new(0.0, lean.y, lean.z).normalize_or_zero();
    (len > 1e-3 && dir.z > 0.0).then(|| (vec![pelvis, pelvis + dir * len], vec![0.0; 2]))
}

/// THE MODULE'S CHAIN from `root` — its root joint, then at every step the child carrying the
/// deepest subtree, to the far end; among children of ONE depth (a hand's five fingers), the one
/// whose subtree REACHES farthest from the root as composed — a tube's tip is its longest finger's,
/// never its thumb's (2026-10-09: the arm chain ran down the thumb, the last of five equals, and
/// stretched it to the fingertip). The twists, fingers, toes and feather groups that branch off
/// it are not in it; they RIDE it (see [`move_chain`]).
fn deep_chain(model: &RawModel, root: usize) -> Vec<usize> {
    let n = model.bones.len();
    let w = model_world_frames(model);
    let subtree = |start: usize| -> (usize, Vec<usize>) {
        let (mut front, mut d, mut all) = (vec![start], 0, vec![start]);
        while !front.is_empty() {
            d += 1;
            front = (0..n)
                .filter(|&i| {
                    model.bones[i].parent >= 0 && front.contains(&(model.bones[i].parent as usize))
                })
                .collect();
            all.extend_from_slice(&front);
        }
        (d, all)
    };
    let origin = pos_of(w[root]);
    let mut out = vec![root];
    loop {
        let at = *out.last().expect("non-empty");
        let next = (0..n)
            .filter(|&i| model.bones[i].parent == at as i32)
            .map(|i| {
                let (d, all) = subtree(i);
                let reach = all
                    .iter()
                    .map(|&j| pos_of(w[j]).distance(origin))
                    .fold(0.0_f32, f32::max);
                (i, d, reach)
            })
            .max_by(|a, b| a.1.cmp(&b.1).then(a.2.total_cmp(&b.2)));
        match next {
            Some((i, _, _)) if !out.contains(&i) => out.push(i),
            _ => break out,
        }
    }
}

/// THE JOINT CANDIDATES OF A TUBE (Aaron on the Elk, 2026-09-29: *"Seems to have inserted a joint
/// in the middle of the foot (lower leg). Not it."*) — the indices along its lead `path` where a
/// joint may land: where the path BENDS, and where the flesh WAISTS ([`narrowings`]: bounded by
/// thicker flesh on both sides within a couple of its own radii). A bone is straight and a joint
/// is where it meets the next one, so a narrowing on a straight run — a long cannon's mid-shaft,
/// a taper that keeps falling on one side — is never a joint.
///
/// A BEND is a local maximum of the turn between the path's run [`BEND_RADII`] of the tube's
/// median radius behind a point and the run as far ahead of it (the path averaged over
/// [`BEND_SMOOTH_CELLS`] first: a voxel path steps a cell sideways), at least [`BEND_DEGREES`];
/// a lesser turn within
/// that reach of a greater one is the same bend. Only where the whole reach lies on the tube:
/// the tip's cap and the stretch where the tube leaves its core read no bend. A waist within a
/// bend's reach is that bend. `path`, `radii`: the tube from its attachment to its TIP; `cell`:
/// its flesh's.
/// What a landmark IS — a joint that bends in the composed chain may only take a BEND
/// ([`chain_targets`]); a waist on a straight run is a straight joint's, never a bending one's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mark {
    Bend,
    Waist,
}

fn landmarks(path: &[Vec3], radii: &[f32], cell: f32) -> Vec<(usize, Mark)> {
    let n = path.len().min(radii.len());
    if n < 3 {
        return Vec::new();
    }
    let prof: Vec<(Vec3, f32)> = path[..n]
        .iter()
        .copied()
        .zip(radii[..n].iter().copied())
        .collect();
    let cum = arc_lengths(&prof);
    let len = cum[n - 1];
    let reach = (BEND_RADII * crate::shape::median(&radii[..n])).max(BEND_CELLS * cell);
    // The path resampled every half cell (a lead's samples are not evenly spaced) and averaged
    // over [`BEND_SMOOTH_CELLS`] of arc either side — each point, and the runs read off it.
    let step = 0.5 * cell.max(1e-3);
    let m = (len / step).ceil() as usize + 1;
    let at = |k: usize| (k as f32 * step).min(len);
    let dense: Vec<Vec3> = (0..m).map(|k| along_path(&prof, &cum, at(k))).collect();
    let dcum: Vec<f32> = (0..m).map(at).collect();
    let half = (BEND_SMOOTH_CELLS * cell / step).round() as usize;
    let smooth: Vec<(Vec3, f32)> = (0..m)
        .map(|k| {
            let (lo, hi) = (k.saturating_sub(half), (k + half).min(m - 1));
            let sum: Vec3 = dense[lo..=hi].iter().copied().sum();
            (sum / (hi - lo + 1) as f32, 0.0)
        })
        .collect();
    // THE FOOT IS NOT THE SHIN. A limb that ends in a BLOB — its flesh widening from the tip and
    // narrowing again above it, within two reaches of the end: a hoof, a paw — carries its lead
    // on through that blob to the far toe, and the path's turn inside it is the cap's, not a
    // joint's: from the tip up to the blob's widest flesh nothing is read. (Read, the hoof's own
    // turn took the fetlock down onto the coronet — the Donkey, the Horse, 2026-09-30.) A limb
    // that only tapers to its tip has no foot, and its cap is the reach alone.
    let mut foot = n - 1;
    while foot > 0 && radii[foot - 1] >= radii[foot] {
        foot -= 1;
    }
    if foot == 0 || len - cum[foot] > 2.0 * reach {
        foot = n;
    }
    let on_tube = |i: usize| cum[i] >= reach && cum[i] <= len - reach && i < foot;
    let turn = |i: usize| {
        let (p, back, ahead) = (
            along_path(&smooth, &dcum, cum[i]),
            along_path(&smooth, &dcum, cum[i] - reach),
            along_path(&smooth, &dcum, cum[i] + reach),
        );
        (p - back).angle_between(ahead - p).to_degrees()
    };
    let mut bends: Vec<(usize, f32)> = (0..n)
        .filter(|&i| on_tube(i))
        .map(|i| (i, turn(i)))
        .filter(|b| b.1 >= BEND_DEGREES)
        .collect();
    bends.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut bent: Vec<usize> = Vec::new();
    for (i, _) in bends {
        if bent.iter().all(|&j| (cum[j] - cum[i]).abs() >= reach) {
            bent.push(i);
        }
    }
    // A waist beside a bend IS that bend, placed at the waist: the waist is the exact spot, the
    // turn maximum is smeared over the reach (measured: the hock's bend read 4 cm above its
    // waist, and a straight joint stole the waist). Beside it: within the tube's own thickness,
    // [`BEND_RADII`] of it and never under four cells — not the whole reach a turn is read over,
    // which on a thin limb spans a bone.
    let beside = (BEND_RADII * crate::shape::median(&radii[..n])).max(4.0 * cell);
    let waists = narrowings(&prof, cell);
    let mut out: Vec<(usize, Mark)> = bent
        .iter()
        .map(|&b| {
            let at = waists
                .iter()
                .copied()
                .filter(|&w| (cum[w] - cum[b]).abs() <= beside)
                .min_by(|&x, &y| (cum[x] - cum[b]).abs().total_cmp(&(cum[y] - cum[b]).abs()))
                .unwrap_or(b);
            (at, Mark::Bend)
        })
        .collect();
    out.extend(
        waists
            .into_iter()
            .filter(|&w| cum[w] >= beside && cum[w] <= len - beside)
            .filter(|&w| bent.iter().all(|&b| (cum[b] - cum[w]).abs() > beside))
            .map(|w| (w, Mark::Waist)),
    );
    out.sort_unstable_by_key(|m| m.0);
    out.dedup_by_key(|m| m.0);
    out
}

/// The turn of a composed chain at each of its joints (degrees; the ends turn nothing) — a joint
/// that turns at least [`BEND_DEGREES`] is a BENDING joint: a knee, a hock, an elbow, a stifle.
fn composed_turns(pts: &[Vec3]) -> Vec<f32> {
    (0..pts.len())
        .map(|k| {
            if k == 0 || k + 1 >= pts.len() {
                return 0.0;
            }
            let (a, b) = (pts[k] - pts[k - 1], pts[k + 1] - pts[k]);
            if a.length_squared() < 1e-6 || b.length_squared() < 1e-6 {
                return 0.0;
            }
            a.angle_between(b).to_degrees()
        })
        .collect()
}

/// THE JOINTS TAKE THE LANDMARKS IN ORDER — the order-preserving matching of joints (their
/// composed places along the path, `want`, in chain order) to landmarks (their places `at`, in
/// path order) that lands every joint it can: a joint takes at most one landmark within `window`
/// of its place, no landmark is taken twice and none out of order, and among the matchings that
/// land as many joints the nearest win (each taken landmark costs its distance over the window,
/// squared, a joint left unlanded costs 1). A landmark no joint reaches is an extra, left alone.
/// Returns each joint's landmark, by index into `at`.
fn in_order(want: &[f32], at: &[f32], window: f32) -> Vec<Option<usize>> {
    let (j, m) = (want.len(), at.len());
    // cost[a][b]: the best for the first `a` joints over the first `b` landmarks.
    let mut cost = vec![vec![0.0_f32; m + 1]; j + 1];
    let mut took = vec![vec![0_u8; m + 1]; j + 1]; // 1 skip the joint, 2 skip the landmark, 3 take
    for a in 0..=j {
        for b in 0..=m {
            if a == 0 && b == 0 {
                continue;
            }
            let mut best = (f32::INFINITY, 0_u8);
            if a > 0 && cost[a - 1][b] + 1.0 < best.0 {
                best = (cost[a - 1][b] + 1.0, 1);
            }
            if b > 0 && cost[a][b - 1] < best.0 {
                best = (cost[a][b - 1], 2);
            }
            let d = if a > 0 && b > 0 {
                (want[a - 1] - at[b - 1]).abs()
            } else {
                f32::INFINITY
            };
            if d <= window && cost[a - 1][b - 1] + (d / window).powi(2) < best.0 {
                best = (cost[a - 1][b - 1] + (d / window).powi(2), 3);
            }
            (cost[a][b], took[a][b]) = best;
        }
    }
    let mut out = vec![None; j];
    let (mut a, mut b) = (j, m);
    while a > 0 || b > 0 {
        match took[a][b] {
            3 => {
                out[a - 1] = Some(b - 1);
                (a, b) = (a - 1, b - 1);
            }
            1 => a -= 1,
            _ => b -= 1,
        }
    }
    out
}

/// WHERE A CHAIN'S JOINTS GO ALONG A PATH (from its root at `path[0]` to its far end): the two
/// ends at the path's ends, and between them, in this order —
///
/// 1. THE ATTACHMENT (`joint_at`: where a limb's tube leaves its core, [`from_the_tip`], and the
///    fit asked of it) goes to the joint whose own limb the tube below it is — the joint whose
///    composed span to the tip is the tube's, within that fit ([`ATTACHMENT_FIT`], unless a
///    pair is reading it every way) — and to no joint when the tube leaves mid-bone:
///    the shoulder, a hoofed leg's stifle or hock, a foreleg's elbow (a narrowing just below it
///    is the muscle's own taper — measured: the elves' upper arms taken 6 cm down the arm onto
///    the deltoid's).
/// 2. BENDING joints — the composed chain turns there by [`BEND_DEGREES`], or the chain's end
///    hangs from them — take the tube's BENDS, in order ([`in_order`]), each within
///    [`BEND_WINDOW`] of the path of its composed place.
/// 3. Every joint still unlanded takes what is left between the joints that landed, within
///    [`LANDMARK_WINDOW`] of that stretch: its WAISTS, then its bends.
/// 4. A joint that took nothing lies between the nearest joints that did, at its composed
///    fraction of the run between them — never on a narrowing of its own, never at a fraction
///    of the whole path that ignores where its neighbours landed.
///
/// `marks`: [`landmarks`] — bends and waists, path indices. Each target comes back with its arc
/// along the path. `pts` is the chain as composed.
fn chain_targets(
    pts: &[Vec3],
    path: &[Vec3],
    marks: &[(usize, Mark)],
    joint_at: Option<(usize, f32)>,
) -> Vec<(Vec3, f32)> {
    let as_prof = |v: &[Vec3]| v.iter().map(|&p| (p, 0.0)).collect::<Vec<_>>();
    let (prof, cum) = (as_prof(path), arc_lengths(&as_prof(pts)));
    let pcum = arc_lengths(&prof);
    let (total, plen) = (cum[cum.len() - 1], pcum[pcum.len() - 1]);
    let n = pts.len();
    let want = |k: usize| cum[k] / total.max(1e-3) * plen;
    let mut landed: Vec<Option<usize>> = vec![None; n];
    // THE ATTACHMENT IS A JOINT'S PLACE ONLY WHEN THE TUBE BELOW IT IS THAT JOINT'S LIMB: the
    // joint whose composed span from it to the tip is as long as the tube's is from its
    // attachment to its tip — to within the fit asked for, and closer than the whole chain is
    // (then the tube is the whole limb, its root at the socket above: a biped's leg leaving at
    // the crotch). A tube that leaves its core at the hock is a cannon and a pastern long; one
    // that leaves at the stifle carries a tibia more. Lengths, not fractions of the path: the
    // run inside the core is a line the fit drew, and a fraction of it says nothing about the
    // tube. (A fixed window of the path left a buried stifle untaken and a lower-leg bend took
    // the calf; the nearest composed place gave a straight leg's knee to a tube that leaves it
    // mid-thigh — 2026-09-29/30.) Otherwise the attachment falls mid-bone and no joint takes it.
    let taker = joint_at.and_then(|(a, fit)| {
        // Straight-line spans, not arcs: a voxel path wanders a cell either side of its tube
        // and reads a tenth longer than the limb it follows.
        let tube = path[a].distance(path[path.len() - 1]);
        let off = |k: usize| {
            (tube.max(1e-3) / pts[k].distance(pts[n - 1]).max(1e-3))
                .ln()
                .abs()
        };
        (1..n.saturating_sub(1))
            .min_by(|&x, &y| off(x).total_cmp(&off(y)))
            .filter(|&k| off(k) <= fit.ln() && off(k) < off(0))
            .map(|k| (k, a))
    });
    let joint_at = joint_at.map(|j| j.0);
    if let Some((k, a)) = taker {
        landed[k] = Some(a);
    }
    // The landmarks in order on either side of the attachment's joint (the whole chain when
    // none took it): a joint below it takes only a landmark below it. BENDING joints (the
    // composed chain turns there — a hock, a stifle, an elbow) take only BENDS, in
    // order, each within [`BEND_WINDOW`] of the path of its composed place: the composed
    // fractions say which bend is whose, and a bend further off than that is some other joint's
    // or none's (a tube's wobble where it leaves its core). Straight joints then take the
    // landmarks left between the joints that landed — a waist on a straight run is theirs, never
    // a bending one's.
    let turns = composed_turns(pts);
    let (split, at) = taker.unwrap_or((0, 0));
    for (joints, lo, hi) in [(1..split.max(1), 0, at), (split + 1..n - 1, at, usize::MAX)] {
        let joints: Vec<usize> = joints.collect();
        let mine: Vec<(usize, Mark)> = marks
            .iter()
            .copied()
            .filter(|&(m, _)| m > lo && m < hi && Some(m) != joint_at)
            .collect();
        // ...and THE JOINT THE CHAIN'S END HANGS FROM bends whatever the module drew: it is where
        // a limb turns onto its foot (a fetlock, an ankle, a wrist), and a module that composes
        // its last bone in line with the one above — a pastern drawn plumb under its cannon —
        // has drawn a rest, not a claim that the body's own foot does not turn there.
        let benders: Vec<usize> = joints
            .iter()
            .copied()
            .filter(|&k| turns[k] >= BEND_DEGREES || k + 2 == n)
            .collect();
        let bends: Vec<usize> = mine
            .iter()
            .filter(|m| m.1 == Mark::Bend)
            .map(|m| m.0)
            .collect();
        if !benders.is_empty() && !bends.is_empty() {
            let wanted: Vec<f32> = benders.iter().map(|&k| want(k)).collect();
            let places: Vec<f32> = bends.iter().map(|&m| pcum[m]).collect();
            for (k, took) in benders
                .iter()
                .zip(in_order(&wanted, &places, BEND_WINDOW * plen))
            {
                landed[*k] = took.map(|t| bends[t]);
            }
        }
        // The joints still unlanded, between the ones that landed: whatever landmark is left in
        // their stretch, in order, near their composed place — its WAISTS first (a waist on a
        // straight run is a straight joint's own landmark: a carpus is where the forearm's taper
        // meets the cannon), and then, for a joint no waist answered, its bends (a straight joint
        // the pose has flexed: a raised knee).
        for waists_only in [true, false] {
            let mut stretch_lo = lo;
            let mut k0 = 0;
            loop {
                let next = (k0..joints.len()).find(|&i| landed[joints[i]].is_some());
                let (k1, stretch_hi) = match next {
                    Some(i) => (i, landed[joints[i]].unwrap()),
                    None => (joints.len(), hi),
                };
                let unlanded: Vec<usize> = joints[k0..k1].to_vec();
                let left: Vec<usize> = mine
                    .iter()
                    .filter(|&&(m, kind)| {
                        m > stretch_lo && m < stretch_hi && (!waists_only || kind == Mark::Waist)
                    })
                    .map(|m| m.0)
                    .collect();
                if !unlanded.is_empty() && !left.is_empty() {
                    let wanted: Vec<f32> = unlanded.iter().map(|&k| want(k)).collect();
                    let places: Vec<f32> = left.iter().map(|&m| pcum[m]).collect();
                    // Within a fraction of the STRETCH between the joints that landed — a carpus
                    // never reaches a fetlock 18 cm down the cannon.
                    let run = pcum.get(stretch_hi).copied().unwrap_or(plen) - pcum[stretch_lo];
                    for (k, took) in
                        unlanded
                            .iter()
                            .zip(in_order(&wanted, &places, LANDMARK_WINDOW * run))
                    {
                        landed[*k] = took.map(|t| left[t]);
                    }
                }
                if next.is_none() {
                    break;
                }
                stretch_lo = stretch_hi;
                k0 = k1 + 1;
            }
        }
    }
    // Every joint that took no landmark: between the nearest that did (the two ends included),
    // at its composed fraction of the run between them.
    let place = |k: usize| match landed[k] {
        Some(m) => Some(pcum[m]),
        None if k == 0 => Some(0.0),
        None if k + 1 == n => Some(plen),
        None => None,
    };
    (0..n)
        .map(|k| {
            if let Some(m) = landed[k] {
                return (path[m], pcum[m]);
            }
            let lo = (0..=k).rev().find(|&q| place(q).is_some()).unwrap_or(0);
            let hi = (k..n).find(|&q| place(q).is_some()).unwrap_or(n - 1);
            let (a, b) = (place(lo).unwrap_or(0.0), place(hi).unwrap_or(plen));
            let s = if hi == lo {
                a
            } else {
                a + (cum[k] - cum[lo]) / (cum[hi] - cum[lo]).max(1e-3) * (b - a)
            };
            (along_path(&prof, &pcum, s), s)
        })
        .collect()
}

/// A LIMB LAID FROM ITS TIP (Aaron on the Elk, BAD0D72C: the hip was skipped and the hind chain
/// started at the stifle — *"the hip bone is actually supposed to be basically at the ass of the
/// animal"*). A limb's tube leaves its trunk core wherever the body's own flesh lets it go, and
/// that is not where the module's root joint is: a quadruped's femur and a heavy biped's thigh are
/// buried in the trunk's flesh, so the tube starts at the stifle or the knee; a foreleg's scapula
/// and upper arm are buried in the chest, so its tube starts at the elbow. Laid from that start,
/// the chain runs one joint late — its root on the stifle, every joint below it on the next one
/// down (measured: the ElkBull's thigh 7 cm under its composed hip, HumanBaseA's 34 cm under its
/// shipped one, on its knee).
///
/// So the chain is laid from the end that IS certain — the GROUND JOINT at the tube's tip — up the
/// tube, and on INTO THE CORE to the module's SOCKET, the root's place in the composed rest (the
/// recipe's own geometry at the fitted knobs, before any chain is laid — rule 513E5F78): the path
/// is the tube (tip to attachment) and then the straight run inside the core from the attachment
/// to the socket. The ATTACHMENT and the tube's LANDMARKS — its bends and waists ([`landmarks`])
/// — are the places the joints take, in order up the tube ([`chain_targets`]), so a hock/stifle
/// zigzag is the tube's own, never assumed; a joint no landmark answers lies between the ones
/// that landed at its composed fraction, and the joints that do not fit on the tube lie on the
/// run inside the core, each inside the flesh (nudged toward the core's centreline where it is
/// not), the root at the socket. The TUBE CARRIES THE FREE LIMB: when no joint but the ground joint
/// landed on it (a sitting body's leg is all buried but its foot), the joint nearest the
/// attachment is laid there. `socket` is `None` when the socket is NOT beyond the attachment (the
/// tube leaves the core at or above the root — [`beyond`]): the whole chain lies on the tube,
/// spread over it exactly as before, from its first point (a pair's shared seat,
/// [`pair_socket`], held inside the flesh: a stride's two attachments can average into the air
/// between the legs). `attach`: how well a joint's limb must fit the tube to take its
/// attachment ([`ATTACHMENT_FIT`]; infinite for "whichever joint fits best"), or `None` for no
/// joint there at all — the readings a pair's two sides are laid under every way
/// ([`fit_to_graph`]). `None` back when the tube is too short to say where the limb goes — not
/// a cell of the flesh long: the module stays unmatched and prompted.
fn from_the_tip(
    flesh: &Flesh,
    core: &crate::shape::Core,
    pts: &[Vec3],
    socket: Option<Vec3>,
    tube: &[Vec3],
    radii: &[f32],
    attach: Option<f32>,
) -> Option<Vec<Vec3>> {
    // A one-bone chain, or nothing to lay it on: as composed.
    let length = |v: &[Vec3]| v.windows(2).map(|s| s[0].distance(s[1])).sum::<f32>();
    if pts.len() < 2 || tube.len() < 2 || length(pts) < 1e-3 {
        return Some(pts.to_vec());
    }
    if length(tube) < flesh.cell() {
        return None;
    }
    let marks = landmarks(tube, radii, flesh.cell());
    // Inside the flesh, or nudged toward the core's own centreline until it is.
    let inside = |p: Vec3| {
        if flesh.contains(p) {
            return p;
        }
        let c = core.at(core.t_of(p));
        let steps = (p.distance(c) / flesh.cell()).ceil().max(1.0) as usize;
        (1..=steps)
            .map(|k| p.lerp(c, k as f32 / steps as f32))
            .find(|&q| flesh.contains(q))
            .unwrap_or(c)
    };
    let Some(socket) = socket else {
        let mut tube = tube.to_vec();
        tube[0] = inside(tube[0]);
        return Some(
            chain_targets(pts, &tube, &marks, None)
                .into_iter()
                .map(|t| t.0)
                .collect(),
        );
    };
    let socket = inside(socket);
    let path: Vec<Vec3> = std::iter::once(socket)
        .chain(tube.iter().copied())
        .collect();
    let marks: Vec<(usize, Mark)> = marks.iter().map(|&(d, k)| (d + 1, k)).collect();
    let run = socket.distance(tube[0]);
    let mut at = chain_targets(pts, &path, &marks, attach.map(|fit| (1, fit)));
    let n = at.len();
    if n > 2 && at[n - 2].1 < run {
        at[n - 2] = (tube[0], run);
    }
    // THE BURIED JOINTS KEEP THE MODULE'S OWN SHAPE. Inside the core there is no tube to read,
    // and the run from the socket to the attachment is a straight line only because nothing
    // says otherwise — but the module does: a foreleg's shoulder joint stands FORWARD of the line
    // from its scapula to its elbow. So the composed chain from the root to the first joint on
    // the tube is turned and scaled as one piece onto the socket and that joint's place, and
    // every joint between them lies where the module's own zigzag puts it.
    //
    // AS FAR AS THE FLESH HOLDS IT: a leg reaching forward swings that zigzag forward with it,
    // and a stifle carried a third of the stature ahead of its hip stands in the flank with its
    // tibia crossing the air under the belly. So the zigzag is taken as far from the straight
    // run as keeps every buried bone inside the flesh — the whole of it where the body is that
    // wide, none of it where the straight run is all there is room for.
    if let Some(first) = (1..n).find(|&k| at[k].1 >= run - 1e-3).filter(|&k| k >= 2) {
        let (u, v) = (pts[first] - pts[0], at[first].0 - socket);
        if u.length() > 1e-3 && v.length() > 1e-3 {
            let turn = Quat::from_rotation_arc(u.normalize(), v.normalize());
            let scale = v.length() / u.length();
            let line: Vec<Vec3> = (0..=first).map(|k| at[k].0).collect();
            let zig: Vec<Vec3> = (0..=first)
                .map(|k| match k {
                    0 => line[0],
                    k if k == first => line[k],
                    k => socket + turn * (pts[k] - pts[0]) * scale,
                })
                .collect();
            let laid = |t: f32| -> Vec<Vec3> {
                line.iter().zip(&zig).map(|(a, b)| a.lerp(*b, t)).collect()
            };
            let held = |t: f32| {
                laid(t).windows(2).all(|s| {
                    let steps = (s[0].distance(s[1]) / flesh.cell()).ceil().max(1.0) as usize;
                    (0..=steps).all(|q| flesh.contains(s[0].lerp(s[1], q as f32 / steps as f32)))
                })
            };
            let t = (0..=10)
                .rev()
                .map(|q| q as f32 / 10.0)
                .find(|&t| held(t))
                .unwrap_or(0.0);
            for (k, p) in laid(t).into_iter().enumerate().take(first).skip(1) {
                at[k].0 = p;
            }
        }
    }
    Some(
        at.into_iter()
            .map(|(p, s)| if s < run { inside(p) } else { p })
            .collect(),
    )
}

/// Where along a run of points (its `cum` from [`arc_lengths`]) the point `p` projects: the
/// station, in cm from the run's start, of its nearest place on the polyline.
fn station_of(prof: &[(Vec3, f32)], cum: &[f32], p: Vec3) -> f32 {
    let mut best = (f32::INFINITY, 0.0);
    for (i, pair) in prof.windows(2).enumerate() {
        let (a, ab) = (pair[0].0, pair[1].0 - pair[0].0);
        let len2 = ab.length_squared();
        let t = if len2 < 1e-6 {
            0.0
        } else {
            ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
        };
        let d = p.distance(a + ab * t);
        if d < best.0 {
            best = (d, cum[i] + t * (cum[i + 1] - cum[i]));
        }
    }
    best.1
}

/// The run's thickness `s` cm along it — its radius interpolated between the points either side.
fn radius_at(prof: &[(Vec3, f32)], cum: &[f32], s: f32) -> f32 {
    if prof.len() < 2 {
        return prof.first().map_or(0.0, |p| p.1);
    }
    let seg = cum
        .iter()
        .rposition(|&d| d <= s)
        .unwrap_or(0)
        .min(prof.len() - 2);
    let f = ((s - cum[seg]) / (cum[seg + 1] - cum[seg]).max(1e-3)).clamp(0.0, 1.0);
    prof[seg].1 + (prof[seg + 1].1 - prof[seg].1) * f
}

/// The first bone of a chain that a FAN spreads from — the index `k` of the first chain bone with
/// [`FAN_CHILDREN`] or more children in its own module (a hand, a foot's ball, a wing's wrist).
fn first_fan(model: &RawModel, chain: &[usize], modules: &[String]) -> Option<usize> {
    let n = model.bones.len();
    chain.iter().position(|&b| {
        (0..n)
            .filter(|&c| model.bones[c].parent == b as i32 && modules.get(c) == modules.get(b))
            .count()
            >= FAN_CHILDREN
    })
}

/// A PAIR'S TWO CHAINS LAID AS ONE WHERE THE MESH IS ONE TUBE MIRRORED. The station a joint
/// takes along its tube is the fit's noisiest reading (measured on the reworked humanoids,
/// 2026-10-09: feet mirroring to 0.5 cm, the laid knees 5–9 cm apart and the hands up to 12);
/// where the tube itself lies is not. So joint by joint below the root, the two sides' STATIONS
/// along their own runs (socket to tip) are averaged, and where the two runs lie within one
/// another's THICKNESS at that station — the same tube, read either side of the plane — the
/// joint is laid at the mean place and its mirror. Where they do not, each side keeps its own
/// place: a stride's two legs part from the hip down, a wing swept further than its twin, a
/// thinning that ends on a different fingertip either side — and a mean across the plane there
/// is a joint in the air between the limbs (a crow's wing tips 17 cm apart averaged 9 cm off
/// either wing, a striding wolf's forelegs each pulled toward the other, 2026-10-09; the first
/// cut took any pair whose tube ends mirrored within a fifth of the limb as one). A joint
/// BURIED in the core (before its tube starts — the module's own zigzag, laid from a socket the
/// composed rest already mirrors) is averaged outright when the joint its tube starts with was:
/// the core is the body's own mirror. FROM A FAN ON (`fan_from`: the hand its fingers spread
/// from, the ball its toes do) the two tubes are two branches of one spread — the thinning runs
/// down whichever finger it reaches first, either side — so the test there is the mesh's own:
/// the joint is laid at the mean of the two places where that mean, and its mirror, lie inside
/// the flesh at its grain (between two fingers held together it does; between two fingertips
/// splayed apart, or two wings swept differently, it hangs in the air and each side keeps its
/// own). The chain's LAST joint is read the same way when BOTH tubes stand on the ground
/// (`grounded`: the graph's own reading of a tip on the floor): it sits at its tube's tip, and a
/// standing foot's tip is the end of a thinning that wanders onto one toe or another of a spread
/// the recipe fans nowhere — but only WHERE it stands is averaged; its HEIGHT is the floor's
/// own reading and stays each side's (a sitting squirrel's two forepaws, one a hand higher than
/// the other, averaged in height lifted the planted one off the floor, 2026-10-09). A tip in the
/// air is not read this way: a raised paw and its planted twin have a mean inside the forearm's
/// flesh and are not one paw. Back: how many joints below the root were laid as one, of how
/// many — every one, and the two modules are exact mirrors.
fn lay_as_one(
    sides: [(&[Vec3], &[f32], Option<Vec3>); 2],
    chosen: &mut [Vec<Vec3>],
    plane_x: f32,
    flesh: &Flesh,
    fan_from: Option<usize>,
    grounded: bool,
) -> (usize, usize) {
    /// One side's run: its profile (socket, then the tube), the arc lengths along it, and how
    /// far along it the tube starts.
    struct Run {
        prof: Vec<(Vec3, f32)>,
        cum: Vec<f32>,
        start: f32,
    }
    let cell = flesh.cell();
    let mirror = |v: Vec3| Vec3::new(2.0 * plane_x - v.x, v.y, v.z);
    let runs: Vec<Run> = sides
        .iter()
        .map(|(path, radii, socket)| {
            let r = |i: usize| radii.get(i).or(radii.last()).copied().unwrap_or(0.0);
            let prof: Vec<(Vec3, f32)> = socket
                .map(|s| (s, r(0)))
                .into_iter()
                .chain(path.iter().enumerate().map(|(i, &p)| (p, r(i))))
                .collect();
            let cum = arc_lengths(&prof);
            let start = socket
                .zip(path.first())
                .map_or(0.0, |(s, &p)| s.distance(p));
            Run { prof, cum, start }
        })
        .collect();
    let [l, r] = chosen else {
        return (0, 0);
    };
    let n = l.len().min(r.len());
    if n < 2 || runs.iter().any(|r| r.prof.len() < 2) {
        return (0, n.saturating_sub(1));
    }
    let station = |i: usize, p: Vec3| station_of(&runs[i].prof, &runs[i].cum, p);
    let buried = |i: usize, s: f32| s < runs[i].start - 1e-3;
    // On the tube: the mean station, where the two runs lie within one another's thickness.
    let on_tube = |sl: f32, sr: f32| -> Option<Vec3> {
        let s = 0.5 * (sl + sr);
        let at = |i: usize| {
            let Run { prof, cum, .. } = &runs[i];
            let s = s.min(cum.last().copied().unwrap_or(0.0));
            (along_path(prof, cum, s), radius_at(prof, cum, s))
        };
        let ((pl, rl), (pr, rr)) = (at(0), at(1));
        (pl.distance(mirror(pr)) <= rl.max(rr).max(cell)).then(|| 0.5 * (pl + mirror(pr)))
    };
    // In a fan: the mean of the two places, where the flesh holds it either side.
    let in_fan = |a: Vec3, b: Vec3| -> Option<Vec3> {
        let m = 0.5 * (a + mirror(b));
        (flesh.distance_outside(m) <= cell && flesh.distance_outside(mirror(m)) <= cell)
            .then_some(m)
    };
    let mut means: Vec<Option<Vec3>> = vec![None; n];
    let mut buried_both = vec![false; n];
    let mut as_one_tips = 0;
    for k in 1..n {
        if fan_from.is_some_and(|f| k >= f) {
            means[k] = in_fan(l[k], r[k]);
            continue;
        }
        if grounded && k + 1 == n {
            if let Some(m) = in_fan(l[k], r[k]) {
                l[k] = Vec3::new(m.x, m.y, l[k].z);
                r[k] = Vec3::new(mirror(m).x, m.y, r[k].z);
                as_one_tips += 1;
            }
            continue;
        }
        let (sl, sr) = (station(0, l[k]), station(1, r[k]));
        match (buried(0, sl), buried(1, sr)) {
            (true, true) => buried_both[k] = true,
            (false, false) => means[k] = on_tube(sl, sr),
            _ => {}
        }
    }
    // ...and the buried joints follow the first joint on the tube.
    let first_on_tube = (1..n).find(|&k| !buried_both[k]);
    if first_on_tube.is_some_and(|k| means[k].is_some()) {
        for k in (1..n).filter(|&k| buried_both[k]) {
            means[k] = Some(0.5 * (l[k] + mirror(r[k])));
        }
    }
    let mut as_one = as_one_tips;
    for (k, m) in means.into_iter().enumerate() {
        if let Some(m) = m {
            l[k] = m;
            r[k] = mirror(m);
            as_one += 1;
        }
    }
    (as_one, n - 1)
}

/// How far `socket` lies BEYOND a tube's attachment `a` along the limb's own axis (from its `tip`
/// to its attachment): positive when the socket is inside the core past where the tube leaves it
/// ([`from_the_tip`]).
fn beyond(socket: Vec3, a: Vec3, tip: Vec3) -> f32 {
    (socket - a).dot((a - tip).normalize_or_zero())
}

/// LAY A CHAIN ALONG A MATCHED SEGMENT — the trunk down its core ([`chain_targets`] on the core's
/// own waists, read at the flesh's `cell`).
fn lay_chain(
    model: &RawModel,
    w: &mut [Mat4],
    chain: &[usize],
    path: &[Vec3],
    radii: &[f32],
    cell: f32,
    modules: &[String],
) {
    if chain.len() < 2 || path.len() < 2 {
        return;
    }
    let pts: Vec<Vec3> = chain.iter().map(|&i| pos_of(w[i])).collect();
    if pts.windows(2).map(|s| s[0].distance(s[1])).sum::<f32>() < 1e-3 {
        return;
    }
    let prof: Vec<(Vec3, f32)> = path
        .iter()
        .copied()
        .zip(radii.iter().copied().chain(std::iter::repeat(0.0)))
        .collect();
    let waists: Vec<(usize, Mark)> = narrowings(&prof, cell)
        .into_iter()
        .map(|w| (w, Mark::Waist))
        .collect();
    let targets: Vec<Vec3> = chain_targets(&pts, path, &waists, None)
        .into_iter()
        .map(|t| t.0)
        .collect();
    move_chain(model, w, chain, &targets, modules);
}

/// The most a chain segment's fitted length scales what rides it, either way.
const RIDER_SCALE: f32 = 2.0;

/// A FAN: a bone that is one of this many or more children of its parent IN ITS OWN MODULE — a
/// hand's fingers, a foot's toes, a wing's feather groups, a head's face (the same structural
/// reading the markers rail uses, 431D08DF; never a name).
const FAN_CHILDREN: usize = 3;

/// Is `top` a FAN member or a twist helper of the module the chain root `root` belongs to? What
/// rides a chain bone in that bone's frame; a single child (a neck on a spine's top) and another
/// module's root (a thigh on the pelvis, a clavicle on the spine) only follow the bone's move.
fn rides_in_frame(model: &RawModel, modules: &[String], root: usize, top: usize) -> bool {
    if model.bones[top].name.contains("_twist_") {
        return true;
    }
    if modules.get(top) != modules.get(root) {
        return false;
    }
    let parent = model.bones[top].parent;
    let siblings = (0..model.bones.len())
        .filter(|&i| model.bones[i].parent == parent && modules.get(i) == modules.get(top))
        .count();
    siblings >= FAN_CHILDREN
}

/// MOVE A CHAIN to its targets. Everything hanging off it RIDES the chain bone it hangs from. A
/// FAN or a TWIST of the chain's own module ([`rides_in_frame`]: a hand's fingers, a foot's toes,
/// a wing's feather groups, the forearm twist) rides IN THAT BONE'S FRAME: its composed offset
/// turned the way the segment turned (the composed segment onto the fitted one, the shortest
/// way) and scaled as the segment's length scaled (within [`RIDER_SCALE`]), so a finger fan
/// points the way its hand now points and spans the hand it is on; the last joint of a chain
/// rides its own last segment. (Carried as a bare translation, the canon's A-posed fingers hung
/// off a fitted hand pointing the canon's way, not the mesh's — 2026-10-09.) Everything else — a
/// neck on the spine's top, the thighs off the pelvis, the clavicles — only follows its bone's
/// move: it is laid by its own match next, from the composed rest it was authored in, and must
/// not arrive turned.
fn move_chain(
    model: &RawModel,
    w: &mut [Mat4],
    chain: &[usize],
    targets: &[Vec3],
    modules: &[String],
) {
    let n = model.bones.len();
    let count = chain.len().min(targets.len());
    if count == 0 {
        return;
    }
    let from: Vec<Vec3> = chain[..count].iter().map(|&b| pos_of(w[b])).collect();
    let mut index = vec![usize::MAX; n];
    for (k, &b) in chain[..count].iter().enumerate() {
        index[b] = k;
    }
    // The frame of chain joint `k`: its segment on to the next joint (the last joint's is the
    // one behind it), as composed and as fitted.
    let frame = |k: usize| -> (Quat, f32) {
        let (a, b) = if k + 1 < count {
            (k, k + 1)
        } else if k > 0 {
            (k - 1, k)
        } else {
            (k, k)
        };
        let (u0, u1) = (from[b] - from[a], targets[b] - targets[a]);
        if u0.length() < 1e-3 || u1.length() < 1e-3 {
            return (Quat::IDENTITY, 1.0);
        }
        (
            Quat::from_rotation_arc(u0.normalize(), u1.normalize()),
            (u1.length() / u0.length()).clamp(1.0 / RIDER_SCALE, RIDER_SCALE),
        )
    };
    let mut moved: Vec<Option<Vec3>> = vec![None; n];
    for (k, &b) in chain[..count].iter().enumerate() {
        moved[b] = Some(targets[k]);
    }
    // Everything else rides the first chain bone above it — in its frame when the bone it
    // hangs from directly (`top`) is the module's own fan or twist.
    for i in 0..n {
        if index[i] != usize::MAX {
            continue;
        }
        let mut at = i;
        let mut hops = 0;
        while let Some(p) = (model.bones[at].parent >= 0).then(|| model.bones[at].parent as usize) {
            hops += 1;
            if index[p] != usize::MAX {
                let k = index[p];
                let (turn, scale) = if rides_in_frame(model, modules, chain[0], at) {
                    frame(k)
                } else {
                    (Quat::IDENTITY, 1.0)
                };
                moved[i] = Some(targets[k] + turn * (pos_of(w[i]) - from[k]) * scale);
                break;
            }
            at = p;
            if hops > n {
                break;
            }
        }
    }
    for (i, m) in moved.into_iter().enumerate() {
        if let Some(p) = m {
            w[i].w_axis = p.extend(1.0);
        }
    }
}

/// The rolls tried about a fan's segment, in degrees, and what a degree of roll away from the
/// composed one costs against a centimetre of a joint outside the flesh.
const FAN_ROLL_STEP_DEG: f32 = 15.0;
const FAN_ROLL_COST_PER_DEG: f32 = 0.01;

/// THE FANS A LAID CHAIN CARRIES: for each chain bone with one, `(k, the joints — every bone of
/// every fan member's subtree, the roll axis — the segment the fan rides, the pivot)`
/// ([`rides_in_frame`], [`FAN_CHILDREN`]).
fn fans_of(
    model: &RawModel,
    w: &[Mat4],
    chain: &[usize],
    modules: &[String],
) -> Vec<(usize, Vec<usize>, Vec3, Vec3)> {
    let n = model.bones.len();
    if chain.len() < 2 {
        return Vec::new();
    }
    let children = |b: usize| -> Vec<usize> {
        (0..n)
            .filter(|&i| model.bones[i].parent == b as i32)
            .collect()
    };
    let subtree = |start: usize| -> Vec<usize> {
        let mut all = vec![start];
        let mut k = 0;
        while k < all.len() {
            all.extend(children(all[k]));
            k += 1;
        }
        all
    };
    let mut fans = Vec::new();
    for (k, &b) in chain.iter().enumerate() {
        let fan: Vec<usize> = children(b)
            .into_iter()
            .filter(|&c| !chain.contains(&c) && rides_in_frame(model, modules, chain[0], c))
            .filter(|&c| {
                let parent = model.bones[c].parent;
                (0..n)
                    .filter(|&i| {
                        model.bones[i].parent == parent && modules.get(i) == modules.get(c)
                    })
                    .count()
                    >= FAN_CHILDREN
            })
            .collect();
        if fan.is_empty() {
            continue;
        }
        let (a, c) = if k + 1 < chain.len() {
            (b, chain[k + 1])
        } else {
            (chain[k - 1], b)
        };
        let axis = (pos_of(w[c]) - pos_of(w[a])).normalize_or_zero();
        if axis.length_squared() < 0.5 {
            continue;
        }
        let joints: Vec<usize> = fan.iter().flat_map(|&f| subtree(f)).collect();
        fans.push((k, joints, axis, pos_of(w[b])));
    }
    fans
}

/// WHAT A ROLL OF A FAN COSTS: its joints' distance outside the flesh, the far outside (off the
/// grid) counted as a metre, and the composed roll preferred by [`FAN_ROLL_COST_PER_DEG`].
fn fan_roll_cost(
    flesh: &Flesh,
    w: &[Mat4],
    joints: &[usize],
    pivot: Vec3,
    axis: Vec3,
    roll: f32,
) -> f32 {
    let q = Quat::from_axis_angle(axis, roll.to_radians());
    joints
        .iter()
        .map(|&j| flesh.distance_outside(pivot + q * (pos_of(w[j]) - pivot)))
        .map(|d| if d.is_finite() { d } else { 1e3 })
        .sum::<f32>()
        + FAN_ROLL_COST_PER_DEG * roll.abs()
}

/// Every roll tried: each [`FAN_ROLL_STEP_DEG`], the whole way round.
fn fan_rolls() -> impl Iterator<Item = f32> {
    let steps = (360.0 / FAN_ROLL_STEP_DEG) as i32;
    (-steps / 2..=steps / 2).map(|i| i as f32 * FAN_ROLL_STEP_DEG)
}

/// Turn a fan's joints `roll` degrees about its segment.
fn roll_fan(w: &mut [Mat4], joints: &[usize], pivot: Vec3, axis: Vec3, roll: f32) {
    if roll.abs() < 0.5 {
        return;
    }
    let q = Quat::from_axis_angle(axis, roll.to_radians());
    for &j in joints {
        w[j].w_axis = (pivot + q * (pos_of(w[j]) - pivot)).extend(1.0);
    }
}

/// SETTLE EVERY FAN OF A LAID CHAIN ON THE FLESH. The segment a fan rides fixes its direction and
/// its size but not its ROLL about that segment — which way the palm faces — and a hand is a
/// flattened tube: a fan rolled across its thickness puts the fingers outside it. So each fan of
/// the chain's own module ([`fans_of`]) is turned about its segment, through every
/// [`FAN_ROLL_STEP_DEG`], to the roll that leaves its joints least outside the flesh
/// ([`fan_roll_cost`]) — the composed roll preferred, so a fan already inside stays as composed
/// and a flip is taken only when it is clearly more inside.
fn settle_fans(
    model: &RawModel,
    w: &mut [Mat4],
    flesh: &Flesh,
    chain: &[usize],
    modules: &[String],
) {
    for (_, joints, axis, pivot) in fans_of(model, w, chain, modules) {
        let cost = |roll: f32| fan_roll_cost(flesh, w, &joints, pivot, axis, roll);
        let best = fan_rolls()
            .min_by(|&x, &y| cost(x).total_cmp(&cost(y)))
            .unwrap_or(0.0);
        roll_fan(w, &joints, pivot, axis, best);
    }
}

/// A PAIR'S FANS SETTLE AS ONE: the fan on the left chain's bone `k` and the one on the right's
/// take ONE roll, read off both sides' flesh at once — `roll` on the left and `-roll` on its
/// twin (a mirror reverses the hand of a turn) — so two hands' fingers never settle one way on
/// one side and another on the other: the flesh field is a finger's thickness coarse, and a roll
/// read off one side alone is noise either side of a symmetric body (2026-10-09). A fan with no
/// twin at its bone settles alone ([`settle_fans`]).
fn settle_fans_mirrored(
    model: &RawModel,
    w: &mut [Mat4],
    flesh: &Flesh,
    left: &[usize],
    right: &[usize],
    modules: &[String],
) {
    let (fl, fr) = (
        fans_of(model, w, left, modules),
        fans_of(model, w, right, modules),
    );
    let mut twinned = vec![false; fr.len()];
    for (k, jl, al, pl) in &fl {
        let Some(i) = fr.iter().position(|f| f.0 == *k) else {
            let cost = |roll: f32| fan_roll_cost(flesh, w, jl, *pl, *al, roll);
            let best = fan_rolls()
                .min_by(|&x, &y| cost(x).total_cmp(&cost(y)))
                .unwrap_or(0.0);
            roll_fan(w, jl, *pl, *al, best);
            continue;
        };
        twinned[i] = true;
        let (_, jr, ar, pr) = &fr[i];
        let cost = |roll: f32| {
            fan_roll_cost(flesh, w, jl, *pl, *al, roll)
                + fan_roll_cost(flesh, w, jr, *pr, *ar, -roll)
        };
        let best = fan_rolls()
            .min_by(|&x, &y| cost(x).total_cmp(&cost(y)))
            .unwrap_or(0.0);
        roll_fan(w, jl, *pl, *al, best);
        roll_fan(w, jr, *pr, *ar, -best);
    }
    for (_, jr, ar, pr) in fr.iter().zip(&twinned).filter(|(_, &t)| !t).map(|(f, _)| f) {
        let cost = |roll: f32| fan_roll_cost(flesh, w, jr, *pr, *ar, roll);
        let best = fan_rolls()
            .min_by(|&x, &y| cost(x).total_cmp(&cost(y)))
            .unwrap_or(0.0);
        roll_fan(w, jr, *pr, *ar, best);
    }
}

/// MIRROR ONE MODULE ONTO ITS TWIN: every bone of `left`'s module (its subtree in that module)
/// lands its twin — the bone of the same name with the other side's suffix — at its mirror across
/// the plane `plane_x`. The pair's two instances are the same module composed either side, so
/// the names pair one to one; a twin not found keeps its place.
fn mirror_module(
    model: &RawModel,
    w: &mut [Mat4],
    modules: &[String],
    left: usize,
    right: usize,
    plane_x: f32,
) {
    let n = model.bones.len();
    let twin = |name: &str| -> Option<usize> {
        let (stem, side) = name.rsplit_once('_')?;
        let other = match side {
            "l" => "r",
            "r" => "l",
            _ => return None,
        };
        let want = format!("{stem}_{other}");
        (0..n).find(|&i| model.bones[i].name == want)
    };
    let mut stack = vec![left];
    while let Some(i) = stack.pop() {
        if modules.get(i) == modules.get(left) {
            if let Some(j) = twin(&model.bones[i].name) {
                let p = pos_of(w[i]);
                w[j].w_axis = Vec3::new(2.0 * plane_x - p.x, p.y, p.z).extend(1.0);
            }
        }
        stack.extend((0..n).filter(|&c| model.bones[c].parent == i as i32 && c != right));
    }
}

/// The bone index of `name`.
fn bone_at(model: &RawModel, name: &str) -> Option<usize> {
    model.bones.iter().position(|b| b.name == name)
}

/// FIT THE COMPOSED REST ONTO THE MESH'S OWN SHAPE (spec 04803E0C) — [`match_recipe`], then every
/// matched module PLACED on what it matched. This is the whole of what `align_trunk` used to do
/// and more: a trunk on its core, a head DETECTED rather than composed a fraction of the stature
/// ahead of the withers, hip and shoulder pairs laid down the tubes the graph paired, tails along
/// the midline tube behind.
///
/// - THE TRUNK is moved so its first hip pair's own authored socket lands where that pair leaves
///   the core. The socket comes from the COMPOSED REST, so the croup's lift over the hips, a
///   biped's hip centre and a centaur's sub-pelvis are each the recipe's own geometry and nothing
///   measured here (rule 513E5F78: a constant may be a fraction of a module's authored geometry,
///   never an animal-shaped number). A quadruped is re-composed first at the stature and length
///   ITS OWN CORE measures; a biped keeps the typed stature, so the humanoid canon stays exact.
/// - A quadruped shifts in X and Y, a biped in X and Z — the axes each measurement speaks to,
///   which is what keeps every ground joint on the floor (the limb modules are authored from a
///   socket at a fixed fraction of the stature).
/// - Modules that matched nothing keep the composed rest and are named in
///   [`ShapeMatch::unmatched`]; the rail prompts them first.
///
/// `None` when the mesh has no graph at all (nothing solid) — the composed rest then stands,
/// exactly as it did when there was no core to measure (4BB12A75).
pub fn fit_to_graph(
    model: &mut RawModel,
    flesh: &Flesh,
    graph: &ShapeGraph,
    recipe: &SkeletonRecipe,
    stature_cm: f32,
) -> anyhow::Result<Option<(ShapeMatch, AlignReport)>> {
    let quadruped = recipe.trunk.orientation == flicker_skeletal::format::Orientation::Quadruped;
    let mut m = match_recipe(graph, recipe);
    let Some(Matched::Core(root)) = m.of(&crate::baseline::module_id("trunk", "")) else {
        tracing::warn!("fit_to_graph: no core for the root trunk — the composed rest stands");
        return Ok(None);
    };
    let core = &graph.cores[root];
    let hip_pair = m
        .of(&crate::baseline::module_id("leg", ""))
        .and_then(|w| match w {
            Matched::Pair(i) => Some(&graph.pairs[i]),
            _ => None,
        });
    let shoulder_pair = m
        .of(&crate::baseline::module_id("arm", ""))
        .and_then(|w| match w {
            Matched::Pair(i) => Some(&graph.pairs[i]),
            _ => None,
        });

    // ── A QUADRUPED'S TWO KNOBS, off its own core. The BACK over the hips is the core's
    // centreline plus its own thickness there — the same croup the belly-run rule reached for,
    // read off the shape instead of hunted for in vertical runs (89E198F4 / 3A61D440). The LENGTH
    // is simply how far apart the two pairs are: pelvis to shoulder socket IS what that knob
    // measures, and the graph knows both ends of it.
    // ── WHERE THE HIPS ARE ALONG A LYING TRUNK. A hip pair's tubes join the core where the
    // thinning happened to run them in — over the stifles, a tenth of the stature and more ahead
    // of the hip joints (measured on the seventeen hoofed sources: every one, by 0–0.14 h against
    // the core's own rear end, which itself sits a core radius inside the rump). The pelvis is
    // the END of the barrel (Aaron on the Elk, BAD0D72C: *"the hip bone is actually supposed to
    // be basically at the ass of the animal"*) — so on a trunk whose core ENDS within its own
    // thickness behind the pair, the hips are at that end, and the femur runs forward from there
    // to wherever its tube leaves the body. A core that runs on behind its hip pair (a tail thick
    // enough to be trunk) keeps the hips at their own station.
    let capped = hip_pair.is_some_and(|p| {
        quadruped
            && p.t <= 0.5
            && core.at(p.t).distance(core.rear()) <= PELVIS_CAP_RADII * core.radius
    });
    let t_pelvis = hip_pair.map_or(0.0, |p| if capped { 0.0 } else { p.t });
    let mut stature = stature_cm;
    let mut length = None;
    if quadruped {
        // THE REAR OF THE CORE, not wherever a pair happened to match: the croup is an end of the
        // barrel and the barrel is the one thing the graph is sure of. A body whose hind pair the
        // graph could not separate (the Horse: its hind legs arrive tangled with its tail hair)
        // would otherwise measure its stature off its own withers crest.
        let t_hip = hip_pair.map_or(0.0, |p| if p.t <= 0.5 { p.t } else { 0.0 });
        // THE BACK over the hips: the top of the solid column the CORE ITSELF runs through, so a
        // mane or an ear floating over it is never read as the back. The core's own centreline is
        // what picks that column out of the vertical stack — where the old rule had to hunt for a
        // belly run first to find it (89E198F4).
        let on_core = core.at(t_hip);
        let back = flesh
            .runs(on_core, 2)
            .into_iter()
            .find(|r| r.0 <= on_core.z && r.1 >= on_core.z)
            .map_or(on_core.z + core.radius_at(t_hip), |r| r.1);
        if back > 1.0 {
            stature = back / crate::baseline::QUAD_WITHERS;
        }
        let mut fitted = recipe.clone();
        // THE LENGTH is pelvis to shoulder socket — the two pairs when both matched, and the
        // CORE'S OWN SPAN when they did not: the barrel is exactly the span that knob measures,
        // and leaving the authored default in place composes a horse's withers into its nose.
        let (a, b) = match (hip_pair, shoulder_pair) {
            (Some(_), Some(sh)) => (core.at(t_pelvis), core.at(sh.t)),
            _ => (core.at(t_hip), core.front()),
        };
        fitted.trunk.length = (a.distance(b) / stature).clamp(0.2, 2.0);
        length = Some(fitted.trunk.length);
        m.warnings.push(format!(
            "quadruped knobs off the core: stature {stature:.1} length {:.3}",
            fitted.trunk.length
        ));
        install_skeleton(model, &fitted, stature)?;
    }

    let mut w = model_world_frames(model);
    // WHICH MODULE EACH BONE CAME FROM — what tells a chain's own fan (the fingers on a hand)
    // from another module's root riding the same bone (a clavicle on the spine's top).
    let modules: Vec<String> = {
        let by_name: HashMap<String, String> =
            crate::baseline::compose_with_modules(recipe, stature_cm)
                .map(|(bones, mods)| {
                    bones
                        .into_iter()
                        .zip(mods)
                        .map(|(b, m)| (b.name, m))
                        .collect()
                })
                .unwrap_or_default();
        model
            .bones
            .iter()
            .map(|b| by_name.get(&b.name).cloned().unwrap_or_default())
            .collect()
    };
    // ── THE TRUNK, by its first hip pair's own socket. With no hip pair at all the core's
    // rear/bottom end is the anchor — a legless body still has an end to be placed by.
    let socket = |w: &[Mat4], prefix: &str, base: &str| -> Option<Vec3> {
        let l = bone_at(model, &format!("{prefix}{base}_l"))?;
        let r = bone_at(model, &format!("{prefix}{base}_r"))?;
        Some(0.5 * (pos_of(w[l]) + pos_of(w[r])))
    };
    let from = socket(&w, "", "thigh")
        .unwrap_or_else(|| bone_at(model, "pelvis").map_or(Vec3::ZERO, |i| pos_of(w[i])));
    // WHERE THE PAIR LEAVES THE BODY — the mean of its two tubes' ATTACHMENTS, which is the
    // socket the spec places a module by ("the socket at the attachment", 04803E0C §3), and the
    // core's rear/bottom end when no pair matched at all.
    let attach = |p: &crate::shape::Pair| 0.5 * (graph.limbs[p.l].at + graph.limbs[p.r].at);
    let to = hip_pair
        .filter(|p| p.t <= 0.5 && !capped)
        .map_or_else(|| core.rear(), attach);
    // A BIPED'S HEIGHT comes off the same junction: where a standing body's legs PART is the one
    // height its own shape states, and the canon's pelvis rides the authored
    // [`crate::baseline::BIPED_PELVIS_OVER_CROTCH`] of the stature above it (Drillis–Contini —
    // the fractions the whole canon is composed from, so this is the recipe's own geometry and
    // not a number tuned to an animal). Its stature stays TYPED: a biped's height IS its height.
    let pelvis_now = bone_at(model, "pelvis").map_or(from, |i| pos_of(w[i]));
    // THE CROTCH IS READ OFF THE FLESH, not off the graph — [`crotch_on_the_plane`] says why the
    // graph cannot state it. Where there is no gap to read (a plinth, a robe) the pair's own
    // attachment is still the best the body offers, which is what a limbless trunk falls back to.
    // AND IT IS NEVER BELOW THE TRUNK ITSELF. The gap between a pair of legs is open all the way
    // to the floor, so the first solid thing the walk meets on the plane can be a toe crossing it
    // or a tail brushing it — on a bird, whose legs are long and whose body starts high, that put
    // the crotch at 12 cm and the pelvis under the bird (the Rooster, the Raptor, the Bat, all of
    // which had their pelvis inside before the reader was consulted at all). The trunk's own core
    // ENDS where the trunk ends, so the crotch cannot be below it.
    // FOUR LOWER BOUNDS, and the crotch is the HIGHEST of them: below any one of them the legs
    // have certainly not parted yet, so the highest is the first height at which they all have.
    // Each answers a body the others get wrong.
    //   · the GAP's top on the plane — the reading that matters on a body whose thick flesh runs
    //     on down inside its thighs, where everything the graph offers sits below the fork (all
    //     seven promoted humanoids: 18 cm low, and the pelvis outside the flesh);
    //   · the hip pair's own ATTACHMENT, and the core's own point beside it — the readings that
    //     matter where the legs are separate tubes all the way up into the trunk and the gap's
    //     top is only the torso's lower face (the biped fixture, 2 cm under the tubes' tops);
    //   · the bottom of the trunk's own CORE — the reading that matters where the gap runs all
    //     the way to the floor and the walk up the plane trips on a toe crossing it (the birds
    //     and the Bat, whose crotch read 12 cm and whose pelvis went under the bird).
    let crotch = (!quadruped)
        .then(|| {
            let (blo, bhi) = bbox(model);
            crotch_on_the_plane(flesh, graph.plane_x, blo.y, bhi.y, blo.z, bhi.z)
        })
        .flatten()
        .unwrap_or(f32::MIN)
        .max(to.z)
        .max(core.rear().z)
        .max(hip_pair.map_or(f32::MIN, |p| core.at(p.t).z));
    let pelvis_to = crotch + crate::baseline::BIPED_PELVIS_OVER_CROTCH * stature;
    let shift = Vec3::new(
        graph.plane_x - from.x,
        if quadruped { to.y - from.y } else { 0.0 },
        if quadruped {
            0.0
        } else {
            pelvis_to - pelvis_now.z
        },
    );
    for f in &mut w {
        f.w_axis += shift.extend(0.0);
    }
    // THE COMPOSED REST ON THE BODY — the recipe at the fitted knobs, moved onto the trunk, before
    // any chain is laid: where each limb module's root joint is authored to hang, its SOCKET
    // ([`from_the_tip`]). After the trunk's own chain is laid down its core, a quadruped's pelvis
    // sits on the core's centreline and a hip composed a third of the stature under it hangs out of
    // the belly — the socket is read here, where the rest still stands on the back it was measured
    // off.
    let rest = w.clone();

    // ── THE TRUNK'S OWN CHAIN, LAID DOWN ITS CORE. A trunk on a LYING core carries its spine as
    // a straight run of the authored `length` knob along the back (`TrunkSpec::length` — "a
    // biped's spine is plumb and ignores it"), and the body's own back IS that core: so the chain
    // is laid down the core's centreline exactly as a leg is laid down its tube (spec 04803E0C
    // §3, "the chain laid along the tube").
    //
    // This is what a rigid shift could never do. One stature knob states ONE height, so a body
    // whose withers sit below its croup — a beaver, a rat, a panther, a goat — has `spine_03`
    // composed at the croup's height over its own shoulders, in the air. Measured on the 32
    // swept quadrupeds that was 25 of them: the single widest failure in the sweep.
    //
    // A trunk on an UPRIGHT core is laid along it too, from the pelvis the crotch placed and at
    // the chain's OWN composed length — the canon's spine is plumb because the canon's body is,
    // and a body that LEANS carries its spine where its flesh goes: the Owl leans its upright core
    // 30° forward, and the plumb spine left `spine_03` behind its own back (0E38BE60). On a plumb
    // body the core IS plumb and the chain lands where the canon put it.
    let a_trunk_to_lay_it_on =
        core.radius >= 2.0 * graph.cell && core.arc >= TRUNK_RUN_RADII * core.radius;
    if a_trunk_to_lay_it_on {
        let chain = spine_chain(model, "");
        let span = if core.upright {
            upright_span(
                core,
                &chain.iter().map(|&i| pos_of(w[i])).collect::<Vec<_>>(),
            )
        } else {
            let t0 = t_pelvis;
            let t1 = shoulder_pair.map_or(1.0, |p| p.t);
            core_span(core, t0, t1)
        };
        if let Some((path, radii)) = span {
            lay_chain(model, &mut w, &chain, &path, &radii, flesh.cell(), &modules);
        }
    }

    // A LIMB BEGINS WHERE IT LEAVES THE TRUNK. The graph's lead starts at the JUNCTION, which
    // sits on the core's own centreline — on the midline. Laying a chain from there would put
    // both thighs of a pair at x = 0, inside the body. The limb proper is the run of the lead
    // outside the core's own inscribed ball at that point: radius-relative, so the same trim
    // reads a mouse's leg and an elephant's.
    // ...AND A LIMB ENDS WHERE ITS FLESH ENDS (`onward`, which `trim` runs after it). Thinning
    // stops about an inscribed radius short of a tube's cap — that is what it is for — so the
    // skeleton's last cell is inside the hoof, not on the ground under it.
    let onward = |mut path: Vec<Vec3>, mut radii: Vec<f32>, l: &crate::shape::Limb| {
        // The FLESH'S OWN TUBE TRACER carries the tip on
        // ([`Flesh::trace`], 3995EF9E): each step re-centres a step ahead on the local medial
        // point, so it follows the limb round its own bend for as far as the limb is long and
        // stops where the tube collapses.
        //
        // A STRAIGHT RAY used to do this and it leaves the leg the moment the leg bends, which is
        // why one side of a matched pair reached the floor while the other stopped a hand's
        // breadth up: measured across the sweep, 1.8 cm against 16.3 on the Horse, 1.8 / 16.4 on
        // the Camel, 2.0 / 14.3 on the Sheep — always the side whose lead ended on a bend.
        //
        // AND IT ONLY EVER GOES ON OUTWARD. The tracer follows the thickest flesh ahead of it, and
        // at the cap of a short limb that is the body the limb hangs from: left to run its full
        // length it turns the corner and walks back up the belly, and because a chain is spread
        // over the WHOLE path the joints then ride up with it (measured: the Rabbit's hind balls
        // went from 4.6 cm off the floor to 30.6). A tip extension that does not get FURTHER FROM
        // THE ATTACHMENT than the tip already was is not the end of this limb, and stops it.
        if let Some(&end) = path.last() {
            let back = path[path.len().saturating_sub(2)];
            let on = flesh.trace(end, (end - back).normalize_or_zero(), flesh.cell(), l.arc);
            let mut far = end.distance(l.at);
            for (p, r) in on {
                if p.distance(l.at) <= far {
                    break;
                }
                far = p.distance(l.at);
                path.push(p);
                radii.push(r);
            }
        }
        (path, radii)
    };
    let trim = |l: &crate::shape::Limb| -> (Vec<Vec3>, Vec<f32>) {
        // The core's OWN thickness (its median), not the local radius at the junction: a binned
        // centreline's last slices are always thin, and a trim measured against those trims
        // nothing at all — both thighs of a pair then land on the midline.
        let k = leaves_core(l, graph.cores[l.core].radius)
            .unwrap_or(0)
            .min(l.lead.len().saturating_sub(2));
        onward(l.lead[k..].to_vec(), l.lead_r[k..].to_vec(), l)
    };

    // ── EVERY MATCHED MODULE, placed on what it matched.
    let trunks: Vec<usize> = m
        .matched
        .iter()
        .filter_map(|(id, what)| match what {
            Matched::Core(c) if id.starts_with("trunk:") => Some(*c),
            _ => None,
        })
        .collect();
    for (id, what) in m.matched.clone() {
        let Some((kind, prefix)) = id.split_once(':') else {
            continue;
        };
        match (kind, what) {
            // THE HEAD IS DETECTED — where the neck leads, followed wherever it bends. This is
            // the column 20 of the 32 swept quadrupeds failed — a pig's, a rat's and a bear's
            // skull is nowhere near a fixed fraction of the stature ahead of the withers.
            ("head", Matched::Core(c)) => {
                follow_the_neck(model, &mut w, graph, &trunks, prefix, Neck::Core(c));
            }
            ("head", Matched::Limb(l)) => {
                follow_the_neck(model, &mut w, graph, &trunks, prefix, Neck::Tube(l));
            }
            // THE PROBOSCIS, down the tube that runs on past the head's mass — from where it
            // leaves that mass to the tube's own tip ([`NeckTube::cut`], carried on to the end
            // of the flesh as every limb is) — or down the whole of a skull's own tube. The
            // joints are spaced evenly along it: the root where it leaves the head, the last a
            // link short of the tip, which the last bone reaches.
            ("proboscis", Matched::Limb(l)) => {
                let stem = if prefix.is_empty() {
                    "proboscis".to_string()
                } else {
                    prefix.trim_end_matches('_').to_string()
                };
                let Some(root) = bone_at(model, &format!("{stem}_01")) else {
                    continue;
                };
                let limb = &graph.limbs[l];
                let run = NeckTube::of(graph, l).and_then(|t| {
                    let cut = t.cut?;
                    (t.path.len() > cut + 1 && t.radii.len() > cut + 1)
                        .then(|| onward(t.path[cut..].to_vec(), t.radii[cut..].to_vec(), limb))
                });
                let (path, _) = run.unwrap_or_else(|| trim(limb));
                let prof: Vec<(Vec3, f32)> = path.iter().map(|&p| (p, 0.0)).collect();
                let cum = arc_lengths(&prof);
                let total = cum.last().copied().unwrap_or(0.0);
                let chain = deep_chain(model, root);
                if path.len() < 2 || total <= graph.cell {
                    unmatch(&mut m, &id);
                    continue;
                }
                let targets: Vec<Vec3> = (0..chain.len())
                    .map(|k| along_path(&prof, &cum, total * k as f32 / chain.len() as f32))
                    .collect();
                move_chain(model, &mut w, &chain, &targets, &modules);
            }
            ("tail", Matched::Limb(l)) => {
                let stem = if prefix.is_empty() {
                    "tail".to_string()
                } else {
                    prefix.trim_end_matches('_').to_string()
                };
                if let Some(root) = bone_at(model, &format!("{stem}_01")) {
                    let (path, radii) = trim(&graph.limbs[l]);
                    let chain = deep_chain(model, root);
                    let pts: Vec<Vec3> = chain.iter().map(|&i| pos_of(w[i])).collect();
                    let core = &graph.cores[graph.limbs[l].core];
                    let socket = Some(pos_of(rest[root])).filter(|&s| {
                        path.len() > 1 && beyond(s, path[0], path[path.len() - 1]) > 0.0
                    });
                    let got = from_the_tip(
                        flesh,
                        core,
                        &pts,
                        socket,
                        &path,
                        &radii,
                        Some(ATTACHMENT_FIT),
                    );
                    probe_dump(
                        model,
                        &id,
                        &chain,
                        &pts,
                        socket,
                        &path,
                        &radii,
                        flesh,
                        core,
                        got.as_deref(),
                        graph.limbs[l].at,
                        &graph.limbs[l].lead,
                        &graph.limbs[l].lead_r,
                    );
                    match got {
                        Some(targets) => {
                            move_chain(model, &mut w, &chain, &targets, &modules);
                            settle_fans(model, &mut w, flesh, &chain, &modules);
                        }
                        None => unmatch(&mut m, &id),
                    }
                }
            }
            // A PAIR — a hip pair, a shoulder pair, a pair of wings: each side laid from the tip of
            // the tube or the sheet's leading path the graph paired it with, on into the core to
            // its socket ([`from_the_tip`]), both tubes leaving the body at the ONE SEAT the pair
            // shares ([`pair_socket`]). The chain is the MODULE's, from its own root (a leg's
            // `thigh`, an arm's `clavicle`): a joint the tube does not hold is still the module's,
            // and lies inside the core.
            (_, Matched::Pair(i)) => {
                let pair = &graph.pairs[i];
                let start = |l: usize| trim(&graph.limbs[l]).0.first().copied();
                let seat = start(pair.l)
                    .zip(start(pair.r))
                    .map(|(l, r)| pair_socket(l, r, graph.plane_x));
                let base = if kind == "leg" { "thigh" } else { "clavicle" };
                let mut sides = Vec::new();
                for (side, limb, seat) in [
                    ("l", pair.l, seat.map(|s| s.0)),
                    ("r", pair.r, seat.map(|s| s.1)),
                ] {
                    let Some(root) = bone_at(model, &format!("{prefix}{base}_{side}"))
                        .or_else(|| bone_at(model, &format!("{prefix}upperarm_{side}")))
                    else {
                        continue;
                    };
                    let (path, radii) = trim(&graph.limbs[limb]);
                    let socket = pos_of(rest[root]);
                    sides.push((root, limb, seat, path, radii, socket));
                }
                // ONE DECISION PER PAIR: both roots inside the core at the socket they share (the
                // composed rest is the pair's mirror), or both at the seat their tubes share — a
                // pair split between the two has two sockets, and squaring one side onto the other
                // then drags a hip across the body.
                let into = sides
                    .iter()
                    .filter_map(|s| Some(beyond(s.5, s.2?, *s.3.last()?)))
                    .sum::<f32>()
                    > 0.0;
                // EACH SIDE LAID EVERY WAY — the joint its tube fits at the attachment
                // ([`ATTACHMENT_FIT`]), the joint it fits BEST there however loosely, and no
                // joint there at all — because a tube that leaves its core between two joints
                // (below the stifle, above the hock) reads as any of them, and one limb cannot
                // say which.
                let mut laid = Vec::new();
                for (root, limb, seat, mut path, radii, socket) in sides {
                    if let (false, Some(seat), Some(first)) = (into, seat, path.first_mut()) {
                        *first = seat;
                    }
                    let chain = deep_chain(model, root);
                    let pts: Vec<Vec3> = chain.iter().map(|&i| pos_of(w[i])).collect();
                    let core = &graph.cores[graph.limbs[limb].core];
                    let socket = into.then_some(socket);
                    let ways: Option<Vec<Vec<Vec3>>> =
                        [Some(ATTACHMENT_FIT), Some(f32::INFINITY), None]
                            .into_iter()
                            .map(|attach| {
                                from_the_tip(flesh, core, &pts, socket, &path, &radii, attach)
                            })
                            .collect();
                    laid.push(ways.map(|ways| (chain, pts, socket, path, radii, limb, ways)));
                }
                // A PAIR IS ONE MODULE: a side whose tube is too short to say where its limb goes
                // leaves the whole module to the rail.
                match laid.into_iter().collect::<Option<Vec<_>>>() {
                    Some(sides) => {
                        // A PAIR'S TWO LIMBS HAVE THE SAME BONES. Of the ways each side was laid,
                        // the two taken are the two whose bones below the root agree best in
                        // length (the root's own bone runs to a socket the pair shares, and says
                        // nothing) — the mesh's own symmetry settles what one tube's shape cannot
                        // (measured: a hock a hand high on one hind leg and true on the other,
                        // 2026-09-30). A lone side, or sides that agree as well any way, keep
                        // the first — the fit's own reading.
                        let bones = |t: &[Vec3]| -> Vec<f32> {
                            t.windows(2).skip(1).map(|b| b[0].distance(b[1])).collect()
                        };
                        let apart = |a: &[Vec3], b: &[Vec3]| -> f32 {
                            bones(a)
                                .iter()
                                .zip(bones(b))
                                .map(|(x, y)| (x.max(1e-3) / y.max(1e-3)).ln().powi(2))
                                .sum()
                        };
                        let mut pick = vec![0usize; sides.len()];
                        if let [l, r] = &sides[..] {
                            // ...and a way that leaves its own tube — a bone cutting the corner
                            // of a bend no joint took — pays for it ([`strays`]): two hocks laid
                            // a hand high agree with each other perfectly.
                            let off = |side: &(_, _, _, Vec<Vec3>, Vec<f32>, _, Vec<Vec<Vec3>>),
                                       t: &[Vec3]| {
                                let from = side
                                    .6
                                    .iter()
                                    .map(|way| first_on_tube(&side.3, &side.4, way))
                                    .max()
                                    .unwrap_or(0);
                                PAIR_STRAY
                                    * (strays(&side.3, &side.4, t, from) - STRAY_NOISE).max(0.0)
                            };
                            let mut best = f32::INFINITY;
                            for (i, a) in l.6.iter().enumerate() {
                                for (j, b) in r.6.iter().enumerate() {
                                    // The first way wins a tie: a hundredth is under any bone's
                                    // real disagreement.
                                    let d =
                                        apart(a, b) + off(l, a) + off(r, b) + 0.01 * (i + j) as f32;
                                    if d < best {
                                        (best, pick) = (d, vec![i, j]);
                                    }
                                }
                            }
                        }
                        // A PAIR ON A SYMMETRIC BODY IS LAID AS ONE, joint by joint, where the
                        // mesh is one tube mirrored ([`lay_as_one`]).
                        let mut chosen: Vec<Vec<Vec3>> = sides
                            .iter()
                            .zip(&pick)
                            .map(|(s, &way)| s.6[way].clone())
                            .collect();
                        let mut as_one = false;
                        if let [l, r] = &sides[..] {
                            let (got, of) = lay_as_one(
                                [(&l.3, &l.4, l.2), (&r.3, &r.4, r.2)],
                                &mut chosen,
                                graph.plane_x,
                                flesh,
                                first_fan(model, &l.0, &modules),
                                graph.limbs[pair.l].grounded && graph.limbs[pair.r].grounded,
                            );
                            as_one = of > 0 && got == of;
                            if as_one {
                                m.warnings
                                    .push(format!("{id}: a mirrored pair, laid as one"));
                            } else if got > 0 {
                                m.warnings.push(format!(
                                    "{id}: a pair laid as one at {got} of {of} joints, each \
                                     its own way at the rest"
                                ));
                            }
                        }
                        let roots: Vec<usize> = sides.iter().map(|s| s.0[0]).collect();
                        let mut chains = Vec::new();
                        for ((chain, pts, socket, path, radii, limb, _ways), targets) in
                            sides.into_iter().zip(chosen)
                        {
                            let targets = &targets;
                            probe_dump(
                                model,
                                &id,
                                &chain,
                                &pts,
                                socket,
                                &path,
                                &radii,
                                flesh,
                                &graph.cores[graph.limbs[limb].core],
                                Some(targets),
                                graph.limbs[limb].at,
                                &graph.limbs[limb].lead,
                                &graph.limbs[limb].lead_r,
                            );
                            move_chain(model, &mut w, &chain, targets, &modules);
                            chains.push(chain);
                        }
                        // A PAIR'S FANS ROLL AS ONE: a hand's fingers settle on the flesh of
                        // both hands at once, one roll mirrored ([`settle_fans_mirrored`]).
                        match &chains[..] {
                            [l, r] => settle_fans_mirrored(model, &mut w, flesh, l, r, &modules),
                            _ => {
                                for chain in &chains {
                                    settle_fans(model, &mut w, flesh, chain, &modules);
                                }
                            }
                        }
                        // A PAIR LAID AS ONE IS ONE TO THE LAST JOINT: the right module's every
                        // bone is the mirror of the left's.
                        if let (true, [l, r]) = (as_one, &roots[..]) {
                            mirror_module(model, &mut w, &modules, *l, *r, graph.plane_x);
                        }
                    }
                    None => unmatch(&mut m, &id),
                }
            }
            _ => {}
        }
    }
    // A CAPPED HEAD on its trunk's front end — the centroid of the core's last slice — with the
    // neck run up the core to it from where the spine was laid (the shoulder attachment).
    for (id, c) in &m.capped {
        let prefix = id.split_once(':').map_or("", |(_, p)| p);
        follow_the_neck(model, &mut w, graph, &trunks, prefix, Neck::Cap(*c));
    }
    write_world_frames(&mut model.bones, &w);
    let at = |name: &str| bone_at(model, name).map(|i| pos_of(w[i]));
    let align = AlignReport {
        plane_x: graph.plane_x,
        pelvis: at("pelvis").unwrap_or(to),
        withers: quadruped.then(|| at("spine_03")).flatten(),
        stature,
        length,
    };
    Ok(Some((m, align)))
}

/// THE TUBE DUMP — a diagnostic, off unless `FLICKER_TUBE_DUMP` names a file: one JSON line per
/// limb laid ([`fit_to_graph`]) with the chain as composed, its socket, the tube and its radii,
/// the landmarks read on it, the places the joints took, and the trunk core's two ends. What a
/// family sweep is drawn and measured from (rule CE0451CE: a fit is proven on the family, not on
/// the one body it was written against).
#[allow(clippy::too_many_arguments)]
fn probe_dump(
    model: &RawModel,
    id: &str,
    chain: &[usize],
    pts: &[Vec3],
    socket: Option<Vec3>,
    path: &[Vec3],
    radii: &[f32],
    flesh: &Flesh,
    core: &crate::shape::Core,
    got: Option<&[Vec3]>,
    at: Vec3,
    lead: &[Vec3],
    lead_r: &[f32],
) {
    let Ok(file) = std::env::var("FLICKER_TUBE_DUMP") else {
        return;
    };
    use std::io::Write;
    let v = |p: Vec3| format!("[{:.3},{:.3},{:.3}]", p.x, p.y, p.z);
    let vs = |ps: &[Vec3]| ps.iter().map(|&p| v(p)).collect::<Vec<_>>().join(",");
    let fs = |rs: &[f32]| {
        rs.iter()
            .map(|r| format!("{r:.3}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    let names: Vec<String> = chain
        .iter()
        .map(|&i| format!("\"{}\"", model.bones[i].name))
        .collect();
    let marks: Vec<String> = landmarks(path, radii, flesh.cell())
        .iter()
        .map(|&(i, k)| format!("[{i},\"{}\"]", if k == Mark::Bend { "B" } else { "W" }))
        .collect();
    let line = format!(
        "{{\"id\":\"{id}\",\"chain\":[{}],\"pts\":[{}],\"socket\":{},\"path\":[{}],\"radii\":[{}],\"cell\":{},\"core_r\":{},\"got\":{},\"at\":{},\"lead\":[{}],\"lead_r\":[{}],\"marks\":[{}],\"core_rear\":{},\"core_front\":{}}}\n",
        names.join(","),
        vs(pts),
        socket.map_or("null".into(), v),
        vs(path),
        fs(radii),
        flesh.cell(),
        core.radius,
        got.map_or("null".into(), |g| format!("[{}]", vs(g))),
        v(at),
        vs(lead),
        fs(lead_r),
        marks.join(","),
        v(core.rear()),
        v(core.front()),
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// What a way of laying a pair's limb pays, per unit of [`strays`] over [`STRAY_NOISE`], against
/// what its bones' disagreement with its twin's costs (a squared log of a length ratio).
const PAIR_STRAY: f32 = 0.1;

/// The [`strays`] a thinned path's own wander reads on a limb whose bones follow it truly —
/// under this a way is not charged at all. Measured on the hoofed sources: a path two cells
/// thick wanders up to 3–5 off straight bones (and charging that made a way that hung an elbow
/// on a forearm's wobble the cheaper one); a bone cutting the corner of a hock reads 12–160.
const STRAY_NOISE: f32 = 6.0;

/// Where along a tube's lead `path` a chain `laid` on it first has a joint ON the tube — the
/// path index nearest the first joint below the root that lies within the tube's own (median)
/// radius of the path; 0 when none does.
fn first_on_tube(path: &[Vec3], radii: &[f32], laid: &[Vec3]) -> usize {
    let r = crate::shape::median(radii).max(1e-3);
    let nearest = |p: Vec3| {
        (0..path.len())
            .map(|i| (i, path[i].distance(p)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    };
    laid.iter()
        .skip(1)
        .filter_map(|&j| nearest(j))
        .find(|hit| hit.1 <= r)
        .map_or(0, |hit| hit.0)
}

/// HOW FAR A TUBE'S OWN AXIS STRAYS FROM THE BONES LAID DOWN IT: the squared distance from each
/// point of the tube's lead `path` (from index `from` to its tip) to the nearest bone of the
/// chain `laid`, summed along the path, in the tube's own (median) `radii` cubed — so the same
/// bend missed on a mouse's leg and an elephant's reads the same. Zero where every bend of the
/// tube has a joint on it. `from`: the stretch every way being compared has joints on
/// ([`first_on_tube`]) — above it a tube is crossed by a bone that comes out of the core (a
/// tibia from a stifle in the flank), off the tube's axis by design and not by mistake, and a
/// way must not read as truer for laying fewer joints on its tube.
fn strays(path: &[Vec3], radii: &[f32], laid: &[Vec3], from: usize) -> f32 {
    let r = crate::shape::median(radii).max(1e-3);
    let to_bone = |p: Vec3, a: Vec3, b: Vec3| {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        p.distance(a + ab * t)
    };
    let mut total = 0.0;
    for i in from..path.len() {
        let p = path[i];
        let d = laid
            .windows(2)
            .map(|b| to_bone(p, b[0], b[1]))
            .fold(f32::INFINITY, f32::min);
        let before = if i > from {
            path[i - 1].distance(p)
        } else {
            0.0
        };
        let after = path.get(i + 1).map_or(0.0, |q| q.distance(p));
        if d.is_finite() {
            total += d * d * 0.5 * (before + after);
        }
    }
    total / (r * r * r)
}

/// THE ONE SOCKET A SYMMETRY PAIR SHARES, as the two sides' own seats: the mean of the `+x`
/// tube's start and the `−x` tube's start reflected across the body's symmetry plane, reflected
/// back for the `−x` side. `l` and `r` are the two tubes' starts (their junctions with the core).
///
/// **A PAIR HAS ONE ATTACHMENT** — spec 04803E0C §2 defines the pair as two tubes "attached
/// within a small arc distance of each other", and §3 seats a matched module at "the socket at
/// the attachment", singular. Seating each side at ITS OWN tube's junction instead splits that
/// socket in two, and on a MID-STRIDE source the swinging limb's tube merges into the body well
/// ahead of its twin's: the Ram's two hips landed 27.7 cm apart along the barrel. Nothing in the
/// fit minds — but `bake::square_stance` is then asked to put one hip where the other is, which
/// is not a pose at all, and the skin follow carried the trunk and the tail's fall with it until
/// the mesh hung 33.7 cm below the floor it was standing on (incident 7CF34E04).
///
/// Only the SEAT is shared. Every joint below it still rides its own side's tube, so a limb that
/// really is longer or more bent than its twin keeps its own shape — what a pair shares is where
/// it hangs off the body, not the pose of what hangs there.
fn pair_socket(l: Vec3, r: Vec3, plane_x: f32) -> (Vec3, Vec3) {
    let across = |p: Vec3| Vec3::new(2.0 * plane_x - p.x, p.y, p.z);
    let mean = (l + across(r)) * 0.5;
    (mean, across(mean))
}

/// A matched module the placement could not lay ([`from_the_tip`]: its tube too short to say where
/// the limb goes) keeps the composed rest and goes back to the rail, like any module nothing
/// answered.
fn unmatch(m: &mut ShapeMatch, id: &str) {
    m.matched.retain(|(x, _)| x != id);
    m.unmatched.push(id.to_string());
}

/// WHERE A LIMB LEAVES ITS CORE — the index of the first point of its lead at least `r` (the
/// core's own thickness) from its attachment; `None` when the whole lead stays inside that ball.
/// A lead starts at a junction deep in the core, and the run before this is trunk flesh.
fn leaves_core(l: &crate::shape::Limb, r: f32) -> Option<usize> {
    l.lead.iter().position(|p| p.distance(l.at) >= r)
}

/// A NECK TUBE READ FOR ITS HEAD — the branch of the tube the neck is followed along, and where
/// on it the head's MASS ends and an APPENDAGE begins. A tube leaving the trunk's front end can
/// run on PAST the head: a trunk-that-is-a-nose, a pair of tall ears, a beak, a horn. None of
/// those is the head, and followed to its end each one carried the head off with it (the
/// Elephant's 107 cm down its proboscis, the Rabbit's 63 cm up its ear — A31C0FAE). Read off
/// nothing but the path's own flesh: an appendage TAPERS ON — thinner than half
/// ([`crate::shape::CORE_FRACTION`], the graph's own thick/thin cut) the thickest flesh behind
/// it, never swelling again, for longer than [`crate::shape::SPUR_RADII`] of that flesh (the
/// graph's own limb-versus-bump reach). A bare head, where the path ends on its own mass, has no
/// such tail.
struct NeckTube {
    /// The branch, from the attachment: its points and the inscribed radius at each — as read,
    /// and lightly smoothed (a three-point mean, so one voxel's step is not a swelling).
    path: Vec<Vec3>,
    radii: Vec<f32>,
    smooth: Vec<f32>,
    /// The first point of it outside the trunk core's own ball ([`leaves_core`]).
    exit: usize,
    /// Where the branch tapers away measured against ALL the flesh behind it, the trunk's own
    /// included — before `exit` when it is thin from inside the trunk's ball.
    thin: Option<usize>,
    /// Where the APPENDAGE begins — measured against the flesh the tube carries OUT of the
    /// trunk's ball, so a head thinner than half the trunk still counts as a mass; `None` when
    /// the path ends on its head.
    cut: Option<usize>,
}

impl NeckTube {
    /// Limb `l` of `graph` read as a neck: its branch that carries the most FLESH (Σ r²·ds over the
    /// lead and the fan branches parting from it outside the trunk core and ending at least half
    /// its reach out — A31C0FAE: the lead runs to whichever end reaches FARTHEST, an ear or a
    /// strand of mane), and that branch's appendage.
    fn of(graph: &ShapeGraph, l: usize) -> Option<NeckTube> {
        let limb = graph.limbs.get(l)?;
        let exit = leaves_core(limb, graph.cores[limb.core].radius)?;
        let reach = limb.tip().distance(limb.at);
        let flesh = |(p, r): &(&Vec<Vec3>, &Vec<f32>)| -> f32 {
            p.windows(2)
                .zip(r.iter().skip(1))
                .map(|(s, r)| r * r * s[0].distance(s[1]))
                .sum()
        };
        let branches = limb.fan.iter().map(|(p, r)| (p, r)).filter(|(p, _)| {
            limb.lead
                .iter()
                .zip(p.iter())
                .take_while(|(a, b)| a == b)
                .count()
                > exit
                && p.last().is_some_and(|e| e.distance(limb.at) >= 0.5 * reach)
        });
        let (path, radii) = std::iter::once((&limb.lead, &limb.lead_r))
            .chain(branches)
            .max_by(|a, b| flesh(a).total_cmp(&flesh(b)))
            .map(|(p, r)| (p.clone(), r.clone()))?;
        let n = radii.len().min(path.len());
        let smooth: Vec<f32> = (0..n)
            .map(|i| {
                let (lo, hi) = (i.saturating_sub(1), (i + 1).min(n - 1));
                radii[lo..=hi].iter().sum::<f32>() / (hi - lo + 1) as f32
            })
            .collect();
        let thin = appendage(&path, &smooth, graph.cell, 0);
        let cut = appendage(&path, &smooth, graph.cell, exit);
        Some(NeckTube {
            path,
            radii,
            smooth,
            exit,
            thin,
            cut,
        })
    }

    /// Does the head LEAVE the trunk? A tube already tapering away inside the trunk core's own
    /// ball — thin against the trunk's flesh from where it leaves the front end, all of it
    /// tapering on — carries no head out of the body: its head is the trunk's own front (the
    /// Rabbit's ears off a skull the trunk core holds; that skull is the FRONT CAP, 0E38BE60).
    fn carries_head(&self) -> bool {
        self.thin.is_none_or(|c| c > self.exit)
    }

    /// THE HEAD'S MASS when an appendage runs on past it: the last mass the path carries before
    /// the appendage — walked back from it while the flesh keeps rising or holds (within half a
    /// cell, the grid's own noise), to the narrowing behind it (the neck) or the trunk's ball —
    /// as the r²-weighted centroid of the samples within a tenth of that mass's thickest (the
    /// prominence floor [`narrowings`] reads a joint by), and its arc along the branch. `None`
    /// without an appendage.
    fn head(&self, cell: f32) -> Option<(Vec3, f32, f32)> {
        let cut = self.cut?;
        let r = &self.smooth;
        let (mut top, mut lo) = (cut - 1, cut - 1);
        while lo > self.exit && r[lo - 1] >= r[top] - 0.5 * cell {
            lo -= 1;
            if r[lo] > r[top] {
                top = lo;
            }
        }
        let cum = self.arcs();
        let (mut at, mut s, mut w) = (Vec3::ZERO, 0.0_f32, 0.0_f32);
        for i in (lo..cut).filter(|&i| r[i] >= 0.9 * r[top]) {
            let m = r[i] * r[i];
            at += self.path[i] * m;
            s += cum[i] * m;
            w += m;
        }
        // The centroid, its arc, and the arc where the mass BEGINS (the neck's end).
        (w > 0.0).then(|| (at / w, s / w, cum[lo]))
    }

    /// The appendage's own length (cm), when there is one.
    fn appendage_arc(&self) -> Option<f32> {
        let cut = self.cut?;
        let cum = self.arcs();
        Some(cum[cum.len() - 1] - cum[cut])
    }

    fn arcs(&self) -> Vec<f32> {
        let prof: Vec<(Vec3, f32)> = self.path.iter().map(|&p| (p, 0.0)).collect();
        arc_lengths(&prof)
    }
}

/// WHERE A PATH TAPERS AWAY — the first point (after `from`) of the tail that never again carries
/// more than half ([`crate::shape::CORE_FRACTION`]) the thickest flesh on the path between `from`
/// and it, and never swells again by more than half a cell (the grid's own noise), provided that
/// tail runs on for more than [`crate::shape::SPUR_RADII`] of that flesh — else `None`: the path
/// ends on its mass. `r` is the path's radii, [`NeckTube`]'s lightly smoothed ones.
fn appendage(path: &[Vec3], r: &[f32], cell: f32, from: usize) -> Option<usize> {
    let n = r.len().min(path.len());
    if n < from + 3 {
        return None;
    }
    let mut before = vec![0.0_f32; n];
    for i in from + 1..n {
        before[i] = before[i - 1].max(r[i - 1]);
    }
    // From the tip inward, while the tail behind stays thin and never swells.
    let (mut beyond, mut cut) = (f32::MIN, None);
    for c in (from + 1..n).rev() {
        let tail = beyond.max(r[c]);
        if beyond > r[c] + 0.5 * cell || tail > crate::shape::CORE_FRACTION * before[c] {
            break;
        }
        cut = Some(c);
        beyond = tail;
    }
    let c = cut?;
    let cum = arc_lengths(&path[..n].iter().map(|&p| (p, 0.0)).collect::<Vec<_>>());
    (cum[n - 1] - cum[c] > crate::shape::SPUR_RADII * before[c]).then_some(c)
}

/// How a trunk's head was reached — the three ways [`follow_the_neck`] walks to it.
#[derive(Debug, Clone, Copy)]
enum Neck {
    /// A core chained ahead through a link: the neck is the link, the head that core.
    Core(usize),
    /// A neck TUBE leaving the trunk core's front end ([`Matcher::neck`]): the head is the mass
    /// at the tube's end.
    Tube(usize),
    /// The trunk core's own FRONT CAP ([`ShapeMatch::capped`]): the neck runs on up the core.
    Cap(usize),
}

/// FOLLOW THE NECK (Aaron 2026-09-28, A79A6131: *"the elk's head turns as well, which is actually
/// pretty common … their heads are turned to the side a bit"*) — lay a trunk's neck and head
/// along the path the shape took from the trunk to its head, wherever it bends, sideways
/// included; never on the chord between the withers and the skull.
///
/// THE PATH starts at the bone the neck hangs from (`spine_03`, already laid down the core) and
/// runs up the trunk core's own centreline to where the neck leaves it, then along what the match
/// found: a LINK's curve into the chained head core, a TUBE along the branch of its own tree that
/// carries the most flesh — and only as far as its head's MASS: an APPENDAGE running on past the
/// head (a trunk-that-is-a-nose, tall ears, a beak) is never followed ([`NeckTube`]) — or nothing
/// more for the FRONT CAP. THE HEAD is that MASS, at its centroid — the chained core's middle, the
/// cap's end, the last mass before a tube's appendage, or, on a tube that ends on its head, its
/// LAST CELLS: the share of the path the composed head holds beyond its joint (its farthest
/// child's reach over the composed spine-to-head run — a fraction of the module's own authored
/// geometry, 513E5F78). THE NECK joints lie along the path at their composed
/// fractions of the run to that mass — the composed spine-to-head run and half the head's own
/// reach, which is where the head's mass sits in the composed rest (a humanoid's neck is a short
/// stub on a long upper back, and measured to the joint alone its two bones bunched under the
/// skull). THE FACE — the head's children, jaw and eyes — keeps its
/// composed offsets from the head, turned about Z to where the head FACES: the way the neck
/// carries INTO it, from its last joint to the head. (Not the way a tube goes on past the head: a
/// lead runs out to whichever end reaches farthest — an ear, a mane strand, a horn — and read off
/// it the Donkey's straight head faced 75° aside.) Only a head carried at least as level as it is
/// steep states a facing (a neck rising into a skull says nothing about where the face points),
/// and never one behind the body; otherwise the composed forward stands. The turn is what
/// `bake::face_forward` reads and undoes at bake — the one un-pose of a turned head.
///
/// The neck chain is the head's ancestors named for the neck, root first — the chain
/// `bake::face_forward` turns, read the same way — so a mounted rider's `trunk2_neck_01` is its own.
fn follow_the_neck(
    model: &RawModel,
    w: &mut [Mat4],
    graph: &ShapeGraph,
    trunks: &[usize],
    prefix: &str,
    neck: Neck,
) {
    let Some(head) = bone_at(model, &format!("{prefix}head")) else {
        return;
    };
    let n = model.bones.len();
    let parent = |i: usize| {
        usize::try_from(model.bones[i].parent)
            .ok()
            .filter(|&p| p < n)
    };
    let mut necks: Vec<usize> = Vec::new();
    let mut top = head;
    while let Some(p) = parent(top).filter(|&p| model.bones[p].name.contains("neck")) {
        necks.push(p);
        top = p;
    }
    necks.reverse();
    let Some(root) = parent(top) else {
        return;
    };
    // THE COMPOSED NECK: the root-to-head run as laid out, and the head's own reach beyond its
    // joint (its farthest child) — the proportions the path is read in.
    let composed: Vec<(Vec3, f32)> = std::iter::once(root)
        .chain(necks.iter().copied())
        .chain([head])
        .map(|i| (pos_of(w[i]), 0.0))
        .collect();
    let cum = arc_lengths(&composed);
    let to_head = cum.last().copied().unwrap_or(0.0);
    let face = (0..n)
        .filter(|&i| parent(i) == Some(head))
        .map(|i| pos_of(w[i]).distance(pos_of(w[head])))
        .fold(0.0_f32, f32::max);
    if to_head < 1e-3 {
        return;
    }

    // THE PATH: from the root up the trunk core to where the neck LEAVES it, then on along what
    // the match found — the link's curve and the chained core's middle, a tube out to its head's
    // mass ([`NeckTube`]), or nothing more for the cap.
    let tube = match neck {
        Neck::Tube(l) => NeckTube::of(graph, l),
        _ => None,
    };
    let (trunk, leaves, onward, radii): (usize, Vec3, Vec<Vec3>, Vec<f32>) = match neck {
        Neck::Core(c) => {
            let Some((link, i)) = graph.links.iter().find_map(|l| {
                let i = (0..2).find(|&i| l.cores[1 - i] == c && trunks.contains(&l.cores[i]))?;
                Some((l, i))
            }) else {
                return;
            };
            let mut onward = link.path.clone();
            if i == 1 {
                onward.reverse();
            }
            onward.push(graph.cores[c].at(0.5));
            (link.cores[i], link.ends[i], onward, Vec::new())
        }
        // A TUBE, out to the end of ITS OWN tree that carries the most FLESH (Σ r²·ds). The lead
        // runs to whichever end reaches FARTHEST, and past the head that can be an ear: the
        // Donkey's lead climbs to its left ear tip while its head hangs down to the muzzle. The
        // tube's own branches part from the lead outside the trunk core, and the head's own ends
        // lie beyond the middle of the tube's reach — a branch parting inside the core is another
        // limb the component carries (the Turtle's neck arrives with a fold of its shell), and one
        // ending short of that middle ends on the neck (a strand of mane).
        Neck::Tube(l) => {
            let Some(t) = &tube else {
                return;
            };
            // THE PATH STOPS AT THE HEAD'S MASS: an appendage past it is never followed.
            let end = t.cut.unwrap_or(t.path.len());
            let limb = &graph.limbs[l];
            (
                limb.core,
                limb.at,
                t.path[..end].to_vec(),
                t.radii[..end].to_vec(),
            )
        }
        Neck::Cap(k) => (k, graph.cores[k].front(), Vec::new(), Vec::new()),
    };
    // Inside the trunk core the neck runs STRAIGHT from the root to where it leaves the core: the
    // core's binned centreline is a line down the middle of a thick body, and between the
    // shoulders and a skull merged into it that line zig-zags a cell either way (the BlackBear's
    // ran 9 cm aside) — what bends is the path out of the core, which is followed.
    let start = pos_of(w[root]);
    let mut path: Vec<Vec3> = vec![start, leaves];
    path.extend(onward);
    let prof: Vec<(Vec3, f32)> = path.iter().map(|&p| (p, 0.0)).collect();
    let pcum = arc_lengths(&prof);
    let total = pcum.last().copied().unwrap_or(0.0);
    let end = path[path.len() - 1];
    // THE HEAD, at the centroid of the MASS the path ends in: a chained core's middle and the
    // cap's end are that already; a TUBE's is its LAST CELLS — the share beyond the composed head
    // joint — each weighed by its own flesh (the inscribed radius squared, a cross-section), so
    // the thin tip the path runs out to does not carry the head off with it. The head sits along
    // the path at the middle of that share.
    let before_tube = pcum.get(2).copied().unwrap_or(0.0);
    // `s_mass`: the arc where the head's MASS begins — the neck's end, which the head joint never
    // falls short of.
    let (head_at, s_head, s_mass) = match (neck, tube.as_ref().and_then(|t| t.head(graph.cell))) {
        // AN APPENDAGE RUNS ON PAST IT: the head is the mass the path stops at.
        (Neck::Tube(_), Some((at, s, lo))) => (at, before_tube + s, before_tube + lo),
        (Neck::Tube(_), None) => {
            let from = to_head / (to_head + face) * total;
            let first = path.len() - radii.len();
            let (mut sum, mut weight) = (Vec3::ZERO, 0.0_f32);
            for (i, (&q, &r)) in path[first..].iter().zip(&radii).enumerate() {
                if pcum[first + i] >= from {
                    sum += q * r * r;
                    weight += r * r;
                }
            }
            (
                if weight > 0.0 { sum / weight } else { end },
                0.5 * (from + total),
                from,
            )
        }
        _ => (end, total, 0.0),
    };
    // THE NECK, along the path at its composed fractions of the run to the head's MASS — its
    // joint and half its own reach — which is where the path's head sits. THE HEAD JOINT is on
    // that same scale, at the composed run's end: the base of the skull, where the neck enters
    // the mass, half the head's reach short of its centroid. (Put at the centroid itself, the
    // joint sat mid-skull and the composed face above it rose out of the crown — the reworked
    // humanoids' eyes 2 cm above the head, 2026-10-09.)
    for (k, &b) in necks.iter().enumerate() {
        let at = along_path(&prof, &pcum, cum[k + 1] / (to_head + 0.5 * face) * s_head);
        w[b].w_axis = at.extend(1.0);
    }
    let head_joint = along_path(
        &prof,
        &pcum,
        (to_head / (to_head + 0.5 * face) * s_head).max(s_mass),
    );
    // THE FACE: the way the neck carries into the head, from its last joint — for a head merged
    // into the trunk core, the way the core's own centreline runs into it from there, because the
    // neck bends INSIDE the core and the straight line its bones lie on cuts across the bend (the
    // Wolf's head looks 39° aside; its chord reads 17).
    let last = necks.last().map_or(start, |&b| pos_of(w[b]));
    let facing = match neck {
        Neck::Cap(k) => {
            let c = &graph.cores[k];
            c.front() - c.at(c.t_of(last))
        }
        _ => head_at - last,
    };
    let (level, yaw) = (facing.truncate().length(), facing.x.atan2(-facing.y));
    // A FACE LOOKS THE WAY ITS TRUNK HEADS, never back over it. A lying trunk read tail-first
    // carries a "head" whose face points back along the body — the Toad's front is decided by its
    // own hind thigh, chained to its rump and taken for the head, and turning that face forward
    // swung the thigh (22% of the body, up to 1.7 m). A standing trunk heads up and says nothing.
    let c = &graph.cores[trunk];
    let ahead = c.upright || facing.truncate().dot((c.front() - c.rear()).truncate()) > 0.0;
    let states = ahead && level > 1e-3 && level >= facing.z.abs() && yaw.abs() <= FRAC_PI_2;
    let turn = Quat::from_rotation_z(if states { yaw } else { 0.0 });
    // The head and everything it carries, its children turned with the face.
    let old = pos_of(w[head]);
    let mut stack = vec![head];
    while let Some(i) = stack.pop() {
        let p = pos_of(w[i]);
        w[i].w_axis = (head_joint + turn * (p - old)).extend(1.0);
        stack.extend((0..n).filter(|&c| parent(c) == Some(i)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::{parse_fbx, RawBone, RawVertex};
    // ONE box builder and ONE tube builder for every fixture in the crate (they live beside the
    // voxel field every fit now reads, and they emit CLOSED surfaces it can voxelise).
    use crate::flesh::fixtures::box_mesh;
    use crate::rig::rename_to_canonical;
    use std::collections::HashSet;

    /// THE SPLICE GUARD (2026-08-20, skips without the content tree): a Meshy-shaped source
    /// parents `head` straight to `neck_01` (it has one neck), so the inferred `neck_02` used
    /// to dangle as a leaf — the canonical clips' neck_02 rotation was silently lost and the
    /// head composed one link short (the golem's measured head jut). After conform, `head`
    /// must hang off `neck_02`, parents must precede children, and the mesh's joint indices
    /// must follow the bones they were weighted to through the re-sort.
    #[test]
    fn conform_splices_inferred_links_into_the_chain() {
        let reference = default_reference();
        if !crate::package::file_exists(&reference) {
            eprintln!("skipping: no content tree");
            return;
        }
        let bone = |name: &str, parent: i32, t: [f32; 3]| RawBone {
            name: name.to_string(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        let mut model = RawModel {
            regions: Vec::new(),
            bones: vec![
                bone("pelvis", -1, [0.0, 0.0, 95.0]),
                bone("spine_01", 0, [0.0, 0.0, 10.0]),
                bone("spine_02", 1, [0.0, 0.0, 10.0]),
                bone("spine_03", 2, [0.0, 0.0, 10.0]),
                bone("neck_01", 3, [0.0, 0.0, 19.0]),
                bone("head", 4, [0.0, 0.0, 4.0]), // Meshy shape: head skips the second neck link
            ],
            vertices: vec![RawVertex {
                p: [0.0, -8.0, 155.0],
                n: [0.0, -1.0, 0.0],
                uv: [0.0, 0.0],
                joints: [5, 0, 0, 0], // weighted to `head` at its pre-splice index
                weights: [1.0, 0.0, 0.0, 0.0],
            }],
            indices: vec![0, 0, 0],
        };
        let out = conform_to_canonical(&mut model, &reference, ConformMode::Canonical)
            .expect("conform runs");
        assert!(
            out.infer.spliced.iter().any(|n| n == "head"),
            "the head must be reported spliced, got {:?}",
            out.infer.spliced
        );
        let idx = |name: &str| {
            model
                .bones
                .iter()
                .position(|b| b.name == name)
                .unwrap_or_else(|| panic!("{name} present"))
        };
        let head = idx("head");
        let parent = model.bones[head].parent;
        assert_eq!(
            model.bones[usize::try_from(parent).expect("head has a parent")].name,
            "neck_02",
            "the head hangs off the spliced neck_02"
        );
        for (i, b) in model.bones.iter().enumerate() {
            assert!(
                b.parent < i as i32,
                "parents precede children after the re-sort ({} at {i} points at {})",
                b.name,
                b.parent
            );
        }
        let v = &model.vertices[0];
        assert_eq!(
            usize::try_from(v.joints[0]).unwrap(),
            head,
            "the vertex follows the bone it was weighted to through the remap"
        );
        assert_eq!(v.weights[0], 1.0);
    }

    /// THE AS-PROVIDED GUARD (2026-08-20): `ConformMode::AsProvided` stages the vendor rig
    /// UNTOUCHED — no hip/shoulder/ankle width derivation, no reorient — while still completing
    /// the bone set so the shared clips have targets. It exists to test whether a raw Meshy rig
    /// already drives our clips, so any pass that MOVES a vendor bone would defeat it.
    #[test]
    fn as_provided_skips_derive_and_reorient_but_completes_the_bone_set() {
        let reference = default_reference();
        if !crate::package::file_exists(&reference) {
            eprintln!("skipping: no content tree");
            return;
        }
        let bone = |name: &str, parent: i32, t: [f32; 3]| RawBone {
            name: name.to_string(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        let mut model = RawModel {
            regions: Vec::new(),
            bones: vec![
                bone("pelvis", -1, [0.0, 0.0, 95.0]),
                bone("spine_01", 0, [0.0, 0.0, 10.0]),
                bone("spine_02", 1, [0.0, 0.0, 10.0]),
                bone("spine_03", 2, [0.0, 0.0, 10.0]),
                bone("neck_01", 3, [0.0, 0.0, 19.0]),
                bone("head", 4, [0.0, 0.0, 4.0]),
            ],
            vertices: vec![],
            indices: vec![],
        };
        let out = conform_to_canonical(&mut model, &reference, ConformMode::AsProvided)
            .expect("as-provided conform runs");
        // No reorient and no derive passes ran.
        assert_eq!(
            out.reorient.limbs_aligned, 0,
            "as-provided runs no reorient"
        );
        assert!(
            out.hip.left.is_none() && out.hip.right.is_none(),
            "as-provided derives no hip width"
        );
        // The bone set is still completed so the shared clips resolve (neck_02 among the added).
        assert!(
            out.infer.added.iter().any(|n| n == "neck_02"),
            "the bone set is completed, got {:?}",
            out.infer.added
        );
        // The vendor root keeps its position exactly (the pelvis is never reparented).
        let pelvis = model
            .bones
            .iter()
            .find(|b| b.name == "pelvis")
            .expect("pelvis present");
        assert_eq!(
            pelvis.translation,
            [0.0, 0.0, 95.0],
            "the vendor pelvis is untouched"
        );
    }

    /// THE AS-PROVIDED INFERENCE GUARD (2026-08-20): inferred bones (twists, fingers, eyes) must land
    /// on the CANONICAL basis even when the vendor core keeps a non-canonical rest frame — the fix for
    /// the "inferred bones translated off the mesh" symptom (eyes/shoulders/knees sticking out). A
    /// vendor head carrying a 90° rest rotation must still get its eye inferred to the SAME world spot
    /// the canonical path places it — not rotated by the vendor frame.
    #[test]
    fn as_provided_infers_on_the_canonical_basis_regardless_of_vendor_frame() {
        use glam::{Mat4, Quat, Vec3};
        let reference = default_reference();
        if !crate::package::file_exists(&reference) {
            eprintln!("skipping: no content tree");
            return;
        }
        // A vendor-style head whose REST FRAME is rotated 90° about X — a differing bone-axis
        // convention, the thing the bug composed the canonical offset onto.
        let rot = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let head_pos = Vec3::new(0.0, 0.0, 148.0);
        let head_world = Mat4::from_rotation_translation(rot, head_pos);
        let make = || RawModel {
            regions: Vec::new(),
            bones: vec![RawBone {
                name: "head".to_string(),
                parent: -1,
                translation: head_pos.to_array(),
                rotation: rot.to_array(),
                scale: [1.0, 1.0, 1.0],
                inverse_bind: head_world.inverse().to_cols_array(),
            }],
            vertices: vec![],
            indices: vec![],
        };
        let eye_world = |m: &RawModel| -> Option<Vec3> {
            let g = model_world_frames(m);
            m.bones
                .iter()
                .position(|b| b.name == "eye_l")
                .map(|i| pos_of(g[i]))
        };

        // Canonical path: reorient the vendor frame, then infer.
        let mut canon = make();
        reorient_to_canonical(&mut canon, &reference).unwrap();
        infer_canonical_bones(&mut canon, &reference, ConformMode::Canonical).unwrap();
        // As-provided: keep the vendor frame, infer on the canonical basis.
        let mut raw = make();
        infer_canonical_bones(&mut raw, &reference, ConformMode::AsProvided).unwrap();

        let (Some(ce), Some(re)) = (eye_world(&canon), eye_world(&raw)) else {
            panic!("eye_l must be inferred off the head in both modes");
        };
        assert!(
            (ce - re).length() < 0.5,
            "as-provided must infer the eye on the canonical basis (canonical {ce:?} vs as-provided {re:?})"
        );
    }

    /// Worst world position (cm) + orientation (deg) delta of `model`'s bones vs the oracle at
    /// `reference`, matched by name (bones absent from the oracle, e.g. a synthesized root, skipped).
    fn oracle_worst_delta(model: &RawModel, reference: &Path) -> (f32, String, f32, String) {
        let refs = load_reference_skeleton(reference).unwrap();
        let og = fk(
            &refs.iter().map(|b| b.local).collect::<Vec<_>>(),
            &refs.iter().map(|b| b.parent).collect::<Vec<_>>(),
        );
        let oidx: HashMap<String, usize> = refs
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let g = model_world_frames(model);
        let (mut wp, mut wd, mut wpn, mut wdn) = (0.0f32, 0.0f32, String::new(), String::new());
        for (i, b) in model.bones.iter().enumerate() {
            let Some(&oi) = oidx.get(&b.name) else {
                continue;
            };
            // The lower-leg + shoulder chains are now intentionally mesh-derived — `derive_ankle_placement`
            // lowers the ankle (re-aligning the calf + shifting the inferred calf-twist), and
            // `derive_shoulder_placement` widens the glenohumeral joint (moving `upperarm` + its inferred
            // twist) — so they deviate from the Blender oracle BY DESIGN. Exclude from the reproduction check.
            if matches!(
                b.name.as_str(),
                "foot_l"
                    | "foot_r"
                    | "calf_twist_01_l"
                    | "calf_twist_01_r"
                    | "upperarm_l"
                    | "upperarm_r"
                    | "upperarm_twist_01_l"
                    | "upperarm_twist_01_r"
            ) {
                continue;
            }
            let dp = (pos_of(g[i]) - pos_of(og[oi])).length();
            let (_, rq, _) = g[i].to_scale_rotation_translation();
            let (_, oq, _) = og[oi].to_scale_rotation_translation();
            let deg = rq.angle_between(oq).to_degrees();
            if dp > wp {
                wp = dp;
                wpn = b.name.clone();
            }
            if deg > wd {
                wd = deg;
                wdn = b.name.clone();
            }
        }
        (wp, wpn, wd, wdn)
    }

    fn find_character() -> Option<std::path::PathBuf> {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/source/PrismHumanBaseA");
        if !dir.exists() {
            return None;
        }
        std::fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| {
                p.to_string_lossy().contains("Character_output")
                    && p.extension().map(|e| e == "fbx").unwrap_or(false)
            })
    }

    /// Convention check (non-circular): FK the reference `PrismHumanBaseA.json` and confirm it reads
    /// as a sane upright human in Z-up cm — pelvis at hip height, head well above, feet near the
    /// ground. If the column-major/FK decode were wrong, these would be nonsense.
    #[test]
    fn reference_fk_is_a_sane_upright_human() {
        let reference = default_reference();
        if !reference.exists() {
            eprintln!("skipping: reference {} not present", reference.display());
            return;
        }
        let refs = load_reference_skeleton(&reference).unwrap();
        let cg = fk(
            &refs.iter().map(|b| b.local).collect::<Vec<_>>(),
            &refs.iter().map(|b| b.parent).collect::<Vec<_>>(),
        );
        let cidx: HashMap<String, usize> = refs
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let z = |n: &str| cidx.get(n).map(|&i| pos_of(cg[i]).z);
        let (pelvis, head, foot) = (
            z("pelvis").unwrap(),
            z("head").unwrap(),
            z("foot_l").unwrap(),
        );
        eprintln!("reference Z-up cm: pelvis {pelvis:.1}, head {head:.1}, foot_l {foot:.1}");
        assert!(
            (60.0..120.0).contains(&pelvis),
            "pelvis at hip height, got {pelvis}"
        );
        assert!(
            head > pelvis + 40.0,
            "head well above the pelvis, got {head}"
        );
        assert!(
            foot < pelvis - 40.0 && foot < 25.0,
            "feet near the ground, got {foot}"
        );
        assert!(
            cg.iter().all(|m| m.w_axis.truncate().is_finite()),
            "no NaN in the FK"
        );
    }

    /// Hip-width derivation reproduces the oracle's femoral-head placement. Raw Meshy plants the
    /// thighs at x≈±5.1 (sep ~10.2 cm, knees cross); after derivation they sit at the oracle's
    /// ±8.67/−8.44 (sep ~17 cm). WIDTH only — the thigh y/z are untouched.
    #[test]
    fn hip_placement_widens_femoral_heads_to_oracle() {
        let Some(fbx) = find_character() else {
            eprintln!("skipping: no source FBX");
            return;
        };
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        let idx: HashMap<String, usize> = model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let (tl, tr) = (idx["thigh_l"], idx["thigh_r"]);

        let before = model_world_frames(&model);
        let (bl, br) = (pos_of(before[tl]), pos_of(before[tr]));
        let report = derive_hip_placement(&mut model);
        let after = model_world_frames(&model);
        let (al, ar) = (pos_of(after[tl]), pos_of(after[tr]));
        eprintln!(
            "thigh_l x {:.2}->{:.2} (oracle 8.67), thigh_r x {:.2}->{:.2} (oracle -8.44)",
            bl.x, al.x, br.x, ar.x
        );
        eprintln!("hip report: {report:?}");

        // Reproduces the oracle femoral-head width within a small tolerance (mesh not decimated here).
        assert!((al.x - 8.67).abs() < 1.5, "thigh_l x → ~8.67, got {}", al.x);
        assert!(
            (ar.x + 8.44).abs() < 1.5,
            "thigh_r x → ~-8.44, got {}",
            ar.x
        );
        assert!(
            al.x - ar.x > 15.0,
            "femoral heads widen to ~17 cm sep, got {}",
            al.x - ar.x
        );
        // WIDTH only: y and z of the thighs are unchanged.
        assert!(
            (al.y - bl.y).abs() < 1e-3 && (al.z - bl.z).abs() < 1e-3,
            "thigh_l y/z untouched"
        );
        assert!(
            (ar.y - br.y).abs() < 1e-3 && (ar.z - br.z).abs() < 1e-3,
            "thigh_r y/z untouched"
        );
    }

    /// Shoulder-width derivation moves the glenohumeral joint (`upperarm_l/r`) to `SHOULDER_FRACTION`
    /// of the way from the midline to the widest shoulder flesh — WIDTH only (y/z untouched), like the
    /// hip. Meshy plants it slightly medial (Aaron: "find the shoulders the same way we find the
    /// pelvis"). Prints raw → derived vs the oracle so the tunable knob can be judged.
    #[test]
    fn shoulder_placement_widens_to_flesh() {
        let Some(fbx) = find_character() else {
            eprintln!("skipping: no source FBX");
            return;
        };
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        let idx: HashMap<String, usize> = model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let (ul, ur) = (idx["upperarm_l"], idx["upperarm_r"]);

        let before = model_world_frames(&model);
        let (bl, br) = (pos_of(before[ul]), pos_of(before[ur]));
        let report = derive_shoulder_placement(&mut model);
        let after = model_world_frames(&model);
        let (al, ar) = (pos_of(after[ul]), pos_of(after[ur]));
        eprintln!(
            "upperarm_l x {:.2}->{:.2}, upperarm_r x {:.2}->{:.2} (oracle ±15.51); report {report:?}",
            bl.x, al.x, br.x, ar.x
        );

        // The joint lands at SHOULDER_FRACTION of the measured widest flesh, per side.
        if let Some((_, tgt, width)) = report.left {
            assert!((al.x - tgt).abs() < 1e-3, "upperarm_l lands on its target");
            assert!(
                (tgt - (0.28 + SHOULDER_FRACTION * width)).abs() < 0.5,
                "target = fraction·widest from ~mid"
            );
        }
        // WIDTH only: the y and z of both shoulders are unchanged.
        assert!(
            (al.y - bl.y).abs() < 1e-3 && (al.z - bl.z).abs() < 1e-3,
            "upperarm_l y/z untouched"
        );
        assert!(
            (ar.y - br.y).abs() < 1e-3 && (ar.z - br.z).abs() < 1e-3,
            "upperarm_r y/z untouched"
        );
        // Shoulders end roughly symmetric and human-width (~13–19 cm half-span).
        assert!(
            (6.0..22.0).contains(&al.x.abs()) && (6.0..22.0).contains(&ar.x.abs()),
            "sane shoulder half-width"
        );
    }

    /// Infer adds the reference's missing bones: 24→65 (fingers/twists/sockets/face; `root` is a bake
    /// concern, added later → the oracle's 67). Because we infer FROM the oracle with scale≈1, each
    /// inferred bone's world position reproduces the oracle within a small tolerance.
    #[test]
    fn infer_adds_canonical_bones_matching_oracle() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else {
            return;
        };
        if !reference.exists() {
            eprintln!("skipping: reference not present");
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        derive_hip_placement(&mut model);
        reorient_to_canonical(&mut model, &reference).unwrap();
        let report = infer_canonical_bones(&mut model, &reference, ConformMode::Canonical).unwrap();
        eprintln!(
            "added {} bones, total {}; hand_scale l={:.3} r={:.3}",
            report.added.len(),
            model.bones.len(),
            report.hand_scale_l,
            report.hand_scale_r
        );

        assert_eq!(
            model.bones.len(),
            66,
            "22 canonical + 44 inferred (root added at bake → 67)"
        );
        let names: HashSet<&str> = model.bones.iter().map(|b| b.name.as_str()).collect();
        for n in [
            "index_01_l",
            "thumb_03_r",
            "pinky_02_l",
            "Weapon_L",
            "Weapon_R",
            "upperarm_twist_01_l",
            "calf_twist_01_r",
            "jaw",
            "eye_l",
            "eye_r",
        ] {
            assert!(names.contains(n), "inferred bone '{n}' present");
        }
        // Hand scale is the forearm ratio; this body IS the oracle's source, so it is ~1.
        assert!(
            (0.9..1.1).contains(&report.hand_scale_l),
            "hand_scale_l ~1, got {}",
            report.hand_scale_l
        );

        // Inferred bone world positions reproduce the oracle (scale≈1 inferring from the oracle).
        let refs = load_reference_skeleton(&reference).unwrap();
        let og = fk(
            &refs.iter().map(|b| b.local).collect::<Vec<_>>(),
            &refs.iter().map(|b| b.parent).collect::<Vec<_>>(),
        );
        let oidx: HashMap<String, usize> = refs
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let g = model_world_frames(&model);
        let midx: HashMap<String, usize> = model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let mut worst = 0.0f32;
        for n in [
            "index_01_l",
            "index_03_l",
            "thumb_03_r",
            "pinky_03_l",
            "Weapon_L",
            "upperarm_twist_01_l",
            "jaw",
            "eye_r",
        ] {
            let d = (pos_of(g[midx[n]]) - pos_of(og[oidx[n]])).length();
            eprintln!("  {n}: {:.3} cm from oracle", d);
            worst = worst.max(d);
        }
        assert!(
            worst < 1.0,
            "inferred bones within ~1 cm of the oracle, worst {worst:.3}"
        );
    }

    /// THE correctness test (handoff step 4): the full in-app conform of the female FBX reproduces
    /// the Blender-produced `PrismHumanBaseA.json` oracle — every shared bone's world POSITION and
    /// ORIENTATION — confirming the whole port (axis/unit + hip-width + limb-align + infer) with no
    /// external tools. (`root` is oracle-only until bake, so it is excluded.)
    #[test]
    fn full_conform_reproduces_the_oracle() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else {
            return;
        };
        if !reference.exists() {
            eprintln!("skipping: reference not present");
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        conform_to_canonical(&mut model, &reference, ConformMode::Canonical).unwrap();

        // Every oracle bone except `root` must be present in my conform.
        let refs = load_reference_skeleton(&reference).unwrap();
        let mine: HashSet<&str> = model.bones.iter().map(|b| b.name.as_str()).collect();
        for b in &refs {
            if b.name != "root" {
                assert!(
                    mine.contains(b.name.as_str()),
                    "conform is missing oracle bone '{}'",
                    b.name
                );
            }
        }

        // Compare each shared bone's world position + orientation.
        let (worst_pos, worst_pos_name, worst_deg, worst_deg_name) =
            oracle_worst_delta(&model, &reference);
        eprintln!(
            "oracle match: worst position {worst_pos:.4} cm ({worst_pos_name}), worst orientation {worst_deg:.4}° ({worst_deg_name}); {} shared bones",
            model.bones.len()
        );
        assert!(
            worst_pos < 0.1,
            "every bone within 0.1 cm of the oracle (worst {worst_pos:.4} at {worst_pos_name})"
        );
        assert!(
            worst_deg < 0.5,
            "every bone within 0.5° of the oracle (worst {worst_deg:.4} at {worst_deg_name})"
        );
    }

    /// The game-ready low-res re-export of the human base (`PrismRaces/HumanBaseA_Low`, ~4k tris)
    /// conforms to a SANE canonical rig. It is a FRESH Meshy export, NOT the same body the old oracle
    /// was built from, so it keeps its OWN proportions rather than reproducing the oracle — which is
    /// exactly what the multi-body conform is for. Diagnostics print how far it sits from the oracle.
    /// `#[ignore]`d (reads the roster); run: `cargo test -p flicker-content -- --ignored low_res_human`.
    #[test]
    #[ignore]
    fn low_res_human_conforms_sanely() {
        let low = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismRaces/HumanBaseA_Low");
        let reference = default_reference();
        if !low.exists() || !reference.exists() {
            eprintln!("skipping: low-res roster / reference not present");
            return;
        }
        let fbx = std::fs::read_dir(&low)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| {
                p.to_string_lossy().contains("Character_output")
                    && p.extension().map(|e| e == "fbx").unwrap_or(false)
            })
            .expect("HumanBaseA_Low Character_output.fbx");
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);

        // RAW landmark heights (pre-conform) — is this the same body as the oracle? (pelvis 95.6,
        // head 153.2, foot_l 9.9 in the oracle.) A different height ⇒ a different body, not a bug.
        let raw = model_world_frames(&model);
        let ridx: HashMap<String, usize> = model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let rz = |n: &str| ridx.get(n).map(|&i| pos_of(raw[i]).z).unwrap_or(f32::NAN);
        eprintln!(
            "low-res human RAW z: pelvis {:.1}, head {:.1}, foot_l {:.1}  (oracle 95.6 / 153.2 / 9.9)",
            rz("pelvis"),
            rz("head"),
            rz("foot_l")
        );

        conform_to_canonical(&mut model, &reference, ConformMode::Canonical).unwrap();

        // Per-bone distance to the oracle — top few, to characterise the difference.
        let refs = load_reference_skeleton(&reference).unwrap();
        let og = fk(
            &refs.iter().map(|b| b.local).collect::<Vec<_>>(),
            &refs.iter().map(|b| b.parent).collect::<Vec<_>>(),
        );
        let oidx: HashMap<String, usize> = refs
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let g = model_world_frames(&model);
        let mut deltas: Vec<(f32, &str)> = model
            .bones
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                oidx.get(&b.name)
                    .map(|&oi| ((pos_of(g[i]) - pos_of(og[oi])).length(), b.name.as_str()))
            })
            .collect();
        deltas.sort_by(|a, b| b.0.total_cmp(&a.0));
        eprintln!(
            "top bone deltas vs oracle ({} tris):",
            model.indices.len() / 3
        );
        for (d, n) in deltas.iter().take(6) {
            eprintln!("  {n:<20} {d:.2} cm");
        }

        // A NEW body need not match the old oracle; it must conform to a SANE upright canonical rig.
        assert_eq!(
            model.bones.len(),
            66,
            "conforms to the 66-bone canonical set (+root at bake → 67)"
        );
        let gz = |n: &str| {
            ridx.get(n)
                .map(|_| pos_of(g[model.bones.iter().position(|b| b.name == n).unwrap()]).z)
        };
        let (pelvis, head, foot) = (
            gz("pelvis").unwrap(),
            gz("head").unwrap(),
            gz("foot_l").unwrap(),
        );
        assert!(
            (60.0..120.0).contains(&pelvis),
            "pelvis at hip height, got {pelvis}"
        );
        assert!(head > pelvis + 40.0, "head well above pelvis, got {head}");
        assert!(foot < 25.0, "feet near the ground, got {foot}");
        assert!(
            g.iter().all(|m| m.w_axis.truncate().is_finite()),
            "finite conform"
        );
    }

    /// DIAGNOSTIC: is `HumanBaseA_Low`'s higher pelvis a genuine build (bone sits inside its mesh
    /// flesh) or Meshy's weak placement (bone floats above the flesh)? And does the ported pelvis-WIDTH
    /// routine fire? Prints bone heights against the z-extent of the flesh each bone actually weights.
    /// `#[ignore]`d; run: `cargo test -p flicker-content -- --ignored diagnose_hip --nocapture`.
    #[test]
    #[ignore]
    fn diagnose_hip_geometry() {
        let reference = default_reference();
        let bodies = [
            ("HumanBaseA_Low (new)", "PrismRaces/HumanBaseA_Low"),
            ("PrismHumanBaseA (old)", "PrismHumanBaseA"),
        ];
        for (label, rel) in bodies {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../content/source")
                .join(rel);
            let Some(fbx) = dir.exists().then_some(()).and_then(|_| {
                std::fs::read_dir(&dir)
                    .ok()?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .find(|p| {
                        p.to_string_lossy().contains("Character_output")
                            && p.extension().map(|e| e == "fbx").unwrap_or(false)
                    })
            }) else {
                eprintln!("-- {label}: not present, skipping");
                continue;
            };
            let mut model = parse_fbx(&fbx).unwrap();
            rename_to_canonical(&mut model);
            let bidx: HashMap<String, usize> = model
                .bones
                .iter()
                .enumerate()
                .map(|(i, b)| (b.name.clone(), i))
                .collect();
            // z-extent of the flesh a bone actually weights (≥0.5), the region the bone should sit in.
            let flesh_z = |name: &str, m: &RawModel| -> (f32, f32, usize) {
                let bi = bidx[name] as u32;
                let zs: Vec<f32> = m
                    .vertices
                    .iter()
                    .filter(|v| (0..4).any(|k| v.joints[k] == bi && v.weights[k] >= 0.5))
                    .map(|v| v.p[2])
                    .collect();
                match zs.len() {
                    0 => (f32::NAN, f32::NAN, 0),
                    n => (
                        zs.iter().cloned().fold(f32::MAX, f32::min),
                        zs.iter().cloned().fold(f32::MIN, f32::max),
                        n,
                    ),
                }
            };
            let raw = model_world_frames(&model);
            let pbz = pos_of(raw[bidx["pelvis"]]).z;
            let tbz = pos_of(raw[bidx["thigh_l"]]).z;
            let (pfmin, pfmax, pn) = flesh_z("pelvis", &model);
            let (tfmin, tfmax, tn) = flesh_z("thigh_l", &model);
            eprintln!("== {label} ==");
            eprintln!(
                "  pelvis bone z {pbz:.1}  | pelvis-flesh z [{pfmin:.1}..{pfmax:.1}] (n={pn}) → bone {}",
                if pbz >= pfmin && pbz <= pfmax {
                    "INSIDE flesh (genuine)"
                } else {
                    "OUTSIDE flesh (floats)"
                }
            );
            eprintln!(
                "  thigh_l bone z {tbz:.1} | thigh-flesh z [{tfmin:.1}..{tfmax:.1}] (n={tn}) → femoral head at flesh-top? gap {:.1}",
                tfmax - tbz
            );
            let hip = derive_hip_placement(&mut model);
            eprintln!("  pelvis-WIDTH routine: {hip:?}");
            if reference.exists() {
                reorient_to_canonical(&mut model, &reference).unwrap();
                let g = model_world_frames(&model);
                eprintln!(
                    "  after conform: thigh_l x {:.2} (femoral-head width)",
                    pos_of(g[bidx["thigh_l"]]).x
                );
            }
        }
    }

    /// DIAGNOSTIC: does Meshy plant the SHOULDER (`upperarm_l/r`, `clavicle_l/r`) where the flesh
    /// says the glenohumeral joint is, or is it mis-placed (Aaron: "find the shoulders the same way
    /// we find the pelvis")? Prints each shoulder bone's world pos against the x/y/z extent + centroid
    /// of the flesh it weights (≥0.5), for HumanBaseA_Low. `#[ignore]`d; run:
    ///   `cargo test -p flicker-content -- --ignored diagnose_shoulder --nocapture`
    #[test]
    #[ignore]
    fn diagnose_shoulder_geometry() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismRaces/HumanBaseA_Low");
        let Some(fbx) = dir.exists().then_some(()).and_then(|_| {
            std::fs::read_dir(&dir)
                .ok()?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| {
                    p.to_string_lossy().contains("Character_output")
                        && p.extension().map(|e| e == "fbx").unwrap_or(false)
                })
        }) else {
            eprintln!("skipping: no HumanBaseA_Low");
            return;
        };
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        let bidx: HashMap<String, usize> = model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), i))
            .collect();
        let w = model_world_frames(&model);

        // Flesh a bone weights (≥0.5): count, centroid, and per-axis min/max.
        let flesh = |name: &str| -> Option<(usize, Vec3, Vec3, Vec3)> {
            let bi = *bidx.get(name)? as u32;
            let ps: Vec<Vec3> = model
                .vertices
                .iter()
                .filter(|v| (0..4).any(|k| v.joints[k] == bi && v.weights[k] >= 0.5))
                .map(|v| Vec3::from(v.p))
                .collect();
            if ps.is_empty() {
                return Some((0, Vec3::NAN, Vec3::NAN, Vec3::NAN));
            }
            let c = ps.iter().copied().sum::<Vec3>() / ps.len() as f32;
            let lo = ps.iter().copied().reduce(|a, b| a.min(b)).unwrap();
            let hi = ps.iter().copied().reduce(|a, b| a.max(b)).unwrap();
            Some((ps.len(), c, lo, hi))
        };

        eprintln!("HumanBaseA_Low shoulder geometry (raw Meshy, renamed):");
        for name in ["clavicle_l", "upperarm_l", "clavicle_r", "upperarm_r"] {
            let Some(&bi) = bidx.get(name) else { continue };
            let bp = pos_of(w[bi]);
            eprintln!("  {name}: bone [{:6.2} {:6.2} {:6.2}]", bp.x, bp.y, bp.z);
            if let Some((n, c, lo, hi)) = flesh(name) {
                eprintln!(
                    "      flesh n={n} centroid [{:6.2} {:6.2} {:6.2}] x[{:.1}..{:.1}] y[{:.1}..{:.1}] z[{:.1}..{:.1}]",
                    c.x, c.y, c.z, lo.x, hi.x, lo.y, hi.y, lo.z, hi.z
                );
            }
        }
        // Shoulder JOINT candidate: the widest shoulder flesh (upperarm+clavicle) per side, like the
        // hip's "widest hip flesh". mid = spine_03 x.
        let mid = pos_of(w[bidx["spine_03"]]).x;
        for (uarm, clav, sign, side) in [
            ("upperarm_l", "clavicle_l", 1.0f32, "l"),
            ("upperarm_r", "clavicle_r", -1.0f32, "r"),
        ] {
            let (Some(&ui), Some(&ci)) = (bidx.get(uarm), bidx.get(clav)) else {
                continue;
            };
            let (ui, ci) = (ui as u32, ci as u32);
            let widest = model
                .vertices
                .iter()
                .filter(|v| {
                    (0..4).any(|k| (v.joints[k] == ui || v.joints[k] == ci) && v.weights[k] >= 0.5)
                })
                .map(|v| sign * (v.p[0] - mid))
                .filter(|d| *d > 0.0)
                .fold(0.0f32, f32::max);
            eprintln!(
                "  {side}: mid(spine_03.x)={mid:.2}, widest shoulder flesh {widest:.2} cm from midline; upperarm now at {:.2}",
                pos_of(w[bidx[uarm]]).x
            );
        }
    }

    /// DIAGNOSTIC: the shoulder fix's ACTUAL effect on the IDLE pose (Aaron's "hands moved forward,
    /// not out"). Conforms HumanBaseA_Low WITHOUT then WITH `derive_shoulder_placement`, bakes each,
    /// retargets the real `idle_neutral.bvh` onto it in-code, poses frame 0, and prints where `hand_l`
    /// lands relative to the hip — so the render observation is measurable without a manual re-bake.
    /// `#[ignore]`d; run: `cargo test -p flicker-content -- --ignored idle_pose_shoulder --nocapture`
    #[test]
    #[ignore]
    fn idle_pose_shoulder_effect() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let src = base.join("Alpha/content/source/PrismRaces/HumanBaseA_Low");
        let idle_bvh = base.join(
            "Alpha/content/source/Motifect/Motifect_locomotion_complete_v1_0/BVH/idle_neutral.bvh",
        );
        let reference = default_reference();
        if !src.exists() || !idle_bvh.exists() || !reference.exists() {
            eprintln!("skipping: content not present");
            return;
        }
        let Some(fbx) = std::fs::read_dir(&src)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| {
                p.to_string_lossy().contains("Character_output")
                    && p.extension().map(|e| e == "fbx").unwrap_or(false)
            })
        else {
            return;
        };
        let tmp = std::env::temp_dir().join("flicker_idle_probe");
        std::fs::create_dir_all(&tmp).unwrap();

        // Pose frame 0 of a retargeted clip file and return world positions by bone name.
        let posed_world = |clip: &Path| -> (Vec<Mat4>, HashMap<String, usize>) {
            let refs = load_reference_skeleton(clip).unwrap();
            let idx: HashMap<String, usize> = refs
                .iter()
                .enumerate()
                .map(|(i, b)| (b.name.clone(), i))
                .collect();
            let v: serde_json::Value =
                serde_json::from_str(&crate::package::read_text(clip).unwrap()).unwrap();
            let mut posed: Vec<Mat4> = refs.iter().map(|b| b.local).collect();
            for t in v["clips"][0]["tracks"].as_array().unwrap() {
                let Some(&bi) = idx.get(t["bone"].as_str().unwrap()) else {
                    continue;
                };
                let k = &t["keys"][0];
                let a = |key: &str, n: usize| k[key][n].as_f64().unwrap() as f32;
                posed[bi] = Mat4::from_scale_rotation_translation(
                    Vec3::new(a("S", 0), a("S", 1), a("S", 2)),
                    Quat::from_xyzw(a("R", 0), a("R", 1), a("R", 2), a("R", 3)),
                    Vec3::new(a("T", 0), a("T", 1), a("T", 2)),
                );
            }
            let parents: Vec<i32> = refs.iter().map(|b| b.parent).collect();
            (fk(&posed, &parents), idx)
        };

        // Sweep the shoulder fraction; the hip flesh outer edge at hand height (z≈86) is x≈17.1, so
        // `hand_l.x − 17.1` is the lateral clearance (negative = clips). `None` = no shoulder fix.
        const HIP_EDGE_X: f32 = 17.1;
        for frac in [None, Some(0.62f32), Some(0.70), Some(0.78)] {
            let mut model = parse_fbx(&fbx).unwrap();
            rename_to_canonical(&mut model);
            derive_hip_placement(&mut model);
            if let Some(f) = frac {
                derive_shoulder_placement_frac(&mut model, f);
            }
            derive_ankle_placement(&mut model);
            reorient_to_canonical(&mut model, &reference).unwrap();
            infer_canonical_bones(&mut model, &reference, ConformMode::Canonical).unwrap();
            let ua_rest = {
                let w = model_world_frames(&model);
                let i = model
                    .bones
                    .iter()
                    .position(|b| b.name == "upperarm_l")
                    .unwrap();
                pos_of(w[i]).x
            };
            let tag = frac.map_or("baseline".to_string(), |f| format!("frac {f:.2}"));
            let skel = tmp.join("skel.json");
            crate::bake::write_rig(&model, &fbx, "HumanBaseA", &skel, &[], None).unwrap();
            let (inplace, _) =
                crate::retarget::emit_variants(&idle_bvh, &skel, &tmp.join("c")).unwrap();
            let (g, idx) = posed_world(&inplace);
            let p = |n: &str| pos_of(g[idx[n]]);
            let (h, _th) = (p("hand_l"), p("thigh_l"));
            // Posed shoulder height (does widening amplify the idle shoulder-drop? rest upperarm z≈137.5).
            let (clav_z, ua_z) = (p("clavicle_l").z, p("upperarm_l").z);
            eprintln!(
                "[{tag:9}] upperarm rest x={ua_rest:5.2} | IDLE hand_l x={:6.2} clear {:+5.2}cm | posed clav_z={clav_z:6.2} upperarm_z={ua_z:6.2} (rest 137.5, drop {:+.2})",
                h.x,
                h.x - HIP_EDGE_X,
                ua_z - 137.55
            );
        }
        eprintln!(
            "(hip flesh outer edge at hand height ≈ x 17.1; positive clearance = hand clears the hip)"
        );
    }

    /// Reorient runs on the real body and produces a sane rig: limbs get aligned, and every bone's
    /// new local TRS + inverse_bind is finite. (The exact oracle match against `PrismHumanBaseA.json`
    /// comes once the conform is COMPLETE — hip-width + infer + axis/unit normalization; at that
    /// point my conform of this FBX should reproduce that file.)
    #[test]
    fn reorient_runs_and_aligns_limbs() {
        let (Some(fbx), reference) = (find_character(), default_reference()) else {
            eprintln!("skipping: no source FBX");
            return;
        };
        if !reference.exists() {
            eprintln!("skipping: reference not present");
            return;
        }
        let mut model = parse_fbx(&fbx).unwrap();
        rename_to_canonical(&mut model);
        let report = reorient_to_canonical(&mut model, &reference).unwrap();
        eprintln!("reoriented {} limbs", report.limbs_aligned);
        assert!(
            report.limbs_aligned >= 8,
            "the arm+leg+foot chains aligned, got {}",
            report.limbs_aligned
        );
        assert!(
            model.bones.iter().all(|b| {
                b.translation.iter().all(|f| f.is_finite())
                    && b.rotation.iter().all(|f| f.is_finite())
                    && b.inverse_bind.iter().all(|f| f.is_finite())
            }),
            "every reoriented bone has finite TRS + inverse_bind"
        );
    }

    /// THE WRIST GUARD (2026-08-21): a hand is a limb — its frame turns to point down THIS body's
    /// hand (to its `middle_01`) exactly as the forearm turns down the forearm. Left on the canon's
    /// world orientation, the golem's hand played every clip's hand direction 37° off its flesh
    /// ("hands bent away from the default angle"). Without fingers yet, the hand continues its own
    /// forearm, so the fingers infer along this body's arm.
    #[test]
    fn a_hand_aligns_to_its_fingers_and_continues_its_forearm_without_them() {
        let reference = default_reference();
        if !crate::package::file_exists(&reference) {
            eprintln!("skipping: no content tree");
            return;
        }
        let refs = load_reference_skeleton(&reference).unwrap();
        let cg = fk(
            &refs.iter().map(|b| b.local).collect::<Vec<_>>(),
            &refs.iter().map(|b| b.parent).collect::<Vec<_>>(),
        );
        let cpos = |n: &str| pos_of(cg[refs.iter().position(|b| b.name == n).unwrap()]);
        let v_canon = (cpos("middle_01_l") - cpos("hand_l")).normalize();
        let bone = |name: &str, parent: i32, t: [f32; 3]| RawBone {
            name: name.to_string(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            inverse_bind: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
        };
        // A forearm running out-and-down, a hand on its end, and (optionally) a finger root
        // straight below the wrist — a hand that does NOT continue its forearm.
        let arm = |fingers: bool| {
            let mut bones = vec![
                bone("lowerarm_l", -1, [45.0, 0.0, 118.0]),
                bone("hand_l", 0, [19.0, 0.0, -17.0]),
            ];
            if fingers {
                bones.push(bone("middle_01_l", 1, [0.0, 0.0, -8.0]));
            }
            RawModel {
                regions: Vec::new(),
                bones,
                vertices: vec![],
                indices: vec![],
            }
        };
        // Where the hand's frame says the hand points: the canon's hand direction carried by it.
        let hand_points = |m: &RawModel| -> Vec3 {
            let g = model_world_frames(m);
            let i = m.bones.iter().position(|b| b.name == "hand_l").unwrap();
            (glam::Mat3::from_mat4(g[i]) * v_canon).normalize()
        };

        let mut with = arm(true);
        reorient_to_canonical(&mut with, &reference).unwrap();
        let d = hand_points(&with);
        assert!(
            (d - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-3,
            "with fingers the hand points down its own finger root, got {d:?}"
        );

        let mut without = arm(false);
        reorient_to_canonical(&mut without, &reference).unwrap();
        let d = hand_points(&without);
        let forearm = Vec3::new(19.0, 0.0, -17.0).normalize();
        assert!(
            (d - forearm).length() < 1e-3,
            "without fingers the hand continues its forearm, got {d:?} vs {forearm:?}"
        );
    }

    fn bbox(m: &RawModel) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for v in &m.vertices {
            let p = Vec3::from(v.p);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo, hi)
    }

    /// STRAIGHTENED FRAMES: a rig whose limb frames were turned comes back with the identity at
    /// every bone, every joint exactly where it was, and the rest skin still the identity.
    #[test]
    fn straighten_frames_keeps_every_joint_and_zeroes_every_rotation() {
        let turned = glam::Quat::from_rotation_y(0.7);
        let world0 = Mat4::from_translation(Vec3::new(0.0, 0.0, 90.0));
        let world1 = world0 * Mat4::from_rotation_translation(turned, Vec3::new(0.0, 5.0, -40.0));
        let mut model = RawModel {
            regions: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            bones: vec![
                RawBone {
                    name: "pelvis".into(),
                    parent: -1,
                    translation: [0.0, 0.0, 90.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0; 3],
                    inverse_bind: world0.inverse().to_cols_array(),
                },
                RawBone {
                    name: "thigh_l".into(),
                    parent: 0,
                    translation: [0.0, 5.0, -40.0],
                    rotation: turned.to_array(),
                    scale: [1.0; 3],
                    inverse_bind: world1.inverse().to_cols_array(),
                },
            ],
        };
        let before = model_world_frames(&model);
        straighten_frames(&mut model);
        let after = model_world_frames(&model);
        for (i, b) in model.bones.iter().enumerate() {
            let q = glam::Quat::from_array(b.rotation);
            assert!(
                q.angle_between(glam::Quat::IDENTITY) < 1e-4,
                "{}: identity",
                b.name
            );
            assert!(
                (after[i].w_axis - before[i].w_axis).length() < 1e-4,
                "{}: the joint stayed",
                b.name
            );
            let palette = after[i] * Mat4::from_cols_array(&b.inverse_bind);
            assert!(
                (palette - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-4),
                "{}: rest skinning stays the identity",
                b.name
            );
        }
    }

    /// A RIGGED body resizes as one thing: the joints take the mesh's map (grounded, centred,
    /// scaled), the binds follow, and the rest skin is still the identity.
    #[test]
    fn scale_mesh_to_stature_carries_a_skeleton_with_the_mesh() {
        let mut model = box_mesh(-20.0, 20.0, -10.0, 10.0, 5.0, 95.0);
        let bone = |name: &str, parent: i32, t: [f32; 3], world: Vec3| RawBone {
            name: name.into(),
            parent,
            translation: t,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            inverse_bind: Mat4::from_translation(world).inverse().to_cols_array(),
        };
        model.bones = vec![
            bone("pelvis", -1, [4.0, 0.0, 50.0], Vec3::new(4.0, 0.0, 50.0)),
            bone("spine_01", 0, [0.0, 0.0, 20.0], Vec3::new(4.0, 0.0, 70.0)),
        ];
        let rep = scale_mesh_to_stature(&mut model, 180.0);
        assert!((rep.scale - 2.0).abs() < 1e-5, "90 tall → 180: ×2");
        let world = model_world_frames(&model);
        let pelvis = world[0].w_axis.truncate();
        let spine = world[1].w_axis.truncate();
        assert!(
            (pelvis - Vec3::new(8.0, 0.0, 90.0)).length() < 1e-3,
            "the pelvis took the mesh's map (grounded 5→0, ×2): {pelvis}"
        );
        assert!(
            (spine - Vec3::new(8.0, 0.0, 130.0)).length() < 1e-3,
            "and its child rode along: {spine}"
        );
        for (b, w) in model.bones.iter().zip(&world) {
            let palette = *w * Mat4::from_cols_array(&b.inverse_bind);
            assert!(
                (palette - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-4),
                "{}: rest skinning stays the identity",
                b.name
            );
        }
    }

    #[test]
    fn scale_mesh_to_stature_grounds_and_centres() {
        // Arbitrary offset + unit scale: height 200, off-origin, off-plumb.
        let mut model = box_mesh(10.0, 70.0, -5.0, 25.0, 100.0, 300.0);
        let rep = scale_mesh_to_stature(&mut model, 170.0);
        assert!((rep.source_height - 200.0).abs() < 1e-3);
        assert!((rep.scale - 170.0 / 200.0).abs() < 1e-4);
        let (lo, hi) = bbox(&model);
        assert!((hi.z - lo.z - 170.0).abs() < 1e-2, "resized to stature");
        assert!(lo.z.abs() < 1e-2, "grounded on the floor");
        assert!(
            ((lo.x + hi.x) * 0.5).abs() < 1e-2 && ((lo.y + hi.y) * 0.5).abs() < 1e-2,
            "planted on the plumb line"
        );
    }

    #[test]
    fn install_baseline_skeleton_is_the_scaled_canon() {
        let mut model = RawModel {
            regions: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            bones: Vec::new(),
        };
        let stature = 190.0_f32; // an elf
        install_baseline_skeleton(&mut model, stature);
        let s = stature / crate::baseline::STATURE;
        let pos = crate::baseline::world_positions();

        // 66 bones, root excluded; pelvis leads and is the root of the set.
        assert_eq!(model.bones.len(), crate::baseline::CANON_BONES - 1);
        assert_eq!(model.bones[0].name, "pelvis");
        assert_eq!(model.bones[0].parent, -1);

        // Pelvis local == its scaled world (parent is the origin root); inverse_bind undoes it.
        let pelvis_w = s * pos["pelvis"];
        assert!((Vec3::from(model.bones[0].translation) - pelvis_w).length() < 1e-3);
        let ib = Mat4::from_cols_array(&model.bones[0].inverse_bind);
        assert!(
            ib.transform_point3(pelvis_w).length() < 1e-3,
            "inverse_bind maps the rest world back to the origin (bind == canon)"
        );

        // A child's local == the scaled parent→child offset.
        let idx: std::collections::HashMap<&str, usize> = model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let spine = &model.bones[idx["spine_01"]];
        let expect = s * (pos["spine_01"] - pos["pelvis"]);
        assert!((Vec3::from(spine.translation) - expect).length() < 1e-3);

        // bake_rig re-synthesizes the root → the full canon count.
        let rig = crate::bake::bake_rig(&model, "Elf");
        assert_eq!(rig.skeleton.bones.len(), crate::baseline::CANON_BONES);
        assert_eq!(rig.skeleton.bones[0].name, "root");
    }

    /// A CLOUD IS NOT A SHAPE (spec 04803E0C under rule 513E5F78): eight loose vertices have no
    /// flesh to thin, so the graph reads no structure at all, NOTHING matches, and the composed
    /// rest stands exactly where the canon put it — which is the fallback the whole matcher rests
    /// on (4BB12A75) and the state the Markers rail then prompts every joint from.
    ///
    /// It used to assert `arm_reach`'s per-side shoulder (`SHOULDER_FRACTION` of the widest
    /// vertex above a band) and its farthest-vertex fingertip. Both are DELETED (S3 67E4124B):
    /// stacked height and outboard bands read off a point cloud are the anatomy rule 513E5F78
    /// forbids, and on a cloud there is nothing for a chain to be laid along. The post-skinning
    /// weight pass `derive_shoulder_placement` still does that work at its own seam and keeps its
    /// own gate — see `shoulder_derivation_moves_the_glenohumeral_joint`.
    #[test]
    fn a_cloud_with_no_shape_leaves_the_composed_rest_standing() {
        // A sparse humanoid cloud (already stature-scaled): hips ±10, shoulders ±15, hands ±25.
        let h = 170.0_f32;
        let v = |x: f32, z: f32| RawVertex {
            p: [x, 0.0, z],
            n: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            joints: [0; 4],
            weights: [0.0; 4],
        };
        let verts = vec![
            v(10.0, 0.54 * h),
            v(-10.0, 0.54 * h),
            v(15.0, 0.82 * h),
            v(-15.0, 0.82 * h),
            v(25.0, 0.45 * h),
            v(-25.0, 0.45 * h),
            v(0.0, 0.0),
            v(0.0, h),
        ];
        let indices = (0..verts.len() as u32).collect();
        let mut m = RawModel {
            regions: Vec::new(),
            vertices: verts,
            indices,
            bones: Vec::new(),
        };
        // The canon this cloud must not be dragged off.
        let mut canon = m.clone();
        install_skeleton(&mut canon, &SkeletonRecipe::humanoid(), h).expect("composes");
        let cw = model_world_frames(&canon);
        let was = |n: &str| {
            pos_of(
                cw[canon
                    .bones
                    .iter()
                    .position(|b| b.name == n)
                    .expect("the canon composed it")],
            )
        };

        let report = fit_baseline_to_mesh(&mut m, h, &SkeletonRecipe::humanoid()).unwrap();

        // NOTHING BUT THE ROOT TRUNK MATCHED — two loose triangles voxelise into one blob, which
        // is a core and nothing else — so every limb joint is the human's to place, which is what
        // the rail reads.
        if let Some(sm) = report.shape.as_ref() {
            let limbs: Vec<&String> = sm
                .matched
                .iter()
                .map(|(id, _)| id)
                .filter(|id| !id.starts_with("trunk:"))
                .collect();
            assert!(
                limbs.is_empty(),
                "a cloud answers no limb module, got {limbs:?}"
            );
            for kind in ["head", "arm", "leg"] {
                let id = crate::baseline::module_id(kind, "");
                assert!(
                    sm.unmatched.contains(&id),
                    "{id} matched nothing and is prompted, got {}",
                    sm.report()
                );
            }
        }
        assert!(report.hand_placed, "every joint is prompted");
        assert!(!report.legs, "no leg was laid down anything");

        let w = model_world_frames(&m);
        let idx: HashMap<&str, usize> = m
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let wx = |n: &str| w[idx[n]].w_axis.x;
        // The one core the blob gives is the root trunk's, so the whole rest is MOVED onto it as
        // a piece. What no limb does is leave that piece: every limb joint keeps exactly the
        // offset the canon composed it at — no band, no farthest vertex, no reach.
        let (was_pelvis, now_pelvis) = (was("pelvis"), pos_of(w[idx["pelvis"]]));
        for n in [
            "thigh_l",
            "thigh_r",
            "upperarm_l",
            "upperarm_r",
            "hand_l",
            "hand_r",
            "middle_03_l",
        ] {
            let before = was(n) - was_pelvis;
            let now = pos_of(w[idx[n]]) - now_pelvis;
            assert!(
                now.distance(before) < 1e-3,
                "{n} keeps the offset the canon composed it at ({before}), got {now}"
            );
        }
        // And the torso is still PLUMB — over the pelvis, wherever the body's own symmetry plane
        // put that (the rig is moved onto the plane the graph measured; it is never bent).
        assert!(
            (wx("spine_02") - wx("pelvis")).abs() < 1e-3
                && (wx("head") - wx("pelvis")).abs() < 1e-3,
            "torso pulled off plumb: pelvis {}, spine_02 {}, head {}",
            wx("pelvis"),
            wx("spine_02"),
            wx("head")
        );
    }

    /// The digitigrade leg fixture's radii: hip, quadriceps BULGE, knee, calf BULGE, hock, foot
    /// pad, toe — the profile of incident D9D837FF, at three fifths of its original centimetres so
    /// that the fattest bulge stays under the core cut and the leg reads as a LIMB.
    const LEG_PROFILE: [f32; 7] = [3.0, 5.4, 3.0, 4.2, 2.1, 3.0, 2.4];

    /// A dense LEFT arm hanging from a shoulder at (15, 0, 139), BENT at an elbow at (25, −1, 108),
    /// to a fingertip at (35, −6, 71) — forward of the arm's line, like a hand that hangs — with
    /// the thickness profile of an arm along that bent path ([`arm_tube_fixture`]).
    const ARM_SHOULDER: Vec3 = Vec3::new(15.0, 0.0, 139.0);
    const ARM_ELBOW: Vec3 = Vec3::new(25.0, -1.0, 108.0);
    const ARM_TIP: Vec3 = Vec3::new(35.0, -6.0, 71.0);

    /// A LIMB ON A BODY — the fixture shape the shape graph can actually read (spec 04803E0C).
    /// A tube on its own is not a limb: it is the thickest thing in its own field, so the graph
    /// calls it a CORE and there is nothing for it to leave. Hang the same tube, and its mirror,
    /// off a torso and the graph reads a core with a symmetry pair on it — which is what the fit
    /// matches the recipe's modules to.
    ///
    /// The `torso` must be THICKER than the fattest bulge on the limb: a core is the flesh at
    /// least [`crate::shape::CORE_FRACTION`] as thick as the body's thickest, so a 9 cm
    /// quadriceps on a 12 cm barrel is itself a core and the limb never reads as a limb at all.
    fn on_a_body(limb: RawModel, torso: (Vec3, Vec3)) -> RawModel {
        let mut mirror = limb.clone();
        for v in &mut mirror.vertices {
            v.p[0] = -v.p[0];
            v.n[0] = -v.n[0];
        }
        // WHOLE TRIANGLES ONLY: a fixture's index list is a flat 0..n run, so its tail can be one
        // or two indices short of a triangle. `chunks_mut` hands that stub over and the winding
        // swap indexes past its end.
        for tri in mirror.indices.as_chunks_mut::<3>().0 {
            tri.swap(1, 2);
        }
        crate::flesh::fixtures::merge(vec![
            crate::flesh::fixtures::box_mesh(
                torso.0.x, torso.1.x, torso.0.y, torso.1.y, torso.0.z, torso.1.z,
            ),
            limb,
            mirror,
        ])
    }

    /// Where a fraction `t` of the bent arm's reach falls, in the fixture's own coordinates.
    fn arm_at(t: f32) -> Vec3 {
        let (l1, l2) = (
            ARM_ELBOW.distance(ARM_SHOULDER),
            ARM_TIP.distance(ARM_ELBOW),
        );
        let d = t * (l1 + l2);
        if d < l1 {
            ARM_SHOULDER + (ARM_ELBOW - ARM_SHOULDER) / l1 * d
        } else {
            ARM_ELBOW + (ARM_TIP - ARM_ELBOW) / l2 * (d - l1)
        }
    }

    fn arm_tube_fixture() -> RawModel {
        // THE SHARED TUBE BUILDER, not a hand-rolled ring stack: it emits a CLOSED surface, which
        // is what the voxel field and the shape graph read. The old stack pushed a flat `0..n`
        // index list, so its "triangles" were slivers across the rings, and the two bounding-box
        // POSTS on its end made one enormous triangle from the fingertip down to the origin —
        // which the graph correctly read as a 291 cm SHEET hanging off the torso, paired it with
        // the other arm, and laid the chain down neither.
        //
        // The thickness is an arm's: shoulder, BICEPS bulge, the ELBOW narrowing ON THE BEND, the
        // forearm's belly, the WRIST narrowing, the palm, the fingers. A joint is a tube's
        // narrowing and never its bulge (incident D9D837FF), so a bend with no narrowing on it is
        // a bend the matcher has nothing to find.
        let (l1, l2) = (
            ARM_ELBOW.distance(ARM_SHOULDER),
            ARM_TIP.distance(ARM_ELBOW),
        );
        let bend = l1 / (l1 + l2);
        crate::flesh::fixtures::tube(
            &[0.0, 0.25, bend, 0.60, 0.72, 0.76, 0.80, 0.90, 1.0].map(arm_at),
            &[5.0, 5.5, 3.6, 4.5, 2.8, 2.8, 2.8, 5.0, 2.5],
        )
    }

    /// THE LEG-FIT GUARD (2026-09-07, the lizard's raptor stance; re-cut 2026-09-11 for the
    /// NARROWING landmarks): a digitigrade leg tube that zig-zags — hip, knee forward, hock back,
    /// toe forward on the ground — AND bulges between its joints ([`LEG_PROFILE`]: a quadriceps
    /// above the knee, a calf above the hock, a pad below it). The knee lands on the narrowing and
    /// NOT on the thigh bulge, the ankle on the hock and not on the calf bulge, and the ball — the
    /// chain's last bone — at the END of the leg, which is the rule that puts a hoof on the floor.
    /// The J-shaped leg of incident D9D837FF, which the old extreme read (most-forward slab =
    /// knee) produced on exactly this shape. The arm module finds no pair on a leg-only body and
    /// keeps its composed rest, so no fingertip comes down the leg.
    #[test]
    fn fit_baseline_lays_the_leg_chain_down_a_digitigrade_leg() {
        let (hip, knee, hock, toe) = (
            Vec3::new(8.0, 0.0, 90.0),
            Vec3::new(8.0, -8.0, 60.0),
            Vec3::new(8.0, 4.0, 25.0),
            Vec3::new(8.0, -14.0, 5.0),
        );
        // The BULGES the joints must not be read as: the quadriceps above the knee, the calf above
        // the hock, the foot's pad below it. The thigh's 9 cm reaches x = 17, still inboard of nine
        // tenths of the shoulder band's 20, so the arm fit has nothing to grab below the shoulder.
        let (thigh_bulge, calf_bulge) = (hip.lerp(knee, 0.4), knee.lerp(hock, 0.35));
        let foot_pad = hock.lerp(toe, 0.45);
        let mut m = on_a_body(
            crate::flesh::fixtures::tube(
                &[hip, thigh_bulge, knee, calf_bulge, hock, foot_pad, toe],
                // The SAME profile, scaled so the fattest bulge stays UNDER the core cut: a
                // quadriceps at least half as thick as the barrel is itself a CORE to the graph,
                // and the leg then never reads as a limb at all. Scaling every radius alike
                // leaves the narrowings — which is what this gate is about — where they were.
                &LEG_PROFILE,
            ),
            // The torso's UNDERSIDE sits just above the hip, as a body's does. A leg whose tube
            // runs far up inside a wide block joins that block's own centreline, and both legs'
            // attachments then read as MIDLINE — no symmetry pair, and nothing for the recipe's
            // leg module to match.
            (Vec3::new(-18.0, -12.0, 87.0), Vec3::new(18.0, 12.0, 150.0)),
        );
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.legs = vec![flicker_skeletal::format::LegKind::Digitigrade { heel: 0.15 }];
        fit_baseline_to_mesh(&mut m, 170.0, &recipe).unwrap();
        let w = model_world_frames(&m);
        let at = |n: &str| pos_of(w[m.bones.iter().position(|b| b.name == n).unwrap()]);
        // THE JOINTS ARE THE NARROWINGS, never the bulges (incident D9D837FF — reading extremes
        // put the lizard's knee on its quadriceps). What the graph guarantees is that the knee is
        // nearer the leg's own narrowing than either bulge beside it, on the tube the matcher
        // chose; the exact centimetre is the mesh's business, not the canon's.
        assert!(
            at("calf_l").distance(knee) < at("calf_l").distance(thigh_bulge),
            "knee nearer the NARROWING at {knee} than the thigh bulge at {thigh_bulge}, got {}",
            at("calf_l")
        );
        assert!(
            at("calf_l").distance(thigh_bulge) > 8.0,
            "and NOT on the thigh bulge at {thigh_bulge} (incident D9D837FF), got {}",
            at("calf_l")
        );
        assert!(
            at("foot_l").distance(hock) < at("foot_l").distance(calf_bulge),
            "ankle nearer the hock's narrowing at {hock} than the calf bulge, got {}",
            at("foot_l")
        );
        assert!(
            at("foot_l").distance(calf_bulge) > 8.0,
            "and NOT in the calf bulge at {calf_bulge}, got {}",
            at("foot_l")
        );
        // THE GROUND JOINT IS THE END OF THE TUBE (S2 431D08DF), not a toe root hunted for
        // behind it: the lead is extended while the flesh lasts, so the chain's last bone stands
        // where the leg stops. `fit_leg`'s toe-root read — the one that put the ball back up the
        // foot — is deleted with it.
        assert!(
            at("ball_l").distance(toe) < 5.0,
            "the ball stands at the END of the leg ({toe}), got {}",
            at("ball_l")
        );
        assert!(
            at("ball_l").z < at("foot_l").z && at("ball_l").y < at("foot_l").y,
            "and below and ahead of the ankle at {}, got {}",
            at("foot_l"),
            at("ball_l")
        );
        assert!(
            (at("thigh_l").x - 8.0).abs() < 2.0,
            "hip on the tube, got {}",
            at("thigh_l")
        );
        // Every joint the fit places lands INSIDE the flesh — it reads the voxel field, so a joint
        // hanging in the air is the field being misread. (Measured on all five promoted bodies:
        // `cargo test -p flicker-content -- --ignored diagnose_leg --nocapture`.)
        let flesh = Flesh::build(&m);
        for n in ["thigh_l", "calf_l", "foot_l"] {
            assert!(
                flesh.contains(at(n)),
                "{n} sits in the flesh, got {}",
                at(n)
            );
        }
        // The right side is the MIRROR (`on_a_body`), laid on its own tube and mirroring the
        // left; the arm module found no pair and kept its composed rest.
        assert!(
            (at("thigh_r").x + at("thigh_l").x).abs() < 2.0
                && (at("thigh_r").z - at("thigh_l").z).abs() < 2.0,
            "thigh_r mirrors thigh_l, got {} vs {}",
            at("thigh_r"),
            at("thigh_l")
        );
        assert!(
            at("hand_l").z > 0.4 * 170.0,
            "no fingertip below the knee, got {}",
            at("hand_l")
        );
    }

    /// THE TAIL-FIT GUARD (2026-09-07, the lizard's curling tail): a tail tube that sweeps back
    /// and down behind the pelvis gets every chain bone on its arc.
    #[test]
    fn fit_baseline_lays_the_tail_chain_along_the_tail() {
        let pelvis = Vec3::new(0.0, 0.0, 0.560 * 170.0);
        let centre = pelvis + Vec3::new(0.0, 14.0, -30.0);
        let arc: Vec<Vec3> = (0..=8)
            .map(|i| {
                let a = std::f32::consts::FRAC_PI_2 * (1.0 - i as f32 / 8.0);
                centre + Vec3::new(0.0, 30.0 * a.cos(), 30.0 * a.sin())
            })
            .collect();
        let radii: Vec<f32> = (0..=8).map(|i| 4.0 - 0.3 * i as f32).collect();
        let mut m = crate::flesh::fixtures::merge(vec![
            crate::flesh::fixtures::box_mesh(-16.0, 16.0, -14.0, 14.0, 78.0, 150.0),
            crate::flesh::fixtures::tube(&arc, &radii),
            crate::flesh::fixtures::tube(
                &[Vec3::new(9.0, 0.0, 84.0), Vec3::new(9.0, 0.0, 0.0)],
                &[6.0, 6.0],
            ),
            crate::flesh::fixtures::tube(
                &[Vec3::new(-9.0, 0.0, 84.0), Vec3::new(-9.0, 0.0, 0.0)],
                &[6.0, 6.0],
            ),
        ]);
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.tails = vec![flicker_skeletal::format::TailKind::Long { bones: 8 }];
        fit_baseline_to_mesh(&mut m, 170.0, &recipe).unwrap();
        let w = model_world_frames(&m);
        for i in 1..=8 {
            let name = format!("tail_{i:02}");
            let p = pos_of(w[m.bones.iter().position(|b| b.name == name).unwrap()]);
            let off_arc = (p - centre).length() - 30.0;
            assert!(
                off_arc.abs() < 5.0,
                "{name} sits on the tail's arc, got {p} ({off_arc:.1} off)"
            );
            assert!(p.y > pelvis.y + 5.0, "{name} is behind the pelvis, got {p}");
        }
        let (first, last) = (
            pos_of(w[m.bones.iter().position(|b| b.name == "tail_01").unwrap()]),
            pos_of(w[m.bones.iter().position(|b| b.name == "tail_08").unwrap()]),
        );
        assert!(
            last.z < first.z - 15.0 && last.y > first.y + 10.0,
            "the chain runs down and back"
        );
    }

    /// THE HAND-FIT GUARD (2026-09-07, GolemBaseV2's folded palms and its forearm bound to the
    /// fingers; re-cut 2026-09-22 onto the matcher): on a dense BENT arm hanging off a torso, the
    /// chain the graph matched is laid down that arm's own tube — the ELBOW joint on the arm's
    /// NARROWING at the bend and not on the biceps or the forearm belly either side of it, the
    /// WRIST on the tube's thinnest slice, and every bone of the chain inside the tube.
    ///
    /// What it no longer asserts, and why: `arm_reach` found the hand as the farthest vertex
    /// outboard of a shoulder band and chained the arm's three sub-chains onto it with a
    /// `Similarity`. Both are DELETED (S3 67E4124B). The fingers now RIDE the chain bone they
    /// hang from (S2 431D08DF), so `middle_03_l` carries the canon's own hand geometry along with
    /// the wrist instead of reaching for the mesh's fingertip — it is the DEEP CHAIN's last bone
    /// that lands at the end of the tube. The fingertip-on-the-far-end assertion went with the
    /// `Similarity` that produced it.
    #[test]
    fn fit_baseline_lays_the_arm_chain_down_the_mesh_arm() {
        let mut m = on_a_body(
            arm_tube_fixture(),
            (Vec3::new(-12.0, -12.0, 80.0), Vec3::new(12.0, 12.0, 150.0)),
        );
        fit_baseline_to_mesh(&mut m, 170.0, &SkeletonRecipe::humanoid()).unwrap();
        let w = model_world_frames(&m);
        let at = |n: &str| pos_of(w[m.bones.iter().position(|b| b.name == n).unwrap()]);
        let (l1, l2) = (
            ARM_ELBOW.distance(ARM_SHOULDER),
            ARM_TIP.distance(ARM_ELBOW),
        );
        let u2 = (ARM_TIP - ARM_ELBOW) / l2;
        let wrist = ARM_ELBOW + u2 * (0.76 * (l1 + l2) - l1);
        // The BULGES the elbow must not be read as (incident D9D837FF, the rule the leg gate
        // holds in the same form): the biceps above the bend, the forearm's belly below it.
        let (biceps, belly) = (arm_at(0.25), arm_at(0.60));
        assert!(
            at("lowerarm_l").distance(ARM_ELBOW) < at("lowerarm_l").distance(biceps)
                && at("lowerarm_l").distance(ARM_ELBOW) < at("lowerarm_l").distance(belly),
            "the elbow is nearer the NARROWING on the bend ({ARM_ELBOW}) than the biceps at \
             {biceps} or the forearm belly at {belly}, got {}",
            at("lowerarm_l")
        );
        assert!(
            at("lowerarm_l").distance(biceps) > 8.0,
            "and NOT up on the biceps at {biceps}, got {}",
            at("lowerarm_l")
        );
        assert!(
            at("hand_l").distance(wrist) < 3.0,
            "the wrist lands on the thinnest slice ({wrist}), got {}",
            at("hand_l")
        );
        // THE DEEP CHAIN reaches the end of the arm: its last bone stands at the tube's far end,
        // the same rule that puts a ground joint on the floor and a tail's last bone at the end
        // of the tail (S2 431D08DF).
        let deep = deep_chain(&m, bone_at(&m, "upperarm_l").expect("the arm composes"));
        let last = *deep.last().expect("a chain");
        let tip = pos_of(w[last]);
        assert!(
            tip.distance(ARM_TIP) < 6.0,
            "the chain's last bone ({}) lands at the tube's far end ({ARM_TIP}), got {tip}",
            m.bones[last].name
        );
        // The chain bones ride the tube's axis; the finger FAN rides the hand, so it spreads the
        // canon's own hand-width either side of it.
        // ON the chain: within the tube's own thickness of its axis. RIDING a chain bone (the
        // forearm twist, the finger fan): the canon's own offset from that bone, carried along —
        // so within a hand's length of the axis, not on it.
        for (n, within) in [
            ("lowerarm_l", 4.0),
            ("hand_l", 4.0),
            ("lowerarm_twist_01_l", 9.0),
            ("middle_03_l", 9.0),
            ("index_02_l", 9.0),
            ("thumb_01_l", 9.0),
            ("pinky_03_l", 9.0),
        ] {
            let d = at(n) - ARM_ELBOW;
            let off_axis = (d - u2 * d.dot(u2)).length();
            assert!(
                off_axis < within,
                "{n} lies within {within} cm of the forearm/hand axis, got {off_axis:.1} cm"
            );
        }
        // The right side is the MIRROR (`on_a_body`) and is laid on its own tube, so its wrist
        // lands where the left's does, reflected.
        assert!(
            (at("hand_r").x + at("hand_l").x).abs() < 3.0
                && (at("hand_r").z - at("hand_l").z).abs() < 3.0,
            "hand_r mirrors hand_l, got {} vs {}",
            at("hand_r"),
            at("hand_l")
        );
    }

    /// THE CHAIN FILL (spec 76EB9552, the guided rig): a chain whose ENDS a human placed and
    /// whose middle joint is unplaced lands that middle on the flesh's NARROWING nearest its rest
    /// proportion — the knee, not the midpoint of the straight line; ends placed a little off the
    /// tube's axis still fill ON the axis; and a joint with no fixed joint beyond it is only
    /// brought onto the flesh, never moved along a chain it has no end for.
    #[test]
    fn fill_chain_lands_the_unplaced_joints_on_the_narrowings() {
        use crate::flesh::fixtures::{bulge_leg, BULGE_HIP, BULGE_KNEE, BULGE_TOE};
        let flesh = Flesh::build(&bulge_leg());
        let (top, toe) = (
            Vec3::new(0.0, 0.0, BULGE_HIP),
            Vec3::new(0.0, 0.0, BULGE_TOE),
        );
        let mut pts = vec![top, top.lerp(toe, 0.5), toe];
        fill_chain(&flesh, &mut pts, &[true, false, true]);
        assert!(
            (pts[1].z - BULGE_KNEE).abs() < 2.0,
            "the unplaced middle lands on the knee narrowing ({BULGE_KNEE}), got {}",
            pts[1]
        );
        assert!(
            pts[0] == top && pts[2] == toe,
            "the placed ends are never moved, got {} and {}",
            pts[0],
            pts[2]
        );
        // Ends dropped 3 cm off the tube's axis: the fill still rides the axis.
        let off = Vec3::new(3.0, 0.0, 0.0);
        let mut pts = vec![top + off, top.lerp(toe, 0.5) + off, toe + off];
        fill_chain(&flesh, &mut pts, &[true, false, true]);
        assert!(
            pts[1].x.abs() < 1.5 && (pts[1].z - BULGE_KNEE).abs() < 3.0,
            "an off-axis chain still fills on the axis at the knee, got {}",
            pts[1]
        );
        // A loose end (nothing fixed beyond it) only comes onto the flesh's medial point. Seeded a
        // little inside the end caps: `top` and `toe` ARE the caps, so a seed on one reads no
        // inscribed radius at all and `fill_chain`'s reach collapses to its floor — a joint on the
        // skin, not a loose end in the limb.
        let inside = Vec3::new(3.0, 0.0, 0.0) - Vec3::Z * 3.0;
        let mut pts = vec![top + inside, top.lerp(toe, 0.5), toe - inside];
        fill_chain(&flesh, &mut pts, &[false, true, false]);
        assert!(
            pts[0].x.abs() < 1.5 && flesh.contains(pts[0]) && flesh.contains(pts[2]),
            "the loose ends centre on the flesh, got {} and {}",
            pts[0],
            pts[2]
        );
    }

    /// DIAGNOSTIC on REAL flesh (incident D9D837FF: *"measure against the lizard's mesh before
    /// trusting it on the golem"*): re-fit every promoted body's own mesh from its own recipe and
    /// print the leg landmarks. On LizardBaseA the old EXTREME read put the knee at z 69.5 / 70.7
    /// and the hock at 31.8 / 28.0, with the ball a quarter back from the toe tip; Aaron's eye put
    /// that mesh's knee near 64 and its heel bend lower. `#[ignore]`d (needs the content tree);
    /// run: `cargo test -p flicker-content -- --ignored diagnose_leg --nocapture`.
    #[test]
    #[ignore]
    fn diagnose_leg_landmarks_on_the_promoted_bodies() {
        for body in [
            "LizardBaseA",
            "ElfBaseA",
            "GolemBaseV2",
            "HumanBaseA",
            "DarkElfBaseA",
        ] {
            let rig = crate::roots::roots()
                .package()
                .join(format!("characters/{body}/{body}.json"));
            if !crate::package::file_exists(&rig) {
                eprintln!("skipping {body}: not promoted");
                continue;
            }
            let text = crate::package::read_text(&rig).expect("the promoted rig reads");
            let file: flicker_skeletal::format::RigFile =
                serde_json::from_str(&text).expect("the promoted rig parses");
            let recipe = file.skeleton_recipe.clone().unwrap_or_else(|| {
                crate::baseline::reference_recipe(crate::baseline::Pattern::Humanoid)
            });
            let mut model = crate::bake::load_rig_raw(&rig).expect("the rig loads as a raw model");
            let stature = model
                .vertices
                .iter()
                .map(|v| v.p[2])
                .fold(f32::MIN, f32::max);
            // What the PROMOTED rig carries — on LizardBaseA that is Aaron's own hand-corrected
            // placement (incident D9D837FF), the only ground truth there is for a leg landmark.
            let promoted = model_world_frames(&model);
            let was: HashMap<String, Vec3> = model
                .bones
                .iter()
                .enumerate()
                .map(|(i, b)| (b.name.clone(), pos_of(promoted[i])))
                .collect();
            let t0 = std::time::Instant::now();
            let report = fit_baseline_to_mesh(&mut model, stature, &recipe).expect("the fit runs");
            let w = model_world_frames(&model);
            let at = |n: &str| {
                model
                    .bones
                    .iter()
                    .position(|b| b.name == n)
                    .map(|i| pos_of(w[i]))
            };
            println!(
                "{body}: {:.0} cm, {} verts, legs fitted: {}, fit {} ms",
                stature,
                model.vertices.len(),
                report.legs,
                t0.elapsed().as_millis()
            );
            let probe = Flesh::build(&model);
            for bone in ["thigh", "calf", "foot", "ball"] {
                for side in ["l", "r"] {
                    let name = format!("{bone}_{side}");
                    let (Some(fit), Some(old)) = (at(&name), was.get(&name).copied()) else {
                        continue;
                    };
                    println!(
                        "  {name:>8}: fit ({:6.1},{:6.1},{:6.1})  rig ({:6.1},{:6.1},{:6.1})  \
                         Δ{:5.1} cm  inside: {}",
                        fit.x,
                        fit.y,
                        fit.z,
                        old.x,
                        old.y,
                        old.z,
                        fit.distance(old),
                        probe.contains(fit)
                    );
                }
            }
        }
    }

    /// A raw mesh rigged on a DIGITIGRADE recipe (the modular skeleton, P2): the same 66 bones
    /// with the ankle raised — the box body skins onto it and bakes a valid rig, as the humanoid.
    #[test]
    fn a_raw_mesh_rigs_on_a_digitigrade_recipe() {
        let mut model = box_mesh(-30.0, 30.0, -15.0, 15.0, 0.0, 180.0);
        let mut recipe = SkeletonRecipe::humanoid();
        recipe.trunk.legs = vec![flicker_skeletal::format::LegKind::Digitigrade { heel: 0.15 }];
        rig_raw_mesh(&mut model, 170.0, &recipe).expect("the digitigrade recipe rigs");
        assert_eq!(model.bones.len(), crate::baseline::CANON_BONES - 1);
        let w = model_world_frames(&model);
        // The ankle is no longer the composed HEEL KNOB: a body with flesh on it has its leg
        // chain laid down its own leg tube, so the ankle is wherever that tube narrows between
        // the knee and the ground joint (spec 04803E0C). What must hold on every body is the
        // ORDER — knee above ankle above ball — and that is what this asserts.
        let at = |n: &str| pos_of(w[model.bones.iter().position(|b| b.name == n).unwrap()]);
        assert!(
            at("calf_l").z > at("foot_l").z && at("foot_l").z > at("ball_l").z,
            "knee {:.1} above ankle {:.1} above ball {:.1}",
            at("calf_l").z,
            at("foot_l").z,
            at("ball_l").z
        );
        for v in &model.vertices {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3, "weights normalised");
        }
    }

    /// A raw mesh rigged on a QUADRUPED recipe (P3): the trunk lies along the body, no humanoid
    /// fit runs (the composed rest is the placement, for the human to move), the body still
    /// skins onto it and the weights normalise.
    #[test]
    fn a_raw_mesh_rigs_on_a_quadruped_recipe_with_the_fits_off() {
        // A body longer than it is tall, standing on the ground.
        let mut model = box_mesh(-25.0, 25.0, -90.0, 60.0, 0.0, 110.0);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let mut probe = model.clone();
        scale_mesh_to_stature(&mut probe, 170.0);
        let report = fit_baseline_to_mesh(&mut probe, 170.0, &recipe).expect("installs");
        assert!(
            report.hand_placed && !report.legs,
            "every fit stays off a quadruped"
        );
        rig_raw_mesh(&mut model, 170.0, &recipe).expect("the quadruped recipe rigs");
        assert_eq!(
            model.bones.len(),
            crate::baseline::compose(&recipe, 170.0).unwrap().len() - 1,
            "every composed bone but the synthesized root is installed"
        );
        let w = model_world_frames(&model);
        let at = |name: &str| {
            let i = model.bones.iter().position(|b| b.name == name).unwrap();
            pos_of(w[i])
        };
        assert!(
            at("pelvis").y > at("spine_03").y + 50.0,
            "the trunk lies along the body: pelvis {} spine_03 {}",
            at("pelvis"),
            at("spine_03")
        );
        assert!(
            at("head").y < at("neck_02").y,
            "the head is carried forward: spine_03 {} neck_01 {} neck_02 {} head {}",
            at("spine_03"),
            at("neck_01"),
            at("neck_02"),
            at("head")
        );
        for v in &model.vertices {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3, "weights normalised");
        }
    }

    /// THE FIT'S READ IS HANDED ON, NEVER REPEATED (0F0208AC's seam). `rig_raw_mesh` reads the
    /// body ONCE — the fit thins the mesh — binds its skin on that read, and hands it back in its
    /// [`FitReport`] beside the match: the bench's rail reads that match, the import squares its
    /// stance on that body, and neither thins the mesh a second time.
    #[test]
    fn rig_raw_mesh_hands_back_the_fits_own_read() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let mut parts = vec![box_mesh(-18.0, 18.0, -12.0, 12.0, 85.0, 170.0)];
        for x in [9.0_f32, -9.0] {
            parts.push(tube(
                &[Vec3::new(x, 0.0, 88.0), Vec3::new(x, 0.0, 0.0)],
                &[5.0, 5.0],
            ));
        }
        let mut model = merge(parts);
        let recipe = SkeletonRecipe::humanoid();
        let reads = ShapeGraph::reads();
        let fit = rig_raw_mesh(&mut model, 170.0, &recipe).expect("the biped rigs");
        assert_eq!(
            ShapeGraph::reads() - reads,
            1,
            "the body is read once, by the fit"
        );
        let body = fit.body.as_ref().expect("the fit hands back its read");
        let graph = body.graph.as_ref().expect("a biped has a graph");
        let m = fit.shape.as_ref().expect("the fit hands back its match");
        let fresh = match_recipe(graph, &recipe);
        assert_eq!(
            (&m.matched, &m.unmatched, &m.capped),
            (&fresh.matched, &fresh.unmatched, &fresh.capped),
            "the report's match is the one the fit acted on"
        );
        assert!(
            matches!(
                m.of(&crate::baseline::module_id("leg", "")),
                Some(Matched::Pair(_))
            ),
            "the legs matched a pair:\n{}",
            m.report()
        );
        // THE SKIN IS THE TUBE BIND ON THAT READ: binding again on it changes nothing, and the
        // plain distance bind — no read at all — is a different skin.
        let (mut again, mut plain) = (model.clone(), model.clone());
        crate::bake::bind(&mut again, Some(body));
        crate::bake::bind(&mut plain, None);
        assert_eq!(
            format!("{:?}", again.vertices),
            format!("{:?}", model.vertices)
        );
        assert_ne!(
            format!("{:?}", plain.vertices),
            format!("{:?}", model.vertices)
        );
        // ...and squaring the stance on it reads nothing more.
        let reads = ShapeGraph::reads();
        crate::bake::square_stance_on(
            &mut model,
            crate::bake::StanceSource::Auto,
            &recipe,
            fit.body.as_ref(),
        );
        assert_eq!(
            ShapeGraph::reads(),
            reads,
            "the stance is squared on the fit's read"
        );
    }

    #[test]
    fn boneless_mesh_rigs_and_bakes_to_a_valid_rig() {
        let mut model = box_mesh(-30.0, 30.0, -15.0, 15.0, 0.0, 180.0);
        assert!(model.bones.is_empty(), "starts with no skeleton");

        scale_mesh_to_stature(&mut model, 170.0);
        install_baseline_skeleton(&mut model, 170.0);
        crate::bake::bake_skin(&mut model);

        // Every vertex carries a normalised weight set pointing at real (66-set) bones.
        for v in &model.vertices {
            let sum: f32 = v.weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-3, "weights not normalised: {sum}");
            for (k, &j) in v.joints.iter().enumerate() {
                if v.weights[k] > 0.0 {
                    assert!((j as usize) < model.bones.len(), "joint out of range");
                }
            }
        }

        let rig = crate::bake::bake_rig(&model, "Body");
        assert_eq!(rig.skeleton.bones.len(), crate::baseline::CANON_BONES);
        assert_eq!(rig.skeleton.bones[0].name, "root");
        for v in &rig.mesh.vertices {
            for &j in &v.joints {
                assert!(
                    (j as usize) < rig.skeleton.bones.len(),
                    "baked joint out of range"
                );
            }
        }
    }

    /// A SYNTHETIC HORSE — a box trunk with a withers block over the shoulder, four tube legs to
    /// the ground, a neck-and-head box carried ahead and above, a tail tube behind, the whole
    /// animal offset `shift` sideways. Its back over the rump stands at [`FIX_BACK`], so the
    /// stature `align_trunk` should measure off it is `FIX_BACK / QUAD_WITHERS` — while its
    /// BOUNDING height is the raised head, which is exactly the trap the incident is about.
    const FIX_BACK: f32 = 136.0;
    const FIX_BELLY: f32 = 76.0;
    const FIX_TRUNK_Y: f32 = 55.0;

    fn horse_fixture(shift: f32) -> RawModel {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let sx = |a: f32, b: f32| (a + shift, b + shift);
        let (t0, t1) = sx(-25.0, 25.0);
        let (w0, w1) = sx(-14.0, 14.0);
        let (n0, n1) = sx(-10.0, 10.0);
        let mut parts = vec![
            // The barrel, and the withers standing over the shoulder (a real topline is not flat:
            // the withers rides above the croup, which is what puts the withers JOINT inside).
            box_mesh(t0, t1, -FIX_TRUNK_Y, FIX_TRUNK_Y, FIX_BELLY, FIX_BACK),
            box_mesh(w0, w1, -52.0, -22.0, FIX_BACK, FIX_BACK + 8.0),
            // The neck and head carried ahead and ABOVE the back — the bounding-box height.
            box_mesh(n0, n1, -95.0, -50.0, FIX_BACK, 180.0),
            // The tail, behind and below the croup.
            tube(
                &[
                    Vec3::new(shift, 55.0, 130.0),
                    Vec3::new(shift, 78.0, 112.0),
                    Vec3::new(shift, 95.0, 95.0),
                ],
                &[4.0, 3.0, 2.0],
            ),
        ];
        for y in [-45.0_f32, 45.0] {
            for sign in [1.0_f32, -1.0] {
                let x = shift + sign * 16.0;
                parts.push(tube(
                    &[
                        Vec3::new(x, y, 80.0),
                        Vec3::new(x, y, 40.0),
                        Vec3::new(x, y, 0.0),
                    ],
                    &[5.0, 4.0, 4.0],
                ));
            }
        }
        merge(parts)
    }

    /// A HORNED, YAWED SOURCE (sweep 2026-09-21): the horse fixture wearing a pair of wide horns
    /// and yawed 37° off the axis — the shape seven of the seventeen hoofed sources actually have
    /// (Aurochs, Bison, DeerStag, ElkBull, MooseBull, Ram, Sheep: yawed 37–59°, and every one of
    /// them wider across its headgear than it is long). The BOUNDING-BOX guess that used to pick
    /// the facing ("a quarter-turn when the longest dimension is X") cannot square any of them;
    /// `measure_facing` reads the BODY's own axis off the core and hands back the yaw, after
    /// which the trunk measures and the rest lands inside the flesh with its hooves on the floor.
    #[test]
    fn a_yawed_horned_source_is_squared_onto_the_rig_before_the_trunk_is_measured() {
        use crate::flesh::fixtures::{box_mesh, merge};
        let yaw_deg = 37.0_f32;
        // The horse, plus horns spreading wider than the animal is long — thin, so they are never
        // core, but they own the bounding box, which is exactly why the box cannot be trusted.
        let mut model = merge(vec![
            horse_fixture(0.0),
            box_mesh(-110.0, 110.0, -82.0, -74.0, 150.0, 158.0),
        ]);
        let yaw = Mat4::from_rotation_z(yaw_deg.to_radians());
        crate::fbx::apply_orientation(&mut model, yaw);

        let flesh = Flesh::build_body(&model);
        let core = flesh.core().expect("the yawed horse still has a core");
        assert!(
            core.along > core.hi.z - core.lo.z,
            "the barrel is longer ({:.1}) than it is tall ({:.1}) — this body is LYING DOWN",
            core.along,
            core.hi.z - core.lo.z
        );
        let measured = measure_facing(&flesh);
        assert!(
            (measured + yaw_deg).abs() < 6.0,
            "the measured facing un-yaws the body it was yawed by (−{yaw_deg}°), got {measured:.1}°"
        );
        // Square it and the trunk measures — where, yawed, it could not be measured at all.
        face_to_rig(&mut model, measured, 0);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 180.0, &recipe).expect("the fit runs");
        let a = report.align.expect("a squared body has a trunk to measure");
        let flesh = Flesh::build_body(&model);
        assert!(flesh.contains(a.pelvis), "the pelvis is INSIDE the flesh");
        let withers = a.withers.expect("a quadruped measures its withers");
        assert!(flesh.contains(withers), "the withers are INSIDE the flesh");
        let w = model_world_frames(&model);
        let at = |n: &str| pos_of(w[model.bones.iter().position(|b| b.name == n).unwrap()]);
        for hoof in ["hoof_l", "hoof_r", "forehoof_l", "forehoof_r"] {
            assert!(
                at(hoof).z.abs() < 0.02 * a.stature,
                "{hoof} reaches the ground, got {}",
                at(hoof)
            );
        }
    }

    /// AN UPRIGHT BODY IS NEVER TURNED: the biped fixture's trunk is taller than it is long, so
    /// `measure_facing` reads no facing at all and the humanoid canon is left exactly where it
    /// stands. The guard that keeps the measured yaw off the five promoted humanoids.
    #[test]
    fn an_upright_body_measures_no_facing() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let (crotch, typed) = (85.0_f32, 170.0_f32);
        let mut parts = vec![box_mesh(-18.0, 18.0, -12.0, 12.0, crotch, typed)];
        for sign in [1.0_f32, -1.0] {
            let x = sign * 9.0;
            parts.push(tube(
                &[Vec3::new(x, 0.0, 88.0), Vec3::new(x, 0.0, 0.0)],
                &[5.0, 5.0],
            ));
        }
        let model = merge(parts);
        let flesh = Flesh::build_body(&model);
        let core = flesh.core().expect("the biped fixture has a core");
        assert!(
            core.along <= core.hi.z - core.lo.z,
            "a standing trunk is taller ({:.1}) than it is long ({:.1})",
            core.hi.z - core.lo.z,
            core.along
        );
        assert_eq!(
            measure_facing(&flesh),
            0.0,
            "an upright body is already facing the rig"
        );
    }

    /// A HEAD AS THICK AS THE BARREL (sweep 2026-09-21): the Hippopotamus, the Rhinoceros, the
    /// Pig and the Boar carry a skull at least half as thick as their own barrel, so the trunk
    /// CORE — "the flesh at least half as thick as the thickest" — runs on into the SNOUT. Read
    /// off the core's front, the chest landed 45–55 cm ahead of the shoulder and the withers were
    /// composed inside the nose, outside the flesh. The chest is the GIRTH (the front of the
    /// belly run) instead: a midline run at belly height ends where the forelegs come down, and
    /// no head extends it, because a head is not something the belly line passes under.
    #[test]
    fn a_head_as_thick_as_the_barrel_does_not_stretch_the_chest_into_the_snout() {
        use crate::flesh::fixtures::{box_mesh, merge};
        // The horse fixture with a BLUNT head: a block as thick as the barrel, carried forward at
        // barrel height on a neck just as thick — a hippo, not a horse.
        let mut model = merge(vec![
            horse_fixture(0.0),
            box_mesh(-24.0, 24.0, -150.0, -95.0, 80.0, 132.0),
            box_mesh(-24.0, 24.0, -100.0, -50.0, 80.0, 134.0),
        ]);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 180.0, &recipe).expect("the fit runs");
        let a = report.align.expect("the blunt-headed fixture measures");
        let flesh = Flesh::build_body(&model);
        let core = flesh.core().expect("it has a core");
        let withers = a.withers.expect("a quadruped measures its withers");
        assert!(
            core.lo.y < -FIX_TRUNK_Y - 20.0,
            "the fixture is the failing SHAPE: its core runs on into the head ({:.1} against a \
             barrel front of {:.1})",
            core.lo.y,
            -FIX_TRUNK_Y
        );
        assert!(
            withers.y > core.lo.y + 20.0,
            "the withers are NOT dragged out to the core's front ({:.1}), got {:.1}",
            core.lo.y,
            withers.y
        );
        assert!(
            flesh.contains(withers),
            "the withers are INSIDE the flesh, got {withers}"
        );
        assert!(flesh.contains(a.pelvis), "the pelvis is INSIDE the flesh");
        let length = a.length.expect("a quadruped measures its length");
        assert!(
            (0.3..=1.2).contains(&length),
            "the barrel is a body's length, not a body and a skull: got {length:.3}"
        );
    }

    /// A barrel on four legs with a tail behind — the box quadruped of the shape gates, less its
    /// neck and head.
    fn headless_barrel() -> Vec<RawModel> {
        use crate::flesh::fixtures::{box_mesh, tube};
        let mut parts = vec![box_mesh(-14.0, 14.0, -40.0, 40.0, 60.0, 92.0)];
        for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let (x, y) = (sx * 10.0_f32, sy * 32.0_f32);
            parts.push(tube(
                &[Vec3::new(x, y, 0.0), Vec3::new(x, y, 66.0)],
                &[4.0, 4.0],
            ));
        }
        parts.push(tube(
            &[Vec3::new(0.0, 38.0, 78.0), Vec3::new(0.0, 88.0, 78.0)],
            &[3.5, 3.5],
        ));
        parts
    }

    /// THE GATE ON THE FRONT CAP. A skull as thick as the barrel's own front, carried on no neck:
    /// one core from rump to snout, with nothing ahead of its front end — no chained core, no
    /// tube (the bear, the panther, the raccoon of the fresh sweep). The head module still matches
    /// nothing and is still PROMPTED, but it no longer keeps the composed rest out in front of the
    /// body: it lands on the core's FRONT CAP, beyond the shoulders, inside the flesh. A barrel
    /// that ENDS at its forelegs has no cap beyond them, and there the composed rest stands.
    #[test]
    fn a_skull_merged_into_the_barrel_puts_the_head_on_its_front_cap() {
        use crate::flesh::fixtures::{box_mesh, merge};
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let head = crate::baseline::module_id("head", "");

        let mut bare = merge(headless_barrel());
        let m = fit_baseline_to_mesh(&mut bare, 110.0, &recipe)
            .expect("the fit runs")
            .shape
            .expect("the barrel has a shape");
        assert!(
            m.unmatched.contains(&head) && m.capped.is_empty(),
            "a barrel ending at its forelegs has no cap:\n{}",
            m.report()
        );

        let mut parts = headless_barrel();
        parts.push(box_mesh(-10.0, 10.0, -62.0, -38.0, 66.0, 96.0));
        let mut model = merge(parts);
        let m = fit_baseline_to_mesh(&mut model, 110.0, &recipe)
            .expect("the fit runs")
            .shape
            .expect("the body has a shape");
        assert!(
            m.unmatched.contains(&head),
            "the head is still prompted:\n{}",
            m.report()
        );
        assert!(
            m.capped.iter().any(|(id, _)| *id == head),
            "...and placed on the front cap:\n{}",
            m.report()
        );
        let flesh = Flesh::build_body(&model);
        let w = model_world_frames(&model);
        let at = |n: &str| pos_of(w[model.bones.iter().position(|b| b.name == n).unwrap()]);
        let (h, withers) = (at("head"), at("spine_03"));
        assert!(flesh.contains(h), "the head is inside the skull: {h}");
        assert!(
            h.y < -45.0 && h.y < withers.y - 10.0,
            "in the cap, ahead of the shoulders (the rig's forward is −Y): head {h}, spine_03 {withers}"
        );
    }

    /// THE GATE ON GRAVITY IN THE MATCH. A head as thick as the barrel's front (one core, snout to
    /// rump) carrying a pair of antlers ahead of the forelegs: the antlers ARE a symmetry pair,
    /// and the FRONT-MOST one. The recipe's shoulder module is a foreleg that ends in a ground
    /// contact, so it takes only a pair that reaches the floor — the forelegs, never the antlers
    /// (the DeerStag of the fresh sweep had its shoulders laid up its antlers).
    #[test]
    fn a_pair_of_antlers_is_never_laid_out_as_the_shoulders() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let mut parts = headless_barrel();
        parts.push(box_mesh(-10.0, 10.0, -62.0, -38.0, 66.0, 96.0));
        for sx in [1.0_f32, -1.0] {
            parts.push(tube(
                &[
                    Vec3::new(sx * 4.0, -52.0, 94.0),
                    Vec3::new(sx * 26.0, -52.0, 128.0),
                ],
                &[2.5, 2.0],
            ));
        }
        let flesh = Flesh::build_body(&merge(parts));
        let g = ShapeGraph::build(&flesh).expect("a graph");
        let antlers = g
            .pairs
            .iter()
            .position(|p| !g.limbs[p.l].grounded && !g.limbs[p.r].grounded)
            .unwrap_or_else(|| panic!("the antlers are a pair of the graph: {}", g.detail()));
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let m = match_recipe(&g, &recipe);
        let Some(Matched::Pair(i)) = m.of(&crate::baseline::module_id("arm", "")) else {
            panic!(
                "the shoulders match a pair:\n{}\n{}",
                m.report(),
                g.detail()
            );
        };
        let p = &g.pairs[i];
        assert!(
            i != antlers && (g.limbs[p.l].grounded || g.limbs[p.r].grounded),
            "the shoulders are the forelegs, not the antlers:\n{}\n{}",
            m.report(),
            g.detail()
        );
    }

    /// HORNS ARE RIGID (Aaron on the horned bodies, 2026-10-04: *"the horns pick up weight from
    /// the bones … there are artifacts visually of solid planes"*). The antlered barrel of the
    /// gate above, fitted and bound: every vertex of both antlers hangs WHOLLY on one bone, the
    /// same one for the two of them, and it is the head's — where read by distance alone each
    /// tine is shared out among whatever bones lie nearest it, and any pose that turns those
    /// against each other shears it into sheets. Nothing of the legs goes with them.
    #[test]
    fn a_pair_of_antlers_is_rigid_with_the_head() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let mut parts = headless_barrel();
        parts.push(box_mesh(-10.0, 10.0, -62.0, -38.0, 66.0, 96.0));
        let trunk = merge(parts.clone()).vertices.len();
        for sx in [1.0_f32, -1.0] {
            parts.push(tube(
                &[
                    Vec3::new(sx * 4.0, -52.0, 94.0),
                    Vec3::new(sx * 26.0, -52.0, 128.0),
                ],
                &[2.5, 2.0],
            ));
        }
        let mut model = merge(parts);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 110.0, &recipe).expect("the fit runs");
        let body = report.body.expect("the fit hands back its read");
        let owner = |m: &RawModel, v: &crate::fbx::RawVertex| {
            let k = (0..4)
                .max_by(|&a, &b| v.weights[a].total_cmp(&v.weights[b]))
                .unwrap_or(0);
            (m.bones[v.joints[k] as usize].name.clone(), v.weights[k])
        };
        // The antlers' own flesh: their vertices clear of the skull they stand on.
        let top = model.vertices[..trunk]
            .iter()
            .map(|v| v.p[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let antlers = |m: &RawModel| -> Vec<(String, f32)> {
            m.vertices[trunk..]
                .iter()
                .filter(|v| v.p[2] > top + 4.0)
                .map(|v| owner(m, v))
                .collect()
        };
        let mut plain = model.clone();
        crate::bake::bind(&mut plain, None);
        let shared = antlers(&plain);
        assert!(shared.len() > 100, "the fixture has antlers");
        assert!(
            shared.iter().filter(|(_, w)| *w < 0.999).count() > shared.len() / 4,
            "read by distance alone the antlers are shared out among the bones near them"
        );
        crate::bake::bind(&mut model, Some(&body));
        let rigid = antlers(&model);
        assert!(
            rigid.iter().all(|(b, w)| *w > 0.999 && b == "head"),
            "every antler vertex is the head's alone: {:?}",
            rigid.iter().find(|(b, w)| *w <= 0.999 || b != "head")
        );
        let head = model.bones.iter().position(|b| b.name == "head").unwrap() as u32;
        assert!(
            model.vertices[..trunk]
                .iter()
                .filter(|v| v.p[2] < top - 50.0)
                .all(|v| (0..4).all(|k| v.joints[k] != head || v.weights[k] == 0.0)),
            "nothing of the legs goes with the head"
        );
    }

    /// THE TRUNK (Aaron, ruling 7881216F: *"trunk should curl and reach"*). The barrel with a
    /// NECK, a head on it and a long thin tube hanging from the head's front nearly to the ground
    /// — what the fit reported as "a tube running on past the head's mass the recipe has no
    /// module for". With a proboscis in the recipe that tube IS the module's: its chain is laid
    /// down it from where the tube leaves the head's mass to its tip, the head stays on its own
    /// mass, and the tube's flesh is skinned to the chain — no longer a rigid bare end.
    #[test]
    fn a_proboscis_is_laid_down_the_tube_past_the_head_and_skinned_to_its_own_bones() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let mut parts = headless_barrel();
        parts.push(tube(
            &[Vec3::new(0.0, -38.0, 80.0), Vec3::new(0.0, -68.0, 94.0)],
            &[7.0, 7.0],
        ));
        parts.push(box_mesh(-11.0, 11.0, -92.0, -66.0, 82.0, 108.0));
        let axis = [
            Vec3::new(0.0, -90.0, 86.0),
            Vec3::new(0.0, -96.0, 70.0),
            Vec3::new(0.0, -98.0, 50.0),
            Vec3::new(0.0, -97.0, 30.0),
            Vec3::new(0.0, -94.0, 16.0),
        ];
        parts.push(tube(&axis, &[4.5, 4.0, 3.5, 3.0, 2.5]));
        let mut model = merge(parts);
        let mut recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        recipe.trunk.proboscis = 6;
        let report = fit_baseline_to_mesh(&mut model, 110.0, &recipe).expect("the fit runs");
        let shape = report.shape.as_ref().expect("the barrel has a shape");
        let id = crate::baseline::module_id("proboscis", "");
        assert!(
            shape.matched.iter().any(|(m, _)| *m == id),
            "the proboscis matched the tube past the head:\n{}",
            shape.report()
        );
        let frames = crate::bake::rest_world_frames(&model);
        let at = |name: &str| {
            let i = model
                .bones
                .iter()
                .position(|b| b.name == name)
                .unwrap_or_else(|| panic!("no bone {name}"));
            frames[i].w_axis.truncate()
        };
        let off_axis = |p: Vec3| {
            axis.windows(2)
                .map(|s| p.distance(crate::bake::closest_point_segment(p, s[0], s[1])))
                .fold(f32::INFINITY, f32::min)
        };
        let head = at("head");
        assert!(
            (-96.0..=-60.0).contains(&head.y) && (78.0..=112.0).contains(&head.z),
            "the head stays on its own mass: {head}"
        );
        let joints: Vec<Vec3> = (1..=6).map(|k| at(&format!("proboscis_{k:02}"))).collect();
        for (k, j) in joints.iter().enumerate() {
            assert!(
                off_axis(*j) < 6.0,
                "proboscis_{:02} lies in the tube: {j} is {:.1} cm off its axis",
                k + 1,
                off_axis(*j)
            );
        }
        assert!(
            (74.0..=98.0).contains(&joints[0].z),
            "the root is where the tube leaves the head: {}",
            joints[0]
        );
        assert!(
            joints[5].z < 38.0,
            "the last joint is down near the tip: {}",
            joints[5]
        );
        assert!(
            joints.windows(2).all(|w| w[1].z < w[0].z),
            "the chain runs down the tube: {joints:?}"
        );
        // Skinned: the tube's flesh below the head rides the chain, not the head.
        let body = report.body.expect("the fit hands back its read");
        crate::bake::bind(&mut model, Some(&body));
        let owner = |v: &crate::fbx::RawVertex| {
            let k = (0..4)
                .max_by(|&a, &b| v.weights[a].total_cmp(&v.weights[b]))
                .unwrap_or(0);
            model.bones[v.joints[k] as usize].name.clone()
        };
        let trunk: Vec<&crate::fbx::RawVertex> = model
            .vertices
            .iter()
            .filter(|v| v.p[2] < 70.0 && v.p[1] < -85.0)
            .collect();
        assert!(trunk.len() > 100, "the fixture has a trunk");
        let on_chain = trunk
            .iter()
            .filter(|v| owner(v).starts_with("proboscis_"))
            .count();
        assert!(
            on_chain * 10 >= trunk.len() * 9,
            "the trunk's flesh rides its own chain: {on_chain} of {}",
            trunk.len()
        );
        let seating = crate::bake::Seating::read(&model, &body).expect("the legs are seated");
        let bare: usize = seating
            .appendages(&model)
            .iter()
            .filter(|a| model.bones[a.bone].name.starts_with("proboscis"))
            .map(|a| a.verts.len())
            .sum();
        assert!(
            bare < trunk.len() / 4,
            "with bones in it the trunk is no bare end: {bare} rigid vertices"
        );
    }

    /// THE HAND-OFF (Aaron, ruling 7881216F: *"Build the hand-off, flat defaults soft"*). The
    /// antlered barrel with a pair of EARS — thin plates standing off the sides of its head.
    /// Fitted and bound, every boneless appendage the bind found arrives as a tagged region on
    /// the head: the round antlers RIGID (no chains), the flat ears SOFT on a comb of chains;
    /// no vertex is in two of them, and what is already a region is never proposed again. The
    /// rig bake then lays the soft ones' chains and binds every one of their vertices, and
    /// leaves the rigid ones bare — which is what the runtime swings and what it does not.
    #[test]
    fn every_boneless_appendage_arrives_as_a_region_and_the_flat_ones_soft() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        use flicker_skeletal::format::RegionTag;
        let mut parts = headless_barrel();
        parts.push(box_mesh(-10.0, 10.0, -62.0, -38.0, 66.0, 96.0));
        for sx in [1.0_f32, -1.0] {
            parts.push(tube(
                &[
                    Vec3::new(sx * 4.0, -52.0, 94.0),
                    Vec3::new(sx * 26.0, -52.0, 128.0),
                ],
                &[2.5, 2.0],
            ));
            // An ear: a plate 2.4 cm thick and 12 wide standing 21 cm out of the head's side —
            // six thin rods laid side by side, so it has a skin's worth of vertices.
            for k in 0..6 {
                let y = -51.0 + 2.0 * k as f32;
                parts.push(tube(
                    &[Vec3::new(sx * 9.0, y, 82.0), Vec3::new(sx * 30.0, y, 82.0)],
                    &[1.2, 1.2],
                ));
            }
        }
        let mut model = merge(parts);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 110.0, &recipe).expect("the fit runs");
        let body = report.body.expect("the fit hands back its read");
        crate::bake::bind(&mut model, Some(&body));
        let seating = crate::bake::Seating::read(&model, &body).expect("the legs are seated");
        let found = seating.appendages(&model);
        let proposed = crate::regions::appendage_regions(&model, &found);
        // Which is which, by where its vertices are: the antlers reach the top of the body,
        // the ears stand off its sides, and the fixture's tail — a tube no bone of this recipe
        // lies in — hangs behind it.
        let reach = |r: &flicker_skeletal::format::ClothRegion, axis: usize, sign: f32| {
            r.verts
                .iter()
                .map(|&v| sign * model.vertices[v as usize].p[axis])
                .fold(f32::NEG_INFINITY, f32::max)
        };
        let top = model
            .vertices
            .iter()
            .map(|v| v.p[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let rear = model
            .vertices
            .iter()
            .map(|v| v.p[1])
            .fold(f32::NEG_INFINITY, f32::max);
        let (mut antlers, mut ears, mut tails) = (0, 0, 0);
        for r in &proposed {
            assert_eq!(r.tag, RegionTag::Appendage, "{} is an appendage", r.name);
            if reach(r, 2, 1.0) > top - 2.0 {
                antlers += 1;
                assert_eq!(
                    (r.chain_count, r.anchor_bone.as_str()),
                    (0, "head"),
                    "an antler is round: rigid with the head"
                );
            } else if reach(r, 1, 1.0) > rear - 2.0 {
                tails += 1;
                assert_eq!(
                    (r.chain_count, r.anchor_bone.as_str()),
                    (0, "pelvis"),
                    "a boneless tail is round: rigid with the pelvis"
                );
            } else {
                ears += 1;
                assert!(
                    r.chain_count >= 1
                        && ["neck_01", "neck_02", "head"].contains(&r.anchor_bone.as_str())
                        && (r.params.stiffness - crate::regions::FLESH.stiffness).abs() < 1e-6,
                    "an ear is flat: soft, on the head's own chain — {} chain(s) on {}",
                    r.chain_count,
                    r.anchor_bone
                );
            }
        }
        assert_eq!(
            (antlers, ears, tails),
            (2, 2, 1),
            "two antlers, two ears and the tail: {:?}",
            proposed
                .iter()
                .map(|r| (&r.name, &r.anchor_bone, r.chain_count, r.verts.len()))
                .collect::<Vec<_>>()
        );
        let mut seen = vec![false; model.vertices.len()];
        for v in proposed.iter().flat_map(|r| &r.verts) {
            assert!(
                !std::mem::replace(&mut seen[*v as usize], true),
                "one region a vertex"
            );
        }
        // An antler BEGINS where it leaves the skull's mass: nothing of the skull's crown (the
        // box top, z 96) is in an antler's region — measured on a real horned source where one
        // horn's run swallowed the poll between the horns and read FLAT for it (2026-10-08).
        for r in proposed.iter().filter(|r| reach(r, 2, 1.0) > top - 2.0) {
            let skull = r
                .verts
                .iter()
                .map(|&v| model.vertices[v as usize].p)
                .filter(|p| p[2] < 97.0 && (p[0].abs() < 2.0 || p[1] > -47.0 || p[1] < -57.0))
                .count();
            assert_eq!(
                skull, 0,
                "{} carries {skull} vertices of the skull's crown",
                r.name
            );
        }
        // Across the plane, the two antlers are TWINS and so are the two ears: read as one.
        let twins: Vec<(usize, Option<usize>)> =
            found.iter().enumerate().map(|(i, a)| (i, a.twin)).collect();
        assert!(
            twins
                .iter()
                .filter(|(i, t)| t.is_some_and(|j| found[j].twin == Some(*i)))
                .count()
                >= 4,
            "two mirrored pairs on the head: {twins:?}"
        );
        model.regions.extend(proposed);
        assert!(
            crate::regions::appendage_regions(&model, &found).is_empty(),
            "what is a region already is never proposed again"
        );
        // The bake lays the combs.
        let rig = crate::bake::bake_rig(&model, "antlered");
        for r in &rig.mesh.cloth.regions {
            if r.chain_count == 0 {
                assert!(
                    r.chains.is_empty() && r.binds.is_empty(),
                    "{} is rigid",
                    r.name
                );
            } else {
                assert!(
                    !r.chains.is_empty() && r.binds.len() == r.verts.len(),
                    "{}: {} chain(s), {} of {} vertices bound",
                    r.name,
                    r.chains.len(),
                    r.binds.len(),
                    r.verts.len()
                );
            }
        }
    }

    /// A BOX QUADRUPED WHOSE NECK TURNS `bend` DEGREES TO ITS LEFT (+x) partway along — the pose
    /// Aaron noted on the ElkBull (A79A6131: *"their heads are turned to the side a bit"*). The
    /// barrel, legs and tail of the shape gates; the neck runs straight out of the barrel's front,
    /// then turns and runs on; the head continues the turned neck. `thick_head`: the skull is as
    /// thick as half the barrel, a CORE the neck links to; else the neck runs on into a head no
    /// thicker than itself, one TUBE. Returns the body, the neck's own axis (for "along the bend"),
    /// the head's centroid, and how many of the body's vertices (its last ones) are the head's.
    fn turned_neck_quadruped(bend: f32, thick_head: bool) -> (RawModel, Vec<Vec3>, Vec3, usize) {
        use crate::flesh::fixtures::{merge, tube};
        let mut parts = headless_barrel();
        let (b, k) = (bend.to_radians(), Vec3::new(0.0, -54.0, 86.0));
        let dir = Vec3::new(b.sin(), -b.cos(), 0.0);
        let on = if thick_head { 18.0 } else { 36.0 };
        let axis = vec![Vec3::new(0.0, -34.0, 82.0), k, k + dir * on];
        parts.push(tube(&axis, &[5.0, 5.0, 5.0]));
        if !thick_head {
            // The head is the neck's own last stretch, no thicker than it.
            return (merge(parts), axis, k + dir * 27.0, 0);
        }
        let (a, z) = (k + dir * 20.0, k + dir * 38.0);
        let head = tube(&[a, z], &[9.0, 9.0]);
        let n = head.vertices.len();
        parts.push(head);
        (merge(parts), axis, 0.5 * (a + z), n)
    }

    /// Where face_forward reads the face: the yaw about +Z from −Y to the eyes' midpoint over the
    /// head joint, in degrees (positive = the body's left).
    fn face_yaw(model: &RawModel) -> f32 {
        let w = model_world_frames(model);
        let at = |n: &str| pos_of(w[bone_at(model, n).unwrap_or_else(|| panic!("no `{n}`"))]);
        let f = 0.5 * (at("eye_l") + at("eye_r")) - at("head");
        f.x.atan2(-f.y).to_degrees()
    }

    /// The nearest distance from `p` to a polyline.
    fn off_the_line(p: Vec3, line: &[Vec3]) -> f32 {
        line.windows(2)
            .map(|s| p.distance(closest_on_segment(p, s[0], s[1])))
            .fold(f32::MAX, f32::min)
    }

    fn closest_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
        let ab = b - a;
        a + ab * ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0)
    }

    /// THE GATE ON FOLLOWING THE NECK (A79A6131). A skull linked to the barrel through a neck that
    /// turns 45° to the side: the head lands at the TURNED head's own centroid, both neck joints
    /// lie along the bend inside the neck (never on the chord from the withers to the skull), and
    /// the face turns with it — the 45° `bake::face_forward` then un-turns at bake. The same body
    /// with its neck STRAIGHT keeps its head exactly where the matcher always put a linked skull,
    /// its neck on the straight line, and a face that needs no un-turning.
    #[test]
    fn a_neck_turned_to_the_side_is_followed_to_its_head() {
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let head = crate::baseline::module_id("head", "");
        for bend in [45.0_f32, 0.0] {
            let (mut model, axis, skull, _) = turned_neck_quadruped(bend, true);
            let m = fit_baseline_to_mesh(&mut model, 110.0, &recipe)
                .expect("the fit runs")
                .shape
                .expect("the body has a shape");
            assert!(
                matches!(m.of(&head), Some(Matched::Core(_))),
                "{bend}°: the linked skull is the head:\n{}",
                m.report()
            );
            let flesh = Flesh::build_body(&model);
            let w = model_world_frames(&model);
            let at = |n: &str| pos_of(w[bone_at(&model, n).expect("the canon composes it")]);
            // THE HEAD JOINT sits in the turned skull — on the neck's way into it, short of the
            // skull's centroid by the composed head's own share (its base, 2026-10-09), never
            // past it and never out of the flesh.
            let last = at("neck_02");
            assert!(
                at("head").distance(skull) < 9.0
                    && flesh.contains(at("head"))
                    && (at("head") - last).dot(skull - last) > 0.0
                    && (at("head") - last).length() <= skull.distance(last) + 1.0,
                "{bend}°: the head sits in the turned skull on the neck's way to its centroid {skull}, got {}",
                at("head")
            );
            for n in ["neck_01", "neck_02"] {
                assert!(
                    flesh.contains(at(n)) && off_the_line(at(n), &axis) < 5.0 + flesh.cell(),
                    "{bend}°: {n} lies along the neck, inside it: {} is {:.1} cm off its axis",
                    at(n),
                    off_the_line(at(n), &axis)
                );
            }
            let yaw = face_yaw(&model);
            assert!(
                (yaw - bend).abs() < 8.0,
                "{bend}°: the face turns with the neck, read {yaw:.1}°"
            );
        }
    }

    /// THE SAME, WHEN THE HEAD IS NO THICKER THAN THE NECK: one TUBE leaves the barrel's front end
    /// and turns 40° aside, and the head is the MASS at its end — its last cells, not the middle of
    /// the tube, which is where the head used to be laid (on a horse, the poll; on this fixture, the
    /// bend).
    #[test]
    fn a_neck_tube_turned_aside_carries_its_head_at_its_end() {
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let head = crate::baseline::module_id("head", "");
        let (mut model, axis, skull, _) = turned_neck_quadruped(40.0, false);
        let fit = fit_baseline_to_mesh(&mut model, 110.0, &recipe).expect("the fit runs");
        let m = fit.shape.expect("the body has a shape");
        let detail = fit
            .body
            .and_then(|b| b.graph)
            .map(|g| g.detail())
            .unwrap_or_default();
        assert!(
            matches!(m.of(&head), Some(Matched::Limb(_))),
            "the neck tube carries the head:\n{}{detail}",
            m.report()
        );
        let flesh = Flesh::build_body(&model);
        let w = model_world_frames(&model);
        let at = |n: &str| pos_of(w[bone_at(&model, n).expect("the canon composes it")]);
        assert!(
            flesh.contains(at("head")) && at("head").distance(skull) < 9.0,
            "the head is the mass at the tube's end ({skull}), got {}",
            at("head")
        );
        for n in ["neck_01", "neck_02"] {
            assert!(
                flesh.contains(at(n)) && off_the_line(at(n), &axis) < 5.5 + flesh.cell(),
                "{n} lies along the neck, inside it: {}",
                at(n)
            );
        }
        let yaw = face_yaw(&model);
        assert!(
            (yaw - 40.0).abs() < 10.0,
            "the face turns with the neck, read {yaw:.1}°"
        );
        assert!(
            !m.spare.iter().any(|s| s.contains("appendage")),
            "a neck that ends on its own head has nothing past it:\n{}",
            m.report()
        );
    }

    /// Where a fit put `name`, world.
    fn placed_at(model: &RawModel, name: &str) -> Vec3 {
        pos_of(model_world_frames(model)[bone_at(model, name).expect("the canon composes it")])
    }

    /// THE GATE ON A NECK THAT RUNS ON PAST ITS HEAD (A31C0FAE — the Elephant's head lay 107 cm
    /// down its proboscis). A neck carries a head BULB thicker than itself, and a long tapering
    /// tube runs on out of the bulb, forward and down towards the floor: the head is the bulb —
    /// the mass the neck's path reaches — and the tube past it is an APPENDAGE, reported and never
    /// followed. Read by the path's own end (the rule before), the head hung down the tube.
    #[test]
    fn a_tube_running_on_past_the_head_is_an_appendage_and_never_the_head() {
        use crate::flesh::fixtures::{merge, tube};
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let mut parts = headless_barrel();
        parts.push(tube(
            &[Vec3::new(0.0, -34.0, 82.0), Vec3::new(0.0, -60.0, 96.0)],
            &[3.5, 3.5],
        ));
        // The bulb is thicker than its neck and thinner than half the barrel, so it stays on the
        // tube (a skull as thick as half the body is a chained CORE, which is its own gate).
        let (a, z) = (Vec3::new(0.0, -58.0, 97.0), Vec3::new(0.0, -76.0, 99.0));
        parts.push(tube(&[a, z], &[6.0, 6.0]));
        parts.push(tube(
            &[
                Vec3::new(0.0, -80.0, 96.0),
                Vec3::new(0.0, -98.0, 70.0),
                Vec3::new(0.0, -102.0, 30.0),
            ],
            &[2.5, 2.0, 1.5],
        ));
        let mut model = merge(parts);
        let fit = fit_baseline_to_mesh(&mut model, 110.0, &recipe).expect("the fit runs");
        let m = fit.shape.expect("the body has a shape");
        let head = crate::baseline::module_id("head", "");
        assert!(
            matches!(m.of(&head), Some(Matched::Limb(_))),
            "the neck tube carries the head:\n{}",
            m.report()
        );
        let (h, bulb) = (placed_at(&model, "head"), 0.5 * (a + z));
        assert!(
            Flesh::build_body(&model).contains(h) && h.distance(bulb) < 7.0,
            "the head is the bulb ({bulb}), not the tube past it: got {h}\n{}",
            m.report()
        );
        assert!(
            m.spare.iter().any(|s| s.contains("appendage")),
            "the tube past the head is reported:\n{}",
            m.report()
        );
    }

    /// THE GATE ON EARS OFF A SKULL THE TRUNK HOLDS (the Rabbit, A31C0FAE: its head lay 63 cm up
    /// its ear). The skull is merged into the barrel's front, as the front-cap gate builds it, and
    /// one stalk rises off its crown and forks into two tall thin ears leaning forward: unpaired,
    /// clear of the floor, leaving the front end and running on beyond it — every reading a neck
    /// had to pass. But it is thin from where it leaves the trunk, all of it tapering on: it carries
    /// no head out of the body. The head is the skull, on the front cap; the ears are a spare tube.
    #[test]
    fn tall_thin_ears_off_a_skull_in_the_trunk_are_never_its_neck() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let mut parts = headless_barrel();
        parts.push(box_mesh(-10.0, 10.0, -62.0, -38.0, 66.0, 96.0));
        parts.push(tube(
            &[Vec3::new(0.0, -52.0, 92.0), Vec3::new(0.0, -54.0, 106.0)],
            &[2.5, 2.5],
        ));
        for sx in [1.0_f32, -1.0] {
            parts.push(tube(
                &[
                    Vec3::new(0.0, -54.0, 105.0),
                    Vec3::new(sx * 7.0, -74.0, 142.0),
                ],
                &[1.5, 1.2],
            ));
        }
        let mut model = merge(parts);
        let m = fit_baseline_to_mesh(&mut model, 110.0, &recipe)
            .expect("the fit runs")
            .shape
            .expect("the body has a shape");
        let head = crate::baseline::module_id("head", "");
        assert!(
            m.of(&head).is_none() && m.capped.iter().any(|(id, _)| *id == head),
            "the ears are no neck — the head is the skull on the front cap:\n{}",
            m.report()
        );
        let h = placed_at(&model, "head");
        assert!(
            Flesh::build_body(&model).contains(h) && h.z < 96.0,
            "the head is in the skull, not up the ears: {h}"
        );
        assert!(
            m.spare.iter().any(|s| s.contains("raised tube")),
            "the ears are reported as a tube nothing claimed:\n{}",
            m.report()
        );
    }

    /// THE GATE ON A BEAK (a bird's head on an upright body). A plumb body on two legs, a thin neck
    /// rising off its top into a skull, and a beak running forward out of the skull: the head is
    /// the skull, and the beak — thinner than half the skull and longer than its reach — an
    /// appendage past it. Read by the path's own end, the head sat in the beak.
    #[test]
    fn a_beak_past_an_upright_bodys_skull_is_never_the_head() {
        use crate::flesh::fixtures::{merge, tube};
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Bird);
        let base = Vec3::new(0.0, 0.0, 88.0);
        let mut parts = vec![tube(&[base, base + Vec3::Z * 70.0], &[16.0, 14.0])];
        for x in [9.0_f32, -9.0] {
            parts.push(tube(
                &[Vec3::new(x, 0.0, 96.0), Vec3::new(x, 0.0, 0.0)],
                &[5.0, 4.5],
            ));
        }
        parts.push(tube(
            &[Vec3::new(0.0, 0.0, 150.0), Vec3::new(0.0, -2.0, 178.0)],
            &[4.0, 3.5],
        ));
        let (a, z) = (Vec3::new(0.0, -1.0, 176.0), Vec3::new(0.0, -8.0, 184.0));
        parts.push(tube(&[a, z], &[7.0, 7.0]));
        parts.push(tube(
            &[Vec3::new(0.0, -12.0, 183.0), Vec3::new(0.0, -44.0, 178.0)],
            &[2.2, 1.0],
        ));
        let mut model = merge(parts);
        let m = fit_baseline_to_mesh(&mut model, 190.0, &recipe)
            .expect("the fit runs")
            .shape
            .expect("the body has a shape");
        let head = crate::baseline::module_id("head", "");
        assert!(
            matches!(m.of(&head), Some(Matched::Limb(_))),
            "the neck rising off the body carries the head:\n{}",
            m.report()
        );
        let (h, skull) = (placed_at(&model, "head"), 0.5 * (a + z));
        // The joint sits in the skull — at its base, where the neck enters it, or on toward its
        // centroid — never down the neck and never out along the beak.
        assert!(
            Flesh::build_body(&model).contains(h)
                && h.distance(skull) < 9.0
                && h.z >= a.z - 1.0
                && h.y > -8.0,
            "the head is the skull ({skull}), not the beak: got {h}\n{}",
            m.report()
        );
    }

    /// THE GATE ON FACE FORWARD BY DEFAULT (A79A6131). The turned-neck quadruped through the
    /// HEADLESS path exactly as `import_folder` runs it ([`crate::pipeline::rig_prepped_mesh`]),
    /// with the Prep as it opens — no flag: the fit reads the 45° turn off the neck it followed,
    /// and the bake un-turns it, so the shipped face looks down −Y and the head's own flesh has
    /// swung round in front of the neck it hangs from. Opted out, the head stays as posed.
    #[test]
    fn a_turned_head_faces_forward_through_the_headless_path_by_default() {
        let prep = crate::pipeline::RawMeshPrep {
            recipe: crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped),
            stature_cm: 110.0,
            ..Default::default()
        };
        assert!(prep.face_forward, "Prep opens facing a turned head forward");
        let (source, _, _, n) = turned_neck_quadruped(45.0, true);
        let baked = |prep: &crate::pipeline::RawMeshPrep| {
            let mut m = source.clone();
            crate::pipeline::rig_prepped_mesh(&mut m, prep).expect("the headless bake runs");
            let w = model_world_frames(&m);
            let at = |b: &str| pos_of(w[bone_at(&m, b).expect("the canon composes it")]);
            let skull = m.vertices[m.vertices.len() - n..]
                .iter()
                .map(|v| Vec3::from_array(v.p))
                .sum::<Vec3>()
                / n as f32;
            (at("neck_01"), skull, face_yaw(&m))
        };
        let (pivot, skull, yaw) = baked(&prep);
        assert!(
            yaw.abs() < 0.5,
            "the shipped face looks down −Y, read {yaw:.1}°"
        );
        let (_, posed, posed_yaw) = baked(&crate::pipeline::RawMeshPrep {
            face_forward: false,
            ..prep
        });
        assert!(
            posed_yaw > 35.0,
            "opted out, the face stays turned: {posed_yaw:.1}°"
        );
        let aside = |p: Vec3| (p.x - pivot.x).abs();
        assert!(
            aside(skull) < 0.5 * aside(posed) && skull.y < posed.y,
            "the head's own flesh swung round in front of its neck: {skull} (posed {posed})"
        );
    }

    /// A FACE LOOKS THE WAY ITS TRUNK HEADS (the Toad). The turned-neck body the wrong way round —
    /// its rump toward the rig's −Y forward — on a longer neck whose skull turns 120° aside, back
    /// past its shoulder: the trunk heads +Y and the face looks back along it, 60° off the rig's
    /// forward. That read is at odds with itself (the Toad's trunk is read tail-first, its own hind
    /// thigh taken for the chained head), so the face states nothing and the default bake moves no
    /// vertex — where it swung the Toad's thigh 58°.
    #[test]
    fn a_face_never_turns_back_over_its_own_trunk() {
        use crate::flesh::fixtures::{merge, tube};
        let prep = crate::pipeline::RawMeshPrep {
            recipe: crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped),
            stature_cm: 110.0,
            ..Default::default()
        };
        let mut parts = headless_barrel();
        let (b, k) = (120.0_f32.to_radians(), Vec3::new(0.0, -74.0, 86.0));
        let dir = Vec3::new(b.sin(), -b.cos(), 0.0);
        parts.push(tube(
            &[Vec3::new(0.0, -34.0, 82.0), k, k + dir * 18.0],
            &[5.0; 3],
        ));
        parts.push(tube(&[k + dir * 20.0, k + dir * 38.0], &[9.0, 9.0]));
        let mut source = merge(parts);
        for v in &mut source.vertices {
            v.p = [-v.p[0], -v.p[1], v.p[2]];
            v.n = [-v.n[0], -v.n[1], v.n[2]];
        }
        let fit = fit_baseline_to_mesh(&mut source.clone(), 110.0, &prep.recipe).expect("fits");
        let m = fit.shape.expect("the body has a shape");
        let graph = fit.body.and_then(|b| b.graph).expect("and a graph");
        let (Some(Matched::Core(t)), Some(Matched::Core(_))) = (
            m.of(&crate::baseline::module_id("trunk", "")),
            m.of(&crate::baseline::module_id("head", "")),
        ) else {
            panic!("the skull is the chained head:\n{}", m.report());
        };
        let heading = graph.cores[t].front() - graph.cores[t].rear();
        assert!(heading.y > 0.0, "the trunk is read heading +Y: {heading}");
        let baked = |prep: &crate::pipeline::RawMeshPrep| {
            let mut m = source.clone();
            crate::pipeline::rig_prepped_mesh(&mut m, prep).expect("the headless bake runs");
            m
        };
        let (faced, posed) = (
            baked(&prep),
            baked(&crate::pipeline::RawMeshPrep {
                face_forward: false,
                ..prep.clone()
            }),
        );
        let yaw = face_yaw(&posed);
        assert!(
            yaw.abs() < 0.5,
            "a face looking back over its trunk states nothing: {yaw:.1}°"
        );
        let moved = faced
            .vertices
            .iter()
            .zip(&posed.vertices)
            .map(|(a, b)| Vec3::from_array(a.p).distance(Vec3::from_array(b.p)))
            .fold(0.0_f32, f32::max);
        assert!(
            moved < 1e-3,
            "the default bake leaves the body as it stands: moved {moved:.1} cm"
        );
    }

    /// THE GATE ON THE FRONT END (the ElkBull, A79A6131). A skull merged into the barrel's front,
    /// and a fringe hanging from the chest halfway along it and sweeping forward past the skull —
    /// unpaired, clear of the floor, on the front half, reaching beyond the front end: everything
    /// the old "unpaired limb beyond the front end" asked of a head, which is how the ElkBull's
    /// head was laid in its chest mane. It does not LEAVE the front end, so it is no neck: the head
    /// is the skull on the front cap.
    #[test]
    fn a_fringe_under_the_chest_is_never_the_neck() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let head = crate::baseline::module_id("head", "");
        let mut parts = headless_barrel();
        parts.push(box_mesh(-10.0, 10.0, -62.0, -38.0, 66.0, 96.0));
        parts.push(tube(
            &[Vec3::new(0.0, -8.0, 62.0), Vec3::new(0.0, -80.0, 30.0)],
            &[3.5, 2.5],
        ));
        let mut model = merge(parts);
        let m = fit_baseline_to_mesh(&mut model, 110.0, &recipe)
            .expect("the fit runs")
            .shape
            .expect("the body has a shape");
        assert!(
            m.of(&head).is_none() && m.capped.iter().any(|(id, _)| *id == head),
            "no neck leaves the front end — the head is the front cap:\n{}",
            m.report()
        );
        let w = model_world_frames(&model);
        let at = |n: &str| pos_of(w[bone_at(&model, n).expect("the canon composes it")]);
        assert!(
            Flesh::build_body(&model).contains(at("head")) && at("head").z > 66.0,
            "the head is in the skull, not down the fringe: {}",
            at("head")
        );
    }

    /// THE GATE ON A BIPED LAID ALONG ITS CORE (the Owl, 0E38BE60). An upright body LEANING 30°
    /// forward on two legs: the plumb spine the canon composes above the pelvis leaves `spine_03`
    /// behind its own back; laid along the core it stays inside, leaning with it, at the chain's
    /// own length. The same body standing PLUMB keeps the canon's plumb spine.
    #[test]
    fn a_leaning_bipeds_spine_leans_with_its_body() {
        use crate::flesh::fixtures::{merge, tube};
        for lean in [30.0_f32, 0.0] {
            let l = lean.to_radians();
            let up = Vec3::new(0.0, -l.sin(), l.cos());
            let base = Vec3::new(0.0, 0.0, 88.0);
            let mut parts = vec![tube(&[base, base + up * 70.0], &[16.0, 14.0])];
            for x in [9.0_f32, -9.0] {
                parts.push(tube(
                    &[Vec3::new(x, 0.0, 96.0), Vec3::new(x, 0.0, 0.0)],
                    &[5.0, 4.5],
                ));
            }
            let mut model = merge(parts);
            fit_baseline_to_mesh(&mut model, 170.0, &SkeletonRecipe::humanoid())
                .expect("the fit runs");
            let flesh = Flesh::build_body(&model);
            let w = model_world_frames(&model);
            let at = |n: &str| pos_of(w[bone_at(&model, n).expect("the canon composes it")]);
            for n in ["pelvis", "spine_01", "spine_02", "spine_03"] {
                assert!(
                    flesh.contains(at(n)),
                    "{lean}°: {n} is inside the body, got {}",
                    at(n)
                );
            }
            let (p, s3) = (at("pelvis"), at("spine_03"));
            let composed = 0.170 * 170.0;
            assert!(
                (p.distance(s3) - composed).abs() < 0.5,
                "{lean}°: the spine keeps its own length {composed:.1}, got {:.1}",
                p.distance(s3)
            );
            let leans = (-(s3 - p).y).atan2((s3 - p).z).to_degrees();
            assert!(
                (leans - lean).abs() < 8.0 && (s3.x - p.x).abs() < 1e-3,
                "{lean}°: the spine leans with the body on its plane, read {leans:.1}° {s3}"
            );
        }
    }

    /// A BARREL WHOSE LEGS' UPPER BONES ARE BURIED IN IT (BAD0D72C) — its back at 100 cm, its
    /// belly 60 under the chest, 54 at the middle and tucked up to 67 under the flank and rump,
    /// and four tubes that leave it where a hoofed animal's do: each HIND tube out of the rump
    /// high on its buried femur (≈0.65h, where the real hind tubes leave — 0.54–0.66h measured
    /// on the hoofed family), running forward-down to a narrowing at the stifle, bulging (the
    /// gaskin), narrowing at the hock set back behind it, bulging again (the cannon) and
    /// narrowing at the fetlock over a hoof; each FORELEG at the chest's floor, narrowing at the
    /// elbow, the carpus and the fetlock. Every narrowing sits where the Unguligrade / Ungulate
    /// modules compose that joint at the stature this back measures (`100 / QUAD_WITHERS`), so
    /// the composed rest is this body's own — the femur and the scapula inside the barrel, the
    /// rest down the tubes. Returns the body and, per side (`+x` first), the designed hind
    /// `[stifle, hock, fetlock, hoof]` and fore `[elbow, carpus, fetlock, hoof]`.
    fn buried_limbs_quadruped() -> (RawModel, [[Vec3; 4]; 2], [[Vec3; 4]; 2]) {
        buried_limbs_quadruped_raising(0.0)
    }

    /// [`buried_limbs_quadruped`] with its −x FORELEG RAISED: everything under that elbow drawn
    /// up toward it until the hoof hangs `raise` cm over the floor — a source caught mid-stride.
    fn buried_limbs_quadruped_raising(raise: f32) -> (RawModel, [[Vec3; 4]; 2], [[Vec3; 4]; 2]) {
        use crate::flesh::fixtures::{merge, tube};
        let h = 100.0 / crate::baseline::QUAD_WITHERS;
        let mut parts = vec![tube(
            &[
                Vec3::new(0.0, 60.0, 83.7),
                Vec3::new(0.0, 12.0, 83.7),
                Vec3::new(0.0, 0.0, 77.0),
                Vec3::new(0.0, -60.0, 80.0),
            ],
            &[16.3, 16.3, 23.0, 20.0],
        )];
        let (mut hind, mut fore) = ([[Vec3::ZERO; 4]; 2], [[Vec3::ZERO; 4]; 2]);
        for (k, sign) in [1.0_f32, -1.0].into_iter().enumerate() {
            let x = sign * 0.09 * h;
            // The hind leg, from its hip socket 0.56h up at the rump (y = 30).
            let at = |y: f32, z: f32| Vec3::new(x, 30.0 + y * h, z * h);
            let (stifle, hock) = (at(-0.14, 0.56), at(0.06, 0.38));
            let (fetlock, hoof) = (at(0.035, 0.10), at(0.02, 0.0));
            parts.push(tube(
                &[
                    Vec3::new(x, 30.0, 74.0),
                    stifle,
                    0.5 * (stifle + hock) + Vec3::new(0.0, -1.5, 0.0),
                    hock,
                    0.5 * (hock + fetlock),
                    fetlock,
                    hoof,
                ],
                &[6.5, 3.4, 5.4, 2.8, 3.8, 2.4, 3.6],
            ));
            hind[k] = [stifle, hock, fetlock, hoof];
            // The foreleg, from the withers (0.92h, y = −33) the forelegs hang from.
            let at = |y: f32, z: f32| Vec3::new(x, -33.0 + y * h, z * h);
            let elbow = Vec3::new(x, -34.0, 50.0);
            let drawn = |p: Vec3| {
                let keep = if sign < 0.0 {
                    1.0 - raise / elbow.z
                } else {
                    1.0
                };
                Vec3::new(p.x, p.y, elbow.z - (elbow.z - p.z) * keep)
            };
            let (carpus, fetlock, hoof) = (
                drawn(at(-0.06, 0.28)),
                drawn(at(-0.07, 0.08)),
                drawn(at(-0.08, 0.0)),
            );
            parts.push(tube(
                &[
                    Vec3::new(x, -34.0, 79.0),
                    elbow,
                    0.5 * (elbow + carpus),
                    carpus,
                    0.5 * (carpus + fetlock),
                    fetlock,
                    hoof,
                ],
                &[6.0, 3.6, 4.6, 2.9, 3.6, 2.4, 3.6],
            ));
            fore[k] = [elbow, carpus, fetlock, hoof];
        }
        (merge(parts), hind, fore)
    }

    /// Every joint of `model` by name, world.
    fn joints(model: &RawModel) -> HashMap<String, Vec3> {
        let w = model_world_frames(model);
        model
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.clone(), pos_of(w[i])))
            .collect()
    }

    /// THE GATE ON A HIND LEG LAID FROM ITS HOOF (Aaron on the Elk, BAD0D72C: *"the hip bone is
    /// actually supposed to be basically at the ass of the animal, this kind of skips the hip bone
    /// and starts to rig from the knee"*). The hind tubes leave the barrel at the stifle with the
    /// femur buried in the rump: laid from where the tube leaves the body, the thigh sat on the
    /// stifle and every joint below it one joint down. Laid from the tip, the hoof is at the tip,
    /// the fetlock, hock and stifle at the tube's own narrowings, and the thigh — the joint the
    /// tube does not hold — inside the rump at its socket, above the stifle: rear, forward, back.
    #[test]
    fn a_hind_leg_whose_femur_is_buried_is_laid_from_its_hoof_into_the_rump() {
        let (mut model, hind, _) = buried_limbs_quadruped();
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let m = report.shape.expect("the barrel has a shape");
        let Some(Matched::Pair(i)) = m.of(&crate::baseline::module_id("leg", "")) else {
            panic!("the hind legs match a pair:\n{}", m.report());
        };
        let body = report.body.expect("the fit hands back its read");
        let graph = body.graph.as_ref().expect("a graph");
        let core = &graph.cores[graph.pairs[i].core];
        let j = joints(&model);
        let h = 100.0 / crate::baseline::QUAD_WITHERS;
        for (k, side) in ["l", "r"].into_iter().enumerate() {
            let at = |n: &str| j[&format!("{n}_{side}")];
            let [stifle, hock, fetlock, hoof] = hind[k];
            let (thigh, calf, foot, ball) = (at("thigh"), at("calf"), at("foot"), at("ball"));
            assert!(
                body.flesh.contains(thigh) && core.t_of(thigh) < 1.0 / 3.0,
                "{side}: the thigh is inside the rump, in the core's rear third: {thigh} t {:.2}",
                core.t_of(thigh)
            );
            assert!(
                (thigh.z - 0.78 * h).abs() < 3.0 && thigh.z > calf.z + 10.0,
                "{side}: the thigh at its socket (z {:.1}), well above the stifle: {thigh} over {calf}",
                0.78 * h
            );
            // The stifle is where the tube leaves the rump: the calf takes the tube's attachment
            // (the graph's own cut, just above the narrowing) or the stifle itself — its
            // narrowing, or the BEND the tube makes there, which a thinned path rounds on the
            // inside of the corner and reads a tube's thickness (two of the narrowing's 3.4 cm
            // radii) down the tibia from the authored joint.
            let limb = &graph.limbs[[graph.pairs[i].l, graph.pairs[i].r][k]];
            let attached = leaves_core(limb, graph.cores[limb.core].radius)
                .is_some_and(|a| calf.distance(limb.lead[a]) < 2.0);
            assert!(
                attached || calf.distance(stifle) < 6.8,
                "{side}: the calf at the attachment or the stifle — {calf} vs {stifle}"
            );
            for (name, got, want) in [
                ("foot at the hock", foot, hock),
                ("ball at the fetlock", ball, fetlock),
                ("hoof at the tip", at("hoof"), hoof),
            ] {
                assert!(
                    got.distance(want) < 4.5,
                    "{side}: {name} — {got} vs {want} ({:.1} cm)",
                    got.distance(want)
                );
            }
            assert!(
                calf.y < thigh.y - 2.0 && foot.y > calf.y + 5.0,
                "{side}: the chain zigzags rear → forward → back: thigh {thigh}, calf {calf}, foot {foot}"
            );
        }
    }

    /// THE GATE ON A FORELEG LEAVING THE CHEST AT ITS ELBOW (BAD0D72C). The scapula and the upper
    /// arm are buried in the chest, so the tube starts at the elbow: laid from there the shoulder
    /// joint sat on the elbow. Laid from the forehoof, the shoulder is INSIDE the chest above the
    /// elbow, the scapula at the withers, and the elbow where the tube leaves the body.
    #[test]
    fn a_foreleg_leaving_the_chest_at_its_elbow_keeps_its_shoulder_inside_the_chest() {
        let (mut model, _, fore) = buried_limbs_quadruped();
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let m = report.shape.expect("the barrel has a shape");
        assert!(
            matches!(
                m.of(&crate::baseline::module_id("arm", "")),
                Some(Matched::Pair(_))
            ),
            "the forelegs match a pair:\n{}",
            m.report()
        );
        let flesh = report.body.expect("the fit hands back its read").flesh;
        let j = joints(&model);
        for (k, side) in ["l", "r"].into_iter().enumerate() {
            let at = |n: &str| j[&format!("{n}_{side}")];
            let [elbow, carpus, fetlock, hoof] = fore[k];
            let (shoulder, scapula) = (at("upperarm"), at("clavicle"));
            assert!(
                flesh.contains(shoulder) && flesh.contains(scapula),
                "{side}: the shoulder {shoulder} and the scapula {scapula} are inside the chest"
            );
            assert!(
                scapula.z > shoulder.z && shoulder.z > at("lowerarm").z + 8.0,
                "{side}: scapula over shoulder over elbow: {scapula} {shoulder} {}",
                at("lowerarm")
            );
            for (name, got, want) in [
                ("elbow", at("lowerarm"), elbow),
                ("carpus", at("hand"), carpus),
                ("fetlock", at("foredigit"), fetlock),
                ("forehoof", at("forehoof"), hoof),
            ] {
                assert!(
                    got.distance(want) < 4.5,
                    "{side}: the {name} — {got} vs {want} ({:.1} cm)",
                    got.distance(want)
                );
            }
        }
    }

    /// THE GIRDLE GATE (Aaron on the Elk, 2026-10-02: *"there's zero motion in the torso or
    /// shoulders"*). On the buried-limbs quadruped the skin a body is UN-POSED on gives the
    /// haunch beside a buried femur to the spine — the stance normaliser swings that leg a whole
    /// stride — and the skin it MOVES in gives it to the leg: flank flesh level with the upper
    /// femur, out past the bone on its own side, is the hind leg's; the ridge of the back over
    /// the hips and the other side's flank are not; the leg's own tube is the leg's either way.
    #[test]
    fn the_skin_a_body_moves_in_puts_its_haunch_on_the_leg_under_it() {
        let (mut model, _, _) = buried_limbs_quadruped();
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let body = report.body.expect("the fit hands back its read");
        let j = joints(&model);
        let (hip, stifle, hock) = (j["thigh_l"], j["calf_l"], j["foot_l"]);
        let (mut posed, mut moves) = (model.clone(), model.clone());
        crate::bake::bind(&mut posed, Some(&body));
        crate::bake::bind(&mut moves, Some(&body));
        let seating = crate::bake::Seating::read(&moves, &body);
        crate::bake::bind_for_motion(&mut moves, seating.as_ref(), Some(&body.flesh));
        // How much of the flesh `keep` picks hangs on the LEFT hind leg's bones, on average.
        let on_the_leg = |m: &RawModel, keep: &dyn Fn(Vec3) -> bool| -> (usize, f32) {
            let leg = |i: u32| {
                ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"]
                    .contains(&m.bones[i as usize].name.as_str())
            };
            let picked: Vec<f32> = m
                .vertices
                .iter()
                .filter(|v| keep(Vec3::from_array(v.p)))
                .map(|v| {
                    (0..4)
                        .filter(|&k| leg(v.joints[k]))
                        .map(|k| v.weights[k])
                        .sum()
                })
                .collect();
            (
                picked.len(),
                picked.iter().sum::<f32>() / picked.len().max(1) as f32,
            )
        };
        let beside = hip.lerp(stifle, 0.3);
        let level = |p: Vec3| (p.y - beside.y).abs() < 6.0 && (p.z - beside.z).abs() < 6.0;
        let haunch = |p: Vec3| level(p) && p.x > hip.x + 2.0;
        let other = |p: Vec3| level(p) && p.x < -hip.x - 2.0;
        let ridge = |p: Vec3| p.x.abs() < 2.0 && (p.y - hip.y).abs() < 8.0 && p.z > hip.z + 5.0;
        let tube = |p: Vec3| p.z < hock.z && p.x > 0.0 && (p.y - hock.y).abs() < 12.0;
        for (name, keep) in [
            ("haunch", &haunch as &dyn Fn(Vec3) -> bool),
            ("other flank", &other),
            ("ridge", &ridge),
            ("tube", &tube),
        ] {
            assert!(
                on_the_leg(&model, keep).0 > 0,
                "the fixture has {name} flesh"
            );
        }
        let (was, now) = (on_the_leg(&posed, &haunch).1, on_the_leg(&moves, &haunch).1);
        assert!(
            was < 0.05 && now > 0.5,
            "the haunch: the spine's to be un-posed on ({was:.2}), the leg's to move in ({now:.2})"
        );
        let (far, top) = (on_the_leg(&moves, &other).1, on_the_leg(&moves, &ridge).1);
        assert!(
            far < 0.01 && top < 0.1,
            "never the other side's flank ({far:.3}) nor the ridge of the back ({top:.2})"
        );
        let (a, b) = (on_the_leg(&posed, &tube).1, on_the_leg(&moves, &tube).1);
        assert!(
            a > 0.95 && b > 0.95,
            "the leg's own tube is the leg's on both skins: {a:.2} {b:.2}"
        );
    }

    /// THE SKIN A BODY MOVES IN IS SHARED AT A JOINT, NOT A SPHERE (Aaron 2026-10-08: *"a clear
    /// sphere around the joints that is rotating directly in line with the joint rotation …
    /// almost a 1:1 quaternion rotation"*). Over the buried hip the leg's share of the flank
    /// falls off smoothly up the barrel to the ridge: the flesh right over the femur is the
    /// leg's mostly but never wholly, the ridge is the spine's, and no two neighbouring bands of
    /// the climb differ by a step. The same bands on the un-pose skin are the spine's whole.
    #[test]
    fn the_skin_a_body_moves_in_shares_the_flesh_over_a_buried_bone() {
        let (mut model, _, _) = buried_limbs_quadruped();
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let body = report.body.expect("the fit hands back its read");
        let j = joints(&model);
        let (hip, stifle) = (j["thigh_l"], j["calf_l"]);
        let mut moves = model.clone();
        crate::bake::bind(&mut moves, Some(&body));
        let posed = moves.clone();
        let seating = crate::bake::Seating::read(&moves, &body);
        crate::bake::bind_for_motion(&mut moves, seating.as_ref(), Some(&body.flesh));
        let leg = |m: &RawModel, i: u32| {
            ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"]
                .contains(&m.bones[i as usize].name.as_str())
        };
        let share = |m: &RawModel, v: &crate::fbx::RawVertex| -> f32 {
            (0..4)
                .filter(|&k| leg(m, v.joints[k]))
                .map(|k| v.weights[k])
                .sum()
        };
        let beside = hip.lerp(stifle, 0.3);
        let top = model
            .vertices
            .iter()
            .map(|v| v.p[2])
            .fold(f32::NEG_INFINITY, f32::max);
        // The climb: the left flank's own vertex levels from the haunch's level to the ridge,
        // 5 cm apart or more (the fixture is a 12-gon, so its flank has a few levels, not a
        // continuum — the bands read what it has).
        let mut levels: Vec<i32> = moves
            .vertices
            .iter()
            .filter(|v| v.p[0] > 0.0 && (v.p[1] - beside.y).abs() < 6.0 && v.p[2] >= beside.z - 2.5)
            .map(|v| v.p[2].round() as i32)
            .collect();
        levels.sort_unstable();
        levels.dedup();
        let mut at: Vec<f32> = Vec::new();
        for z in levels {
            if at.last().is_none_or(|l| z as f32 - l >= 5.0) {
                at.push(z as f32);
            }
        }
        let bands: Vec<(f32, f32)> = at
            .into_iter()
            .take_while(|&z| z <= top)
            .filter_map(|z| {
                let pick: Vec<&crate::fbx::RawVertex> = moves
                    .vertices
                    .iter()
                    .filter(|v| {
                        v.p[0] > 0.0 && (v.p[1] - beside.y).abs() < 6.0 && (v.p[2] - z).abs() < 2.5
                    })
                    .collect();
                (pick.len() >= 4).then(|| {
                    let on_moves =
                        pick.iter().map(|v| share(&moves, v)).sum::<f32>() / pick.len() as f32;
                    let on_posed = pick
                        .iter()
                        .map(|v| {
                            let i = moves
                                .vertices
                                .iter()
                                .position(|w| std::ptr::eq(w, *v))
                                .unwrap();
                            share(&posed, &posed.vertices[i])
                        })
                        .sum::<f32>()
                        / pick.len() as f32;
                    (on_moves, on_posed)
                })
            })
            .collect();
        assert!(
            bands.len() >= 3,
            "the flank climbs through bands: {bands:?}"
        );
        let (first, last) = (bands[0].0, bands[bands.len() - 1].0);
        assert!(
            first > 0.5 && first < 0.97,
            "over the femur the leg has most of the flesh and not all of it: {first:.2} ({bands:?})"
        );
        assert!(
            last < 0.1,
            "the ridge is the spine's: {last:.2} ({bands:?})"
        );
        for w in bands.windows(2) {
            assert!(
                w[0].0 - w[1].0 > -0.05 && w[0].0 - w[1].0 < 0.4,
                "the leg's share falls off up the flank without a step: {bands:?}"
            );
        }
        assert!(
            bands.iter().all(|b| b.1 < 0.05),
            "on the un-pose skin the flank is the spine's whole: {bands:?}"
        );
    }

    /// A RAISED LEG IS STILL A LEG. A source caught mid-stride holds one hoof off the floor, and
    /// a limb that does not reach the ground was an arm to the bind: its shoulder bone stayed
    /// with the trunk, owned the flank over it on the skin a stride is un-posed on, and had no
    /// girdle on the skin the body moves in — one shoulder walked and its twin did not (2 of
    /// the 17 hoofed sources). A limb STANDS when it or its twin across the plane reaches the
    /// ground: on both skins the raised side's shoulder is its standing twin's.
    #[test]
    fn a_raised_legs_shoulder_is_bound_like_its_standing_twins() {
        let (mut model, _, _) = buried_limbs_quadruped_raising(20.0);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let body = report.body.expect("the fit hands back its read");
        let graph = body.graph.as_ref().expect("the body has a graph");
        assert!(
            graph
                .pairs
                .iter()
                .any(|p| graph.limbs[p.l].grounded != graph.limbs[p.r].grounded),
            "the fixture stands on one foreleg and holds its twin up:\n{}",
            graph.detail()
        );
        let j = joints(&model);
        let (mut posed, mut moves) = (model.clone(), model.clone());
        crate::bake::bind(&mut posed, Some(&body));
        crate::bake::bind(&mut moves, Some(&body));
        let seating = crate::bake::Seating::read(&moves, &body);
        crate::bake::bind_for_motion(&mut moves, seating.as_ref(), Some(&body.flesh));
        // The flank level with the middle of the STANDING side's scapula, out past the bone —
        // and the same flesh across the plane on the raised side.
        let beside = j["clavicle_l"].lerp(j["upperarm_l"], 0.5);
        let mut shares = Vec::new();
        for (side, out) in [("l", 1.0_f32), ("r", -1.0)] {
            let flank = |p: Vec3| {
                (p.y - beside.y).abs() < 6.0
                    && (p.z - beside.z).abs() < 6.0
                    && out * p.x > beside.x + 2.0
            };
            let on_the_leg = |m: &RawModel| -> f32 {
                let leg = |i: u32| {
                    [
                        "clavicle",
                        "upperarm",
                        "lowerarm",
                        "hand",
                        "foredigit",
                        "forehoof",
                    ]
                    .iter()
                    .any(|n| m.bones[i as usize].name == format!("{n}_{side}"))
                };
                let picked: Vec<f32> = m
                    .vertices
                    .iter()
                    .filter(|v| flank(Vec3::from_array(v.p)))
                    .map(|v| {
                        (0..4)
                            .filter(|&k| leg(v.joints[k]))
                            .map(|k| v.weights[k])
                            .sum()
                    })
                    .collect();
                assert!(!picked.is_empty(), "{side}: the fixture has flank flesh");
                picked.iter().sum::<f32>() / picked.len() as f32
            };
            let (was, now) = (on_the_leg(&posed), on_the_leg(&moves));
            // The fixture's scapula lies 1.8 of its own radii under this flank, where the
            // sleeve's bell leaves the foreleg about a third of the flesh (the rest is the
            // trunk's to share): a part, never none.
            assert!(
                was < 0.02 && now > 0.25,
                "{side}: the flank over the shoulder is the spine's to be un-posed on ({was:.2}) \
                 and the foreleg's to move in ({now:.2})"
            );
            shares.push(now);
        }
        assert!(
            (shares[0] - shares[1]).abs() < 0.1,
            "the raised side's shoulder is bound like its standing twin's: {shares:?}"
        );
    }

    /// THE SKIN A BODY MOVES IN KEEPS THE LIMBS IT WAS UN-POSED ON. An un-pose can stand one foot
    /// against its twin's — a squared limb is its twin's reflection, and a source that walks on
    /// a line plants its hooves on that line (measured: a squared forefoot 9 cm from its twin's;
    /// read back off the mesh as it then stood, the two feet were one limb's flesh and 267
    /// positions of one hoof hung on the other leg's bones). The seating is read AS POSED
    /// ([`crate::bake::Seating`]) and the skin it moves in is bound on that: flesh that was
    /// wholly a leg's is wholly that leg's still, wherever the un-pose stood it.
    #[test]
    fn the_skin_a_body_moves_in_keeps_each_limbs_flesh_on_its_own_bones() {
        let (mut model, _, _) = buried_limbs_quadruped();
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let body = report.body.expect("the fit hands back its read");
        crate::bake::bind(&mut model, Some(&body));
        let seating = crate::bake::Seating::read(&model, &body);
        let leg = ["thigh_l", "calf_l", "foot_l", "ball_l", "hoof_l"];
        let lower = &leg[2..];
        let on = |m: &RawModel, v: &crate::fbx::RawVertex, names: &[&str]| -> f32 {
            (0..4)
                .filter(|&k| names.contains(&m.bones[v.joints[k] as usize].name.as_str()))
                .map(|k| v.weights[k])
                .sum()
        };
        let own: Vec<bool> = model
            .vertices
            .iter()
            .map(|v| on(&model, v, &leg) > 0.999)
            .collect();
        assert!(
            own.iter().filter(|o| **o).count() > 100,
            "the leg has flesh"
        );
        // THE UN-POSE, by hand: the left hind leg from its hock down, carried across until its
        // hoof stands 4 cm from its twin's — the skin following by its weights, as an un-pose's
        // does.
        let across = -(2.0 * joints(&model)["hoof_l"].x - 4.0);
        let carried: Vec<f32> = model
            .vertices
            .iter()
            .map(|v| on(&model, v, lower))
            .collect();
        for (v, w) in model.vertices.iter_mut().zip(&carried) {
            v.p[0] += across * w;
        }
        let mut world = model_world_frames(&model);
        for (b, g) in model.bones.iter().zip(&mut world) {
            if lower.contains(&b.name.as_str()) {
                g.w_axis.x += across;
            }
        }
        write_world_frames(&mut model.bones, &world);
        let hooves = joints(&model);
        assert!(
            hooves["hoof_l"].distance(hooves["hoof_r"]) < 4.5,
            "the hooves stand against each other: {} / {}",
            hooves["hoof_l"],
            hooves["hoof_r"]
        );

        // Read back off the mesh as it now stands, the leg is no longer all its own...
        let mut reread = model.clone();
        let stood = Body::read(&reread);
        let again = crate::bake::Seating::read(&reread, &stood);
        crate::bake::bind_for_motion(&mut reread, again.as_ref(), Some(&stood.flesh));
        let lost = |m: &RawModel| {
            m.vertices
                .iter()
                .zip(&own)
                .filter(|(v, o)| **o && on(m, v, &leg) < 0.999)
                .count()
        };
        assert!(
            lost(&reread) > 0,
            "the fixture stands a foot where a second read hands its flesh away"
        );
        // ...and on the seating it was un-posed on, it is.
        crate::bake::bind_for_motion(&mut model, seating.as_ref(), None);
        assert_eq!(
            lost(&model),
            0,
            "flesh that was wholly the leg's is wholly the leg's still"
        );
    }

    /// THE GATE ON A BIPED'S STRAIGHT LEG (BAD0D72C). A canon-shaped figure whose leg tubes leave
    /// the trunk at the canon's own crotch: laid from the foot, the hip is the canon's own (its
    /// socket, inside the pelvis over the crotch, where the leg's tube start put it 8 cm lower)
    /// and the knee lands within 4 cm of the canon's — the humanoid canon moves no further. And a
    /// tube that leaves its trunk AT its socket keeps the whole chain on the tube exactly as the
    /// tube alone lays it, bit for bit.
    #[test]
    fn a_bipeds_straight_leg_keeps_the_canons_hip_and_knee() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let h = 170.0_f32;
        let crotch = (0.560 - crate::baseline::BIPED_PELVIS_OVER_CROTCH) * h;
        let mut parts = vec![box_mesh(-18.0, 18.0, -12.0, 12.0, crotch, h)];
        for sign in [1.0_f32, -1.0] {
            let x = sign * 0.051 * h;
            parts.push(tube(
                &[
                    Vec3::new(x, 0.0, crotch + 3.0),
                    Vec3::new(x, 1.5, 12.0),
                    Vec3::new(x, 2.5, 5.0),
                    Vec3::new(x, -16.0, 3.0),
                ],
                &[6.0, 5.0, 4.5, 3.0],
            ));
        }
        let mut model = merge(parts);
        let mut canon = model.clone();
        install_skeleton(&mut canon, &SkeletonRecipe::humanoid(), h).expect("composes");
        let (was, flesh) = (joints(&canon), Flesh::build_body(&model));
        fit_baseline_to_mesh(&mut model, h, &SkeletonRecipe::humanoid()).expect("the fit runs");
        let now = joints(&model);
        for side in ["l", "r"] {
            for (n, within) in [("thigh", flesh.cell()), ("calf", 4.0)] {
                let (p, c) = (now[&format!("{n}_{side}")], was[&format!("{n}_{side}")]);
                assert!(
                    p.distance(c) <= within && flesh.contains(p),
                    "{n}_{side} within {within:.1} cm of the canon, inside: {p} vs {c}"
                );
            }
        }
        // THE TUBE THAT LEAVES AT ITS SOCKET: the chain on the tube alone, unchanged.
        let tube_path: Vec<Vec3> = (0..=40)
            .map(|k| Vec3::new(9.0, 0.0, 88.0 - 2.2 * k as f32))
            .collect();
        let radii = vec![5.0; tube_path.len()];
        let chain = [
            Vec3::new(9.0, 0.0, 90.0),
            Vec3::new(9.0, 0.0, 48.0),
            Vec3::new(9.0, 2.0, 6.0),
            Vec3::new(9.0, -12.0, 0.0),
        ];
        let body = Flesh::build_body(&merge(vec![tube(
            &[Vec3::new(9.0, 0.0, 95.0), Vec3::new(9.0, 0.0, 0.0)],
            &[6.0, 6.0],
        )]));
        let core = crate::shape::ShapeGraph::build(&body)
            .and_then(|g| g.cores.first().cloned())
            .expect("a tube has a core");
        let alone: Vec<Vec3> = chain_targets(&chain, &tube_path, &[], None)
            .into_iter()
            .map(|t| t.0)
            .collect();
        let tip = tube_path[tube_path.len() - 1];
        for socket in [tube_path[0], tube_path[0] - Vec3::Z * 3.0] {
            assert!(
                beyond(socket, tube_path[0], tip) <= 0.0,
                "a socket at {socket} is not beyond the tube's start"
            );
        }
        assert_eq!(
            from_the_tip(
                &body,
                &core,
                &chain,
                None,
                &tube_path,
                &radii,
                Some(ATTACHMENT_FIT)
            ),
            Some(alone),
            "...so the tube holds the whole chain, laid as the tube alone lays it"
        );
    }

    /// THE GATE ON THE SPINE LAID DOWN THE CORE. A HUMPBACKED quadruped — its croup 40 cm higher
    /// than its withers — has no single stature that states both heights, so a rest composed at
    /// one of them and shifted rigidly puts the other in the air. This is the widest failure the
    /// 32-body sweep found: `spine_03` outside the flesh on 25 of them, the Beaver, the Rat, the
    /// Panther and both Goats among them. The chain laid down the body's own back line puts every
    /// trunk joint inside it by construction.
    #[test]
    fn a_humpbacked_quadrupeds_withers_land_inside_its_own_shoulders() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        // A barrel that slopes: tall over the rump (+y), low over the shoulders (−y).
        let mut parts = vec![
            box_mesh(-22.0, 22.0, 6.0, 60.0, 74.0, 150.0),
            box_mesh(-22.0, 22.0, -60.0, 6.0, 74.0, 110.0),
        ];
        for (x, y) in [(15.0, 46.0), (-15.0, 46.0), (15.0, -46.0), (-15.0, -46.0)] {
            parts.push(tube(
                &[
                    Vec3::new(x, y, 0.0),
                    Vec3::new(x, y, 40.0),
                    Vec3::new(x, y, 80.0),
                ],
                &[6.0, 5.0, 8.0],
            ));
        }
        // A neck and head ahead of the low shoulders, and a tail behind the high rump.
        parts.push(tube(
            &[Vec3::new(0.0, -58.0, 100.0), Vec3::new(0.0, -92.0, 118.0)],
            &[9.0, 9.0],
        ));
        parts.push(box_mesh(-13.0, 13.0, -122.0, -88.0, 106.0, 132.0));
        parts.push(tube(
            &[Vec3::new(0.0, 58.0, 138.0), Vec3::new(0.0, 112.0, 120.0)],
            &[5.0, 3.0],
        ));
        let mut model = merge(parts);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        fit_baseline_to_mesh(&mut model, 170.0, &recipe).expect("the fit runs");
        let flesh = Flesh::build_body(&model);
        let w = model_world_frames(&model);
        for name in ["pelvis", "spine_01", "spine_02", "spine_03"] {
            let i = bone_at(&model, name).expect("the canon composes a spine");
            let p = pos_of(w[i]);
            assert!(
                flesh.contains(p),
                "{name} is INSIDE this body's own back, got {p}"
            );
        }
        let pelvis = pos_of(w[bone_at(&model, "pelvis").expect("a pelvis")]);
        let withers = pos_of(w[bone_at(&model, "spine_03").expect("a spine_03")]);
        assert!(
            pelvis.z > withers.z + 20.0,
            "the rest FOLLOWS the hump — the croup rides well above the withers, got pelvis \
             {pelvis} withers {withers}"
        );
        assert!(
            pelvis.y > withers.y,
            "and the pelvis is still behind the withers, got pelvis {pelvis} withers {withers}"
        );
    }

    /// THE GATE ON THE CROTCH READ. A LONG-LEGGED biped — a bird's build, body high, legs bare all
    /// the way to the floor — has a gap on its symmetry plane that runs to the ground, so the
    /// walk up the plane trips on the first thing that crosses it (a toe, a trailing tail) and
    /// reads a crotch at ankle height. The crotch is the HIGHEST of the three lower bounds, so
    /// the bottom of the trunk's own core catches it and the pelvis stays in the body.
    #[test]
    fn a_long_legged_bipeds_pelvis_is_not_read_off_its_ankles() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let mut parts = vec![
            // The body, well clear of the ground.
            box_mesh(-20.0, 20.0, -26.0, 26.0, 96.0, 150.0),
            // A head on a short neck.
            box_mesh(-11.0, 11.0, -20.0, 20.0, 150.0, 168.0),
        ];
        for x in [13.0_f32, -13.0] {
            // The leg: thigh, a narrowing at the knee, shin, and a FOOT that crosses the plane.
            parts.push(tube(
                &[
                    Vec3::new(x, 8.0, 100.0),
                    Vec3::new(x, 6.0, 60.0),
                    Vec3::new(x, 4.0, 20.0),
                    Vec3::new(x, 0.0, 4.0),
                ],
                &[9.0, 4.5, 5.0, 4.0],
            ));
            parts.push(box_mesh(
                x.min(0.0) - 4.0,
                x.max(0.0) + 4.0,
                -14.0,
                10.0,
                0.0,
                6.0,
            ));
        }
        let mut model = merge(parts);
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Humanoid);
        fit_baseline_to_mesh(&mut model, 168.0, &recipe).expect("the fit runs");
        let flesh = Flesh::build_body(&model);
        let w = model_world_frames(&model);
        let pelvis = pos_of(w[bone_at(&model, "pelvis").expect("a pelvis")]);
        assert!(
            flesh.contains(pelvis),
            "the pelvis is INSIDE the body and not down among the legs, got {pelvis}"
        );
        assert!(
            pelvis.z > 96.0,
            "it is in the BODY, which starts at 96, and not at the ankles the plane walk trips \
             on, got {pelvis}"
        );
    }

    /// THE QUADRUPED ALIGNMENT (incident D81498B7): the composed rest lands ON the animal's own
    /// trunk — the pelvis in the rear of the barrel just under the croup, the withers at the front
    /// of it under the withers block, both INSIDE the flesh, every hoof on the ground and the head
    /// ahead of the withers. The stature is MEASURED off the back, not taken from the typed
    /// bounding height (which here is the raised head, 180 cm for a 136 cm back).
    #[test]
    fn align_trunk_lands_a_quadruped_rest_on_its_own_trunk() {
        let shift = 6.0;
        let mut model = horse_fixture(shift);
        let typed = 180.0;
        let recipe = crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped);
        let report = fit_baseline_to_mesh(&mut model, typed, &recipe).expect("the fit runs");
        let a = report.align.expect("the horse fixture has a trunk core");
        let h = a.stature;
        let expected = FIX_BACK / crate::baseline::QUAD_WITHERS;
        assert!(
            (h - expected).abs() < 0.05 * expected,
            "the stature is measured off the BACK ({expected:.1}), not the typed {typed}: got {h:.1}"
        );
        assert!(
            (a.plane_x - shift).abs() < 1.0,
            "the midline goes on the body's own symmetry plane ({shift}), got {:.2}",
            a.plane_x
        );
        let flesh = Flesh::build_body(&model);
        let withers = a.withers.expect("a quadruped measures its withers");
        for (what, p) in [("pelvis", a.pelvis), ("withers", withers)] {
            assert!(flesh.contains(p), "the {what} is INSIDE the flesh, got {p}");
            assert!(
                (p.x - shift).abs() < 1.0,
                "the {what} sits on the plane, got {p}"
            );
            // IN THE BARREL, between its belly and its back — not a fixed depth under the
            // croup. The trunk's chain is now LAID DOWN ITS CORE (spec 04803E0C §3), so where a
            // trunk joint sits in height is the body's own centreline there and not a fraction
            // of one stature measured somewhere else. That is the whole point of the change: a
            // body whose withers sit below its croup used to have `spine_03` composed at the
            // croup's height over its own shoulders, in the air.
            assert!(
                p.z > FIX_BELLY && p.z < FIX_BACK,
                "the {what} rides INSIDE the barrel ({FIX_BELLY}..{FIX_BACK}), got {p}"
            );
        }
        assert!(
            a.pelvis.y > FIX_TRUNK_Y / 3.0,
            "the pelvis is in the REAR of the barrel, got {}",
            a.pelvis
        );
        assert!(
            withers.y < -FIX_TRUNK_Y / 3.0,
            "the withers are at the FRONT of the barrel, got {withers}"
        );
        let w = model_world_frames(&model);
        let at = |n: &str| pos_of(w[model.bones.iter().position(|b| b.name == n).unwrap()]);
        for hoof in ["hoof_l", "hoof_r", "forehoof_l", "forehoof_r"] {
            assert!(
                at(hoof).z.abs() < 0.02 * h,
                "{hoof} reaches the ground by construction, got {}",
                at(hoof)
            );
        }
        assert!(
            at("head").y < withers.y,
            "the head is carried AHEAD of the withers, got {} vs {withers}",
            at("head")
        );
    }

    /// A SYNTHETIC BIPED — a torso box over two tube legs, the whole figure offset sideways: the
    /// pelvis lands on the figure's OWN midline, the canon fraction above where its legs part,
    /// and the stature stays the typed one (a biped's height IS its height).
    #[test]
    fn align_trunk_puts_a_bipeds_pelvis_over_its_own_crotch() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let (shift, crotch, typed) = (7.0_f32, 85.0_f32, 170.0_f32);
        let mut parts = vec![box_mesh(
            shift - 18.0,
            shift + 18.0,
            -12.0,
            12.0,
            crotch,
            typed,
        )];
        for sign in [1.0_f32, -1.0] {
            let x = shift + sign * 9.0;
            parts.push(tube(
                &[Vec3::new(x, 0.0, 88.0), Vec3::new(x, 0.0, 0.0)],
                &[5.0, 5.0],
            ));
        }
        let mut model = merge(parts);
        let report = fit_baseline_to_mesh(&mut model, typed, &SkeletonRecipe::humanoid())
            .expect("the fit runs");
        let a = report.align.expect("the biped fixture has a trunk core");
        assert_eq!(a.stature, typed, "a biped keeps the TYPED stature");
        assert_eq!(a.length, None, "a plumb trunk measures no body length");
        assert!(
            (a.pelvis.x - shift).abs() < 0.5,
            "the pelvis goes on the figure's own plane ({shift}), got {}",
            a.pelvis
        );
        // WHERE THE LEGS PART is the junction the graph reads — the top of the leg tubes, not the
        // torso block's lower face (the tubes run up inside it). The old belly-run read took the
        // block's face; the shape graph takes the attachment, which is the figure's own statement
        // of where it forks.
        let legs_part = 88.0_f32;
        let want = legs_part + crate::baseline::BIPED_PELVIS_OVER_CROTCH * typed;
        assert!(
            (a.pelvis.z - want).abs() < 1.0,
            "the pelvis sits the canon fraction over the crotch ({want:.1}), got {}",
            a.pelvis
        );
    }

    /// AND A BODY THAT ALREADY MATCHES THE CANON IS LEFT WHERE IT IS — the humanoid canon is
    /// bit-exact at its own knob (spec C658F114) and an alignment that nudged it would be a
    /// regression on every promoted human body. A figure whose legs part at the canon's own crotch
    /// keeps the canon's pelvis to within one cell of the flesh field, which is the finest the
    /// measurement can resolve.
    #[test]
    fn align_trunk_leaves_a_canon_shaped_body_where_it_stands() {
        use crate::flesh::fixtures::{box_mesh, merge, tube};
        let h = 170.0_f32;
        let canon_crotch = (0.560 - crate::baseline::BIPED_PELVIS_OVER_CROTCH) * h;
        let mut parts = vec![box_mesh(-18.0, 18.0, -12.0, 12.0, canon_crotch, h)];
        for sign in [1.0_f32, -1.0] {
            let x = sign * 0.051 * h; // the canon's own femoral-head offset
            parts.push(tube(
                &[
                    Vec3::new(x, 0.0, canon_crotch + 3.0),
                    Vec3::new(x, 0.0, 0.0),
                ],
                &[6.0, 6.0],
            ));
        }
        let mut model = merge(parts);
        let mut canon = model.clone();
        install_skeleton(&mut canon, &SkeletonRecipe::humanoid(), h).expect("composes");
        let was = named(&canon);
        fit_baseline_to_mesh(&mut model, h, &SkeletonRecipe::humanoid()).expect("the fit runs");
        let now = named(&model);
        let cell = Flesh::build_body(&model).cell();
        for (name, p) in &now {
            if !matches!(name.as_str(), "pelvis" | "spine_03") {
                continue; // the LIMBS are the limb fits' business, not the alignment's
            }
            let before = was
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, q)| *q)
                .unwrap();
            assert!(
                p.distance(before) <= cell,
                "{name} stays on the canon within one cell ({cell:.2} cm), moved {:.2}",
                p.distance(before)
            );
        }
    }

    /// THE REAL HORSE — the body of incident D81498B7 itself, when the content tree carries it.
    /// WHAT THE MATCHER GUARANTEES TODAY, and nothing it does not (rule CE0451CE: a gate that
    /// claims more than was measured is worse than no gate):
    ///
    /// * the trunk joints the graph MEASURES — pelvis, withers and the DETECTED head — stand
    ///   INSIDE the animal, and the stature is the withers height and not the bounding box the
    ///   raised head sets. This is the column that failed on 20 of the 32 swept quadrupeds
    ///   (3A086B41) and the headline of the shape graph (S2 431D08DF);
    /// * a limb module the graph MATCHED has its chain laid down that limb's own tube, so its
    ///   ground joints stand ON the leg and down at the bottom of it;
    /// * a limb module the graph matched NOTHING for is PROMPTED, not asserted: it keeps its
    ///   composed rest and appears in the rail's opening list (`ShapeMatch::marker_order`). Rule
    ///   513E5F78 — what does not match is what the human is asked for.
    ///
    /// THE GAP THIS GATE RECORDS. On this Horse the hind legs arrive tangled with the tail hair,
    /// so the graph separates only ONE pair: the forelegs, which the shoulder module takes, while
    /// the hind leg module is unmatched and prompted. And the matched pair is not square — the
    /// left forehoof lands 1.8 cm off the floor, the right 16.3, because the right foreleg's
    /// thinned lead stops where the leg meets the chest and the extension runs out of flesh
    /// before the hoof. Both are inside the leg. The centimetres are banked against the graph
    /// rather than papered over here; [`GROUND_CEILING`] is the measurement, not a target.
    ///
    /// Skipped (not failed) on a tree without the creature, so a content-less checkout stays
    /// green.
    #[test]
    fn the_real_horses_rest_lands_inside_the_horse() {
        /// What a MATCHED limb's ground joints actually reach on this body today (cm off the
        /// floor). The left forehoof reads 1.8; the right 16.3.
        const GROUND_CEILING: f32 = 17.0;

        let Some((_, rig)) = horse_rig() else {
            eprintln!("skipping: no Horse in the tree");
            return;
        };
        let text = crate::package::read_text(&rig).expect("the Horse reads");
        let file: flicker_skeletal::format::RigFile =
            serde_json::from_str(&text).expect("the Horse parses");
        let Some(recipe) = file.skeleton_recipe.clone() else {
            eprintln!("skipping: this Horse carries no recipe (a static bake)");
            return;
        };
        let mut model = crate::bake::load_rig_raw(&rig).expect("the Horse loads");
        let (lo, hi) = bbox(&model);
        let report = fit_baseline_to_mesh(&mut model, hi.z - lo.z, &recipe).expect("the fit runs");
        let a = report.align.expect("the Horse has a trunk core");
        let m = report.shape.as_ref().expect("the Horse has a shape graph");
        let flesh = Flesh::build_body(&model);
        let w = model_world_frames(&model);
        let at = |n: &str| pos_of(w[model.bones.iter().position(|b| b.name == n).unwrap()]);

        // ── THE TRUNK the graph measured, inside the animal.
        assert!(
            flesh.contains(a.pelvis),
            "the pelvis is INSIDE the horse, got {}",
            a.pelvis
        );
        let withers = a.withers.expect("a quadruped measures its withers");
        assert!(
            flesh.contains(withers),
            "the withers are INSIDE the horse, got {withers}"
        );
        // THE HEAD IS DETECTED, not composed a fraction of the stature ahead of the withers.
        assert!(
            flesh.contains(at("head")),
            "the DETECTED head is INSIDE the horse, got {}",
            at("head")
        );
        assert!(
            at("head").y < withers.y,
            "the head is carried AHEAD of the withers"
        );
        assert!(
            a.stature < 0.9 * (hi.z - lo.z),
            "the stature is the WITHERS height, well under the bounding {:.0} cm the head sets: got {:.1}",
            hi.z - lo.z,
            a.stature
        );

        // ── EVERY LIMB MODULE: matched limbs are PLACED, unmatched ones are PROMPTED.
        let prompts = m.marker_order(&recipe);
        let mut matched_any = false;
        for (kind, ground) in [
            ("leg", ["hoof_l", "hoof_r"]),
            ("arm", ["forehoof_l", "forehoof_r"]),
        ] {
            let id = crate::baseline::module_id(kind, "");
            if m.of(&id).is_some() {
                matched_any = true;
                for g in ground {
                    let p = at(g);
                    assert!(
                        flesh.contains(p),
                        "{g} belongs to the MATCHED {id} and stands on its own leg, got {p}"
                    );
                    assert!(
                        p.z - lo.z <= GROUND_CEILING,
                        "{g} belongs to the MATCHED {id} and reaches the bottom of it (within \
                         {GROUND_CEILING} cm of the floor), got {:.1} cm up",
                        p.z - lo.z
                    );
                }
            } else {
                assert!(
                    m.unmatched.contains(&id),
                    "{id} matched nothing, so it is in the unmatched list"
                );
                assert!(
                    !prompts.is_empty(),
                    "and the rail opens on the prompts of the modules that matched nothing"
                );
            }
        }
        assert!(
            matched_any,
            "the Horse's legs are not all invisible to the graph: {}",
            m.report()
        );
    }

    /// DIAGNOSTIC on REAL BODIES — the trunk alignment measured on whole FAMILIES, not on one
    /// animal (rule CE0451CE: a rule measured on one horse is unproven on the elk). Re-fits each
    /// body from its own recipe and prints what [`align_trunk`] measured, where the shipped rest
    /// landed, and whether each trunk joint is inside the flesh and each ground joint on the floor.
    ///
    /// `FLICKER_ALIGN_SWEEP=<dir>` sweeps every `<dir>/<Name>/<Name>.json` a headless
    /// `--example import_folder` wrote there; unset, it reads whichever Horse the content tree
    /// carries (incident D81498B7 — Aaron in the window: the composed rest "has never been
    /// close"). `#[ignore]`d (it needs real bodies); run:
    /// `cargo test -p flicker-content -- --ignored align --nocapture`.
    #[test]
    #[ignore]
    fn diagnose_align_trunk_on_the_real_bodies() {
        let rigs: Vec<(String, std::path::PathBuf)> = match std::env::var("FLICKER_ALIGN_SWEEP") {
            Ok(dir) => {
                let mut out: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
                    .unwrap_or_else(|e| panic!("reading the sweep folder {dir}: {e}"))
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let rig = e.path().join(format!("{name}.json"));
                        crate::package::file_exists(&rig).then_some((name, rig))
                    })
                    .collect();
                out.sort();
                assert!(!out.is_empty(), "no <Name>/<Name>.json under {dir}");
                out
            }
            Err(_) => match horse_rig() {
                Some((tier, rig)) => vec![(format!("{tier} Horse"), rig)],
                None => {
                    eprintln!("skipping: no Horse in the tree and no FLICKER_ALIGN_SWEEP folder");
                    return;
                }
            },
        };
        for (name, rig) in rigs {
            diagnose_one(&name, &rig);
        }
    }

    /// THE SHAPE SWEEP (spec 04803E0C's harness). `FLICKER_SHAPE_SWEEP=<dir>` walks every
    /// `<dir>/<Name>/<Name>.json` a headless `import_folder` baked and prints, per body: the
    /// graph's own summary and detail, the recipe MATCH (what took what, what matched nothing,
    /// what the mesh has that the recipe does not), and the verdict columns — every joint inside
    /// the flesh, the worst ground joint off the floor, and how far the rest moved.
    ///
    /// Unset, it reads the tree's Horse, so a checkout with content still exercises it.
    #[test]
    #[ignore]
    fn diagnose_shape_on_the_real_bodies() {
        let rigs: Vec<(String, std::path::PathBuf)> = match std::env::var("FLICKER_SHAPE_SWEEP") {
            Ok(dir) => {
                let mut out: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
                    .expect("the sweep folder reads")
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let rig = e.path().join(format!("{name}.json"));
                        let gz = e.path().join(format!("{name}.json.gz"));
                        rig.is_file()
                            .then(|| (name.clone(), rig))
                            .or_else(|| gz.is_file().then_some((name, gz)))
                    })
                    .collect();
                out.sort();
                assert!(!out.is_empty(), "no <Name>/<Name>.json under {dir}");
                out
            }
            Err(_) => match horse_rig() {
                Some((tier, rig)) => vec![(format!("{tier} Horse"), rig)],
                None => {
                    eprintln!("skipping: no Horse and no FLICKER_SHAPE_SWEEP folder");
                    return;
                }
            },
        };
        for (name, rig) in rigs {
            shape_one(&name, &rig);
        }
    }

    /// One body through the shape sweep.
    fn shape_one(name: &str, rig: &std::path::Path) {
        let t0 = std::time::Instant::now();
        let text = crate::package::read_text(rig).expect("the rig reads");
        let file: flicker_skeletal::format::RigFile =
            serde_json::from_str(&text).expect("the rig parses");
        let recipe = file.skeleton_recipe.clone().unwrap_or_else(|| {
            crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped)
        });
        let mut model = crate::bake::load_rig_raw(rig).expect("the rig loads as a raw model");
        let (lo, hi) = bbox(&model);
        let stature = hi.z - lo.z;
        let pattern = format!("{:?}", recipe.pattern());
        let verts = model.vertices.len();
        println!(
            "\n### {name}: {pattern}, {verts} verts, bbox z {:.1}..{:.1}",
            lo.z, hi.z
        );
        // THE FIT READS THE BODY, once, and hands the read back: every question below — the graph
        // printed, a joint inside the flesh, the stance squared, a foot's height — is asked of it.
        let report = fit_baseline_to_mesh(&mut model, stature, &recipe).expect("the fit runs");
        let Some(body) = report.body else {
            println!("ROW\t{name}\t{pattern}\tNO BODY");
            return;
        };
        let Some(graph) = body.graph.as_ref() else {
            println!("ROW\t{name}\t{pattern}\tNO GRAPH");
            return;
        };
        println!("  GRAPH {}", graph.summary());
        print!("{}", graph.detail());
        let m = report.shape.clone().unwrap_or_default();
        print!("{}", m.report());
        let placed = named(&model);
        let inside = |n: &str| {
            placed
                .iter()
                .find(|(q, _)| q == n)
                .map(|(_, p)| body.flesh.contains(*p))
        };
        let yes = |b: Option<bool>| match b {
            Some(true) => "Y",
            Some(false) => "N",
            None => "-",
        };
        for n in ["pelvis", "spine_03", "neck_01", "neck_02", "head"] {
            if let Some((_, p)) = placed.iter().find(|(q, _)| q == n) {
                let (x, y, z) = (p.x, p.y, p.z);
                println!(
                    "  trunk  {n:>14}: ({x:7.1},{y:7.1},{z:7.1})  inside {}",
                    body.flesh.contains(*p)
                );
            }
        }
        // THE FACE THE FIT READ — the yaw `bake::face_forward` would un-turn on this body (on a
        // body the import already faced forward, what is left of the turn), and whether the neck
        // it followed stayed inside the flesh.
        let necks_in = format!("{}{}", yes(inside("neck_01")), yes(inside("neck_02")));
        let face = {
            let at = |n: &str| placed.iter().find(|(q, _)| q == n).map(|(_, p)| *p);
            match (at("head"), at("eye_l"), at("eye_r")) {
                (Some(h), Some(l), Some(r)) => {
                    let f = 0.5 * (l + r) - h;
                    format!("{:+.1}", f.x.atan2(-f.y).to_degrees())
                }
                _ => "-".to_string(),
            }
        };
        println!("  face yaw {face}°  neck inside {necks_in}");

        // THE LIMB CHAINS AS LAID (BAD0D72C: a chain laid one joint late skipped the hip) — every
        // matched pair's own chain, root to far end, each joint inside the flesh or not and where
        // along its trunk core it sits (0 = the rear). A matched LEG is judged on its root: the
        // joint the tube does not hold sits in the body's REAR THIRD, inside the flesh, above the
        // joint below it — `hip` is `Y` per side when it does, else what failed (o outside, f not
        // in the rear third, b below the next joint).
        let frames = model_world_frames(&model);
        let mut hips = String::new();
        let root_core = match m.of(&crate::baseline::module_id("trunk", "")) {
            Some(Matched::Core(c)) => graph.cores.get(c),
            _ => None,
        };
        for (id, what) in &m.matched {
            let (Some((kind, prefix)), Matched::Pair(_)) = (id.split_once(':'), what) else {
                continue;
            };
            for side in ["l", "r"] {
                let base = if kind == "leg" { "thigh" } else { "clavicle" };
                let Some(root) = bone_at(&model, &format!("{prefix}{base}_{side}"))
                    .or_else(|| bone_at(&model, &format!("{prefix}upperarm_{side}")))
                else {
                    continue;
                };
                let chain = deep_chain(&model, root);
                let pts: Vec<Vec3> = chain.iter().map(|&i| pos_of(frames[i])).collect();
                let line = chain
                    .iter()
                    .zip(&pts)
                    .map(|(&i, p)| {
                        format!(
                            "{} ({:.1},{:.1},{:.1}){}{}",
                            model.bones[i].name,
                            p.x,
                            p.y,
                            p.z,
                            if body.flesh.contains(*p) { "" } else { "!out" },
                            root_core.map_or(String::new(), |c| format!(" t{:.2}", c.t_of(*p)))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" → ");
                println!("  chain {id}{side}: {line}");
                if kind == "leg" && pts.len() >= 2 {
                    let t = root_core.map_or(0.0, |c| c.t_of(pts[0]));
                    hips.push(if !body.flesh.contains(pts[0]) {
                        'o'
                    } else if t > 1.0 / 3.0 {
                        'f'
                    } else if pts[0].z <= pts[1].z {
                        'b'
                    } else {
                        'Y'
                    });
                }
            }
        }
        if hips.is_empty() {
            hips.push('-');
        }

        // THE STANCE FIRST, because the bake squares it first (56091EDF: `square_stance` runs
        // inside the ONE bake path after the fit and before the rig is written). EVERY Meshy
        // source is generated mid-stride with one paw or hoof raised (42AB9BA8), and now that the
        // graph lays each matched limb down its own tube the fit READS that raise instead of
        // composing it away. A foot measured before the normaliser is measuring the pose, not the
        // fit. Squared on the fit's own read, as the import squares it — on the TUBE skin the
        // import binds first (`bake::bind`). The rig as written carries the skin it MOVES in,
        // whose shares reach up the flank by design; a limb carried on that skin drags the flank
        // with it and the re-read body loses the hind tube under its foot (one source's hind
        // foot read 4.7 cm off the floor on the motion skin, 1.1 on the tube skin, 2026-10-08).
        crate::bake::bind(&mut model, Some(&body));
        let squared = crate::bake::square_stance_on(
            &mut model,
            crate::bake::StanceSource::Auto,
            &recipe,
            Some(&body),
        );
        // What it squared, `limb(lift)`, and what it DECLINED, `!limb(the sink it would cost)`.
        let stance = if squared.squared.is_empty() && squared.declined.is_empty() {
            "-".to_string()
        } else {
            squared
                .squared
                .iter()
                .map(|(b, d)| format!("{b}({d:.0})"))
                .chain(
                    squared
                        .declined
                        .iter()
                        .map(|(b, s)| format!("!{b}({s:.0})")),
                )
                .collect::<Vec<_>>()
                .join(",")
        };
        // A square MOVED flesh, and a moved foot is read off the mesh it moved to.
        let restood = (!squared.squared.is_empty()).then(|| Body::read(&model));
        let stands = restood.as_ref().unwrap_or(&body);

        // THE FEET — every STANDING pair's (the recipe's own legs and ground-contact forelegs,
        // [`crate::bake::standing_pairs`]: a wing's digit on the floor is never one), each read by
        // its FLESH ([`crate::bake::Foot`]), with the MATCHED ones told apart from the
        // composed. A module in [`ShapeMatch::unmatched`] keeps its composed rest BY DESIGN (spec
        // 04803E0C §3 — it is prompted, not placed), so its foot standing where the canon put it
        // says nothing about the match. Only a foot the fit actually laid is evidence for or
        // against it.
        let pos: Vec<Vec3> = model_world_frames(&model)
            .iter()
            .map(|g| pos_of(*g))
            .collect();
        let (mut worst_matched, mut worst_any) = (0.0_f32, 0.0_f32);
        for pair in crate::bake::standing_pairs(&model, &recipe, &pos) {
            let mine = matches!(m.of(&pair.module), Some(Matched::Pair(_)));
            let tag = if mine { "MATCHED " } else { "composed" };
            for (_, g) in &pair.sides {
                let foot =
                    crate::bake::Foot::of(&model, Some(stands), pos[*g]).floor(&model) - lo.z;
                println!(
                    "  ground {:>14} {tag}: foot {foot:6.1} cm off the floor (joint {:6.1})",
                    model.bones[*g].name,
                    pos[*g].z - lo.z
                );
                worst_any = worst_any.max(foot.abs());
                if mine {
                    worst_matched = worst_matched.max(foot.abs());
                }
            }
        }

        let want = recipe_pairs(&recipe.trunk);
        let got = m
            .matched
            .iter()
            .filter(|(_, w)| matches!(w, Matched::Pair(_)))
            .count();
        let ratio = report.align.map_or(f32::NAN, |a| a.stature / stature);
        let counts = format!("{}/{}", m.matched.len(), m.unmatched.len());
        // THE HEAD COLUMN IS LOWER CASE WHEN THE HEAD MATCHED NOTHING. A module in
        // [`ShapeMatch::unmatched`] keeps the composed rest and is PROMPTED (spec 04803E0C §3):
        // its head sitting where the canon puts it is the rail's work, not a bad fit, and what
        // the matcher guarantees is the DETECTED head inside the flesh (63797813). A head PLACED
        // ON ITS CORE'S FRONT CAP ([`ShapeMatch::capped`]) is still prompted but no longer
        // composed, so it is judged: `C` inside, `X` outside.
        let head_id = crate::baseline::module_id("head", "");
        let capped = m.capped.iter().any(|(id, _)| *id == head_id);
        let head_found = m.of(&head_id).is_some() || capped;
        let head_in = inside("head");
        let insides = format!(
            "{}{}{}",
            yes(inside("pelvis")),
            yes(inside("spine_03")),
            match (capped, head_found) {
                (true, _) => if head_in == Some(true) { "C" } else { "X" }.to_string(),
                (_, true) => yes(head_in).to_string(),
                _ => yes(head_in).to_lowercase(),
            }
        );
        let pairs = format!("{got}/{want}");
        // A CAPPED HEAD IS PLACED: it sits inside the flesh on its trunk's front cap (judged `C`/`X`
        // above), and only the rail still prompts it — so it is named `head:cap` here and is not
        // work left for the verdict below.
        let is_capped = |u: &String| m.capped.iter().any(|(id, _)| id == u);
        let missing = if m.unmatched.is_empty() {
            "-".to_string()
        } else {
            m.unmatched
                .iter()
                .map(|u| {
                    if is_capped(u) {
                        format!("{u}cap")
                    } else {
                        u.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        // THE VERDICT: a trunk joint outside the flesh, a MATCHED foot off the floor, or a stature
        // the mesh's own height does not support is a FAIL; every trunk joint in and every matched
        // foot down, with a limb pair the recipe asked for — or any module nothing placed — still
        // prompted, is PARTIAL: the rail's work, not the matcher's failure. A CAPPED head is placed
        // (inside its front cap; the rail still asks the human to eye it) and leaves a body PASS.
        // "Off the floor" is the body's own NOISE FLOOR
        // ([`crate::bake::noise_floor`]: 2 cm, or one cell of the grid its flesh is read at on a
        // body big enough for a cell to be coarser) — the same line the normaliser squares by, so
        // a 4 m body is judged at the grain it can be read at.
        let noise = crate::bake::noise_floor(lo, hi);
        let trunk_in = inside("pelvis") == Some(true)
            && inside("spine_03") != Some(false)
            && (!head_found || head_in != Some(false));
        let absurd = !ratio.is_nan() && !(0.4..=1.4).contains(&ratio);
        let verdict = if !trunk_in || worst_matched > noise || absurd {
            "FAIL"
        } else if got < want || m.unmatched.iter().any(|u| !is_capped(u)) {
            "PARTIAL"
        } else {
            "PASS"
        };
        println!(
            "ROW\t{name}\t{pattern}\t{}\t{}\t{}\t{}\t{counts}\t{pairs}\t{insides}\t\
             {worst_matched:.1}\t{worst_any:.1}\t{ratio:.3}\t{verdict}\t{:.1}s\t{missing}\t{stance}\t\
             {noise:.1}\t{necks_in}\t{face}\t{hips}",
            graph.cores.len(),
            graph.limbs.len(),
            graph.pairs.len(),
            graph.limbs.iter().filter(|l| l.sheet).count(),
            t0.elapsed().as_secs_f32()
        );
    }

    /// How many LIMB PAIRS the recipe asks for, over its whole trunk tree — what the PARTIAL
    /// verdict counts the matched pairs against.
    fn recipe_pairs(spec: &flicker_skeletal::format::TrunkSpec) -> usize {
        spec.arms.len()
            + spec.legs.len()
            + spec
                .mounts
                .iter()
                .map(|m| recipe_pairs(&m.trunk))
                .sum::<usize>()
    }

    /// One body through the diagnostic above. Prints a human block and one `ROW` line per body
    /// (tab-separated) so a sweep is a table without re-reading the prose.
    fn diagnose_one(name: &str, rig: &std::path::Path) {
        let t0 = std::time::Instant::now();
        let text = crate::package::read_text(rig).expect("the rig reads");
        let file: flicker_skeletal::format::RigFile =
            serde_json::from_str(&text).expect("the rig parses");
        let recipe = file.skeleton_recipe.clone().unwrap_or_else(|| {
            crate::baseline::reference_recipe(crate::baseline::Pattern::Quadruped)
        });
        let mut model = crate::bake::load_rig_raw(rig).expect("the rig loads as a raw model");
        let (lo, hi) = bbox(&model);
        let stature = hi.z - lo.z;
        println!(
            "\n### {name}: {:?}, {} verts, {} regions, bbox ({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1}), typed stature {stature:.1}",
            recipe.pattern(), model.vertices.len(), model.regions.len(),
            lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
        );
        let shipped_body = Body::read(&model);
        let flesh = &shipped_body.flesh;

        // AS SHIPPED — the rest the headless import actually baked into this rig.
        let shipped = named(&model);
        let inside = |n: &str| {
            shipped
                .iter()
                .find(|(m, _)| m == n)
                .map(|(_, p)| flesh.contains(*p))
        };
        for (n, p) in &shipped {
            println!(
                "  shipped {n:>12}: ({:7.1},{:7.1},{:7.1})  inside: {}",
                p.x,
                p.y,
                p.z,
                flesh.contains(*p)
            );
        }
        // THE FEET — the bake's own standing pairs ([`crate::bake::standing_pairs`], the pairs
        // `square_stance` squares: the recipe's legs and ground-contact forelegs, never a wing or a
        // hanging arm), each read by its FLESH ([`crate::bake::Foot`]), never by its joint.
        let pos: Vec<Vec3> = model_world_frames(&model)
            .iter()
            .map(|g| pos_of(*g))
            .collect();
        let ground: Vec<(String, f32)> = crate::bake::standing_pairs(&model, &recipe, &pos)
            .iter()
            .flat_map(|p| p.sides.iter())
            .map(|(_, g)| {
                let foot =
                    crate::bake::Foot::of(&model, Some(&shipped_body), pos[*g]).floor(&model);
                (model.bones[*g].name.clone(), foot - lo.z)
            })
            .collect();
        let worst_ground = ground.iter().fold(0.0_f32, |w, (_, z)| w.max(z.abs()));
        for (n, z) in &ground {
            println!("  ground  {n:>12}: foot {z:6.1} cm off the floor");
        }

        // THE MEASUREMENT ITSELF, re-run on this body's own geometry.
        let report = fit_baseline_to_mesh(&mut model, stature, &recipe).expect("the fit runs");
        let core = flesh.core();
        let (chest_y, rump_y, core_r) = core.map_or((f32::NAN, f32::NAN, f32::NAN), |c| {
            (c.lo.y, c.hi.y, c.radius)
        });
        if let Some(c) = core {
            // WHICH WAY THE BODY LIES: the core is the barrel with the limbs, neck, tail, ears —
            // and the horns, antlers and wool — left out, so its own axis is the BODY's, where
            // the bounding box is the headgear's (what `measure_facing` squares onto the rig).
            println!(
                "  CORE bbox ({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1})  axis {:.1}deg  along {:.1}  across {:.1}",
                c.lo.x, c.lo.y, c.lo.z, c.hi.x, c.hi.y, c.hi.z, c.axis_deg, c.along, c.across
            );
        }
        let (plane, measured, length, croup) = match report.align {
            Some(a) => (
                a.plane_x,
                a.stature,
                a.length.unwrap_or(f32::NAN),
                crate::baseline::QUAD_WITHERS * a.stature,
            ),
            None => (f32::NAN, f32::NAN, f32::NAN, f32::NAN),
        };
        println!(
            "  MEASURED plane {plane:.2}  chest_y {chest_y:.1}  rump_y {rump_y:.1}  core_r {core_r:.1}  \
             croup_skin_z {croup:.1}\n  KNOBS stature {measured:.1} (typed {stature:.1}, ratio {:.3})  length {length:.3}",
            measured / stature
        );
        println!(
            "ROW\t{name}\t{:?}\t{:.2}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{:.1}\t{:.1}",
            recipe.pattern(),
            plane,
            chest_y,
            rump_y,
            croup,
            measured,
            measured / stature,
            length,
            inside("pelvis").unwrap_or(false),
            inside("spine_03").unwrap_or(false),
            inside("head").unwrap_or(false),
            worst_ground,
            t0.elapsed().as_secs_f32(),
        );
    }

    /// The Horse the tree carries — the STAGED rig (the faced-forward, recipe-carrying one)
    /// before the promoted static bake.
    fn horse_rig() -> Option<(&'static str, std::path::PathBuf)> {
        let roots = crate::roots::roots();
        [
            (
                "staging",
                roots.staging().join("creatures/Horse/Horse.json"),
            ),
            (
                "package",
                roots.package().join("creatures/Horse/Horse.json"),
            ),
        ]
        .into_iter()
        .find(|(_, p)| crate::package::file_exists(p))
    }

    /// The trunk joints a trunk alignment is judged on, world, in the order they are printed.
    fn named(model: &RawModel) -> Vec<(String, Vec3)> {
        let w = model_world_frames(model);
        model
            .bones
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                matches!(
                    b.name.as_str(),
                    "pelvis"
                        | "spine_03"
                        | "neck_01"
                        | "neck_02"
                        | "head"
                        | "eye_l"
                        | "eye_r"
                        | "thigh_l"
                        | "hoof_l"
                        | "hoof_r"
                        | "forehoof_l"
                        | "forehoof_r"
                        | "ball_l"
                        | "ball_r"
                        | "foredigit_l"
                        | "foredigit_r"
                        | "clavicle_l"
                )
            })
            .map(|(i, b)| (b.name.clone(), pos_of(w[i])))
            .collect()
    }
}
