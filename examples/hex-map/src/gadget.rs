//! The **wheel + axis gadget**: one map's roll control and its XYZ compass as a
//! single unit. Each map structure carries one. The gadget owns the roll angle,
//! the column the map rolls about (and the south map is mirror-flipped across),
//! the wheel handle south of the map, and the compass at the map centre — so all
//! the "where does this map sit / how is it turned" state lives in one place.

use flicker::render::{Mat4, Renderer, Vec2, Vec3};

/// Half-length of each compass axis. A touch past the hex so the arrowheads
/// clear its outline.
const AXIS_LEN: f32 = 1536.0;
/// World size of the arrowhead at each axis's positive tip.
const ARROW: f32 = 96.0;

/// Radius of the roll wheel handles.
const WHEEL_RADIUS: f32 = 850.0;
/// World Z (south) of each map's roll wheel, on the map's N-S axis — south of
/// the ring-3 extent.
const RIGHT_WHEEL_Z: f32 = -8000.0;
const LEFT_WHEEL_Z: f32 = -7000.0;
/// Screen-pixel radius for hit-testing a wheel against the cursor.
const WHEEL_PICK_PX: f32 = 90.0;
/// Roll change per pixel of vertical drag (radians).
const ROLL_SENS: f32 = 0.005;
/// Wheel handle colour (a warm yellow).
const WHEEL_COLOR: [f32; 4] = [0.95, 0.85, 0.45, 1.0];

/// One map's roll wheel and compass. Built by [`WheelAxisGadget::north`] /
/// [`WheelAxisGadget::south`] with that map's column and centre.
pub struct WheelAxisGadget {
    /// Roll about the N-S axis, 0..π (always accumulated the same way; the map's
    /// turn direction comes from `roll_sign`).
    pub roll: f32,
    /// +1 for the north map (rolls one way about world Z), −1 for the
    /// record-flipped south map (rolls the opposite way, tops tilting apart).
    roll_sign: f32,
    /// World X of the column the map rolls about — and, for the south map, the
    /// axis its tiles are mirror-flipped across.
    column_x: f32,
    /// Wheel handle centre (on the map's N-S axis, south of the map).
    wheel: Vec3,
    /// Compass origin (the map centre) and whether its X is mirrored (south).
    compass_origin: Vec3,
    compass_flip: bool,
}

impl WheelAxisGadget {
    /// North/right map: rolls about world Z (`x = 0`); wheel + compass on the
    /// origin column; starts upright.
    pub fn north() -> Self {
        Self {
            roll: 0.0,
            roll_sign: 1.0,
            column_x: 0.0,
            wheel: Vec3::new(0.0, 0.0, RIGHT_WHEEL_Z),
            compass_origin: Vec3::ZERO,
            compass_flip: false,
        }
    }

    /// South/left map: record-flipped about its own column `cx`, rolling the
    /// opposite way; starts rolled to π (facing down). `compass_origin` is the
    /// reflected map centre.
    pub fn south(cx: f32, compass_origin: Vec3) -> Self {
        Self {
            roll: std::f32::consts::PI,
            roll_sign: -1.0,
            column_x: cx,
            wheel: Vec3::new(cx, 0.0, LEFT_WHEEL_Z),
            compass_origin,
            compass_flip: true,
        }
    }

    /// This gadget's map placement transform: roll about its column by the
    /// signed angle. Shared by drawing and the mouse pick so both agree.
    pub fn transform(&self) -> Mat4 {
        roll_transform(self.column_x, self.roll_sign * self.roll)
    }

    /// The map centre this gadget sits on (the compass origin) — the anchor the
    /// fence fold measures its sides' outward normals from.
    pub fn center(&self) -> Vec3 {
        self.compass_origin
    }

    /// Accumulate a vertical drag (pixels) into the roll, clamped to 0..π.
    pub fn apply_drag(&mut self, dy_pixels: f32) {
        self.roll = (self.roll + dy_pixels * ROLL_SENS).clamp(0.0, std::f32::consts::PI);
    }

    /// Whether the cursor is over this gadget's wheel handle (projected to
    /// screen).
    pub fn wheel_hit(&self, view_proj: Mat4, screen: Vec2, cursor: Vec2) -> bool {
        project_to_screen(self.wheel, view_proj, screen)
            .is_some_and(|sp| (sp - cursor).length() < WHEEL_PICK_PX)
    }

    /// Draw the compass at the map centre, carried by the map's roll.
    pub fn paint_compass(&self, renderer: &mut Renderer) {
        draw_compass(renderer, self.compass_origin, self.compass_flip, &self.transform());
    }

    /// Draw the roll wheel south of the map (its spokes track the signed roll).
    /// The wheel itself is not rolled.
    pub fn paint_wheel(&self, renderer: &mut Renderer) {
        draw_wheel(renderer, self.wheel, WHEEL_RADIUS, self.roll_sign * self.roll, WHEEL_COLOR);
    }
}

/// Draw an XYZ compass gadget at `origin`: red X, green Y, blue Z, each positive
/// half bright with a pyramid arrowhead and the negative half dim. With `flip`
/// the frame is mirrored west↔east about the N-S axis — **X points east**, Y/up
/// and Z/north unchanged — matching the X-mirrored left-chart tiles.
fn draw_compass(renderer: &mut Renderer, origin: Vec3, flip: bool, xform: &Mat4) {
    let axes = [
        (Vec3::X, [0.95, 0.30, 0.30, 1.0]),
        (Vec3::Y, [0.40, 0.90, 0.45, 1.0]),
        (Vec3::Z, [0.40, 0.62, 1.00, 1.0]),
    ];
    for (dir, color) in axes {
        let d = if flip {
            Vec3::new(-dir.x, dir.y, dir.z)
        } else {
            dir
        };
        let dim = [color[0] * 0.4, color[1] * 0.4, color[2] * 0.4, 0.7];
        let tip = origin + d * AXIS_LEN;
        let t = |p: Vec3| xform.transform_point3(p);
        renderer.draw_lines(&[(t(origin), t(origin - d * AXIS_LEN))], dim);
        renderer.draw_lines(&[(t(origin), t(tip))], color);
        for (s, e) in arrowhead(tip, d, ARROW) {
            renderer.draw_lines(&[(t(s), t(e))], color);
        }
    }
}

/// Roll transform: rotate about the vertical Z-line at world X `axis_x` by
/// `angle`. The map's N-S axis stays fixed; its east/west halves tilt up/down.
fn roll_transform(axis_x: f32, angle: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(axis_x, 0.0, 0.0))
        * Mat4::from_rotation_z(angle)
        * Mat4::from_translation(Vec3::new(-axis_x, 0.0, 0.0))
}

/// Project a world point to screen pixels (origin top-left). `None` if behind
/// the camera. Used to hit-test the roll wheels against the cursor.
fn project_to_screen(world: Vec3, view_proj: Mat4, screen: Vec2) -> Option<Vec2> {
    let clip = view_proj * world.extend(1.0);
    if clip.w <= 1e-3 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * screen.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * screen.y,
    ))
}

/// Draw a roll wheel: a vertical ring (XY plane) with three spokes whose angle
/// tracks `roll`, so dragging it visibly turns. The wheel sits on the map's
/// Z-axis (the roll axis) and is *not* itself rolled.
fn draw_wheel(renderer: &mut Renderer, centre: Vec3, radius: f32, roll: f32, color: [f32; 4]) {
    use std::f32::consts::TAU;
    const SEGS: usize = 32;
    let rim: Vec<(Vec3, Vec3)> = (0..SEGS)
        .map(|i| {
            let a0 = i as f32 / SEGS as f32 * TAU;
            let a1 = (i + 1) as f32 / SEGS as f32 * TAU;
            (
                centre + Vec3::new(radius * a0.cos(), radius * a0.sin(), 0.0),
                centre + Vec3::new(radius * a1.cos(), radius * a1.sin(), 0.0),
            )
        })
        .collect();
    renderer.draw_lines(&rim, color);
    for k in 0..3 {
        let a = roll + k as f32 * TAU / 3.0;
        let tip = centre + Vec3::new(radius * a.cos(), radius * a.sin(), 0.0);
        renderer.draw_lines(&[(centre, tip)], color);
    }
}

/// Four short segments forming a pyramid arrowhead at `tip`, opening back along
/// `-dir`. `dir` must be unit length.
fn arrowhead(tip: Vec3, dir: Vec3, size: f32) -> [(Vec3, Vec3); 4] {
    // Any axis not parallel to `dir` seeds the perpendicular basis.
    let seed = if dir.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let p1 = dir.cross(seed).normalize_or_zero();
    let p2 = dir.cross(p1).normalize_or_zero();
    let base = tip - dir * size;
    [
        (tip, base + p1 * size * 0.5),
        (tip, base - p1 * size * 0.5),
        (tip, base + p2 * size * 0.5),
        (tip, base - p2 * size * 0.5),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex_center, left_axis_x, LEFT_AXIS_X, LEFT_MAP_STEPS};

    #[test]
    fn roll() {
        use std::f32::consts::{FRAC_PI_2, PI};

        // Roll 0 is the identity.
        let p = Vec3::new(300.0, 90.0, 50.0);
        assert!((roll_transform(0.0, 0.0).transform_point3(p) - p).length() < 1e-3);

        // Roll π about X=0 is a 180° flip: (x, y, z) → (−x, −y, z).
        let q = roll_transform(0.0, PI).transform_point3(p);
        assert!((q - Vec3::new(-300.0, -90.0, 50.0)).length() < 1e-1);

        // The maps roll OPPOSITE ways (the left map's angle is negated). At the
        // half-roll the right map's surface "up" (+Y) tilts to −X (east) and the
        // left map's to +X (west) — i.e. each tilts away from the other; their
        // bottoms meet. If they shared a sign they'd tilt the same way.
        let right_up = roll_transform(0.0, FRAC_PI_2).transform_point3(Vec3::Y);
        let left_up = roll_transform(0.0, -FRAC_PI_2).transform_point3(Vec3::Y);
        assert!(right_up.x < -0.9 && right_up.y.abs() < 0.1); // right top → east
        assert!(left_up.x > 0.9 && left_up.y.abs() < 0.1); // left top → west (opposite)
        assert!((right_up.x - left_up.x).abs() > 1.5); // genuinely opposite, not the same

        // Left map: a full roll turns the record-flipped tile 19 back upright —
        // its X lands at the un-reflected position, flat again. (±π land the same
        // place; the negation only flips the *path*, not the endpoint.)
        let lx = left_axis_x();
        let logical19 = hex_center(LEFT_MAP_STEPS[0]);
        let reflected19 = Vec3::new(2.0 * LEFT_AXIS_X - logical19.x, 0.0, logical19.z);
        let rolled = roll_transform(lx, -PI).transform_point3(reflected19);
        let upright_x = lx + (logical19.x - hex_center(LEFT_MAP_STEPS[18]).x);
        assert!((rolled.x - upright_x).abs() < 1e-1 && rolled.y.abs() < 1e-2);
    }
}
