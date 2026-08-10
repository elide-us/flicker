//! **The bench's surface, as data.**
//!
//! The shape, top to bottom:
//!
//! ```text
//! default_page              screen root (the five signals this page dispatches)
//!  └─ frame                 the PAGE chrome — runes, backdrop, edge clearance
//!      └─ paged_menu        the PTT: LT/RT pages, LB/RB tabs, one content slot
//!          └─ frame         the page's content panel (runes OFF — inner frame)
//!              └─ multi_view    N panes as N siblings — one per roster entry
//! ```
//!
//! # The roster is DATA
//!
//! [`PAGES`] is the whole surface: pages, their tabs, and — since this pass —
//! each tab's PANES. A pane row names its id (which is also its focus group)
//! and the one thing it holds; [`content`] turns a row into `multi_view` slot
//! content and knows nothing else. So a second page, a third tab or a fourth
//! pane is a ROW here, never an edit to the navigation and never a new
//! function. There is deliberately no `if pages == 1` anywhere: a control that
//! special-cases the count it happens to have today is a control that has to be
//! rewritten the first time it grows.
//!
//! Every node this module emits is a TEMPLATE INSTANCE — `default_page`,
//! `paged_menu`, `frame`, `multi_view`, `ui_panel`, `rtt_panel`,
//! `pop_size_dial`, `stat_row` — plus the `option` entries the two rails read.
//! Nothing here builds a `text` node, names a style, or formats a number: a
//! caption is a `$token` the proto carries, a value is a Model bind the scene
//! pre-formats, and a colour is a style token in `ui_elements.json`.
//!
//! # The bench knows nothing about focus
//!
//! There is no pane enum here, no rim style and no enter/exit mode. Each pane
//! carries its own `tab_group`; the WALKER's panel tier (the left stick) cycles
//! those groups, and the `panel` component draws its own rim from the focus the
//! walker holds. Likewise the rails: each strip carries its own
//! `next_action`/`prev_action`, so the PTT steps ITSELF and the scene only ever
//! sees the resulting numeric index. There is no `step` here.

use flicker::script::{UiNode, Value};
use flicker::ui::TemplateRegistry;

/// The proto every page starts as — frame + the menu signal.
pub const DEFAULT_PAGE: &str = "default_page";
/// The PTT — the page/tab navigation control docked into the default page.
const PAGED_MENU: &str = "paged_menu";
/// The docking container a page's content panel is built from.
const FRAME: &str = "frame";
/// The N-panel view docked inside the content panel — one slot, N siblings.
const MULTI_VIEW: &str = "multi_view";
/// A control/readout pane of that view.
const UI_PANEL: &str = "ui_panel";
/// The viewport pane of that view.
const RTT_PANEL: &str = "rtt_panel";
/// The planet-size dial, whole — the slider proto the left pane instances.
const POP_SIZE_DIAL: &str = "pop_size_dial";
/// One caption/value readout line — the stats pane instances it three times.
const STAT_ROW: &str = "stat_row";

/// The three panes' ids — each is ALSO its `tab_group`, which is what makes it a
/// panel for the walker's panel cursor (the left stick cycles these).
pub const LEFT_PANE: &str = "pop_left";
pub const VIEW_PANE: &str = "pop_view";
pub const RIGHT_PANE: &str = "pop_right";
/// The viewport node — the `rtt` whose rect the walker reserves and the scene
/// fills with the globe. `VIEW_PANE` + the `rtt_panel` proto's `_rtt` suffix.
pub const VIEW_SLOT: &str = "pop_view_rtt";
/// The `stages.<source>` block the globe is authored by — the default light
/// source and backdrop the world is seen under.
pub const STAGE_SOURCE: &str = "populous_globe";

/// The size dial's two-way bind — the frequency the map is rebuilt at. Stated
/// once so the tree, the Model and the dispatcher cannot drift apart.
pub const FREQ_BIND: &str = "pop_freq";
/// The three readout binds: the scene publishes a PRE-FORMATTED string on each
/// and the `stat_row` proto shows it. A number never reaches a node.
pub const HEXES_BIND: &str = "pop_hexes";
pub const DIAMETER_BIND: &str = "pop_diameter";
pub const TILE_BIND: &str = "pop_tile";
/// The seams tab's one action. Nothing is built behind it yet — the scene
/// answers it with a loud warn, so the authored name fails LOUD instead of to
/// nothing (rule 4BB12A75).
pub const SEAMS_ACTION: &str = "pop_seams_randomize";

/// Model keys the rails bind to. The `paged_menu` proto names these as its
/// `@page_bind` / `@tab_bind` / `@tabs_shown` defaults; stated once here so the
/// tree, the Model and the dispatcher cannot drift apart.
pub const PAGE_BIND: &str = "page";
pub const TAB_BIND: &str = "tab";
pub const TABS_SHOWN: &str = "paged_tabs_shown";

/// What ONE pane holds — the whole vocabulary the roster can name. Adding a
/// pane kind is a variant here and an arm in [`fill`]; it is never a new
/// per-tab builder.
pub enum Holds {
    /// The `pop_size_dial`, bound to [`FREQ_BIND`].
    SizeDial,
    /// The hex world's viewport — an `rtt_panel` rather than a `ui_panel`.
    World,
    /// The three `stat_row`s, each on its own bind.
    Stats,
    /// One `button` wearing a `$token` face and firing a named action.
    Action { action: &'static str, label: &'static str },
    /// Nothing: the `ui_panel` proto's own localized `$ui_pane_empty` fallback.
    Empty,
}

/// One pane of a tab's multi-view: its id (which is also its focus group) and
/// what it holds.
pub struct PaneRow {
    pub id: &'static str,
    pub holds: Holds,
}

/// One section tab within a page, and the panes it shows.
pub struct Tab {
    pub id: &'static str,
    /// Stringtable token — never raw English (rule: every UI string localized).
    pub label: &'static str,
    /// The panes, left to right. `multi_view` splices N siblings out of ONE
    /// slot, so a one- or four-pane tab is the same row with a different list.
    pub panes: &'static [PaneRow],
}

/// One top-level page: what the page rail shows, and the tabs it contains.
pub struct Page {
    pub id: &'static str,
    pub label: &'static str,
    pub tabs: &'static [Tab],
}

/// **The bench's whole surface.** One page, two tabs — the MAP view and the
/// SEAMS view — and both tabs name the SAME centre pane, so the one world is
/// what every tab shows. The code that reads this never learns the counts, so
/// the next page, tab or pane costs exactly one row here.
pub static PAGES: &[Page] = &[Page {
    id: "world",
    label: "$pop_page_world",
    tabs: &[
        Tab {
            id: "map",
            label: "$pop_tab_map",
            panes: &[
                PaneRow { id: LEFT_PANE, holds: Holds::SizeDial },
                PaneRow { id: VIEW_PANE, holds: Holds::World },
                PaneRow { id: RIGHT_PANE, holds: Holds::Stats },
            ],
        },
        Tab {
            id: "seams",
            label: "$pop_tab_seams",
            panes: &[
                PaneRow {
                    id: LEFT_PANE,
                    holds: Holds::Action {
                        action: SEAMS_ACTION,
                        label: "$pop_seams_randomize",
                    },
                },
                PaneRow { id: VIEW_PANE, holds: Holds::World },
                PaneRow { id: RIGHT_PANE, holds: Holds::Empty },
            ],
        },
    ],
}];

/// The page at `sel`, clamped — the roster is the authority on what exists, so an
/// out-of-range selection reads as the last page rather than panicking.
pub fn page(sel: usize) -> &'static Page {
    &PAGES[sel.min(PAGES.len() - 1)]
}

/// A rail entry: the `value` the bind compares against and the `$token` it shows.
/// `tabs` and `pill_toggle` both read children as `{ value, label }`, so one
/// helper serves the page rail and the tab rail. An index is a NUMBER — here,
/// on the bind, and in the dispatcher (rule 1B64FF03).
fn rail_entry(index: usize, label: &str) -> UiNode {
    let mut n = UiNode { component: "option".into(), ..Default::default() };
    n.props.insert("value".into(), Value::Number(index as f64));
    n.props.insert("label".into(), Value::Text(label.into()));
    n
}

/// A template instance carrying `props` — the ONE node shape this module emits.
fn instance(template: &str, props: &[(&str, Value)]) -> UiNode {
    let mut n = UiNode { template: Some(template.into()), ..Default::default() };
    for (k, v) in props {
        n.props.insert((*k).into(), v.clone());
    }
    n
}

/// What a pane's row puts inside its panel — the instances, and nothing else.
/// An empty list leaves the pane on the proto's own localized fallback.
fn fill(row: &PaneRow) -> Vec<UiNode> {
    match row.holds {
        // The dial's WHOLE contract lives in the proto: pointer drag with
        // commit-on-release, the live readout on the handle, and the walker's
        // pad channel. The instance passes identity, range and focus order.
        Holds::SizeDial => vec![instance(
            POP_SIZE_DIAL,
            &[
                ("id", Value::Text(FREQ_BIND.into())),
                ("bind", Value::Text(FREQ_BIND.into())),
                ("label", Value::Text("$pop_size".into())),
                ("min", Value::Number(f64::from(crate::map::MIN_FREQ))),
                ("max", Value::Number(f64::from(crate::map::MAX_FREQ))),
                ("tab_group", Value::Text(row.id.into())),
            ],
        )],
        // Captions are `$tokens` (the unit lives in the caption, so the value
        // stays pure data); every value is a Model bind the scene pre-formats.
        Holds::Stats => [
            ("$pop_stat_hexes", HEXES_BIND),
            ("$pop_stat_diameter", DIAMETER_BIND),
            ("$pop_stat_tile", TILE_BIND),
        ]
        .iter()
        .map(|(caption, bind)| {
            instance(
                STAT_ROW,
                &[
                    ("caption", Value::Text((*caption).into())),
                    ("value_bind", Value::Text((*bind).into())),
                ],
            )
        })
        .collect(),
        // A plain BUTTON: a `$token` face, one action name, and the focus order
        // its pane gives it. No style — the `button` component owns what a
        // button looks like, which is the whole point of a component.
        Holds::Action { action, label } => {
            let mut b = UiNode { component: "button".into(), ..Default::default() };
            b.action = Some(action.into());
            b.tab_group = row.id.into();
            b.nav_ordinal = 1; // the PANEL itself sits at 0 — the cursor lands there first
            b.size = Some(40.0);
            b.props.insert("label".into(), Value::Text(label.into()));
            vec![b]
        }
        Holds::World | Holds::Empty => Vec::new(),
    }
}

/// One PANE: a `ui_panel` (or, for the viewport, an `rtt_panel`) carrying its id
/// — which is also its `tab_group`, and that is what makes it a panel for the
/// walker's cursor — and its interior. No style, no rim, no focus flag: the
/// panel component owns all three.
fn pane(row: &PaneRow) -> UiNode {
    let world = matches!(row.holds, Holds::World);
    let mut p = instance(
        if world { RTT_PANEL } else { UI_PANEL },
        &[("id", Value::Text(row.id.into())), ("tab_group", Value::Text(row.id.into()))],
    );
    if world {
        p.props.insert("source".into(), Value::Text(STAGE_SOURCE.into()));
    }
    let content = fill(row);
    if !content.is_empty() {
        p.slots.insert("content".into(), content);
    }
    p
}

/// The content for one (page, tab) pair: ONE `multi_view` instance whose `panes`
/// slot holds the row's panes as N siblings.
///
/// A pair that names no panes is a hole in the roster, not a silent blank: it
/// warns and stands an empty cell up, so a tab added to [`PAGES`] without
/// content fails VISIBLY rather than rendering an unexplained void (4BB12A75).
fn content(page: &Page, tab: &Tab) -> UiNode {
    if tab.panes.is_empty() {
        tracing::warn!("populous: page `{}` tab `{}` has no view", page.id, tab.id);
        return UiNode { component: "cell".into(), ..Default::default() };
    }
    let mut view = UiNode { template: Some(MULTI_VIEW.into()), ..Default::default() };
    view.slots.insert("panes".into(), tab.panes.iter().map(pane).collect());
    view
}

/// **This bench's screen**: a [`DEFAULT_PAGE`] instance carrying the rail
/// intents, with the PTT docked into its content — rails built from the roster,
/// the selected tab's multi-panel view inside the content frame. Expanded at the
/// END — the one seam, so the scene and every gate walk the SAME tree.
pub fn build(sel_page: usize, sel_tab: usize, templates: &TemplateRegistry) -> UiNode {
    let page = page(sel_page);
    let sel_tab = sel_tab.min(page.tabs.len().saturating_sub(1));

    // The content panel: the inner docked `frame`, runes OFF (a second set of
    // corners inside the already-runed page chrome reads as noise). GROW, not
    // `w_frac`/`h_frac`: the frame sits in the content cell's vertical FLOW, and
    // a flow child without grow collapses to its measured chrome (~100px) —
    // exactly the squashed band, and the 1/4-inch globe, Aaron saw.
    let mut inner = UiNode { template: Some(FRAME.into()), ..Default::default() };
    inner.grow = Some(1.0);
    inner.props.insert("runes".into(), Value::Bool(false));
    inner.slots.insert("center".into(), vec![content(page, &page.tabs[sel_tab])]);

    let mut menu = UiNode { template: Some(PAGED_MENU.into()), ..Default::default() };
    menu.slots.insert(
        "pages".into(),
        PAGES.iter().enumerate().map(|(i, p)| rail_entry(i, p.label)).collect(),
    );
    menu.slots.insert(
        "tabs".into(),
        page.tabs.iter().enumerate().map(|(i, t)| rail_entry(i, t.label)).collect(),
    );
    menu.slots.insert("content".into(), vec![inner]);

    let mut screen =
        UiNode { template: Some(DEFAULT_PAGE.into()), id: "populous".into(), ..Default::default() };
    // The rail intents ride the proto's optional params onto the screen root —
    // declared here because THIS page dispatches them (the bare default page
    // deliberately declares none of these). NOTHING ELSE is declared: Confirm,
    // Cancel, `Nav*` and `Panel*` mean the same thing on every screen in Prism
    // and the walker answers all four, so a screen that named one would kill it
    // on itself (violation F1, 2026-08-09).
    for (signal, result) in [
        ("on_page_next", "page_next"),
        ("on_page_prev", "page_prev"),
        ("on_tab_next", "tab_next"),
        ("on_tab_prev", "tab_prev"),
    ] {
        screen.props.insert(signal.into(), Value::Text(result.into()));
    }
    screen.slots.insert("content".into(), vec![menu]);
    flicker::ui::expand(screen, templates)
}
