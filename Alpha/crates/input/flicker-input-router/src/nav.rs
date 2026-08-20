//! Window-free directional-nav helpers (spec §4.4 / §8).
//!
//! Pure functions over a flat list of [`Focusable`]s — no window, no walker, so
//! they unit-test in isolation. The output is an id the caller writes into
//! `UiState.focus` through the walker adapter, so pointer + d-pad share one focus
//! identity (spec §4.3).
//!
//! Two traversal modes share the same flat [`Focusable`] list:
//! - [`nav`] is **ordinal-primary within the active group** — it moves by
//!   `ordinal` inside the current item's `group` (wrapping). It drives the
//!   in-cluster d-pad ring, and is the fallback when no geometry is available.
//! - [`nav_geometric`] is **geometric nearest-in-direction** over a set of panel
//!   STOPS, using each stop's resolved `rect`. It drives the flattened panel tier
//!   (both stick and d-pad), where "Left from the centre lands on the channel
//!   beside it" has to mean what it says on screen. Deterministic and total — a
//!   stated rule with explicit tiebreaks, not a heuristic.

/// A focusable UI node, flattened for routing. `id` is the `UiState.focus`
/// identity (spec §4.3); `group` + `ordinal` are the Lua-authored
/// `tab_group` / `nav_ordinal` props (spec §8). `rect` (`[x, y, w, h]`) is the
/// resolved screen rect, consulted by [`nav_geometric`].
#[derive(Clone, Debug, PartialEq)]
pub struct Focusable {
    /// Node id — the shared pointer/d-pad focus identity.
    pub id: String,
    /// The `tab_group` this node belongs to.
    pub group: String,
    /// Ordinal within the group (traversal order).
    pub ordinal: u32,
    /// Screen rect `[x, y, w, h]` from the resolved layout — read by
    /// [`nav_geometric`]; the walker patches it in per frame from `UiFrame.rects`.
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

/// Geometric nearest-in-direction over a set of panel STOPS, using each stop's
/// resolved screen `rect`. This is the flattened panel tier: "Left from the
/// centre panel lands on the channel beside it."
///
/// Deterministic and total (BA4487BD — nav is never a fuzzy guess):
/// - The candidate must lie strictly in `dir` by CENTRE (`Left`: its centre-x is
///   left of `current`'s, etc.), and have a non-degenerate rect.
/// - **Tier 1 — banded**: candidates whose orthogonal extent overlaps
///   `current`'s by more than 0 px, ranked ascending by `(facing-edge gap,
///   ordinal, id)`. The gap is the signed distance between the facing edges, so
///   the nearest in the travel direction wins and equal-distance peers fall to
///   the authored `(ordinal, id)` order.
/// - **Tier 2 — unbanded** (only if Tier 1 is empty): ranked ascending by
///   `(|primary centre delta|, |orthogonal centre delta|, ordinal, id)`.
/// - **No candidate → `None`, and there is NO WRAP** — a directional press at the
///   surface edge does nothing (wrapping stays the ring's business, `PanelNext`).
///
/// Returns `None` when `current` is absent or its rect is degenerate; the caller
/// then falls back to the ordinal ring ([`nav`]), which keeps headless callers
/// (tests, rect-less scenes) working.
pub fn nav_geometric(stops: &[Focusable], current: &str, dir: NavDir) -> Option<String> {
    let a = find(stops, current)?;
    let [ax, ay, aw, ah] = a.rect;
    if !(aw > 0.0 && ah > 0.0) {
        return None;
    }
    let (acx, acy) = (ax + aw * 0.5, ay + ah * 0.5);

    // For a candidate strictly in `dir`, return (facing-edge gap, band overlap,
    // primary centre delta, orthogonal centre delta); `None` if not in `dir` or
    // degenerate. Overlap > 0 means "banded" (Tier 1).
    let facing = |b: &Focusable| -> Option<(f32, f32, f32, f32)> {
        let [bx, by, bw, bh] = b.rect;
        if !(bw > 0.0 && bh > 0.0) {
            return None;
        }
        let (bcx, bcy) = (bx + bw * 0.5, by + bh * 0.5);
        match dir {
            NavDir::Left => (bcx < acx).then(|| {
                let gap = ax - (bx + bw);
                let overlap = (ay + ah).min(by + bh) - ay.max(by);
                (gap, overlap, acx - bcx, (acy - bcy).abs())
            }),
            NavDir::Right => (bcx > acx).then(|| {
                let gap = bx - (ax + aw);
                let overlap = (ay + ah).min(by + bh) - ay.max(by);
                (gap, overlap, bcx - acx, (acy - bcy).abs())
            }),
            NavDir::Up => (bcy < acy).then(|| {
                let gap = ay - (by + bh);
                let overlap = (ax + aw).min(bx + bw) - ax.max(bx);
                (gap, overlap, acy - bcy, (acx - bcx).abs())
            }),
            NavDir::Down => (bcy > acy).then(|| {
                let gap = by - (ay + ah);
                let overlap = (ax + aw).min(bx + bw) - ax.max(bx);
                (gap, overlap, bcy - acy, (acx - bcx).abs())
            }),
        }
    };

    let mut banded: Vec<(f32, u32, &str)> = Vec::new();
    let mut unbanded: Vec<(f32, f32, u32, &str)> = Vec::new();
    for b in stops {
        if b.id == current {
            continue;
        }
        let Some((gap, overlap, primary, orth)) = facing(b) else {
            continue;
        };
        if overlap > 0.0 {
            banded.push((gap, b.ordinal, b.id.as_str()));
        } else {
            unbanded.push((primary, orth, b.ordinal, b.id.as_str()));
        }
    }

    if !banded.is_empty() {
        banded.sort_by(|x, y| {
            x.0.total_cmp(&y.0)
                .then(x.1.cmp(&y.1))
                .then_with(|| x.2.cmp(y.2))
        });
        return Some(banded[0].2.to_string());
    }
    unbanded.sort_by(|x, y| {
        x.0.total_cmp(&y.0)
            .then(x.1.total_cmp(&y.1))
            .then(x.2.cmp(&y.2))
            .then_with(|| x.3.cmp(y.3))
    });
    unbanded.first().map(|c| c.3.to_string())
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

    /// A stop with an explicit rect (group is irrelevant to the geometric tier).
    fn fr(id: &str, ordinal: u32, rect: [f32; 4]) -> Focusable {
        Focusable {
            id: id.into(),
            group: String::new(),
            ordinal,
            rect,
        }
    }

    /// Centre `c` with an L/R/U/D neighbour banded to it on each side.
    fn cross() -> Vec<Focusable> {
        vec![
            fr("c", 0, [100.0, 100.0, 40.0, 40.0]),
            fr("l", 1, [40.0, 105.0, 40.0, 40.0]),
            fr("r", 2, [160.0, 105.0, 40.0, 40.0]),
            fr("u", 3, [105.0, 40.0, 40.0, 40.0]),
            fr("d", 4, [105.0, 160.0, 40.0, 40.0]),
        ]
    }

    #[test]
    fn geometric_moves_to_the_banded_neighbour() {
        let s = cross();
        assert_eq!(nav_geometric(&s, "c", NavDir::Left).as_deref(), Some("l"));
        assert_eq!(nav_geometric(&s, "c", NavDir::Right).as_deref(), Some("r"));
        assert_eq!(nav_geometric(&s, "c", NavDir::Up).as_deref(), Some("u"));
        assert_eq!(nav_geometric(&s, "c", NavDir::Down).as_deref(), Some("d"));
    }

    #[test]
    fn geometric_does_not_wrap_at_the_edge() {
        let s = cross();
        // Nothing further left of the left cell → no move (wrap is PanelNext's job).
        assert_eq!(nav_geometric(&s, "l", NavDir::Left), None);
    }

    #[test]
    fn geometric_banded_beats_a_closer_unbanded_centre() {
        let s = vec![
            fr("a", 0, [0.0, 100.0, 40.0, 40.0]), // centre (20,120)
            fr("n", 1, [60.0, 100.0, 40.0, 40.0]), // banded (same y), gap 20
            fr("x", 2, [50.0, 0.0, 40.0, 40.0]),   // closer centre-x, but no y-overlap
        ];
        assert_eq!(nav_geometric(&s, "a", NavDir::Right).as_deref(), Some("n"));
    }

    #[test]
    fn geometric_ties_break_by_ordinal_then_id() {
        // Identical-geometry peers to the right → lowest ordinal wins.
        let s = vec![
            fr("a", 0, [0.0, 100.0, 40.0, 40.0]),
            fr("hi", 5, [60.0, 100.0, 40.0, 40.0]),
            fr("lo", 2, [60.0, 100.0, 40.0, 40.0]),
        ];
        assert_eq!(nav_geometric(&s, "a", NavDir::Right).as_deref(), Some("lo"));
    }

    #[test]
    fn geometric_falls_back_to_unbanded_when_nothing_bands() {
        let s = vec![
            fr("a", 0, [0.0, 0.0, 40.0, 40.0]),
            fr("diag", 1, [100.0, 100.0, 40.0, 40.0]), // only reachable diagonally
        ];
        assert_eq!(
            nav_geometric(&s, "a", NavDir::Right).as_deref(),
            Some("diag")
        );
    }

    #[test]
    fn geometric_degenerate_or_unknown_current_is_none() {
        let s = vec![fr("a", 0, [0.0; 4]), fr("b", 1, [60.0, 0.0, 40.0, 40.0])];
        assert_eq!(
            nav_geometric(&s, "a", NavDir::Right),
            None,
            "no anchor rect → caller falls back to the ordinal ring"
        );
        assert_eq!(nav_geometric(&cross(), "ghost", NavDir::Left), None);
    }
}
