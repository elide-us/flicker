//! **Body capsules** — the per-bone collision volumes a baked rig carries, measured off the
//! body's own flesh.
//!
//! The runtime cloth needs something to hang OFF: a floor-length coat's front falls pass straight
//! through the thighs the moment the character walks (the duster analysis, EC30FD2E). The capsules
//! come free from the guided rig's [`Flesh`] field — the inscribed radius sampled along a bone IS
//! that limb's thickness (3995EF9E) — so nothing here is authored and nothing is new geometry.
//!
//! **It writes the rig's EXISTING `collision` contract**, not a parallel one:
//! [`CollisionVolume`] with a [`CollisionShape::Capsule`] in the bone's local frame, `role:
//! Physics` — already serde-default, already carried to the runtime on `Model.collision`, and
//! already mirrored by `flicker_mechanics::collision::Shape`. A rig baked before this reads an
//! empty `volumes` and collides with nothing.
//!
//! Only load-bearing bones get one. A finger or a twist bone contributes nothing a hem can hit and
//! would cost a per-node capsule test every frame for it (405F7034), so a candidate must be both
//! long enough and thick enough relative to the body it belongs to.

use glam::{Mat4, Vec3};

use flicker_skeletal::format::{BoneRaw, CollisionRole, CollisionShape, CollisionVolume};

use crate::fbx::RawModel;
use crate::flesh::Flesh;

/// Samples taken along a bone when reading its thickness profile.
const SAMPLES: usize = 9;
/// A bulge (a hip, a shoulder pad) must not inflate the whole limb: no half's median may exceed
/// this multiple of the bone's overall median radius.
const BULGE_CAP: f32 = 1.5;
/// A candidate bone must be at least this fraction of the body's height long…
const MIN_LEN_FRAC: f32 = 0.04;
/// …and at least this fraction thick. Together these drop fingers, toes and twist bones.
const MIN_RADIUS_FRAC: f32 = 0.02;
/// Bones that never own flesh (the same exclusions the skin bake makes): the synthesized root,
/// whose segment spans the entire lower core, and the weapon mount sockets, which hang in air.
const NEVER: [&str; 3] = ["root", "Weapon_L", "Weapon_R"];

/// One `Physics` capsule per load-bearing bone of `bones`, measured against the model's BODY
/// flesh (`Flesh::build_body`, so a mane or a garment's hanging panels are not measured as limb).
/// `bones` is the baked skeleton — indices and parents as they will be written to the rig, so the
/// synthesized root at 0 is expected. Empty for a boneless or mesh-less model.
pub fn bone_capsules(model: &RawModel, bones: &[BoneRaw]) -> Vec<CollisionVolume> {
    if bones.len() < 2 || model.vertices.is_empty() {
        return Vec::new();
    }
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for v in &model.vertices {
        lo = lo.min(Vec3::from(v.p));
        hi = hi.max(Vec3::from(v.p));
    }
    let height = (hi - lo).max_element();
    if !height.is_finite() || height <= 0.0 {
        return Vec::new();
    }
    let flesh = Flesh::build_body(model);
    // Bind-pose world frames: inverse_bind IS world.inverse(), so the bone's origin is the
    // inverse's translation and the same matrix takes a world point into the bone's local frame.
    let world: Vec<Mat4> = bones
        .iter()
        .map(|b| Mat4::from_cols_array(&b.inverse_bind).inverse())
        .collect();
    let (min_len, min_radius, search) = (
        height * MIN_LEN_FRAC,
        height * MIN_RADIUS_FRAC,
        (height * 0.10).max(2.0 * flesh.cell()),
    );

    let mut out = Vec::new();
    for (i, bone) in bones.iter().enumerate() {
        if NEVER.contains(&bone.name.as_str()) {
            continue;
        }
        // The bone's TAIL is the MEAN of its children's heads, not its first child's — a
        // first-child tail runs `pelvis→thigh_l` (sideways), which is the same skew the skin
        // bake's segment rule was hardened against.
        let kids: Vec<Vec3> = bones
            .iter()
            .enumerate()
            .filter(|(_, c)| c.parent == i as i32)
            .map(|(k, _)| world[k].w_axis.truncate())
            .collect();
        if kids.is_empty() {
            continue;
        }
        let head = world[i].w_axis.truncate();
        let tail = kids.iter().copied().sum::<Vec3>() / kids.len() as f32;
        if head.distance(tail) < min_len {
            continue;
        }
        let Some(radius) = limb_radius(&flesh, head, tail, search) else {
            continue;
        };
        if radius < min_radius {
            continue;
        }
        let to_local = Mat4::from_cols_array(&bone.inverse_bind);
        out.push(CollisionVolume {
            name: format!("flesh_{}", bone.name),
            bone: bone.name.clone(),
            shape: CollisionShape::Capsule {
                a: to_local.transform_point3(head).to_array(),
                b: to_local.transform_point3(tail).to_array(),
                radius,
            },
            role: CollisionRole::Physics,
        });
    }
    out
}

/// The bone's flesh radius: the thicker of its two halves' MEDIAN inscribed radii, each capped at
/// [`BULGE_CAP`]× the whole bone's median so one bulge cannot inflate the limb. `None` when the
/// bone reads no flesh at all (a socket hanging in air, a bone outside the body mask).
///
/// The two halves are measured separately because a limb TAPERS — a thigh is half again as thick
/// at the hip as at the knee — and the rig's capsule carries one radius, so the thicker half is
/// the conservative read: cloth stands off the thin end rather than sinking into the thick one.
fn limb_radius(flesh: &Flesh, head: Vec3, tail: Vec3, search: f32) -> Option<f32> {
    let prof = flesh.profile(head, tail, SAMPLES, search);
    let mut all: Vec<f32> = prof.iter().map(|(_, r)| *r).filter(|r| *r > 0.0).collect();
    if all.len() * 2 < SAMPLES {
        return None; // more than half the bone is outside the flesh
    }
    let mid = median(&mut all);
    if mid <= 0.0 {
        return None;
    }
    let cap = mid * BULGE_CAP;
    let half = prof.len() / 2;
    let mut first: Vec<f32> = prof[..half].iter().map(|(_, r)| *r).collect();
    let mut second: Vec<f32> = prof[half..].iter().map(|(_, r)| *r).collect();
    Some(
        median(&mut first)
            .min(cap)
            .max(median(&mut second).min(cap)),
    )
}

/// The median of `v` (it is sorted in place). 0 for an empty slice.
fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fbx::RawVertex;

    /// An axis-aligned box of `half` about `centre`, as 12 triangles of corner vertices (the
    /// one-vertex-per-corner convention `parse_fbx` emits).
    fn box_tris(centre: Vec3, half: Vec3, out: &mut Vec<RawVertex>) {
        const FACES: [[usize; 4]; 6] = [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ];
        let corner = |i: usize| {
            centre
                + Vec3::new(
                    if i & 1 == 0 { -half.x } else { half.x },
                    if i & 2 == 0 { -half.y } else { half.y },
                    if i & 4 == 0 { -half.z } else { half.z },
                )
        };
        for f in FACES {
            for [a, b, c] in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
                for i in [a, b, c] {
                    out.push(RawVertex {
                        p: corner(i).to_array(),
                        n: [0.0, 0.0, 1.0],
                        uv: [0.0, 0.0],
                        joints: [0; 4],
                        weights: [1.0, 0.0, 0.0, 0.0],
                    });
                }
            }
        }
    }

    fn bone(name: &str, parent: i32, at: Vec3) -> BoneRaw {
        BoneRaw {
            name: name.to_string(),
            parent,
            local: Mat4::IDENTITY.to_cols_array(),
            inverse_bind: Mat4::from_translation(-at).to_cols_array(),
        }
    }

    /// A 12 cm-thick, 100 cm-tall trunk with a bone down its middle: the emitted capsule must run
    /// head→tail and carry a radius near the tube's own half-width — the whole point of reading
    /// the capsule off the FLESH instead of authoring it. The leaf bone gets none (no segment),
    /// and so does `root` (it never owns flesh).
    #[test]
    fn a_limb_capsule_reads_the_tube_it_runs_down() {
        let mut verts = Vec::new();
        box_tris(
            Vec3::new(0.0, 0.0, 50.0),
            Vec3::new(6.0, 6.0, 50.0),
            &mut verts,
        );
        let model = RawModel {
            indices: (0..verts.len() as u32).collect(),
            vertices: verts,
            ..Default::default()
        };
        let bones = vec![
            bone("root", -1, Vec3::ZERO),
            bone("hip", 0, Vec3::new(0.0, 0.0, 5.0)),
            bone("knee", 1, Vec3::new(0.0, 0.0, 95.0)),
        ];
        let caps = bone_capsules(&model, &bones);
        assert_eq!(
            caps.len(),
            1,
            "only `hip` has both a child and flesh: {caps:?}"
        );
        assert_eq!(caps[0].bone, "hip");
        let CollisionShape::Capsule { a, b, radius } = caps[0].shape else {
            panic!("a limb is a capsule");
        };
        // Local frame: the bone's own origin is (0,0,0) and the tail is 90 cm up it.
        assert!(
            Vec3::from(a).length() < 1e-3,
            "the head is the bone's origin"
        );
        assert!(
            (Vec3::from(b).z - 90.0).abs() < 1e-3,
            "the tail is the child"
        );
        assert!(
            (4.0..=7.0).contains(&radius),
            "the capsule must read the 6 cm half-width of the tube it runs down, got {radius}"
        );
    }

    /// A model with no bones (a prop) and one whose only bone is a leaf both emit nothing — the
    /// rig's `collision` stays empty and every old reader is unaffected.
    #[test]
    fn a_boneless_or_leaf_only_model_emits_no_capsules() {
        let mut verts = Vec::new();
        box_tris(Vec3::ZERO, Vec3::splat(10.0), &mut verts);
        let model = RawModel {
            indices: (0..verts.len() as u32).collect(),
            vertices: verts,
            ..Default::default()
        };
        assert!(bone_capsules(&model, &[]).is_empty());
        assert!(bone_capsules(&model, &[bone("root", -1, Vec3::ZERO)]).is_empty());
    }
}
