//! The analog channel types + the abstract control tuning that shapes analog
//! look/move.
//!
//! [`AnalogFrame`] / [`AnalogCache`] are the pure, std-only shell of the 120 Hz
//! volatile analog channel (spec R7 / §6.3). The **sampler** that fills the cache
//! lives in `flicker-input-device`; here we only hold the types. [`AbstractControls`]
//! (per-device sensitivity / invert / move-speed) rides along in this module —
//! it is pure `glam`, shapes the analog look, and is used by ~11 scenes, so it is
//! kept here rather than treated as legacy (spec §11.1).

use std::cell::Cell;
use std::time::{Duration, Instant};

use glam::Vec2;
use serde::{Deserialize, Serialize};

// ───────────────────────────────────────────────────────────────────
// Analog channel (120 Hz, current + previous, volatile)
// ───────────────────────────────────────────────────────────────────

/// One 120 Hz analog sample: both sticks, both triggers, a monotonic sequence
/// number, and the wall-clock instant it was captured.
///
/// `captured` is wall-clock and is used **only** for staleness ([`AnalogCache::is_stale`]);
/// it is never sim-credited (determinism lives on the discrete tick, spec §3.2a).
#[derive(Copy, Clone, Debug)]
pub struct AnalogFrame {
    pub left_stick: Vec2,
    pub right_stick: Vec2,
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// Monotonic sample counter (advances once per push).
    pub seq: u64,
    /// Wall-clock capture instant — staleness ONLY, never sim-credited.
    pub captured: Instant,
}

impl AnalogFrame {
    /// A neutral (all-zero sticks/triggers) frame captured `now`. Used to seed a
    /// cache and as the disconnect fallback (spec §6.4).
    pub fn neutral(now: Instant) -> Self {
        Self {
            left_stick: Vec2::ZERO,
            right_stick: Vec2::ZERO,
            left_trigger: 0.0,
            right_trigger: 0.0,
            seq: 0,
            captured: now,
        }
    }
}

/// Volatile current+previous analog cache. Single-threaded interior mutability
/// (`Cell`) so [`sample`](Self::sample) reads fresh through a shared `&self`
/// while the world/camera handler holds a shared borrow.
///
/// Consumers hold `&AnalogCache` and never a bare `&AnalogFrame`; every read is
/// an owned copy, and `seq` + `captured` make staleness first-class — that is how
/// the type advertises its volatility (spec §6.3).
pub struct AnalogCache {
    /// `[current, previous]`.
    frames: Cell<[AnalogFrame; 2]>,
}

impl AnalogCache {
    /// New cache seeded with `initial` in both the current and previous slots.
    pub fn new(initial: AnalogFrame) -> Self {
        Self {
            frames: Cell::new([initial, initial]),
        }
    }

    /// The latest sample — an owned COPY, taken through `&self`, so every read is
    /// fresh.
    pub fn sample(&self) -> AnalogFrame {
        self.frames.get()[0]
    }

    /// The previous sample — for velocity / flick deltas without keeping local
    /// history.
    pub fn previous(&self) -> AnalogFrame {
        self.frames.get()[1]
    }

    /// Has the latest sample aged past `max`? (Bounds a display hitch; the analog
    /// channel is never sim-credited, so this wall-clock read is safe.)
    pub fn is_stale(&self, now: Instant, max: Duration) -> bool {
        now.saturating_duration_since(self.sample().captured) > max
    }

    /// Push a fresh sample, shifting current → previous. **Sampler-only**: called
    /// by the `flicker-input-device` 120 Hz accumulator; game code never pushes.
    pub fn push(&self, f: AnalogFrame) {
        let current = self.frames.get()[0];
        self.frames.set([f, current]);
    }
}

// ───────────────────────────────────────────────────────────────────
// Abstract Controls
// ───────────────────────────────────────────────────────────────────

/// Tunable control preferences with per-device settings.
///
/// Mouse and gamepad sticks have independent sensitivity and invert
/// flags so players can configure each pointing device separately.
/// Use [`AbstractControls::look_delta_mouse`] and
/// [`AbstractControls::look_delta_stick`] to convert raw deltas into
/// `(yaw, pitch)` increments ready to add to camera angles.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AbstractControls {
    // ── Mouse section ──
    /// Look sensitivity for mouse in radians per pixel of cursor movement.
    pub mouse_sensitivity: f32,
    /// Invert vertical look for mouse: when `true`, moving the cursor
    /// up pitches the view down.
    pub invert_mouse_pitch: bool,
    /// Invert horizontal look for mouse.
    pub invert_mouse_yaw: bool,

    // ── Joystick section ──
    /// Look sensitivity for gamepad right stick in radians per
    /// unit-deflection per second.
    pub stick_sensitivity: f32,
    /// Invert vertical look for gamepad right stick.
    pub invert_stick_pitch: bool,
    /// Invert horizontal look for gamepad right stick.
    pub invert_stick_yaw: bool,

    // ── Movement section ──
    /// Movement speed in world units per second.
    pub move_speed: f32,
    /// Deadzone for gamepad sticks (0.0–1.0).
    pub stick_deadzone: f32,
}

impl Default for AbstractControls {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.005,
            invert_mouse_pitch: false,
            invert_mouse_yaw: false,
            stick_sensitivity: 2.5,
            invert_stick_pitch: false,
            invert_stick_yaw: false,
            move_speed: 120.0,
            stick_deadzone: 0.15,
        }
    }
}

impl AbstractControls {
    /// Convert a raw cursor pixel delta into a `(yaw_delta,
    /// pitch_delta)` in radians, applying mouse sensitivity and
    /// mouse invert flags.
    ///
    /// Pitch convention: positive pitch looks up. Screen Y grows
    /// downward, so a cursor moved up (negative `dy`) yields
    /// positive pitch in the non-inverted default.
    pub fn look_delta_mouse(&self, cursor_delta: Vec2) -> (f32, f32) {
        let yaw_sign = if self.invert_mouse_yaw { -1.0 } else { 1.0 };
        let pitch_sign = if self.invert_mouse_pitch { -1.0 } else { 1.0 };
        let yaw = cursor_delta.x * self.mouse_sensitivity * yaw_sign;
        let pitch = (-cursor_delta.y) * self.mouse_sensitivity * pitch_sign;
        (yaw, pitch)
    }

    /// Convert a gamepad right-stick deflection (already deadzone-
    /// filtered) into a `(yaw_delta, pitch_delta)` in radians per
    /// frame, applying stick sensitivity and stick invert flags.
    ///
    /// Multiply by `dt` if you want frame-rate-independent rotation.
    pub fn look_delta_stick(&self, stick: Vec2) -> (f32, f32) {
        let yaw_sign = if self.invert_stick_yaw { -1.0 } else { 1.0 };
        let pitch_sign = if self.invert_stick_pitch { -1.0 } else { 1.0 };
        let yaw = stick.x * self.stick_sensitivity * yaw_sign;
        let pitch = (-stick.y) * self.stick_sensitivity * pitch_sign;
        (yaw, pitch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analog_cache_shifts_current_to_previous() {
        let t = Instant::now();
        let cache = AnalogCache::new(AnalogFrame::neutral(t));
        assert_eq!(cache.sample().seq, 0);

        let mut f = AnalogFrame::neutral(t);
        f.seq = 1;
        f.left_stick = Vec2::new(0.5, 0.0);
        cache.push(f);
        assert_eq!(cache.sample().seq, 1);
        assert_eq!(cache.previous().seq, 0);
        assert_eq!(cache.sample().left_stick, Vec2::new(0.5, 0.0));
    }

    #[test]
    fn analog_cache_staleness() {
        let t = Instant::now();
        let cache = AnalogCache::new(AnalogFrame::neutral(t));
        assert!(!cache.is_stale(t, Duration::from_millis(100)));
        assert!(cache.is_stale(t + Duration::from_millis(200), Duration::from_millis(100)));
    }

    #[test]
    fn abstract_controls_mouse_defaults() {
        let ctrl = AbstractControls::default();
        let (yaw, pitch) = ctrl.look_delta_mouse(Vec2::new(10.0, 10.0));
        assert!(yaw > 0.0);
        assert!(pitch < 0.0);
    }

    #[test]
    fn abstract_controls_invert_mouse_pitch() {
        let mut ctrl = AbstractControls::default();
        let d = Vec2::new(10.0, 10.0);
        let (_, pitch0) = ctrl.look_delta_mouse(d);
        ctrl.invert_mouse_pitch = true;
        let (_, pitch1) = ctrl.look_delta_mouse(d);
        assert_eq!(pitch0, -pitch1);
    }

    #[test]
    fn abstract_controls_invert_mouse_yaw() {
        let mut ctrl = AbstractControls::default();
        let d = Vec2::new(10.0, 10.0);
        let (yaw0, _) = ctrl.look_delta_mouse(d);
        ctrl.invert_mouse_yaw = true;
        let (yaw1, _) = ctrl.look_delta_mouse(d);
        assert_eq!(yaw0, -yaw1);
    }

    #[test]
    fn abstract_controls_stick_independent_of_mouse() {
        let mut ctrl = AbstractControls::default();
        let d = Vec2::new(0.5, 0.5);
        ctrl.invert_mouse_pitch = true;
        ctrl.invert_stick_pitch = false;
        let (_, pitch_mouse) = ctrl.look_delta_mouse(d);
        let (_, pitch_stick) = ctrl.look_delta_stick(d);
        // Mouse is inverted, stick is not — they should have opposite signs
        assert!(pitch_mouse > 0.0); // inverted
        assert!(pitch_stick < 0.0); // normal
    }

    #[test]
    fn abstract_controls_stick_sensitivity() {
        let ctrl = AbstractControls {
            stick_sensitivity: 5.0,
            ..Default::default()
        };
        let (yaw, _) = ctrl.look_delta_stick(Vec2::new(1.0, 0.0));
        assert!((yaw - 5.0).abs() < 0.01);
    }
}
