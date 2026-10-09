//! WHAT A FOOT LANDS ON. The world implements [`SurfaceQuery`] (the island heightfield, the
//! trees' trunks and branches — G3); the generator asks it one question per landing: the nearest
//! support to a point, with its normal. Two implementations ship here for the tests and the
//! flat cases; mesh supports arrive with G3.

use glam::Vec3;

/// The kind of support a contact stands on — a climber treats a trunk or branch differently
/// from the ground (it clings), a walker only ever asks for ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportKind {
    Ground,
    Trunk,
    Branch,
    Ledge,
}

/// A support point a foot can stand on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub point: Vec3,
    /// Unit normal pointing away from the support.
    pub normal: Vec3,
    pub kind: SupportKind,
}

/// The one question the generator asks the world: the nearest support to `near` (searched
/// along `up_hint`, downward first) within `reach`, or `None` when nothing is there to stand on.
pub trait SurfaceQuery {
    fn support(&self, near: Vec3, up_hint: Vec3, reach: f32) -> Option<Contact>;
}

/// A level floor at `height` — the Controller Tester's stage, the tests' ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatFloor {
    pub height: f32,
}

impl SurfaceQuery for FlatFloor {
    fn support(&self, near: Vec3, _up_hint: Vec3, reach: f32) -> Option<Contact> {
        ((near.z - self.height).abs() <= reach).then_some(Contact {
            point: Vec3::new(near.x, near.y, self.height),
            normal: Vec3::Z,
            kind: SupportKind::Ground,
        })
    }
}

/// A height function over the plane — `height(x, y)` — such as the island's heightfield; the
/// normal is taken by central differences at `step`.
pub struct HeightFn<F: Fn(f32, f32) -> f32> {
    pub height: F,
    pub step: f32,
}

impl<F: Fn(f32, f32) -> f32> SurfaceQuery for HeightFn<F> {
    fn support(&self, near: Vec3, _up_hint: Vec3, reach: f32) -> Option<Contact> {
        let z = (self.height)(near.x, near.y);
        if (near.z - z).abs() > reach {
            return None;
        }
        let s = self.step.max(1e-3);
        let dx =
            ((self.height)(near.x + s, near.y) - (self.height)(near.x - s, near.y)) / (2.0 * s);
        let dy =
            ((self.height)(near.x, near.y + s) - (self.height)(near.x, near.y - s)) / (2.0 * s);
        Some(Contact {
            point: Vec3::new(near.x, near.y, z),
            normal: Vec3::new(-dx, -dy, 1.0).normalize_or_zero(),
            kind: SupportKind::Ground,
        })
    }
}

impl<S: SurfaceQuery + ?Sized> SurfaceQuery for &S {
    fn support(&self, near: Vec3, up_hint: Vec3, reach: f32) -> Option<Contact> {
        (**self).support(near, up_hint, reach)
    }
}
