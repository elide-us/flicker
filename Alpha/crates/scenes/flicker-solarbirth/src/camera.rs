//! The orbit camera — the holder the cinematic drives and manual control uses.
//!
//! It orbits the sun at the origin: drag to rotate, wheel to zoom. During the
//! intro it is a passive holder: the scene feeds it poses from the `.flight`
//! (`FlightPlayer`) via [`OrbitCam::set_pose`] every frame. The moment the user
//! drags, the scene flips it to `active` and [`OrbitCam::update`] takes over — the
//! pose is already in sync, so the handoff is seamless.
//!
//! (The intro's *choreography* used to live here as hardcoded `glide`/`coast`
//! functions; it now lives as authored data in `flights/intro.flight`, played by
//! the `flicker-flight` service.)

use flicker_input_core::InputState;
use flicker::render::{Camera, Vec2, Vec3};

pub struct OrbitCam {
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
    prev_mouse: Vec2,
    dragging: bool,
}

impl OrbitCam {
    pub fn new(system_outer: f32) -> Self {
        Self {
            yaw: 0.7,
            pitch: 0.55,
            distance: system_outer * 2.2,
            min_distance: system_outer * 0.15, // allow gliding right into the inner system
            max_distance: system_outer * 6.0,
            prev_mouse: Vec2::ZERO,
            dragging: false,
        }
    }

    /// Per-frame input. When `active`, drag rotates and the wheel zooms; when not
    /// (the cinematic is driving the pose), it only tracks the cursor so the next
    /// manual drag starts from the right place.
    pub fn update(&mut self, input: &InputState, active: bool) {
        let mouse = input.mouse_position;
        if active && input.mouse_left {
            if self.dragging {
                let delta = mouse - self.prev_mouse;
                self.yaw -= delta.x * 0.01;
                self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = mouse;

        if active {
            let wheel = input.mouse_wheel_delta;
            if wheel != 0.0 {
                self.distance = (self.distance * (1.0 - wheel * 0.1))
                    .clamp(self.min_distance, self.max_distance);
            }
        }
    }

    /// Set the orbit pose directly (used by the cinematic each frame). Clamped to
    /// the camera's pitch/zoom limits.
    pub fn set_pose(&mut self, yaw: f32, pitch: f32, distance: f32) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-1.5, 1.5);
        self.distance = distance.clamp(self.min_distance, self.max_distance);
    }

    pub fn camera(&self) -> Camera {
        Camera::orbit(Vec3::ZERO, self.distance, self.yaw, self.pitch)
    }
}
