//! In-app clip retargeting onto a `flicker.rig` skeleton — the Rust port of `tools/retarget_bvh.py`
//! (WS-F, no external tools). ONE rest-rebase, fed by two kinds of [`Source`]:
//!
//! - a Motifect **BVH** (Y-up T-pose, converted here), and
//! - an existing **`flicker.rig` clip document** authored on another skeleton — the recovered
//!   Katanami library (Z-up, its own rest, canon bone names). Its motion is read as each joint's
//!   global rotation RELATIVE TO ITS OWN REST, which is exactly what a BVH's FK already is (a BVH
//!   rests at identity), so the same algebra replays it.
//!
//! Either way the motion is replayed FROM the TARGET rig's actual bind (`Ta_b = Sa_b · inv(Sm_b) ·
//! A_b`, A_b = the target's bind global rotation), so re-baking onto a different bind (e.g.
//! HumanBaseA's flat foot instead of PrismHumanBaseA's heeled one) is just a change of target
//! skeleton. Emits both the root-motion and in-place variants at the 60 Hz canon.
//!
//! Two transfer semantics, chosen per bone by [`dir_child`]: LIMBS transfer by the direction they
//! point (`Sm_b` reconciles the source rest direction onto ours), the TORSO chain transfers its
//! rotation DELTA from the source rest (`Sm_b = identity`) — a spine segment's absolute direction
//! is an artefact of where the vendor placed its joints, not a pose.
//!
//! The quaternion algebra mirrors `flicker_rebase.py` (memory 614E5958): glam `a * b` == the Python
//! `qmul(a,b)` ("apply b then a"); `Quat::from_rotation_arc(u,v)` == `q_between(u,v)`; the Y-up→Z-up
//! convert is the similarity `C · q · inv(C)` with `C = Rx(+90°)`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use glam::{Mat4, Quat, Vec3};
use serde_json::{json, Value};

use crate::bvh::parse_bvh;

const CANON_FPS: u32 = 60; // golden-spec 60 Hz output canon (memory 302BBB85)

/// `C = Rx(+90°)` — the Motifect Y-up → engine Z-up basis change.
fn c_yup_to_zup() -> Quat {
    Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)
}

/// Similarity transform of a source-space GLOBAL rotation into Z-up: `C · q · inv(C)`.
fn convert_global(q: Quat) -> Quat {
    let c = c_yup_to_zup();
    c * q * c.inverse()
}

/// Target canonical bone → the BVH SOURCE joint whose GLOBAL rotation drives it (spec §C.1). Bones
/// absent here (twists, weapon sockets, root) get no track — left at rest.
fn name_map() -> HashMap<String, String> {
    let mut m: HashMap<String, String> = HashMap::new();
    let base = [
        ("pelvis", "Hips"),
        ("spine_01", "Spine1"),
        ("spine_02", "Spine2"),
        ("spine_03", "Chest"),
        ("neck_01", "Neck2"), // Neck2's global already contains Neck1 → composes Neck1·Neck2
        ("head", "Head"),
        ("jaw", "Jaw"),
        ("eye_l", "LeftEye"),
        ("eye_r", "RightEye"),
    ];
    for (t, s) in base {
        m.insert(t.into(), s.into());
    }
    let fingers = [
        ("thumb", "Thumb"),
        ("index", "Index"),
        ("middle", "Middle"),
        ("ring", "Ring"),
        ("pinky", "Pinky"),
    ];
    for (side_t, side_s) in [("l", "Left"), ("r", "Right")] {
        m.insert(format!("clavicle_{side_t}"), format!("{side_s}Shoulder"));
        m.insert(format!("upperarm_{side_t}"), format!("{side_s}Arm"));
        m.insert(format!("lowerarm_{side_t}"), format!("{side_s}ForeArm"));
        m.insert(format!("hand_{side_t}"), format!("{side_s}Hand"));
        m.insert(format!("thigh_{side_t}"), format!("{side_s}Leg"));
        m.insert(format!("calf_{side_t}"), format!("{side_s}Shin"));
        m.insert(format!("foot_{side_t}"), format!("{side_s}Foot"));
        m.insert(format!("ball_{side_t}"), format!("{side_s}ToeBase"));
        for (ft, fs) in fingers {
            for n in 1..=3 {
                m.insert(
                    format!("{ft}_{n:02}_{side_t}"),
                    format!("{side_s}Hand{fs}{n}"),
                );
            }
        }
    }
    m
}

/// LIMB bone → the child bone whose rest offset defines this bone's forward DIRECTION (spec §C.2):
/// a limb transfers by where it POINTS, so the source's rest direction is reconciled onto ours.
///
/// The torso chain (pelvis / spine / clavicles / neck / head) is deliberately ABSENT, so it
/// transfers ROTATION DELTAS from the source rest instead (`Sm = identity`). Both rests stand
/// straight, but the Motifect spine joints zig-zag through the body (Chest leans back, Neck2 and
/// Head lean 17° forward) while the canon stacks them plumb — direction-matching those segments
/// transcribed the vendor's joint LAYOUT as a permanent bend on every clip (spine_02 −15°,
/// spine_03 +18°, neck +19° with the actor standing straight; the 2026-08-21 slouch). Leaves and
/// the torso (no entry) inherit their parent's reconciliation, which for the torso is identity.
fn dir_child() -> HashMap<String, String> {
    let mut m: HashMap<String, String> = HashMap::new();
    let base = [
        ("thigh_l", "calf_l"),
        ("calf_l", "foot_l"),
        ("foot_l", "ball_l"),
        ("thigh_r", "calf_r"),
        ("calf_r", "foot_r"),
        ("foot_r", "ball_r"),
    ];
    for (a, b) in base {
        m.insert(a.into(), b.into());
    }
    for s in ["l", "r"] {
        m.insert(format!("upperarm_{s}"), format!("lowerarm_{s}"));
        m.insert(format!("lowerarm_{s}"), format!("hand_{s}"));
        m.insert(format!("hand_{s}"), format!("middle_01_{s}"));
        for f in ["thumb", "index", "middle", "ring", "pinky"] {
            m.insert(format!("{f}_01_{s}"), format!("{f}_02_{s}"));
            m.insert(format!("{f}_02_{s}"), format!("{f}_03_{s}"));
        }
    }
    m
}

/// [`dir_child`] less the FOOT CHAIN — the bones that transfer by direction from any source, as
/// the runtime's `flicker_skeletal::format::limb_direction_child` names them (the ONE table:
/// what bakes by direction is what the load-time limb rebase re-aims). The feet transfer by
/// delta so a target keeps its own stance (see `Source::dir_child`).
fn limb_dirs() -> HashMap<String, String> {
    dir_child()
        .into_iter()
        .filter(|(b, _)| flicker_skeletal::format::limb_direction_child(b).is_some())
        .collect()
}

/// A `flicker.rig` bone `local` (contract-space, column-major row-vector floats — read as columns,
/// never transposed, exactly as `flicker_skeletal::format` does) → its rest translation + rotation.
fn decode_local(local: &Value) -> Option<(Vec3, Quat)> {
    let arr = local.as_array()?;
    let mut m = [0.0f32; 16];
    for (i, f) in arr.iter().enumerate().take(16) {
        m[i] = f.as_f64().unwrap_or(0.0) as f32;
    }
    let mat = Mat4::from_cols_array(&m);
    Some((mat.w_axis.truncate(), mat.to_scale_rotation_translation().1))
}

/// The target skeleton, decomposed for the retarget: bind global rotations (`A_b`), rest offsets,
/// and the bind world direction to each bone's `DIR_CHILD`. `bones_json` is kept verbatim to embed.
struct Target {
    bones_json: Value,
    names: Vec<String>,
    idx: HashMap<String, usize>,
    parent: Vec<i32>,
    rest_transl: Vec<Vec3>,
    /// Each bone's LOCAL rest rotation — what an undriven bone plays at runtime.
    rest_rot: Vec<Quat>,
    bind_global: Vec<Quat>,
    bind_dir: HashMap<String, Vec3>,
}

/// Load a `flicker.rig` skeleton as a retarget [`Target`]: FK the bones' own local rotations to the
/// bind global (`A_b`) and derive each bone's bind-pose direction to its `DIR_CHILD`.
fn load_target(skeleton: &Path) -> Result<Target> {
    // Gz-transparent read: the target rig is PROCESSED content (gz at rest).
    let text = crate::package::read_text(skeleton)
        .with_context(|| format!("reading target {}", skeleton.display()))?;
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing target {}", skeleton.display()))?;
    let bones_json = v["skeleton"]["bones"].clone();
    let bones = bones_json
        .as_array()
        .context("target has no skeleton.bones")?;

    let (mut names, mut parent, mut rest_transl, mut loc_r) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for b in bones {
        names.push(b["name"].as_str().context("bone.name")?.to_string());
        parent.push(b["parent"].as_i64().context("bone.parent")? as i32);
        let (t, r) = decode_local(&b["local"]).context("bone.local")?;
        rest_transl.push(t);
        loc_r.push(r);
    }
    let idx: HashMap<String, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();

    // A_b = FK of the skeleton's OWN local rotations (the pose the mesh is skinned in).
    let mut bind_global = vec![Quat::IDENTITY; names.len()];
    for i in 0..names.len() {
        bind_global[i] = if parent[i] < 0 {
            loc_r[i]
        } else {
            bind_global[parent[i] as usize] * loc_r[i]
        };
    }

    // bind world direction to each bone's DIR_CHILD = A_b · (child's rest offset, normalised).
    let dc = dir_child();
    let mut bind_dir = HashMap::new();
    for (i, nm) in names.iter().enumerate() {
        if let Some(c) = dc.get(nm).and_then(|c| idx.get(c)) {
            let off = rest_transl[*c];
            if off.length() > 1e-9 {
                bind_dir.insert(nm.clone(), bind_global[i] * off.normalize());
            }
        }
    }

    Ok(Target {
        bones_json,
        names,
        idx,
        parent,
        rest_transl,
        rest_rot: loc_r,
        bind_global,
        bind_dir,
    })
}

/// One clip's motion in the shape the rest-rebase consumes, whatever it was parsed from. `Sa` is a
/// source joint's GLOBAL rotation relative to the SOURCE REST, in Z-up: a BVH rests at identity so
/// it is the converted FK; a rig document rests at `R_b`, so it is `G_b(t) · inv(R_b)`.
struct Source {
    /// The clip name / emitted file stem, and the source file name recorded in the output.
    stem: String,
    file: String,
    /// The `source.applied_transform` provenance written into the clip.
    applied: &'static str,
    native_hz: u32,
    /// Target bone → the source joint whose motion drives it: spec §C.1 for a BVH; identity by
    /// NAME for a rig document, `root` excluded (clips resolve by bone name — the whole contract).
    nmap: HashMap<String, String>,
    /// Source joint → Z-up world direction of the rest segment that ENDS at it (parent → joint).
    rest_dir: HashMap<String, Vec3>,
    /// Which target bones transfer by DIRECTION for this source (their `DIR_CHILD`); the rest
    /// transfer rotation deltas. Limbs above the ankle transfer by direction from EITHER source
    /// (a T-pose calibration or an authored bind both tell where an arm or a shin points). The
    /// FOOT CHAIN transfers by DELTA from both (`limb_dirs`): a source's foot pitch is its own
    /// stance — Katanami's heels, a mocap's flat sole — never the target's, so the target keeps
    /// its own foot (a lizardman's raised heel, the canon's flat sole) and the source's ANIMATED
    /// foot pitch rides on it as a delta. Direction-matching the foot pitched a toe walker's
    /// 30°-steep foot flat and lifted its toes 13 cm (Aaron 2026-09-07).
    dir_child: HashMap<String, String>,
    /// Per frame: `Sa` by source joint, plus the hip position in SOURCE space.
    frames: Vec<(HashMap<String, Quat>, Vec3)>,
    /// The hip position travel is measured FROM: the first frame for a BVH; for a rig document
    /// the first frame planar but the REST height vertically, so a clip that starts crouched
    /// starts low instead of at the target's standing hip height.
    hip_ref: Vec3,
    /// Source space → Z-up, applied to hip travel (`C` for a BVH, identity for a Z-up rig).
    to_zup: Quat,
    /// Standing hip height in source space, for scaling travel onto the target's hip height.
    hip_h: f32,
}

impl Source {
    /// Parse a clip source by its file kind: `.bvh`, or a `flicker.rig` `.json` / `.json.gz`.
    fn load(path: &Path) -> Result<Source> {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.ends_with(".bvh") {
            Source::from_bvh(path)
        } else if name.ends_with(".json") || name.ends_with(".json.gz") {
            Source::from_rig(path)
        } else {
            bail!(
                "unsupported clip source {} (want a .bvh or a flicker.rig .json / .json.gz)",
                path.display()
            )
        }
    }

    /// A Motifect BVH: Y-up, T-posed at identity, so `Sa` is the converted FK and a joint's rest
    /// direction is just its OFFSET (Y-up → Z-up).
    fn from_bvh(path: &Path) -> Result<Source> {
        let bvh = parse_bvh(path)?;
        if bvh.frames.is_empty() {
            bail!("BVH {} has no motion frames", path.display());
        }
        let c = c_yup_to_zup();
        let rest_dir = bvh
            .joints
            .iter()
            .filter_map(|j| {
                let off = Vec3::from(j.offset);
                (off.length() > 1e-9).then(|| (j.name.clone(), (c * off.normalize()).normalize()))
            })
            .collect();
        let frames: Vec<(HashMap<String, Quat>, Vec3)> = bvh
            .frames
            .iter()
            .map(|frame| {
                let (local_q, root_pos) = bvh.frame_locals(frame);
                let g = bvh.global_rotations(&local_q);
                let sa = bvh
                    .joints
                    .iter()
                    .enumerate()
                    .map(|(i, j)| (j.name.clone(), convert_global(g[i])))
                    .collect();
                (sa, Vec3::from(root_pos))
            })
            .collect();
        let hip_h = frames[0].1.y; // Y-up standing height
        let hip_ref = frames[0].1;
        Ok(Source {
            stem: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("clip")
                .to_string(),
            file: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("clip.bvh")
                .to_string(),
            applied: "bvh-retarget: Y_up->Z_up (Rx+90) + rest-rebase onto target bind",
            native_hz: bvh.fps().round().max(1.0) as u32,
            nmap: name_map(),
            rest_dir,
            dir_child: limb_dirs(),
            frames,
            hip_ref,
            to_zup: c,
            hip_h,
        })
    }

    /// A `flicker.rig` clip document (its FIRST clip), Z-up in its own rest: FK the embedded
    /// skeleton for the rest globals `R_b` and every tick's posed globals `G_b(t)` — keys are
    /// dense, one per tick, clamped past the end (the runtime's rule) — and take `Sa_b(t) =
    /// G_b(t) · inv(R_b)`. The hip position is the pelvis's posed WORLD position, so travel
    /// carried on the source's `root` (the Katanami RootMotion clips) counts as travel.
    fn from_rig(path: &Path) -> Result<Source> {
        let text = crate::package::read_text(path)
            .with_context(|| format!("reading clip source {}", path.display()))?;
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("parsing clip source {}", path.display()))?;
        let bones = v["skeleton"]["bones"]
            .as_array()
            .context("clip source has no skeleton.bones")?;
        let n = bones.len();
        let (mut names, mut parent, mut loc_t, mut loc_r) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        for b in bones {
            names.push(b["name"].as_str().context("bone.name")?.to_string());
            parent.push(b["parent"].as_i64().context("bone.parent")? as i32);
            let (t, r) = decode_local(&b["local"]).context("bone.local")?;
            loc_t.push(t);
            loc_r.push(r);
        }
        let idx: HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(i, nm)| (nm.as_str(), i))
            .collect();
        let pelvis = *idx.get("pelvis").context("clip source has no pelvis")?;

        // Rest FK: global rotation + world position per bone (parents precede children).
        let (mut rest_g, mut rest_p) = (vec![Quat::IDENTITY; n], vec![Vec3::ZERO; n]);
        for i in 0..n {
            match usize::try_from(parent[i]) {
                Ok(p) => {
                    rest_g[i] = rest_g[p] * loc_r[i];
                    rest_p[i] = rest_p[p] + rest_g[p] * loc_t[i];
                }
                Err(_) => {
                    rest_g[i] = loc_r[i];
                    rest_p[i] = loc_t[i];
                }
            }
        }
        let rest_dir: HashMap<String, Vec3> = (0..n)
            .filter_map(|i| {
                let p = usize::try_from(parent[i]).ok()?;
                let d = rest_g[p] * loc_t[i];
                (d.length() > 1e-9).then(|| (names[i].clone(), d.normalize()))
            })
            .collect();

        let clip = v["clips"]
            .as_array()
            .and_then(|c| c.first())
            .context("clip source has no clips")?;
        let native_hz = clip["tick_rate_hz"]
            .as_u64()
            .filter(|&h| h > 0)
            .context("clip.tick_rate_hz")? as u32;
        let duration = clip["duration_ticks"].as_u64().unwrap_or(0) as usize;
        if duration == 0 {
            bail!("clip source {} has no ticks", path.display());
        }
        let mut keys: Vec<Option<Vec<(Vec3, Quat)>>> = vec![None; n];
        for tr in clip["tracks"].as_array().context("clip.tracks")? {
            let Some(&bi) = tr["bone"].as_str().and_then(|b| idx.get(b)) else {
                continue; // a source-only bone (cloth, hair, IK helpers) drives nothing
            };
            let ks: Vec<(Vec3, Quat)> = tr["keys"]
                .as_array()
                .context("track.keys")?
                .iter()
                .map(|k| {
                    let f = |a: &Value, i: usize| a[i].as_f64().unwrap_or(0.0) as f32;
                    let t = Vec3::new(f(&k["T"], 0), f(&k["T"], 1), f(&k["T"], 2));
                    let r =
                        Quat::from_xyzw(f(&k["R"], 0), f(&k["R"], 1), f(&k["R"], 2), f(&k["R"], 3));
                    let r = if r.length_squared() > 1e-8 {
                        r.normalize()
                    } else {
                        Quat::IDENTITY
                    };
                    (t, r)
                })
                .collect();
            if !ks.is_empty() {
                keys[bi] = Some(ks);
            }
        }
        let frames: Vec<(HashMap<String, Quat>, Vec3)> = (0..duration)
            .map(|t| {
                let (mut g, mut p) = (vec![Quat::IDENTITY; n], vec![Vec3::ZERO; n]);
                for i in 0..n {
                    let (lt, lr) = keys[i]
                        .as_ref()
                        .map_or((loc_t[i], loc_r[i]), |k| k[t.min(k.len() - 1)]);
                    match usize::try_from(parent[i]) {
                        Ok(pp) => {
                            g[i] = g[pp] * lr;
                            p[i] = p[pp] + g[pp] * lt;
                        }
                        Err(_) => {
                            g[i] = lr;
                            p[i] = lt;
                        }
                    }
                }
                let sa = names
                    .iter()
                    .enumerate()
                    .map(|(i, nm)| (nm.clone(), g[i] * rest_g[i].inverse()))
                    .collect();
                (sa, p[pelvis])
            })
            .collect();

        let hip_ref = Vec3::new(frames[0].1.x, frames[0].1.y, rest_p[pelvis].z);
        let file = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("clip.json")
            .to_string();
        let stem = file
            .strip_suffix(".json.gz")
            .or_else(|| file.strip_suffix(".json"))
            .unwrap_or(&file)
            .to_string();
        Ok(Source {
            stem,
            file,
            applied: "rig-retarget: rest-rebase onto target bind (Z_up flicker.rig clip source)",
            native_hz,
            nmap: names
                .iter()
                .filter(|nm| nm.as_str() != "root")
                .map(|nm| (nm.clone(), nm.clone()))
                .collect(),
            rest_dir,
            dir_child: limb_dirs(),
            frames,
            hip_ref,
            to_zup: Quat::IDENTITY,
            hip_h: rest_p[pelvis].z,
        })
    }
}

/// The source base pose `Sm_b`: the source rig posed so each bone points along OUR bind direction
/// (spec §C.2) — the minimal rotation taking the source's rest direction of the mapped
/// `DIR_CHILD` segment onto ours. Leaves and the torso (no `DIR_CHILD`) inherit their parent's.
fn source_base_pose(src: &Source, target: &Target) -> HashMap<String, Quat> {
    let dc = &src.dir_child;
    let mut sm: HashMap<String, Quat> = HashMap::new();
    for (i, nm) in target.names.iter().enumerate() {
        let resolved = (|| {
            src.nmap.get(nm)?;
            let bd = target.bind_dir.get(nm)?;
            let s_child = src.nmap.get(dc.get(nm)?)?;
            let s_dir = src.rest_dir.get(s_child)?;
            Some(Quat::from_rotation_arc(*s_dir, bd.normalize()))
        })();
        let q = resolved.unwrap_or_else(|| {
            usize::try_from(target.parent[i])
                .ok()
                .and_then(|p| sm.get(&target.names[p]).copied())
                .unwrap_or(Quat::IDENTITY)
        });
        sm.insert(nm.clone(), q);
    }
    sm
}

/// One frame's rest-rebase → target bone → LOCAL rotation quat. `Ta_b = Sa_map(b) · inv(Sm_b) · A_b`
/// for a bone the source drives; `local_b = inv(Ta_parent) · Ta_b`. Driven bones only get a track.
///
/// A bone the source does NOT drive keeps its REST local at playback, so the global it actually
/// plays in is its parent's POSED global × that local — never its bind global. Rebasing a driven
/// child against the bind instead applies the parent's rotation to the child twice: the canon's
/// `neck_02` sits undriven between `neck_01` and `head`, and the head took the neck's rotation
/// twice (10.7° → 29.4° forward on the idle; the measured 2026-08-21 head jut).
fn rebase_frame(
    sa_zup: &HashMap<String, Quat>,
    sm: &HashMap<String, Quat>,
    target: &Target,
    nmap: &HashMap<String, String>,
) -> HashMap<String, Quat> {
    let n = target.names.len();
    let mut ta = vec![Quat::IDENTITY; n];
    let mut driven = vec![false; n];
    for (i, nm) in target.names.iter().enumerate() {
        let p = target.parent[i];
        ta[i] = match nmap.get(nm).and_then(|s| sa_zup.get(s)) {
            Some(&sa) => {
                driven[i] = true;
                sa * sm.get(nm).copied().unwrap_or(Quat::IDENTITY).inverse() * target.bind_global[i]
            }
            None if p >= 0 => ta[p as usize] * target.rest_rot[i],
            None => target.rest_rot[i],
        };
    }
    let mut out = HashMap::new();
    for (i, nm) in target.names.iter().enumerate() {
        if !driven[i] {
            continue;
        }
        let p = target.parent[i];
        let pg = if p >= 0 {
            ta[p as usize]
        } else {
            Quat::IDENTITY
        };
        out.insert(nm.clone(), (pg.inverse() * ta[i]).normalize());
    }
    out
}

/// One keyframe of a clip track (Z-up / cm).
#[derive(Clone)]
struct Key {
    t: u32,
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

/// Per-bone clip tracks (bone name → its keyframes), in target-bone order.
type Tracks = Vec<(String, Vec<Key>)>;

/// The ROOT-MOTION retarget of one source onto `target`, at the source's native rate: per-bone
/// tracks in target-bone order. The pelvis carries the source's hip travel, scaled by the ratio of
/// standing hip heights; every other driven bone keeps the target's rest offset.
fn retarget_root_motion(src: &Source, target: &Target) -> Result<Tracks> {
    let sm = source_base_pose(src, target);

    let pelvis_i = *target.idx.get("pelvis").context("target has no pelvis")?;
    let hip0 = src.hip_ref;
    let tgt_hip_h = target.rest_transl[pelvis_i].z; // Z-up pelvis rest height
    let prop = if src.hip_h.abs() > 1e-6 {
        tgt_hip_h / src.hip_h
    } else {
        1.0
    };
    let pelvis_rest = target.rest_transl[pelvis_i];

    let mut tracks: HashMap<String, Vec<Key>> = HashMap::new();
    for (t, (sa_zup, hip)) in src.frames.iter().enumerate() {
        let locals_t = rebase_frame(sa_zup, &sm, target, &src.nmap);

        let delta = (src.to_zup * (*hip - hip0)) * prop;
        let pelvis_t = pelvis_rest + delta;

        for (nm, q) in &locals_t {
            let tr = if nm == "pelvis" {
                pelvis_t
            } else {
                target.rest_transl[target.idx[nm]]
            };
            tracks.entry(nm.clone()).or_default().push(Key {
                t: t as u32,
                translation: round3(tr, 6),
                rotation: round4([q.x, q.y, q.z, q.w], 8),
                scale: [1.0, 1.0, 1.0],
            });
        }
    }

    // Target-bone order, only mapped + non-empty.
    Ok(target
        .names
        .iter()
        .filter_map(|nm| {
            tracks
                .remove(nm)
                .filter(|k| !k.is_empty())
                .map(|k| (nm.clone(), k))
        })
        .collect())
}

/// Upsample one track's keys by integer `mult`: source frame k → tick k·mult (VERBATIM), with mult−1
/// slerp/lerp in-betweens (preserves per-frame TAE accuracy; only in-betweens are interpolated).
fn resample_keys(keys: &[Key], mult: u32) -> Vec<Key> {
    if mult == 1 || keys.len() < 2 {
        return keys.to_vec();
    }
    let mut out = Vec::new();
    for k in 0..keys.len() - 1 {
        let (a, b) = (&keys[k], &keys[k + 1]);
        out.push(Key {
            t: k as u32 * mult,
            ..a.clone()
        });
        for j in 1..mult {
            let u = j as f32 / mult as f32;
            let ra = Quat::from_xyzw(a.rotation[0], a.rotation[1], a.rotation[2], a.rotation[3]);
            let rb = Quat::from_xyzw(b.rotation[0], b.rotation[1], b.rotation[2], b.rotation[3]);
            let r = ra.slerp(rb, u);
            out.push(Key {
                t: k as u32 * mult + j,
                translation: round3(lerp3(a.translation, b.translation, u), 6),
                rotation: round4([r.x, r.y, r.z, r.w], 8),
                scale: round3(lerp3(a.scale, b.scale, u), 6),
            });
        }
    }
    let last = keys.last().unwrap();
    out.push(Key {
        t: (keys.len() as u32 - 1) * mult,
        ..last.clone()
    });
    out
}

/// Build the `flicker.rig` clip JSON: the target skeleton embedded verbatim + one clip's tracks.
fn build_clip_json(
    target: &Target,
    name: &str,
    tick_rate_hz: u32,
    duration_ticks: u32,
    tracks: &[(String, Vec<Key>)],
    source_file: &str,
    applied: &str,
) -> Value {
    let track_json: Vec<Value> = tracks
        .iter()
        .map(|(bone, keys)| {
            let keys_json: Vec<Value> = keys
                .iter()
                .map(|k| json!({ "t": k.t, "T": k.translation, "R": k.rotation, "S": k.scale }))
                .collect();
            json!({ "bone": bone, "keys": keys_json })
        })
        .collect();
    json!({
        "format": "flicker.rig", "version": 1,
        "source": {
            "file": source_file, "fbx_version": "0",
            "source_axis": "Z_up", "source_unit": "cm",
            "applied_transform": applied,
            "textures": [],
        },
        "retarget": true,
        "skeleton": { "bones": target.bones_json },
        "mesh": { "vertices": [], "indices": [], "submeshes": [], "materials": [] },
        "morphs": [],
        "clips": [{ "name": name, "tick_rate_hz": tick_rate_hz, "duration_ticks": duration_ticks, "tracks": track_json }],
    })
}

/// One source clip retargeted onto a skeleton, BOTH variants built in memory — the seam the
/// Clayworks clip preview samples before anything is committed to disk. Each value is
/// a complete `flicker.rig` clip JSON (target skeleton embedded, 60 Hz canon).
pub struct ClipVariants {
    /// The source's stem — the clip's name, and the emitted file stem.
    pub stem: String,
    /// Pelvis planar (X/Y) translation pinned to rest (treadmill); vertical bob kept.
    pub in_place: Value,
    /// Full planar travel kept.
    pub root_motion: Value,
}

/// Retarget one clip source (a Motifect `.bvh`, or a `flicker.rig` clip `.json` / `.json.gz`
/// authored on another skeleton) onto the `skeleton` rig at the 60 Hz canon and return both
/// variants in memory. [`write_variants`] / [`emit_variants`] are the disk halves.
pub fn build_variants(source: &Path, skeleton: &Path) -> Result<ClipVariants> {
    let src = Source::load(source)?;
    let target = load_target(skeleton)?;
    let tracks_native = retarget_root_motion(&src, &target)?;
    let nframes = src.frames.len() as u32;

    // Resample to the 60 Hz canon (source frames land on even ticks).
    let ratio = CANON_FPS as f32 / src.native_hz as f32;
    let mult = ratio.round() as u32;
    if mult < 1 || (ratio - mult as f32).abs() > 1e-4 {
        bail!(
            "{CANON_FPS} Hz is not an integer multiple of the clip rate {}",
            src.native_hz
        );
    }
    let rm_tracks: Vec<(String, Vec<Key>)> = tracks_native
        .iter()
        .map(|(nm, keys)| (nm.clone(), resample_keys(keys, mult)))
        .collect();
    let duration = if nframes > 0 {
        (nframes - 1) * mult + 1
    } else {
        0
    };

    // In-place: pin the pelvis planar (X/Y) translation to its rest; keep the vertical bob (Z).
    let pelvis_rest = target.rest_transl[target.idx["pelvis"]];
    let mut ip_tracks = rm_tracks.clone();
    for (nm, keys) in ip_tracks.iter_mut() {
        if nm == "pelvis" {
            for k in keys.iter_mut() {
                k.translation[0] = round1(pelvis_rest.x, 6);
                k.translation[1] = round1(pelvis_rest.y, 6);
            }
        }
    }

    let clip = |tracks: &[(String, Vec<Key>)]| {
        build_clip_json(
            &target,
            &src.stem,
            CANON_FPS,
            duration,
            tracks,
            &src.file,
            src.applied,
        )
    };
    Ok(ClipVariants {
        in_place: clip(&ip_tracks),
        root_motion: clip(&rm_tracks),
        stem: src.stem,
    })
}

/// Write the PICKED variants under `out_dir/{In-Place,RootMotion}/<stem>.json` — the
/// bench's Commit passes the user's side-by-side choice (one, the other, or both, per
/// the ruled Clip-role UX). Emits the gz-at-rest form via the shared seam; the returned
/// paths stay LOGICAL (`<stem>.json`) — that is how loaders and callers address clips.
pub fn write_variants(
    v: &ClipVariants,
    out_dir: &Path,
    in_place: bool,
    root_motion: bool,
) -> Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for (on, sub, value) in [
        (in_place, "In-Place", &v.in_place),
        (root_motion, "RootMotion", &v.root_motion),
    ] {
        if !on {
            continue;
        }
        let dir = out_dir.join(sub);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.json", v.stem));
        crate::package::write_text(&path, &serde_json::to_string(value)?)?;
        out.push(path);
    }
    Ok(out)
}

/// Retarget one clip source onto the `skeleton` rig and write BOTH variants under
/// `out_dir/{In-Place,RootMotion}/<stem>.json` at the 60 Hz canon — the CLI/library
/// write-both form of [`build_variants`] + [`write_variants`].
pub fn emit_variants(
    source: &Path,
    skeleton: &Path,
    out_dir: &Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let v = build_variants(source, skeleton)?;
    let paths = write_variants(&v, out_dir, true, true)?;
    let mut it = paths.into_iter();
    match (it.next(), it.next()) {
        (Some(ip), Some(rm)) => Ok((ip, rm)),
        _ => bail!("write_variants(true, true) must emit both paths"),
    }
}

fn round1(v: f32, dp: i32) -> f32 {
    let m = 10f32.powi(dp);
    (v * m).round() / m
}
fn round3(v: Vec3, dp: i32) -> [f32; 3] {
    [round1(v.x, dp), round1(v.y, dp), round1(v.z, dp)]
}
fn round4(v: [f32; 4], dp: i32) -> [f32; 4] {
    [
        round1(v[0], dp),
        round1(v[1], dp),
        round1(v[2], dp),
        round1(v[3], dp),
    ]
}
fn lerp3(a: [f32; 3], b: [f32; 3], u: f32) -> Vec3 {
    Vec3::from(a).lerp(Vec3::from(b), u)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The source MOTION channel order (DFS) of [`ZIGZAG_BVH`].
    const ZIGZAG_JOINTS: [&str; 11] = [
        "Hips",
        "Spine1",
        "Spine2",
        "Chest",
        "Neck1",
        "Neck2",
        "Head",
        "LeftShoulder",
        "LeftArm",
        "LeftForeArm",
        "LeftHand",
    ];

    /// A Motifect-SHAPED source: T-posed, Y-up, with the spine joints ZIG-ZAGGING through the
    /// body (Chest leans back, Neck2 and Head lean forward) and one horizontal arm.
    const ZIGZAG_BVH: &str = "HIERARCHY
ROOT Hips
{
  OFFSET 0 0 0
  CHANNELS 6 Xposition Yposition Zposition Zrotation Yrotation Xrotation
  JOINT Spine1
  {
    OFFSET 0 5 0
    CHANNELS 3 Zrotation Yrotation Xrotation
    JOINT Spine2
    {
      OFFSET 0 7 0
      CHANNELS 3 Zrotation Yrotation Xrotation
      JOINT Chest
      {
        OFFSET 0 7.6 -0.8
        CHANNELS 3 Zrotation Yrotation Xrotation
        JOINT Neck1
        {
          OFFSET 0 26 0
          CHANNELS 3 Zrotation Yrotation Xrotation
          JOINT Neck2
          {
            OFFSET 0 7.7 2.3
            CHANNELS 3 Zrotation Yrotation Xrotation
            JOINT Head
            {
              OFFSET 0 6.1 2.0
              CHANNELS 3 Zrotation Yrotation Xrotation
              End Site
              {
                OFFSET 0 16 0
              }
            }
          }
        }
        JOINT LeftShoulder
        {
          OFFSET 1.6 23 5
          CHANNELS 3 Zrotation Yrotation Xrotation
          JOINT LeftArm
          {
            OFFSET 15 0 -5.5
            CHANNELS 3 Zrotation Yrotation Xrotation
            JOINT LeftForeArm
            {
              OFFSET 28.7 0 0
              CHANNELS 3 Zrotation Yrotation Xrotation
              JOINT LeftHand
              {
                OFFSET 27 0 0
                CHANNELS 3 Zrotation Yrotation Xrotation
                End Site
                {
                  OFFSET 10 0 0
                }
              }
            }
          }
        }
      }
    }
  }
}
MOTION
Frames: 1
Frame Time: 0.0333333
";

    /// The canon-SHAPED target of the zig-zag fixtures: a plumb spine with an UNDRIVEN `neck_02`
    /// between `neck_01` and `head`, identity rest frames, one A-posed arm (`(name, parent, world)`).
    const TARGET: [(&str, i32, [f32; 3]); 12] = [
        ("root", -1, [0.0, 0.0, 0.0]),
        ("pelvis", 0, [0.0, 0.0, 95.0]),
        ("spine_01", 1, [0.0, 0.0, 104.0]),
        ("spine_02", 2, [0.0, 0.0, 114.0]),
        ("spine_03", 3, [0.0, 0.0, 124.0]),
        ("neck_01", 4, [0.0, 0.0, 143.0]),
        ("neck_02", 5, [0.0, 0.0, 146.0]),
        ("head", 6, [0.0, 0.0, 148.0]),
        ("clavicle_l", 4, [2.5, 0.0, 139.0]),
        ("upperarm_l", 8, [22.0, 0.0, 139.0]),
        ("lowerarm_l", 9, [45.0, 0.0, 118.0]),
        ("hand_l", 10, [64.0, 0.0, 101.0]),
    ];

    /// A `flicker.rig` document from `(name, parent, WORLD position, LOCAL rest rotation)` rows —
    /// each local translation is derived so the rest FK lands the bone on its world position —
    /// carrying `clips` (`(name, per-bone keys (T, R) per tick)`; unlisted bones stay undriven).
    #[allow(clippy::type_complexity)]
    fn rig_doc(
        rows: &[(&str, i32, [f32; 3], Quat)],
        clips: &[(&str, Vec<(&str, Vec<(Vec3, Quat)>)>)],
    ) -> Value {
        let mut global = Vec::new();
        let bones: Vec<Value> = rows
            .iter()
            .map(|(name, parent, pos, rot)| {
                let (pg, pp) = usize::try_from(*parent).map_or((Quat::IDENTITY, Vec3::ZERO), |p| {
                    (global[p], Vec3::from(rows[p].2))
                });
                global.push(pg * *rot);
                let local_t = pg.inverse() * (Vec3::from(*pos) - pp);
                json!({
                    "name": name,
                    "parent": parent,
                    "local": Mat4::from_rotation_translation(*rot, local_t).to_cols_array(),
                    "inverse_bind": Mat4::IDENTITY.to_cols_array(),
                })
            })
            .collect();
        let clips: Vec<Value> = clips
            .iter()
            .map(|(name, tracks)| {
                let duration = tracks.iter().map(|(_, k)| k.len()).max().unwrap_or(0);
                let tracks: Vec<Value> = tracks
                    .iter()
                    .map(|(bone, keys)| {
                        let keys: Vec<Value> = keys
                            .iter()
                            .enumerate()
                            .map(|(t, (tr, r))| {
                                json!({ "t": t, "T": [tr.x, tr.y, tr.z], "R": [r.x, r.y, r.z, r.w], "S": [1.0, 1.0, 1.0] })
                            })
                            .collect();
                        json!({ "bone": bone, "keys": keys })
                    })
                    .collect();
                json!({ "name": name, "tick_rate_hz": 60, "duration_ticks": duration, "tracks": tracks })
            })
            .collect();
        json!({
            "format": "flicker.rig", "version": 1,
            "skeleton": { "bones": bones },
            "mesh": { "vertices": [], "indices": [], "submeshes": [], "materials": [] },
            "clips": clips,
        })
    }

    /// The rest locals of a [`rig_doc`] row set: `(name, local translation, local rotation)`.
    fn rest_locals(rows: &[(&str, i32, [f32; 3], Quat)]) -> Vec<(String, Vec3, Quat)> {
        let mut global = Vec::new();
        rows.iter()
            .map(|(name, parent, pos, rot)| {
                let (pg, pp) = usize::try_from(*parent).map_or((Quat::IDENTITY, Vec3::ZERO), |p| {
                    (global[p], Vec3::from(rows[p].2))
                });
                global.push(pg * *rot);
                (
                    name.to_string(),
                    pg.inverse() * (Vec3::from(*pos) - pp),
                    *rot,
                )
            })
            .collect()
    }

    fn write_json(dir: &Path, name: &str, v: &Value) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, serde_json::to_string(v).unwrap()).unwrap();
        p
    }

    /// Write the zig-zag source (one frame; `rotations` = per-joint `(Z, Y, X)` degrees, unlisted
    /// joints zero) and the [`TARGET`] skeleton under `dir`.
    fn zigzag_fixture(
        dir: &Path,
        rotations: &[(&str, [f32; 3])],
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let mut frame = vec!["0".to_string(), "100".to_string(), "0".to_string()];
        for j in ZIGZAG_JOINTS {
            let r = rotations
                .iter()
                .find(|(n, _)| *n == j)
                .map_or([0.0; 3], |(_, r)| *r);
            frame.extend(r.iter().map(|v| format!("{v}")));
        }
        let bvh = dir.join("zigzag.bvh");
        std::fs::write(&bvh, format!("{ZIGZAG_BVH}{}\n", frame.join(" "))).unwrap();

        let rows: Vec<(&str, i32, [f32; 3], Quat)> = TARGET
            .iter()
            .map(|(n, p, pos)| (*n, *p, *pos, Quat::IDENTITY))
            .collect();
        let skel = write_json(dir, "target.json", &rig_doc(&rows, &[]));
        (bvh, skel)
    }

    /// Play tick `tick` of the in-place variant through the REAL runtime path (`rig_bones` →
    /// `resolve_clips` → `sample_local_poses` → `global_transforms`): posed world frames by name.
    fn play_tick(v: &ClipVariants, tick: u32) -> HashMap<String, Mat4> {
        let file: flicker_skeletal::format::RigFile =
            serde_json::from_value(v.in_place.clone()).unwrap();
        let bones = flicker_skeletal::format::rig_bones(&file);
        let clip = flicker_skeletal::format::resolve_clips(&file, &bones, false)
            .pop()
            .unwrap();
        let locals = flicker_skeletal::pose::sample_local_poses(&bones, &clip, tick, true);
        let g = flicker_skeletal::pose::global_transforms(&bones, &locals);
        bones
            .iter()
            .zip(g)
            .map(|(b, m)| (b.name.clone(), m))
            .collect()
    }

    fn play_tick0(v: &ClipVariants) -> HashMap<String, Mat4> {
        play_tick(v, 0)
    }

    fn world_deg(w: &HashMap<String, Mat4>, name: &str) -> f32 {
        w[name]
            .to_scale_rotation_translation()
            .1
            .angle_between(Quat::IDENTITY)
            .to_degrees()
    }

    /// THE SLOUCH GUARD (2026-08-21): an actor standing in the source rest must stand plumb on
    /// the canon even though the source's spine joints zig-zag — the torso transfers rotation
    /// DELTAS, not segment directions. The arm, a limb, still transfers by direction: the
    /// source's horizontal T-pose arm raises the canon's A-posed arm to horizontal.
    #[test]
    fn the_torso_rebases_by_rotation_delta_not_segment_direction() {
        let dir = std::env::temp_dir().join("flicker_retarget_zigzag_delta");
        let (bvh, skel) = zigzag_fixture(&dir, &[]);
        let w = play_tick0(&build_variants(&bvh, &skel).unwrap());
        for b in [
            "pelvis",
            "spine_01",
            "spine_02",
            "spine_03",
            "clavicle_l",
            "neck_01",
            "neck_02",
            "head",
        ] {
            let d = world_deg(&w, b);
            assert!(
                d < 0.05,
                "{b} stands plumb under a rest-posed source, got {d:.2}°"
            );
        }
        let dir_arm = (w["lowerarm_l"].w_axis - w["upperarm_l"].w_axis)
            .truncate()
            .normalize();
        assert!(
            (dir_arm - Vec3::X).length() < 1e-3,
            "the T-posed source arm raises the A-posed canon arm to horizontal, got {dir_arm:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE HEAD-JUT GUARD (2026-08-21): `neck_02` has no source joint, so it plays its rest local
    /// under `neck_01`'s posed rotation — the head, rebased against that frame, must carry the
    /// neck's 20° exactly once (rebasing it against `neck_02`'s BIND frame applied it twice).
    #[test]
    fn an_undriven_link_between_driven_bones_plays_the_parent_rotation_once() {
        let dir = std::env::temp_dir().join("flicker_retarget_zigzag_undriven");
        let (bvh, skel) = zigzag_fixture(&dir, &[("Neck1", [0.0, 0.0, 20.0])]);
        let w = play_tick0(&build_variants(&bvh, &skel).unwrap());
        assert!(
            world_deg(&w, "spine_03") < 0.05,
            "the chest below the neck is untouched"
        );
        for b in ["neck_01", "neck_02", "head"] {
            let d = world_deg(&w, b);
            assert!(
                (d - 20.0).abs() < 0.05,
                "{b} carries the neck's 20° exactly once, got {d:.2}°"
            );
        }
        let (axis, _) = w["head"].to_scale_rotation_translation().1.to_axis_angle();
        assert!(
            axis.x > 0.99,
            "the nod is about the body's X axis, got {axis:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rig-document source with ROTATED rest frames (a Katanami-like pelvis turned 90°, tilted
    /// spine segments, a swung arm, a pitched foot) retargeted onto ITS OWN skeleton must come back
    /// as the very same clip: `Sm` is identity when the rest directions agree, and `Sa · A_b` is then
    /// the source's posed global — every driven local returns, and the pelvis carries its own sway.
    #[test]
    fn a_rig_clip_retargets_onto_its_own_skeleton_unchanged() {
        let deg = |d: f32| d.to_radians();
        let rows: Vec<(&str, i32, [f32; 3], Quat)> = vec![
            ("root", -1, [0.0, 0.0, 0.0], Quat::IDENTITY),
            (
                "pelvis",
                0,
                [0.0, 0.0, 95.0],
                Quat::from_rotation_y(deg(90.0)),
            ),
            (
                "spine_01",
                1,
                [0.0, 0.0, 104.0],
                Quat::from_rotation_x(deg(10.0)),
            ),
            (
                "spine_02",
                2,
                [0.0, 0.0, 114.0],
                Quat::from_rotation_x(deg(-10.0)),
            ),
            (
                "spine_03",
                3,
                [0.0, 0.0, 124.0],
                Quat::from_rotation_z(deg(5.0)),
            ),
            ("neck_01", 4, [0.0, 0.0, 143.0], Quat::IDENTITY),
            ("neck_02", 5, [0.0, 0.0, 146.0], Quat::IDENTITY),
            (
                "head",
                6,
                [0.0, 0.0, 148.0],
                Quat::from_rotation_y(deg(-20.0)),
            ),
            (
                "clavicle_l",
                4,
                [2.5, 0.0, 139.0],
                Quat::from_rotation_z(deg(30.0)),
            ),
            (
                "upperarm_l",
                8,
                [22.0, 0.0, 139.0],
                Quat::from_rotation_y(deg(45.0)),
            ),
            (
                "lowerarm_l",
                9,
                [45.0, 0.0, 118.0],
                Quat::from_rotation_x(deg(15.0)),
            ),
            ("hand_l", 10, [64.0, 0.0, 101.0], Quat::IDENTITY),
            (
                "thigh_l",
                1,
                [9.0, 0.0, 95.0],
                Quat::from_rotation_z(deg(-8.0)),
            ),
            ("calf_l", 12, [9.0, 0.0, 50.0], Quat::IDENTITY),
            (
                "foot_l",
                13,
                [9.0, 0.0, 8.0],
                Quat::from_rotation_x(deg(25.0)),
            ),
            ("ball_l", 14, [9.0, -12.0, 2.0], Quat::IDENTITY),
        ];
        let rest = rest_locals(&rows);
        let sway = Vec3::new(0.0, 5.0, -3.0);
        // Tick 0 = the rest; tick 1 nods the neck, swings the arm, and sways the pelvis.
        let tracks: Vec<(&str, Vec<(Vec3, Quat)>)> = rest
            .iter()
            .filter(|(n, _, _)| n != "root")
            .map(|(n, t, r)| {
                let posed = match n.as_str() {
                    "neck_01" => (*t, *r * Quat::from_rotation_x(deg(20.0))),
                    "upperarm_l" => (*t, *r * Quat::from_rotation_y(deg(-30.0))),
                    "pelvis" => (*t + sway, *r),
                    _ => (*t, *r),
                };
                (n.as_str(), vec![(*t, *r), posed])
            })
            .collect();
        let doc = rig_doc(&rows, &[("wave", tracks.clone())]);
        let dir = std::env::temp_dir().join("flicker_retarget_rig_identity");
        let path = write_json(&dir, "wave.json", &doc);

        let v = build_variants(&path, &path).unwrap();
        assert_eq!(v.stem, "wave");
        let clip = &v.root_motion["clips"][0];
        assert_eq!(clip["duration_ticks"], 2);
        let out: HashMap<String, Vec<(Vec3, Quat)>> = clip["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tr| {
                let keys = tr["keys"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|k| {
                        let f = |a: &Value, i: usize| a[i].as_f64().unwrap() as f32;
                        (
                            Vec3::new(f(&k["T"], 0), f(&k["T"], 1), f(&k["T"], 2)),
                            Quat::from_xyzw(
                                f(&k["R"], 0),
                                f(&k["R"], 1),
                                f(&k["R"], 2),
                                f(&k["R"], 3),
                            ),
                        )
                    })
                    .collect();
                (tr["bone"].as_str().unwrap().to_string(), keys)
            })
            .collect();
        assert_eq!(
            out.len(),
            tracks.len(),
            "every driven bone gets a track, root none"
        );
        for (bone, keys) in &tracks {
            let got = &out[*bone];
            for (t, ((_, want_r), (got_t, got_r))) in keys.iter().zip(got).enumerate() {
                assert!(
                    want_r.dot(*got_r).abs() > 1.0 - 1e-5,
                    "{bone} tick {t}: rotation returns unchanged ({want_r:?} vs {got_r:?})"
                );
                let want_t = rest.iter().find(|(n, _, _)| n == bone).unwrap().1
                    + if *bone == "pelvis" && t == 1 {
                        sway
                    } else {
                        Vec3::ZERO
                    };
                assert!(
                    (*got_t - want_t).length() < 1e-4,
                    "{bone} tick {t}: translation is the rest offset (+ the pelvis's own sway), got {got_t:?} want {want_t:?}"
                );
            }
        }
        // The in-place variant pins the pelvis's planar sway and keeps the drop.
        let ip = v.in_place["clips"][0]["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tr| tr["bone"] == "pelvis")
            .unwrap();
        let k1 = &ip["keys"][1]["T"];
        assert!(
            (k1[0].as_f64().unwrap() as f32).abs() < 1e-5
                && (k1[1].as_f64().unwrap() as f32).abs() < 1e-5
                && (k1[2].as_f64().unwrap() as f32 - (95.0 - 3.0)).abs() < 1e-4,
            "in-place pelvis tick 1 = rest X/Y + swayed Z, got {k1}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A rig-document source gets the SAME rest reconciliation as a BVH: T-posed and zig-zagging in
    /// its own document, replayed at rest onto the plumb A-posed [`TARGET`] it stands plumb (torso
    /// = rotation deltas) with the arm raised to horizontal (limbs = direction).
    #[test]
    fn a_rig_source_rest_reconciles_like_a_bvh_rest() {
        let src_rows: Vec<(&str, i32, [f32; 3], Quat)> = vec![
            ("root", -1, [0.0, 0.0, 0.0], Quat::IDENTITY),
            ("pelvis", 0, [0.0, 0.0, 100.0], Quat::IDENTITY),
            ("spine_01", 1, [0.0, 0.0, 105.0], Quat::IDENTITY),
            ("spine_02", 2, [0.0, 0.0, 112.0], Quat::IDENTITY),
            ("spine_03", 3, [0.0, 0.8, 119.6], Quat::IDENTITY),
            ("neck_01", 4, [0.0, 0.8, 145.6], Quat::IDENTITY),
            ("neck_02", 5, [0.0, -1.5, 153.3], Quat::IDENTITY),
            ("head", 6, [0.0, -3.5, 159.4], Quat::IDENTITY),
            ("clavicle_l", 4, [1.6, -4.2, 168.6], Quat::IDENTITY),
            ("upperarm_l", 8, [16.6, 1.3, 168.6], Quat::IDENTITY),
            ("lowerarm_l", 9, [45.3, 1.3, 168.6], Quat::IDENTITY),
            ("hand_l", 10, [72.3, 1.3, 168.6], Quat::IDENTITY),
        ];
        let rest = rest_locals(&src_rows);
        let tracks: Vec<(&str, Vec<(Vec3, Quat)>)> = rest
            .iter()
            .filter(|(n, _, _)| n != "root")
            .map(|(n, t, r)| (n.as_str(), vec![(*t, *r)]))
            .collect();
        let dir = std::env::temp_dir().join("flicker_retarget_rig_rest");
        let (_, skel) = zigzag_fixture(&dir, &[]);
        let src = write_json(
            &dir,
            "tpose.json",
            &rig_doc(&src_rows, &[("tpose", tracks)]),
        );

        let w = play_tick0(&build_variants(&src, &skel).unwrap());
        for b in [
            "pelvis",
            "spine_01",
            "spine_02",
            "spine_03",
            "clavicle_l",
            "neck_01",
            "neck_02",
            "head",
        ] {
            let d = world_deg(&w, b);
            assert!(
                d < 0.05,
                "{b} stands plumb under a rest-posed rig source, got {d:.2}°"
            );
        }
        let dir_arm = (w["lowerarm_l"].w_axis - w["upperarm_l"].w_axis)
            .truncate()
            .normalize();
        assert!(
            (dir_arm - Vec3::X).length() < 1e-3,
            "the T-posed rig source raises the A-posed canon arm to horizontal, got {dir_arm:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE KATANAMI ORACLE: the recovered Idle, retargeted onto the packaged Humanoid reference, must
    /// point every limb where Katanami's own FK points it at tick 0. `#[ignore]`d — reads the
    /// sibling `PrismContentSource` repo. Run:
    ///   `cargo test -p flicker-content -- --ignored katanami --nocapture`
    #[test]
    #[ignore]
    fn katanami_idle_limbs_point_where_the_source_points() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../../../PrismContentSource/Katanami/clips/In-Place/MoveBasic/Idle_nonWeapon.json.gz",
        );
        if !src.exists() {
            eprintln!("skipping: no PrismContentSource/Katanami beside the repo");
            return;
        }
        let skel = crate::conform::default_reference();
        let v = build_variants(&src, &skel).unwrap();
        let w = play_tick0(&v);
        let s = Source::from_rig(&src).unwrap();
        let sa0 = &s.frames[0].0;
        let dc = dir_child();
        for b in [
            "thigh_l",
            "calf_l",
            "upperarm_l",
            "lowerarm_l",
            "thigh_r",
            "calf_r",
            "upperarm_r",
            "lowerarm_r",
        ] {
            let child = &dc[b];
            let src_dir = sa0[b] * s.rest_dir[child];
            let tgt_dir = (w[child].w_axis - w[b].w_axis).truncate().normalize();
            let deg = src_dir.angle_between(tgt_dir).to_degrees();
            eprintln!("{b:>11} → {child:<11} {deg:6.3}°");
            assert!(
                deg < 0.5,
                "{b} points where Katanami's does at tick 0, got {deg:.2}°"
            );
        }
        // The feet transfer by DELTA: Katanami's heeled rest must not pitch the canon's flat
        // foot into the floor — at the idle the toes stay on the ground plane.
        for b in ["ball_l", "ball_r"] {
            let z = w[b].w_axis.z;
            eprintln!("{b:>11} z = {z:6.2} cm (canon rest 2.0)");
            assert!(
                (-1.5..=6.0).contains(&z),
                "{b} rests on the ground plane, got z = {z:.2} cm"
            );
        }
        assert!(
            w["head"].w_axis.z > w["pelvis"].w_axis.z
                && w["pelvis"].w_axis.z > w["foot_l"].w_axis.z,
            "the body stands the right way up"
        );
    }
}
