//! Conform a parsed body to our canonical skeleton — the in-app port of
//! `rename_meshy_to_canonical.py::reorient_to_canonical`, referencing the AUTHORED
//! **GolemBaseSkeleton** baseline (2026-08-04; see [`crate::baseline`] — the Katanami-derived
//! reference lineage is retired, and the live Motifect clips are retarget-baked against the
//! same authored bind bodies conform to).
//!
//! `reorient` rebuilds each bone's rest frame in two steps: (1) base frame = the REFERENCE's world
//! orientation + THIS body's positions (fixes the Mixamo Y-down-bone vs UE X-down-bone convention);
//! (2) LIMB-ALIGN each limb bone (arms/legs/feet) — rotate its frame by the minimal rotation taking
//! the reference's limb direction onto THIS body's, so its axis points down this body's own limb and
//! the shared clips' absolute rotations land where they should. Torso bones (pelvis/spine/clavicle/
//! neck/head) are NEVER limb-aligned (the pelvis→child tilt trap) — they keep the reference frame.
//! No `retarget_rot = t·s⁻¹` — absolute retarget only (03BBF8F4). Follow-on: infer + hip-width.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};

use crate::fbx::{RawBone, RawModel};

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
/// absent — deliberately NOT limb-aligned.
fn limb_child(name: &str) -> Option<&'static str> {
    Some(match name {
        "upperarm_l" => "lowerarm_l",
        "lowerarm_l" => "hand_l",
        "upperarm_r" => "lowerarm_r",
        "lowerarm_r" => "hand_r",
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
fn model_world_frames(model: &RawModel) -> Vec<Mat4> {
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
fn write_world_frames(bones: &mut [RawBone], w: &[Mat4]) {
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
        let (Some(&this_ch), Some(&ref_bone), Some(&ref_ch)) =
            (idx.get(ch), cidx.get(&b.name), cidx.get(ch))
        else {
            continue;
        };
        let u = (pos_of(g0[this_ch]) - pos_of(g0[i])).normalize_or_zero();
        let v = (pos_of(cg[ref_ch]) - pos_of(cg[ref_bone])).normalize_or_zero();
        if u.length_squared() < 1e-10 || v.length_squared() < 1e-10 {
            continue;
        }
        let align = Quat::from_rotation_arc(v, u); // minimal rotation v → u
        t[i] = rotated_frame(align, g0[i], pos_of(g0[i]));
        report.limbs_aligned += 1;
    }

    // 3. derive local + inverse_bind from the final frames T (absolute retarget; no retarget_rot).
    write_world_frames(&mut model.bones, &t);
    Ok(report)
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
    pub hand_scale_l: f32,
    pub hand_scale_r: f32,
}

/// Add the canonical bones Meshy never produces — the in-app port of
/// `rename_meshy_to_canonical.py::infer_canonical_bones`. Whatever the reference (PrismHumanBaseA)
/// has and this body lacks (30 fingers + 8 twists + 2 weapon sockets + jaw/eyes) is inferred from the
/// reference's own local offset, hung off THIS body's already-canonical parent frame and scaled by
/// this body's limb-length ratio.
///
/// Runs AFTER [`reorient_to_canonical`], and that ordering is what makes it correct: every parent
/// frame is already canonical, so a twist lands at the same FRACTION along this body's limb, and a
/// finger chain off the (deliberately un-limb-aligned) hand reproduces the reference hand orientation.
/// SCALE: Meshy gives no hand length, so fingers + sockets are sized by the FOREARM ratio ("a
/// straight hand of standard proportion"). Inferred bones carry NO weights — they are appended after
/// the mesh's joint indices are baked, so no vertex references them; they resolve and rotate but
/// deform nothing until each body's hand mesh is weighted to them (the follow-on).
pub fn infer_canonical_bones(model: &mut RawModel, reference: &Path) -> Result<InferReport> {
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

    // Append each inferred bone: parent frame is already canonical, so W = parent_global · scaled_local.
    let mut g = g0;
    let mut idx = idx0;
    let mut report = InferReport {
        added: Vec::new(),
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
        let w = g[pidx] * new_l;
        let (sc, r, t) = new_l.to_scale_rotation_translation();
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
        report.added.push(b.name.clone());
    }
    Ok(report)
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
pub fn conform_to_canonical(model: &mut RawModel, reference: &Path) -> Result<ConformOutput> {
    let hip = derive_hip_placement(model);
    let shoulder = derive_shoulder_placement(model);
    let ankle = derive_ankle_placement(model);
    let reorient = reorient_to_canonical(model, reference)?;
    let infer = infer_canonical_bones(model, reference)?;
    Ok(ConformOutput {
        hip,
        shoulder,
        ankle,
        reorient,
        infer,
    })
}

/// The canonical reference rig — **GolemBaseSkeleton**, the AUTHORED baseline
/// (Aaron's ruling, 2026-08-04): a generated, skeleton-only A-pose at 170 cm — see
/// [`crate::baseline`]. The reference is nobody's body: characters (the golem
/// included) are conformed ONTO it, which retires the Katanami-derived bind lineage
/// for good. "Canonical" = the 66 names + parent topology + conventions (Z-up, cm,
/// root at feet) + THIS authored rest bind; body proportions stay per-rig by design.
pub fn default_reference() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content/package/characters/GolemBaseSkeleton/GolemBaseSkeleton.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::parse_fbx;
    use crate::rig::rename_to_canonical;
    use std::collections::HashSet;

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
        let report = infer_canonical_bones(&mut model, &reference).unwrap();
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
        conform_to_canonical(&mut model, &reference).unwrap();

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
        eprintln!("oracle match: worst position {worst_pos:.4} cm ({worst_pos_name}), worst orientation {worst_deg:.4}° ({worst_deg_name}); {} shared bones", model.bones.len());
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
        eprintln!("low-res human RAW z: pelvis {:.1}, head {:.1}, foot_l {:.1}  (oracle 95.6 / 153.2 / 9.9)", rz("pelvis"), rz("head"), rz("foot_l"));

        conform_to_canonical(&mut model, &reference).unwrap();

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
            eprintln!("  pelvis bone z {pbz:.1}  | pelvis-flesh z [{pfmin:.1}..{pfmax:.1}] (n={pn}) → bone {}",
                if pbz >= pfmin && pbz <= pfmax { "INSIDE flesh (genuine)" } else { "OUTSIDE flesh (floats)" });
            eprintln!("  thigh_l bone z {tbz:.1} | thigh-flesh z [{tfmin:.1}..{tfmax:.1}] (n={tn}) → femoral head at flesh-top? gap {:.1}", tfmax - tbz);
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
                eprintln!("      flesh n={n} centroid [{:6.2} {:6.2} {:6.2}] x[{:.1}..{:.1}] y[{:.1}..{:.1}] z[{:.1}..{:.1}]",
                    c.x, c.y, c.z, lo.x, hi.x, lo.y, hi.y, lo.z, hi.z);
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
            eprintln!("  {side}: mid(spine_03.x)={mid:.2}, widest shoulder flesh {widest:.2} cm from midline; upperarm now at {:.2}", pos_of(w[bidx[uarm]]).x);
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
            infer_canonical_bones(&mut model, &reference).unwrap();
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
            crate::bake::write_rig(&model, &fbx, "HumanBaseA", &skel, &[]).unwrap();
            let (inplace, _) =
                crate::retarget::emit_variants(&idle_bvh, &skel, &tmp.join("c")).unwrap();
            let (g, idx) = posed_world(&inplace);
            let p = |n: &str| pos_of(g[idx[n]]);
            let (h, _th) = (p("hand_l"), p("thigh_l"));
            // Posed shoulder height (does widening amplify the idle shoulder-drop? rest upperarm z≈137.5).
            let (clav_z, ua_z) = (p("clavicle_l").z, p("upperarm_l").z);
            eprintln!(
                "[{tag:9}] upperarm rest x={ua_rest:5.2} | IDLE hand_l x={:6.2} clear {:+5.2}cm | posed clav_z={clav_z:6.2} upperarm_z={ua_z:6.2} (rest 137.5, drop {:+.2})",
                h.x, h.x - HIP_EDGE_X, ua_z - 137.55
            );
        }
        eprintln!("(hip flesh outer edge at hand height ≈ x 17.1; positive clearance = hand clears the hip)");
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
}
