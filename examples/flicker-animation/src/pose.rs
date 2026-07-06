//! CPU-authoritative pose evaluation.
//!
//! This is the layer Slice 1 exists to validate: sample a clip at an integer tick
//! to per-bone LOCAL transforms, then accumulate down the hierarchy into GLOBAL
//! transforms. If the bone wireframe animates correctly from this, the whole
//! authoritative layer is proven before a single triangle is skinned.

use glam::{Mat4, Quat, Vec3};

use crate::format::{Bone, ResolvedClip};

/// Sample `clip` at integer `tick` → per-bone LOCAL transforms (one per bone).
/// Bones with no track for this clip keep their rest local transform.
///
/// Keys are dense (one per tick), so we index directly, clamping to the last key
/// past the end (defensive — the caller wraps `tick` within the clip duration).
pub fn sample_local_poses(bones: &[Bone], clip: &ResolvedClip, tick: u32) -> Vec<Mat4> {
    let mut locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
    for track in &clip.tracks {
        if track.keys.is_empty() {
            continue;
        }
        let idx = (tick as usize).min(track.keys.len() - 1);
        let k = &track.keys[idx];
        let t = Vec3::from(k.translation);
        let mut r = Quat::from_xyzw(k.rotation[0], k.rotation[1], k.rotation[2], k.rotation[3]);
        // Baked quats should be unit; guard against a zero/denormal quat producing NaNs.
        r = if r.length_squared() > 1e-8 {
            r.normalize()
        } else {
            Quat::IDENTITY
        };
        let s = Vec3::from(k.scale);
        // glam composes this as T * R * S — the correct local TRS order.
        locals[track.bone] = Mat4::from_scale_rotation_translation(s, r, t);
    }
    locals
}

/// Accumulate LOCAL transforms into GLOBAL (model-space) transforms.
///
/// Bones are emitted in hierarchy order (parents precede children), so a single
/// forward pass suffices: `global[i] = global[parent] * local[i]`.
pub fn global_transforms(bones: &[Bone], locals: &[Mat4]) -> Vec<Mat4> {
    let mut g = vec![Mat4::IDENTITY; bones.len()];
    for (i, bone) in bones.iter().enumerate() {
        g[i] = if bone.parent < 0 {
            locals[i]
        } else {
            g[bone.parent as usize] * locals[i]
        };
    }
    g
}
