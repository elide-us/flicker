//! Orbit camera: drag the globe to turn it, wheel over it to zoom.
//!
//! **The gesture starts on the planet or it does not start.** The globe is an
//! `rtt` node on the bench now, and a styled `rtt` CLAIMS the pointer — the PiP
//! image is UI surface, not a hole through to the world — so the old
//! `!hud_hit` gate cannot work here: it would be false exactly where the globe
//! is. What replaces it is stricter and simpler to reason about: rotation
//! latches on the press EDGE inside the viewport, and a drag that began
//! anywhere else can never turn the planet however far it wanders. That is what
//! stops a slider drag from spinning the world, which is what it used to do.

use flicker::render::{Camera, Rect, Vec2, Vec3};
use flicker_input_core::InputState;

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

    /// Apply this frame's input against the globe's viewport.
    ///
    /// `viewport` is where the walker put the globe this frame; `None` means it
    /// is not on screen, and then nothing here moves. A drag continues once
    /// latched even if the pointer leaves the rect — losing the planet
    /// mid-gesture because you overshot the panel edge would be its own bug.
    pub fn update(&mut self, input: &InputState, viewport: Option<Rect>) {
        let mouse = input.mouse_position;
        let Some(rect) = viewport else {
            self.dragging = false;
            self.prev_mouse = mouse;
            return;
        };
        let inside = |p: Vec2| {
            p.x >= rect.pos.x
                && p.x <= rect.pos.x + rect.size.x
                && p.y >= rect.pos.y
                && p.y <= rect.pos.y + rect.size.y
        };

        if input.mouse_left {
            if self.dragging {
                let delta = mouse - self.prev_mouse;
                self.yaw -= delta.x * 0.01;
                self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.4, 1.4);
            } else if input.mouse_left_pressed && inside(mouse) {
                // The press EDGE, on the planet: this gesture is ours.
                self.dragging = true;
            }
        } else {
            self.dragging = false;
        }
        self.prev_mouse = mouse;

        // Zoom only over the globe, so a wheel tick meant for a scrolling
        // column never also pulls the camera in.
        let wheel = input.mouse_wheel_delta;
        if wheel != 0.0 && inside(mouse) {
            self.distance =
                (self.distance * (1.0 - wheel * 0.1)).clamp(self.min_distance, self.max_distance);
        }
    }

    pub fn camera(&self) -> Camera {
        Camera::orbit(Vec3::ZERO, self.distance, self.yaw, self.pitch)
    }
}
