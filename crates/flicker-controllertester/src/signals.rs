//! The action-signal model the viewer displays: physical control × event-kind → SIGNAL,
//! resolved per input CONTEXT. This is a design mockup living in the tester so the mapping
//! and its case-by-case press/release semantics can be iterated visually; once settled it
//! promotes into the engine (flicker-core `Action` + the input bridge).
//!
//! Aaron's rules (2026-07-24): the engine cares WHAT is signalled, not HOW. Event-kind is
//! case-by-case — combat fires on PRESS (responsive), menus on RELEASE (so a mis-press can
//! be slid off / escaped), nav is press-with-repeat, and Y is the split case
//! (press→ChordBegin, release→Jump). Edges come from the standard last-frame snapshot.

use flicker::app::{GamepadButton as Gb, InputState, Key};

/// The semantic action a control tells the game to do — the WHAT.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    // ── world / combat ──
    AttackLight, AttackHeavy, Defend, Special, Dodge, Run, Jump, ChordBegin,
    Interact, Activate, LockOn, Crouch, ItemSelect,
    // ── menu / dialog ──
    Confirm, Cancel, TabNext, TabPrev, NavUp, NavDown, NavLeft, NavRight, Yes, No,
}

impl Signal {
    pub fn label(self) -> &'static str {
        use Signal::*;
        match self {
            AttackLight => "Attack (light)",
            AttackHeavy => "Attack (heavy)",
            Defend => "Defend / block",
            Special => "Special",
            Dodge => "Dodge / roll",
            Run => "Run",
            Jump => "Jump",
            ChordBegin => "Chord begin",
            Interact => "Interact",
            Activate => "Activate",
            LockOn => "Lock-on",
            Crouch => "Crouch",
            ItemSelect => "Item select",
            Confirm => "Confirm",
            Cancel => "Cancel / back",
            TabNext => "Tab \u{2192}",
            TabPrev => "\u{2190} Tab",
            NavUp => "Nav up",
            NavDown => "Nav down",
            NavLeft => "Nav left",
            NavRight => "Nav right",
            Yes => "Yes",
            No => "No",
        }
    }
}

/// When a control fires its signal.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Edge down — combat responsiveness.
    Press,
    /// Edge up — menu UX (escape a mis-press before releasing).
    Release,
    /// Held continuously (a modal state, e.g. B-hold = Run). Not an edge.
    Hold,
}

/// A physical control the viewer tracks (a gamepad button or a key).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Btn(Gb),
    Key(Key),
}

impl Control {
    /// Is the control down in `s`?
    pub fn down(self, s: &InputState) -> bool {
        match self {
            Control::Btn(b) => s.gamepad(0).is_some_and(|g| g.button_down(b)),
            Control::Key(k) => s.key_down(k),
        }
    }
}

/// One mapping row: a control + when it fires → the signal (+ an optional note).
pub struct Bind {
    pub control: Control,
    pub when: When,
    pub signal: Signal,
}

const fn b(control: Control, when: When, signal: Signal) -> Bind {
    Bind { control, when, signal }
}

/// The demo input contexts (Aaron's starter set).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    World,
    Menu,
    Inventory,
    Dialog,
}

impl Ctx {
    pub const ALL: [Ctx; 4] = [Ctx::World, Ctx::Menu, Ctx::Inventory, Ctx::Dialog];

    /// Short label for the context tab bar.
    pub fn short(self) -> &'static str {
        match self {
            Ctx::World => "World",
            Ctx::Menu => "Menu",
            Ctx::Inventory => "Inventory",
            Ctx::Dialog => "Dialog",
        }
    }

    pub fn next(self) -> Ctx {
        let i = Ctx::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Ctx::ALL[(i + 1) % Ctx::ALL.len()]
    }
}

/// The binding table for a context. Rebuilt per call (cheap); a control may appear more
/// than once with different `when` (Y: press→ChordBegin, release→Jump; B: press→Dodge,
/// hold→Run).
pub fn binds(ctx: Ctx) -> Vec<Bind> {
    use Control::{Btn, Key as K};
    use Signal::*;
    use When::{Hold, Press, Release};
    match ctx {
        // Combat is PRESS-driven (responsive). Y is the split case; B is press+hold.
        Ctx::World => vec![
            b(Btn(Gb::RightBumper), Press, AttackLight),
            b(Btn(Gb::RightTrigger), Press, AttackHeavy),
            b(Btn(Gb::LeftBumper), Press, Defend),
            b(Btn(Gb::LeftTrigger), Press, Special),
            b(Btn(Gb::East), Press, Dodge),
            b(Btn(Gb::East), Hold, Run),
            b(Btn(Gb::North), Press, ChordBegin),
            b(Btn(Gb::North), Release, Jump),
            b(Btn(Gb::West), Press, Interact),
            b(Btn(Gb::South), Press, Activate),
            b(Btn(Gb::RightStick), Press, LockOn),
            b(Btn(Gb::LeftStick), Press, Crouch),
            b(Btn(Gb::DPadUp), Press, ItemSelect),
            b(Btn(Gb::DPadDown), Press, ItemSelect),
            b(Btn(Gb::DPadLeft), Press, ItemSelect),
            b(Btn(Gb::DPadRight), Press, ItemSelect),
            // keyboard parity (a taste)
            b(K(Key::Space), Press, Jump),
            b(K(Key::LeftShift), Hold, Run),
        ],
        // Menus fire on RELEASE (escape a mis-press); nav is press-with-repeat.
        Ctx::Menu => vec![
            b(Btn(Gb::South), Release, Confirm),
            b(Btn(Gb::East), Release, Cancel),
            b(Btn(Gb::LeftBumper), Release, TabPrev),
            b(Btn(Gb::RightBumper), Release, TabNext),
            b(Btn(Gb::DPadUp), Press, NavUp),
            b(Btn(Gb::DPadDown), Press, NavDown),
            b(Btn(Gb::DPadLeft), Press, NavLeft),
            b(Btn(Gb::DPadRight), Press, NavRight),
        ],
        Ctx::Inventory => vec![
            b(Btn(Gb::South), Release, Confirm),
            b(Btn(Gb::East), Release, Cancel),
            b(Btn(Gb::North), Release, ItemSelect),
            b(Btn(Gb::LeftBumper), Release, TabPrev),
            b(Btn(Gb::RightBumper), Release, TabNext),
            b(Btn(Gb::DPadUp), Press, NavUp),
            b(Btn(Gb::DPadDown), Press, NavDown),
            b(Btn(Gb::DPadLeft), Press, NavLeft),
            b(Btn(Gb::DPadRight), Press, NavRight),
        ],
        Ctx::Dialog => vec![
            b(Btn(Gb::South), Release, Yes),
            b(Btn(Gb::East), Release, No),
            b(Btn(Gb::DPadUp), Press, NavUp),
            b(Btn(Gb::DPadDown), Press, NavDown),
        ],
    }
}

/// Whether a bind fires THIS frame, given the previous + current snapshots (last-frame
/// edge detection). `Hold` "fires" only on the frame it becomes held, so it logs once.
pub fn fired(bind: &Bind, prev: Option<&InputState>, curr: &InputState) -> bool {
    let now = bind.control.down(curr);
    let before = prev.is_some_and(|p| bind.control.down(p));
    match bind.when {
        When::Press => now && !before,
        When::Release => !now && before,
        When::Hold => now && !before,
    }
}

