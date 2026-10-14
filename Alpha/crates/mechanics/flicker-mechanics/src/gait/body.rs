//! THE BODY SOLVER: where the trunk stands, given where the feet are. The trunk's origin sits
//! over the stance contacts at its rest height (bobbing with the gait); its up is the world's
//! blended toward the support plane's normal by `cling` — a walker on level ground stays level,
//! a climber lies on its branch; its forward is the controller's heading laid into that plane.

use glam::{Mat4, Quat, Vec3};

/// The trunk's frame this tick: the ROOT bone's world placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyFrame {
    pub origin: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl BodyFrame {
    /// A frame at `origin` facing `forward` with the world's up.
    pub fn level(origin: Vec3, forward: Vec3) -> Self {
        Self::from_axes(origin, forward, Vec3::Z)
    }

    /// A frame from an up and a heading (the heading is laid into the plane of the up).
    pub fn from_axes(origin: Vec3, heading: Vec3, up: Vec3) -> Self {
        let up = up.try_normalize().unwrap_or(Vec3::Z);
        let forward = (heading - up * heading.dot(up))
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        let right = forward.cross(up).normalize_or_zero();
        Self {
            origin,
            forward,
            right,
            up,
        }
    }

    /// The matrix that carries a skeleton authored at the origin, facing the canon's −Y, into
    /// this frame.
    pub fn matrix(&self) -> Mat4 {
        // The canon faces −Y with +Z up and +X to its left: map (−Y, Z, X) → (forward, up, −right).
        let basis = glam::Mat3::from_cols(-self.right, -self.forward, self.up);
        Mat4::from_translation(self.origin) * Mat4::from_mat3(basis)
    }

    /// The rotation part alone.
    pub fn rotation(&self) -> Quat {
        Quat::from_mat4(&self.matrix())
    }
}

/// The trunk over its feet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodySolver {
    /// The root's height above the feet's contacts at rest (the recentred rig puts its root on the
    /// ground under the trunk, so this is 0 for a packaged reference).
    pub rest_height: f32,
    /// 0 = level with the world; 1 = lying on the support plane.
    pub cling: f32,
}

impl BodySolver {
    /// Stand the trunk over `contacts` (their `normals` alongside) at the controller's planar
    /// `position` and `heading`, bobbing by `bob` — the frame the whole rig is placed in. With
    /// no contacts at all the trunk stands at the rest height over `position`.
    pub fn solve(
        &self,
        contacts: &[Vec3],
        normals: &[Vec3],
        position: Vec3,
        heading: Vec3,
        bob: f32,
    ) -> BodyFrame {
        let (mean, normal) = if contacts.is_empty() {
            (position, Vec3::Z)
        } else {
            let mean = contacts.iter().copied().sum::<Vec3>() / contacts.len() as f32;
            let surface = normals
                .iter()
                .copied()
                .sum::<Vec3>()
                .try_normalize()
                .unwrap_or(Vec3::Z);
            // The plane through the contacts (least squares) gives the pitch and roll the
            // feet actually stand on; the surface normals only orient it. Fewer than three
            // spread contacts leave no plane, so the surface normal stands in.
            let normal = contact_plane(contacts, mean)
                .map(|n| if n.dot(surface) < 0.0 { -n } else { n })
                .unwrap_or(surface);
            (mean, normal)
        };
        let up = Vec3::Z.lerp(normal, self.cling.clamp(0.0, 1.0));
        let origin = Vec3::new(position.x, position.y, mean.z + self.rest_height + bob);
        BodyFrame::from_axes(origin, heading, up)
    }
}

/// The unit normal of the least-squares plane through `points` about their `mean` — the
/// eigenvector of the smallest eigenvalue of the covariance, found as the cross product of
/// the two most spread directions (a robust fit for a handful of feet). `None` when the
/// points are fewer than three or collinear.
pub fn contact_plane(points: &[Vec3], mean: Vec3) -> Option<Vec3> {
    if points.len() < 3 {
        return None;
    }
    // The longest offset from the mean, then the offset most orthogonal to it.
    let first = points
        .iter()
        .map(|p| *p - mean)
        .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))?;
    let first = first.try_normalize()?;
    let second = points
        .iter()
        .map(|p| {
            let d = *p - mean;
            d - first * d.dot(first)
        })
        .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))?;
    if second.length() < 1e-3 {
        return None;
    }
    first.cross(second).try_normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four feet on a 20° ramp: the contact plane's normal is the ramp's; three collinear
    /// feet give no plane; a level walker (cling 0) ignores it, a clinger takes it.
    #[test]
    fn the_contact_plane_reads_the_ramp_the_feet_stand_on() {
        let slope = |y: f32| -0.364 * y; // tan 20°, rising toward −Y
        let feet = [
            Vec3::new(10.0, 0.0, slope(0.0)),
            Vec3::new(-10.0, 0.0, slope(0.0)),
            Vec3::new(10.0, -40.0, slope(-40.0)),
            Vec3::new(-10.0, -40.0, slope(-40.0)),
        ];
        let mean = feet.iter().copied().sum::<Vec3>() / 4.0;
        let n = contact_plane(&feet, mean).expect("four spread feet make a plane");
        let want = Vec3::new(0.0, 0.364, 1.0).normalize();
        assert!((n.abs() - want.abs()).length() < 1e-3, "{n} vs {want}");
        let line = [Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
        assert!(
            contact_plane(&line, Vec3::X).is_none(),
            "collinear feet make no plane"
        );
        let clinger = BodySolver {
            rest_height: 0.0,
            cling: 1.0,
        };
        let f = clinger.solve(&feet, &[Vec3::Z; 4], Vec3::ZERO, -Vec3::Y, 0.0);
        assert!(
            (f.up - want).length() < 1e-3,
            "the clinger's up is the ramp's: {}",
            f.up
        );
        assert!(f.forward.z > 0.3, "and it faces up the ramp: {}", f.forward);
    }

    /// The frame stands at the mean contact height plus the rest height, faces the heading,
    /// stays level with cling 0 and lies on the support plane with cling 1; its matrix carries
    /// the canon's −Y face onto the heading.
    #[test]
    fn the_trunk_stands_over_its_feet_and_clings_when_told() {
        // Four feet on the plane z = 0.2·y, whose normal is `tilted`.
        let contacts = [
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(10.0, 40.0, 8.0),
            Vec3::new(-10.0, 40.0, 8.0),
        ];
        let tilted = Vec3::new(0.0, -0.2, 1.0).normalize();
        let normals = [tilted; 4];
        let level = BodySolver {
            rest_height: 5.0,
            cling: 0.0,
        };
        let f = level.solve(&contacts, &normals, Vec3::new(0.0, 20.0, 0.0), Vec3::X, 0.0);
        assert!(
            (f.origin.z - 9.0).abs() < 1e-4,
            "mean 4 + rest 5: {}",
            f.origin.z
        );
        assert_eq!(f.up, Vec3::Z);
        assert!((f.forward - Vec3::X).length() < 1e-5);
        let carried = f.matrix().transform_vector3(-Vec3::Y);
        assert!(
            (carried - Vec3::X).length() < 1e-5,
            "the canon's face lands on the heading"
        );
        let clinger = BodySolver {
            rest_height: 5.0,
            cling: 1.0,
        };
        let g = clinger.solve(&contacts, &normals, Vec3::ZERO, Vec3::X, 0.0);
        assert!(
            (g.up - tilted).length() < 1e-4,
            "up follows the support: {}",
            g.up
        );
        assert!(
            g.forward.dot(g.up).abs() < 1e-5,
            "forward lies in the plane"
        );
        let none = level.solve(&[], &[], Vec3::new(1.0, 2.0, 0.0), Vec3::X, 0.5);
        assert!((none.origin.z - 5.5).abs() < 1e-5);
    }
}
