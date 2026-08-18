//! Window-free directional-nav helpers (spec §4.4 / §8).
//!
//! Pure functions over a flat list of [`Focusable`]s — no window, no walker, so
//! they unit-test in isolation. The output is an id the caller writes into
//! `UiState.focus` through the walker adapter, so pointer + d-pad share one focus
//! identity (spec §4.3).
//!
//! Traversal is **ordinal-primary within the active group**: [`nav`] moves by
//! `ordinal` inside the current item's `group` (wrapping), and [`tab`] cycles
//! between groups. Geometric nearest-in-direction (using [`Focusable::rect`]) is
//! a **documented future refinement** — `rect` is carried now so the signature is
//! stable, but this slice does not consult it.

/// A focusable UI node, flattened for routing. `id` is the `UiState.focus`
/// identity (spec §4.3); `group` + `ordinal` are the Lua-authored
/// `tab_group` / `nav_ordinal` props (spec §8). `rect` (`[x, y, w, h]`) is
/// reserved for the future geometric refinement and is not read this slice.
#[derive(Clone, Debug, PartialEq)]
pub struct Focusable {
    /// Node id — the shared pointer/d-pad focus identity.
    pub id: String,
    /// The `tab_group` this node belongs to.
    pub group: String,
    /// Ordinal within the group (traversal order).
    pub ordinal: u32,
    /// Screen rect `[x, y, w, h]` — reserved for geometric nav (future).
    pub rect: [f32; 4],
}

/// A directional-nav intent. `Down`/`Right` step forward by ordinal;
/// `Up`/`Left` step backward.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NavDir {
    /// Backward by ordinal.
    Up,
    /// Forward by ordinal.
    Down,
    /// Backward by ordinal.
    Left,
    /// Forward by ordinal.
    Right,
}

impl NavDir {
    /// Whether this direction advances (`Down`/`Right`) rather than retreats.
    fn is_forward(self) -> bool {
        matches!(self, NavDir::Down | NavDir::Right)
    }
}

fn find<'a>(items: &'a [Focusable], id: &str) -> Option<&'a Focusable> {
    items.iter().find(|f| f.id.as_str() == id)
}

/// Sort key for entering the list with no current focus: `(ordinal, group, id)`,
/// fully deterministic.
fn entry_key(f: &Focusable) -> (u32, &str, &str) {
    (f.ordinal, f.group.as_str(), f.id.as_str())
}

/// Move focus one step in `dir` from `current` (spec §4.4).
///
/// - With a `current` in the list: steps by `ordinal` **within that item's
///   group** (wrapping at the ends), so nav never leaves the active group.
/// - With no `current` (or an id not in `items`): enters the list at the
///   lowest-`ordinal` node for a forward direction, the highest for a backward
///   one.
/// - Empty `items`: `None`.
pub fn nav(items: &[Focusable], current: Option<&str>, dir: NavDir) -> Option<String> {
    let forward = dir.is_forward();

    match current.and_then(|id| find(items, id)) {
        Some(cur) => {
            // Group-filtered, ordinal-sorted ring (id as a stable tiebreak).
            let mut ring: Vec<&Focusable> = items.iter().filter(|f| f.group == cur.group).collect();
            ring.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)));
            let pos = ring.iter().position(|f| f.id == cur.id)?;
            let n = ring.len();
            let next = if forward {
                (pos + 1) % n
            } else {
                (pos + n - 1) % n
            };
            Some(ring[next].id.clone())
        }
        None => {
            // No active group: enter at the global extreme by ordinal.
            let entry = if forward {
                items.iter().min_by(|a, b| entry_key(a).cmp(&entry_key(b)))
            } else {
                items.iter().max_by(|a, b| entry_key(a).cmp(&entry_key(b)))
            };
            entry.map(|f| f.id.clone())
        }
    }
}

/// Cycle to the next (`forward`) or previous group and focus its lowest-`ordinal`
/// node (spec §4.4 / §8 — `TabNext`/`TabPrev` cycle groups).
///
/// Groups are ordered by **first appearance** in `items` (authoring order).
/// - With a `current` in the list: moves to the adjacent group, wrapping.
/// - With no `current`: enters the first group (forward) or last (backward).
/// - Empty `items`: `None`.
/// - A SINGLE group with `current` already in it: `None` — there is no other pane to
///   cycle to, so the left stick is a clean no-op (a flat single-context surface like the
///   settings modal must not have its focus yanked to the top row by a stray stick nudge).
pub fn tab(items: &[Focusable], current: Option<&str>, forward: bool) -> Option<String> {
    // Distinct groups in first-appearance order.
    let mut groups: Vec<&str> = Vec::new();
    for f in items {
        let g = f.group.as_str();
        if !groups.contains(&g) {
            groups.push(g);
        }
    }
    if groups.is_empty() {
        return None;
    }

    let target: &str = match current.and_then(|id| find(items, id)) {
        Some(cur) => {
            if groups.len() == 1 {
                return None;
            }
            let pos = groups.iter().position(|g| *g == cur.group.as_str())?;
            let n = groups.len();
            let next = if forward {
                (pos + 1) % n
            } else {
                (pos + n - 1) % n
            };
            groups[next]
        }
        None => {
            if forward {
                groups[0]
            } else {
                groups[groups.len() - 1]
            }
        }
    };

    items
        .iter()
        .filter(|f| f.group.as_str() == target)
        .min_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.id.cmp(&b.id)))
        .map(|f| f.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, group: &str, ordinal: u32) -> Focusable {
        Focusable {
            id: id.into(),
            group: group.into(),
            ordinal,
            rect: [0.0; 4],
        }
    }

    #[test]
    fn nav_moves_by_ordinal_with_wrap() {
        let items = vec![f("x", "a", 0), f("y", "a", 1), f("z", "a", 2)];
        assert_eq!(nav(&items, Some("x"), NavDir::Down).as_deref(), Some("y"));
        assert_eq!(nav(&items, Some("y"), NavDir::Down).as_deref(), Some("z"));
        // Forward wrap: last → first.
        assert_eq!(nav(&items, Some("z"), NavDir::Down).as_deref(), Some("x"));
        // Backward wrap: first → last.
        assert_eq!(nav(&items, Some("x"), NavDir::Up).as_deref(), Some("z"));
        // Right == Down, Left == Up.
        assert_eq!(nav(&items, Some("x"), NavDir::Right).as_deref(), Some("y"));
        assert_eq!(nav(&items, Some("y"), NavDir::Left).as_deref(), Some("x"));
    }

    #[test]
    fn nav_stays_within_current_group() {
        let items = vec![
            f("a0", "A", 0),
            f("a1", "A", 1),
            f("b0", "B", 0),
            f("b1", "B", 1),
        ];
        // From a group-A item, nav wraps inside A and never returns a B id.
        assert_eq!(nav(&items, Some("a1"), NavDir::Down).as_deref(), Some("a0"));
        assert_eq!(nav(&items, Some("a0"), NavDir::Up).as_deref(), Some("a1"));
    }

    #[test]
    fn nav_no_current_enters_at_extreme() {
        let items = vec![f("a0", "A", 0), f("a1", "A", 1), f("b0", "B", 0)];
        // Forward → lowest (ordinal, group): a0 (ord 0, A) beats b0 (ord 0, B).
        assert_eq!(nav(&items, None, NavDir::Down).as_deref(), Some("a0"));
        // Backward → highest ordinal: a1 (ord 1).
        assert_eq!(nav(&items, None, NavDir::Up).as_deref(), Some("a1"));
    }

    #[test]
    fn nav_unknown_current_is_treated_as_no_current() {
        let items = vec![f("a0", "A", 0), f("a1", "A", 1)];
        assert_eq!(
            nav(&items, Some("ghost"), NavDir::Down).as_deref(),
            Some("a0")
        );
    }

    #[test]
    fn nav_empty_is_none() {
        let items: Vec<Focusable> = vec![];
        assert_eq!(nav(&items, None, NavDir::Down), None);
        assert_eq!(nav(&items, Some("x"), NavDir::Down), None);
    }

    #[test]
    fn tab_cycles_groups_with_wrap() {
        let items = vec![
            f("a0", "A", 0),
            f("a1", "A", 1),
            f("b0", "B", 0),
            f("b1", "B", 1),
            f("c0", "C", 0),
        ];
        // A → B → C, each landing on the group's lowest ordinal.
        assert_eq!(tab(&items, Some("a1"), true).as_deref(), Some("b0"));
        assert_eq!(tab(&items, Some("b0"), true).as_deref(), Some("c0"));
        // Forward wrap C → A.
        assert_eq!(tab(&items, Some("c0"), true).as_deref(), Some("a0"));
        // Backward wrap A → C.
        assert_eq!(tab(&items, Some("a0"), false).as_deref(), Some("c0"));
    }

    #[test]
    fn tab_no_current_enters_first_or_last_group() {
        let items = vec![f("a0", "A", 0), f("b0", "B", 0)];
        assert_eq!(tab(&items, None, true).as_deref(), Some("a0"));
        assert_eq!(tab(&items, None, false).as_deref(), Some("b0"));
    }

    #[test]
    fn tab_empty_is_none() {
        let items: Vec<Focusable> = vec![];
        assert_eq!(tab(&items, None, true), None);
    }

    /// A SINGLE group with the cursor already inside it is a no-op — there is no other
    /// pane to cycle to, so the left stick must not yank focus to the group's top item
    /// (the flat single-context surface, e.g. the settings modal, nav-tier contract
    /// 1B5F6BB8). With NO current, a single group still acquires (useful first landing).
    #[test]
    fn tab_single_group_with_current_is_a_no_op() {
        let items = vec![f("r0", "settings_rows", 0), f("r1", "settings_rows", 1)];
        assert_eq!(
            tab(&items, Some("r1"), true),
            None,
            "no other pane to cycle to"
        );
        assert_eq!(tab(&items, Some("r0"), false), None);
        // …but with no focus yet, entering the sole group is still useful acquisition.
        assert_eq!(tab(&items, None, true).as_deref(), Some("r0"));
    }
}
