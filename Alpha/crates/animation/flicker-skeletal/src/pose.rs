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
pub fn sample_local_poses(
    bones: &[Bone],
    clip: &ResolvedClip,
    tick: u32,
    retarget: bool,
) -> Vec<Mat4> {
    let mut locals: Vec<Mat4> = bones.iter().map(|b| b.local).collect();
    for track in &clip.tracks {
        if track.keys.is_empty() {
            continue;
        }
        let idx = (tick as usize).min(track.keys.len() - 1);
        let k = &track.keys[idx];
        // Translation retargeting: rebase each non-root bone's translation off the SOURCE's
        // rest onto THIS rig's rest — `target_rest + (clip_T - source_rest)`. For a constant-
        // offset bone (limbs) `clip_T == source_rest`, so this collapses to the rig's own rest
        // offset — its proportions are preserved and a clip from a differently-proportioned
        // skeleton doesn't drag it. For a bone that actually TRANSLATES — the pelvis's hip
        // sway/bob — it keeps that animated delta, rebased onto this rig's hip height, instead
        // of freezing the hips to rest (which killed the walk's hip motion). The root keeps the
        // clip translation (root motion). No-op for the authoring rig (source_rest == its rest).
        let t = if retarget && bones[track.bone].parent >= 0 {
            bones[track.bone].local.w_axis.truncate() + Vec3::from(k.translation)
                - Vec3::from(track.source_rest)
        } else {
            Vec3::from(k.translation)
        };
        let r = Quat::from_xyzw(k.rotation[0], k.rotation[1], k.rotation[2], k.rotation[3]);
        // Baked quats should be unit; guard against a zero/denormal quat producing NaNs.
        let r = if r.length_squared() > 1e-8 {
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

/// Crossfade two LOCAL pose sets (per-bone TRS matrices) by weight `w`: `0.0`
/// returns `from`, `1.0` returns `to`, and values between interpolate. Used for
/// transition blending — sample the outgoing and incoming clips separately (each
/// with [`sample_local_poses`]), blend the LOCAL transforms here, then accumulate
/// [`global_transforms`] on the result.
///
/// Blending is done at the TRS level (not by lerping matrix elements, which would
/// shear/skew a rotating bone): each local is decomposed, translation and scale are
/// lerped, and rotation is slerped along the shortest arc. `from` and `to` should be
/// the same length (the same skeleton); a mismatch clamps to the shorter.
///
/// Only runs while a transition is actively crossfading (a handful of frames), so
/// the per-bone decompose/recompose is not a per-frame steady-state cost.
pub fn blend_local_poses(from: &[Mat4], to: &[Mat4], w: f32) -> Vec<Mat4> {
    let w = w.clamp(0.0, 1.0);
    from.iter()
        .zip(to.iter())
        .map(|(&a, &b)| {
            let (sa, ra, ta) = a.to_scale_rotation_translation();
            let (sb, mut rb, tb) = b.to_scale_rotation_translation();
            // Put the target rotation in the same hemisphere so the slerp takes the
            // shortest arc (a quaternion and its negation are the same orientation).
            if ra.dot(rb) < 0.0 {
                rb = -rb;
            }
            let s = sa.lerp(sb, w);
            let r = ra.slerp(rb, w);
            let t = ta.lerp(tb, w);
            Mat4::from_scale_rotation_translation(s, r, t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Bone, Keyframe, ResolvedClip, ResolvedTrack};
    use glam::Quat;

    /// Rotation retargeting keeps a non-root bone's OWN rest translation and ignores the
    /// clip's baked source offset (`retarget = true`); the authoring path (`false`) uses
    /// the clip translation. The root always uses the clip translation (root motion).
    #[test]
    fn retarget_keeps_rest_translation_for_nonroot() {
        let bones = vec![
            Bone {
                name: "root".into(),
                parent: -1,
                local: Mat4::IDENTITY,
                inverse_bind: Mat4::IDENTITY,
            },
            Bone {
                name: "child".into(),
                parent: 0,
                local: Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
                inverse_bind: Mat4::IDENTITY,
            },
        ];
        // Clip drives BOTH bones' local translation to x=99 (the source skeleton's offset).
        let key = |x: f32| Keyframe {
            t: 0,
            translation: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        let clip = ResolvedClip {
            name: "t".into(),
            tick_rate_hz: 60,
            duration_ticks: 1,
            // The clip's offset EQUALS the source rest (a constant-offset bone) → delta 0.
            tracks: vec![
                ResolvedTrack {
                    bone: 0,
                    keys: vec![key(99.0)],
                    source_rest: [99.0, 0.0, 0.0],
                },
                ResolvedTrack {
                    bone: 1,
                    keys: vec![key(99.0)],
                    source_rest: [99.0, 0.0, 0.0],
                },
            ],
            unresolved: vec![],
        };
        // Authoring rig: full clip translation on both bones.
        let off = sample_local_poses(&bones, &clip, 0, false);
        assert!(
            (off[1].w_axis.x - 99.0).abs() < 1e-4,
            "retarget off → clip translation"
        );
        // Retargeted rig: child keeps its rest offset (10, delta 0), root keeps clip motion (99).
        let on = sample_local_poses(&bones, &clip, 0, true);
        assert!(
            (on[1].w_axis.x - 10.0).abs() < 1e-4,
            "retarget on → child rest translation"
        );
        assert!(
            (on[0].w_axis.x - 99.0).abs() < 1e-4,
            "retarget on → root keeps clip motion"
        );
    }

    /// A bone that actually TRANSLATES (the pelvis's hip motion) keeps its animated delta,
    /// rebased onto THIS rig's rest — not frozen to rest, and not dragged to the source's hip
    /// height. `target_rest + (clip_T - source_rest)`.
    #[test]
    fn retarget_applies_translation_delta_for_moving_bone() {
        let bones = vec![
            Bone {
                name: "root".into(),
                parent: -1,
                local: Mat4::IDENTITY,
                inverse_bind: Mat4::IDENTITY,
            },
            Bone {
                name: "pelvis".into(),
                parent: 0,
                local: Mat4::from_translation(Vec3::new(0.0, 0.0, 95.0)), // this rig's hip height
                inverse_bind: Mat4::IDENTITY,
            },
        ];
        let key = |z: f32| Keyframe {
            t: 0,
            translation: [0.0, 0.0, z],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        // Source rest hip z=106; the clip drops it to 103 mid-stride (a -3 delta).
        let clip = ResolvedClip {
            name: "walk".into(),
            tick_rate_hz: 60,
            duration_ticks: 1,
            tracks: vec![ResolvedTrack {
                bone: 1,
                keys: vec![key(103.0)],
                source_rest: [0.0, 0.0, 106.0],
            }],
            unresolved: vec![],
        };
        let on = sample_local_poses(&bones, &clip, 0, true);
        // her hip = 95 + (103 - 106) = 92 → the stride drop preserved, at HER height (not 103).
        assert!(
            (on[1].w_axis.z - 92.0).abs() < 1e-4,
            "pelvis keeps animated delta at rig's height"
        );
    }

    #[test]
    fn blend_hits_endpoints_and_midpoint() {
        let a = Mat4::from_scale_rotation_translation(Vec3::ONE, Quat::IDENTITY, Vec3::ZERO);
        let b = Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(2.0, 0.0, 0.0),
        );
        let (_, _, t0) = blend_local_poses(&[a], &[b], 0.0)[0].to_scale_rotation_translation();
        let (_, _, t1) = blend_local_poses(&[a], &[b], 1.0)[0].to_scale_rotation_translation();
        assert!(t0.length() < 1e-5, "w=0 returns `from`");
        assert!(
            (t1 - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5,
            "w=1 returns `to`"
        );

        let (s, r, t) = blend_local_poses(&[a], &[b], 0.5)[0].to_scale_rotation_translation();
        assert!(
            (t - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5,
            "translation lerps"
        );
        assert!((s - Vec3::splat(1.5)).length() < 1e-5, "scale lerps");
        assert!(r.is_normalized(), "slerped rotation stays a unit quat");
    }

    #[test]
    fn blend_clamps_weight_no_extrapolation() {
        let a = Mat4::from_translation(Vec3::ZERO);
        let b = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let (_, _, below) = blend_local_poses(&[a], &[b], -1.0)[0].to_scale_rotation_translation();
        let (_, _, above) = blend_local_poses(&[a], &[b], 2.0)[0].to_scale_rotation_translation();
        assert!(below.length() < 1e-5, "w<0 clamps to `from`");
        assert!(
            (above - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-5,
            "w>1 clamps to `to`"
        );
    }
}
