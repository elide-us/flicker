//! The `flicker.rig` → runtime bridge: convert an asset's authored `collision` section into runtime
//! [`Volume`]s, and AUTO-FIT a default collision volume per bone from a skeleton (the starting point
//! the import editor hand-tunes). This is the one module that touches `flicker-skeletal`; the
//! geometry (`collision`/`drop`/`debug`) stays pure.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};

use flicker_skeletal::format::{self, Bone};

use crate::collision::{Role, Shape, Volume};

/// A default leaf-bone (no child) sphere radius, cm.
const LEAF_RADIUS_CM: f32 = 2.0;
/// Auto-fit capsule radius as a fraction of the bone's length, clamped to a sane range (cm).
const AUTOFIT_RADIUS_FRAC: f32 = 0.15;
const AUTOFIT_RADIUS_MIN: f32 = 1.0;
const AUTOFIT_RADIUS_MAX: f32 = 15.0;

/// Runtime shape from the wire `collision` shape.
pub fn shape_from_format(s: &format::CollisionShape) -> Shape {
    match *s {
        format::CollisionShape::Sphere { center, radius } => {
            Shape::Sphere { center: Vec3::from(center), radius }
        }
        format::CollisionShape::Capsule { a, b, radius } => {
            Shape::Capsule { a: Vec3::from(a), b: Vec3::from(b), radius }
        }
        format::CollisionShape::Box { center, half_extents, rotation } => Shape::Obb {
            center: Vec3::from(center),
            half_extents: Vec3::from(half_extents),
            rotation: Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]),
        },
    }
}

/// Runtime role from the wire role.
pub fn role_from_format(r: format::CollisionRole) -> Role {
    match r {
        format::CollisionRole::Physics => Role::Physics,
        format::CollisionRole::Hitbox => Role::Hitbox,
        format::CollisionRole::Attach => Role::Attach,
    }
}

/// Convert an asset's authored `collision` section into runtime volumes, resolving each volume's
/// bone NAME against `bones` (share-by-name, the format contract). A volume whose bone is absent
/// from the skeleton is dropped (not fatal — same policy as unresolved clip tracks).
pub fn volumes_from_format(collision: &format::Collision, bones: &[Bone]) -> Vec<Volume> {
    let index: HashMap<&str, usize> =
        bones.iter().enumerate().map(|(i, b)| (b.name.as_str(), i)).collect();
    collision
        .volumes
        .iter()
        .filter_map(|v| {
            let bone = *index.get(v.bone.as_str())?;
            Some(Volume::new(shape_from_format(&v.shape), bone, role_from_format(v.role)))
        })
        .collect()
}

/// Auto-fit a default collision volume for every deformable bone in a skeleton — the starting set
/// the import editor presents for hand-tuning (collision node CDDBC150: "auto-fit capsules per
/// bone"). Each bone gets a **capsule** from its origin to its farthest child's origin (the bone's
/// main segment), with a radius a fraction of that length; a leaf bone (no child) gets a small
/// **sphere**. The synthetic `root` (at the feet, parent < 0) is skipped — its child is a whole
/// body-height away, which would be one giant stray capsule. Every volume is tagged `Physics`
/// (the persistent occupancy/hurtbox role); the editor re-tags hitboxes.
pub fn autofit_capsules(bones: &[Bone]) -> Vec<Volume> {
    // A bone's `local` IS its parent-relative translation, so `child_local(c)` is `bones[c].local`.
    autofit_volumes(bones.len(), |i| bones[i].parent, |c| bones[c].local.w_axis.truncate())
}

/// Auto-fit collision volumes from plain skeleton TOPOLOGY + rest-pose GLOBAL frames — the SAME
/// algorithm as [`autofit_capsules`], for a caller that holds posed globals rather than
/// `format::Bone`. The asset-pipeline editor's overlay draws over a `RawModel`'s rest frames, which
/// are exactly `parents` + `globals` (the pair [`crate::debug`] already takes) — so it reuses this
/// rather than fabricating `format::Bone`s. One algorithm, two entry points; they differ only in
/// where each bone's parent-relative translation is read from.
pub fn autofit_capsules_from(parents: &[i32], globals: &[Mat4]) -> Vec<Volume> {
    let inv: Vec<Mat4> = globals.iter().map(|g| g.inverse()).collect();
    autofit_volumes(
        parents.len(),
        |i| parents[i],
        |c| {
            let local = match parents[c] {
                p if p < 0 => globals[c],
                p => inv[p as usize] * globals[c],
            };
            local.w_axis.truncate()
        },
    )
}

/// The shared auto-fit core. `parent(i)` = bone i's parent index (`< 0` = root, skipped).
/// `child_local(c)` = bone c's origin in its PARENT's frame (its parent-relative translation).
///
/// Each non-root bone gets a **capsule** from its origin to its farthest child's origin (the bone's
/// main segment — a multi-child bone like pelvis/spine points at its longest child, a naive default
/// the editor refines), radius a fraction of that length; a leaf bone (no child) gets a small
/// **sphere** (the joint ball). Every volume is tagged `Physics` (persistent occupancy/hurtbox); the
/// editor re-tags hitboxes.
fn autofit_volumes(
    n: usize,
    parent: impl Fn(usize) -> i32,
    child_local: impl Fn(usize) -> Vec3,
) -> Vec<Volume> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if parent(i) < 0 {
            continue; // skip the synthetic root at the feet
        }
        let farthest_child = (0..n)
            .filter(|&c| parent(c) == i as i32)
            .map(&child_local)
            .max_by(|p, q| p.length().partial_cmp(&q.length()).unwrap_or(std::cmp::Ordering::Equal));

        let shape = match farthest_child {
            Some(end) if end.length() > 1e-3 => {
                let len = end.length();
                let radius = (len * AUTOFIT_RADIUS_FRAC).clamp(AUTOFIT_RADIUS_MIN, AUTOFIT_RADIUS_MAX);
                Shape::Capsule { a: Vec3::ZERO, b: end, radius }
            }
            _ => Shape::Sphere { center: Vec3::ZERO, radius: LEAF_RADIUS_CM },
        };
        out.push(Volume::new(shape, i, Role::Physics));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;

    fn bone(name: &str, parent: i32, local: Mat4) -> Bone {
        Bone { name: name.to_string(), parent, local, inverse_bind: Mat4::IDENTITY }
    }

    #[test]
    fn format_shape_and_role_convert() {
        let s = shape_from_format(&format::CollisionShape::Capsule {
            a: [0.0, 0.0, 0.0],
            b: [0.0, 0.0, 10.0],
            radius: 2.0,
        });
        assert!(matches!(s, Shape::Capsule { radius, .. } if (radius - 2.0).abs() < 1e-6));
        assert_eq!(role_from_format(format::CollisionRole::Hitbox), Role::Hitbox);
        assert!(matches!(
            shape_from_format(&format::CollisionShape::Box {
                center: [0.0; 3],
                half_extents: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
            }),
            Shape::Obb { .. }
        ));
    }

    #[test]
    fn volumes_resolve_bone_names_and_drop_unknowns() {
        let bones = [bone("root", -1, Mat4::IDENTITY), bone("pelvis", 0, Mat4::IDENTITY)];
        let collision = format::Collision {
            volumes: vec![
                format::CollisionVolume {
                    name: "hull".into(),
                    bone: "pelvis".into(),
                    shape: format::CollisionShape::Sphere { center: [0.0; 3], radius: 8.0 },
                    role: format::CollisionRole::Physics,
                },
                format::CollisionVolume {
                    name: "ghost".into(),
                    bone: "no_such_bone".into(),
                    shape: format::CollisionShape::Sphere { center: [0.0; 3], radius: 1.0 },
                    role: format::CollisionRole::Physics,
                },
            ],
        };
        let vols = volumes_from_format(&collision, &bones);
        assert_eq!(vols.len(), 1, "the unresolved-bone volume is dropped");
        assert_eq!(vols[0].bone, 1, "resolved to the pelvis index");
    }

    #[test]
    fn autofit_makes_a_capsule_per_bone_and_skips_the_root() {
        // root(feet) → a(+z 10) → b(+z 5, leaf). a gets a capsule to b; b is a leaf sphere; root skipped.
        let bones = [
            bone("root", -1, Mat4::IDENTITY),
            bone("a", 0, Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0))),
            bone("b", 1, Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0))),
        ];
        let vols = autofit_capsules(&bones);
        assert_eq!(vols.len(), 2, "root skipped; a + b fitted");
        // bone `a` (index 1): capsule from origin to b's offset (0,0,5), radius clamped to min 1.
        let a = vols.iter().find(|v| v.bone == 1).unwrap();
        match a.shape {
            Shape::Capsule { a: s, b: e, radius } => {
                assert!(s.length() < 1e-6 && (e - Vec3::new(0.0, 0.0, 5.0)).length() < 1e-6);
                assert!((radius - 1.0).abs() < 1e-6, "5cm*0.15=0.75 clamped up to 1.0");
            }
            _ => panic!("bone a should be a capsule"),
        }
        // bone `b` (index 2) is a leaf → sphere.
        let b = vols.iter().find(|v| v.bone == 2).unwrap();
        assert!(matches!(b.shape, Shape::Sphere { radius, .. } if (radius - LEAF_RADIUS_CM).abs() < 1e-6));
    }

    /// The globals-based entry point must produce byte-identical volumes to the bone-based one — it
    /// is the SAME algorithm, and the asset-pipeline overlay relies on that equivalence. A skeleton
    /// with a MULTI-child bone and a non-trivial parent transform, so a bug in the local-from-globals
    /// derivation would show.
    #[test]
    fn autofit_from_globals_matches_autofit_from_bones() {
        // root(feet) → pelvis(+z8) → {spine(+z12), leg(+x3,-z6, longer)} ; spine → head(+z9, leaf).
        let locals = [
            (-1i32, Mat4::IDENTITY),
            (0, Mat4::from_translation(Vec3::new(0.0, 0.0, 8.0))),
            (1, Mat4::from_translation(Vec3::new(0.0, 0.0, 12.0))),
            (1, Mat4::from_translation(Vec3::new(3.0, 0.0, -6.0))),
            (2, Mat4::from_translation(Vec3::new(0.0, 0.0, 9.0))),
        ];
        let bones: Vec<Bone> = locals
            .iter()
            .enumerate()
            .map(|(i, (p, l))| bone(&format!("b{i}"), *p, *l))
            .collect();
        // Forward-kinematics the locals into rest globals — what the editor holds.
        let parents: Vec<i32> = locals.iter().map(|(p, _)| *p).collect();
        let mut globals = vec![Mat4::IDENTITY; locals.len()];
        for (i, (p, l)) in locals.iter().enumerate() {
            globals[i] = if *p < 0 { *l } else { globals[*p as usize] * *l };
        }

        let from_bones = autofit_capsules(&bones);
        let from_globals = autofit_capsules_from(&parents, &globals);
        assert_eq!(from_bones.len(), from_globals.len());
        for (a, b) in from_bones.iter().zip(&from_globals) {
            assert_eq!(a.bone, b.bone);
            assert_eq!(a.role, b.role);
            match (a.shape, b.shape) {
                (Shape::Capsule { a: a0, b: a1, radius: ar }, Shape::Capsule { a: b0, b: b1, radius: br }) => {
                    assert!((a0 - b0).length() < 1e-4 && (a1 - b1).length() < 1e-4 && (ar - br).abs() < 1e-4);
                }
                (Shape::Sphere { radius: ar, .. }, Shape::Sphere { radius: br, .. }) => {
                    assert!((ar - br).abs() < 1e-4);
                }
                _ => panic!("shape kind differs between the two entry points: {:?} vs {:?}", a.shape, b.shape),
            }
        }
        // The multi-child bone (pelvis, index 1) points at its LONGER child (leg, len ~6.7 > spine 12)
        // — sanity that both took the same farthest-child branch. (spine is longer here: 12 > 6.7.)
        let pelvis = from_globals.iter().find(|v| v.bone == 1).unwrap();
        assert!(matches!(pelvis.shape, Shape::Capsule { .. }), "multi-child pelvis is a capsule");
    }
}
