//! In-app BVH → `flicker.rig` clip retargeting — the Rust port of `tools/retarget_bvh.py`
//! (WS-F, no external tools). Replays each Motifect BVH's motion FROM the TARGET rig's actual bind
//! (`Ta_b = Sa_b · inv(Sm_b) · A_b`, A_b = the target's bind global rotation), so re-baking onto a
//! different bind (e.g. HumanBaseA's flat foot instead of PrismHumanBaseA's heeled one) is just a
//! change of target skeleton. Emits both the root-motion and in-place variants at the 60 Hz canon.
//!
//! The quaternion algebra mirrors `flicker_rebase.py` (memory 614E5958): glam `a * b` == the Python
//! `qmul(a,b)` ("apply b then a"); `Quat::from_rotation_arc(u,v)` == `q_between(u,v)`; the Y-up→Z-up
//! convert is the similarity `C · q · inv(C)` with `C = Rx(+90°)`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use glam::{Mat4, Quat, Vec3};
use serde_json::{json, Value};

use crate::bvh::{parse_bvh, Bvh};

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
    let fingers = [("thumb", "Thumb"), ("index", "Index"), ("middle", "Middle"), ("ring", "Ring"), ("pinky", "Pinky")];
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
                m.insert(format!("{ft}_{n:02}_{side_t}"), format!("{side_s}Hand{fs}{n}"));
            }
        }
    }
    m
}

/// Target bone → the child bone whose rest offset defines this bone's forward DIRECTION (spec §C.2).
/// Leaves (no entry) inherit their parent's reconciliation.
fn dir_child() -> HashMap<String, String> {
    let mut m: HashMap<String, String> = HashMap::new();
    let base = [
        ("pelvis", "spine_01"), ("spine_01", "spine_02"), ("spine_02", "spine_03"),
        ("spine_03", "neck_01"), ("neck_01", "head"),
        ("thigh_l", "calf_l"), ("calf_l", "foot_l"), ("foot_l", "ball_l"),
        ("thigh_r", "calf_r"), ("calf_r", "foot_r"), ("foot_r", "ball_r"),
    ];
    for (a, b) in base {
        m.insert(a.into(), b.into());
    }
    for s in ["l", "r"] {
        m.insert(format!("clavicle_{s}"), format!("upperarm_{s}"));
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

/// The target skeleton, decomposed for the retarget: bind global rotations (`A_b`), rest offsets,
/// and the bind world direction to each bone's `DIR_CHILD`. `bones_json` is kept verbatim to embed.
struct Target {
    bones_json: Value,
    names: Vec<String>,
    idx: HashMap<String, usize>,
    parent: Vec<i32>,
    rest_transl: Vec<Vec3>,
    bind_global: Vec<Quat>,
    bind_dir: HashMap<String, Vec3>,
}

/// Load a `flicker.rig` skeleton as a retarget [`Target`]: FK the bones' own local rotations to the
/// bind global (`A_b`) and derive each bone's bind-pose direction to its `DIR_CHILD`.
fn load_target(skeleton: &Path) -> Result<Target> {
    // Gz-transparent read: the target rig is PROCESSED content (gz at rest).
    let text = crate::package::read_text(skeleton)
        .with_context(|| format!("reading target {}", skeleton.display()))?;
    let v: Value = serde_json::from_str(&text).with_context(|| format!("parsing target {}", skeleton.display()))?;
    let bones_json = v["skeleton"]["bones"].clone();
    let bones = bones_json.as_array().context("target has no skeleton.bones")?;

    let (mut names, mut parent, mut rest_transl, mut loc_r) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for b in bones {
        names.push(b["name"].as_str().context("bone.name")?.to_string());
        parent.push(b["parent"].as_i64().context("bone.parent")? as i32);
        let arr = b["local"].as_array().context("bone.local")?;
        let mut m = [0.0f32; 16];
        for (i, f) in arr.iter().enumerate().take(16) {
            m[i] = f.as_f64().unwrap_or(0.0) as f32;
        }
        let mat = Mat4::from_cols_array(&m);
        rest_transl.push(mat.w_axis.truncate());
        loc_r.push(mat.to_scale_rotation_translation().1);
    }
    let idx: HashMap<String, usize> = names.iter().enumerate().map(|(i, n)| (n.clone(), i)).collect();

    // A_b = FK of the skeleton's OWN local rotations (the pose the mesh is skinned in).
    let mut bind_global = vec![Quat::IDENTITY; names.len()];
    for i in 0..names.len() {
        bind_global[i] = if parent[i] < 0 { loc_r[i] } else { bind_global[parent[i] as usize] * loc_r[i] };
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

    Ok(Target { bones_json, names, idx, parent, rest_transl, bind_global, bind_dir })
}

/// The source base pose `Sm_b`: the source rig posed so each bone points along OUR bind direction
/// (spec §C.2). For BVH the source rest global is identity, so a bone's rest direction is just its
/// child OFFSET (Y-up → Z-up); `Sm_b` = the minimal rotation source-dir → bind-dir. Leaves inherit.
fn source_base_pose(bvh: &Bvh, target: &Target) -> HashMap<String, Quat> {
    let nmap = name_map();
    let dc = dir_child();
    let jidx: HashMap<&str, usize> = bvh.joints.iter().enumerate().map(|(i, j)| (j.name.as_str(), i)).collect();
    let c = c_yup_to_zup();

    let mut sm: HashMap<String, Quat> = HashMap::new();
    for (i, nm) in target.names.iter().enumerate() {
        let s_child = dc.get(nm).and_then(|cb| nmap.get(cb));
        let resolved = (|| {
            let _src = nmap.get(nm)?;
            let bd = target.bind_dir.get(nm)?;
            let &sj = jidx.get(s_child?.as_str())?;
            let off = Vec3::from(bvh.joints[sj].offset);
            (off.length() > 1e-9).then(|| {
                let s_dir = (c * off.normalize()).normalize();
                Quat::from_rotation_arc(s_dir, bd.normalize())
            })
        })();
        let q = resolved.unwrap_or_else(|| {
            let p = target.parent[i];
            if p >= 0 {
                sm.get(&target.names[p as usize]).copied().unwrap_or(Quat::IDENTITY)
            } else {
                Quat::IDENTITY
            }
        });
        sm.insert(nm.clone(), q);
    }
    sm
}

/// One frame's rest-rebase → target bone → LOCAL rotation quat. `Ta_b = Sa_map(b) · inv(Sm_b) · A_b`
/// for mapped bones (else `A_b`, left at bind); `local_b = inv(Ta_parent) · Ta_b`. Mapped bones only.
fn rebase_frame(
    sa_zup: &HashMap<String, Quat>,
    sm: &HashMap<String, Quat>,
    target: &Target,
    nmap: &HashMap<String, String>,
) -> HashMap<String, Quat> {
    let mut ta = vec![Quat::IDENTITY; target.names.len()];
    for (i, nm) in target.names.iter().enumerate() {
        let a_b = target.bind_global[i];
        ta[i] = match nmap.get(nm).and_then(|s| sa_zup.get(s)) {
            Some(&sa) => sa * sm.get(nm).copied().unwrap_or(Quat::IDENTITY).inverse() * a_b,
            None => a_b,
        };
    }
    let mut out = HashMap::new();
    for (i, nm) in target.names.iter().enumerate() {
        if !nmap.contains_key(nm) {
            continue;
        }
        let p = target.parent[i];
        let pg = if p >= 0 { ta[p as usize] } else { Quat::IDENTITY };
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

/// The ROOT-MOTION retarget of one BVH onto `target`, at the BVH's native rate. Returns the per-bone
/// tracks (in target-bone order) + `(native_hz, frame_count)`.
fn retarget_root_motion(bvh: &Bvh, target: &Target) -> Result<(Tracks, u32, u32)> {
    let nmap = name_map();
    let sm = source_base_pose(bvh, target);
    let native_hz = bvh.fps().round().max(1.0) as u32;

    let pelvis_i = *target.idx.get("pelvis").context("target has no pelvis")?;
    let (_, root_pos0) = bvh.frame_locals(&bvh.frames[0]);
    let root_pos0 = Vec3::from(root_pos0);
    let src_hip_h = root_pos0.y; // Y-up standing height
    let tgt_hip_h = target.rest_transl[pelvis_i].z; // Z-up pelvis rest height
    let prop = if src_hip_h.abs() > 1e-6 { tgt_hip_h / src_hip_h } else { 1.0 };
    let pelvis_rest = target.rest_transl[pelvis_i];
    let c = c_yup_to_zup();

    let mut tracks: HashMap<String, Vec<Key>> = HashMap::new();
    for (t, frame) in bvh.frames.iter().enumerate() {
        let (local_q, root_pos) = bvh.frame_locals(frame);
        let g = bvh.global_rotations(&local_q);
        let sa_zup: HashMap<String, Quat> =
            bvh.joints.iter().enumerate().map(|(i, j)| (j.name.clone(), convert_global(g[i]))).collect();
        let locals_t = rebase_frame(&sa_zup, &sm, target, &nmap);

        let delta = (c * (Vec3::from(root_pos) - root_pos0)) * prop;
        let pelvis_t = pelvis_rest + delta;

        for (nm, q) in &locals_t {
            let tr = if nm == "pelvis" { pelvis_t } else { target.rest_transl[target.idx[nm]] };
            tracks.entry(nm.clone()).or_default().push(Key {
                t: t as u32,
                translation: round3(tr, 6),
                rotation: round4([q.x, q.y, q.z, q.w], 8),
                scale: [1.0, 1.0, 1.0],
            });
        }
    }

    // Target-bone order, only mapped + non-empty.
    let ordered: Vec<(String, Vec<Key>)> = target
        .names
        .iter()
        .filter_map(|nm| tracks.remove(nm).filter(|k| !k.is_empty()).map(|k| (nm.clone(), k)))
        .collect();
    Ok((ordered, native_hz, bvh.frames.len() as u32))
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
        out.push(Key { t: k as u32 * mult, ..a.clone() });
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
    out.push(Key { t: (keys.len() as u32 - 1) * mult, ..last.clone() });
    out
}

/// Build the `flicker.rig` clip JSON: the target skeleton embedded verbatim + one clip's tracks.
fn build_clip_json(target: &Target, name: &str, tick_rate_hz: u32, duration_ticks: u32, tracks: &[(String, Vec<Key>)], source_file: &str) -> Value {
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
            "applied_transform": "bvh-retarget: Y_up->Z_up (Rx+90) + rest-rebase onto target bind",
            "textures": [],
        },
        "retarget": true,
        "skeleton": { "bones": target.bones_json },
        "mesh": { "vertices": [], "indices": [], "submeshes": [], "materials": [] },
        "morphs": [],
        "clips": [{ "name": name, "tick_rate_hz": tick_rate_hz, "duration_ticks": duration_ticks, "tracks": track_json }],
    })
}

/// One BVH retargeted onto a skeleton, BOTH variants built in memory — the seam the
/// Clayworks clip preview samples before anything is committed to disk. Each value is
/// a complete `flicker.rig` clip JSON (target skeleton embedded, 60 Hz canon).
pub struct ClipVariants {
    /// The BVH stem — the clip's name, and the emitted file stem.
    pub stem: String,
    /// Pelvis planar (X/Y) translation pinned to rest (treadmill); vertical bob kept.
    pub in_place: Value,
    /// Full planar travel kept.
    pub root_motion: Value,
}

/// Retarget one BVH onto the `skeleton` rig at the 60 Hz canon and return both
/// variants in memory. [`write_variants`] / [`emit_variants`] are the disk halves.
pub fn build_variants(bvh_path: &Path, skeleton: &Path) -> Result<ClipVariants> {
    let bvh = parse_bvh(bvh_path)?;
    if bvh.frames.is_empty() {
        bail!("BVH {} has no motion frames", bvh_path.display());
    }
    let target = load_target(skeleton)?;
    let (tracks_native, native_hz, nframes) = retarget_root_motion(&bvh, &target)?;

    // Resample to the 60 Hz canon (source frames land on even ticks).
    let ratio = CANON_FPS as f32 / native_hz as f32;
    let mult = ratio.round() as u32;
    if mult < 1 || (ratio - mult as f32).abs() > 1e-4 {
        bail!("{CANON_FPS} Hz is not an integer multiple of the clip rate {native_hz}");
    }
    let rm_tracks: Vec<(String, Vec<Key>)> =
        tracks_native.iter().map(|(nm, keys)| (nm.clone(), resample_keys(keys, mult))).collect();
    let duration = if nframes > 0 { (nframes - 1) * mult + 1 } else { 0 };

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

    let stem = bvh_path.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let src_file = bvh_path.file_name().and_then(|s| s.to_str()).unwrap_or("clip.bvh");
    Ok(ClipVariants {
        stem: stem.to_string(),
        in_place: build_clip_json(&target, stem, CANON_FPS, duration, &ip_tracks, src_file),
        root_motion: build_clip_json(&target, stem, CANON_FPS, duration, &rm_tracks, src_file),
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
    for (on, sub, value) in
        [(in_place, "In-Place", &v.in_place), (root_motion, "RootMotion", &v.root_motion)]
    {
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

/// Retarget one BVH onto the `skeleton` rig and write BOTH variants under
/// `out_dir/{In-Place,RootMotion}/<stem>.json` at the 60 Hz canon — the CLI/library
/// write-both form of [`build_variants`] + [`write_variants`].
pub fn emit_variants(bvh_path: &Path, skeleton: &Path, out_dir: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let v = build_variants(bvh_path, skeleton)?;
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
    [round1(v[0], dp), round1(v[1], dp), round1(v[2], dp), round1(v[3], dp)]
}
fn lerp3(a: [f32; 3], b: [f32; 3], u: f32) -> Vec3 {
    Vec3::from(a).lerp(Vec3::from(b), u)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VALIDATION (the oracle discipline): retargeting `walk_forward.bvh` onto PrismHumanBaseA's bind
    /// must reproduce the existing Python-baked clip (`retarget/clips/locomotion/RootMotion/…`). If my
    /// Rust port matches the Blender/Python tool per-key, re-baking onto HumanBaseA is trustworthy.
    /// `#[ignore]`d (reads source + oracle). HISTORICAL since the 2026-08-04 content sweep: the
    /// PrismHumanBaseA skeleton fixture was retired with the example content, so this skips — the
    /// port-parity validation it existed for passed (0.08° vs the Python oracle) and is banked. Run:
    ///   `cargo test -p flicker-content -- --ignored retarget_reproduces_python --nocapture`
    #[test]
    #[ignore]
    fn retarget_reproduces_python_oracle() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content");
        let bvh = content.join("source/Motifect/Motifect_locomotion_complete_v1_0/BVH/walk_forward.bvh");
        let skeleton = content.join("package/characters/PrismHumanBaseA/PrismHumanBaseA.json");
        let oracle = content.join("package/retarget/clips/locomotion/RootMotion/walk_forward.json");
        if !bvh.exists() || !skeleton.exists() || !oracle.exists() {
            eprintln!("skipping: BVH / reference / oracle clip not all present");
            return;
        }
        let out = std::env::temp_dir().join("flicker_retarget_validate");
        let (_ip, rm) = emit_variants(&bvh, &skeleton, &out).unwrap();

        let load = |p: &Path| -> Value { serde_json::from_str(&crate::package::read_text(p).unwrap()).unwrap() };
        let (mine, theirs) = (load(&rm), load(&oracle));
        let tracks = |v: &Value| -> HashMap<String, Vec<[f32; 4]>> {
            v["clips"][0]["tracks"].as_array().unwrap().iter().map(|tr| {
                let bone = tr["bone"].as_str().unwrap().to_string();
                let rs: Vec<[f32; 4]> = tr["keys"].as_array().unwrap().iter().map(|k| {
                    let r = k["R"].as_array().unwrap();
                    [r[0].as_f64().unwrap() as f32, r[1].as_f64().unwrap() as f32, r[2].as_f64().unwrap() as f32, r[3].as_f64().unwrap() as f32]
                }).collect();
                (bone, rs)
            }).collect()
        };
        let (mt, tt) = (tracks(&mine), tracks(&theirs));
        eprintln!("my clip: {} tracks, {} ticks; oracle: {} tracks", mt.len(), mine["clips"][0]["duration_ticks"], tt.len());

        let mut worst_deg = 0.0f32;
        let mut worst_bone = String::new();
        let mut compared = 0usize;
        for (bone, tr) in &tt {
            let Some(mr) = mt.get(bone) else { continue };
            for (a, b) in mr.iter().zip(tr.iter()) {
                let qa = Quat::from_xyzw(a[0], a[1], a[2], a[3]);
                let qb = Quat::from_xyzw(b[0], b[1], b[2], b[3]);
                let deg = qa.angle_between(qb).to_degrees();
                if deg > worst_deg {
                    worst_deg = deg;
                    worst_bone = bone.clone();
                }
                compared += 1;
            }
        }
        eprintln!("worst rotation delta vs Python oracle: {worst_deg:.4}° at '{worst_bone}' ({compared} keys compared)");
        let _ = std::fs::remove_dir_all(&out);
        assert!(compared > 100, "compared a real number of keys");
        assert!(worst_deg < 1.0, "Rust retarget reproduces the Python clip within ~1° (worst {worst_deg:.4}° at {worst_bone})");
    }
}
