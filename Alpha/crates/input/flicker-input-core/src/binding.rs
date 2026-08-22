//! Semantic input bindings: signals decoupled from physical inputs.
//!
//! Consumer code asks "is the `MoveForward` signal active?" — never "is the W
//! key down?". The mapping from physical input to [`ActionSignal`] lives in an
//! [`InputMap`] table that can be swapped or rebound at runtime.
//!
//! This module also carries the §3.5 per-binding descriptors ([`Activation`],
//! [`BindingDescriptor`]) that encode the three input concepts (stance,
//! long-press, modifier-chord) without minting new signals.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::device::{AxisDirection, GamepadAxis, Key, MouseButton};
use crate::signal::ActionSignal;
use crate::snapshot::{GamepadConfig, InputState};

// ───────────────────────────────────────────────────────────────────
// Section: Input Mapping
// ───────────────────────────────────────────────────────────────────

/// A single physical input that can be bound to a signal.
///
/// Which mouse-motion axis a [`InputBinding::MouseMotion`] reads.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseAxis {
    X,
    Y,
}

impl MouseAxis {
    /// The stringtable token for this motion axis' player-facing display name
    /// (the input-catalog convention — seeds "Mouse X" / "Mouse Y").
    pub fn token(self) -> &'static str {
        match self {
            Self::X => "$mouse_x",
            Self::Y => "$mouse_y",
        }
    }
}

/// Covers keyboard, mouse, and gamepad inputs. Gamepad axes use
/// [`AxisDirection`] to specify which half of the axis triggers the
/// binding.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputBinding {
    Key(Key),
    MouseButton(MouseButton),
    GamepadButton(crate::device::GamepadButton),
    GamepadAxis {
        axis: GamepadAxis,
        direction: AxisDirection,
    },
    /// Mouse MOTION on one axis — the pointer-look channel. `direction` picks the
    /// half (as gamepad axes do); `gate`, when set, restricts it to while that mouse
    /// button is held (right-drag look). Resolved as a per-frame DELTA via
    /// [`ContextualBindings::signal_pointer_delta`](crate::ContextualBindings::signal_pointer_delta),
    /// NOT the 0..1 `signal_axis` rate channel — a mouse delta is frame-absolute
    /// motion, not a stick deflection, and the two integrate differently (the delta
    /// is used as-is; a stick is a rate multiplied by `dt`).
    MouseMotion {
        axis: MouseAxis,
        direction: AxisDirection,
        gate: Option<MouseButton>,
    },
}

impl fmt::Display for InputBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(k) => write!(f, "{k}"),
            Self::MouseButton(mb) => write!(f, "{mb}"),
            Self::GamepadButton(gb) => write!(f, "{gb}"),
            Self::GamepadAxis { axis, direction } => write!(f, "{axis} {direction}"),
            Self::MouseMotion {
                axis,
                direction,
                gate,
            } => match gate {
                Some(g) => write!(f, "Mouse {axis:?} {direction} (hold {g})"),
                None => write!(f, "Mouse {axis:?} {direction}"),
            },
        }
    }
}

impl InputBinding {
    /// The new down-state `edge` gives this control, or `None` if the edge belongs
    /// to a different control.
    ///
    /// Gamepad bindings never match: they are polled per frame rather than evented,
    /// so they carry no intra-frame ordering and stay level-resolved.
    pub fn edge_down(self, edge: crate::snapshot::InputEdge) -> Option<bool> {
        use crate::snapshot::InputEdge;
        match (self, edge) {
            (InputBinding::Key(k), InputEdge::Key { key, down }) if k == key => Some(down),
            (InputBinding::MouseButton(b), InputEdge::Mouse { button, down }) if b == button => {
                Some(down)
            }
            _ => None,
        }
    }

    /// THE single "is this control active" query — deadzone/threshold-aware,
    /// player 0. Consolidates `InputMap::action_pressed`, `InputState::input_active`,
    /// and the tester's `Control::down` into one (spec §3.2). Stick/trigger axes
    /// use the passed [`GamepadConfig`] thresholds.
    pub fn is_down(self, s: &InputState, cfg: &GamepadConfig) -> bool {
        match self {
            InputBinding::Key(k) => s.key_down(k),
            InputBinding::MouseButton(mb) => s.mouse_button_down(mb),
            InputBinding::GamepadButton(btn) => s.gamepad(0).is_some_and(|gp| gp.button_down(btn)),
            InputBinding::GamepadAxis { axis, direction } => s.gamepad(0).is_some_and(|gp| {
                let v = gp.axis_value(axis);
                let threshold = match axis {
                    GamepadAxis::LeftStickX | GamepadAxis::LeftStickY => cfg.left_stick_deadzone,
                    GamepadAxis::RightStickX | GamepadAxis::RightStickY => cfg.right_stick_deadzone,
                    GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger => cfg.trigger_threshold,
                };
                match direction {
                    AxisDirection::Positive => v > threshold,
                    AxisDirection::Negative => v < -threshold,
                }
            }),
            // Mouse motion is a per-frame DELTA (the pointer-look channel), not a held
            // control — it resolves only through `signal_pointer_delta`, never here.
            InputBinding::MouseMotion { .. } => false,
        }
    }

    /// The gated, directional mouse-motion delta this frame for a [`MouseMotion`]
    /// binding — `0.0` for any other binding, or when the gate button is not held.
    /// The per-binding value the pointer-look channel sums.
    ///
    /// [`MouseMotion`]: InputBinding::MouseMotion
    pub fn mouse_delta_axis(self, s: &InputState) -> f32 {
        let InputBinding::MouseMotion {
            axis,
            direction,
            gate,
        } = self
        else {
            return 0.0;
        };
        if let Some(g) = gate {
            if !s.mouse_button_down(g) {
                return 0.0;
            }
        }
        let d = match axis {
            MouseAxis::X => s.mouse_delta.x,
            MouseAxis::Y => s.mouse_delta.y,
        };
        match direction {
            AxisDirection::Positive => d.max(0.0),
            AxisDirection::Negative => (-d).max(0.0),
        }
    }
}

/// Maps semantic signals to one or more physical inputs.
///
/// Multiple inputs may map to the same signal (e.g. both W key and
/// left stick forward for `MoveForward`). One input may only map to
/// one signal (last-write-wins on conflict).
///
/// Use [`InputMap::wasd_and_mouse`] or [`InputMap::gamepad_default`]
/// for sensible presets, then customize at runtime.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "InputMapData", into = "InputMapData")]
pub struct InputMap {
    /// signal → list of physical bindings
    action_to_bindings: HashMap<ActionSignal, Vec<InputBinding>>,
    /// reverse lookup: physical input → signal (for conflict detection)
    input_to_action: HashMap<InputBinding, ActionSignal>,
}

/// On-disk form of [`InputMap`]. Only the forward map is stored; the reverse
/// `input_to_action` index is rebuilt on load.
///
/// Why this exists: `input_to_action` is keyed by [`InputBinding`], a non-unit enum that
/// JSON cannot use as an object key — serializing the live struct directly fails at
/// runtime, which is why rebinds never persisted. Storing the bindings as a flat list of
/// pairs sidesteps map-key encoding entirely and rebuilds both indices through [`bind`],
/// preserving the one-input-one-signal invariant.
///
/// [`bind`]: InputMap::bind
#[derive(Serialize, Deserialize)]
struct InputMapData {
    action_to_bindings: Vec<(ActionSignal, Vec<InputBinding>)>,
}

impl From<InputMap> for InputMapData {
    fn from(map: InputMap) -> Self {
        Self {
            action_to_bindings: map.action_to_bindings.into_iter().collect(),
        }
    }
}

impl From<InputMapData> for InputMap {
    fn from(data: InputMapData) -> Self {
        let mut map = InputMap::empty();
        for (action, inputs) in data.action_to_bindings {
            for input in inputs {
                map.bind(action, input);
            }
        }
        map
    }
}

impl InputMap {
    /// Empty map — nothing bound.
    pub fn empty() -> Self {
        Self {
            action_to_bindings: HashMap::new(),
            input_to_action: HashMap::new(),
        }
    }

    /// Bind a physical input to a semantic signal. If the input is
    /// already bound to a different signal, that old binding is
    /// removed first (one-input-one-signal invariant).
    pub fn bind(&mut self, action: ActionSignal, input: InputBinding) {
        // Remove from previous signal if re-binding
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

    /// Remove a specific binding from a signal.
    pub fn unbind(&mut self, action: ActionSignal, input: InputBinding) {
        self.input_to_action.remove(&input);
        if let Some(bindings) = self.action_to_bindings.get_mut(&action) {
            bindings.retain(|b| *b != input);
        }
    }

    /// Remove all bindings for a signal.
    pub fn clear_action(&mut self, action: ActionSignal) {
        if let Some(bindings) = self.action_to_bindings.remove(&action) {
            for b in bindings {
                self.input_to_action.remove(&b);
            }
        }
    }

    /// All physical bindings for a signal.
    pub fn bindings_for(&self, action: ActionSignal) -> &[InputBinding] {
        self.action_to_bindings
            .get(&action)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Adopt `defaults`' bindings for every signal THIS map leaves wholly
    /// unbound. The profile-migration half of the dead-hardware law: a
    /// persisted map freezes the defaults at save time, so a signal whose
    /// default binding was added in a LATER build resolves to nothing forever
    /// for anyone with a settings file. Signals the user has bound (or rebound)
    /// are untouched — only silence gains a default.
    pub fn backfill_unbound_from(&mut self, defaults: &InputMap) {
        for &signal in ActionSignal::ALL {
            if self.bindings_for(signal).is_empty() {
                for &binding in defaults.bindings_for(signal) {
                    self.bind(signal, binding);
                }
            }
        }
    }

    /// The signal bound to a physical input, if any.
    pub fn action_for(&self, input: InputBinding) -> Option<ActionSignal> {
        self.input_to_action.get(&input).copied()
    }

    /// All signals that have at least one binding.
    pub fn bound_actions(&self) -> impl Iterator<Item = ActionSignal> + '_ {
        self.action_to_bindings.keys().copied()
    }

    /// Is the given signal currently pressed? Checks all bindings for
    /// the signal against the current input state.
    pub fn action_pressed(&self, action: ActionSignal, input: &InputState) -> bool {
        self.bindings_for(action).iter().any(|b| match b {
            InputBinding::Key(k) => input.key_down(*k),
            InputBinding::MouseButton(mb) => input.mouse_button_down(*mb),
            InputBinding::GamepadButton(btn) => {
                input.gamepad(0).is_some_and(|gp| gp.button_down(*btn))
            }
            InputBinding::GamepadAxis { axis, direction } => input.gamepad(0).is_some_and(|gp| {
                let v = gp.axis_value(*axis);
                match direction {
                    AxisDirection::Positive => v > 0.5,
                    AxisDirection::Negative => v < -0.5,
                }
            }),
            InputBinding::MouseMotion { .. } => false,
        })
    }

    // ── Preset constructors ──

    /// WASD keyboard + mouse layout. Maps:
    /// - W/S → forward/backward, A/D → strafe
    /// - R/F → up/down, Escape → quit
    /// - Mouse left → primary action, mouse right → secondary action
    pub fn wasd_and_mouse() -> Self {
        let mut map = Self::empty();
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
        map.bind(ActionSignal::MoveBackward, InputBinding::Key(Key::S));
        map.bind(ActionSignal::StrafeLeft, InputBinding::Key(Key::A));
        map.bind(ActionSignal::StrafeRight, InputBinding::Key(Key::D));
        map.bind(ActionSignal::MoveUp, InputBinding::Key(Key::R));
        map.bind(ActionSignal::MoveDown, InputBinding::Key(Key::F));
        map.bind(ActionSignal::Quit, InputBinding::Key(Key::Escape));
        map.bind(ActionSignal::Menu, InputBinding::Key(Key::Escape));
        // Gamepad menu-open: the Start / hamburger button also opens the menu, so a
        // scene on this KBM preset is pad-openable too (Start is otherwise unbound
        // here, so one-input-one-action holds). Mirrors the gamepad presets.
        map.bind(
            ActionSignal::Menu,
            InputBinding::GamepadButton(crate::device::GamepadButton::Start),
        );
        map.bind(
            ActionSignal::PrimaryAction,
            InputBinding::MouseButton(MouseButton::Left),
        );
        map.bind(
            ActionSignal::SecondaryAction,
            InputBinding::MouseButton(MouseButton::Right),
        );
        // Mouse-look: RIGHT-drag orbits (LEFT stays free for select-target — the MMO
        // default). X motion → look left/right, Y → up/down (screen y is down, so look
        // up = negative Y); gated on RMB held, so a plain cursor never turns the camera.
        map.bind(
            ActionSignal::LookRight,
            InputBinding::MouseMotion {
                axis: MouseAxis::X,
                direction: AxisDirection::Positive,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(
            ActionSignal::LookLeft,
            InputBinding::MouseMotion {
                axis: MouseAxis::X,
                direction: AxisDirection::Negative,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(
            ActionSignal::LookDown,
            InputBinding::MouseMotion {
                axis: MouseAxis::Y,
                direction: AxisDirection::Positive,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(
            ActionSignal::LookUp,
            InputBinding::MouseMotion {
                axis: MouseAxis::Y,
                direction: AxisDirection::Negative,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(ActionSignal::Jump, InputBinding::Key(Key::Space));
        map.bind(ActionSignal::Sprint, InputBinding::Key(Key::LeftShift));
        // S9b additive binds: RIGHT Shift also sprints and C also crouches —
        // paperdoll's combat feed honoured both raw keys before it moved onto
        // `signal_held`, so the DEFAULT preset carries them for identical
        // physical behaviour (both keys were unbound here, so nothing is stolen).
        map.bind(ActionSignal::Sprint, InputBinding::Key(Key::RightShift));
        map.bind(ActionSignal::Crouch, InputBinding::Key(Key::LeftControl));
        map.bind(ActionSignal::Crouch, InputBinding::Key(Key::C));
        map.bind(ActionSignal::Interact, InputBinding::Key(Key::E));
        map.bind(ActionSignal::Reload, InputBinding::Key(Key::X));
        // The bench surfaces run in the World context, and their page/tab rails
        // are chrome, not modal-menu state — so the rail signals must resolve
        // HERE. (They once lived only in the Menu context's map, which no bench
        // ever pushes: every rail was declared, dispatched, and unreachable.)
        map.bind_menu_rails();
        map.bind_bench_nav();
        map
    }

    /// The bench-nav tier of the focus model (controller is the floor), bound in the
    /// WORLD preset because benches run there — a declared nav/confirm signal with no
    /// World binding is dead hardware, exactly the trap the rails fell into.
    ///
    /// FLATTENED (plan 1A292918 T2): the pane tier answers BOTH devices, but through
    /// SEPARATE intents that must never be tangled (Aaron 2026-08-18) — the D-PAD emits
    /// `Nav*`, the LEFT STICK emits `Panel*`. The walker's pane tier consumes both (each
    /// moves the focus cursor between panel stops, geometrically when rects are present);
    /// in-control components (a slider nudge, a list step) answer ONLY the d-pad's `Nav*`,
    /// never the stick. The stick is AXIS-SPLIT because one physical input binds exactly
    /// one signal (binding.rs invariant): X → `Panel*` (the pane cursor — `PanelPrev`
    /// left, `PanelNext` right), Y → `Zoom*` (an entered viewport's camera, the Populous
    /// globe). So the D-PAD owns VERTICAL pane moves; the stick cannot also drive them
    /// without stealing the globe's zoom. A stick FLICK past the deadzone is one step.
    /// A screen that declares none of these leaves every event to the walker's nav
    /// defaults, so gameplay maps are unaffected. Keyboard Confirm rides Enter (Space
    /// stays Jump — the Menu map chooses its own).
    pub(crate) fn bind_bench_nav(&mut self) {
        use crate::device::GamepadButton;
        self.bind(
            ActionSignal::PanelPrev,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                direction: AxisDirection::Negative,
            },
        );
        self.bind(
            ActionSignal::PanelNext,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                direction: AxisDirection::Positive,
            },
        );
        self.bind(ActionSignal::NavUp, InputBinding::Key(Key::Up));
        self.bind(ActionSignal::NavDown, InputBinding::Key(Key::Down));
        self.bind(ActionSignal::NavLeft, InputBinding::Key(Key::Left));
        self.bind(ActionSignal::NavRight, InputBinding::Key(Key::Right));
        self.bind(
            ActionSignal::NavUp,
            InputBinding::GamepadButton(GamepadButton::DPadUp),
        );
        self.bind(
            ActionSignal::NavDown,
            InputBinding::GamepadButton(GamepadButton::DPadDown),
        );
        self.bind(
            ActionSignal::NavLeft,
            InputBinding::GamepadButton(GamepadButton::DPadLeft),
        );
        self.bind(
            ActionSignal::NavRight,
            InputBinding::GamepadButton(GamepadButton::DPadRight),
        );
        self.bind(ActionSignal::Confirm, InputBinding::Key(Key::Enter));
        self.bind(
            ActionSignal::Confirm,
            InputBinding::GamepadButton(GamepadButton::South),
        );
        self.bind(
            ActionSignal::Cancel,
            InputBinding::GamepadButton(GamepadButton::East),
        );
        // The stick AXIS-SPLIT (one physical input binds exactly one signal —
        // binding.rs invariant): X is the pane cursor (Panel*, above — its own intent,
        // NOT the d-pad's Nav*), Y is the entered viewport's camera (Zoom*, the Populous
        // globe): up = ZoomIn, down = ZoomOut. So VERTICAL pane moves are the d-pad's —
        // the stick cannot also drive them without stealing the globe's zoom. The wheel
        // stays the pointer's zoom, outside the signal map.
        self.bind(
            ActionSignal::ZoomIn,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            },
        );
        self.bind(
            ActionSignal::ZoomOut,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Negative,
            },
        );
        // The RIGHT stick looks around — the other half of the three-tier floor,
        // and dead hardware on every bench until this binding existed: a scene
        // that wanted an orbit had to reach past the map for `gamepad(0)`
        // directly, which is exactly the reach this map is here to remove. The
        // signal carries the DIRECTION, so each of the four names its own half
        // of an axis and a reader takes the difference.
        self.bind(
            ActionSignal::LookRight,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Positive,
            },
        );
        self.bind(
            ActionSignal::LookLeft,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Negative,
            },
        );
        self.bind(
            ActionSignal::LookUp,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Positive,
            },
        );
        self.bind(
            ActionSignal::LookDown,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Negative,
            },
        );
        // The chord modifier (Y / North — Aaron's ergonomic ruling: A/B are
        // Confirm/Cancel, so Y opens the chord). Bound PLAIN here, not as a
        // suppressing `modifier`: a bench holds it to scale a step (chord +
        // d-pad = ±10), read via `signal_held` beside the step's own edge. A
        // bench that grows real chord VERBS graduates to the ChordLayer.
        self.bind(
            ActionSignal::ChordBegin,
            InputBinding::GamepadButton(GamepadButton::North),
        );
    }

    /// The two menu-rail scales, on both devices — ONE definition, shared by the
    /// `World` preset above (bench chrome) and the `Menu` context map (modal
    /// menus), so the two cannot drift (spec §7.1: no hand-duplicated binding
    /// lists). The BUMPERS cycle a section tab and the TRIGGERS cycle the whole
    /// page — the same two scales the `paged_menu` surface draws as its LB/RB
    /// and LT/RT rails. The keyboard equivalents keep that shape: the adjacent
    /// `,`/`.` pair for the inner rail, the outer `[`/`]` pair for the outer
    /// one, so the reach matches the scale of the move.
    pub(crate) fn bind_menu_rails(&mut self) {
        use crate::device::GamepadButton;
        self.bind(ActionSignal::TabPrev, InputBinding::Key(Key::Comma));
        self.bind(ActionSignal::TabNext, InputBinding::Key(Key::Period));
        self.bind(
            ActionSignal::TabPrev,
            InputBinding::GamepadButton(GamepadButton::LeftBumper),
        );
        self.bind(
            ActionSignal::TabNext,
            InputBinding::GamepadButton(GamepadButton::RightBumper),
        );
        self.bind(ActionSignal::PagePrev, InputBinding::Key(Key::LeftBracket));
        self.bind(ActionSignal::PageNext, InputBinding::Key(Key::RightBracket));
        self.bind(
            ActionSignal::PagePrev,
            InputBinding::GamepadButton(GamepadButton::LeftTrigger),
        );
        self.bind(
            ActionSignal::PageNext,
            InputBinding::GamepadButton(GamepadButton::RightTrigger),
        );
    }

    /// ESDF keyboard layout. Same idea as WASD but shifted right.
    pub fn esdf_and_mouse() -> Self {
        let mut map = Self::empty();
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::E));
        map.bind(ActionSignal::MoveBackward, InputBinding::Key(Key::D));
        map.bind(ActionSignal::StrafeLeft, InputBinding::Key(Key::S));
        map.bind(ActionSignal::StrafeRight, InputBinding::Key(Key::F));
        map.bind(ActionSignal::MoveUp, InputBinding::Key(Key::R));
        map.bind(ActionSignal::MoveDown, InputBinding::Key(Key::W));
        map.bind(ActionSignal::Quit, InputBinding::Key(Key::Escape));
        map.bind(ActionSignal::Menu, InputBinding::Key(Key::Escape));
        // Gamepad menu-open (Start / hamburger), same as wasd_and_mouse.
        map.bind(
            ActionSignal::Menu,
            InputBinding::GamepadButton(crate::device::GamepadButton::Start),
        );
        map.bind(
            ActionSignal::PrimaryAction,
            InputBinding::MouseButton(MouseButton::Left),
        );
        map.bind(
            ActionSignal::SecondaryAction,
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
        use crate::device::GamepadButton;
        let mut map = Self::empty();
        // Movement via left stick
        map.bind(
            ActionSignal::MoveForward,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::MoveBackward,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Negative,
            },
        );
        map.bind(
            ActionSignal::StrafeRight,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::StrafeLeft,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                direction: AxisDirection::Negative,
            },
        );
        // Look via right stick
        map.bind(
            ActionSignal::LookRight,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::LookLeft,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Negative,
            },
        );
        map.bind(
            ActionSignal::LookUp,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::LookDown,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Negative,
            },
        );
        // Face buttons
        map.bind(
            ActionSignal::Jump,
            InputBinding::GamepadButton(GamepadButton::South),
        );
        map.bind(
            ActionSignal::Cancel,
            InputBinding::GamepadButton(GamepadButton::East),
        );
        map.bind(
            ActionSignal::Interact,
            InputBinding::GamepadButton(GamepadButton::West),
        );
        map.bind(
            ActionSignal::Inventory,
            InputBinding::GamepadButton(GamepadButton::North),
        );
        // Triggers & bumpers
        map.bind(
            ActionSignal::PrimaryAction,
            InputBinding::GamepadButton(GamepadButton::RightTrigger),
        );
        map.bind(
            ActionSignal::SecondaryAction,
            InputBinding::GamepadButton(GamepadButton::LeftTrigger),
        );
        map.bind(
            ActionSignal::Sprint,
            InputBinding::GamepadButton(GamepadButton::LeftBumper),
        );
        map.bind(
            ActionSignal::Crouch,
            InputBinding::GamepadButton(GamepadButton::RightBumper),
        );
        // Meta
        map.bind(
            ActionSignal::Menu,
            InputBinding::GamepadButton(GamepadButton::Start),
        );
        map.bind(
            ActionSignal::Map,
            InputBinding::GamepadButton(GamepadButton::Select),
        );
        map.bind(
            ActionSignal::Quit,
            InputBinding::GamepadButton(GamepadButton::Guide),
        );
        map
    }

    /// The Look / Interact / Menu / free-click binds SHARED by both flight-camera modes
    /// ([`flight_path`](Self::flight_path) on the rail, [`flying`](Self::flying) off it):
    /// only the LEFT STICK differs between them (throttle vs zoom). This is the aerial
    /// case of the input-context-is-a-vehicle model (MCP `3B4DB4C2`) — one flight camera,
    /// two modes/contexts.
    ///
    /// - **Look** — right stick, and mouse RIGHT-drag (a [`MouseMotion`](InputBinding::MouseMotion)
    ///   gated on RMB, the MMO default): left-click stays free for select-target. The
    ///   stick is a rate (a caller multiplies by `dt`), the mouse delta is frame-absolute
    ///   — the two channels [`signal_axis`](crate::ContextualBindings::signal_axis) and
    ///   [`signal_pointer_delta`](crate::ContextualBindings::signal_pointer_delta) resolve.
    /// - **Interact** — controller West (X) and keyboard `E`: the "use" edge a flight
    ///   scene maps onto start/restart.
    /// - **Menu** — `Esc` and Start, so pause still opens from the active context (the
    ///   active map is the TOP of the stack, never merged down it).
    fn flight_camera_common(map: &mut InputMap) {
        use crate::device::GamepadButton;
        // Look via right stick (mirrors gamepad_default's convention: +Y = up).
        map.bind(
            ActionSignal::LookRight,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::LookLeft,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                direction: AxisDirection::Negative,
            },
        );
        map.bind(
            ActionSignal::LookUp,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::LookDown,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                direction: AxisDirection::Negative,
            },
        );
        // Look via mouse RIGHT-drag (left stays free). Screen y is down, so look up = −Y.
        map.bind(
            ActionSignal::LookRight,
            InputBinding::MouseMotion {
                axis: MouseAxis::X,
                direction: AxisDirection::Positive,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(
            ActionSignal::LookLeft,
            InputBinding::MouseMotion {
                axis: MouseAxis::X,
                direction: AxisDirection::Negative,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(
            ActionSignal::LookDown,
            InputBinding::MouseMotion {
                axis: MouseAxis::Y,
                direction: AxisDirection::Positive,
                gate: Some(MouseButton::Right),
            },
        );
        map.bind(
            ActionSignal::LookUp,
            InputBinding::MouseMotion {
                axis: MouseAxis::Y,
                direction: AxisDirection::Negative,
                gate: Some(MouseButton::Right),
            },
        );
        // Interact = start/restart the cinematic: controller West (X) + keyboard E.
        map.bind(ActionSignal::Interact, InputBinding::Key(Key::E));
        map.bind(
            ActionSignal::Interact,
            InputBinding::GamepadButton(GamepadButton::West),
        );
        // Left-click stays FREE for select-target — bound to a NAMED signal a future
        // pick consumes, never a raw poll; RMB is the look gate, also named.
        map.bind(
            ActionSignal::PrimaryAction,
            InputBinding::MouseButton(MouseButton::Left),
        );
        map.bind(
            ActionSignal::SecondaryAction,
            InputBinding::MouseButton(MouseButton::Right),
        );
        // Pause still opens from the active context: Esc + Start (mirrors wasd_and_mouse).
        map.bind(ActionSignal::Quit, InputBinding::Key(Key::Escape));
        map.bind(ActionSignal::Menu, InputBinding::Key(Key::Escape));
        map.bind(
            ActionSignal::Menu,
            InputBinding::GamepadButton(GamepadButton::Start),
        );
    }

    /// The flight camera ON its rail — the map for context
    /// [`FlightPath`](crate::InputContext::FlightPath). The **LEFT STICK is THROTTLE**
    /// (LeftStickY → `MoveForward`/`MoveBackward`, mirrored to `W`/`S` so it drives with
    /// no pad): a caller scales the flight's rail speed by the Move-axis difference. A
    /// Look gesture drops out to [`flying`](Self::flying) (free camera). Shares
    /// look/interact/menu with `flying` — only the left stick differs.
    pub fn flight_path() -> Self {
        let mut map = Self::empty();
        // Throttle: left stick Y, mirrored to W/S so the flight is drivable KBM-only.
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
        map.bind(ActionSignal::MoveBackward, InputBinding::Key(Key::S));
        map.bind(
            ActionSignal::MoveForward,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::MoveBackward,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Negative,
            },
        );
        Self::flight_camera_common(&mut map);
        map
    }

    /// The flight camera OFF the rail — the map for context
    /// [`Flying`](crate::InputContext::Flying), the free manual camera. The **LEFT STICK
    /// is ZOOM** (LeftStickY → `ZoomIn`/`ZoomOut`; the mouse wheel zooms too) and look
    /// PANS. There is NO throttle off the rail (nothing to speed up); Interact restarts
    /// the flight, re-entering [`flight_path`](Self::flight_path). Shares
    /// look/interact/menu with `flight_path`.
    pub fn flying() -> Self {
        let mut map = Self::empty();
        // Zoom (dolly): left stick Y. The wheel zooms too (pointer channel), so this is
        // the pad equivalent — no throttle here, the camera is off the rail.
        map.bind(
            ActionSignal::ZoomIn,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            },
        );
        map.bind(
            ActionSignal::ZoomOut,
            InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Negative,
            },
        );
        Self::flight_camera_common(&mut map);
        map
    }

    /// The default Xbox controller config — the controller tab's default selection
    /// (Aaron 2026-08-14, `xbox_souls`). Ruled layout (MCP `DE46BDB8` / `D167E43A`).
    /// Physical positions: A=`South` B=`East` X=`West` Y=`North`. Binds the **press** of
    /// each control; tap-vs-hold, long-press and the Y-modifier chords (Y+LT `Kick`,
    /// Y+LB&LT `CounterPerilous`/mikiri — `1195F78A`) live in the input-concept layer, so
    /// they are absent here. Signals added since are simply unbound (the derived keyboard
    /// page, S4a, exposes them to bind).
    pub fn xbox_souls() -> Self {
        use crate::device::GamepadButton;
        let mut map = Self::empty();
        // Move + look come from the 120 Hz analog channel (`InputState::analog_latch`),
        // NOT the discrete map (spec RT-12 / §7.1): binding the sticks here as well would
        // double-drive `signal_held(MoveForward)` (once digital, once from the stick). Only
        // the stick *presses* stay discrete.
        map.bind(
            ActionSignal::LockOn,
            InputBinding::GamepadButton(GamepadButton::RightStick),
        );
        map.bind(
            ActionSignal::Crouch,
            InputBinding::GamepadButton(GamepadButton::LeftStick),
        );
        // Attacks / defend / special on the shoulders.
        map.bind(
            ActionSignal::AttackLight,
            InputBinding::GamepadButton(GamepadButton::RightBumper),
        );
        map.bind(
            ActionSignal::AttackHeavy,
            InputBinding::GamepadButton(GamepadButton::RightTrigger),
        );
        map.bind(
            ActionSignal::Defend,
            InputBinding::GamepadButton(GamepadButton::LeftBumper),
        );
        map.bind(
            ActionSignal::Special,
            InputBinding::GamepadButton(GamepadButton::LeftTrigger),
        );
        // Face buttons (A=South, B=East, X=West, Y=North).
        map.bind(
            ActionSignal::Dodge,
            InputBinding::GamepadButton(GamepadButton::East),
        );
        map.bind(
            ActionSignal::Jump,
            InputBinding::GamepadButton(GamepadButton::North),
        );
        map.bind(
            ActionSignal::Interact,
            InputBinding::GamepadButton(GamepadButton::South),
        );
        map.bind(
            ActionSignal::Interact,
            InputBinding::GamepadButton(GamepadButton::West),
        );
        // Meta.
        map.bind(
            ActionSignal::Menu,
            InputBinding::GamepadButton(GamepadButton::Start),
        );
        map.bind(
            ActionSignal::Map,
            InputBinding::GamepadButton(GamepadButton::Select),
        );
        map
    }
}

impl Default for InputMap {
    fn default() -> Self {
        Self::wasd_and_mouse()
    }
}

// ───────────────────────────────────────────────────────────────────
// Section: Binding descriptors (§3.5 — the three input concepts)
// ───────────────────────────────────────────────────────────────────

/// How a bound control's press is interpreted. Toggle-vs-hold is a user-settings
/// choice per binding (e.g. Crouch defaults `Toggle` with a hold option,
/// `4AC56490 §3`). Scaffolded this slice; wired to the resolver when combat
/// scenes land.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Activation {
    /// Fires while the control is held (momentary).
    #[default]
    Press,
    /// First press latches on, second press latches off.
    Toggle,
    /// Fires only after being held past a threshold.
    Hold,
}

/// The three input concepts (`C60AE43C §6`) encoded per binding, not as new
/// signals: **stance** (continuous, read via `signal_held`), **long-press**
/// ([`on_hold`](Self::on_hold)), and **modifier-chord** ([`modifier`](Self::modifier)).
///
/// A scaffold this slice — the per-context reshape table that carries these is
/// promoted with `InputProfile` in a later build step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BindingDescriptor {
    /// Press / toggle / hold interpretation.
    pub activation: Activation,
    /// If set, a tap fires the primary signal while a hold past `threshold_ticks`
    /// fires this alternate signal with `EventKind::Hold`.
    pub on_hold: Option<(ActionSignal, u32)>,
    /// When `true`, this control opens the chord layer: it emits `ChordBegin`
    /// and suppresses the members' normal signals while held.
    pub modifier: bool,
}

// ───────────────────────────────────────────────────────────────────
// Section: Tests
// ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::GamepadButton;
    use crate::snapshot::InputState;

    // ── InputMap tests ──

    #[test]
    fn input_map_wasd_and_mouse() {
        let map = InputMap::wasd_and_mouse();
        assert_eq!(
            map.action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward)
        );
        assert_eq!(
            map.action_for(InputBinding::MouseButton(MouseButton::Left)),
            Some(ActionSignal::PrimaryAction)
        );
    }

    /// S9b: the default preset's ADDITIVE combat-feed binds — RShift sprints
    /// beside LShift, C crouches beside LCtrl — so paperdoll's signal-fed
    /// `Inputs` behave identically to its old raw `key_down` reads on the same
    /// physical keys. Additive means the originals still resolve too.
    #[test]
    fn wasd_preset_carries_the_additive_combat_binds() {
        let map = InputMap::wasd_and_mouse();
        assert_eq!(
            map.action_for(InputBinding::Key(Key::LeftShift)),
            Some(ActionSignal::Sprint)
        );
        assert_eq!(
            map.action_for(InputBinding::Key(Key::RightShift)),
            Some(ActionSignal::Sprint)
        );
        assert_eq!(
            map.action_for(InputBinding::Key(Key::LeftControl)),
            Some(ActionSignal::Crouch)
        );
        assert_eq!(
            map.action_for(InputBinding::Key(Key::C)),
            Some(ActionSignal::Crouch)
        );
        assert_eq!(
            map.action_for(InputBinding::Key(Key::Space)),
            Some(ActionSignal::Jump)
        );
    }

    /// The FLATTENED bench-nav tier (plan 1A292918 T2): the World preset benches run
    /// under must bind the whole focus tier, or a control is dead hardware (drift-gate
    /// 8634C200). Pins the SEPARATE-INTENT contract (Aaron 2026-08-18): the d-pad drives
    /// `Nav*`, the left stick drives its OWN `Panel*` — the two are never tangled.
    #[test]
    fn the_world_preset_covers_the_whole_bench_focus_tier() {
        use crate::device::GamepadButton;
        let map = InputMap::wasd_and_mouse();
        for sig in [
            ActionSignal::NavUp,
            ActionSignal::NavDown,
            ActionSignal::NavLeft,
            ActionSignal::NavRight,
            ActionSignal::Confirm,
            ActionSignal::Cancel,
        ] {
            assert!(
                !map.bindings_for(sig).is_empty(),
                "{sig:?} is unbound in the bench World map — dead hardware"
            );
        }
        // The stick is AXIS-SPLIT and emits its OWN pane intent: X → Panel* (the pane
        // cursor, separate from the d-pad's Nav*), Y → Zoom (the entered globe's camera).
        // The d-pad — never the stick — owns Nav*, so in-control components (sliders) that
        // answer only Nav* are never disturbed by the stick.
        assert!(
            map.bindings_for(ActionSignal::PanelPrev)
                .iter()
                .any(|b| matches!(
                    b,
                    InputBinding::GamepadAxis {
                        axis: GamepadAxis::LeftStickX,
                        direction: AxisDirection::Negative
                    }
                )),
            "left stick X- is the stick's OWN pane intent (PanelPrev), not Nav*"
        );
        assert!(
            !map.bindings_for(ActionSignal::NavLeft)
                .iter()
                .any(|b| matches!(
                    b,
                    InputBinding::GamepadAxis {
                        axis: GamepadAxis::LeftStickX,
                        ..
                    }
                )),
            "Nav* is the D-PAD's alone — the stick must not be tangled into it"
        );
        assert!(
            map.bindings_for(ActionSignal::NavUp)
                .iter()
                .any(|b| matches!(b, InputBinding::GamepadButton(GamepadButton::DPadUp))),
            "the d-pad owns vertical pane moves (stick Y is the camera's)"
        );
        assert!(
            map.bindings_for(ActionSignal::ZoomIn)
                .iter()
                .any(|b| matches!(
                    b,
                    InputBinding::GamepadAxis {
                        axis: GamepadAxis::LeftStickY,
                        direction: AxisDirection::Positive
                    }
                )),
            "stick Y+ stays the entered viewport's zoom (Populous globe — not regressed)"
        );
    }

    /// The flight camera's TWO modes (MCP `3B4DB4C2` / Solar Birth signal map `5B9A8B50`):
    /// the LEFT STICK is throttle ON the rail (`flight_path`) and zoom OFF it (`flying`);
    /// everything else is SHARED — look is a bound signal on BOTH the right stick and
    /// mouse RIGHT-drag (never LEFT — left stays free for select-target), Interact is
    /// West + E, and Menu survives so pause still opens.
    #[test]
    fn flight_camera_presets_bind_the_two_mode_contract() {
        use crate::device::GamepadButton;
        let rail = InputMap::flight_path();
        let free = InputMap::flying();

        // The LEFT STICK differs by mode: throttle on the rail, zoom off it — and each
        // mode binds ONLY its own (the rail never zooms, the free camera never throttles).
        assert_eq!(
            rail.action_for(InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            }),
            Some(ActionSignal::MoveForward),
        );
        assert_eq!(
            free.action_for(InputBinding::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                direction: AxisDirection::Positive,
            }),
            Some(ActionSignal::ZoomIn),
        );
        assert!(rail.bindings_for(ActionSignal::ZoomIn).is_empty());
        assert!(free.bindings_for(ActionSignal::MoveForward).is_empty());

        // SHARED by both modes: look on right-stick + RMB-drag (never left), left-click
        // free, Interact West+E, Menu Esc.
        for map in [&rail, &free] {
            assert_eq!(
                map.action_for(InputBinding::GamepadAxis {
                    axis: GamepadAxis::RightStickX,
                    direction: AxisDirection::Positive,
                }),
                Some(ActionSignal::LookRight),
            );
            assert!(map
                .bindings_for(ActionSignal::LookRight)
                .iter()
                .any(|b| matches!(
                    b,
                    InputBinding::MouseMotion {
                        gate: Some(MouseButton::Right),
                        ..
                    }
                )));
            assert!(!map
                .bindings_for(ActionSignal::LookRight)
                .iter()
                .any(|b| matches!(
                    b,
                    InputBinding::MouseMotion {
                        gate: Some(MouseButton::Left),
                        ..
                    }
                )));
            assert_eq!(
                map.action_for(InputBinding::MouseButton(MouseButton::Left)),
                Some(ActionSignal::PrimaryAction),
            );
            assert!(map
                .bindings_for(ActionSignal::Interact)
                .contains(&InputBinding::GamepadButton(GamepadButton::West)));
            assert!(map
                .bindings_for(ActionSignal::Interact)
                .contains(&InputBinding::Key(Key::E)));
            assert!(map
                .bindings_for(ActionSignal::Menu)
                .contains(&InputBinding::Key(Key::Escape)));
        }
    }

    #[test]
    fn input_map_bind_unbind() {
        let mut map = InputMap::empty();
        map.bind(ActionSignal::Jump, InputBinding::Key(Key::Space));
        assert_eq!(
            map.action_for(InputBinding::Key(Key::Space)),
            Some(ActionSignal::Jump)
        );
        map.unbind(ActionSignal::Jump, InputBinding::Key(Key::Space));
        assert_eq!(map.action_for(InputBinding::Key(Key::Space)), None);
    }

    #[test]
    fn input_map_rebind_enforces_one_action_per_input() {
        let mut map = InputMap::empty();
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
        map.bind(ActionSignal::MoveBackward, InputBinding::Key(Key::W));
        // W should now map to MoveBackward, not MoveForward
        assert_eq!(
            map.action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveBackward)
        );
        // MoveForward should have no W binding
        assert!(map.bindings_for(ActionSignal::MoveForward).is_empty());
    }

    #[test]
    fn input_map_multiple_inputs_per_action() {
        let mut map = InputMap::empty();
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
        map.bind(ActionSignal::MoveForward, InputBinding::Key(Key::Up));
        assert_eq!(map.bindings_for(ActionSignal::MoveForward).len(), 2);
    }

    #[test]
    fn input_map_gamepad_default() {
        let map = InputMap::gamepad_default();
        // A/South = jump
        assert_eq!(
            map.action_for(InputBinding::GamepadButton(GamepadButton::South)),
            Some(ActionSignal::Jump)
        );
        // Right trigger = primary action
        assert_eq!(
            map.action_for(InputBinding::GamepadButton(GamepadButton::RightTrigger)),
            Some(ActionSignal::PrimaryAction)
        );
    }

    #[test]
    fn input_map_clear_action() {
        let mut map = InputMap::wasd_and_mouse();
        assert!(!map.bindings_for(ActionSignal::MoveForward).is_empty());
        map.clear_action(ActionSignal::MoveForward);
        assert!(map.bindings_for(ActionSignal::MoveForward).is_empty());
    }

    #[test]
    fn input_map_bound_actions() {
        let map = InputMap::wasd_and_mouse();
        let actions: Vec<ActionSignal> = map.bound_actions().collect();
        assert!(actions.contains(&ActionSignal::MoveForward));
        assert!(actions.contains(&ActionSignal::Quit));
    }

    // ── is_down (the consolidated query) ──

    #[test]
    fn is_down_covers_key_and_gamepad() {
        let cfg = GamepadConfig::default();
        let mut input = InputState::new();
        input.set_key(Key::W, true);
        assert!(InputBinding::Key(Key::W).is_down(&input, &cfg));
        assert!(!InputBinding::Key(Key::S).is_down(&input, &cfg));

        input.gamepad_mut(0).set_axis(GamepadAxis::LeftStickX, 0.9);
        assert!(InputBinding::GamepadAxis {
            axis: GamepadAxis::LeftStickX,
            direction: AxisDirection::Positive,
        }
        .is_down(&input, &cfg));
    }

    // ── InputMap serde (rebinds persist to disk) ──

    #[test]
    fn input_map_round_trips_through_json() {
        // The defect this guards: `input_to_action` was keyed by `InputBinding` (a non-unit
        // enum), so serializing the live map to JSON failed and rebinds never persisted.
        // `wasd_and_mouse` is a rich sample (keys + mouse + pad + a gated MouseMotion).
        let map = InputMap::wasd_and_mouse();
        let json = serde_json::to_string(&map).expect("InputMap serializes to JSON");
        let back: InputMap = serde_json::from_str(&json).expect("InputMap deserializes");
        for action in [
            ActionSignal::MoveForward,
            ActionSignal::Interact,
            ActionSignal::Jump,
            ActionSignal::PrimaryAction,
            ActionSignal::Confirm,
            ActionSignal::Cancel,
        ] {
            assert_eq!(
                map.bindings_for(action),
                back.bindings_for(action),
                "{action} survives"
            );
        }
        // the reverse index is rebuilt on load, not serialized
        assert_eq!(
            back.action_for(InputBinding::GamepadButton(GamepadButton::East)),
            Some(ActionSignal::Cancel),
        );
    }

    /// The ruled Xbox controller layout (MCP `DE46BDB8`) — `xbox_souls` is the controller
    /// tab's default config (Aaron 2026-08-14). Pins the shoulders / face-button / stick
    /// assignments so a careless edit to the provisional layout is loud.
    #[test]
    fn xbox_souls_binds_ruled_layout() {
        use GamepadButton as B;
        let m = InputMap::xbox_souls();
        let expect = |a: ActionSignal, b: B| {
            assert_eq!(m.action_for(InputBinding::GamepadButton(b)), Some(a));
        };
        expect(ActionSignal::AttackLight, B::RightBumper);
        expect(ActionSignal::AttackHeavy, B::RightTrigger);
        expect(ActionSignal::Defend, B::LeftBumper);
        expect(ActionSignal::Special, B::LeftTrigger);
        expect(ActionSignal::Dodge, B::East); // B = dodge (hold-B sprint is the concept layer)
        expect(ActionSignal::Jump, B::North); // Y = jump
        expect(ActionSignal::LockOn, B::RightStick);
        expect(ActionSignal::Crouch, B::LeftStick);
    }

    // ── Binding descriptor scaffold ──

    #[test]
    fn binding_descriptor_defaults() {
        let d = BindingDescriptor::default();
        assert_eq!(d.activation, Activation::Press);
        assert!(d.on_hold.is_none());
        assert!(!d.modifier);
    }
}
