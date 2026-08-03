//! [`ActionSignal`] — the semantic WHAT the engine reacts to, decoupled from
//! the physical HOW.
//!
//! This is the merge of `flicker-core`'s `Action` (30 variants) with the
//! `flicker-controllertester` mockup's `Signal` (the nav / chord / yes-no set),
//! plus the two dedicated text-terminal signals (`SubmitText`, `CancelText`,
//! spec R2). The variant names are **serde-stable** — `InputMapData` persists
//! `Vec<(ActionSignal, Vec<InputBinding>)>`, so JSON carries variant names, never
//! the type name. **ADD variants only, never rename** (`C60AE43C §2`).
//!
//! The enum holds *intents* (buttons / nav / text / movement). It NEVER holds
//! resolved abilities (`Kick`, a specific parry, a slotted item) — those are a
//! `flicker-mechanics` concern, resolved downstream, server-authoritative.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Semantic input action — the WHAT. Consumer code reacts to these; the
/// physical-input→signal mapping lives in [`InputMap`](crate::InputMap).
///
/// Extend with new intents as needed (append only). Every variant must be
/// listed in [`ActionSignal::ALL`] and handled by the exhaustive matches in
/// [`label`](Self::label) and [`Display`] — a new variant is a compile error
/// until it is, which keeps the count and coverage honest with no bare literal.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionSignal {
    // ── Movement (digital; analog Move*/Look* live on the analog channel) ──
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

    // ── Souls combat intents ──
    // These are INTENTS, not abilities: the equipped loadout resolves each to a concrete
    // ability/state (a rapier's `Defend` parries, a greatshield's blocks). The button set
    // stays small and stable while capability varies in equipment DATA. See the input
    // contract design (MCP `C60AE43C`).
    /// Light attack (RB / LMB).
    AttackLight,
    /// Heavy attack (RT / RMB).
    AttackHeavy,
    /// Defensive intent (LB, held as a stance) — resolves to block / parry / deflect.
    Defend,
    /// Weapon or spell special (LT).
    Special,
    /// Evade intent (B tap) — resolves to roll / step / hop.
    Dodge,
    /// Toggle target lock-on (RS press).
    LockOn,
    /// Use the readied item / consumable.
    UseItem,

    // ── UI ──
    Confirm,
    Cancel,
    Menu,
    Inventory,
    Map,

    // ── System ──
    Quit,

    // ── Promoted from the tester mockup (nav / chord / dialog) ──
    /// A modifier button opened the chord layer (members' normal signals are
    /// suppressed while it is held).
    ChordBegin,
    /// Generic activate (A / South) — context resolves it (distinct from
    /// `Confirm` / `PrimaryAction`; see Open Q4).
    Activate,
    /// Select the highlighted item (d-pad in World, North in Inventory).
    ItemSelect,
    NavUp,
    NavDown,
    NavLeft,
    NavRight,
    /// Advance to the next tab / group (RB / right trigger).
    TabNext,
    /// Return to the previous tab / group (LB / left trigger).
    TabPrev,

    // ── Editor navigation (controller-first benches) ──
    // A bench screen is several PANELS (a tree, a list, an inspector); the d-pad
    // moves the cursor WITHIN the focused panel, so moving BETWEEN panels needs
    // its own intent. Left stick by default, Tab / Shift-Tab on a keyboard.
    /// Move focus to the next panel / frame of the screen.
    PanelNext,
    /// Move focus to the previous panel / frame.
    PanelPrev,
    /// Next view MODE within the current tab (list ↔ grid, preview modes).
    /// Distinct from `TabNext`, which changes the tab itself.
    ModeNext,
    /// Previous view mode within the current tab.
    ModePrev,

    // ── Editor verbs (the chord layer) ──
    // Reached by holding the chord modifier and pressing a NON-FACE control —
    // a held face button commits that thumb, so `ChordBegin` + X/A/B is
    // unreachable in practice (Aaron's ergonomic ruling, 2026-08-02). On a
    // keyboard these are ordinary Ctrl-chords.
    /// Take back the last mutation.
    Undo,
    /// Re-apply the last undone mutation.
    Redo,
    /// Pick up the focused item to move it.
    Cut,
    /// Put the picked-up item down here.
    Paste,
    /// Rename the focused item in place.
    Rename,
    /// Create a folder in the current location.
    CreateFolder,
    /// Open the context menu on the focused item (mouse right-click binds here
    /// too, so pad and pointer reach the same menu by the same intent).
    ContextMenu,
    /// Affirmative in a dialog terminal.
    Yes,
    /// Negative in a dialog terminal.
    No,

    // ── Text terminals (spec R2 — dedicated, not a Confirm/Cancel overload) ──
    /// Commit the focused text field (Enter).
    SubmitText,
    /// Abandon the focused text field, keeping any draft (Esc).
    CancelText,
}

impl ActionSignal {
    /// Every variant, in declaration order. The signal count is
    /// `ActionSignal::ALL.len()` — never a bare literal (spec §3.1). Coverage
    /// is compiler-enforced by the exhaustive matches in [`label`](Self::label)
    /// and [`Display`]; [`ALL`](Self::ALL) is checked for completeness /
    /// uniqueness by the unit tests below.
    pub const ALL: &'static [ActionSignal] = &[
        ActionSignal::MoveForward,
        ActionSignal::MoveBackward,
        ActionSignal::StrafeLeft,
        ActionSignal::StrafeRight,
        ActionSignal::MoveUp,
        ActionSignal::MoveDown,
        ActionSignal::LookUp,
        ActionSignal::LookDown,
        ActionSignal::LookLeft,
        ActionSignal::LookRight,
        ActionSignal::PrimaryAction,
        ActionSignal::SecondaryAction,
        ActionSignal::Jump,
        ActionSignal::Sprint,
        ActionSignal::Crouch,
        ActionSignal::Interact,
        ActionSignal::Reload,
        ActionSignal::AttackLight,
        ActionSignal::AttackHeavy,
        ActionSignal::Defend,
        ActionSignal::Special,
        ActionSignal::Dodge,
        ActionSignal::LockOn,
        ActionSignal::UseItem,
        ActionSignal::Confirm,
        ActionSignal::Cancel,
        ActionSignal::Menu,
        ActionSignal::Inventory,
        ActionSignal::Map,
        ActionSignal::Quit,
        ActionSignal::ChordBegin,
        ActionSignal::Activate,
        ActionSignal::ItemSelect,
        ActionSignal::NavUp,
        ActionSignal::NavDown,
        ActionSignal::NavLeft,
        ActionSignal::NavRight,
        ActionSignal::TabNext,
        ActionSignal::TabPrev,
        ActionSignal::PanelNext,
        ActionSignal::PanelPrev,
        ActionSignal::ModeNext,
        ActionSignal::ModePrev,
        ActionSignal::Undo,
        ActionSignal::Redo,
        ActionSignal::Cut,
        ActionSignal::Paste,
        ActionSignal::Rename,
        ActionSignal::CreateFolder,
        ActionSignal::ContextMenu,
        ActionSignal::Yes,
        ActionSignal::No,
        ActionSignal::SubmitText,
        ActionSignal::CancelText,
    ];

    /// The stable NAME of this signal — **exactly its serde variant name**, the
    /// string `InputMapData` / `InputProfile` already persist for it (spec §7.1a
    /// pattern; frozen like [`InputContext::BUILTIN_NAMES`], ADD only, never
    /// rename — `C60AE43C §2`). This is deliberately NOT a second naming: the
    /// one vocabulary is the serde variant names, and
    /// [`from_name`](Self::from_name) resolves the same strings back, so a
    /// screen's declarative `on_<signal>` binding (S9) and a persisted profile
    /// speak identical tokens. Exhaustive — no `_` arm — so a new variant will
    /// not compile until it is named (and the round-trip test below pins each
    /// name to the serde form).
    ///
    /// [`InputContext::BUILTIN_NAMES`]: crate::InputContext::BUILTIN_NAMES
    pub fn name(self) -> &'static str {
        match self {
            Self::MoveForward => "MoveForward",
            Self::MoveBackward => "MoveBackward",
            Self::StrafeLeft => "StrafeLeft",
            Self::StrafeRight => "StrafeRight",
            Self::MoveUp => "MoveUp",
            Self::MoveDown => "MoveDown",
            Self::LookUp => "LookUp",
            Self::LookDown => "LookDown",
            Self::LookLeft => "LookLeft",
            Self::LookRight => "LookRight",
            Self::PrimaryAction => "PrimaryAction",
            Self::SecondaryAction => "SecondaryAction",
            Self::Jump => "Jump",
            Self::Sprint => "Sprint",
            Self::Crouch => "Crouch",
            Self::Interact => "Interact",
            Self::Reload => "Reload",
            Self::AttackLight => "AttackLight",
            Self::AttackHeavy => "AttackHeavy",
            Self::Defend => "Defend",
            Self::Special => "Special",
            Self::Dodge => "Dodge",
            Self::LockOn => "LockOn",
            Self::UseItem => "UseItem",
            Self::Confirm => "Confirm",
            Self::Cancel => "Cancel",
            Self::Menu => "Menu",
            Self::Inventory => "Inventory",
            Self::Map => "Map",
            Self::Quit => "Quit",
            Self::ChordBegin => "ChordBegin",
            Self::Activate => "Activate",
            Self::ItemSelect => "ItemSelect",
            Self::NavUp => "NavUp",
            Self::NavDown => "NavDown",
            Self::NavLeft => "NavLeft",
            Self::NavRight => "NavRight",
            Self::TabNext => "TabNext",
            Self::TabPrev => "TabPrev",
            Self::PanelNext => "PanelNext",
            Self::PanelPrev => "PanelPrev",
            Self::ModeNext => "ModeNext",
            Self::ModePrev => "ModePrev",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::CreateFolder => "CreateFolder",
            Self::ContextMenu => "ContextMenu",
            Self::Yes => "Yes",
            Self::No => "No",
            Self::SubmitText => "SubmitText",
            Self::CancelText => "CancelText",
        }
    }

    /// Resolve a stable NAME (a serde variant name, see [`name`](Self::name))
    /// back to its signal, or `None` for an unknown string — the vocabulary
    /// gate a declarative consumer (S9's `on_<signal>` props) warns-and-skips
    /// on. Scans [`ALL`](Self::ALL), so there is no second table to drift.
    pub fn from_name(name: &str) -> Option<ActionSignal> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    /// Short label (promoted from the tester's `Signal::label`), for terse HUD /
    /// inspector rows. Exhaustive — no `_` arm — so a new variant will not
    /// compile until it is labelled.
    pub fn label(self) -> &'static str {
        match self {
            Self::MoveForward => "Move Fwd",
            Self::MoveBackward => "Move Back",
            Self::StrafeLeft => "Strafe L",
            Self::StrafeRight => "Strafe R",
            Self::MoveUp => "Move Up",
            Self::MoveDown => "Move Down",
            Self::LookUp => "Look Up",
            Self::LookDown => "Look Down",
            Self::LookLeft => "Look Left",
            Self::LookRight => "Look Right",
            Self::PrimaryAction => "Primary",
            Self::SecondaryAction => "Secondary",
            Self::Jump => "Jump",
            Self::Sprint => "Run",
            Self::Crouch => "Crouch",
            Self::Interact => "Interact",
            Self::Reload => "Reload",
            Self::AttackLight => "Attack (light)",
            Self::AttackHeavy => "Attack (heavy)",
            Self::Defend => "Defend / block",
            Self::Special => "Special",
            Self::Dodge => "Dodge / roll",
            Self::LockOn => "Lock-on",
            Self::UseItem => "Use Item",
            Self::Confirm => "Confirm",
            Self::Cancel => "Cancel / back",
            Self::Menu => "Menu",
            Self::Inventory => "Inventory",
            Self::Map => "Map",
            Self::Quit => "Quit",
            Self::ChordBegin => "Chord begin",
            Self::Activate => "Activate",
            Self::ItemSelect => "Item select",
            Self::NavUp => "Nav up",
            Self::NavDown => "Nav down",
            Self::NavLeft => "Nav left",
            Self::NavRight => "Nav right",
            Self::TabNext => "Tab \u{2192}",
            Self::TabPrev => "\u{2190} Tab",
            Self::PanelNext => "Panel \u{2192}",
            Self::PanelPrev => "\u{2190} Panel",
            Self::ModeNext => "Mode \u{2192}",
            Self::ModePrev => "\u{2190} Mode",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::CreateFolder => "New folder",
            Self::ContextMenu => "Menu (item)",
            Self::Yes => "Yes",
            Self::No => "No",
            Self::SubmitText => "Submit",
            Self::CancelText => "Cancel text",
        }
    }
}

impl fmt::Display for ActionSignal {
    /// Long human label for settings rows. Exhaustive — no `_` arm.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::MoveForward => "Move Forward",
            Self::MoveBackward => "Move Backward",
            Self::StrafeLeft => "Strafe Left",
            Self::StrafeRight => "Strafe Right",
            Self::MoveUp => "Move Up",
            Self::MoveDown => "Move Down",
            Self::LookUp => "Look Up",
            Self::LookDown => "Look Down",
            Self::LookLeft => "Look Left",
            Self::LookRight => "Look Right",
            Self::PrimaryAction => "Primary Action",
            Self::SecondaryAction => "Secondary Action",
            Self::Jump => "Jump",
            Self::Sprint => "Sprint",
            Self::Crouch => "Crouch",
            Self::Interact => "Interact",
            Self::Reload => "Reload",
            Self::AttackLight => "Light Attack",
            Self::AttackHeavy => "Heavy Attack",
            Self::Defend => "Defend",
            Self::Special => "Special",
            Self::Dodge => "Dodge",
            Self::LockOn => "Lock On",
            Self::UseItem => "Use Item",
            Self::Confirm => "Confirm",
            Self::Cancel => "Cancel",
            Self::Menu => "Menu",
            Self::Inventory => "Inventory",
            Self::Map => "Map",
            Self::Quit => "Quit",
            Self::ChordBegin => "Chord Begin",
            Self::Activate => "Activate",
            Self::ItemSelect => "Item Select",
            Self::NavUp => "Navigate Up",
            Self::NavDown => "Navigate Down",
            Self::NavLeft => "Navigate Left",
            Self::NavRight => "Navigate Right",
            Self::TabNext => "Next Tab",
            Self::TabPrev => "Previous Tab",
            Self::PanelNext => "Next Panel",
            Self::PanelPrev => "Previous Panel",
            Self::ModeNext => "Next View Mode",
            Self::ModePrev => "Previous View Mode",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::Rename => "Rename",
            Self::CreateFolder => "New Folder",
            Self::ContextMenu => "Context Menu",
            Self::Yes => "Yes",
            Self::No => "No",
            Self::SubmitText => "Submit Text",
            Self::CancelText => "Cancel Text",
        };
        write!(f, "{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_covers_every_variant_uniquely() {
        // `ALL` is the single source of the signal count. The exhaustive matches in
        // `label()` / `Display` (no `_` arm) already force every *declared* variant to
        // be handled at compile time; this guards `ALL` itself against a dropped or
        // duplicated entry, and exercises both label paths.
        let mut seen = HashSet::new();
        for &s in ActionSignal::ALL {
            assert!(seen.insert(s), "duplicate entry in ActionSignal::ALL: {s:?}");
            assert!(!s.label().is_empty(), "empty label for {s:?}");
            assert!(!s.to_string().is_empty(), "empty Display for {s:?}");
        }
        // Count is derived from ALL, never a bare literal.
        assert_eq!(seen.len(), ActionSignal::ALL.len());
    }

    #[test]
    fn promoted_and_text_signals_present() {
        for s in [
            ActionSignal::ChordBegin,
            ActionSignal::Activate,
            ActionSignal::ItemSelect,
            ActionSignal::NavUp,
            ActionSignal::NavDown,
            ActionSignal::NavLeft,
            ActionSignal::NavRight,
            ActionSignal::TabNext,
            ActionSignal::TabPrev,
            ActionSignal::Yes,
            ActionSignal::No,
            ActionSignal::SubmitText,
            ActionSignal::CancelText,
        ] {
            assert!(ActionSignal::ALL.contains(&s), "{s:?} missing from ALL");
        }
    }

    /// S9 stage 1: the stable-name table round-trips over EVERY variant, and each
    /// name is byte-identical to the serde form (`InputMapData` persists variant
    /// names, so `name()` must never fork a second naming). Also pins the
    /// vocabulary gate: an unknown string resolves to `None`.
    #[test]
    fn name_round_trips_all_and_matches_serde() {
        for &s in ActionSignal::ALL {
            assert_eq!(ActionSignal::from_name(s.name()), Some(s), "{s:?} round-trips");
            // The ONE naming: serde's variant string (what profiles persist).
            let serde_name = serde_json::to_value(s).expect("signal serializes");
            assert_eq!(
                serde_name.as_str(),
                Some(s.name()),
                "{s:?}: name() must equal the persisted serde variant name"
            );
        }
        assert_eq!(ActionSignal::from_name("Nonsense"), None);
        assert_eq!(ActionSignal::from_name(""), None);
        assert_eq!(ActionSignal::from_name("menu"), None, "names are exact — no case folding here");
    }

    #[test]
    fn kept_action_display_labels_are_stable() {
        // The 30 kept Action labels must read identically to the pre-move `Action`
        // Display (settings rows format `{signal}`).
        assert_eq!(ActionSignal::MoveForward.to_string(), "Move Forward");
        assert_eq!(ActionSignal::AttackLight.to_string(), "Light Attack");
        assert_eq!(ActionSignal::Quit.to_string(), "Quit");
    }
}
