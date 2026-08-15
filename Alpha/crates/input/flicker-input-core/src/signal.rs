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
//! resolved abilities (the concrete parry a rapier's `Defend` becomes, a
//! slotted item's effect) — those are a `flicker-mechanics` concern, resolved
//! downstream, server-authoritative. `Kick` / `CounterPerilous` (Aaron
//! 2026-08-14) sit on the intent side of that line: the chord expresses the
//! player's INTENT; what the kick or counter resolves to stays loadout data.
//! Parry stays folded into `Defend` (re-affirmed same day — variance is DATA,
//! `C60AE43C §4/5`).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Semantic input action — the WHAT. Consumer code reacts to these; the
/// physical-input→signal mapping lives in [`InputMap`](crate::InputMap).
///
/// Extend with new intents as needed (append only). Every variant must be
/// listed in [`ActionSignal::ALL`] and handled by the exhaustive matches in
/// [`label`](Self::label), [`Display`], [`group`](Self::group) and
/// [`rebind_scope`](Self::rebind_scope) — a new variant is a compile error
/// until it is named, labelled AND classified, which keeps the count, the
/// coverage and the DERIVED settings surface honest with no bare literal
/// (`C60AE43C §2`: a new signal shows up in the remap surface automatically).
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
    /// Guard-break kick / posture poke (pad default: the chord layer, hold
    /// North + LT — ruled OFF `Special`, Aaron's Elden Ring complaint; the
    /// keyboard binding is the player's).
    Kick,
    /// Counter a PERILOUS attack — the mikiri (pad default: the chord layer,
    /// hold North + LB and LT pressed together; keyboard binding is the
    /// player's). The intent only: what the counter resolves to stays a
    /// loadout/state-machine concern.
    CounterPerilous,

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
    /// Advance to the next top-level PAGE (RT / `]`).
    ///
    /// A rung ABOVE `TabNext`: a controller-first menu has two rails, and they are
    /// different scales of movement — the triggers cycle the whole page, the bumpers
    /// cycle a section within it. Keeping them distinct is what lets a surface show
    /// both rails at once (the `paged_menu` template) without one shadowing the other.
    PageNext,
    /// Return to the previous top-level page (LT / `[`).
    PagePrev,

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
    /// Draw the focused viewport's camera closer (left stick up on a bench —
    /// the same stick whose X axis walks the panels; the wheel is the pointer's
    /// zoom and stays outside the signal map).
    ZoomIn,
    /// Back the focused viewport's camera away (left stick down).
    ZoomOut,

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
        ActionSignal::Kick,
        ActionSignal::CounterPerilous,
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
        ActionSignal::PageNext,
        ActionSignal::PagePrev,
        ActionSignal::PanelNext,
        ActionSignal::PanelPrev,
        ActionSignal::ModeNext,
        ActionSignal::ModePrev,
        ActionSignal::ZoomIn,
        ActionSignal::ZoomOut,
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
            Self::Kick => "Kick",
            Self::CounterPerilous => "CounterPerilous",
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
            Self::PageNext => "PageNext",
            Self::PagePrev => "PagePrev",
            Self::PanelNext => "PanelNext",
            Self::PanelPrev => "PanelPrev",
            Self::ModeNext => "ModeNext",
            Self::ModePrev => "ModePrev",
            Self::ZoomIn => "ZoomIn",
            Self::ZoomOut => "ZoomOut",
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
            Self::Kick => "Kick",
            Self::CounterPerilous => "Mikiri",
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
            Self::PageNext => "Page \u{2192}",
            Self::PagePrev => "\u{2190} Page",
            Self::PanelNext => "Panel \u{2192}",
            Self::PanelPrev => "\u{2190} Panel",
            Self::ModeNext => "Mode \u{2192}",
            Self::ModePrev => "\u{2190} Mode",
            Self::ZoomIn => "Zoom In",
            Self::ZoomOut => "Zoom Out",
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

    /// The catalog GROUP this signal belongs to — the enum's own section
    /// comments made queryable, so a derived surface (the settings key-bindings
    /// page heads its rows with [`SignalGroup::token`]) never keeps a second
    /// hand-maintained partition. Exhaustive — no `_` arm — a new variant will
    /// not compile until classified.
    pub fn group(self) -> SignalGroup {
        match self {
            Self::MoveForward
            | Self::MoveBackward
            | Self::StrafeLeft
            | Self::StrafeRight
            | Self::MoveUp
            | Self::MoveDown => SignalGroup::Movement,
            Self::LookUp | Self::LookDown | Self::LookLeft | Self::LookRight => {
                SignalGroup::Camera
            }
            Self::PrimaryAction
            | Self::SecondaryAction
            | Self::Jump
            | Self::Sprint
            | Self::Crouch
            | Self::Interact
            | Self::Reload => SignalGroup::Combat,
            Self::AttackLight
            | Self::AttackHeavy
            | Self::Defend
            | Self::Special
            | Self::Dodge
            | Self::LockOn
            | Self::UseItem
            | Self::Kick
            | Self::CounterPerilous => SignalGroup::Souls,
            Self::Confirm | Self::Cancel | Self::Menu | Self::Inventory | Self::Map => {
                SignalGroup::Ui
            }
            Self::Quit => SignalGroup::System,
            Self::ChordBegin
            | Self::Activate
            | Self::ItemSelect
            | Self::NavUp
            | Self::NavDown
            | Self::NavLeft
            | Self::NavRight
            | Self::TabNext
            | Self::TabPrev
            | Self::PageNext
            | Self::PagePrev => SignalGroup::Nav,
            Self::PanelNext
            | Self::PanelPrev
            | Self::ModeNext
            | Self::ModePrev
            | Self::ZoomIn
            | Self::ZoomOut => SignalGroup::EditorNav,
            Self::Undo
            | Self::Redo
            | Self::Cut
            | Self::Paste
            | Self::Rename
            | Self::CreateFolder
            | Self::ContextMenu
            | Self::Yes
            | Self::No => SignalGroup::EditorVerbs,
            Self::SubmitText | Self::CancelText => SignalGroup::Text,
        }
    }

    /// Where this signal sits on the remap/settings surface (Aaron's rulings,
    /// 2026-08-14: the Souls tier is player-rebindable; the analog Look channel
    /// and every system-owned default stay off the page). Exhaustive — no `_`
    /// arm — a new variant will not compile until scoped, which is what makes
    /// the settings page's row set a DERIVATION instead of a parallel list.
    pub fn rebind_scope(self) -> RebindScope {
        match self {
            // Player: movement + combat/interaction + souls intents + UI verbs + Quit.
            Self::MoveForward
            | Self::MoveBackward
            | Self::StrafeLeft
            | Self::StrafeRight
            | Self::MoveUp
            | Self::MoveDown
            | Self::PrimaryAction
            | Self::SecondaryAction
            | Self::Jump
            | Self::Sprint
            | Self::Crouch
            | Self::Interact
            | Self::Reload
            | Self::AttackLight
            | Self::AttackHeavy
            | Self::Defend
            | Self::Special
            | Self::Dodge
            | Self::LockOn
            | Self::UseItem
            | Self::Kick
            | Self::CounterPerilous
            | Self::Confirm
            | Self::Cancel
            | Self::Menu
            | Self::Inventory
            | Self::Map
            | Self::Quit => RebindScope::Player,
            // Locked: the analog/pointer Look channel, the walker's nav family,
            // the tab/page rails, bench panel/zoom nav, the chord layer's editor
            // verbs and the text-terminal pair — system-owned defaults.
            Self::LookUp
            | Self::LookDown
            | Self::LookLeft
            | Self::LookRight
            | Self::ChordBegin
            | Self::NavUp
            | Self::NavDown
            | Self::NavLeft
            | Self::NavRight
            | Self::TabNext
            | Self::TabPrev
            | Self::PageNext
            | Self::PagePrev
            | Self::PanelNext
            | Self::PanelPrev
            | Self::ZoomIn
            | Self::ZoomOut
            | Self::Undo
            | Self::Redo
            | Self::Cut
            | Self::Paste
            | Self::Rename
            | Self::CreateFolder
            | Self::ContextMenu
            | Self::SubmitText
            | Self::CancelText => RebindScope::Locked,
            // Reserved: declared with zero engine bindings — hidden from
            // surfaces until wired.
            Self::Activate
            | Self::ItemSelect
            | Self::ModeNext
            | Self::ModePrev
            | Self::Yes
            | Self::No => RebindScope::Reserved,
        }
    }

    /// The stringtable token for this signal's player-facing display name —
    /// `"$sig_" + snake(name())`, in the `$`-sigil form authored straight into
    /// a node's `text` prop (the `PRESET_NAMES` precedent; the widgets
    /// stringtable resolves it at draw). ONE mechanical fold of the ONE
    /// vocabulary ([`name`](Self::name)) — never a second naming. The
    /// stringtable's `sig_*` STEMS and the Model's transient `sig_<result>`
    /// intent mirror are different namespaces (stringtable keys vs ValueMap
    /// keys).
    pub fn token(self) -> String {
        format!("$sig_{}", snake(self.name()))
    }

    /// The settings key-bindings page's row source: every
    /// [`RebindScope::Player`] signal, in declaration order. The page's rows,
    /// its model publishes and its rebind dispatch all iterate THIS — there is
    /// no parallel list to drift.
    pub fn rebindable() -> impl Iterator<Item = ActionSignal> {
        Self::ALL
            .iter()
            .copied()
            .filter(|s| s.rebind_scope() == RebindScope::Player)
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
            Self::Kick => "Kick",
            Self::CounterPerilous => "Perilous Counter",
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
            Self::PageNext => "Next Page",
            Self::PagePrev => "Previous Page",
            Self::PanelNext => "Next Panel",
            Self::PanelPrev => "Previous Panel",
            Self::ModeNext => "Next View Mode",
            Self::ModePrev => "Previous View Mode",
            Self::ZoomIn => "Zoom In",
            Self::ZoomOut => "Zoom Out",
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

/// One Pascal→snake fold for stringtable token stems (`"MoveForward"` →
/// `"move_forward"`, `"F10"` → `"f10"`, `"DPadUp"` → `"d_pad_up"`): an
/// underscore before each interior uppercase, everything lowercased. The
/// inverse of the widgets-side `snake_to_pascal` (intents.rs) — each crate
/// folds its own direction of the same convention; neither is a second
/// vocabulary.
pub(crate) fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The catalog grouping of the signal vocabulary — [`ActionSignal`]'s section
/// comments made queryable via [`ActionSignal::group`]. Membership lives ONLY
/// in that exhaustive match; this enum is the section list a derived surface
/// iterates (heading each section with [`token`](Self::token)), so the
/// partition can never fork into a second hand-maintained list.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SignalGroup {
    /// Digital movement intents.
    Movement,
    /// The analog/pointer Look channel.
    Camera,
    /// Combat / interaction basics.
    Combat,
    /// Souls combat intents (loadout-resolved).
    Souls,
    /// Menu / UI verbs.
    Ui,
    /// Engine-level intents.
    System,
    /// Walker nav family, the tab/page rails, chord + dialog plumbing.
    Nav,
    /// Bench editor navigation (panels, view modes, viewport zoom).
    EditorNav,
    /// Chord-layer editor verbs + dialog terminals.
    EditorVerbs,
    /// The text-terminal pair.
    Text,
}

impl SignalGroup {
    /// Every group, in [`ActionSignal`] declaration order (the order a derived
    /// page walks its sections).
    pub const ALL: &'static [SignalGroup] = &[
        SignalGroup::Movement,
        SignalGroup::Camera,
        SignalGroup::Combat,
        SignalGroup::Souls,
        SignalGroup::Ui,
        SignalGroup::System,
        SignalGroup::Nav,
        SignalGroup::EditorNav,
        SignalGroup::EditorVerbs,
        SignalGroup::Text,
    ];

    /// The stringtable token for the group's section header, in the `$`-sigil
    /// form like [`ActionSignal::token`]. Exhaustive — no `_` arm.
    pub fn token(self) -> &'static str {
        match self {
            Self::Movement => "$siggroup_movement",
            Self::Camera => "$siggroup_camera",
            Self::Combat => "$siggroup_combat",
            Self::Souls => "$siggroup_souls",
            Self::Ui => "$siggroup_ui",
            Self::System => "$siggroup_system",
            Self::Nav => "$siggroup_nav",
            Self::EditorNav => "$siggroup_editor_nav",
            Self::EditorVerbs => "$siggroup_editor_verbs",
            Self::Text => "$siggroup_text",
        }
    }
}

/// Where a signal sits on the remap/settings surface
/// ([`ActionSignal::rebind_scope`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RebindScope {
    /// Listed AND rebindable on the settings key-bindings page.
    Player,
    /// System-owned default bindings (walker nav, rails, bench nav, chord
    /// verbs, text terminals) — catalogued, not player-editable.
    Locked,
    /// Declared with zero engine bindings — hidden from surfaces until wired.
    Reserved,
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

    /// S1 catalog: the group partition covers both directions — every signal's
    /// group is listed in `SignalGroup::ALL`, and every listed group has at
    /// least one member (no orphan section on a derived page).
    #[test]
    fn every_signal_is_grouped_and_every_group_has_members() {
        let mut member_counts = std::collections::HashMap::new();
        for &s in ActionSignal::ALL {
            assert!(
                SignalGroup::ALL.contains(&s.group()),
                "{s:?} groups outside SignalGroup::ALL"
            );
            *member_counts.entry(s.group()).or_insert(0usize) += 1;
        }
        for &g in SignalGroup::ALL {
            assert!(
                member_counts.get(&g).copied().unwrap_or(0) > 0,
                "{g:?} has no members"
            );
        }
    }

    /// The settings page's row source, pinned: 28 Player signals (Aaron's
    /// 2026-08-14 rulings fold the Souls tier onto the page and add
    /// `Kick` / `CounterPerilous` as chord-reached souls INTENTS), the analog
    /// Look channel and the walker-owned nav family stay off it, and the whole
    /// vocabulary is classified (Player + Locked + Reserved == ALL).
    #[test]
    fn rebindable_set_is_the_ruled_28() {
        let rows: Vec<ActionSignal> = ActionSignal::rebindable().collect();
        assert_eq!(rows.len(), 28);
        for s in [
            ActionSignal::AttackLight,
            ActionSignal::Dodge,
            ActionSignal::UseItem,
            ActionSignal::Kick,
            ActionSignal::CounterPerilous,
        ] {
            assert!(rows.contains(&s), "souls tier is Player-rebindable: {s:?}");
        }
        for s in [
            ActionSignal::LookUp,
            ActionSignal::NavUp,
            ActionSignal::Undo,
            ActionSignal::Activate,
        ] {
            assert!(!rows.contains(&s), "{s:?} must stay off the page");
        }
        let (mut player, mut locked, mut reserved) = (0usize, 0usize, 0usize);
        for &s in ActionSignal::ALL {
            match s.rebind_scope() {
                RebindScope::Player => player += 1,
                RebindScope::Locked => locked += 1,
                RebindScope::Reserved => reserved += 1,
            }
        }
        assert_eq!((player, locked, reserved), (28, 26, 6));
        assert_eq!(player + locked + reserved, ActionSignal::ALL.len());
    }

    /// Token derivation is ONE mechanical fold of the ONE vocabulary: every
    /// signal token is unique, `$sig_`-prefixed and exactly
    /// `"$sig_" + snake(name())`; group tokens are unique and
    /// `$siggroup_`-prefixed. Spot-pins for the fold itself.
    #[test]
    fn tokens_are_the_snake_fold_of_the_one_vocabulary() {
        let mut seen = HashSet::new();
        for &s in ActionSignal::ALL {
            let t = s.token();
            assert!(t.starts_with("$sig_"), "{s:?} token {t:?} lacks $sig_");
            assert_eq!(t, format!("$sig_{}", snake(s.name())));
            assert!(seen.insert(t), "duplicate token for {s:?}");
        }
        assert_eq!(seen.len(), ActionSignal::ALL.len());
        let mut groups = HashSet::new();
        for &g in SignalGroup::ALL {
            assert!(g.token().starts_with("$siggroup_"), "{g:?}");
            assert!(groups.insert(g.token()), "duplicate group token for {g:?}");
        }
        assert_eq!(snake("MoveForward"), "move_forward");
        assert_eq!(snake("CreateFolder"), "create_folder");
        assert_eq!(snake("Quit"), "quit");
    }
}
