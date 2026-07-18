//! Orbit camera: drag to rotate around the planet, wheel to zoom. (Salvaged from
//! the old `flicker-world` viewer — unchanged.)

use flicker::app::InputState;
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
    /// An orbit camera framing a planet of the given `radius`.
    pub fn new(radius: f32) -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.35,
            distance: radius * 3.0,
            min_distance: radius * 1.2,
            max_distance: radius * 9.0,
            prev_mouse: Vec2::ZERO,
            dragging: false,
        }
    }

    /// Apply this frame's input: left-drag rotates, wheel zooms. `allow_rotate`
    /// is `false` while the pointer is over the HUD, so clicking a widget doesn't
    /// also spin the planet (wheel-zoom still works).
    pub fn update(&mut self, input: &InputState, allow_rotate: bool) {
        let mouse = input.mouse_position;
        if input.mouse_left && allow_rotate {
            if self.dragging {
                let delta = mouse - self.prev_mouse;
                self.yaw -= delta.x * 0.01;
                self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.4, 1.4);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = mouse;

        let wheel = input.mouse_wheel_delta;
        if wheel != 0.0 {
            self.distance =
                (self.distance * (1.0 - wheel * 0.1)).clamp(self.min_distance, self.max_distance);
        }
    }

    pub fn camera(&self) -> Camera {
        Camera::orbit(Vec3::ZERO, self.distance, self.yaw, self.pitch)
    }
}
