//! **The bench's roster, as data.**
//!
//! What the surface CONTAINS — its pages and tabs — as a small data roster the
//! scene reads for COUNTS and dispatch, plus the stable id / bind / action names
//! shared by the static component tree (`populous.scene.json`), the Model, and the
//! dispatcher so the three cannot drift apart.
//!
//! This module no longer BUILDS the tree. The surface is authored as data in
//! `populous.scene.json` (a root `stack` → `paged_menu` → a `row` of the three
//! panes) and its per-tab slices are lit by `populous.lua`'s
//! `arrange()`. The former `ui::build` Rust tree builder — and its `Holds` /
//! `PaneRow` pane vocabulary and template-name constants — were retired once that
//! static tree landed and was verified in-window; only the DATA the scene still
//! reads lives on.

/// The three panes' ids — each is ALSO its `tab_group` in the static tree, which is
/// what makes it a panel for the walker's panel cursor (the left stick cycles these).
/// ONE three-pane arrangement, shared by every page and tab: a page owns its VIEW —
/// the gated slices inside each pane — never a second set of panes.
pub const LEFT_PANE: &str = "pop_left";
pub const VIEW_PANE: &str = "pop_view";
pub const RIGHT_PANE: &str = "pop_right";
/// The viewport node — the `rtt` whose rect the walker reserves and the scene fills
/// with the globe (`VIEW_PANE` + the `_rtt` slot-id suffix).
pub const VIEW_SLOT: &str = "pop_view_rtt";
/// The `stages.<source>` block the globe is authored by — the default light source
/// and backdrop the world is seen under.
pub const STAGE_SOURCE: &str = "populous_globe";
/// The HEX page's viewport node — the centre pane's second gated `surface`, where
/// the centre cell's column stack stands (molten below, bedrock above, more to come).
pub const HEX_SLOT: &str = "pop_hex_rtt";
/// The stage the hex-stack view is lit by. Authors NO shells and no graticule:
/// the column is published data, alone under the stage's light.
pub const HEX_STAGE_SOURCE: &str = "populous_hex";

/// The size dial's two-way bind — the frequency the map is rebuilt at. Stated once
/// so the tree, the Model and the dispatcher cannot drift apart.
pub const FREQ_BIND: &str = "pop_freq";
/// The three readout binds: the scene publishes a PRE-FORMATTED string on each and
/// the `stat_row` proto shows it. A number never reaches a node.
pub const HEXES_BIND: &str = "pop_hexes";
pub const DIAMETER_BIND: &str = "pop_diameter";
pub const TILE_BIND: &str = "pop_tile";
/// The seams tab's re-roll action: a new random set of convection cells.
pub const SEAMS_ACTION: &str = "pop_seams_randomize";
/// The seams tab's cell-count dial — how many convection cells the molten heat
/// field is rolled with (two-way bind, like the size dial).
pub const CELLS_BIND: &str = "pop_cells";
/// The seams tab's hot-spot dial — how many mantle plumes the field rolls.
pub const SPOTS_BIND: &str = "pop_spots";
/// The evolve tab's three actions: run/pause the era, advance one tick, and
/// reset to the bare shell.
pub const EVOLVE_RUN_ACTION: &str = "pop_evolve_run";
pub const EVOLVE_STEP_ACTION: &str = "pop_evolve_step";
pub const EVOLVE_RESET_ACTION: &str = "pop_evolve_reset";
/// The fast-roll button: another BOOTSTRAP_TICKS-sized leap without baking —
/// the "what does 4500 look like?" control.
pub const EVOLVE_ROLL_ACTION: &str = "pop_evolve_roll";
/// The evolve tab's readouts: ticks run, and layers formed — pre-formatted
/// strings the scene publishes.
pub const TICKS_BIND: &str = "pop_ticks";
/// The evolve tab's PROCEDURE label — which pipeline phase the next engine
/// step runs; the tick counts once per completed cycle of them.
pub const PHASE_BIND: &str = "pop_phase";
/// The evolve tab's water-coverage dial — percent of the surface flooded.
pub const WATER_BIND: &str = "pop_water";
/// The climate gauge — the ice-age runner's live temperature, published every
/// frame (the knob MOVES with the glacials); a user write sets the baseline.
pub const TEMP_BIND: &str = "pop_temp";
/// The water TARGET dial — the coverage share the in-fall pursues. Its
/// sibling `WATER_BIND` is the live gauge beside it (display, never a
/// control).
pub const WATER_TARGET_BIND: &str = "pop_water_target";
/// The material census TABLE — a fixed roster of two-column rows (material |
/// hex count), most-common first; unused rows publish empty strings and
/// vanish. The last row overflows as "+K" with the remaining hexes summed.
pub const CENSUS_ROWS: usize = 18;
/// The row binds: `pop_census_n{i}` (the material label) and
/// `pop_census_c{i}` (its count), authored in the tree for every row index.
pub fn census_name_bind(i: usize) -> String {
    format!("pop_census_n{i}")
}
pub fn census_count_bind(i: usize) -> String {
    format!("pop_census_c{i}")
}
/// The evolve tab's motion-arrows checkbox — show each plate's Euler velocity.
pub const ARROWS_BIND: &str = "pop_arrows";
pub const STRATA_BIND: &str = "pop_strata";

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

/// **The bench's page/tab roster.** Two pages — the WORLD (the globe, with its
/// MAP, SEAMS, CRUST and EVOLVE tabs) and the HEX stack (the centre cell's column
/// inspected up close, layer by layer). The scene reads only COUNTS from this (the
/// rail bounds + the dispatch clamp); each tab's actual panes are authored in
/// `populous.scene.json` and gated by `arrange()`. The code that reads this
/// never learns the counts, so the next page or tab costs exactly one row here.
pub static PAGES: &[Page] = &[
    Page {
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
            Tab {
                id: "crust",
                label: "$pop_tab_crust",
            },
            Tab {
                id: "evolve",
                label: "$pop_tab_evolve",
            },
        ],
    },
    Page {
        id: "hex",
        label: "$pop_page_hex",
        tabs: &[Tab {
            id: "stack",
            label: "$pop_tab_stack",
        }],
    },
];

/// The page at `sel`, clamped — the roster is the authority on what exists, so an
/// out-of-range selection reads as the last page rather than panicking.
pub fn page(sel: usize) -> &'static Page {
    &PAGES[sel.min(PAGES.len() - 1)]
}
