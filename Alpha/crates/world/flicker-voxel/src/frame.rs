//! THE GENERATOR ↔ WORLD FRAME BRIDGE. The gait generator (`flicker-mechanics::gait`) and every
//! packaged prop work in CENTIMETRES with +Z UP; the voxel world is Y-up at
//! [`CM_PER_VOXEL`] per voxel, off the engine's one world scale (`clayengine::FEET_PER_VOXEL`).
//! The ONE conversion between the two lives here, in the world crate every scene reaches, so
//! a creature walking a scene is drawn exactly where its feet were cast and no scene mints a
//! second spelling of the voxel's length (invariant 7D53A8AE, the engine-config charter).

use std::f32::consts::FRAC_PI_2;

use glam::{Mat4, Vec3};

use crate::FEET_PER_VOXEL;

/// Centimetres in a foot.
pub const CM_PER_FOOT: f32 = 30.48;

/// Centimetres per voxel — derived from [`FEET_PER_VOXEL`] (6 in), never a second number.
pub const CM_PER_VOXEL: f32 = FEET_PER_VOXEL * CM_PER_FOOT;

/// Carries the generator's frame (centimetres, +Z up) into the voxel world (voxels, +Y up):
/// the props' `Z-up → Y-up` quarter turn and the centimetre scale, in one place. A generator
/// point `(x, y, z)` lands at world `(x/s, z/s, −y/s)` with `s = CM_PER_VOXEL`.
pub fn gait_to_world() -> Mat4 {
    Mat4::from_rotation_x(-FRAC_PI_2) * Mat4::from_scale(Vec3::splat(1.0 / CM_PER_VOXEL))
}

/// The inverse of [`gait_to_world`]: a voxel-world point in the generator's frame.
pub fn world_to_gait() -> Mat4 {
    Mat4::from_scale(Vec3::splat(CM_PER_VOXEL)) * Mat4::from_rotation_x(FRAC_PI_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scale is the engine's (a 6-inch voxel is 15.24 cm), the bridge maps generator
    /// `(x, y, z)` to world `(x/s, z/s, −y/s)`, and the two matrices invert each other.
    #[test]
    fn the_bridge_is_the_engines_scale_and_inverts() {
        assert!((CM_PER_VOXEL - 15.24).abs() < 1e-5);
        let p = Vec3::new(152.4, 30.48, 304.8);
        let w = gait_to_world().transform_point3(p);
        assert!((w - Vec3::new(10.0, 20.0, -2.0)).length() < 1e-4, "{w}");
        let back = world_to_gait().transform_point3(w);
        assert!((back - p).length() < 1e-3, "{back}");
        let round = world_to_gait() * gait_to_world();
        assert!((round - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-5));
    }
}
