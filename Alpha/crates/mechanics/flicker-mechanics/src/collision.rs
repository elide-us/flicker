//! Collision primitives + overlap queries — the pure geometry layer (golden-spec WS-D).
//!
//! Bone-bound shapes (a hitbox capsule on `Weapon_R`, a hurtbox pill on the torso, a box hull on a
//! dropped item) tagged by ROLE (physics / hitbox / attach), placed into the world by a bone's
//! pose, and tested for overlap. glam only — no animation or render deps.
//!
//! The three shapes match the `flicker.rig` `collision` section: **sphere**, **capsule**, oriented
//! **box** (Obb). A sphere is a degenerate (zero-length) capsule, so one segment-vs-segment
//! closest-distance drives every sphere/capsule overlap; box overlaps add closest-point (sphere),
//! iterative closest-point (capsule) and the separating-axis test (box-box).

use glam::{Mat3, Mat4, Quat, Vec3};

/// What a collision volume is for — the golden-spec three roles. Mirrors
/// `flicker_skeletal::format::CollisionRole`; the wire → runtime conversion is added when combat is
/// wired (kept a plain enum here so the geometry layer needs no `flicker-skeletal` dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Persistent, always-on: item-vs-world occupancy and the damageable hurtbox.
    Physics,
    /// Transient combat box, switched on/off by a TAE `HitboxActive` tick-window.
    Hitbox,
    /// An attachment point where a prop/weapon mounts.
    Attach,
}

/// A collision primitive in some frame (bone-local until placed, then world). Lengths are in the
/// frame's units — centimetres, matching the rig.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// Ball of `radius` about `center`.
    Sphere { center: Vec3, radius: f32 },
    /// Swept sphere: `radius` about every point of the segment `a`..`b`.
    Capsule { a: Vec3, b: Vec3, radius: f32 },
    /// Oriented box: `half_extents` about `center`, turned by `rotation`.
    Obb {
        center: Vec3,
        half_extents: Vec3,
        rotation: Quat,
    },
}

impl Shape {
    /// The sphere/capsule core as `(segment start, segment end, radius)` — a sphere is a zero-length
    /// capsule. `None` for a box (which is not a swept segment).
    fn segment(&self) -> Option<(Vec3, Vec3, f32)> {
        match *self {
            Shape::Sphere { center, radius } => Some((center, center, radius)),
            Shape::Capsule { a, b, radius } => Some((a, b, radius)),
            Shape::Obb { .. } => None,
        }
    }

    /// This shape transformed by a world matrix (e.g. a bone's global pose) — translation, rotation
    /// and scale together. A radius/half-extent has one meaningful scale factor, the matrix's mean
    /// axis length (exact for the uniform scale a bone pose carries).
    pub fn transformed(&self, m: Mat4) -> Shape {
        let (sx, sy, sz) = (m.x_axis.length(), m.y_axis.length(), m.z_axis.length());
        let s = ((sx + sy + sz) / 3.0).max(1e-6);
        match *self {
            Shape::Sphere { center, radius } => Shape::Sphere {
                center: m.transform_point3(center),
                radius: radius * s,
            },
            Shape::Capsule { a, b, radius } => Shape::Capsule {
                a: m.transform_point3(a),
                b: m.transform_point3(b),
                radius: radius * s,
            },
            Shape::Obb {
                center,
                half_extents,
                rotation,
            } => {
                // Extract the pose's rotation (columns normalised) and compose it with the box's own.
                let rot = Mat3::from_cols(
                    m.x_axis.truncate() / sx.max(1e-6),
                    m.y_axis.truncate() / sy.max(1e-6),
                    m.z_axis.truncate() / sz.max(1e-6),
                );
                Shape::Obb {
                    center: m.transform_point3(center),
                    half_extents: half_extents * s,
                    rotation: Quat::from_mat3(&rot) * rotation,
                }
            }
        }
    }

    /// This shape translated by `v` (no rotation/scale) — used to settle a falling item onto ground.
    pub fn translated(&self, v: Vec3) -> Shape {
        match *self {
            Shape::Sphere { center, radius } => Shape::Sphere {
                center: center + v,
                radius,
            },
            Shape::Capsule { a, b, radius } => Shape::Capsule {
                a: a + v,
                b: b + v,
                radius,
            },
            Shape::Obb {
                center,
                half_extents,
                rotation,
            } => Shape::Obb {
                center: center + v,
                half_extents,
                rotation,
            },
        }
    }

    /// Radius of a bounding sphere about the shape's centre — for broad-phase culling.
    pub fn bounding_radius(&self) -> f32 {
        match *self {
            Shape::Sphere { radius, .. } => radius,
            Shape::Capsule { a, b, radius } => a.distance(b) * 0.5 + radius,
            Shape::Obb { half_extents, .. } => half_extents.length(),
        }
    }

    /// Centre of the shape (a capsule's midpoint, the box's centre).
    pub fn center(&self) -> Vec3 {
        match *self {
            Shape::Sphere { center, .. } => center,
            Shape::Capsule { a, b, .. } => (a + b) * 0.5,
            Shape::Obb { center, .. } => center,
        }
    }

    /// The shape's lowest point along world −Z (Z-up) — its support point for resting on a ground
    /// plane. A box projects its half-extents onto Z through its rotation.
    pub fn lowest_z(&self) -> f32 {
        match *self {
            Shape::Sphere { center, radius } => center.z - radius,
            Shape::Capsule { a, b, radius } => a.z.min(b.z) - radius,
            Shape::Obb {
                center,
                half_extents,
                rotation,
            } => {
                let (ax, ay, az) = obb_axes(rotation);
                let extent_z = half_extents.x * ax.z.abs()
                    + half_extents.y * ay.z.abs()
                    + half_extents.z * az.z.abs();
                center.z - extent_z
            }
        }
    }
}

/// A bone-bound collision volume as it comes off an asset: the shape in the bone's LOCAL frame,
/// which bone it rides (an index into the pose palette), and its role. Place it in the world with
/// [`Volume::world`].
#[derive(Debug, Clone, Copy)]
pub struct Volume {
    pub shape: Shape,
    pub bone: usize,
    pub role: Role,
}

impl Volume {
    pub fn new(shape: Shape, bone: usize, role: Role) -> Self {
        Self { shape, bone, role }
    }

    /// The volume's shape in world space, given its bone's global (world) matrix. The volume
    /// follows the pose because it is expressed in the bone's frame.
    pub fn world(&self, bone_global: Mat4) -> Shape {
        self.shape.transformed(bone_global)
    }
}

/// A confirmed overlap: penetration `depth`, the separation `normal` (unit, pointing from `b`
/// toward `a` — the direction to push `a` to resolve), and a representative contact `point`.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub depth: f32,
    pub normal: Vec3,
    pub point: Vec3,
}

impl Contact {
    /// The same contact from the other shape's point of view (normal reversed).
    fn flipped(self) -> Contact {
        Contact {
            normal: -self.normal,
            ..self
        }
    }
}

/// Do two shapes overlap?
pub fn overlap(a: &Shape, b: &Shape) -> bool {
    penetration(a, b).is_some()
}

/// Overlap with contact detail, or `None` if the shapes are disjoint. The `normal` points from `b`
/// toward `a`.
pub fn penetration(a: &Shape, b: &Shape) -> Option<Contact> {
    match (a.segment(), b.segment()) {
        // sphere/capsule vs sphere/capsule — one segment-vs-segment distance covers all three pairs.
        (Some(sa), Some(sb)) => {
            let (pa, pb) = closest_points_segment_segment(sa.0, sa.1, sb.0, sb.1);
            contact_between(pa, sa.2, pb, sb.2)
        }
        // capsule/sphere vs box.
        (Some(sa), None) => segment_vs_obb(sa, b).map(Contact::flipped),
        (None, Some(sb)) => segment_vs_obb(sb, a),
        // box vs box.
        (None, None) => obb_vs_obb(a, b),
    }
}

/// Fit a capsule to a point cloud — a capsule down the cloud's longest AABB axis, radius covering
/// the cross-section. For an elongated prop (a blade, a scabbard) this reads as a pill along its
/// length; a squat cloud degenerates to a sphere. Empty input → a zero-radius sphere at the origin.
/// Used for the auto-fit of a rigid PROP (which has no skeleton to fit per-bone).
pub fn fit_capsule(points: impl IntoIterator<Item = Vec3>) -> Shape {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    if !min.is_finite() || !max.is_finite() {
        return Shape::Sphere {
            center: Vec3::ZERO,
            radius: 0.0,
        };
    }
    let center = (min + max) * 0.5;
    let half = (max - min) * 0.5; // AABB half-extents (each ≥ 0)
    let (axis, half_len) = if half.x >= half.y && half.x >= half.z {
        (Vec3::X, half.x)
    } else if half.y >= half.z {
        (Vec3::Y, half.y)
    } else {
        (Vec3::Z, half.z)
    };
    // Radius covers the wider of the two cross extents (the long-axis component zeroed out).
    let radius = (half - axis * half_len).max_element().max(1e-4);
    if half_len <= radius {
        // Not elongated → a sphere bounding the box.
        return Shape::Sphere {
            center,
            radius: half.length().max(radius),
        };
    }
    let d = axis * (half_len - radius); // inset the caps so the capsule bounds the box ends
    Shape::Capsule {
        a: center - d,
        b: center + d,
        radius,
    }
}

/// An UPRIGHT (world-Z) capsule bounding a point cloud — feet (z-min) to head (z-max) at the cloud's
/// horizontal centre, radius a fraction of the height. The "whole-unit" character movement pill for
/// simple non-IK terrain collision: it deliberately does NOT hug the arms (a controller capsule, not
/// a mesh-tight fit), so a swinging limb never changes the collision radius. `radius_frac` ≈ 0.12–0.16
/// reads as a humanoid pill. Shorter-than-wide or empty input → a sphere.
pub fn upright_capsule(points: impl IntoIterator<Item = Vec3>, radius_frac: f32) -> Shape {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    if !min.is_finite() || !max.is_finite() {
        return Shape::Sphere {
            center: Vec3::ZERO,
            radius: 0.0,
        };
    }
    let height = (max.z - min.z).max(1e-4);
    let radius = (height * radius_frac).max(1e-4);
    let cx = (min.x + max.x) * 0.5;
    let cy = (min.y + max.y) * 0.5;
    let (lo, hi) = (min.z + radius, max.z - radius);
    if hi <= lo {
        let center = Vec3::new(cx, cy, (min.z + max.z) * 0.5);
        return Shape::Sphere {
            center,
            radius: (height * 0.5).max(radius),
        };
    }
    Shape::Capsule {
        a: Vec3::new(cx, cy, lo),
        b: Vec3::new(cx, cy, hi),
        radius,
    }
}

/// Closest points between a RAY (origin `o`, direction `d`, parameter t ≥ 0) and a SEGMENT `a`..`b`
/// (parameter s ∈ [0,1]). Returns `(point on ray, point on segment)`; `d` need not be unit length.
/// Used to pick the bone (its joint segment) nearest a click-ray — the distance between the two
/// returned points is the ray's miss distance to that bone.
pub fn closest_point_ray_segment(o: Vec3, d: Vec3, a: Vec3, b: Vec3) -> (Vec3, Vec3) {
    const EPS: f32 = 1e-12;
    let ab = b - a;
    let r = o - a;
    let dd = d.dot(d); // |d|²  (ray direction; t ∈ [0, ∞))
    let ee = ab.dot(ab); // |ab|² (segment; s ∈ [0, 1])
    let c = d.dot(r); // d · (o − a)
    let f = ab.dot(r); // ab · (o − a)

    if dd <= EPS {
        // Ray has no direction → treat it as the point `o`.
        let s = if ee > EPS {
            (f / ee).clamp(0.0, 1.0)
        } else {
            0.0
        };
        return (o, a + ab * s);
    }
    if ee <= EPS {
        // Segment is a point → nearest ray parameter (t ≥ 0) to `a`.
        let t = (-c / dd).max(0.0);
        return (o + d * t, a);
    }

    let bnd = d.dot(ab);
    let denom = dd * ee - bnd * bnd; // ≥ 0
    let mut t = if denom > EPS {
        (bnd * f - c * ee) / denom
    } else {
        0.0
    };
    let mut s = (bnd * t + f) / ee;

    // Clamp s to the segment and re-solve t; then clamp t to the ray and re-solve s.
    if s < 0.0 {
        s = 0.0;
        t = -c / dd;
    } else if s > 1.0 {
        s = 1.0;
        t = (bnd - c) / dd;
    }
    if t < 0.0 {
        t = 0.0;
        s = (f / ee).clamp(0.0, 1.0);
    }
    (o + d * t, a + ab * s)
}

/// Contact from two nearest core points + their radii (from `b`'s point `pb` toward `a`'s `pa`).
fn contact_between(pa: Vec3, ra: f32, pb: Vec3, rb: f32) -> Option<Contact> {
    let delta = pa - pb;
    let dist = delta.length();
    let sep = ra + rb;
    if dist > sep {
        return None;
    }
    let normal = if dist > 1e-6 { delta / dist } else { Vec3::Z };
    Some(Contact {
        depth: sep - dist,
        normal,
        point: pb + normal * rb,
    })
}

/// Capsule/sphere (as a segment + radius) vs a box. Returns the contact with the normal pointing
/// from the BOX toward the capsule (the caller flips as needed). Alternating closest-point between
/// the segment and the box — both convex, so it converges in a few iterations.
fn segment_vs_obb(seg: (Vec3, Vec3, f32), obb: &Shape) -> Option<Contact> {
    let (a, b, r) = seg;
    let mut p = closest_point_on_segment(obb.center(), a, b);
    let mut q = closest_point_on_obb(p, obb);
    for _ in 0..8 {
        p = closest_point_on_segment(q, a, b);
        let nq = closest_point_on_obb(p, obb);
        if nq.distance_squared(q) < 1e-10 {
            q = nq;
            break;
        }
        q = nq;
    }
    contact_between(p, r, q, 0.0)
}

/// Box vs box by the separating-axis test (15 axes: 3 + 3 face normals + 9 edge cross products).
/// Reports the minimum-translation contact when they overlap.
fn obb_vs_obb(a: &Shape, b: &Shape) -> Option<Contact> {
    let (ca, ha, ra) = obb_parts(a);
    let (cb, hb, rb) = obb_parts(b);
    let (ax0, ax1, ax2) = obb_axes(ra);
    let (bx0, bx1, bx2) = obb_axes(rb);
    let a_axes = [ax0, ax1, ax2];
    let b_axes = [bx0, bx1, bx2];
    let t = cb - ca; // centre-to-centre, box A toward box B

    let mut best_depth = f32::INFINITY;
    let mut best_axis = Vec3::Z;

    let mut test = |axis: Vec3| -> Option<()> {
        let len = axis.length();
        if len < 1e-6 {
            return Some(()); // degenerate (parallel edges) — skip, not separating
        }
        let l = axis / len;
        let proj = |h: Vec3, axes: &[Vec3; 3]| {
            h.x * l.dot(axes[0]).abs() + h.y * l.dot(axes[1]).abs() + h.z * l.dot(axes[2]).abs()
        };
        let overlap = proj(ha, &a_axes) + proj(hb, &b_axes) - l.dot(t).abs();
        if overlap < 0.0 {
            return None; // a separating axis: disjoint
        }
        if overlap < best_depth {
            best_depth = overlap;
            best_axis = l;
        }
        Some(())
    };

    for &a_ax in &a_axes {
        test(a_ax)?;
    }
    for &b_ax in &b_axes {
        test(b_ax)?;
    }
    for &a_ax in &a_axes {
        for &b_ax in &b_axes {
            test(a_ax.cross(b_ax))?;
        }
    }

    // Overlap on every axis → collision. Normal points from B toward A (push A off B).
    let normal = if best_axis.dot(t) > 0.0 {
        -best_axis
    } else {
        best_axis
    };
    Some(Contact {
        depth: best_depth,
        normal,
        point: (ca + cb) * 0.5,
    })
}

/// Closest point on the box to `p`: project into the box's local frame, clamp to the half-extents,
/// project back.
fn closest_point_on_obb(p: Vec3, obb: &Shape) -> Vec3 {
    let (c, h, rot) = obb_parts(obb);
    let local = rot.conjugate() * (p - c);
    let clamped = local.clamp(-h, h);
    c + rot * clamped
}

/// Closest point on segment `a`..`b` to point `p`.
fn closest_point_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let denom = ab.dot(ab);
    if denom <= 1e-12 {
        return a; // degenerate segment (a sphere's centre)
    }
    let t = ((p - a).dot(ab) / denom).clamp(0.0, 1.0);
    a + ab * t
}

fn obb_parts(s: &Shape) -> (Vec3, Vec3, Quat) {
    match *s {
        Shape::Obb {
            center,
            half_extents,
            rotation,
        } => (center, half_extents, rotation),
        _ => unreachable!("obb_parts called on a non-box shape"),
    }
}

/// The box's three local axes in world space.
fn obb_axes(rotation: Quat) -> (Vec3, Vec3, Vec3) {
    (rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::Z)
}

/// Closest points between segments `p1..q1` and `p2..q2` (Ericson, *Real-Time Collision Detection*
/// §5.1.9). Returns `(point on segment 1, point on segment 2)`. Handles degenerate (zero-length)
/// segments, so a sphere — a capsule with `a == b` — falls out of the same routine.
fn closest_points_segment_segment(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    const EPS: f32 = 1e-9;
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);

    if a <= EPS && e <= EPS {
        return (p1, p2);
    }

    let (s, t);
    if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let s0 = if denom > EPS {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                t = t0;
                s = s0;
            }
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(c: [f32; 3], r: f32) -> Shape {
        Shape::Sphere {
            center: Vec3::from(c),
            radius: r,
        }
    }
    fn capsule(a: [f32; 3], b: [f32; 3], r: f32) -> Shape {
        Shape::Capsule {
            a: Vec3::from(a),
            b: Vec3::from(b),
            radius: r,
        }
    }
    fn obb(c: [f32; 3], h: [f32; 3], rot: Quat) -> Shape {
        Shape::Obb {
            center: Vec3::from(c),
            half_extents: Vec3::from(h),
            rotation: rot,
        }
    }

    #[test]
    fn sphere_sphere_overlap_and_gap() {
        assert!(!overlap(
            &sphere([0.0, 0.0, 0.0], 2.0),
            &sphere([5.0, 0.0, 0.0], 2.0)
        ));
        let c = penetration(&sphere([0.0, 0.0, 0.0], 3.0), &sphere([5.0, 0.0, 0.0], 3.0)).unwrap();
        assert!((c.depth - 1.0).abs() < 1e-5, "depth {}", c.depth);
        assert!(
            (c.normal - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-5,
            "normal {:?}",
            c.normal
        );
    }

    #[test]
    fn sphere_vs_capsule_uses_nearest_point_on_segment() {
        let cap = capsule([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 1.0);
        assert!(
            !overlap(&cap, &sphere([5.0, 2.5, 0.0], 1.0)),
            "centre dist 2.5 > 2 → miss"
        );
        assert!(
            overlap(&cap, &sphere([5.0, 1.5, 0.0], 1.0)),
            "centre dist 1.5 < 2 → overlap"
        );
        assert!(
            !overlap(&cap, &sphere([12.5, 0.0, 0.0], 1.0)),
            "past the end cap → miss"
        );
        assert!(
            overlap(&cap, &sphere([11.5, 0.0, 0.0], 1.0)),
            "within the end cap → overlap"
        );
    }

    #[test]
    fn capsule_capsule_crossing_and_parallel() {
        let x = capsule([-5.0, 0.0, 0.0], [5.0, 0.0, 0.0], 0.5);
        assert!(
            !overlap(&x, &capsule([0.0, -5.0, 1.5], [0.0, 5.0, 1.5], 0.5)),
            "z gap 1.5 > 1"
        );
        assert!(
            overlap(&x, &capsule([0.0, -5.0, 0.8], [0.0, 5.0, 0.8], 0.5)),
            "z gap 0.8 < 1"
        );
        let p1 = capsule([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 1.0);
        assert!(!overlap(
            &p1,
            &capsule([0.0, 3.0, 0.0], [10.0, 3.0, 0.0], 1.0)
        ));
        assert!(overlap(
            &p1,
            &capsule([0.0, 1.5, 0.0], [10.0, 1.5, 0.0], 1.0)
        ));
    }

    #[test]
    fn sphere_vs_box() {
        // Axis-aligned box, half 2 about origin. Sphere outside a face at x=3.5 r=1 → gap 0.5 miss.
        let b = obb([0.0, 0.0, 0.0], [2.0, 2.0, 2.0], Quat::IDENTITY);
        assert!(
            !overlap(&sphere([3.5, 0.0, 0.0], 1.0), &b),
            "face gap 0.5 → miss"
        );
        assert!(
            overlap(&sphere([2.5, 0.0, 0.0], 1.0), &b),
            "face gap −0.5 → overlap"
        );
        // Near a corner: closest point is the corner (2,2,2); sphere at (3,3,3) dist √3≈1.73 > 1 miss.
        assert!(
            !overlap(&sphere([3.0, 3.0, 3.0], 1.0), &b),
            "corner dist 1.73 > 1 → miss"
        );
        assert!(
            overlap(&sphere([2.8, 2.8, 2.8], 1.5), &b),
            "corner within 1.5 → overlap"
        );
    }

    #[test]
    fn box_vs_box_sat_axis_and_edge() {
        let a = obb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0], Quat::IDENTITY);
        // Face-axis separation: 3 apart on x, half 1 + 1 = 2 < 3 → disjoint; 1.5 apart → overlap.
        assert!(!overlap(
            &a,
            &obb([3.0, 0.0, 0.0], [1.0, 1.0, 1.0], Quat::IDENTITY)
        ));
        assert!(overlap(
            &a,
            &obb([1.5, 0.0, 0.0], [1.0, 1.0, 1.0], Quat::IDENTITY)
        ));
        // A 45°-about-Z box needs the EDGE cross-axes to separate correctly (its corner reaches √2).
        let spun = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        assert!(
            overlap(&a, &obb([2.2, 0.0, 0.0], [1.0, 1.0, 1.0], spun)),
            "corner √2≈1.41 reaches in"
        );
        assert!(
            !overlap(&a, &obb([2.6, 0.0, 0.0], [1.0, 1.0, 1.0], spun)),
            "past the corner → disjoint"
        );
    }

    #[test]
    fn capsule_vs_box() {
        let b = obb([0.0, 0.0, 0.0], [2.0, 2.0, 2.0], Quat::IDENTITY);
        // Capsule running along x well above the box (z=4) r=1 → 2 gap → miss; z=2.5 r=1 → overlap.
        assert!(
            !overlap(&capsule([-5.0, 0.0, 4.0], [5.0, 0.0, 4.0], 1.0), &b),
            "z gap 2 → miss"
        );
        assert!(
            overlap(&capsule([-5.0, 0.0, 2.5], [5.0, 0.0, 2.5], 1.0), &b),
            "z gap 0.5 → overlap"
        );
    }

    #[test]
    fn volume_follows_its_bone_pose() {
        let v = Volume::new(sphere([0.0, 0.0, 0.0], 1.0), 3, Role::Hitbox);
        let bone = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let world = v.world(bone);
        assert!(overlap(&world, &sphere([10.0, 0.0, 0.0], 1.0)));
        assert!(!overlap(&world, &sphere([0.0, 0.0, 0.0], 1.0)));
    }

    #[test]
    fn lowest_z_support_point() {
        assert!((sphere([0.0, 0.0, 5.0], 1.0).lowest_z() - 4.0).abs() < 1e-6);
        assert!((capsule([0.0, 0.0, 3.0], [0.0, 0.0, 7.0], 1.0).lowest_z() - 2.0).abs() < 1e-6);
        // Axis-aligned box centre z=10, half-z 2 → lowest 8.
        assert!(
            (obb([0.0, 0.0, 10.0], [2.0, 2.0, 2.0], Quat::IDENTITY).lowest_z() - 8.0).abs() < 1e-6
        );
        // Box spun 45° about X: its z-extent grows to (2+2)/√2·… — the projected half-extent is
        // hy·|Yz| + hz·|Zz| = 2·sin45 + 2·cos45 = 2√2 ≈ 2.828, so lowest = 10 − 2.828.
        let spun = obb(
            [0.0, 0.0, 10.0],
            [2.0, 2.0, 2.0],
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_4),
        );
        assert!(
            (spun.lowest_z() - (10.0 - 2.0 * std::f32::consts::SQRT_2)).abs() < 1e-4,
            "{}",
            spun.lowest_z()
        );
    }

    #[test]
    fn fit_capsule_to_an_elongated_cloud() {
        // 20 long (x) × 2 × 2 corners → capsule along x, radius ~1, caps inset by the radius.
        let pts = [
            Vec3::new(-10.0, -1.0, -1.0),
            Vec3::new(10.0, 1.0, 1.0),
            Vec3::new(-10.0, 1.0, -1.0),
            Vec3::new(10.0, -1.0, 1.0),
        ];
        match fit_capsule(pts) {
            Shape::Capsule { a, b, radius } => {
                assert!((radius - 1.0).abs() < 1e-4, "radius {radius}");
                assert!(
                    (a.x + 9.0).abs() < 1e-4 && (b.x - 9.0).abs() < 1e-4,
                    "caps inset: a {a:?} b {b:?}"
                );
                assert!(
                    a.y.abs() < 1e-4 && a.z.abs() < 1e-4,
                    "centred on the cross-section"
                );
            }
            _ => panic!("elongated cloud → capsule"),
        }
    }

    #[test]
    fn fit_capsule_squat_or_empty_is_a_sphere() {
        assert!(matches!(
            fit_capsule([Vec3::splat(-2.0), Vec3::splat(2.0)]),
            Shape::Sphere { .. }
        ));
        assert!(
            matches!(fit_capsule(std::iter::empty()), Shape::Sphere { radius, .. } if radius == 0.0)
        );
    }

    #[test]
    fn upright_capsule_is_a_vertical_pill_ignoring_arm_span() {
        // 170 tall, ±30 wide (arms out) → a VERTICAL pill: radius from the HEIGHT (≈23.8), not the
        // arm span; centred horizontally; caps inset by the radius so the ends land at feet/head.
        let pts = [Vec3::new(-30.0, -10.0, 0.0), Vec3::new(30.0, 10.0, 170.0)];
        match upright_capsule(pts, 0.14) {
            Shape::Capsule { a, b, radius } => {
                assert!(
                    (radius - 170.0 * 0.14).abs() < 1e-3,
                    "radius {radius} from height, not arms"
                );
                assert!(
                    a.x.abs() < 1e-4 && a.y.abs() < 1e-4 && b.x.abs() < 1e-4,
                    "centred horizontally"
                );
                assert!(
                    (a.z - radius).abs() < 1e-3 && (b.z - (170.0 - radius)).abs() < 1e-3,
                    "caps inset"
                );
                assert!(b.z > a.z, "runs feet → head");
            }
            _ => panic!("tall cloud → vertical capsule"),
        }
    }

    #[test]
    fn ray_segment_perpendicular_miss_distance() {
        // Ray straight down (−z) at (0,3,·); an x-axis segment through the origin. Nearest ray point
        // (0,3,0), nearest segment point (0,0,0), miss distance = the y offset = 3.
        let (pr, ps) = closest_point_ray_segment(
            Vec3::new(0.0, 3.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
        );
        assert!(
            (pr - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-3,
            "ray pt {pr:?}"
        );
        assert!((ps - Vec3::ZERO).length() < 1e-3, "seg pt {ps:?}");
        assert!(((pr - ps).length() - 3.0).abs() < 1e-3, "miss dist");
    }

    #[test]
    fn ray_segment_behind_origin_clamps_to_the_origin() {
        // Ray points AWAY from the segment (starts at z=−10, heads further −z) → nearest ray point
        // is the origin itself (t clamped to 0).
        let (pr, _) = closest_point_ray_segment(
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
        );
        assert!(
            (pr - Vec3::new(0.0, 0.0, -10.0)).length() < 1e-3,
            "clamped to ray origin, got {pr:?}"
        );
    }

    #[test]
    fn ray_segment_clamps_to_a_segment_endpoint() {
        // Ray passes beyond the segment's +x end → nearest segment point is the endpoint b=(5,0,0).
        let (_, ps) = closest_point_ray_segment(
            Vec3::new(20.0, 3.0, 10.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
        );
        assert!(
            (ps - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-3,
            "clamped to endpoint, got {ps:?}"
        );
    }
}
