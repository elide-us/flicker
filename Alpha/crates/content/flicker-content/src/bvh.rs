//! BVH motion parser — the in-app port of `tools/retarget_bvh.py`'s parse stage (WS-F, clip
//! retargeting). Reads a Motifect `.bvh` (Y-up) into a hierarchy + per-frame motion, and decodes
//! each frame to per-joint local rotation quats (glam) + the root position. The Y-up→Z-up convert,
//! canonical name-map, and rest-rebase live in the retarget stage; this module only parses.

use std::path::Path;

use anyhow::{bail, Context, Result};
use glam::Quat;

/// One BVH joint: name, parent index (`-1` for the root), local `offset`, and its declared channel
/// list (root: 6 = 3 position + 3 rotation; others: 3 rotation). End Sites carry no channels and are
/// dropped during parse.
#[derive(Debug, Clone)]
pub struct BvhJoint {
    pub name: String,
    pub parent: i32,
    pub offset: [f32; 3],
    pub channels: Vec<String>,
}

/// A parsed BVH: DFS-ordered joints + per-frame channel values + the frame time (seconds).
#[derive(Debug, Clone)]
pub struct Bvh {
    pub joints: Vec<BvhJoint>,
    pub frames: Vec<Vec<f32>>,
    pub frame_time: f32,
}

impl Bvh {
    /// Source frame rate (Hz), derived from `frame_time`.
    pub fn fps(&self) -> f32 {
        if self.frame_time > 0.0 {
            1.0 / self.frame_time
        } else {
            0.0
        }
    }

    /// Decode one motion `frame` into per-joint LOCAL rotation quats + the root position. Rotations
    /// compose in each joint's DECLARED channel order (BVH applies them in listed order, e.g. Z,Y,X →
    /// `qz * qy * qx`). Positions fill the root's translation (other joints use their static offset).
    pub fn frame_locals(&self, frame: &[f32]) -> (Vec<Quat>, [f32; 3]) {
        let mut local = vec![Quat::IDENTITY; self.joints.len()];
        let mut root_pos = [0.0f32; 3];
        let mut p = 0usize;
        for (idx, j) in self.joints.iter().enumerate() {
            let mut q = Quat::IDENTITY;
            let mut pos = [0.0f32; 3];
            for ch in &j.channels {
                let v = frame.get(p).copied().unwrap_or(0.0);
                p += 1;
                let c = ch.as_bytes()[0];
                if ch.ends_with("position") {
                    let axis = match c {
                        b'X' => 0,
                        b'Y' => 1,
                        _ => 2,
                    };
                    pos[axis] = v;
                } else {
                    // rotation: compose in listed order (glam `a * b` = apply b then a, matching the
                    // Python `qmul(q, q_axis)`).
                    q *= q_axis(c, v);
                }
            }
            local[idx] = q;
            if j.parent < 0 {
                root_pos = pos;
            }
        }
        (local, root_pos)
    }

    /// FK the source hierarchy: GLOBAL rotation quat per joint (Y-up source space).
    pub fn global_rotations(&self, local: &[Quat]) -> Vec<Quat> {
        let mut g = vec![Quat::IDENTITY; self.joints.len()];
        for (idx, j) in self.joints.iter().enumerate() {
            g[idx] = if j.parent < 0 { local[idx] } else { g[j.parent as usize] * local[idx] };
        }
        g
    }
}

/// Right-handed rotation of `deg` degrees about a principal axis (`X`/`Y`/`Z`), as a quat.
fn q_axis(axis: u8, deg: f32) -> Quat {
    let r = deg.to_radians();
    match axis {
        b'X' => Quat::from_rotation_x(r),
        b'Y' => Quat::from_rotation_y(r),
        b'Z' => Quat::from_rotation_z(r),
        _ => Quat::IDENTITY,
    }
}

/// Parse a BVH file into a [`Bvh`]. End Sites are dropped (no channels); channel order is respected
/// AS DECLARED per joint.
pub fn parse_bvh(path: &Path) -> Result<Bvh> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading BVH {}", path.display()))?;

    let mut joints: Vec<BvhJoint> = Vec::new();
    let mut stack: Vec<Option<usize>> = Vec::new(); // joint index, or None for an End Site sentinel
    let mut frames: Vec<Vec<f32>> = Vec::new();
    let mut frame_time = 0.0f32;
    let mut in_motion = false;

    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        let Some(&head) = w.first() else { continue };
        match head {
            "ROOT" | "JOINT" => {
                let parent = stack.iter().rev().find_map(|s| *s).map(|i| i as i32).unwrap_or(-1);
                joints.push(BvhJoint {
                    name: w.get(1).unwrap_or(&"").to_string(),
                    parent,
                    offset: [0.0; 3],
                    channels: Vec::new(),
                });
                stack.push(Some(joints.len() - 1));
            }
            "End" => stack.push(None), // "End Site"
            "OFFSET" => {
                if let Some(Some(idx)) = stack.last().copied() {
                    joints[idx].offset =
                        [parse3(&w, 1)?, parse3(&w, 2)?, parse3(&w, 3)?];
                }
            }
            "CHANNELS" => {
                if let Some(Some(idx)) = stack.last().copied() {
                    joints[idx].channels = w[2..].iter().map(|s| s.to_string()).collect();
                }
            }
            "}" => {
                stack.pop();
            }
            "MOTION" => in_motion = true,
            "Frames:" => {}
            "Frame" if w.len() >= 3 && w[1] == "Time:" => {
                frame_time = w[2].parse().unwrap_or(0.0);
            }
            _ if in_motion => {
                let vals: Vec<f32> = w.iter().filter_map(|x| x.parse().ok()).collect();
                if !vals.is_empty() {
                    frames.push(vals);
                }
            }
            _ => {}
        }
    }

    if joints.is_empty() {
        bail!("no joints parsed from BVH {}", path.display());
    }
    Ok(Bvh { joints, frames, frame_time })
}

fn parse3(w: &[&str], i: usize) -> Result<f32> {
    w.get(i)
        .and_then(|s| s.parse().ok())
        .with_context(|| format!("BVH OFFSET missing component {i}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locomotion_bvh(stem: &str) -> Option<std::path::PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content/source/Motifect/Motifect_locomotion_complete_v1_0/BVH");
        let p = root.join(format!("{stem}.bvh"));
        p.exists().then_some(p)
    }

    /// Parse a real Motifect locomotion clip: DFS hierarchy with `Hips` root, 30 fps, ≥1 frame, and
    /// per-frame decode yields finite quats. `#[ignore]`d (reads source content). Run:
    ///   `cargo test -p flicker-content -- --ignored parses_a_real_bvh --nocapture`
    #[test]
    #[ignore]
    fn parses_a_real_bvh() {
        let Some(path) = locomotion_bvh("walk_forward") else {
            eprintln!("skipping: no Motifect locomotion BVH");
            return;
        };
        let bvh = parse_bvh(&path).unwrap();
        eprintln!(
            "{}: {} joints, {} frames @ {:.3}s ({:.0} fps); root '{}'",
            path.file_name().unwrap().to_string_lossy(),
            bvh.joints.len(), bvh.frames.len(), bvh.frame_time, bvh.fps(), bvh.joints[0].name
        );
        assert_eq!(bvh.joints[0].name, "Hips", "root is Hips");
        assert_eq!(bvh.joints[0].parent, -1, "root has no parent");
        assert_eq!(bvh.joints[0].channels.len(), 6, "root has 6 channels (pos+rot)");
        assert!(bvh.joints.iter().skip(1).all(|j| j.channels.len() == 3 || j.channels.is_empty()));
        assert!((28.0..32.0).contains(&bvh.fps()), "≈30 fps, got {}", bvh.fps());
        assert!(!bvh.frames.is_empty(), "has motion frames");
        assert!(bvh.joints.iter().any(|j| j.name == "Spine1"), "has Spine1");
        assert!(bvh.joints.iter().any(|j| j.name == "LeftFoot"), "has LeftFoot");

        // Frame decode: finite local quats + FK globals.
        let (local, root_pos) = bvh.frame_locals(&bvh.frames[0]);
        assert_eq!(local.len(), bvh.joints.len());
        assert!(local.iter().all(|q| q.is_finite()), "finite local quats");
        assert!(root_pos.iter().all(|v| v.is_finite()), "finite root position");
        let g = bvh.global_rotations(&local);
        assert!(g.iter().all(|q| q.is_finite()), "finite FK globals");
        eprintln!("frame 0: root_pos={root_pos:?}, {} local quats decoded", local.len());
    }
}
