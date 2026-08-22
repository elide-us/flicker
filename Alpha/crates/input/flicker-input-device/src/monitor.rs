//! Last-used input device monitor (plan 1A292918 T4).
//!
//! A tiny process-global answering one question: is the player on keyboard/mouse
//! or a gamepad *right now*? That is the fact the binding-aware glyph display (T5)
//! needs to choose between a keycap of the bound key (kbm) and a controller glyph
//! (pad) for the same authored signal.
//!
//! It is DEVICE-KIND only, never the provenance of a specific event. The runner
//! calls [`note_frame`] once per frame at the device-drain choke point — every
//! physical event has landed in the [`InputState`] snapshot, and the frame's KBM
//! edges have not been cleared yet — and this latches whichever family showed
//! activity. A frame with input on neither family leaves the latch untouched, so
//! it reflects the LAST device the player actually touched. This mirrors the
//! `last_window_geometry` global idiom next door in `flicker-app`.
//!
//! The device backend classifies the pad's vendor on connect
//! ([`PadVendor::from_metadata`]) and hands it in via [`note_frame`]. Only Xbox
//! ATLAS ART exists today, so a PlayStation pad still resolves to Xbox glyphs at the
//! display layer until per-vendor sheets land — that is an atlas concern (plan
//! B9150E95), not this module's.

use std::sync::Mutex;

use flicker_input_core::{GamepadAxis, GamepadButton, InputState};

/// A gamepad's button-face family — which glyph atlas its bindings render through
/// (the "controller vocabulary" axis of display, concept 0F5E0201). Only Xbox ATLAS
/// ART exists today, but the vendor is carried regardless so a per-vendor sheet can
/// be selected later without minting a second enum.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum PadVendor {
    Xbox,
    PlayStation,
    /// Unknown / third-party pad — the safe default before metadata classifies one.
    #[default]
    Generic,
}

impl PadVendor {
    /// Classify a connected pad from its OS metadata — the platform backends call
    /// this on connect. A known USB vendor id wins (Sony `0x054C`, Microsoft
    /// `0x045E`); otherwise the product / name string decides. Unknown → [`Generic`].
    pub fn from_metadata(name: &str, vendor_id: Option<u16>) -> Self {
        match vendor_id {
            Some(0x054C) => return Self::PlayStation,
            Some(0x045E) => return Self::Xbox,
            _ => {}
        }
        let n = name.to_ascii_lowercase();
        if n.contains("xbox") || n.contains("xinput") {
            Self::Xbox
        } else if n.contains("dualsense")
            || n.contains("dualshock")
            || n.contains("playstation")
            || n.contains("sony")
        {
            Self::PlayStation
        } else {
            Self::Generic
        }
    }
}

/// The input-device family the player last used — the T5 display resolves a
/// signal's binding, then renders a keycap ([`Kbm`](InputDeviceKind::Kbm)) or a
/// controller glyph ([`Pad`](InputDeviceKind::Pad)) from this.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputDeviceKind {
    /// Keyboard and/or mouse.
    Kbm,
    /// A gamepad, tagged with its button-face family for atlas selection.
    Pad(PadVendor),
}

impl InputDeviceKind {
    /// The stable token published into a scene's HUD model as `input_device` and
    /// read by the walker's device-adaptive hints — `"kbm"`, or the pad vendor.
    pub fn token(self) -> &'static str {
        match self {
            Self::Kbm => "kbm",
            Self::Pad(PadVendor::Xbox) => "xbox",
            Self::Pad(PadVendor::PlayStation) => "playstation",
            Self::Pad(PadVendor::Generic) => "generic",
        }
    }

    /// Whether the player is on a gamepad (glyph atlas) rather than kbm (keycaps).
    pub fn is_pad(self) -> bool {
        matches!(self, Self::Pad(_))
    }
}

/// A stick past this magnitude counts as deliberate pad use — well above any
/// resting deadzone, so a centred (or lightly drifting) stick never masquerades as
/// activity. Triggers are excluded from the axis test on purpose: some backends
/// rest a trigger at a non-zero value, and a full pull already lands as the
/// `LeftTrigger`/`RightTrigger` *button*.
const PAD_STICK_ACTIVITY: f32 = 0.5;

/// The four stick axes — the only axes whose rest value is reliably zero, so the
/// only ones the activity test reads (see [`PAD_STICK_ACTIVITY`]).
const STICKS: [GamepadAxis; 4] = [
    GamepadAxis::LeftStickX,
    GamepadAxis::LeftStickY,
    GamepadAxis::RightStickX,
    GamepadAxis::RightStickY,
];

/// The last device family the player touched. Defaults to [`InputDeviceKind::Kbm`]
/// — the desktop resting state before any input, and the safe default for the
/// keycap display.
static LAST_INPUT: Mutex<InputDeviceKind> = Mutex::new(InputDeviceKind::Kbm);

/// Latch the last-used device family from this frame's snapshot. The runner calls
/// this once per frame, AFTER the sources drain and BEFORE `clear_frame_edges`, so
/// the KBM edges that the clear wipes are still visible. A frame with no input on
/// either family leaves the latch unchanged.
pub fn note_frame(input: &InputState, vendor: PadVendor) {
    if let Some(kind) = classify(input, vendor) {
        if let Ok(mut g) = LAST_INPUT.lock() {
            *g = kind;
        }
    }
}

/// The device family the player last used, for a scene's HUD to publish and the
/// walker's hints to branch on. [`InputDeviceKind::Kbm`] until the first input (or
/// if the lock is poisoned — the display then simply shows keycaps).
pub fn last_input_context() -> InputDeviceKind {
    LAST_INPUT
        .lock()
        .map(|g| *g)
        .unwrap_or(InputDeviceKind::Kbm)
}

/// The device family THIS frame's snapshot shows activity for, or `None` when the
/// player touched neither family (the latch then holds). The pure seam — the
/// global [`note_frame`] latches its result and the tests exercise it without the
/// shared state.
///
/// KBM wins a tie: a key/click/scroll/motion is a deliberate act this frame,
/// whereas a pad stick can sit deflected while the player reaches for the keyboard.
fn classify(input: &InputState, vendor: PadVendor) -> Option<InputDeviceKind> {
    if kbm_active(input) {
        Some(InputDeviceKind::Kbm)
    } else if pad_active(input) {
        Some(InputDeviceKind::Pad(vendor))
    } else {
        None
    }
}

/// Keyboard/mouse acted this frame: a key or mouse-button edge, committed text, a
/// wheel tick, or pointer motion. Motion and wheel are per-frame deltas (not edges,
/// which the resolver reads), so they are checked directly.
fn kbm_active(input: &InputState) -> bool {
    !input.edges().is_empty()
        || !input.typed().is_empty()
        || input.mouse_wheel_delta != 0.0
        || input.mouse_delta.x != 0.0
        || input.mouse_delta.y != 0.0
}

/// The slot-0 pad is being actively used: any button held, or a stick past
/// [`PAD_STICK_ACTIVITY`]. Pad state is polled (no edges), so "active" is level, not
/// an edge — a held control keeps the pad selected, which is the intent.
fn pad_active(input: &InputState) -> bool {
    let Some(gp) = input.gamepad(0) else {
        return false;
    };
    GamepadButton::ALL.iter().any(|&b| gp.button_down(b))
        || STICKS
            .iter()
            .any(|&a| gp.axis_value(a).abs() > PAD_STICK_ACTIVITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resting_frame_reports_no_activity() {
        // Nothing touched → the latch must hold (None), never flip.
        assert_eq!(classify(&InputState::new(), PadVendor::Generic), None);
    }

    #[test]
    fn wheel_or_motion_reads_as_kbm() {
        let mut s = InputState::new();
        s.mouse_wheel_delta = 1.0;
        assert_eq!(classify(&s, PadVendor::Xbox), Some(InputDeviceKind::Kbm));

        let mut s = InputState::new();
        s.mouse_delta.y = -4.0;
        assert_eq!(classify(&s, PadVendor::Xbox), Some(InputDeviceKind::Kbm));
    }

    #[test]
    fn a_held_pad_button_carries_the_detected_vendor() {
        // The vendor the backend detected flows straight through — not hardcoded.
        let mut s = InputState::new();
        s.gamepad_mut(0).set_button(GamepadButton::South, true);
        assert_eq!(
            classify(&s, PadVendor::PlayStation),
            Some(InputDeviceKind::Pad(PadVendor::PlayStation))
        );
    }

    #[test]
    fn a_stick_reads_as_pad_only_past_the_threshold() {
        let mut s = InputState::new();
        s.gamepad_mut(0).set_axis(GamepadAxis::LeftStickX, 0.9);
        assert_eq!(
            classify(&s, PadVendor::Xbox),
            Some(InputDeviceKind::Pad(PadVendor::Xbox))
        );

        // A resting/drifting stick under the threshold is not activity.
        let mut s = InputState::new();
        s.gamepad_mut(0).set_axis(GamepadAxis::LeftStickX, 0.2);
        assert_eq!(classify(&s, PadVendor::Xbox), None);
    }

    #[test]
    fn a_resting_trigger_axis_is_not_pad_activity() {
        // Triggers are excluded from the stick test; a resting trigger value must
        // not pin the player to "pad" forever. The trigger BUTTON still counts.
        let mut s = InputState::new();
        s.gamepad_mut(0).set_axis(GamepadAxis::LeftTrigger, 1.0);
        assert_eq!(classify(&s, PadVendor::Xbox), None);
    }

    #[test]
    fn kbm_wins_a_tie_with_a_held_stick() {
        // A key press while a stick sits deflected → the deliberate KBM act wins.
        let mut s = InputState::new();
        s.mouse_wheel_delta = 1.0;
        s.gamepad_mut(0).set_button(GamepadButton::South, true);
        assert_eq!(classify(&s, PadVendor::Xbox), Some(InputDeviceKind::Kbm));
    }

    #[test]
    fn from_metadata_classifies_vendors() {
        // A known USB vendor id wins outright …
        assert_eq!(
            PadVendor::from_metadata("Wireless Controller", Some(0x054C)),
            PadVendor::PlayStation
        );
        assert_eq!(
            PadVendor::from_metadata("Controller", Some(0x045E)),
            PadVendor::Xbox
        );
        // … else the product / name string decides (case-insensitive) …
        assert_eq!(
            PadVendor::from_metadata("Xbox Wireless Controller", None),
            PadVendor::Xbox
        );
        assert_eq!(
            PadVendor::from_metadata("DualSense Wireless Controller", None),
            PadVendor::PlayStation
        );
        // … and an unrecognised pad is Generic, the default.
        assert_eq!(
            PadVendor::from_metadata("8BitDo Pro 2", None),
            PadVendor::Generic
        );
        assert_eq!(PadVendor::default(), PadVendor::Generic);
    }

    #[test]
    fn tokens_are_stable() {
        assert_eq!(InputDeviceKind::Kbm.token(), "kbm");
        assert_eq!(InputDeviceKind::Pad(PadVendor::Xbox).token(), "xbox");
        assert_eq!(
            InputDeviceKind::Pad(PadVendor::PlayStation).token(),
            "playstation"
        );
        assert!(InputDeviceKind::Pad(PadVendor::Xbox).is_pad());
        assert!(!InputDeviceKind::Kbm.is_pad());
    }
}
