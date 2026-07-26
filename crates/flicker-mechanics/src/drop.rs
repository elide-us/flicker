//! Item drop-to-ground — a kinematic settle, NOT a rigid-body solver.
//!
//! The physics-volume role's core behaviour (collision node CDDBC150): "a dropped item knows how to
//! fall and REST on the ground." Gravity pulls the item's collision shape straight down along world
//! −Z until its lowest support point reaches the ground plane, then it stops. No tumbling, bouncing
//! or friction — a proper rigid-body pass (ragdoll/props/destruction) is a thin Rapier/Avian
//! middleware job added only when true dynamics are wanted (memory B5E4CFC6).

use glam::Vec3;

use crate::collision::Shape;

/// Standard gravity in the rig's units: −981 cm/s² along world −Z (Z-up).
pub const GRAVITY_CM_S2: f32 = -981.0;

/// A dropped item falling straight down onto a horizontal ground plane (Z-up).
#[derive(Debug, Clone, Copy)]
pub struct FallingItem {
    /// The item's collision shape IN WORLD SPACE (its position is baked into the shape).
    pub shape: Shape,
    /// Vertical velocity (cm/s), negative while falling.
    pub vel_z: f32,
    /// True once the item has settled on the ground.
    pub resting: bool,
}

impl FallingItem {
    /// Start an item falling from rest at its current shape position.
    pub fn new(shape: Shape) -> Self {
        Self { shape, vel_z: 0.0, resting: false }
    }

    /// Advance `dt` seconds under `gravity` (cm/s², negative). When the shape's lowest support point
    /// would cross `ground_z` this step, snap it exactly onto the ground and stop. Idempotent once
    /// resting.
    pub fn step(&mut self, ground_z: f32, dt: f32, gravity: f32) {
        if self.resting {
            return;
        }
        self.vel_z += gravity * dt;
        let dz = self.vel_z * dt;
        let low = self.shape.lowest_z();
        if low + dz <= ground_z {
            self.shape = self.shape.translated(Vec3::new(0.0, 0.0, ground_z - low));
            self.vel_z = 0.0;
            self.resting = true;
        } else {
            self.shape = self.shape.translated(Vec3::new(0.0, 0.0, dz));
        }
    }

    /// Run the fall to completion (until resting) at a fixed timestep, capped so a bad call can't
    /// spin forever. Returns the number of steps taken.
    pub fn settle(&mut self, ground_z: f32, dt: f32, gravity: f32, max_steps: u32) -> u32 {
        let mut n = 0;
        while !self.resting && n < max_steps {
            self.step(ground_z, dt, gravity);
            n += 1;
        }
        n
    }
}

/// The vertical offset that places `shape` exactly resting on the ground plane at `ground_z`
/// (positive = lift up, negative = drop down): `shape.translated((0,0,offset))` sits on the ground.
pub fn settle_offset(shape: &Shape, ground_z: f32) -> f32 {
    ground_z - shape.lowest_z()
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn sphere_at(z: f32, r: f32) -> Shape {
        Shape::Sphere { center: Vec3::new(0.0, 0.0, z), radius: r }
    }

    #[test]
    fn settle_offset_rests_the_shape_on_the_plane() {
        // Sphere centre z=10 r=1 → lowest 9; ground 2 → offset 2−9 = −7 (drop 7).
        assert!((settle_offset(&sphere_at(10.0, 1.0), 2.0) + 7.0).abs() < 1e-6);
        let s = sphere_at(10.0, 1.0);
        let rested = s.translated(Vec3::new(0.0, 0.0, settle_offset(&s, 2.0)));
        assert!((rested.lowest_z() - 2.0).abs() < 1e-6, "lowest point sits on the ground");
    }

    #[test]
    fn falling_item_settles_on_the_ground() {
        // Sphere r=1 dropped from z=100 onto ground z=0 → rests with lowest_z == 0.
        let mut item = FallingItem::new(sphere_at(100.0, 1.0));
        let steps = item.settle(0.0, 1.0 / 60.0, GRAVITY_CM_S2, 10_000);
        assert!(item.resting, "item must come to rest");
        assert!(item.shape.lowest_z().abs() < 1e-3, "rests on the ground, lowest {}", item.shape.lowest_z());
        assert!(item.vel_z.abs() < 1e-9, "vertical velocity zeroed on settle");
        assert!(steps > 1 && steps < 10_000, "fell over several steps, {steps}");
    }

    #[test]
    fn an_item_at_ground_height_snaps_and_rests() {
        // Starts already resting (lowest 0 on ground 0): the first contact step keeps it put.
        let mut item = FallingItem::new(sphere_at(1.0, 1.0));
        item.step(0.0, 1.0 / 60.0, GRAVITY_CM_S2);
        assert!(item.resting);
        assert!(item.shape.lowest_z().abs() < 1e-4);
    }

    #[test]
    fn resting_item_is_idempotent() {
        let mut item = FallingItem::new(sphere_at(1.0, 1.0));
        item.settle(0.0, 1.0 / 60.0, GRAVITY_CM_S2, 100);
        let before = item.shape;
        item.step(0.0, 1.0 / 60.0, GRAVITY_CM_S2);
        assert_eq!(before, item.shape, "a resting item does not keep moving");
    }
}
