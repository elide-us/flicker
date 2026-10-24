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
//!
//! **A field is a frequency across the whole planet and an alpha mask — that is
//! it** (Aaron 2026-10-15). The RESONANCE tab is a list of slots, each a
//! frequency (editable), an ON switch and a MASK switch; the stack's relief,
//! tilt, wave count and mask seed are its dials.

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

// ── THE WAVE (tab 0) — the frequency-1 field on the pentagons ──
/// The SEA dial — percent, `0..=100` (0 = the lowest tile is the waterline,
/// 98 = the highest, the last two percent flood the planet).
pub const SEA_BIND: &str = "wb_sea";
/// RELIEF — the wave's amplitude as percent of the planet radius.
pub const AMP_BIND: &str = "wb_amp";
/// LOBE — the pentagon domes' sharpness, ×10.
pub const LOBE_BIND: &str = "wb_lobe";
/// MASK SEED — a scan through the wave's alpha masks.
pub const MASK_SEED_BIND: &str = "wb_mask_seed";
/// The wave's re-roll.
pub const RANDOMIZE_ACTION: &str = "wb_randomize";

// ── THE RESONANCE STACK (tab 1) ──
/// How many slots the stack offers.
pub const RESONANCE_SLOTS: usize = 12;
/// SPREAD — how far a slot's heard amplitude may stray from its dial, octaves ×10.
pub const RES_SPREAD_BIND: &str = "wb_res_spread";
/// SEAM — the seam ribbon's half-width, rings from a converging plate edge.
pub const RES_SEAM_BIND: &str = "wb_res_seam";
/// MASK SEED — a scan through every slot's alpha mask at once.
pub const RES_MASK_SEED_BIND: &str = "wb_res_mask_seed";
/// The stack's re-roll (every slot's axes, phases and amplitude).
pub const RES_REROLL_ACTION: &str = "wb_res_reroll";
/// A slot's ON switch, frequency, amplitude and wave-count steppers and MASK pill binds
/// (the pill's value is the `MaskKind` index: 0 off, 1 region, 2 seam).
pub fn slot_on_bind(i: usize) -> String {
    format!("wb_r_on_{i}")
}
pub fn slot_freq_bind(i: usize) -> String {
    format!("wb_r_freq_{i}")
}
pub fn slot_amp_bind(i: usize) -> String {
    format!("wb_r_amp_{i}")
}
pub fn slot_axes_bind(i: usize) -> String {
    format!("wb_r_axes_{i}")
}
pub fn slot_mask_bind(i: usize) -> String {
    format!("wb_r_mask_{i}")
}
/// The frequency stepper's range, cycles per great circle — to the finest
/// layer under the hex (a half-mile hummock); the grid carries what it
/// resolves, the 2K hex the rest.
pub const FREQ_RANGE: (u32, u32) = (1, 65536);
/// The resonance tab's readouts: active slot count, the stack's roll.
pub const RES_ACTIVE_BIND: &str = "wb_res_active";
pub const RES_SEED_BIND: &str = "wb_res_seed";

// ── THE HEAT (tab 2) and its EMITTERS (tab 3) ──
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

// ── THE EPOCHS (tab 4) — the stone ocean moving: rafts, floor, compression ──
/// RATE — how far the fastest plate turns per tick, hundredths of a tile.
pub const EPOCH_RATE_BIND: &str = "wb_ep_rate";
/// UPLIFT — the isostatic share of crust gone under that stands up, tenths (2 = a fifth).
pub const EPOCH_UPLIFT_BIND: &str = "wb_ep_uplift";
/// RIFT — the tension at which a raft splits, tenths of the rate.
pub const EPOCH_RIFT_BIND: &str = "wb_ep_rift";
/// IMPACTS — strikes a thousand epochs, their radii a power law from two
/// tiles to half a radian (most small, a few vast); each a crater wave on
/// the crust, shocked hard where it lands.
pub const EPOCH_IMPACTS_BIND: &str = "wb_ep_impacts";
/// One epoch; RUN the count in the box (or pause a run); back to the composed
/// world.
pub const EPOCH_TICK_ACTION: &str = "wb_ep_tick";
pub const EPOCH_RUN_ACTION: &str = "wb_ep_run";
pub const EPOCH_RESET_ACTION: &str = "wb_ep_reset";
/// The RUN count box — digits, remembered as typed; anything unparseable
/// falls back to [`EPOCH_RUN_DEFAULT`] on use.
pub const EPOCH_COUNT_BIND: &str = "wb_ep_count";
pub const EPOCH_RUN_DEFAULT: u32 = 1000;
/// VIEW — what the globe shows on the epochs tab: the RELIEF (0) or the
/// HARDNESS ledger (1), the pill's value an index.
pub const EPOCH_VIEW_BIND: &str = "wb_ep_view";
pub const EPOCH_VIEW_RELIEF: u32 = 0;
pub const EPOCH_VIEW_HARDNESS: u32 = 1;
/// The epoch readouts: ticks, rafts, floor consumed · young floor opened, uplift.
pub const EPOCH_TICKS_BIND: &str = "wb_ep_ticks";
pub const EPOCH_RAFTS_BIND: &str = "wb_ep_rafts";
pub const EPOCH_CRUST_BIND: &str = "wb_ep_crust";
pub const EPOCH_UPLIFT_VAL_BIND: &str = "wb_ep_uplift_val";
/// The EVENTS landed on the crust so far: eruptions · impacts.
pub const EPOCH_EVENTS_BIND: &str = "wb_ep_events";

// ── THE EROSION (tab 5) — the era over the wave planet: water, air, the cut ──
/// One surface cycle; RUN the count in the box (or pause a run); back to the
/// world the era adopted.
pub const ERA_TICK_ACTION: &str = "wb_er_tick";
pub const ERA_RUN_ACTION: &str = "wb_er_run";
pub const ERA_RESET_ACTION: &str = "wb_er_reset";
/// The RUN count box — digits, remembered as typed; anything unparseable
/// falls back to [`ERA_RUN_DEFAULT`] on use.
pub const ERA_COUNT_BIND: &str = "wb_er_count";
pub const ERA_RUN_DEFAULT: u32 = 200;
/// The era readouts: cycles run, the resolved sea against the adopted sea
/// (percent of radius), the land's texture against the adopted relief's.
pub const ERA_TICKS_BIND: &str = "wb_er_ticks";
pub const ERA_SEA_BIND: &str = "wb_er_sea";
pub const ERA_TEXTURE_BIND: &str = "wb_er_texture";
/// THE FOREVER LOOP — write the era's cut back into the crust's parcels,
/// move `LOOP_EPOCHS` epochs, adopt again, cut `LOOP_CYCLES` cycles; the
/// box counts loop steps, LOOP plays them across frames (LOOP again
/// pauses); anything unparseable falls back to [`LOOP_RUN_DEFAULT`].
pub const LOOP_RUN_ACTION: &str = "wb_loop_run";
pub const LOOP_COUNT_BIND: &str = "wb_loop_count";
pub const LOOP_RUN_DEFAULT: u32 = 20;
/// Loop steps done (+ owed, and where the live step stands).
pub const ERA_LOOPS_BIND: &str = "wb_er_loops";

// ── THE TILE (tab 6) — the hex the camera faces, at 2K ──
/// CUT — the passes of rain the pixel eroder runs over the materialized hex
/// (by the hex's hardness, nothing under the sea line), as typed; empty or
/// unparseable reads [`CUT_DEFAULT`], `0` leaves the ground as the fields
/// made it.
pub const CUT_COUNT_BIND: &str = "wb_cut_count";
pub const CUT_DEFAULT: u32 = 50;
/// The thread hands a picture back every this many passes, so the water is
/// watched working rather than waited for.
pub const CUT_PICTURE_EVERY: u32 = 10;
/// MATERIALIZE the facing hex through the pixel tier (on its own thread).
pub const TILE_MAKE_ACTION: &str = "wb_tile_make";
/// What the centre pane shows: the globe, or the hex's 2K picture once one
/// is materialized and the TILE tab is selected.
pub const GLOBE_SHOWN: &str = "wb_globe_shown";
pub const TILE_SHOWN: &str = "wb_tile_shown";
/// The HUD texture index the tree's sprite is authored with — the slot after
/// the theme's three (`white`, `muse`, `pad_glyphs`); the scene appends the
/// hex picture there. The static-tree gate pins the sprite's `tex` to this.
pub const TILE_TEXTURE: u32 = 3;
/// The sprite's contain-fit reads the image's size from these model keys.
pub const IMG_W_BIND: &str = "img_w";
pub const IMG_H_BIND: &str = "img_h";
/// VIEW — what the centre pane shows of a materialized hex on the TILE tab:
/// its PICTURE (0, the 2K map hill-shaded, a sprite) or its FIELD (1, the
/// hex as the engine's field of vectors: one LOD8 vector per pixel, the
/// surface contoured between them, flown over with the orbit camera), the
/// pill's value an index. The field shows once built; until
/// then the picture stands in.
pub const TILE_VIEW_BIND: &str = "wb_tile_view";
pub const TILE_VIEW_PICTURE: u32 = 0;
pub const TILE_VIEW_FIELD: u32 = 1;
/// The field's `surface` node and its authored stage; what the pane shows.
pub const FIELD_SLOT: &str = "wb_field_rtt";
pub const FIELD_STAGE_SOURCE: &str = "worldbench_field";
pub const FIELD_SHOWN: &str = "wb_field_shown";
/// Clusters a region mesh spans an axis: a tile at 2048 is 64 meshes of
/// 65 536 vectors, each a draw.
pub const FIELD_REGION: u32 = 256;
/// The field's readout: vectors · quads.
pub const TILE_QUADS_BIND: &str = "wb_tile_quads";
/// The tile readouts: the hex (facing, and the one materialized), its column
/// height in metres, the pixels' span and their roughness.
pub const TILE_HEX_BIND: &str = "wb_tile_hex";
pub const TILE_COLUMN_BIND: &str = "wb_tile_column";
pub const TILE_SPAN_BIND: &str = "wb_tile_span";
pub const TILE_ROUGH_BIND: &str = "wb_tile_rough";
/// The cut's readout: passes run · megatonnes carried out across the rim.
pub const TILE_CUT_BIND: &str = "wb_tile_cut";

// ── THE FIELDS (the tree's switches) and each tab's enabled key ──
pub const FIELD_WAVE_BIND: &str = "wb_field_wave";
pub const FIELD_MASK_BIND: &str = "wb_field_mask";
pub const FIELD_RES_BIND: &str = "wb_field_res";
pub const FIELD_HEAT_BIND: &str = "wb_field_heat";
pub const FIELD_EMIT_BIND: &str = "wb_field_emit";
/// BUBBLES ON BUBBLES — domes and bowls at every scale (the octaves the hex
/// grid resolves on the globe, every octave at the tile).
pub const FIELD_BUBBLES_BIND: &str = "wb_field_bubbles";
pub const FIELD_EPOCHS_BIND: &str = "wb_field_epochs";
pub const FIELD_EROSION_BIND: &str = "wb_field_erosion";
pub const TAB_WAVE_ENABLED: &str = "wb_tab_wave_on";
pub const TAB_RES_ENABLED: &str = "wb_tab_res_on";
pub const TAB_HEAT_ENABLED: &str = "wb_tab_heat_on";
pub const TAB_EMIT_ENABLED: &str = "wb_tab_emit_on";
pub const TAB_EPOCHS_ENABLED: &str = "wb_tab_epochs_on";
pub const TAB_EROSION_ENABLED: &str = "wb_tab_erosion_on";
pub const TAB_TILE_ENABLED: &str = "wb_tab_tile_on";

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

/// **The bench's page/tab roster.** The WORLD page, with one tab per field
/// that carries controls: the WAVE, the RESONANCE stack, the HEAT and its
/// EMITTERS, the EPOCHS that move and compress the whole, the EROSION — the
/// era's water, air and weathering over the epoched planet — and the TILE:
/// one hex of all that at 2K, the ground underfoot. Then the MAP page: the
/// world laid flat, zoomable to a hex, where a picked hex's 2K heightmap is
/// drawn IN THE WORLD and RENDER WORLD walks every hex (Aaron 2026-10-16).
pub static PAGES: &[Page] = &[
    Page {
        id: "world",
        label: "$wb_page_world",
        tabs: &[
            Tab {
                id: "wave",
                label: "$wb_tab_wave",
            },
            Tab {
                id: "resonance",
                label: "$wb_tab_res",
            },
            Tab {
                id: "heat",
                label: "$wb_tab_heat",
            },
            Tab {
                id: "emitters",
                label: "$wb_tab_emit",
            },
            Tab {
                id: "epochs",
                label: "$wb_tab_epochs",
            },
            Tab {
                id: "erosion",
                label: "$wb_tab_erosion",
            },
            Tab {
                id: "tile",
                label: "$wb_tab_tile",
            },
        ],
    },
    Page {
        id: "map",
        label: "$wb_page_map",
        tabs: &[],
    },
];

/// The page index each page stands at.
pub const PAGE_WORLD: usize = 0;
pub const PAGE_MAP: usize = 1;

// ── THE MAP (page 1) — the world laid flat, the hexes drawn in it ──
/// The map's surface node and its authored stage.
pub const MAP_SLOT: &str = "wb_map_rtt";
pub const MAP_STAGE_SOURCE: &str = "worldbench_map";
/// How far the map zooms in, in hex spacings across the view: one hex and
/// a half, so a picked hex's 2K picture fills the sheet.
pub const MAP_ZOOM_FLOOR_CELLS: f32 = 1.5;
/// THE MATERIALIZER's controls (the UX law: a click PICKS, a control does
/// what it says). RENDER HEX materializes the picked hex, one job, ahead of
/// everything; RENDER WORLD walks every hex, nearest the picked one first,
/// drawing each in the world as it lands; STOP halts the materializer — the
/// hex in flight is dropped back on the queue, the walk pauses (RENDER WORLD
/// resumes it).
pub const RENDER_HEX_ACTION: &str = "wb_render_hex";
pub const RENDER_WORLD_ACTION: &str = "wb_render_world";
pub const STOP_ACTION: &str = "wb_stop";
/// Which controls stand: the materializer running (STOP), idle (RENDER
/// WORLD), idle with a hex picked (RENDER HEX).
pub const MAT_RUNNING_BIND: &str = "wb_mat_running";
pub const MAT_IDLE_BIND: &str = "wb_mat_idle";
pub const HEX_READY_BIND: &str = "wb_hex_ready";
/// The hex in flight — which, and the passes run of its cut — and the same
/// as the share its gauge fills.
pub const MAP_HEX_BIND: &str = "wb_map_hex";
pub const MAP_HEX_PROGRESS_BIND: &str = "wb_map_hex_progress";
/// How many hex pictures stay resident on the sheet: a pool of textures
/// reused oldest-first (the whole world is 92K of them — the ultimate render
/// is a job and a cache, not a pool).
pub const MAP_PICTURES_RESIDENT: usize = 512;
/// How many cached pictures the loader is asked for a frame as the view
/// moves over the sheet (a PNG decodes in milliseconds on its own thread).
pub const MAP_LOADS_PER_FRAME: usize = 6;
/// The map readouts: the picked hex, the pictures cached of the world's
/// hexes, the render job (done / owed and the time it will take).
pub const MAP_PICK_BIND: &str = "wb_map_pick";
pub const MAP_TILES_BIND: &str = "wb_map_tiles";
pub const MAP_JOB_BIND: &str = "wb_map_job";

/// The tab index each field's tab stands at — the roster's own order.
pub const TAB_WAVE: usize = 0;
pub const TAB_RES: usize = 1;
pub const TAB_HEAT: usize = 2;
pub const TAB_EMIT: usize = 3;
pub const TAB_EPOCHS: usize = 4;
pub const TAB_EROSION: usize = 5;
pub const TAB_TILE: usize = 6;

/// The page at `sel`, clamped.
pub fn page(sel: usize) -> &'static Page {
    &PAGES[sel.min(PAGES.len() - 1)]
}
