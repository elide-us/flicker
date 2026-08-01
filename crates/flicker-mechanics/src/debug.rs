//! Debug-draw / inspection geometry: turn a collision [`Shape`] into wireframe segments, and turn a
//! posed skeleton into joint + bone-frame-axis segments. Renderer-agnostic — every function returns
//! `(Vec3, Vec3)` line pairs a caller feeds to its own line pipeline. The consolidated home for the
//! rig/collision OVERLAY geometry (collision volumes + the skeleton rig-view).
//!
//! Skeleton functions take plain topology (`parents[i]` = bone i's parent index, `< 0` = root) +
//! the posed bone GLOBAL matrices, so this module stays pure glam — no `flicker-skeletal` dep.

use glam::{Mat4, Vec3};

use crate::collision::Shape;

const RING_SEGMENTS: usize = 16;

/// Wireframe line segments for a shape: a sphere as three orthogonal rings, a capsule as two end
/// rings + four longitudinal lines + its axis, a box as its twelve edges.
pub fn wireframe(shape: &Shape) -> Vec<(Vec3, Vec3)> {
    match *shape {
        Shape::Sphere { center, radius } => {
            let mut s = ring(center, Vec3::X, radius);
            s.extend(ring(center, Vec3::Y, radius));
            s.extend(ring(center, Vec3::Z, radius));
            s
        }
        Shape::Capsule { a, b, radius } => {
            let axis = b - a;
            let dir = if axis.length() > 1e-6 { axis.normalize() } else { Vec3::Z };
            let (u, v) = perp_basis(dir);
            let mut s = ring(a, dir, radius);
            s.extend(ring(b, dir, radius));
            for off in [u, -u, v, -v] {
                s.push((a + off * radius, b + off * radius));
            }
            // Hemisphere DOMES on the caps — two ribs each (the u-plane and the v-plane) bulging
            // OUTWARD (−dir at `a`, +dir at `b`), so the ends read as a true pill, not open rings.
            s.extend(cap_arc(a, u, -dir, radius));
            s.extend(cap_arc(a, v, -dir, radius));
            s.extend(cap_arc(b, u, dir, radius));
            s.extend(cap_arc(b, v, dir, radius));
            s.push((a, b)); // centre axis
            s
        }
        Shape::Obb { center, half_extents, rotation } => {
            // 8 corners (all ± sign combos), 12 edges connecting corners that differ in one axis.
            let corner = |sx: f32, sy: f32, sz: f32| {
                center + rotation * (half_extents * Vec3::new(sx, sy, sz))
            };
            let c = [
                corner(-1.0, -1.0, -1.0), corner(1.0, -1.0, -1.0),
                corner(1.0, 1.0, -1.0), corner(-1.0, 1.0, -1.0),
                corner(-1.0, -1.0, 1.0), corner(1.0, -1.0, 1.0),
                corner(1.0, 1.0, 1.0), corner(-1.0, 1.0, 1.0),
            ];
            const EDGES: [(usize, usize); 12] = [
                (0, 1), (1, 2), (2, 3), (3, 0), // bottom face
                (4, 5), (5, 6), (6, 7), (7, 4), // top face
                (0, 4), (1, 5), (2, 6), (3, 7), // verticals
            ];
            EDGES.iter().map(|&(i, j)| (c[i], c[j])).collect()
        }
    }
}

/// An `n`-gon ring of `radius` about `center`, in the plane perpendicular to `axis`.
fn ring(center: Vec3, axis: Vec3, radius: f32) -> Vec<(Vec3, Vec3)> {
    let (u, v) = perp_basis(if axis.length() > 1e-6 { axis.normalize() } else { Vec3::Z });
    let point = |t: f32| center + (u * t.cos() + v * t.sin()) * radius;
    let mut segs = Vec::with_capacity(RING_SEGMENTS);
    let mut prev = point(0.0);
    for i in 1..=RING_SEGMENTS {
        let t = (i as f32) / (RING_SEGMENTS as f32) * std::f32::consts::TAU;
        let p = point(t);
        segs.push((prev, p));
        prev = p;
    }
    segs
}

/// A half-circle ARC of `radius` about `center` in the plane spanned by `side` and `out` (both
/// unit): it sweeps from `+side` up through the pole (`center + out*radius`) and back down to
/// `−side` — one rib of a hemisphere dome on a capsule cap.
fn cap_arc(center: Vec3, side: Vec3, out: Vec3, radius: f32) -> Vec<(Vec3, Vec3)> {
    let n = (RING_SEGMENTS / 2).max(1);
    let point = |t: f32| center + (side * t.cos() + out * t.sin()) * radius;
    let mut segs = Vec::with_capacity(n);
    let mut prev = point(0.0);
    for i in 1..=n {
        let t = (i as f32) / (n as f32) * std::f32::consts::PI;
        let p = point(t);
        segs.push((prev, p));
        prev = p;
    }
    segs
}

/// Two unit vectors spanning the plane perpendicular to `dir` (assumed unit-ish).
fn perp_basis(dir: Vec3) -> (Vec3, Vec3) {
    let seed = if dir.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = dir.cross(seed).normalize();
    let v = dir.cross(u).normalize();
    (u, v)
}

// ─────────────────────────────── skeleton rig-view ────────────────────────────
// `parents[i]` = bone i's parent index (`< 0` for the root). `globals[i]` = the bone's posed GLOBAL
// matrix; `world` maps the rig's source space (Z-up/cm) to engine space at draw. Plain topology in,
// so the module needs no `flicker-skeletal` dependency.

/// Parent→child JOINT wireframe: one segment from each non-root bone's parent joint to its own
/// joint (world space). The skeleton twin of [`wireframe`] — shows joint POSITIONS.
pub fn joint_segments(world: Mat4, parents: &[i32], globals: &[Mat4]) -> Vec<(Vec3, Vec3)> {
    let mut segs = Vec::with_capacity(parents.len());
    for (i, &parent) in parents.iter().enumerate() {
        if parent < 0 {
            continue;
        }
        let a = world.transform_point3(globals[parent as usize].w_axis.truncate());
        let b = world.transform_point3(globals[i].w_axis.truncate());
        segs.push((a, b));
    }
    segs
}

/// Per-bone FRAME-AXIS wireframe: from each bone's joint along its local +X (world), signed toward
/// its child and scaled to the bone length. For a correctly limb-aligned bone the axis lies ON the
/// joint segment; a bone whose frame is off its limb (an arm "hunch") shows its axis CROSSING the
/// joint line — the rig-orientation diagnostic the joint wireframe alone can't reveal. Root and leaf
/// bones (no child) are skipped.
pub fn frame_axis_segments(world: Mat4, parents: &[i32], globals: &[Mat4]) -> Vec<(Vec3, Vec3)> {
    let mut segs = Vec::with_capacity(parents.len());
    for i in 0..parents.len() {
        if parents[i] < 0 {
            continue;
        }
        let Some(child) = parents.iter().position(|&p| p == i as i32) else {
            continue; // leaf: no limb to compare against
        };
        let origin = globals[i].w_axis.truncate();
        let x_axis = globals[i].x_axis.truncate();
        let to_child = globals[child].w_axis.truncate() - origin;
        let len = to_child.length();
        if x_axis.length_squared() < 1e-12 || len <= 1e-6 {
            continue;
        }
        let dir = x_axis.normalize() * len * to_child.dot(x_axis).signum();
        segs.push((world.transform_point3(origin), world.transform_point3(origin + dir)));
    }
    segs
}

/// Parent→child OCTAHEDRAL "bone" wireframe: a diamond from each non-root bone's parent joint (head)
/// to its own joint (tail) — widest at a waist near the head, tapering to a point at the tail, the
/// DCC skeleton look. Replaces the flat parent→child [`joint_segments`] line. `waist_frac` sets both
/// where the waist square sits along the bone (from the head) and its half-size, both as a fraction
/// of the bone length, so short bones stay slim. Twelve segments per bone; root bones are skipped.
pub fn bone_diamonds(world: Mat4, parents: &[i32], globals: &[Mat4], waist_frac: f32) -> Vec<(Vec3, Vec3)> {
    let mut segs = Vec::with_capacity(parents.len() * 12);
    for (i, &parent) in parents.iter().enumerate() {
        if parent < 0 {
            continue;
        }
        let head = globals[parent as usize].w_axis.truncate();
        let tail = globals[i].w_axis.truncate();
        let axis = tail - head;
        let len = axis.length();
        if len <= 1e-6 {
            continue;
        }
        let dir = axis / len;
        let (u, v) = perp_basis(dir);
        let w = waist_frac * len;
        let waist = head + dir * (waist_frac * len);
        let ring = [
            world.transform_point3(waist + u * w),
            world.transform_point3(waist + v * w),
            world.transform_point3(waist - u * w),
            world.transform_point3(waist - v * w),
        ];
        let h = world.transform_point3(head);
        let t = world.transform_point3(tail);
        for k in 0..4 {
            segs.push((h, ring[k])); // head → waist vertex
            segs.push((ring[k], t)); // waist vertex → tail (point)
            segs.push((ring[k], ring[(k + 1) % 4])); // waist square edge
        }
    }
    segs
}

/// Per-joint ball radius for the rig view, scaled to the joint's bone LENGTH (distance to its first
/// child, or to its parent for a leaf) so extremity joints read smaller than the hips/spine, then
/// clamped to `[min_r, max_r]`. Lengths are in the rig's own units (a translation/axis-swap `world`
/// preserves them, so the caller can draw the balls under the same `world`).
pub fn joint_ball_radii(parents: &[i32], globals: &[Mat4], frac: f32, min_r: f32, max_r: f32) -> Vec<f32> {
    (0..parents.len())
        .map(|i| {
            let here = globals[i].w_axis.truncate();
            let len = parents
                .iter()
                .position(|&p| p == i as i32)
                .map(|c| (globals[c].w_axis.truncate() - here).length())
                .or_else(|| {
                    (parents[i] >= 0).then(|| {
                        (globals[parents[i] as usize].w_axis.truncate() - here).length()
                    })
                })
                .unwrap_or(0.0);
            (len * frac).clamp(min_r, max_r)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    #[test]
    fn wireframe_shapes_produce_segments_on_their_surface() {
        // Sphere: three rings of 16 segments each; every vertex is `radius` from the centre.
        let sph = wireframe(&Shape::Sphere { center: Vec3::ZERO, radius: 5.0 });
        assert_eq!(sph.len(), 3 * RING_SEGMENTS);
        for (p, q) in &sph {
            assert!((p.length() - 5.0).abs() < 1e-4 && (q.length() - 5.0).abs() < 1e-4);
        }
        // Capsule: two rings + 4 longitudinals + centre axis + 4 dome ribs (half a ring each).
        let cap = wireframe(&Shape::Capsule { a: Vec3::ZERO, b: Vec3::new(0.0, 0.0, 10.0), radius: 1.0 });
        assert_eq!(cap.len(), 2 * RING_SEGMENTS + 4 + 1 + 4 * (RING_SEGMENTS / 2));
        // The domes bulge PAST the segment ends: the bottom cap reaches its pole at z = -radius.
        assert!(
            cap.iter().any(|(p, q)| (p.z + 1.0).abs() < 1e-4 || (q.z + 1.0).abs() < 1e-4),
            "the bottom dome reaches its pole at z = -radius"
        );
        // Box: exactly 12 edges.
        let obb = wireframe(&Shape::Obb {
            center: Vec3::ZERO,
            half_extents: Vec3::splat(2.0),
            rotation: Quat::IDENTITY,
        });
        assert_eq!(obb.len(), 12);
    }

    #[test]
    fn joint_segments_link_parent_to_child_and_skip_the_root() {
        // root(0) at origin, bone 1 (child of 0) at +x10, bone 2 (child of 1) at +x15.
        let parents = [-1i32, 0, 1];
        let globals = [
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            Mat4::from_translation(Vec3::new(15.0, 0.0, 0.0)),
        ];
        let segs = joint_segments(Mat4::IDENTITY, &parents, &globals);
        assert_eq!(segs.len(), 2, "root skipped; two parent→child links");
        assert_eq!(segs[0], (Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)));
        assert_eq!(segs[1], (Vec3::new(10.0, 0.0, 0.0), Vec3::new(15.0, 0.0, 0.0)));
    }

    #[test]
    fn frame_axis_lies_on_the_limb_when_aligned_and_crosses_when_off() {
        let parents = [-1i32, 0, 1];
        // Aligned: bone 1's frame +X points +x, its child (bone 2) is at +x10 → axis reaches it.
        let aligned = [Mat4::IDENTITY, Mat4::IDENTITY, Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))];
        let a = frame_axis_segments(Mat4::IDENTITY, &parents, &aligned);
        assert_eq!(a.len(), 1, "only bone 1 has a child (root skipped, bone 2 is a leaf)");
        assert!((a[0].1 - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-4, "aligned axis reaches the child");
        // Off: bone 1's frame rotated 90° about z (its +X now points +y) → axis crosses the +x limb.
        let rot = Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let off = [Mat4::IDENTITY, rot, Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))];
        let b = frame_axis_segments(Mat4::IDENTITY, &parents, &off);
        assert!(b[0].1.y.abs() > 1.0, "off-limb axis points along y, crossing the x limb: {:?}", b[0].1);
    }

    #[test]
    fn bone_diamonds_make_twelve_edges_per_bone_and_skip_the_root() {
        let parents = [-1i32, 0, 1];
        let globals = [
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            Mat4::from_translation(Vec3::new(15.0, 0.0, 0.0)),
        ];
        let d = bone_diamonds(Mat4::IDENTITY, &parents, &globals, 0.1);
        assert_eq!(d.len(), 2 * 12, "two non-root bones, twelve edges each");
    }

    #[test]
    fn joint_ball_radii_scale_with_bone_length_and_clamp() {
        // bone 0→1 has length 10, 1→2 has length 2; a leaf falls back to its parent bone.
        let parents = [-1i32, 0, 1];
        let globals = [
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            Mat4::from_translation(Vec3::new(12.0, 0.0, 0.0)),
        ];
        let r = joint_ball_radii(&parents, &globals, 0.2, 0.1, 100.0);
        assert!(r[0] > r[1], "joint 0 heads a length-10 bone, joint 1 a length-2 bone");
        assert!((r[1] - 0.4).abs() < 1e-4, "joint 1: 2 × 0.2");
        assert!((r[2] - 0.4).abs() < 1e-4, "leaf joint 2 falls back to its parent bone (2 × 0.2)");
        // The clamp holds: a huge frac is capped at max_r.
        let capped = joint_ball_radii(&parents, &globals, 100.0, 0.1, 5.0);
        assert!(capped.iter().all(|&x| x <= 5.0));
    }
}
