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

use flicker::render::{Camera, Vec3};
use flicker::ui::SurfacePointer;

/// Radians of yaw/pitch per pixel of mouse RIGHT-drag — the old raw-drag feel, kept
/// now that the delta arrives as a resolved `LookRight/LookLeft` pointer signal.
const MOUSE_LOOK: f32 = 0.01;

/// This frame's resolved look input, the two channels kept apart because they
/// integrate differently. `stick_*` is a RATE the scene already multiplied by `dt`
/// (from `signal_axis`); `mouse_*` is a frame-absolute pixel DELTA (from
/// `signal_pointer_delta`, RMB-gated in the binding). The camera adds both.
#[derive(Clone, Copy, Default)]
pub struct LookDelta {
    pub stick_yaw: f32,
    pub stick_pitch: f32,
    pub mouse_dx: f32,
    pub mouse_dy: f32,
}

pub struct OrbitCam {
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
}

impl OrbitCam {
    pub fn new(system_outer: f32) -> Self {
        Self {
            yaw: 0.7,
            pitch: 0.55,
            distance: system_outer * 2.2,
            min_distance: system_outer * 0.15, // allow gliding right into the inner system
            max_distance: system_outer * 6.0,
        }
    }

    /// Per-frame input. When `active`, drag rotates and the wheel zooms; when not
    /// (the cinematic is driving the pose), it only tracks the cursor so the next
    /// manual drag starts from the right place.
    /// Apply this frame's input, gated to the 3D viewport the walker reserved.
    ///
    /// `viewport` is where the walker put the full-screen rtt this frame (`None` = not
    /// on screen → nothing moves). `look` is this frame's resolved look (stick rate +
    /// mouse RIGHT-drag delta) from the active context, `zoom` the per-frame stick-zoom
    /// step (left stick in the free `Flying` camera; `0` on the rail). Mouse look latches
    /// only on the RMB press EDGE on the OPEN sky — inside the viewport and NOT over a
    /// readout panel (`!over_hud`) — and a latched drag continues even if the pointer
    /// wanders off; stick look/zoom, not being a pointer, apply whenever `active`.
    /// `active` is false while the cinematic drives the pose (so neither look nor zoom
    /// move the camera on the rail).
    pub fn update(
        &mut self,
        pointer: Option<&SurfacePointer>,
        look: LookDelta,
        zoom: f32,
        active: bool,
    ) {
        if !active {
            return;
        }

        // Mouse look: RIGHT-drag, captured on the open sky. The MouseMotion binding
        // already gates the delta on RMB held; the walker's surface capture adds "the
        // drag must have STARTED on the open sky" (the press was unclaimed inside the
        // surface), so a right-drag begun on a panel never spins us — and the scene reads
        // no device or `hud_hit` for it (the barrier, A8C9F02B §4b).
        if pointer.is_some_and(|p| p.captured && p.right) {
            self.yaw -= look.mouse_dx * MOUSE_LOOK;
            self.pitch = (self.pitch + look.mouse_dy * MOUSE_LOOK).clamp(-1.5, 1.5);
        }

        // Stick look: a rate, live whenever active (a stick isn't a pointer, so it is
        // neither viewport- nor panel-gated).
        self.yaw -= look.stick_yaw;
        self.pitch = (self.pitch + look.stick_pitch).clamp(-1.5, 1.5);

        // Stick zoom (dolly): the free camera's left stick, same multiplicative step as
        // the wheel but not pointer-gated (a stick isn't over anything).
        if zoom != 0.0 {
            self.distance =
                (self.distance * (1.0 - zoom)).clamp(self.min_distance, self.max_distance);
        }

        // Wheel zoom only over the open sky (the surface is hot: no readout under the
        // cursor), so a tick meant for a scrolling readout never also pulls the camera in.
        if let Some(wheel) = pointer.map(|p| p.wheel).filter(|w| *w != 0.0) {
            self.distance =
                (self.distance * (1.0 - wheel * 0.1)).clamp(self.min_distance, self.max_distance);
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
