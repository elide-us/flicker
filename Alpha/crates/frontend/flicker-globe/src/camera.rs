//! Orbit camera: drag the globe to turn it, wheel over it to zoom.
//!
//! **The gesture starts on the planet or it does not start** — and the WALKER decides
//! that, not this camera. The globe is a `surface` node; the walker hands its element a
//! [`SurfacePointer`] only while the cursor is over the planet with no UI painted over
//! it, or while a press that began there is still held (the surface capture — the
//! live-scene container's barrier, A8C9F02B §4b). So a drag that began on a panel or a
//! slider can never turn the planet however far it wanders, and this camera never
//! reads the device or a `hud_hit` flag at all.

use flicker::render::{Camera, Vec2, Vec3};
use flicker::ui::SurfacePointer;
use flicker_input_core::AbstractControls;

pub struct OrbitCam {
    radius: f32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
    /// What the orbit goes AROUND. The origin for a planet (the world is centred
    /// there); somewhere else for an inspector framing a region of it — a column
    /// standing at its true radius is orbited about its own midpoint.
    target: Vec3,
    /// The player's OWN look settings — sensitivity and the two invert flags,
    /// straight off the settings panel. The planet turns at the rate the player
    /// asked for, the same as every other camera in the app; this struct does
    /// not keep a second opinion about how fast a stick should feel.
    controls: AbstractControls,
}

impl OrbitCam {
    /// An orbit camera framing a planet of the given `radius`.
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            yaw: 0.6,
            pitch: 0.35,
            distance: radius * 3.0,
            min_distance: radius * 1.2,
            max_distance: radius * 9.0,
            target: Vec3::ZERO,
            controls: AbstractControls::default(),
        }
    }

    /// Install the player's look settings (the settings panel's live values).
    pub fn set_controls(&mut self, controls: AbstractControls) {
        self.controls = controls;
    }

    /// Aim the orbit at `target` — the point the camera goes around and looks
    /// at. The origin (the default) for a whole planet; a region's own centre
    /// for an inspector view of a piece of it.
    pub fn look_at(&mut self, target: Vec3) {
        self.target = target;
    }

    /// Re-frame in place around a region of the given `radius`: the same
    /// derivation as [`new`](Self::new) — start distance and both zoom clamps —
    /// with the pose (yaw, pitch, target) and the player's controls kept. What
    /// a view calls when the thing it frames changes size.
    pub fn set_frame(&mut self, radius: f32) {
        self.radius = radius;
        self.distance = radius * 3.0;
        self.min_distance = radius * 1.2;
        self.max_distance = radius * 9.0;
    }

    /// Open with the planet FILLING `fill` of the viewport's height (0..1):
    /// the start distance is derived from the default vertical FOV, so a
    /// square viewport shows the sphere at that fraction on entry. The sphere's
    /// apparent angular radius is `asin(R/D)`; solving for the distance where
    /// that equals `fill` of the half-FOV gives `D = R / sin(fill · fov/2)`.
    /// Zoom clamps are untouched — this frames the OPENING shot, not the range.
    pub fn with_fill(mut self, fill: f32) -> Self {
        self.refill(fill);
        self
    }

    /// [`with_fill`](Self::with_fill)'s body, callable on a live camera — what
    /// [`set_frame`](Self::set_frame) pairs with when a re-framed view also
    /// wants the framed opening shot.
    pub fn refill(&mut self, fill: f32) {
        let fov = Camera::default().fov_y_radians;
        let half = (fill.clamp(0.05, 1.0) * fov * 0.5).min(1.5);
        self.distance = (self.radius / half.sin()).clamp(self.min_distance, self.max_distance);
    }

    /// Apply this frame's POINTER SAMPLE — the walker's [`SurfacePointer`] for this
    /// globe's surface. `None` while the cursor is elsewhere or UI over the globe has it,
    /// and then nothing here moves. A left-drag orbits only while the walker holds the
    /// CAPTURE for this surface (the press began unclaimed on the planet; the drag keeps
    /// the planet even after the cursor leaves its rect — losing it mid-gesture because
    /// you overshot the panel edge would be its own bug). The wheel zooms whenever the
    /// surface is hot, so a tick meant for a scrolling column never also pulls the
    /// camera in.
    pub fn apply_pointer(&mut self, pointer: Option<&SurfacePointer>) {
        let Some(p) = pointer else { return };
        if p.captured && p.left {
            self.yaw -= p.delta.x * 0.01;
            self.pitch = (self.pitch + p.delta.y * 0.01).clamp(-1.4, 1.4);
        }
        if p.wheel != 0.0 {
            self.distance =
                (self.distance * (1.0 - p.wheel * 0.1)).clamp(self.min_distance, self.max_distance);
        }
    }

    /// Held-input orbit — the SIGNAL channel beside the mouse drag. `dx` / `dy`
    /// are axis units (−1..1; stick right and stick up positive, deadzone
    /// already applied once, by the device snapshot). Third-person standard:
    /// stick right swings the camera east around the body, stick up rises above
    /// it. The world gates WHEN this runs (the focused viewport panel owns the
    /// look signals); this method only owns the motion.
    ///
    /// The deflection goes through [`AbstractControls::look_delta_stick`] — the
    /// ONE place a stick becomes a look delta — so the settings panel's
    /// sensitivity and its two invert flags reach the planet exactly as they
    /// reach every other camera.
    pub fn orbit(&mut self, dx: f32, dy: f32, dt: f32) {
        // `look_delta_stick` reads Y in the SCREEN sense (down positive) and
        // negates it; `dy` here is stick-up positive, so it goes in flipped and
        // comes out as "stick up rises above the body" — with the player's
        // `invert_stick_pitch` still free to turn that around.
        let (yaw, pitch) = self.controls.look_delta_stick(Vec2::new(dx, -dy));
        self.yaw += yaw * ORBIT_RATE * dt;
        self.pitch = (self.pitch + pitch * ORBIT_RATE * dt).clamp(-1.4, 1.4);
    }

    /// Held-input zoom — the stick channel beside the pointer's wheel: `dz` is
    /// axis units (stick up = +1 = draw in), and a full deflection moves
    /// [`ZOOM_RATE`] of the CURRENT distance per second — exponential, so each
    /// moment of travel feels equal near the surface and far from it. Same
    /// clamps as the wheel.
    pub fn zoom(&mut self, dz: f32, dt: f32) {
        self.distance = (self.distance * (1.0 - dz * ZOOM_RATE * dt))
            .clamp(self.min_distance, self.max_distance);
    }

    pub fn camera(&self) -> Camera {
        Camera::orbit(self.target, self.distance, self.yaw, self.pitch)
    }
}

/// The orbit's UNIT SCALE: radians per second per unit of
/// [`AbstractControls::look_delta_stick`] output. At the shipped default stick
/// sensitivity (2.5) a full deflection orbits at 1.6 rad/s — brisk enough to
/// cross the visible face in under two seconds, slow enough to aim a tile — and
/// a player who raises their sensitivity raises this with it.
const ORBIT_RATE: f32 = 0.64;
/// Fraction of the current distance travelled per second at full stick
/// deflection — matches the wheel's 10%-per-tick exponential feel.
const ZOOM_RATE: f32 = 1.2;

#[cfg(test)]
mod tests {
    use super::*;

    /// The held-input channel turns the same yaw/pitch the drag turns, scaled
    /// by time, and the pitch stays inside the gimbal clamp however long the
    /// stick is held.
    #[test]
    fn orbit_turns_by_axis_time_and_clamps_pitch() {
        let mut cam = OrbitCam::new(100.0);
        let sens = AbstractControls::default().stick_sensitivity;
        let (yaw0, pitch0) = (cam.yaw, cam.pitch);
        cam.orbit(1.0, 0.0, 0.5);
        assert!(
            (cam.yaw - (yaw0 + sens * ORBIT_RATE * 0.5)).abs() < 1e-5,
            "yaw follows dx · sensitivity · rate · dt"
        );
        assert_eq!(cam.pitch, pitch0, "no dy, no pitch");
        for _ in 0..100 {
            cam.orbit(0.0, 1.0, 0.5);
        }
        assert!(cam.pitch <= 1.4, "the pitch clamp holds under a held stick");
        cam.orbit(0.0, 0.0, 0.5);
        let yaw = cam.yaw;
        cam.orbit(0.0, 0.0, 0.5);
        assert_eq!(cam.yaw, yaw, "a centred stick moves nothing");
    }

    /// **The player's look settings reach the planet.** Two different
    /// [`AbstractControls`] give two different orbits from the identical
    /// deflection — a raised sensitivity turns further, and `invert_stick_yaw`
    /// turns the other way. The globe camera once carried its own private rate
    /// and the settings panel's sliders simply did not apply to it.
    #[test]
    fn look_sensitivity_and_invert_reach_the_camera() {
        let turn = |c: AbstractControls| {
            let mut cam = OrbitCam::new(100.0);
            let y0 = cam.yaw;
            cam.set_controls(c);
            cam.orbit(1.0, 0.0, 0.5);
            cam.yaw - y0
        };
        let base = turn(AbstractControls::default());
        let fast = turn(AbstractControls {
            stick_sensitivity: AbstractControls::default().stick_sensitivity * 2.0,
            ..AbstractControls::default()
        });
        assert!(
            (fast - base * 2.0).abs() < 1e-5,
            "sensitivity scales the orbit: {base} → {fast}"
        );
        let inverted = turn(AbstractControls {
            invert_stick_yaw: true,
            ..AbstractControls::default()
        });
        assert!((inverted + base).abs() < 1e-5, "invert turns the other way");

        // …and the pitch flag likewise, on the axis it owns.
        let rise = |invert: bool| {
            let mut cam = OrbitCam::new(100.0);
            let p0 = cam.pitch;
            cam.set_controls(AbstractControls {
                invert_stick_pitch: invert,
                ..AbstractControls::default()
            });
            cam.orbit(0.0, 1.0, 0.1);
            cam.pitch - p0
        };
        assert!(rise(false) > 0.0, "stick up rises above the body");
        assert!(
            (rise(true) + rise(false)).abs() < 1e-5,
            "inverted, it drops"
        );
    }

    /// `with_fill` frames the opening shot from the default FOV: at 85% of a
    /// 60° viewport the sphere sits ~2.3 radii out — measurably nearer than
    /// the plain 3-radii start — and the answer respects the zoom clamps.
    #[test]
    fn with_fill_frames_the_sphere_at_the_asked_fraction() {
        let r = 200.0;
        let cam = OrbitCam::new(r).with_fill(0.85);
        let fov = Camera::default().fov_y_radians;
        let want = r / (0.85 * fov * 0.5).sin();
        assert!(
            (cam.distance - want).abs() < 1e-3,
            "D = R / sin(fill · fov/2)"
        );
        assert!(cam.distance < r * 3.0, "nearer than the unframed start");
        assert!(cam.distance >= cam.min_distance);
        // An absurd ask still lands inside the clamps.
        let tight = OrbitCam::new(r).with_fill(5.0);
        assert!(tight.distance >= tight.min_distance);
    }

    /// **The orbit goes around what it is aimed at, at the frame it was set.**
    /// `look_at` moves the target — the camera looks there and its position
    /// rides the same offset; `set_frame` re-derives distance and both zoom
    /// clamps around a new radius while KEEPING the pose and target. The
    /// inspector contract: re-frame a column without losing where you were
    /// looking from.
    #[test]
    fn look_at_and_set_frame_move_the_orbit_not_the_pose() {
        let mut cam = OrbitCam::new(100.0);
        let base = cam.camera();
        let target = Vec3::new(0.0, 250.0, 0.0);
        cam.look_at(target);
        let aimed = cam.camera();
        assert_eq!(aimed.target, target, "the camera looks at the target");
        assert!(
            (aimed.position - (base.position + target)).length() < 1e-4,
            "the whole orbit translated with it"
        );

        let (yaw, pitch) = (cam.yaw, cam.pitch);
        cam.set_frame(10.0);
        assert_eq!(cam.distance, 30.0, "re-derived start distance");
        assert_eq!(cam.min_distance, 12.0);
        assert_eq!(cam.max_distance, 90.0);
        assert_eq!((cam.yaw, cam.pitch), (yaw, pitch), "the pose is kept");
        assert_eq!(cam.camera().target, target, "and so is the target");
        // refill = with_fill on a live camera: the same derivation.
        cam.refill(0.85);
        let want = OrbitCam::new(10.0).with_fill(0.85).distance;
        assert!((cam.distance - want).abs() < 1e-4);
    }

    /// The stick zoom is exponential inside the same clamps as the wheel:
    /// stick up draws in, down backs out, and a held stick parks at the
    /// nearest/farthest distance instead of tunnelling through the planet.
    #[test]
    fn zoom_is_exponential_and_clamped() {
        let mut cam = OrbitCam::new(100.0);
        let d0 = cam.distance;
        cam.zoom(1.0, 0.25);
        assert!(cam.distance < d0, "stick up draws the camera in");
        cam.zoom(-1.0, 0.25);
        for _ in 0..200 {
            cam.zoom(1.0, 0.5);
        }
        assert_eq!(
            cam.distance, cam.min_distance,
            "held zoom parks at the near clamp"
        );
        for _ in 0..400 {
            cam.zoom(-1.0, 0.5);
        }
        assert_eq!(cam.distance, cam.max_distance, "and at the far clamp");
    }
}
