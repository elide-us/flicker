//! **The bench's roster, as data.**
//!
//! What the surface CONTAINS — its pages and tabs — as a small data roster the
//! scene reads for COUNTS and dispatch, plus the stable id / bind / action names
//! shared by the static component tree (`populous.scene.json`), the Model, and the
//! dispatcher so the three cannot drift apart.
//!
//! This module no longer BUILDS the tree. The surface is authored as data in
//! `populous.scene.json` (`default_page` then `paged_menu`, `frame`, `multi_view`
//! and its three panes) and its per-tab slices are lit by `populous.lua`'s
//! `arrange()`. The former `ui::build` Rust tree builder — and its `Holds` /
//! `PaneRow` pane vocabulary and template-name constants — were retired once that
//! static tree landed and was verified in-window; only the DATA the scene still
//! reads lives on.

/// The three panes' ids — each is ALSO its `tab_group` in the static tree, which is
/// what makes it a panel for the walker's panel cursor (the left stick cycles these).
pub const LEFT_PANE: &str = "pop_left";
pub const VIEW_PANE: &str = "pop_view";
pub const RIGHT_PANE: &str = "pop_right";
/// The viewport node — the `rtt` whose rect the walker reserves and the scene fills
/// with the globe (`VIEW_PANE` + the `rtt_panel` proto's `_rtt` suffix).
pub const VIEW_SLOT: &str = "pop_view_rtt";
/// The `stages.<source>` block the globe is authored by — the default light source
/// and backdrop the world is seen under.
pub const STAGE_SOURCE: &str = "populous_globe";

/// The size dial's two-way bind — the frequency the map is rebuilt at. Stated once
/// so the tree, the Model and the dispatcher cannot drift apart.
pub const FREQ_BIND: &str = "pop_freq";
/// The three readout binds: the scene publishes a PRE-FORMATTED string on each and
/// the `stat_row` proto shows it. A number never reaches a node.
pub const HEXES_BIND: &str = "pop_hexes";
pub const DIAMETER_BIND: &str = "pop_diameter";
pub const TILE_BIND: &str = "pop_tile";
/// The seams tab's one action. Nothing is built behind it yet — the scene answers
/// it with a loud warn, so the authored name fails LOUD instead of to nothing
/// (rule 4BB12A75).
pub const SEAMS_ACTION: &str = "pop_seams_randomize";

/// Model keys the rails bind to. The `paged_menu` proto names these as its
/// `@page_bind` / `@tab_bind` / `@tabs_shown` defaults; stated once here so the
/// tree, the Model and the dispatcher cannot drift apart.
pub const PAGE_BIND: &str = "page";
pub const TAB_BIND: &str = "tab";
pub const TABS_SHOWN: &str = "paged_tabs_shown";

/// One section tab within a page.
pub struct Tab {
    pub id: &'static str,
    /// Stringtable token — never raw English (rule: every UI string localized).
    pub label: &'static str,
}

/// One top-level page: what the page rail shows, and the tabs it contains.
pub struct Page {
    pub id: &'static str,
    pub label: &'static str,
    pub tabs: &'static [Tab],
}

/// **The bench's page/tab roster.** One page, two tabs — the MAP view and the SEAMS
/// view. The scene reads only COUNTS from this (the rail bounds + the dispatch
/// clamp); each tab's actual panes are authored in `populous.scene.json` and gated
/// by `arrange()`. The code that reads this never learns the counts, so the next
/// page or tab costs exactly one row here.
pub static PAGES: &[Page] = &[Page {
    id: "world",
    label: "$pop_page_world",
    tabs: &[
        Tab {
            id: "map",
            label: "$pop_tab_map",
        },
        Tab {
            id: "seams",
            label: "$pop_tab_seams",
        },
    ],
}];

/// The page at `sel`, clamped — the roster is the authority on what exists, so an
/// out-of-range selection reads as the last page rather than panicking.
pub fn page(sel: usize) -> &'static Page {
    &PAGES[sel.min(PAGES.len() - 1)]
}
