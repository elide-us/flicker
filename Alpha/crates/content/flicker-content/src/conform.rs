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
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for v in &model.vertices {
        let p = Vec3::from_array(v.p);
        lo = lo.min(p);
        hi = hi.max(p);
    }
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
    ScaleReport {
        scale: s,
        source_height: height,
        stature: stature_cm,
    }
}

/// Install the authored canonical skeleton onto a mesh that has NONE — the raw-mesh rig path
/// (Aaron 2026-08-22). The bones are `baseline::golem_base_skeleton()` **uniformly scaled to
/// `stature_cm`** and planted at the canonical origin (feet on the ground, plumb), so the bind
/// IS the authored canon by construction (invariant BIND == AUTHORED CANON) — no mesh-fitting,
/// no `pose_mesh_to_canon` transport (that mesh-warp path was rolled back).
///
/// Emits the 66-bone `RawModel` convention conform produces (root EXCLUDED — `bake_rig`
/// synthesizes it and shifts +1; `pelvis`'s parent is `-1`). Pair with a prior
/// [`scale_mesh_to_stature`] at the same stature so mesh and skeleton co-locate, then
/// [`crate::bake::bake_skin`] derives weights from these bones.
pub fn install_baseline_skeleton(model: &mut RawModel, stature_cm: f32) {
    let s = stature_cm / crate::baseline::STATURE;
    let pos = crate::baseline::world_positions();
    // Output index per bone name, root excluded, in canonical hierarchy order.
    let mut out_index: HashMap<&str, usize> = HashMap::new();
    let mut i = 0usize;
    for (name, _) in crate::baseline::TOPOLOGY.iter() {
        if *name == "root" {
            continue;
        }
        out_index.insert(*name, i);
        i += 1;
    }
    let world = |name: &str| -> Vec3 { s * pos[name] };
    let bones: Vec<RawBone> = crate::baseline::TOPOLOGY
        .iter()
        .filter(|(name, _)| *name != "root")
        .map(|(name, parent)| {
            let w = world(name);
            let (parent_idx, parent_world) = if *parent == "-" || *parent == "root" {
                (-1, Vec3::ZERO)
            } else {
                (out_index[*parent] as i32, world(parent))
            };
            let local_t = w - parent_world;
            RawBone {
                name: (*name).to_string(),
                parent: parent_idx,
                translation: local_t.to_array(),
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                inverse_bind: Mat4::from_translation(-w).to_cols_array(),
            }
        })
        .collect();
    model.bones = bones;
}

/// ROUGH-FIT the installed canon to a raw mesh's own geometry — the raw-mesh rig starting template
/// (Aaron 2026-08-22, "rough auto-template + manual"). A rig-less mesh has no joint positions, so the
/// bare canon lands generic (stocky shoulders, a wide bird A-pose that fits nothing). This measures
/// the mesh and pulls the LIMB joints onto it, so the human's follow-up joint-drag is a nudge, not a
/// rebuild. It NEVER touches the torso chain (pelvis/spine/neck/head stay plumb) — posture is the
/// clip's job, and bending the body is the logged fundamental dead-end.
///
/// Fits the ARMS (weightless, from mesh geometry): shoulder width at `SHOULDER_FRACTION` of the
/// shoulder-band flesh, and the whole arm chain remapped by a similarity (scale+rotate in XZ) from
/// the canon shoulder→hand onto the fitted shoulder → the mesh's widest vertex (the A-posed hand).
/// The LEGS are NOT fitted here — a Z-band at hip height catches the A-posed hands, so the hip is
/// fitted by weight OWNERSHIP with [`derive_hip_placement`] after a rough skin (the caller's job).
/// Requires the mesh already scaled to `stature_cm` (grounded, x-centred) — call after
/// [`scale_mesh_to_stature`].
pub fn fit_baseline_to_mesh(model: &mut RawModel, stature_cm: f32) {
    install_baseline_skeleton(model, stature_cm);
    if model.vertices.len() < 4 || stature_cm <= 0.0 {
        return;
    }
    let h = stature_cm;
    // The mesh's half-width (max |x|) among vertices in a Z band — the flesh extent at that height.
    let band = |z0: f32, z1: f32| -> f32 {
        model
            .vertices
            .iter()
            .filter(|v| v.p[2] >= z0 && v.p[2] <= z1)
            .map(|v| v.p[0].abs())
            .fold(0.0_f32, f32::max)
    };
    let sh_w = band(0.78 * h, 0.86 * h);
    // The widest vertex overall is the A-posed hand/fingertip — the arm's reach target.
    let (mut hand_x, mut hand_z) = (0.0_f32, 0.5 * h);
    for v in &model.vertices {
        if v.p[0].abs() > hand_x {
            hand_x = v.p[0].abs();
            hand_z = v.p[2];
        }
    }

    let idx: HashMap<String, usize> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.clone(), i))
        .collect();
    let mut w = model_world_frames(model);

    // Bone indices of `root_name` and every descendant, by walking the parent table.
    let subtree = |root_name: &str| -> Vec<usize> {
        let Some(&r) = idx.get(root_name) else {
            return Vec::new();
        };
        let mut out = vec![r];
        let mut changed = true;
        while changed {
            changed = false;
            for (i, b) in model.bones.iter().enumerate() {
                if b.parent >= 0 && out.contains(&(b.parent as usize)) && !out.contains(&i) {
                    out.push(i);
                    changed = true;
                }
            }
        }
        out
    };

    for (side, sign) in [("l", 1.0_f32), ("r", -1.0_f32)] {
        // NB: the LEGS are deliberately NOT fitted here. A bare Z-band at hip height catches the
        // A-posed HANDS (they hang at hip height), which shoved the legs out to the wrists. The hip
        // is fitted by weight OWNERSHIP (`derive_hip_placement`) after a rough skin instead.
        //
        // ARM: map the canon (shoulder→hand) segment onto (fitted shoulder → the mesh hand) as a
        // similarity in the XZ plane, applied to the whole arm subtree — so the arm points down the
        // mesh's actual arm and reaches its hand instead of flailing past it.
        let (Some(&ua), Some(&hd)) = (
            idx.get(&format!("upperarm_{side}")),
            idx.get(&format!("hand_{side}")),
        ) else {
            continue;
        };
        let (sx0, sz0) = (w[ua].w_axis.x, w[ua].w_axis.z); // canon shoulder
        let (hx0, hz0) = (w[hd].w_axis.x, w[hd].w_axis.z); // canon hand
        let sx1 = if sh_w > 1.0 {
            sign * SHOULDER_FRACTION * sh_w
        } else {
            sx0
        };
        let (hx1, hz1) = (sign * hand_x, hand_z); // the mesh's hand
        let (ax, az) = (hx0 - sx0, hz0 - sz0);
        let (bx, bz) = (hx1 - sx1, hz1 - sz0);
        let (alen, blen) = ((ax * ax + az * az).sqrt(), (bx * bx + bz * bz).sqrt());
        if alen < 1e-3 || blen < 1e-3 {
            continue;
        }
        let scale = blen / alen;
        let dtheta = bz.atan2(bx) - az.atan2(ax);
        let (c, s) = (dtheta.cos() * scale, dtheta.sin() * scale);
        for bi in subtree(&format!("upperarm_{side}")) {
            let (px, pz) = (w[bi].w_axis.x - sx0, w[bi].w_axis.z - sz0);
            w[bi].w_axis.x = sx1 + c * px - s * pz;
            w[bi].w_axis.z = sz0 + s * px + c * pz;
        }
    }

    write_world_frames(&mut model.bones, &w);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::{parse_fbx, RawBone, RawVertex};
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

    /// A closed box mesh (non-deduped, one vertex per corner) — a stand-in raw character mesh.
    fn box_mesh(x0: f32, x1: f32, y0: f32, y1: f32, z0: f32, z1: f32) -> RawModel {
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ];
        let faces = [
            ([0usize, 1, 2, 3], [0.0, 0.0, -1.0]),
            ([4, 7, 6, 5], [0.0, 0.0, 1.0]),
            ([0, 4, 5, 1], [0.0, -1.0, 0.0]),
            ([3, 2, 6, 7], [0.0, 1.0, 0.0]),
            ([0, 3, 7, 4], [-1.0, 0.0, 0.0]),
            ([1, 5, 6, 2], [1.0, 0.0, 0.0]),
        ];
        let mut verts = Vec::new();
        for (q, n) in faces {
            for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
                for &vi in &tri {
                    verts.push(RawVertex {
                        p: c[vi],
                        n,
                        uv: [0.0, 0.0],
                        joints: [0; 4],
                        weights: [0.0; 4],
                    });
                }
            }
        }
        let indices = (0..verts.len() as u32).collect();
        RawModel {
            vertices: verts,
            indices,
            bones: Vec::new(),
        }
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

    #[test]
    fn fit_baseline_pulls_limbs_onto_the_mesh() {
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
            vertices: verts,
            indices,
            bones: Vec::new(),
        };
        fit_baseline_to_mesh(&mut m, h);

        let w = model_world_frames(&m);
        let idx: HashMap<&str, usize> = m
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let wx = |n: &str| w[idx[n]].w_axis.x;
        let wz = |n: &str| w[idx[n]].w_axis.z;

        // fit_baseline_to_mesh does NOT touch the legs (the weight-based derive_hip does, after
        // skinning — a band here would catch the A-posed hands). Thighs stay at the canon width.
        let canon_thigh = 0.051 * h; // hip_x fraction, baseline.rs
        assert!(
            (wx("thigh_l") - canon_thigh).abs() < 1.0,
            "thigh_l should stay canon ({canon_thigh}), got {}",
            wx("thigh_l")
        );
        // Shoulder at SHOULDER_FRACTION of the shoulder flesh (0.70 · 15 = 10.5), NOT the wide canon.
        assert!(
            (wx("upperarm_l") - SHOULDER_FRACTION * 15.0).abs() < 1.0,
            "upperarm_l x = {}",
            wx("upperarm_l")
        );
        // The hand reaches the mesh's widest vertex (±25 at z = 0.45·170), not the canon's reach.
        assert!(
            (wx("hand_l") - 25.0).abs() < 1.5,
            "hand_l x = {}",
            wx("hand_l")
        );
        assert!(
            (wz("hand_l") - 0.45 * h).abs() < 3.0,
            "hand_l z = {}",
            wz("hand_l")
        );
        // The torso is NEVER touched — spine and head stay on the plumb line.
        assert!(
            wx("spine_02").abs() < 1e-3 && wx("head").abs() < 1e-3,
            "torso pulled off plumb"
        );
        // Mirror symmetry holds on the right side.
        assert!(
            (wx("hand_r") + 25.0).abs() < 1.5,
            "hand_r x = {}",
            wx("hand_r")
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
}
