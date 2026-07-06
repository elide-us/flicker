//! Persisted application settings (`settings.json`) and the data-driven control
//! defs the settings workbench renders.
//!
//! Each control's value lives under the flat key `<page>_<id>` (sliders store the
//! value, toggles `0/1`, choices the option index). The same `(page, id, kind)`
//! list drives the Rust model/harvest *and* mirrors `ui_elements.json`'s
//! `settings.pages` (labels/ranges/options), the way `PARAM_DEFS` mirrors the
//! epoch panel. Keybindings (the engine `InputMap` + `RebindCapture`) are a later
//! slice — the Key Mappings page is a placeholder for now.

use std::collections::BTreeMap;
use std::sync::Mutex;

use flicker::app::{Action, InputMap};
use serde::{Deserialize, Serialize};

/// A control kind on a settings page.
pub enum Ctl {
    Slider { default: f64 },
    Toggle { default: bool },
    Choice { default: usize },
}

/// One setting: its page, id, and control kind. Model key is `<page>_<id>`.
pub struct Setting {
    pub page: &'static str,
    pub id: &'static str,
    pub ctl: Ctl,
}

/// The settings the workbench exposes. Ranges/labels/options live in
/// `ui_elements.json`; this is the source of truth for defaults + persistence.
pub const SETTINGS: &[Setting] = &[
    // Camera / input (sensitivities, inversion, deadzones, vibration prefs).
    Setting { page: "camera", id: "invert_y", ctl: Ctl::Toggle { default: false } },
    Setting { page: "camera", id: "invert_x", ctl: Ctl::Toggle { default: false } },
    Setting { page: "camera", id: "mouse_sensitivity", ctl: Ctl::Slider { default: 0.005 } },
    Setting { page: "camera", id: "stick_sensitivity", ctl: Ctl::Slider { default: 2.0 } },
    Setting { page: "camera", id: "left_deadzone", ctl: Ctl::Slider { default: 0.15 } },
    Setting { page: "camera", id: "right_deadzone", ctl: Ctl::Slider { default: 0.15 } },
    Setting { page: "camera", id: "trigger_threshold", ctl: Ctl::Slider { default: 0.5 } },
    Setting { page: "camera", id: "deadzone_shape", ctl: Ctl::Choice { default: 0 } },
    Setting { page: "camera", id: "vibration", ctl: Ctl::Toggle { default: true } },
    Setting { page: "camera", id: "vibration_intensity", ctl: Ctl::Slider { default: 0.6 } },
    // Audio (persisted prefs; no audio backend yet).
    Setting { page: "audio", id: "master", ctl: Ctl::Slider { default: 0.8 } },
    Setting { page: "audio", id: "music", ctl: Ctl::Slider { default: 0.6 } },
    Setting { page: "audio", id: "sfx", ctl: Ctl::Slider { default: 0.7 } },
    Setting { page: "audio", id: "voice", ctl: Ctl::Slider { default: 0.9 } },
    // Video.
    Setting { page: "video", id: "display_mode", ctl: Ctl::Choice { default: 0 } },
    Setting { page: "video", id: "resolution", ctl: Ctl::Choice { default: 1 } },
    Setting { page: "video", id: "quality", ctl: Ctl::Choice { default: 2 } },
    Setting { page: "video", id: "fps_limit", ctl: Ctl::Choice { default: 1 } },
    Setting { page: "video", id: "vsync", ctl: Ctl::Toggle { default: true } },
];

/// The flat model/persistence key for a control.
pub fn key(page: &str, id: &str) -> String {
    format!("{page}_{id}")
}

/// The actions shown (and rebindable) on the Key Mappings page: `(id, label)`.
/// Ids are `Action` variant names and mirror `ui_elements.json`
/// `settings.pages.keymap.actions`. (These are the engine's generic game
/// actions; flicker-world's viewer keys aren't routed through them yet.)
pub const KEYMAP_ACTIONS: &[(&str, &str)] = &[
    ("MoveForward", "Move Forward"),
    ("MoveBackward", "Move Backward"),
    ("StrafeLeft", "Strafe Left"),
    ("StrafeRight", "Strafe Right"),
    ("MoveUp", "Move Up"),
    ("MoveDown", "Move Down"),
    ("Jump", "Jump"),
    ("Sprint", "Sprint"),
    ("Interact", "Interact"),
    ("Menu", "Menu"),
];

/// Parse an [`Action`] from its variant id (see [`KEYMAP_ACTIONS`]).
pub fn parse_action(id: &str) -> Option<Action> {
    Some(match id {
        "MoveForward" => Action::MoveForward,
        "MoveBackward" => Action::MoveBackward,
        "StrafeLeft" => Action::StrafeLeft,
        "StrafeRight" => Action::StrafeRight,
        "MoveUp" => Action::MoveUp,
        "MoveDown" => Action::MoveDown,
        "Jump" => Action::Jump,
        "Sprint" => Action::Sprint,
        "Interact" => Action::Interact,
        "Menu" => Action::Menu,
        _ => return None,
    })
}

/// The primary binding for an action, formatted ("—" if unbound).
pub fn binding_label(map: &InputMap, action: Action) -> String {
    map.bindings_for(action)
        .first()
        .map(|b| format!("{b}"))
        .unwrap_or_else(|| "—".to_string())
}

fn default_input_map() -> InputMap {
    InputMap::wasd_and_mouse()
}

/// All persisted settings — a flat value map (sliders=value, toggles=0/1,
/// choices=index) plus the keybinding map. Serialised to `settings.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameSettings {
    #[serde(default)]
    values: BTreeMap<String, f64>,
    /// Action → physical input bindings (rebound on the Key Mappings page).
    /// **Not persisted to disk yet:** the engine `InputMap` can't serialise to
    /// JSON (its reverse map keys on `InputBinding`, which isn't a string key),
    /// so it's `skip`ped and defaults on load. Rebinds still persist across the
    /// session via the `GAME_SETTINGS` global. Disk persistence needs a
    /// JSON-friendly binding form (a follow-up).
    #[serde(skip, default = "default_input_map")]
    pub input_map: InputMap,
}

impl Default for GameSettings {
    fn default() -> Self {
        let mut values = BTreeMap::new();
        for s in SETTINGS {
            let v = match s.ctl {
                Ctl::Slider { default } => default,
                Ctl::Toggle { default } => f64::from(default),
                Ctl::Choice { default } => default as f64,
            };
            values.insert(key(s.page, s.id), v);
        }
        Self {
            values,
            input_map: default_input_map(),
        }
    }
}

impl GameSettings {
    /// The numeric value for a key (default 0 if absent).
    pub fn num(&self, k: &str) -> f64 {
        self.values.get(k).copied().unwrap_or(0.0)
    }

    /// The boolean value for a key (non-zero = true).
    pub fn flag(&self, k: &str) -> bool {
        self.num(k) != 0.0
    }

    pub fn set(&mut self, k: &str, v: f64) {
        self.values.insert(k.to_string(), v);
    }

    fn path() -> std::path::PathBuf {
        std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/settings.json"))
    }

    /// Load from `settings.json`, falling back to defaults (missing keys are
    /// backfilled so new settings always have a value).
    pub fn load() -> Self {
        let mut s = match std::fs::read(Self::path()) {
            Ok(bytes) => serde_json::from_slice::<GameSettings>(&bytes).unwrap_or_else(|e| {
                tracing::warn!("ignoring invalid settings.json: {e}");
                GameSettings::default()
            }),
            Err(_) => GameSettings::default(),
        };
        for d in GameSettings::default().values {
            s.values.entry(d.0).or_insert(d.1);
        }
        s
    }

    /// Persist to `settings.json` (best-effort).
    pub fn save(&self) {
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(Self::path(), bytes) {
                    tracing::warn!("failed to write settings.json: {e}");
                }
            }
            Err(e) => tracing::warn!("failed to serialize settings: {e}"),
        }
    }
}

/// Process-wide live settings (loaded at startup, written on Back).
pub static GAME_SETTINGS: Mutex<Option<GameSettings>> = Mutex::new(None);

/// Load `settings.json` into the global at startup.
pub fn load_global() {
    *GAME_SETTINGS.lock().expect("settings lock") = Some(GameSettings::load());
}

/// A copy of the current settings (defaults if not yet loaded).
pub fn snapshot() -> GameSettings {
    GAME_SETTINGS
        .lock()
        .expect("settings lock")
        .clone()
        .unwrap_or_default()
}

/// Store + persist edited settings.
pub fn store(settings: GameSettings) {
    settings.save();
    *GAME_SETTINGS.lock().expect("settings lock") = Some(settings);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_setting_and_round_trip() {
        let mut s = GameSettings::default();
        for def in SETTINGS {
            let k = key(def.page, def.id);
            assert!(s.values.contains_key(&k), "missing default for {k}");
        }
        // A round-trip through JSON preserves an edited value.
        s.set("audio_master", 0.25);
        let json = serde_json::to_vec(&s).unwrap();
        let back: GameSettings = serde_json::from_slice(&json).unwrap();
        assert_eq!(back.num("audio_master"), 0.25);
        assert!(back.flag("video_vsync"), "vsync default true survives");
        // Keybindings aren't JSON-persisted yet (InputMap has non-string keys);
        // the field is skipped and defaults on load — which still has bindings.
        assert!(
            !back.input_map.bindings_for(Action::MoveForward).is_empty(),
            "the default input map has bindings"
        );
    }

    #[test]
    fn keymap_action_ids_parse() {
        for (id, _) in KEYMAP_ACTIONS {
            assert!(parse_action(id).is_some(), "unparsed keymap action id: {id}");
        }
    }
}
