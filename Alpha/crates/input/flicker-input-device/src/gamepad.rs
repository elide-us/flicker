//! The active gamepad: discrete held-buttons source + the 120 Hz analog hub.
//!
//! Single local player only — slot 0 is the active pad; any additional pads are
//! ignored (spec §6.4). The macOS (GameController) / gilrs cfg split is
//! load-bearing (ruling `A9599BF3`: gilrs's raw-HID path enumerates pads on macOS
//! but delivers no input) and is preserved here wholesale.
//!
//! One private [`GamepadBackend`] is shared by both consumers so the platform
//! event queue is drained exactly once per [`refresh`](GamepadBackend::refresh):
//! a single refresh updates the held-button bitset **and** the latest-axis cache
//! of the [`PadSnapshot`]. [`GamepadSource`] reads the buttons into the discrete
//! [`InputState`] (per frame); the analog sampler
//! ([`GamepadSource::tick_analog`]) reads the axes into the volatile
//! [`AnalogCache`]. Deadzone/sensitivity stay in core
//! ([`GamepadConfig`](flicker_input_core::GamepadConfig) /
//! `AbstractControls`); the device only normalizes sign/range (macOS is already
//! Y-up), so raw axis values flow through here (spec §6.3).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use flicker_input_core::{AnalogCache, AnalogFrame, GamepadAxis, GamepadButton, InputState};
use glam::Vec2;

use crate::DiscreteSource;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::PlatformGamepad;

#[cfg(not(target_os = "macos"))]
mod gilrs_backend;
#[cfg(not(target_os = "macos"))]
use gilrs_backend::PlatformGamepad;

// ───────────────────────────────────────────────────────────────────
// Capabilities (spec §6.4)
// ───────────────────────────────────────────────────────────────────

/// Which controls the active (slot-0) pad exposes. Lets settings / the tester
/// gray out controls a pad lacks (shared-model ruling `34BE2610`). All-false when
/// no pad is connected.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceCaps {
    pub has_left_stick: bool,
    pub has_right_stick: bool,
    pub has_triggers: bool,
    pub has_dpad: bool,
}

impl DeviceCaps {
    /// The full set — a standard extended pad (macOS `extendedGamepad`, a
    /// connected gilrs pad). Per-feature detection can refine this later; nothing
    /// consumes caps yet this slice.
    fn all() -> Self {
        Self {
            has_left_stick: true,
            has_right_stick: true,
            has_triggers: true,
            has_dpad: true,
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Shared slot-0 snapshot (filled by one refresh)
// ───────────────────────────────────────────────────────────────────

/// The maintained slot-0 state: held buttons + latest axis values + caps +
/// connection flag. Filled by a single [`GamepadBackend::refresh`] and read by
/// both the discrete pump and the analog sampler.
#[derive(Clone, Debug, Default)]
struct PadSnapshot {
    connected: bool,
    buttons: HashSet<GamepadButton>,
    axes: HashMap<GamepadAxis, f32>,
    caps: DeviceCaps,
}

impl PadSnapshot {
    /// Latest value of `axis` (0.0 if unseen). Raw — deadzone stays in core.
    fn axis(&self, axis: GamepadAxis) -> f32 {
        self.axes.get(&axis).copied().unwrap_or(0.0)
    }

    /// Clear held buttons + axes (on disconnect / on a fresh macOS re-read).
    fn reset_neutral(&mut self) {
        self.buttons.clear();
        self.axes.clear();
    }
}

/// Write the held-button state of slot 0 into the discrete snapshot.
///
/// Held **buttons** are the device's discrete output (spec §6.2). The trailing
/// axis block is the RT-12 transitional bridge (see below).
fn apply_discrete(snap: &PadSnapshot, out: &mut InputState) {
    if !snap.connected {
        out.remove_gamepad(0);
        return;
    }
    let gp = out.gamepad_mut(0);
    for &button in GamepadButton::ALL {
        gp.set_button(button, snap.buttons.contains(&button));
    }

    // ── RT-12 transitional discrete-axis bridge (spec §6.1 / risk RT-12) ──
    // The permanent home for analog axes is the 120 Hz channel above; nothing
    // reads it yet. Until the world/camera handler polls the cache (the
    // pocclusters port), keep the discrete `GamepadState` axes populated so
    // existing consumers do not regress: the controllertester pad viz and the
    // `LeftStick*/RightStick*` → Move/Look rows still live in the `InputMap`
    // presets. DELETE this block (only) when those rows migrate — the analog
    // channel already carries the same values.
    for &axis in GamepadAxis::ALL {
        gp.set_axis(axis, snap.axis(axis));
    }
}

/// Copy the latest raw axes of slot 0 into an analog frame (spec §6.3). A
/// disconnected (cleared) snapshot yields a neutral frame.
fn apply_axes(snap: &PadSnapshot, frame: &mut AnalogFrame) {
    frame.left_stick = Vec2::new(
        snap.axis(GamepadAxis::LeftStickX),
        snap.axis(GamepadAxis::LeftStickY),
    );
    frame.right_stick = Vec2::new(
        snap.axis(GamepadAxis::RightStickX),
        snap.axis(GamepadAxis::RightStickY),
    );
    frame.left_trigger = snap.axis(GamepadAxis::LeftTrigger);
    frame.right_trigger = snap.axis(GamepadAxis::RightTrigger);
}

// ───────────────────────────────────────────────────────────────────
// Backend (private): the platform reader + the maintained snapshot
// ───────────────────────────────────────────────────────────────────

/// The platform reader plus the slot-0 snapshot it maintains. `refresh` drains
/// the platform queue once; `pump_buttons` / `sample_axes` read the maintained
/// state (never the raw queue), so buttons and axes stay coherent within a frame.
struct GamepadBackend {
    platform: PlatformGamepad,
    snapshot: PadSnapshot,
}

impl GamepadBackend {
    fn new() -> Self {
        Self {
            platform: PlatformGamepad::new(),
            snapshot: PadSnapshot::default(),
        }
    }

    /// Drain the platform queue once into the maintained slot-0 snapshot.
    fn refresh(&mut self) {
        self.platform.read_into(&mut self.snapshot);
    }

    /// Held buttons (+ the RT-12 axis bridge) → discrete slot 0.
    fn pump_buttons(&self, out: &mut InputState) {
        apply_discrete(&self.snapshot, out);
    }

    /// Latest raw axes → analog frame.
    fn sample_axes(&self, frame: &mut AnalogFrame) {
        apply_axes(&self.snapshot, frame);
    }

    fn caps(&self) -> DeviceCaps {
        self.snapshot.caps
    }

    fn connected(&self) -> bool {
        self.snapshot.connected
    }
}

// ───────────────────────────────────────────────────────────────────
// GamepadSource: discrete source + analog hub
// ───────────────────────────────────────────────────────────────────

/// The active pad. Implements [`DiscreteSource`] (held buttons → snapshot) and
/// doubles as the analog hub: it owns the shared [`GamepadBackend`] and the 120 Hz
/// [`AnalogCache`], and fills the cache via [`tick_analog`](Self::tick_analog).
pub struct GamepadSource {
    backend: GamepadBackend,
    cache: AnalogCache,
    /// App-start instant, stamped on the neutral frame pushed while slot 0 is
    /// disconnected so the sample reads stale (camera holds, doesn't snap — §6.4).
    epoch: Instant,
}

impl GamepadSource {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            backend: GamepadBackend::new(),
            cache: AnalogCache::new(AnalogFrame::neutral(now)),
            epoch: now,
        }
    }

    /// Fill the analog cache from one refresh. Called once per frame this slice
    /// (frame-rate sampling — the runner keeps `ControlFlow::Poll`; true 120 Hz
    /// pacing is deferred to RT-11, since no consumer reads analog yet). On slot-0
    /// disconnect the pushed frame is neutral (cleared snapshot → zero axes) and
    /// stamped stale (spec §6.3 / §6.4).
    pub fn tick_analog(&mut self) {
        self.backend.refresh();
        let mut frame = self.cache.sample();
        self.backend.sample_axes(&mut frame);
        frame.seq = frame.seq.wrapping_add(1);
        frame.captured = if self.backend.connected() {
            Instant::now()
        } else {
            self.epoch
        };
        self.cache.push(frame);
    }

    /// The 120 Hz analog cache (current + previous). Consumers hold `&AnalogCache`
    /// and read fresh owned copies via `sample()` (spec §6.3).
    pub fn analog(&self) -> &AnalogCache {
        &self.cache
    }

    /// Capabilities of the active pad (spec §6.4).
    pub fn caps(&self) -> DeviceCaps {
        self.backend.caps()
    }

    /// Is slot 0 currently connected?
    pub fn connected(&self) -> bool {
        self.backend.connected()
    }
}

impl DiscreteSource for GamepadSource {
    fn drain_into(&mut self, out: &mut InputState) {
        self.backend.refresh();
        self.backend.pump_buttons(out);
    }
}

impl Default for GamepadSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_with(buttons: &[GamepadButton], axes: &[(GamepadAxis, f32)]) -> PadSnapshot {
        let mut snap = PadSnapshot {
            connected: true,
            caps: DeviceCaps::all(),
            ..Default::default()
        };
        for &b in buttons {
            snap.buttons.insert(b);
        }
        for &(a, v) in axes {
            snap.axes.insert(a, v);
        }
        snap
    }

    #[test]
    fn discrete_writes_held_buttons_and_axis_bridge_to_slot0() {
        let snap = snap_with(
            &[GamepadButton::South, GamepadButton::DPadUp],
            &[
                (GamepadAxis::LeftStickX, 0.5),
                (GamepadAxis::RightTrigger, 0.9),
            ],
        );
        let mut input = InputState::new();
        apply_discrete(&snap, &mut input);

        let gp = input.gamepad(0).expect("slot 0 present when connected");
        assert!(gp.button_down(GamepadButton::South));
        assert!(gp.button_down(GamepadButton::DPadUp));
        assert!(!gp.button_down(GamepadButton::East));
        // RT-12 bridge keeps discrete axes populated.
        assert!((gp.axis_value(GamepadAxis::LeftStickX) - 0.5).abs() < f32::EPSILON);
        assert!((gp.axis_value(GamepadAxis::RightTrigger) - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn single_player_touches_slot0_only() {
        let snap = snap_with(&[GamepadButton::South], &[]);
        let mut input = InputState::new();
        apply_discrete(&snap, &mut input);
        assert!(input.gamepad_connected(0));
        // No other slots are ever created — single local player (spec §6.4).
        assert!(!input.gamepad_connected(1));
        assert_eq!(input.gamepads().count(), 1);
    }

    #[test]
    fn disconnect_removes_slot0() {
        let mut input = InputState::new();
        // Slot 0 exists from a prior connected frame.
        apply_discrete(&snap_with(&[GamepadButton::South], &[]), &mut input);
        assert!(input.gamepad_connected(0));

        // A disconnected snapshot removes it.
        let gone = PadSnapshot::default(); // connected = false
        apply_discrete(&gone, &mut input);
        assert!(!input.gamepad_connected(0));
    }

    #[test]
    fn axes_map_to_analog_frame() {
        let snap = snap_with(
            &[],
            &[
                (GamepadAxis::LeftStickX, 0.25),
                (GamepadAxis::LeftStickY, -0.5),
                (GamepadAxis::RightStickX, 1.0),
                (GamepadAxis::RightStickY, -1.0),
                (GamepadAxis::LeftTrigger, 0.3),
                (GamepadAxis::RightTrigger, 0.7),
            ],
        );
        let mut frame = AnalogFrame::neutral(Instant::now());
        apply_axes(&snap, &mut frame);
        assert_eq!(frame.left_stick, Vec2::new(0.25, -0.5));
        assert_eq!(frame.right_stick, Vec2::new(1.0, -1.0));
        assert!((frame.left_trigger - 0.3).abs() < f32::EPSILON);
        assert!((frame.right_trigger - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn neutral_on_disconnect_yields_zero_frame() {
        // A cleared (disconnected) snapshot samples to a neutral frame, so a
        // camera reading the cache holds rather than snapping (spec §6.4).
        let snap = PadSnapshot::default();
        let mut frame = AnalogFrame::neutral(Instant::now());
        frame.left_stick = Vec2::new(0.9, 0.9); // pretend a live value
        apply_axes(&snap, &mut frame);
        assert_eq!(frame.left_stick, Vec2::ZERO);
        assert_eq!(frame.right_stick, Vec2::ZERO);
        assert_eq!(frame.left_trigger, 0.0);
        assert_eq!(frame.right_trigger, 0.0);
    }

    #[test]
    fn reset_neutral_clears_held_state() {
        let mut snap = snap_with(&[GamepadButton::South], &[(GamepadAxis::LeftStickX, 1.0)]);
        snap.reset_neutral();
        assert!(snap.buttons.is_empty());
        assert!(snap.axes.is_empty());
        assert_eq!(snap.axis(GamepadAxis::LeftStickX), 0.0);
    }

    #[test]
    fn caps_all_vs_default() {
        assert_eq!(
            DeviceCaps::default(),
            DeviceCaps {
                has_left_stick: false,
                has_right_stick: false,
                has_triggers: false,
                has_dpad: false,
            }
        );
        let all = DeviceCaps::all();
        assert!(all.has_left_stick && all.has_right_stick && all.has_triggers && all.has_dpad);
    }
}
