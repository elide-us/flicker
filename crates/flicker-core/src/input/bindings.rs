//! Semantic input bindings: actions decoupled from physical inputs.
//!
//! Consumer code asks "is the `MoveForward` action active?" — never
//! "is the W key down?". The mapping from physical input to
//! [`Action`] lives in an [`InputMap`] table that can be swapped or
//! rebound at runtime without touching the consumer.
//!
//! [`AbstractControls`] sits on top: per-device invert flags,
//! sensitivity, and movement speed. Use [`AbstractControls::look_delta_mouse`]
//! and [`AbstractControls::look_delta_stick`] to turn raw deltas into
//! `(yaw, pitch)` increments with all invert / sensitivity already
//! applied.
//!
//! # Sections
//!
//! - **Input Mapping** — [`InputBinding`], [`InputMap`], [`AxisDirection`]
//! - **Controller Support** — [`GamepadState`], [`GamepadConfig`]
//!   (lives in [`super::mod`])
//! - **Legacy Keyboard Bindings** — [`Bindings`] (kept for backward
//!   compat with existing examples)
//! - **Abstract Controls** — [`AbstractControls`], [`ControlConfig`]
//!   (legacy)

use std::collections::HashMap;
use std::fmt;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use super::{GamepadAxis, GamepadButton, Key, MouseButton};

// ───────────────────────────────────────────────────────────────────
// Section: Semantic Actions
// ───────────────────────────────────────────────────────────────────

/// Semantic input action. Game/example code reacts to these; the
/// physical-input→action mapping lives in [`InputMap`].
///
/// Extend this enum with game-specific actions as needed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    // ── Movement ──
    MoveForward,
    MoveBackward,
    StrafeLeft,
    StrafeRight,
    MoveUp,
    MoveDown,

    // ── Camera ──
    LookUp,
    LookDown,
    LookLeft,
    LookRight,

    // ── Combat / interaction ──
    PrimaryAction,
    SecondaryAction,
    Jump,
    Sprint,
    Crouch,
    Interact,
    Reload,

    // ── UI ──
    Confirm,
    Cancel,
    Menu,
    Inventory,
    Map,

    // ── System ──
    Quit,
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MoveForward => write!(f, "Move Forward"),
            Self::MoveBackward => write!(f, "Move Backward"),
            Self::StrafeLeft => write!(f, "Strafe Left"),
            Self::StrafeRight => write!(f, "Strafe Right"),
            Self::MoveUp => write!(f, "Move Up"),
            Self::MoveDown => write!(f, "Move Down"),
            Self::LookUp => write!(f, "Look Up"),
            Self::LookDown => write!(f, "Look Down"),
            Self::LookLeft => write!(f, "Look Left"),
            Self::LookRight => write!(f, "Look Right"),
            Self::PrimaryAction => write!(f, "Primary Action"),
            Self::SecondaryAction => write!(f, "Secondary Action"),
            Self::Jump => write!(f, "Jump"),
            Self::Sprint => write!(f, "Sprint"),
            Self::Crouch => write!(f, "Crouch"),
            Self::Interact => write!(f, "Interact"),
            Self::Reload => write!(f, "Reload"),
            Self::Confirm => write!(f, "Confirm"),
            Self::Cancel => write!(f, "Cancel"),
            Self::Menu => write!(f, "Menu"),
            Self::Inventory => write!(f, "Inventory"),
            Self::Map => write!(f, "Map"),
            Self::Quit => write!(f, "Quit"),
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Section: Input Mapping
// ───────────────────────────────────────────────────────────────────

/// A single physical input that can be bound to an action.
///
/// Covers keyboard, mouse, and gamepad inputs. Gamepad axes use
/// [`AxisDirection`] to specify which half of the axis triggers the
/// binding.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputBinding {
    Key(Key),
    MouseButton(MouseButton),
    GamepadButton(GamepadButton),
    GamepadAxis {
        axis: GamepadAxis,
        direction: AxisDirection,
    },
}

impl fmt::Display for InputBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(k) => write!(f, "{k}"),
            Self::MouseButton(mb) => write!(f, "{mb}"),
            Self::GamepadButton(gb) => write!(f, "{gb}"),
            Self::GamepadAxis { axis, direction } => write!(f, "{axis} {direction}"),
        }
    }
}

/// Which half of an analog axis triggers a binding.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AxisDirection {
    /// Positive half (> 0.5) — right / down on most sticks.
    Positive,
    /// Negative half (< -0.5) — left / up on most sticks.
    Negative,
}

impl fmt::Display for AxisDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Positive => write!(f, "+"),
            Self::Negative => write!(f, "-"),
        }
    }
}

/// Maps semantic actions to one or more physical inputs.
///
/// Multiple inputs may map to the same action (e.g. both W key and
/// left stick forward for `MoveForward`). One input may only map to
/// one action (last-write-wins on conflict).
///
/// Use [`InputMap::wasd_and_mouse`] or [`InputMap::gamepad_default`]
/// for sensible presets, then customize at runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputMap {
    /// action → list of physical bindings
    action_to_bindings: HashMap<Action, Vec<InputBinding>>,
    /// reverse lookup: physical input → action (for conflict detection)
    input_to_action: HashMap<InputBinding, Action>,
}

impl InputMap {
    /// Empty map — nothing bound.
    pub fn empty() -> Self {
        Self {
            action_to_bindings: HashMap::new(),
            input_to_action: HashMap::new(),
        }
    }

    /// Bind a physical input to a semantic action. If the input is
    /// already bound to a different action, that old binding is
    /// removed first (one-input-one-action invariant).
    pub fn bind(&mut self, action: Action, input: InputBinding) {
        // Remove from previous action if re-binding
        if let Some(old_action) = self.input_to_action.get(&input) {
            if *old_action != action {
                if let Some(bindings) = self.action_to_bindings.get_mut(old_action) {
                    bindings.retain(|b| *b != input);
                }
            }
        }
        self.input_to_action.insert(input, action);
        let bindings = self.action_to_bindings.entry(action).or_default();
        if !bindings.contains(&input) {
            bindings.push(input);
        }
    }

    /// Remove a specific binding from an action.
    pub fn unbind(&mut self, action: Action, input: InputBinding) {
        self.input_to_action.remove(&input);
        if let Some(bindings) = self.action_to_bindings.get_mut(&action) {
            bindings.retain(|b| *b != input);
        }
    }

    /// Remove all bindings for an action.
    pub fn clear_action(&mut self, action: Action) {
        if let Some(bindings) = self.action_to_bindings.remove(&action) {
            for b in bindings {
                self.input_to_action.remove(&b);
            }
        }
    }

    /// All physical bindings for an action.
    pub fn bindings_for(&self, action: Action) -> &[InputBinding] {
        self.action_to_bindings
            .get(&action)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The action bound to a physical input, if any.
    pub fn action_for(&self, input: InputBinding) -> Option<Action> {
        self.input_to_action.get(&input).copied()
    }

    /// All actions that have at least one binding.
    pub fn bound_actions(&self) -> impl Iterator<Item = Action> + '_ {
        self.action_to_bindings.keys().copied()
    }

    // ── Preset constructors ──

    /// WASD keyboard + mouse layout. Maps:
    /// - W/S → forward/backward, A/D → strafe
    /// - R/F → up/down, Escape → quit
    /// - Mouse left → primary action, mouse right → secondary action
    pub fn wasd_and_mouse() -> Self {
        let mut map = Self::empty();
        map.bind(Action::MoveForward, InputBinding::Key(Key::W));
        map.bind(Action::MoveBackward, InputBinding::Key(Key::S));
        map.bind(Action::StrafeLeft, InputBinding::Key(Key::A));
        map.bind(Action::StrafeRight, InputBinding::Key(Key::D));
        map.bind(Action::MoveUp, InputBinding::Key(Key::R));
        map.bind(Action::MoveDown, InputBinding::Key(Key::F));
        map.bind(Action::Quit, InputBinding::Key(Key::Escape));
        map.bind(
            Action::PrimaryAction,
            InputBinding::MouseButton(MouseButton::Left),
        );
        map.bind(
            Action::SecondaryAction,
            InputBinding::MouseButton(MouseButton::Right),
        );
        map.bind(Action::Jump, InputBinding::Key(Key::Space));
        map.bind(Action::Sprint, InputBinding::Key(Key::LeftShift));
        map.bind(Action::Crouch, InputBinding::Key(Key::LeftControl));
        map.bind(Action::Interact, InputBinding::Key(Key::E));
        map.bind(Action::Reload, InputBinding::Key(Key::X));
        map
    }

    /// ESDF keyboard layout. Same idea as WASD but shifted right.
    pub fn esdf_and_mouse() -> Self {
        let mut map = Self::empty();
        map.bind(Action::MoveForward, InputBinding::Key(Key::E));
        map.bind(Action::MoveBackward, InputBinding::Key(Key::D));
        map.bind(Action::StrafeLeft, InputBinding::Key(Key::S));
        map.bind(Action::StrafeRight, InputBinding::Key(Key::F));
        map.bind(Action::MoveUp, InputBinding::Key(Key::R));
        map.bind(Action::MoveDown, InputBinding::Key(Key::W));
        map.bind(Action::Quit, InputBinding::Key(Key::Escape));
        map.bind(
            Action::PrimaryAction,
            InputBinding::MouseButton(MouseButton::Left),
        );
        map.bind(
            Action::SecondaryAction,
            InputBinding::MouseButton(MouseButton::Right),
        );
        map
    }

    /// Default Xbox/PS gamepad layout. Maps:
    /// - Left stick Y → forward/backward, left stick X → strafe
    /// - Right stick → look
    /// - A/South → jump, B/East → cancel, X/West → interact, Y/North → inventory
    /// - Start → menu, Select → map
    /// - Triggers → primary/secondary action
    /// - Bumpers → crouch/sprint
    pub fn gamepad_default() -> Self {
        let mut map = Self::empty();
        // Movement via left stick
        map.bind(
            Action::MoveForward,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            Action::MoveBackward,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Negative,
            },
        );
        map.bind(
            Action::StrafeRight,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            Action::StrafeLeft,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                direction: AxisDirection::Negative,
            },
        );
        // Look via right stick
        map.bind(
            Action::LookRight,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            Action::LookLeft,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Negative,
            },
        );
        map.bind(
            Action::LookUp,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            Action::LookDown,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Negative,
            },
        );
        // Face buttons
        map.bind(Action::Jump, InputBinding::GamepadButton(GamepadButton::South));
        map.bind(Action::Cancel, InputBinding::GamepadButton(GamepadButton::East));
        map.bind(
            Action::Interact,
            InputBinding::GamepadButton(GamepadButton::West),
        );
        map.bind(
            Action::Inventory,
            InputBinding::GamepadButton(GamepadButton::North),
        );
        // Triggers & bumpers
        map.bind(
            Action::PrimaryAction,
            InputBinding::GamepadButton(GamepadButton::RightTrigger),
        );
        map.bind(
            Action::SecondaryAction,
            InputBinding::GamepadButton(GamepadButton::LeftTrigger),
        );
        map.bind(
            Action::Sprint,
            InputBinding::GamepadButton(GamepadButton::LeftBumper),
        );
        map.bind(
            Action::Crouch,
            InputBinding::GamepadButton(GamepadButton::RightBumper),
        );
        // Meta
        map.bind(Action::Menu, InputBinding::GamepadButton(GamepadButton::Start));
        map.bind(Action::Map, InputBinding::GamepadButton(GamepadButton::Select));
        map.bind(Action::Quit, InputBinding::GamepadButton(GamepadButton::Guide));
        map
    }
}

impl Default for InputMap {
    fn default() -> Self {
        Self::wasd_and_mouse()
    }
}

// ───────────────────────────────────────────────────────────────────
// Section: Legacy Keyboard Bindings
// ───────────────────────────────────────────────────────────────────

/// Simple keyboard-only action map, kept for backward compatibility
/// with existing examples that use `Bindings::wasd()`.
///
/// New code should prefer [`InputMap`] which supports all device
/// types.
#[derive(Clone, Debug)]
pub struct Bindings {
    key_to_action: HashMap<Key, Action>,
}

impl Bindings {
    /// Empty bindings — nothing bound.
    pub fn empty() -> Self {
        Self {
            key_to_action: HashMap::new(),
        }
    }

    /// Default WASD layout: W/A/S/D move + strafe, R/F up/down, Esc quit.
    pub fn wasd() -> Self {
        let mut b = Self::empty();
        b.bind(Key::W, Action::MoveForward);
        b.bind(Key::S, Action::MoveBackward);
        b.bind(Key::A, Action::StrafeLeft);
        b.bind(Key::D, Action::StrafeRight);
        b.bind(Key::R, Action::MoveUp);
        b.bind(Key::F, Action::MoveDown);
        b.bind(Key::Escape, Action::Quit);
        b
    }

    /// ESDF layout.
    pub fn esdf() -> Self {
        let mut b = Self::empty();
        b.bind(Key::E, Action::MoveForward);
        b.bind(Key::D, Action::MoveBackward);
        b.bind(Key::S, Action::StrafeLeft);
        b.bind(Key::F, Action::StrafeRight);
        b.bind(Key::R, Action::MoveUp);
        b.bind(Key::W, Action::MoveDown);
        b.bind(Key::Escape, Action::Quit);
        b
    }

    /// Bind a physical key to an action.
    pub fn bind(&mut self, key: Key, action: Action) {
        self.key_to_action.insert(key, action);
    }

    /// Remove any binding for a key.
    pub fn unbind(&mut self, key: Key) {
        self.key_to_action.remove(&key);
    }

    /// The action bound to `key`, if any.
    pub fn action_for(&self, key: Key) -> Option<Action> {
        self.key_to_action.get(&key).copied()
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::wasd()
    }
}

// ───────────────────────────────────────────────────────────────────
// Section: Abstract Controls
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

/// Legacy control config, kept for backward compatibility.
///
/// New code should prefer [`AbstractControls`].
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ControlConfig {
    pub look_sensitivity: f32,
    pub move_speed: f32,
    pub invert_pitch: bool,
    pub invert_yaw: bool,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            look_sensitivity: 0.005,
            move_speed: 120.0,
            invert_pitch: false,
            invert_yaw: false,
        }
    }
}

impl ControlConfig {
    /// Convert a raw cursor pixel delta into a `(yaw_delta,
    /// pitch_delta)` in radians.
    pub fn look_delta(&self, cursor_delta: Vec2) -> (f32, f32) {
        let yaw_sign = if self.invert_yaw { -1.0 } else { 1.0 };
        let pitch_sign = if self.invert_pitch { -1.0 } else { 1.0 };
        let yaw = cursor_delta.x * self.look_sensitivity * yaw_sign;
        let pitch = (-cursor_delta.y) * self.look_sensitivity * pitch_sign;
        (yaw, pitch)
    }
}

// ───────────────────────────────────────────────────────────────────
// Section: Tests
// ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::InputState;

    // ── Legacy Bindings tests ──

    #[test]
    fn wasd_binds_movement() {
        let b = Bindings::wasd();
        assert_eq!(b.action_for(Key::W), Some(Action::MoveForward));
        assert_eq!(b.action_for(Key::S), Some(Action::MoveBackward));
        assert_eq!(b.action_for(Key::A), Some(Action::StrafeLeft));
        assert_eq!(b.action_for(Key::D), Some(Action::StrafeRight));
        assert_eq!(b.action_for(Key::R), Some(Action::MoveUp));
        assert_eq!(b.action_for(Key::F), Some(Action::MoveDown));
        assert_eq!(b.action_for(Key::Escape), Some(Action::Quit));
    }

    #[test]
    fn esdf_binds_movement() {
        let b = Bindings::esdf();
        assert_eq!(b.action_for(Key::E), Some(Action::MoveForward));
        assert_eq!(b.action_for(Key::D), Some(Action::MoveBackward));
        assert_eq!(b.action_for(Key::S), Some(Action::StrafeLeft));
        assert_eq!(b.action_for(Key::F), Some(Action::StrafeRight));
        assert_eq!(b.action_for(Key::R), Some(Action::MoveUp));
        assert_eq!(b.action_for(Key::W), Some(Action::MoveDown));
    }

    #[test]
    fn empty_has_no_bindings() {
        let b = Bindings::empty();
        assert_eq!(b.action_for(Key::W), None);
    }

    #[test]
    fn rebinding_overwrites() {
        let mut b = Bindings::empty();
        b.bind(Key::E, Action::MoveForward);
        b.bind(Key::E, Action::MoveBackward);
        assert_eq!(b.action_for(Key::E), Some(Action::MoveBackward));
    }

    #[test]
    fn unbind_removes() {
        let mut b = Bindings::wasd();
        b.unbind(Key::W);
        assert_eq!(b.action_for(Key::W), None);
    }

    #[test]
    fn default_is_wasd() {
        let b = Bindings::default();
        assert_eq!(b.action_for(Key::W), Some(Action::MoveForward));
    }

    // ── ControlConfig (legacy) tests ──

    #[test]
    fn look_delta_default_is_not_inverted() {
        let cfg = ControlConfig::default();
        let (yaw, pitch) = cfg.look_delta(Vec2::new(10.0, 10.0));
        assert!(yaw > 0.0);
        assert!(pitch < 0.0);
    }

    #[test]
    fn invert_pitch_flips_vertical_only() {
        let mut cfg = ControlConfig::default();
        let d = Vec2::new(10.0, 10.0);
        let (yaw0, pitch0) = cfg.look_delta(d);
        cfg.invert_pitch = true;
        let (yaw1, pitch1) = cfg.look_delta(d);
        assert_eq!(yaw0, yaw1, "yaw unaffected by invert_pitch");
        assert_eq!(pitch0, -pitch1, "pitch sign flipped");
    }

    #[test]
    fn invert_yaw_flips_horizontal_only() {
        let mut cfg = ControlConfig::default();
        let d = Vec2::new(10.0, 10.0);
        let (yaw0, pitch0) = cfg.look_delta(d);
        cfg.invert_yaw = true;
        let (yaw1, pitch1) = cfg.look_delta(d);
        assert_eq!(pitch0, pitch1);
        assert_eq!(yaw0, -yaw1);
    }

    // ── InputMap tests ──

    #[test]
    fn input_map_wasd_and_mouse() {
        let map = InputMap::wasd_and_mouse();
        assert_eq!(
            map.action_for(InputBinding::Key(Key::W)),
            Some(Action::MoveForward)
        );
        assert_eq!(
            map.action_for(InputBinding::MouseButton(MouseButton::Left)),
            Some(Action::PrimaryAction)
        );
    }

    #[test]
    fn input_map_bind_unbind() {
        let mut map = InputMap::empty();
        map.bind(Action::Jump, InputBinding::Key(Key::Space));
        assert_eq!(
            map.action_for(InputBinding::Key(Key::Space)),
            Some(Action::Jump)
        );
        map.unbind(Action::Jump, InputBinding::Key(Key::Space));
        assert_eq!(map.action_for(InputBinding::Key(Key::Space)), None);
    }

    #[test]
    fn input_map_rebind_enforces_one_action_per_input() {
        let mut map = InputMap::empty();
        map.bind(Action::MoveForward, InputBinding::Key(Key::W));
        map.bind(Action::MoveBackward, InputBinding::Key(Key::W));
        // W should now map to MoveBackward, not MoveForward
        assert_eq!(
            map.action_for(InputBinding::Key(Key::W)),
            Some(Action::MoveBackward)
        );
        // MoveForward should have no W binding
        assert!(map.bindings_for(Action::MoveForward).is_empty());
    }

    #[test]
    fn input_map_multiple_inputs_per_action() {
        let mut map = InputMap::empty();
        map.bind(Action::MoveForward, InputBinding::Key(Key::W));
        map.bind(Action::MoveForward, InputBinding::Key(Key::Up));
        assert_eq!(map.bindings_for(Action::MoveForward).len(), 2);
    }

    #[test]
    fn input_map_gamepad_default() {
        let map = InputMap::gamepad_default();
        // A/South = jump
        assert_eq!(
            map.action_for(InputBinding::GamepadButton(GamepadButton::South)),
            Some(Action::Jump)
        );
        // Right trigger = primary action
        assert_eq!(
            map.action_for(InputBinding::GamepadButton(GamepadButton::RightTrigger)),
            Some(Action::PrimaryAction)
        );
    }

    #[test]
    fn input_map_clear_action() {
        let mut map = InputMap::wasd_and_mouse();
        assert!(!map.bindings_for(Action::MoveForward).is_empty());
        map.clear_action(Action::MoveForward);
        assert!(map.bindings_for(Action::MoveForward).is_empty());
    }

    #[test]
    fn input_map_bound_actions() {
        let map = InputMap::wasd_and_mouse();
        let actions: Vec<Action> = map.bound_actions().collect();
        assert!(actions.contains(&Action::MoveForward));
        assert!(actions.contains(&Action::Quit));
    }

    // ── AbstractControls tests ──

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
        let ctrl = AbstractControls { stick_sensitivity: 5.0, ..Default::default() };
        let (yaw, _) = ctrl.look_delta_stick(Vec2::new(1.0, 0.0));
        assert!((yaw - 5.0).abs() < 0.01);
    }

    #[test]
    fn action_active_reads_bindings() {
        let b = Bindings::wasd();
        let mut input = InputState::new();
        input.set_key(Key::W, true);
        assert!(input.action_active(&b, Action::MoveForward));
        assert!(!input.action_active(&b, Action::MoveBackward));
    }

    #[test]
    fn action_active_supports_multiple_keys_per_action() {
        let mut b = Bindings::wasd();
        b.bind(Key::Up, Action::MoveForward);
        let mut input = InputState::new();
        input.set_key(Key::Up, true);
        assert!(input.action_active(&b, Action::MoveForward));
    }
}
