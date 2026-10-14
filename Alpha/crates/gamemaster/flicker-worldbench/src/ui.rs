//! **The bench's roster, as data.**
//!
//! What the surface CONTAINS — its pages and tabs — as a small data roster the
//! scene reads for COUNTS and dispatch, plus the stable id / bind / action names
//! shared by the static component tree (`worldbench.scene.json`), the Model, and
//! the dispatcher so the three cannot drift apart. The tree itself is authored
//! as data; `worldbench.lua`'s `arrange()` lights its per-tab slices.
//!
//! The bench's state IS a [`flicker_worldengine::Recipe`]: every dial here is one
//! of the recipe's numbers, and the scene composes through the engine's one
//! `compose` — the headless evaluator (`resonance_eval`) and the window cannot
//! disagree.

/// The three panes' ids — each is ALSO its `tab_group` in the static tree.
pub const LEFT_PANE: &str = "wb_left";
pub const VIEW_PANE: &str = "wb_view";
pub const RIGHT_PANE: &str = "wb_right";
/// The viewport node — the `surface` whose rect the walker reserves.
pub const VIEW_SLOT: &str = "wb_view_rtt";
/// The `stages.<source>` block the globe is authored by.
pub const STAGE_SOURCE: &str = "worldbench_globe";
/// The nav footer's panel id.
pub const NAV_PANE: &str = "wb_nav";

// ── THE WAVE (tab 0) ──
/// The SEA dial — percent, `0..=100` (0 = the lowest tile is the waterline,
/// 98 = the highest, the last two percent flood the planet).
pub const SEA_BIND: &str = "wb_sea";
/// RELIEF — the wave's amplitude as percent of the planet radius.
pub const AMP_BIND: &str = "wb_amp";
/// LOBE — the pentagon domes' sharpness, ×10.
pub const LOBE_BIND: &str = "wb_lobe";
/// PLATEAU — the wave's shaping exponent, ×10 (10 = domes as rolled; lower
/// = flat-topped continents with steep flanks).
pub const PLATEAU_BIND: &str = "wb_plateau";
/// MASK SEED — a scan through the wave's alpha masks.
pub const MASK_SEED_BIND: &str = "wb_mask_seed";
/// The wave's re-roll.
pub const RANDOMIZE_ACTION: &str = "wb_randomize";

// ── THE RIDGE (tab 1) — the wave's child: a band on the wave's own contour ──
/// LEVEL — the wave value (×100, signed) the ridge crest follows.
pub const RIDGE_LEVEL_BIND: &str = "wb_ridge_level";
/// WIDTH — the band's half-width in wave value, ×100.
pub const RIDGE_WIDTH_BIND: &str = "wb_ridge_width";
/// HEIGHT — the ridge's height, half-percents of the radius.
pub const RIDGE_HEIGHT_BIND: &str = "wb_ridge_height";

// ── THE COAST BAND (tab 5) — an altitude belt over the composed relief ──
/// LEVEL — height above the sea the crest follows, thousandths of the radius.
pub const COAST_LEVEL_BIND: &str = "wb_coast_level";
/// WIDTH — the belt's half-width, thousandths of the radius.
pub const COAST_WIDTH_BIND: &str = "wb_coast_width";
/// HEIGHT — the band's height, thousandths of the radius.
pub const COAST_HEIGHT_BIND: &str = "wb_coast_height";

// ── THE HEAT (tab 6) and its EMITTERS (tab 7) ──
pub const CELLS_BIND: &str = "wb_cells";
pub const SPOTS_BIND: &str = "wb_spots";
pub const SEAMS_ACTION: &str = "wb_randomize_seams";
pub const CELLS_VAL_BIND: &str = "wb_cells_val";
pub const SPOTS_VAL_BIND: &str = "wb_spots_val";
pub const SEAMS_SEED_BIND: &str = "wb_seams_seed";
pub const RADIUS_BIND: &str = "wb_radius";
pub const ARMS_BIND: &str = "wb_arms";
pub const RINGS_BIND: &str = "wb_rings";
pub const HEIGHT_BIND: &str = "wb_height";
pub const EMITTERS_VAL_BIND: &str = "wb_emitters_val";

// ── THE FIELDS (the tree's switches) and each tab's enabled key ──
pub const FIELD_WAVE_BIND: &str = "wb_field_wave";
pub const FIELD_MASK_BIND: &str = "wb_field_mask";
pub const FIELD_RIDGE_BIND: &str = "wb_field_ridge";
pub const FIELD_COAST_BIND: &str = "wb_field_coast";
pub const FIELD_HEAT_BIND: &str = "wb_field_heat";
pub const FIELD_EMIT_BIND: &str = "wb_field_emit";
pub const TAB_WAVE_ENABLED: &str = "wb_tab_wave_on";
pub const TAB_RIDGE_ENABLED: &str = "wb_tab_ridge_on";
pub const TAB_COAST_ENABLED: &str = "wb_tab_coast_on";
pub const TAB_HEAT_ENABLED: &str = "wb_tab_heat_on";
pub const TAB_EMIT_ENABLED: &str = "wb_tab_emit_on";

// ── THE READOUTS ──
pub const HEXES_BIND: &str = "wb_hexes";
pub const LAND_BIND: &str = "wb_land";
pub const LAND_VAL_BIND: &str = "wb_land_val";
pub const WATER_VAL_BIND: &str = "wb_water_val";
pub const SEA_VAL_BIND: &str = "wb_sea_val";
pub const CRESTS_BIND: &str = "wb_crests";
pub const SEED_BIND: &str = "wb_seed";

/// Model keys the rails bind to.
pub const PAGE_BIND: &str = "page";
pub const TAB_BIND: &str = "tab";
pub const TABS_SHOWN: &str = "paged_tabs_shown";

/// **A harmonic's descriptor** — one per resonance field of the planetary wave
/// at another frequency ratio; each has its own alpha mask child, its own tab.
pub struct Harmonic {
    pub tab: usize,
    /// The opening ratio to the base wave (the RATIO dial moves it, ×100).
    pub ratio: f32,
    pub field_bind: &'static str,
    pub mask_bind: &'static str,
    pub tab_enabled: &'static str,
    pub amp_bind: &'static str,
    pub ratio_bind: &'static str,
    pub axes_bind: &'static str,
    pub mask_seed_bind: &'static str,
    pub reroll_action: &'static str,
    pub ratio_val_bind: &'static str,
    pub seed_bind: &'static str,
    /// The relief dial's (max, default), tenths of a percent of radius.
    pub amp_range: (u32, u32),
}

pub static HARMONICS: [Harmonic; 3] = [
    Harmonic {
        tab: 2,
        ratio: 8.0 / 9.0,
        field_bind: "wb_field_w89",
        mask_bind: "wb_mask_w89",
        tab_enabled: "wb_tab_w89_on",
        amp_bind: "wb_amp_w89",
        ratio_bind: "wb_ratio_w89",
        axes_bind: "wb_axes_w89",
        mask_seed_bind: "wb_mseed_w89",
        reroll_action: "wb_reroll_w89",
        ratio_val_bind: "wb_ratioval_w89",
        seed_bind: "wb_seed_w89",
        amp_range: (60, 20),
    },
    Harmonic {
        tab: 3,
        ratio: 13.0,
        field_bind: "wb_field_w13",
        mask_bind: "wb_mask_w13",
        tab_enabled: "wb_tab_w13_on",
        amp_bind: "wb_amp_w13",
        ratio_bind: "wb_ratio_w13",
        axes_bind: "wb_axes_w13",
        mask_seed_bind: "wb_mseed_w13",
        reroll_action: "wb_reroll_w13",
        ratio_val_bind: "wb_ratioval_w13",
        seed_bind: "wb_seed_w13",
        amp_range: (20, 6),
    },
    Harmonic {
        tab: 4,
        ratio: 15.0,
        field_bind: "wb_field_w15",
        mask_bind: "wb_mask_w15",
        tab_enabled: "wb_tab_w15_on",
        amp_bind: "wb_amp_w15",
        ratio_bind: "wb_ratio_w15",
        axes_bind: "wb_axes_w15",
        mask_seed_bind: "wb_mseed_w15",
        reroll_action: "wb_reroll_w15",
        ratio_val_bind: "wb_ratioval_w15",
        seed_bind: "wb_seed_w15",
        amp_range: (20, 5),
    },
];

/// One section tab within a page.
pub struct Tab {
    pub id: &'static str,
    /// Stringtable token — never raw English.
    pub label: &'static str,
}

/// One top-level page.
pub struct Page {
    pub id: &'static str,
    pub label: &'static str,
    pub tabs: &'static [Tab],
}

/// **The bench's page/tab roster.** One page — the WORLD — with one tab per
/// field that carries controls: the WAVE, its RIDGE, the three harmonics, the
/// COAST band, the HEAT and its EMITTERS.
pub static PAGES: &[Page] = &[Page {
    id: "world",
    label: "$wb_page_world",
    tabs: &[
        Tab {
            id: "wave",
            label: "$wb_tab_wave",
        },
        Tab {
            id: "ridge",
            label: "$wb_tab_ridge",
        },
        Tab {
            id: "w89",
            label: "$wb_tab_w89",
        },
        Tab {
            id: "w13",
            label: "$wb_tab_w13",
        },
        Tab {
            id: "w15",
            label: "$wb_tab_w15",
        },
        Tab {
            id: "coast",
            label: "$wb_tab_coast",
        },
        Tab {
            id: "heat",
            label: "$wb_tab_heat",
        },
        Tab {
            id: "emitters",
            label: "$wb_tab_emit",
        },
    ],
}];

/// The tab index each field's tab stands at — the roster's own order (the
/// harmonics' are in [`HARMONICS`]).
pub const TAB_WAVE: usize = 0;
pub const TAB_RIDGE: usize = 1;
pub const TAB_COAST: usize = 5;
pub const TAB_HEAT: usize = 6;
pub const TAB_EMIT: usize = 7;

/// The page at `sel`, clamped.
pub fn page(sel: usize) -> &'static Page {
    &PAGES[sel.min(PAGES.len() - 1)]
}
