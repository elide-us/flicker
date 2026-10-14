//! MESH AND CAPSULE SUPPORTS — G3 of the gait generator (37704D6B, Aaron's ruling CD36B9BE):
//! what a climber's foot lands on when the world is not a height function. A [`MeshSupport`]
//! is any triangle soup — a tree's branches, a ledge — cast against from above the foot; a
//! [`CapsuleSupport`] is one trunk or branch segment; a [`Composite`] is the world: every
//! support asked in turn, the contact nearest the foot kept. World-agnostic like the rest of
//! the crate: the scene that owns a mesh transforms it into the generator's frame (centimetres,
//! +Z up) before building the support.
//!
//! The ray kernels live here beside the rest of the crate's pure geometry
//! ([`crate::closest_point_ray_segment`] is the segment one).

use std::cmp::Ordering;

use glam::{Mat4, Vec3};

use super::surface::{Contact, SupportKind, SurfaceQuery};
use crate::collision::closest_point_ray_segment;

/// Ray–triangle intersection (Möller–Trumbore), TWO-SIDED: the parametric `t` (> 0) along
/// `(origin, dir)` to the hit and whether the ray met the triangle's FRONT face (counter-clockwise
/// winding seen from the origin), or `None` when the ray misses, runs parallel or the hit lies
/// behind the origin. A pick keeps only the front (what the viewer sees); a support cast takes
/// either, its normal flipped to face the ray.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, bool)> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = dir.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() <= 1e-7 {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let bu = inv_det * s.dot(h);
    if !(0.0..=1.0).contains(&bu) {
        return None;
    }
    let q = s.cross(edge1);
    let bv = inv_det * dir.dot(q);
    if bv < 0.0 || bu + bv > 1.0 {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    (t > 1e-4).then_some((t, det > 0.0))
}

/// The smaller root of `qa·t² + qb·t + qc = 0` when it is positive — where a ray ENTERS a
/// quadric; a ray starting inside (or past it) gets `None`.
fn entry_root(qa: f32, qb: f32, qc: f32) -> Option<f32> {
    if qa <= 1e-12 {
        return None;
    }
    let disc = qb * qb - 4.0 * qa * qc;
    if disc < 0.0 {
        return None;
    }
    let t = (-qb - disc.sqrt()) / (2.0 * qa);
    (t > 0.0).then_some(t)
}

/// Ray–capsule intersection: the parametric `t` (> 0) along `(origin, dir)` where the ray ENTERS
/// the capsule of `radius` around the segment `a`..`b` — the cylinder body or one of the end
/// caps, whichever comes first — or `None` when it misses or starts inside.
pub fn ray_capsule(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, radius: f32) -> Option<f32> {
    let ab = b - a;
    let len = ab.length();
    let axis = if len > 1e-6 { ab / len } else { Vec3::Z };
    let m = origin - a;
    let mut best: Option<f32> = None;
    // The body: the quadratic across the axis, a hit kept only between the caps.
    if len > 1e-6 {
        let dp = dir - axis * dir.dot(axis);
        let mp = m - axis * m.dot(axis);
        if let Some(t) = entry_root(dp.dot(dp), 2.0 * mp.dot(dp), mp.dot(mp) - radius * radius) {
            let s = (m + dir * t).dot(axis);
            if (0.0..=len).contains(&s) {
                best = Some(t);
            }
        }
    }
    // The caps: a sphere hit counts only beyond its own end of the body.
    for (centre, at_b) in [(a, false), (b, true)] {
        let mc = origin - centre;
        if let Some(t) = entry_root(
            dir.dot(dir),
            2.0 * mc.dot(dir),
            mc.dot(mc) - radius * radius,
        ) {
            let s = (m + dir * t).dot(axis);
            let on_cap = if at_b { s >= len } else { s <= 0.0 };
            if on_cap && best.is_none_or(|bt| t < bt) {
                best = Some(t);
            }
        }
    }
    best
}

/// A support cast: from `reach` above `near` (along `up_hint`) straight down, a hit counting
/// within `2·reach` — the window `reach` above and below the foot's expected landing. Returns
/// the cast's origin and direction.
fn cast(near: Vec3, up_hint: Vec3, reach: f32) -> (Vec3, Vec3) {
    let up = up_hint.try_normalize().unwrap_or(Vec3::Z);
    (near + up * reach, -up)
}

/// A triangle soup a foot can land on — a tree's branches, a rock, a ledge — of one
/// [`SupportKind`]. Positions are in the generator's frame (centimetres, +Z up); the owner
/// transforms a prop's mesh into place with [`MeshSupport::transformed`].
#[derive(Clone, Debug, PartialEq)]
pub struct MeshSupport {
    pub positions: Vec<Vec3>,
    /// Triangle list, three indices per face.
    pub indices: Vec<u32>,
    pub kind: SupportKind,
}

impl MeshSupport {
    pub fn new(positions: Vec<Vec3>, indices: Vec<u32>, kind: SupportKind) -> Self {
        Self {
            positions,
            indices,
            kind,
        }
    }

    /// The same mesh carried by `placement` (a prop stood in the world).
    pub fn transformed(&self, placement: Mat4) -> Self {
        Self {
            positions: self
                .positions
                .iter()
                .map(|p| placement.transform_point3(*p))
                .collect(),
            indices: self.indices.clone(),
            kind: self.kind,
        }
    }

    /// The faces, as position triples.
    pub fn triangles(&self) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        self.indices.as_chunks::<3>().0.iter().filter_map(|tri| {
            Some([
                *self.positions.get(tri[0] as usize)?,
                *self.positions.get(tri[1] as usize)?,
                *self.positions.get(tri[2] as usize)?,
            ])
        })
    }
}

impl SurfaceQuery for MeshSupport {
    fn support(&self, near: Vec3, up_hint: Vec3, reach: f32) -> Option<Contact> {
        let (origin, dir) = cast(near, up_hint, reach);
        let mut best: Option<(f32, Vec3)> = None;
        for [a, b, c] in self.triangles() {
            if let Some((t, _)) = ray_triangle(origin, dir, a, b, c) {
                if t <= 2.0 * reach && best.is_none_or(|(bt, _)| t < bt) {
                    let mut normal = (b - a).cross(c - a).normalize_or_zero();
                    if normal.dot(dir) > 0.0 {
                        normal = -normal; // face the ray, whichever way the face winds
                    }
                    best = Some((t, normal));
                }
            }
        }
        best.map(|(t, normal)| Contact {
            point: origin + dir * t,
            normal,
            kind: self.kind,
        })
    }
}

/// One trunk or branch segment: the capsule of `radius` around `a`..`b`. The contact is where a
/// cast from above the foot meets the capsule's surface, the normal pointing out of it — a foot
/// on a branch's flank stands tilted, and a clinging body rolls to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapsuleSupport {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f32,
    pub kind: SupportKind,
}

impl SurfaceQuery for CapsuleSupport {
    fn support(&self, near: Vec3, up_hint: Vec3, reach: f32) -> Option<Contact> {
        let (origin, dir) = cast(near, up_hint, reach);
        let t = ray_capsule(origin, dir, self.a, self.b, self.radius)?;
        if t > 2.0 * reach {
            return None;
        }
        let point = origin + dir * t;
        // A zero-length ray is the point itself: the segment kernel answers the axis point.
        let (_, on_axis) = closest_point_ray_segment(point, Vec3::ZERO, self.a, self.b);
        Some(Contact {
            point,
            normal: (point - on_axis).normalize_or_zero(),
            kind: self.kind,
        })
    }
}

/// The world as a set of supports — the ground, the trees, the ledges — answering with the
/// contact nearest the foot's expected landing.
#[derive(Default)]
pub struct Composite(pub Vec<Box<dyn SurfaceQuery + Send + Sync>>);

impl Composite {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a support.
    pub fn with(mut self, support: impl SurfaceQuery + Send + Sync + 'static) -> Self {
        self.0.push(Box::new(support));
        self
    }
}

impl SurfaceQuery for Composite {
    fn support(&self, near: Vec3, up_hint: Vec3, reach: f32) -> Option<Contact> {
        self.0
            .iter()
            .filter_map(|s| s.support(near, up_hint, reach))
            .min_by(|p, q| {
                (p.point - near)
                    .length_squared()
                    .partial_cmp(&(q.point - near).length_squared())
                    .unwrap_or(Ordering::Equal)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gait::surface::FlatFloor;

    /// A ramp mesh rising toward −Y at 1:4 (two triangles over a 200 × 200 cm square).
    fn ramp() -> MeshSupport {
        let z = |y: f32| -0.25 * y;
        let positions = vec![
            Vec3::new(-100.0, 100.0, z(100.0)),
            Vec3::new(100.0, 100.0, z(100.0)),
            Vec3::new(100.0, -100.0, z(-100.0)),
            Vec3::new(-100.0, -100.0, z(-100.0)),
        ];
        MeshSupport::new(positions, vec![0, 1, 2, 0, 2, 3], SupportKind::Ledge)
    }

    /// The ray kernels: a front-face hit reports itself front, the same triangle wound the other
    /// way reports back at the same distance, a miss is `None`; a capsule is entered on its
    /// body or its cap, never from inside.
    #[test]
    fn the_ray_kernels_hit_where_they_should() {
        let (a, b, c) = (
            Vec3::new(-10.0, -10.0, 0.0),
            Vec3::new(10.0, -10.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        );
        let hit = ray_triangle(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z, a, b, c).expect("hit");
        assert!(
            (hit.0 - 5.0).abs() < 1e-5 && hit.1,
            "front face at 5: {hit:?}"
        );
        let back = ray_triangle(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z, a, c, b).expect("hit");
        assert!(
            (back.0 - 5.0).abs() < 1e-5 && !back.1,
            "back face at 5: {back:?}"
        );
        assert!(ray_triangle(Vec3::new(50.0, 0.0, 5.0), -Vec3::Z, a, b, c).is_none());
        assert!(ray_triangle(Vec3::new(0.0, 0.0, -5.0), -Vec3::Z, a, b, c).is_none());

        let (p, q, r) = (
            Vec3::new(0.0, 100.0, 0.0),
            Vec3::new(0.0, -100.0, 0.0),
            10.0,
        );
        let body = ray_capsule(Vec3::new(0.0, 0.0, 50.0), -Vec3::Z, p, q, r).expect("body");
        assert!((body - 40.0).abs() < 1e-4, "enters the top at z=10: {body}");
        let flank = ray_capsule(Vec3::new(6.0, 0.0, 50.0), -Vec3::Z, p, q, r).expect("flank");
        assert!(
            (flank - (50.0 - 8.0)).abs() < 1e-4,
            "z = √(100−36) = 8: {flank}"
        );
        let cap = ray_capsule(Vec3::new(0.0, -105.0, 50.0), -Vec3::Z, p, q, r).expect("cap");
        assert!(
            (cap - (50.0 - (100.0f32 - 25.0).sqrt())).abs() < 1e-4,
            "the cap's dome at y=−105: {cap}"
        );
        assert!(ray_capsule(Vec3::new(0.0, -120.0, 50.0), -Vec3::Z, p, q, r).is_none());
        assert!(ray_capsule(Vec3::new(20.0, 0.0, 50.0), -Vec3::Z, p, q, r).is_none());
        assert!(
            ray_capsule(Vec3::new(0.0, 0.0, 0.0), -Vec3::Z, p, q, r).is_none(),
            "inside is no entry"
        );
    }

    /// A ramp mesh answers the ramp's height under the foot with the ramp's tilted normal
    /// (facing up whichever way the faces wind), only within reach, and carries its kind.
    #[test]
    fn a_ramp_mesh_returns_its_height_and_tilted_normal() {
        let ramp = ramp();
        let want = Vec3::new(0.0, 0.25, 1.0).normalize();
        let c = ramp
            .support(Vec3::new(20.0, -40.0, 8.0), Vec3::Z, 30.0)
            .expect("on the ramp");
        assert!((c.point.z - 10.0).abs() < 1e-4, "z = −0.25·(−40): {c:?}");
        assert!((c.point.x - 20.0).abs() < 1e-4 && (c.point.y + 40.0).abs() < 1e-4);
        assert!((c.normal - want).length() < 1e-4, "tilted up: {c:?}");
        assert_eq!(c.kind, SupportKind::Ledge);
        // Wound the other way, the normal still faces the foot.
        let flipped = MeshSupport::new(ramp.positions.clone(), vec![0, 2, 1, 0, 3, 2], ramp.kind);
        let f = flipped
            .support(Vec3::new(20.0, -40.0, 8.0), Vec3::Z, 30.0)
            .expect("on the ramp");
        assert!((f.normal - want).length() < 1e-4, "still up: {f:?}");
        // Too far below the foot, or off the mesh: nothing.
        assert!(ramp
            .support(Vec3::new(20.0, -40.0, 80.0), Vec3::Z, 30.0)
            .is_none());
        assert!(ramp
            .support(Vec3::new(500.0, 0.0, 0.0), Vec3::Z, 30.0)
            .is_none());
        // Stood somewhere else, the mesh answers there.
        let moved = ramp.transformed(Mat4::from_translation(Vec3::new(0.0, 0.0, 100.0)));
        let m = moved
            .support(Vec3::new(20.0, -40.0, 108.0), Vec3::Z, 30.0)
            .expect("on the raised ramp");
        assert!((m.point.z - 110.0).abs() < 1e-4);
    }

    /// A horizontal capsule answers its top with an upward normal (and its flank with an
    /// outward one), as a Branch.
    #[test]
    fn a_horizontal_capsule_returns_its_top_as_a_branch() {
        let branch = CapsuleSupport {
            a: Vec3::new(0.0, 200.0, 50.0),
            b: Vec3::new(0.0, -200.0, 50.0),
            radius: 15.0,
            kind: SupportKind::Branch,
        };
        let top = branch
            .support(Vec3::new(0.0, 30.0, 60.0), Vec3::Z, 20.0)
            .expect("the top");
        assert!(
            (top.point - Vec3::new(0.0, 30.0, 65.0)).length() < 1e-4,
            "{top:?}"
        );
        assert!((top.normal - Vec3::Z).length() < 1e-4, "{top:?}");
        assert_eq!(top.kind, SupportKind::Branch);
        let flank = branch
            .support(Vec3::new(9.0, -30.0, 60.0), Vec3::Z, 20.0)
            .expect("the flank");
        assert!((flank.point.z - (50.0 + 12.0)).abs() < 1e-4, "{flank:?}");
        assert!(
            (flank.normal - Vec3::new(9.0, 0.0, 12.0).normalize()).length() < 1e-4,
            "outward: {flank:?}"
        );
        assert!(branch
            .support(Vec3::new(30.0, 0.0, 60.0), Vec3::Z, 20.0)
            .is_none());
        assert!(
            branch
                .support(Vec3::new(0.0, 0.0, 200.0), Vec3::Z, 20.0)
                .is_none(),
            "out of reach"
        );
    }

    /// A composite answers with the support nearest the foot: over the branch the branch,
    /// beside it the floor, and nothing where neither is in reach.
    #[test]
    fn a_composite_picks_the_nearest_support() {
        let world = Composite::new()
            .with(FlatFloor { height: 0.0 })
            .with(CapsuleSupport {
                a: Vec3::new(0.0, 200.0, 20.0),
                b: Vec3::new(0.0, -200.0, 20.0),
                radius: 10.0,
                kind: SupportKind::Branch,
            });
        let on_branch = world
            .support(Vec3::new(0.0, 0.0, 28.0), Vec3::Z, 30.0)
            .expect("something");
        assert_eq!(on_branch.kind, SupportKind::Branch);
        assert!((on_branch.point.z - 30.0).abs() < 1e-4);
        let floor_under = world
            .support(Vec3::new(0.0, 0.0, 4.0), Vec3::Z, 30.0)
            .expect("something");
        assert_eq!(floor_under.kind, SupportKind::Ground, "the floor is nearer");
        let beside = world
            .support(Vec3::new(40.0, 0.0, 10.0), Vec3::Z, 30.0)
            .expect("the floor");
        assert_eq!(beside.kind, SupportKind::Ground);
        assert!(world
            .support(Vec3::new(40.0, 0.0, 300.0), Vec3::Z, 30.0)
            .is_none());
    }
}
