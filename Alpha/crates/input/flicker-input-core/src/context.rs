//! Input contexts and the per-context binding stack.
//!
//! [`InputContext`] is an **open newtype** (spec R5): a `u16` with associated
//! consts for the built-in contexts plus open registration for consumer-defined
//! ones. Call sites use `==` / `!=`, so `InputContext::World` reads identically
//! to the old enum variant. [`ContextualBindings`] migrates unchanged in shape
//! and behavior and gains [`signal_held`](ContextualBindings::signal_held).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::analog::AbstractControls;
use crate::binding::{InputBinding, InputMap};
use crate::resolve::EventKind;
use crate::signal::ActionSignal;
use crate::snapshot::{GamepadConfig, InputState};

// ───────────────────────────────────────────────────────────────────
// Section: Input Contexts
// ───────────────────────────────────────────────────────────────────

/// Which action map is live. A first-class **engine service**: the same physical button
/// can mean different things per context (LB = `Defend` in `World`, tab in `Menu`).
///
/// An **open newtype** rather than a closed enum: the built-in contexts are associated
/// consts, and consumers register their own with [`InputContext::register`] (or the
/// [`FIRST_CUSTOM`](InputContext::FIRST_CUSTOM) base). Ratified in MCP `4AC56490 §5` /
/// spec R5.
///
/// Note (spec §7.1a): a persisted profile keys contexts by **stable string name**, not by
/// this `u16` — the integer ids are runtime-assigned and not stable across registry
/// changes. Serialization by name lands with `InputProfile` in a later build step.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InputContext(pub u16);

#[allow(non_upper_case_globals)] // consts mirror the old enum variants so `== InputContext::World` reads unchanged
impl InputContext {
    /// Gameplay: movement + combat. The immutable base of every stack.
    pub const World: InputContext = InputContext(0);
    /// Menus / inventory / journal navigation.
    pub const Menu: InputContext = InputContext(1);
    /// A radial selector (chat wheel, item radial).
    pub const Radial: InputContext = InputContext(2);
    /// A text field owns the keyboard (chat entry, rename, search). Gameplay
    /// movement/look is suppressed while active; typed characters flow to the
    /// focused UI field instead. Register it with an empty map so no gameplay
    /// signal resolves; the base `World` map still underlies it on the stack.
    pub const TextEntry: InputContext = InputContext(3);
    /// Riding a creature.
    pub const Mounted: InputContext = InputContext(4);
    /// Free flight.
    pub const Flying: InputContext = InputContext(5);
    /// Piloting a vehicle.
    pub const Vehicle: InputContext = InputContext(6);
    /// The CHORD LAYER: a modifier is held (`ActionSignal::ChordBegin`), so the
    /// controls under it mean editor verbs instead of their normal signals.
    ///
    /// Layering it as a context is what gives the documented ChordBegin contract
    /// — *"members' normal signals are suppressed while it is held"* — for free:
    /// while this is on the stack it IS the active map, so a control bound to
    /// `ModePrev` in the base map resolves to `Cut` here and to nothing else.
    /// Push on the modifier's press edge, pop on its release ([`chord`]).
    ///
    /// [`chord`]: crate::chord
    pub const Chord: InputContext = InputContext(7);

    /// First id available to consumer-defined contexts (built-ins occupy `0..FIRST_CUSTOM`).
    pub const FIRST_CUSTOM: u16 = 8;

    /// Register/refer to a consumer-defined context by raw id. Callers should
    /// use ids `>= FIRST_CUSTOM` to avoid colliding with the built-ins.
    pub fn register(id: u16) -> InputContext {
        InputContext(id)
    }

    /// The raw id backing this context.
    pub fn id(self) -> u16 {
        self.0
    }
}

impl InputContext {
    /// Built-in context ↔ stable NAME registry (spec §7.1a / Risk RT-4). A persisted
    /// [`InputProfile`] keys contexts by these NAMES, never by the runtime `u16` (the
    /// open registry assigns ids that are not stable across runs); load resolves the
    /// name back to an [`InputContext`] through here. The names are **FROZEN**, exactly
    /// like [`ActionSignal`](crate::ActionSignal) variant names — ADD only, never
    /// rename (`C60AE43C §2`).
    pub const BUILTIN_NAMES: &'static [(&'static str, InputContext)] = &[
        ("World", InputContext::World),
        ("Menu", InputContext::Menu),
        ("Radial", InputContext::Radial),
        ("TextEntry", InputContext::TextEntry),
        ("Mounted", InputContext::Mounted),
        ("Flying", InputContext::Flying),
        ("Vehicle", InputContext::Vehicle),
        ("Chord", InputContext::Chord),
    ];

    /// Resolve a built-in context NAME to its [`InputContext`], or `None` if the name
    /// is not a built-in (a consumer-registered context has no frozen name).
    pub fn from_name(name: &str) -> Option<InputContext> {
        Self::BUILTIN_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, c)| c)
    }

    /// The stable NAME of a built-in context, or `None` for a consumer-registered one
    /// (which is runtime-only and never persisted).
    pub fn name(self) -> Option<&'static str> {
        Self::BUILTIN_NAMES
            .iter()
            .find(|&&(_, c)| c == self)
            .map(|&(n, _)| n)
    }
}

/// Per-context input maps plus the active-context stack — the runtime home of the
/// [`InputContext`] service.
///
/// A stack, not a single value, because contexts nest: opening a menu over the world
/// pushes [`InputContext::Menu`] and closing it pops back to whatever was underneath. The
/// base of the stack is always [`InputContext::World`], so [`active`](Self::active) always
/// has an answer. A context with no map of its own falls back to the `World` map, so a
/// partially-configured context still moves the camera rather than going dead.
///
/// Serializes through [`ContextualBindingsData`] (the `#[serde(from/into)]` pattern
/// [`InputMap`] uses): the per-context maps persist keyed by **stable NAME** and the
/// runtime `stack` is skipped, rebuilt to `[World]` on load (spec §7.1a). Only built-in
/// (named) contexts round-trip; a consumer-registered context is runtime-only.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(from = "ContextualBindingsData", into = "ContextualBindingsData")]
pub struct ContextualBindings {
    maps: HashMap<InputContext, InputMap>,
    stack: Vec<InputContext>,
}

/// On-disk form of [`ContextualBindings`]: the per-context maps as name-keyed pairs,
/// with the runtime `stack` omitted (rebuilt to `[World]` on load). Mirrors
/// [`InputMapData`](crate::binding::InputMap) — serializing by NAME keeps saved data
/// stable across registry-id changes (spec §7.1a / RT-4). Contexts without a frozen
/// built-in name are runtime-only and are skipped.
#[derive(Serialize, Deserialize)]
struct ContextualBindingsData {
    maps: Vec<(String, InputMap)>,
}

impl From<ContextualBindings> for ContextualBindingsData {
    fn from(cb: ContextualBindings) -> Self {
        // Iterate the registry (not the HashMap) so the output order is stable.
        let mut maps = Vec::new();
        for (name, ctx) in InputContext::BUILTIN_NAMES {
            if let Some(m) = cb.maps.get(ctx) {
                maps.push(((*name).to_string(), m.clone()));
            }
        }
        Self { maps }
    }
}

impl From<ContextualBindingsData> for ContextualBindings {
    fn from(data: ContextualBindingsData) -> Self {
        // World must exist for `active_map` never to panic; fall back to empty.
        let world = data
            .maps
            .iter()
            .find(|(n, _)| n == "World")
            .map(|(_, m)| m.clone())
            .unwrap_or_else(InputMap::empty);
        let mut cb = ContextualBindings::new(world);
        for (name, map) in data.maps {
            if name == "World" {
                continue;
            }
            if let Some(ctx) = InputContext::from_name(&name) {
                cb.maps.insert(ctx, map);
            }
        }
        cb
    }
}

impl ContextualBindings {
    /// New service with `world` as the base `World` map. Add more contexts with
    /// [`with`](Self::with).
    pub fn new(world: InputMap) -> Self {
        let mut maps = HashMap::new();
        maps.insert(InputContext::World, world);
        Self { maps, stack: vec![InputContext::World] }
    }

    /// Register a map for a context (builder style).
    pub fn with(mut self, context: InputContext, map: InputMap) -> Self {
        self.maps.insert(context, map);
        self
    }

    /// The context currently on top of the stack.
    pub fn active(&self) -> InputContext {
        self.stack.last().copied().unwrap_or(InputContext::World)
    }

    /// Push a context (e.g. a menu opening over the world).
    pub fn push(&mut self, context: InputContext) {
        self.stack.push(context);
    }

    /// Pop the top context, never emptying below the `World` base. Returns the popped
    /// context, or `None` if only the base remained.
    pub fn pop(&mut self) -> Option<InputContext> {
        if self.stack.len() > 1 {
            self.stack.pop()
        } else {
            None
        }
    }

    /// The map for the active context, falling back to the `World` map.
    pub fn active_map(&self) -> &InputMap {
        self.maps
            .get(&self.active())
            .or_else(|| self.maps.get(&InputContext::World))
            .expect("the World map is always present")
    }

    /// Is `action` pressed in the active context?
    pub fn action_pressed(&self, action: ActionSignal, input: &InputState) -> bool {
        self.active_map().action_pressed(action, input)
    }

    /// Continuous / stance-level query: is any binding for `signal` held in the
    /// active context (deadzone/threshold-aware)? Replaces the scenes'
    /// `input.input_active(map, MoveForward)` poll (spec §3.2). No edge, no
    /// resolver state.
    pub fn signal_held(&self, signal: ActionSignal, s: &InputState, cfg: &GamepadConfig) -> bool {
        self.active_map()
            .bindings_for(signal)
            .iter()
            .any(|&b| b.is_down(s, cfg))
    }

    /// Rebuild a runtime binding stack from a persisted [`InputProfile`]: install each
    /// named context's map (resolving the stable NAME → [`InputContext`] via the
    /// registry; unknown names are skipped), and reset the stack to `[World]` (spec
    /// §7.1a). The profile's per-context `default_event` / `signals` reshape is not yet
    /// carried by the runtime stack (a scaffold this slice, spec §3.5) — only the maps
    /// install; the reshape survives in the profile itself.
    pub fn from_profile(profile: &InputProfile) -> Self {
        let world = profile
            .context_map("World")
            .cloned()
            .unwrap_or_else(InputMap::empty);
        let mut cb = Self::new(world);
        for (name, ctx) in &profile.contexts {
            if name == "World" {
                continue;
            }
            if let Some(context) = InputContext::from_name(name) {
                cb.maps.insert(context, ctx.map.clone());
            }
        }
        cb
    }

    /// Snapshot the current per-context maps into an [`InputProfile`] for persistence —
    /// built-in (named) contexts only, in registry order, paired with the caller's
    /// `controls` / `gamepad`. Runtime-only (unnamed) contexts are skipped (spec
    /// §7.1a). The map-level inverse of [`from_profile`](Self::from_profile).
    pub fn to_profile(
        &self,
        name: impl Into<String>,
        controls: AbstractControls,
        gamepad: GamepadConfig,
    ) -> InputProfile {
        let mut contexts = Vec::new();
        for (cname, ctx) in InputContext::BUILTIN_NAMES {
            if let Some(map) = self.maps.get(ctx) {
                contexts.push((
                    (*cname).to_string(),
                    ContextBindings {
                        map: map.clone(),
                        default_event: EventKind::Press,
                        signals: Vec::new(),
                    },
                ));
            }
        }
        InputProfile {
            schema: InputProfile::SCHEMA,
            name: name.into(),
            contexts,
            controls,
            gamepad,
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Section: Profile model (contexts + bindings as DATA — spec §7.1)
// ───────────────────────────────────────────────────────────────────

/// A per-(control, context) event-kind reshape entry (spec §7.1): the control fires
/// `signal` with a specific [`EventKind`] in this context, overriding the context's
/// `default_event`. Promotes the `signals.rs` mockup's per-context reshape into a
/// canonical serde type. Empty for simple contexts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalBinding {
    pub control: InputBinding,
    pub event: EventKind,
    pub signal: ActionSignal,
}

/// One context's bindings as DATA (spec §7.1): the base physical→signal [`InputMap`],
/// the context's `default_event` (combat = `Press`, menus = `Release`, …), and the
/// per-signal `EventKind` reshape table (empty for simple contexts). `InputMap` stays
/// the base map; `signals` only carries the controls that need a non-default event
/// kind (`C60AE43C §6`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextBindings {
    pub map: InputMap,
    pub default_event: EventKind,
    #[serde(default)]
    pub signals: Vec<SignalBinding>,
}

impl ContextBindings {
    /// A simple context: just the base map, `Press` default, no reshape rows.
    pub fn simple(map: InputMap) -> Self {
        Self { map, default_event: EventKind::Press, signals: Vec::new() }
    }
}

/// A complete, named, serializable input profile (spec §7.1): the per-context maps
/// (keyed by stable NAME, §7.1a), the analog control tuning, and the gamepad config.
/// This is the on-disk unit the shell persists so rebinds survive relaunch (spec §7.2).
///
/// `contexts` is an ordered `Vec` of `(context NAME, ContextBindings)` pairs — the same
/// name-keyed, stable-order shape [`InputMapData`](crate::binding::InputMap) uses for
/// its signal→binding pairs — so JSON carries frozen context names, never the
/// runtime `u16`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputProfile {
    /// Schema version, for a future migration (bumped when the shape changes).
    pub schema: u32,
    /// Stable profile id (e.g. `"default"`, `"kbm_souls"`) — persisted, and the value
    /// the settings profile selector round-trips (spec §7.3).
    pub name: String,
    /// Per-context maps + reshape, keyed by stable context NAME (spec §7.1a).
    pub contexts: Vec<(String, ContextBindings)>,
    /// Analog look / move tuning (mouse + stick sensitivity, invert, move speed).
    pub controls: AbstractControls,
    /// Gamepad deadzones + trigger threshold.
    pub gamepad: GamepadConfig,
}

impl InputProfile {
    /// Current profile schema version.
    pub const SCHEMA: u32 = 1;

    /// The built-in named profiles, in menu order: `(stable id, display label)`. The
    /// settings controller tab lists these as its profile selector (spec §7.3);
    /// [`by_name`](Self::by_name) constructs one. The id is persisted as
    /// [`InputProfile::name`]; the label is display-only.
    pub const PRESET_NAMES: &'static [(&'static str, &'static str)] = &[
        ("default", "Default (Keyboard & Mouse)"),
        ("kbm_souls", "Souls — Keyboard & Mouse"),
        ("xbox_souls", "Souls — Gamepad"),
    ];

    /// Construct a named built-in profile, or `None` if the id is unknown.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "default" => Some(Self::default_profile()),
            "kbm_souls" => Some(Self::kbm_souls()),
            "xbox_souls" => Some(Self::xbox_souls()),
            _ => None,
        }
    }

    /// The default profile: `World` = [`InputMap::wasd_and_mouse`], `TextEntry` = empty
    /// (a text field owns the keyboard, so no gameplay signal resolves), and a small
    /// `Menu` map (confirm / cancel / nav). Reuses the preset constructors — no
    /// hand-duplicated binding lists (spec §7.1).
    pub fn default_profile() -> Self {
        Self::from_world("default", InputMap::wasd_and_mouse())
    }

    /// The souls keyboard+mouse profile (`World` = [`InputMap::kbm_souls`]).
    pub fn kbm_souls() -> Self {
        Self::from_world("kbm_souls", InputMap::kbm_souls())
    }

    /// The souls gamepad profile (`World` = [`InputMap::xbox_souls`]).
    pub fn xbox_souls() -> Self {
        Self::from_world("xbox_souls", InputMap::xbox_souls())
    }

    /// Assemble a profile from a `World` map + the shared non-World contexts
    /// (`TextEntry` empty, a simple `Menu` map). The single place the built-in context
    /// set is defined, so every preset stays consistent (spec §7.1 / `DDD070C7`).
    fn from_world(name: &str, world: InputMap) -> Self {
        Self {
            schema: Self::SCHEMA,
            name: name.to_string(),
            contexts: vec![
                ("World".to_string(), ContextBindings::simple(world)),
                ("TextEntry".to_string(), ContextBindings::simple(InputMap::empty())),
                (
                    "Menu".to_string(),
                    // Menus fire on RELEASE (escape a mis-press before letting go, spec §3.2).
                    ContextBindings { map: menu_map(), default_event: EventKind::Release, signals: Vec::new() },
                ),
            ],
            controls: AbstractControls::default(),
            gamepad: GamepadConfig::default(),
        }
    }

    /// The map for a context by stable NAME, if the profile carries one.
    pub fn context_map(&self, name: &str) -> Option<&InputMap> {
        self.contexts.iter().find(|(n, _)| n == name).map(|(_, cb)| &cb.map)
    }

    /// Set (or insert) the map for a context by stable NAME — how the shell writes a
    /// freshly-edited `World` map into the profile before persisting (spec §7.2).
    pub fn set_context_map(&mut self, name: &str, map: InputMap) {
        if let Some((_, cb)) = self.contexts.iter_mut().find(|(n, _)| n == name) {
            cb.map = map;
        } else {
            self.contexts.push((name.to_string(), ContextBindings::simple(map)));
        }
    }
}

impl Default for InputProfile {
    /// The [`default_profile`](Self::default_profile) — used by `#[serde(default)]` (so
    /// an old settings file with no profile still loads) and by derived `Default`s.
    fn default() -> Self {
        Self::default_profile()
    }
}

/// A minimal `Menu` context map: confirm / cancel + directional nav on the arrow keys
/// AND the gamepad (d-pad → `Nav*`, bumpers → `TabNext`/`TabPrev`, A → `Confirm`,
/// B → `Cancel`), so a menu is fully keyboard- **and** pad-navigable (spec §8).
/// Small on purpose — a menu mostly reuses `World` via the fall-through; this only adds
/// the nav / confirm intents a menu needs (spec §7.1 "a simple Menu map").
fn menu_map() -> InputMap {
    use crate::device::{GamepadButton, Key};
    let mut m = InputMap::empty();
    // Keyboard.
    m.bind(ActionSignal::Confirm, InputBinding::Key(Key::Enter));
    m.bind(ActionSignal::Confirm, InputBinding::Key(Key::Space));
    m.bind(ActionSignal::Cancel, InputBinding::Key(Key::Escape));
    m.bind(ActionSignal::NavUp, InputBinding::Key(Key::Up));
    m.bind(ActionSignal::NavDown, InputBinding::Key(Key::Down));
    m.bind(ActionSignal::NavLeft, InputBinding::Key(Key::Left));
    m.bind(ActionSignal::NavRight, InputBinding::Key(Key::Right));
    // Gamepad: d-pad moves, bumpers cycle groups, A confirms, B cancels.
    m.bind(ActionSignal::NavUp, InputBinding::GamepadButton(GamepadButton::DPadUp));
    m.bind(ActionSignal::NavDown, InputBinding::GamepadButton(GamepadButton::DPadDown));
    m.bind(ActionSignal::NavLeft, InputBinding::GamepadButton(GamepadButton::DPadLeft));
    m.bind(ActionSignal::NavRight, InputBinding::GamepadButton(GamepadButton::DPadRight));
    m.bind(ActionSignal::TabPrev, InputBinding::GamepadButton(GamepadButton::LeftBumper));
    m.bind(ActionSignal::TabNext, InputBinding::GamepadButton(GamepadButton::RightBumper));
    m.bind(ActionSignal::Confirm, InputBinding::GamepadButton(GamepadButton::South));
    m.bind(ActionSignal::Cancel, InputBinding::GamepadButton(GamepadButton::East));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::InputBinding;
    use crate::device::{GamepadButton, Key};

    #[test]
    fn world_const_reads_as_base() {
        let cb = ContextualBindings::new(InputMap::empty());
        assert_eq!(cb.active(), InputContext::World);
        assert_eq!(InputContext::World.id(), 0);
        assert!(InputContext::register(64).id() >= InputContext::FIRST_CUSTOM);
    }

    #[test]
    fn context_switch_changes_which_action_a_button_fires() {
        let world = {
            let mut m = InputMap::empty();
            m.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
            m
        };
        let menu = {
            let mut m = InputMap::empty();
            m.bind(ActionSignal::Confirm, InputBinding::Key(Key::W));
            m
        };
        let mut cb = ContextualBindings::new(world).with(InputContext::Menu, menu);
        let mut input = InputState::new();
        input.set_key(Key::W, true);

        // World active: W = MoveForward.
        assert_eq!(cb.active(), InputContext::World);
        assert!(cb.action_pressed(ActionSignal::MoveForward, &input));
        assert!(!cb.action_pressed(ActionSignal::Confirm, &input));

        // Push Menu: the same key now fires Confirm.
        cb.push(InputContext::Menu);
        assert!(cb.action_pressed(ActionSignal::Confirm, &input));
        assert!(!cb.action_pressed(ActionSignal::MoveForward, &input));

        // Pop back to World; the base can never be popped away.
        assert_eq!(cb.pop(), Some(InputContext::Menu));
        assert!(cb.action_pressed(ActionSignal::MoveForward, &input));
        assert_eq!(cb.pop(), None);
    }

    #[test]
    fn context_without_its_own_map_falls_back_to_world() {
        // Radial has no map registered → it should read the World map rather than go dead.
        let mut cb = ContextualBindings::new(InputMap::xbox_souls());
        cb.push(InputContext::Radial);
        assert_eq!(cb.active(), InputContext::Radial);
        assert_eq!(
            cb.active_map().action_for(InputBinding::GamepadButton(GamepadButton::East)),
            Some(ActionSignal::Dodge),
        );
    }

    #[test]
    fn signal_held_reads_active_context() {
        let cfg = GamepadConfig::default();
        let mut cb = ContextualBindings::new({
            let mut m = InputMap::empty();
            m.bind(ActionSignal::MoveForward, InputBinding::Key(Key::W));
            m
        });
        let mut input = InputState::new();
        assert!(!cb.signal_held(ActionSignal::MoveForward, &input, &cfg));
        input.set_key(Key::W, true);
        assert!(cb.signal_held(ActionSignal::MoveForward, &input, &cfg));
        // Push a context with no binding for MoveForward but falling back to World keeps it live.
        cb.push(InputContext::Menu);
        assert!(cb.signal_held(ActionSignal::MoveForward, &input, &cfg));
    }

    // ── Name registry (spec §7.1a / RT-4) ──

    #[test]
    fn builtin_context_names_round_trip() {
        // Every built-in resolves name → context → name, and the count matches.
        for (name, ctx) in InputContext::BUILTIN_NAMES {
            assert_eq!(InputContext::from_name(name), Some(*ctx), "{name} resolves");
            assert_eq!(ctx.name(), Some(*name), "{name} reverses");
        }
        assert_eq!(InputContext::from_name("World"), Some(InputContext::World));
        assert_eq!(InputContext::from_name("TextEntry"), Some(InputContext::TextEntry));
        // Unknown / consumer-registered names have no frozen entry.
        assert_eq!(InputContext::from_name("Nonsense"), None);
        assert_eq!(InputContext::register(64).name(), None);
    }

    // ── InputProfile serde round-trip (mirrors the InputMapData round-trip, §7.1a) ──

    #[test]
    fn input_profile_round_trips_by_name() {
        // The verifiable win: a profile serializes its contexts by STABLE NAME (never the
        // runtime u16), so rebinds survive a save→load. This mirrors
        // `binding::tests::input_map_round_trips_through_json` at the profile level.
        let mut profile = InputProfile::default_profile();
        // Rebind MoveForward W → Up in the World map (a "rebind" the shell would persist).
        let world = {
            let mut m = profile.context_map("World").cloned().unwrap();
            m.bind(ActionSignal::MoveForward, InputBinding::Key(Key::Up));
            m
        };
        profile.set_context_map("World", world);

        let json = serde_json::to_string(&profile).expect("InputProfile serializes");
        // The JSON carries the frozen context NAME, not the u16 id (RT-4).
        assert!(json.contains("\"World\""), "contexts keyed by name in JSON: {json}");
        assert!(json.contains("\"TextEntry\""), "TextEntry present by name");
        let back: InputProfile = serde_json::from_str(&json).expect("InputProfile deserializes");

        assert_eq!(back.schema, InputProfile::SCHEMA);
        assert_eq!(back.name, "default");
        // The rebind survived, resolved through the name.
        assert_eq!(
            back.context_map("World").unwrap().action_for(InputBinding::Key(Key::Up)),
            Some(ActionSignal::MoveForward),
        );
        // The Menu context's non-default event kind (Release) round-trips too.
        let menu = back.contexts.iter().find(|(n, _)| n == "Menu").unwrap();
        assert_eq!(menu.1.default_event, EventKind::Release);
    }

    #[test]
    fn profile_names_and_signal_binding_round_trip() {
        // Each named preset constructs, names itself, and re-serializes; and a
        // SignalBinding (control + EventKind + signal) survives JSON — the reshape row.
        for (id, _label) in InputProfile::PRESET_NAMES {
            let p = InputProfile::by_name(id).unwrap_or_else(|| panic!("{id} builds"));
            assert_eq!(&p.name, id);
            let json = serde_json::to_string(&p).expect("serializes");
            let back: InputProfile = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(&back.name, id);
            assert!(back.context_map("World").is_some(), "{id} has a World map");
        }
        let sb = SignalBinding {
            control: InputBinding::Key(Key::Space),
            event: EventKind::Press,
            signal: ActionSignal::Jump,
        };
        let back: SignalBinding =
            serde_json::from_str(&serde_json::to_string(&sb).unwrap()).unwrap();
        assert_eq!(back, sb);
    }

    // ── from_profile / to_profile (the runtime ↔ persisted bridge, §7.1a) ──

    #[test]
    fn contextual_bindings_from_and_to_profile() {
        // from_profile resolves names → contexts and resets the stack to [World]; to_profile
        // snapshots the maps back by name. The maps must survive the round-trip.
        let profile = InputProfile::default_profile();
        let cb = ContextualBindings::from_profile(&profile);
        assert_eq!(cb.active(), InputContext::World, "stack reset to base");
        // World map came across (W = MoveForward).
        assert_eq!(
            cb.active_map().action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward),
        );
        // The named non-World context (Menu) installed under the right id.
        let mut cb2 = cb.clone();
        cb2.push(InputContext::Menu);
        assert_eq!(
            cb2.active_map().action_for(InputBinding::Key(Key::Up)),
            Some(ActionSignal::NavUp),
            "Menu map resolved via name → InputContext",
        );

        // Round-trip back to a profile, then serde: the World rebind survives.
        let profile2 = cb.to_profile("default", AbstractControls::default(), GamepadConfig::default());
        let json = serde_json::to_string(&profile2).unwrap();
        let back: InputProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.context_map("World").unwrap().action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward),
        );
    }

    #[test]
    fn contextual_bindings_serde_skips_stack_and_keys_by_name() {
        // The ContextualBindings serde companion: maps persist by name, the runtime
        // stack is dropped and rebuilt to [World] on load.
        let mut cb = ContextualBindings::new(InputMap::wasd_and_mouse())
            .with(InputContext::Menu, menu_map());
        cb.push(InputContext::Menu); // a non-base stack state that must NOT persist
        assert_eq!(cb.active(), InputContext::Menu);

        let json = serde_json::to_string(&cb).expect("ContextualBindings serializes");
        assert!(json.contains("\"World\"") && json.contains("\"Menu\""), "keyed by name");
        let back: ContextualBindings = serde_json::from_str(&json).expect("deserializes");
        // Stack was skipped → rebuilt to [World].
        assert_eq!(back.active(), InputContext::World, "stack rebuilt to base on load");
        // Maps survived, resolved by name.
        assert_eq!(
            back.active_map().action_for(InputBinding::Key(Key::W)),
            Some(ActionSignal::MoveForward),
        );
    }
}
