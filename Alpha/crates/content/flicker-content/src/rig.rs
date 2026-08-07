//! Canonical rig conform — the in-app port of `tools/blender/rename_meshy_to_canonical.py`.
//!
//! THIS SLICE (WS-F slice 3, step 1): rename the raw Meshy (Mixamo-style) bones to canonical names,
//! producing a canonically-NAMED, displayable rig — enough for the bake/display slice. The
//! conform-QUALITY steps from the Blender script are follow-on slices, each reference-rig-dependent:
//!   * limb-align `reorient` (needs the Katanami reference orientation),
//!   * infer 30 fingers + 8 twists + 2 weapon sockets + face + neck_02 (24→67 canon),
//!   * mesh-derived hip WIDTH, and mesh decimation.

use std::collections::HashMap;

use crate::fbx::RawModel;

/// Meshy (Mixamo-style) bone name → canonical, verbatim from `rename_meshy_to_canonical.py::RENAME`.
/// Spine is bottom-up in Meshy (Spine02 is the LOWEST → spine_01).
fn rename_map() -> HashMap<&'static str, &'static str> {
    [
        ("Hips", "pelvis"),
        ("Spine02", "spine_01"),
        ("Spine01", "spine_02"),
        ("Spine", "spine_03"),
        ("neck", "neck_01"),
        ("Head", "head"),
        ("LeftShoulder", "clavicle_l"),
        ("LeftArm", "upperarm_l"),
        ("LeftForeArm", "lowerarm_l"),
        ("LeftHand", "hand_l"),
        ("RightShoulder", "clavicle_r"),
        ("RightArm", "upperarm_r"),
        ("RightForeArm", "lowerarm_r"),
        ("RightHand", "hand_r"),
        ("LeftUpLeg", "thigh_l"),
        ("LeftLeg", "calf_l"),
        ("LeftFoot", "foot_l"),
        ("LeftToeBase", "ball_l"),
        ("RightUpLeg", "thigh_r"),
        ("RightLeg", "calf_r"),
        ("RightFoot", "foot_r"),
        ("RightToeBase", "ball_r"),
    ]
    .into_iter()
    .collect()
}

/// Meshy head-tip markers — not deform bones (`rename_meshy_to_canonical.py::DROP`).
const DROP: &[&str] = &["head_end", "headfront"];

/// Strip a vendor namespace prefix like `mixamorig:Hips` or `Armature|Hips` → `Hips` (ufbx keeps the
/// raw FBX name; Blender's importer strips these, which is what the rename map assumes).
fn bare_name(n: &str) -> &str {
    n.rsplit([':', '|', '/']).next().unwrap_or(n)
}

/// What the rename did — for the editor to surface, and to catch a rig whose bones don't match the
/// Meshy scheme (a large `unmapped` set means the source isn't a standard Meshy biped).
#[derive(Debug, Clone, Default)]
pub struct RenameReport {
    pub renamed: usize,
    pub dropped: usize,
    /// Bare names that had no canonical target (kept as-is).
    pub unmapped: Vec<String>,
}

/// Rename the model's bones in place to canonical names, and drop the non-deform head markers
/// (removing a dropped bone re-indexes parents + every vertex joint that referenced it).
pub fn rename_to_canonical(model: &mut RawModel) -> RenameReport {
    let map = rename_map();

    // 1. rename (strip vendor prefix first).
    let mut report = RenameReport::default();
    for b in &mut model.bones {
        let bare = bare_name(&b.name).to_string();
        match map.get(bare.as_str()) {
            Some(&canon) => {
                b.name = canon.to_string();
                report.renamed += 1;
            }
            None => {
                b.name = bare.clone();
                if !DROP.contains(&bare.as_str()) {
                    report.unmapped.push(bare);
                }
            }
        }
    }

    // 2. drop the head-tip markers, compacting the bone list and rewiring parents + joint indices.
    let keep: Vec<bool> = model.bones.iter().map(|b| !DROP.contains(&b.name.as_str())).collect();
    report.dropped = keep.iter().filter(|k| !**k).count();
    if report.dropped > 0 {
        // old bone index → new index (None if dropped).
        let mut remap = vec![None; model.bones.len()];
        let mut next = 0u32;
        for (i, &k) in keep.iter().enumerate() {
            if k {
                remap[i] = Some(next);
                next += 1;
            }
        }
        // rebuild bones, rewiring parent to the new index (a dropped parent → root -1; markers are
        // leaves so this is defensive).
        let mut new_bones = Vec::with_capacity(next as usize);
        for (i, b) in model.bones.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            let mut b = b.clone();
            b.parent = if b.parent < 0 {
                -1
            } else {
                remap[b.parent as usize].map(|n| n as i32).unwrap_or(-1)
            };
            new_bones.push(b);
        }
        model.bones = new_bones;
        // rewire vertex joints; a dropped bone carried no weight, but keep indices valid regardless.
        for v in &mut model.vertices {
            for j in &mut v.joints {
                *j = remap[*j as usize].unwrap_or(0);
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::parse_fbx;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn renames_the_real_female_base_to_canonical() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/PrismHumanBaseA");
        if !dir.exists() {
            eprintln!("skipping: {} not present", dir.display());
            return;
        }
        let Some(fbx) = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).find(|p| {
            p.to_string_lossy().contains("Character_output") && p.extension().map(|e| e == "fbx").unwrap_or(false)
        }) else {
            eprintln!("skipping: no Character_output.fbx");
            return;
        };

        let mut model = parse_fbx(&fbx).unwrap();
        let before = model.bones.len();
        let report = rename_to_canonical(&mut model);
        eprintln!(
            "renamed {}, dropped {}, {} bones ({before} before); unmapped: {:?}",
            report.renamed, report.dropped, model.bones.len(), report.unmapped
        );

        let names: HashSet<&str> = model.bones.iter().map(|b| b.name.as_str()).collect();
        for n in [
            "pelvis", "spine_01", "spine_02", "spine_03", "head", "clavicle_l", "upperarm_l",
            "lowerarm_l", "hand_l", "thigh_l", "calf_l", "foot_l", "thigh_r", "hand_r",
        ] {
            assert!(names.contains(n), "canonical bone '{n}' present after rename (unmapped: {:?})", report.unmapped);
        }
        // Parents stay valid after any drop/compaction.
        let nb = model.bones.len() as i32;
        assert!(model.bones.iter().all(|b| b.parent == -1 || (b.parent >= 0 && b.parent < nb)));
        // Joints still valid after re-index.
        let nbu = model.bones.len() as u32;
        assert!(model.vertices.iter().all(|v| v.joints.iter().all(|&j| j < nbu)));
    }
}
