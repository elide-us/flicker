//! The bench: the DEFAULT PAGE — an empty frame, and the pause-menu signal.
//!
//! # The UI is DATA
//!
//! Following the Quartermaster (rule E5AFBBAB): this scene owns no HUD Lua and
//! composes nothing. [`crate::ui`] instances the `default_page` proto and calls
//! `expand` at its end — the single seam, so the scene and every gate walk the
//! same tree.
//!
//! # Five intents, one channel — and NOTHING the walker owns
//!
//! The screen's whole input declaration rides in data: `on_menu → pause_open`
//! from the `default_page` proto itself, plus the four rail intents
//! (`page_next` / `page_prev` / `tab_next` / `tab_prev`) through its optional
//! params. That is the entire list. Confirm, Cancel, `Nav*` and `Panel*` are
//! the WALKER's on every screen in Prism — it activates the focused control,
//! backs out, moves the cursor and cycles the panels — and this bench once
//! declared four of them, which statically killed activation on itself
//! (violation F1, 2026-08-09).
//!
//! A pad press, the bound key (`[` `]` pages, `,` `.` tabs) and a click are the
//! same event by the time the walker fires the result name; `update` folds them
//! into ONE dispatch. The rails then step THEMSELVES — each strip carries its
//! own `next_action`/`prev_action` — so `apply_results` only ever reads the
//! resulting numeric index, and the pause arm pushes the shell overlay.
//!
//! # Two tabs of one world
//!
//! The centre pane hands the bench's data core ([`flicker_worldengine::HexMap`], the hex
//! ledger's tiling) to ONE component — [`flicker_globe::GlobeWorld`] — which
//! owns the meshes, the offscreen target, the authored stage and the camera. The
//! scene names no colour, no radius and no inset: what the world LOOKS like is
//! authored in `stages.populous_globe` (the near-black under-shell, the inset
//! tile shell whose seams show it as outlines, and the shared reference frame
//! over both). No simulation, no layers, no data meaning: the sphere itself,
//! under the stage's authored light. The MAP tab flanks it with
//! the size dial and the world's numbers; the SEAMS tab (its view arrives
//! next) shows the SAME world between panes resting on their default text.
//! One viewport id, one render path — a tab swaps only its panes, and each
//! interior arrives as a layer that docks in, never into the chrome.
//!
//! **The world itself is SHARED state (Aaron's ruling).** One [`HexMap`], one
//! size, held once on this scene — a control that writes it changes it for
//! every tab, even though the control is shown on only one. The size dial is
//! currently the ONLY such control. A tab owns its VIEW — panes, captions,
//! someday its own overlays — never a copy of the world; the cross-tab gate
//! below is what keeps a future refactor from quietly forking it per tab.

use std::time::Duration;

use flicker::render::{FrameGraph, MeshDrawOptions, Renderer, TextureHandle, Vec3};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, Plot, PlotKind, PlotSeries, PlotStyle, SceneDef, UiInput, UiIntents,
    UiState, WalkerHandler,
};
use flicker_globe::{
    column_frame, lerp3, stippled, temp_color, tile_width, water_temp_color, Arrows, GlobeWorld,
    HexSphereMap, ShellSpec, WorldMap, RADIUS,
};
use flicker_input_core::{
    AbstractControls, ActionSignal, GamepadConfig, InputContext, InputMap, InputState,
};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_worldengine::{
    CrustField, Evolution, HexMap, SeamField, TileId, DEFAULT_CELLS, DEFAULT_FREQ, DEFAULT_SPOTS,
    MAX_FREQ, MIN_FREQ,
};

use crate::ui;

/// The hex-stack view's opening framing — the framed region fills this share of
/// the viewport, which puts the stack's columns at about a tenth of the panel's
/// width: small at the bottom, with the room the ~50-cell stack above will need.
const HEX_FILL: f32 = 0.85;
/// How many tile-widths across the hex view frames. Five tiles across ⇒ the
/// column is ~1/10 of the viewport at the opening fill.
const HEX_FRAME_TILES: f32 = 5.0;
/// The evolve view's motion-arrow ink — one ink for the one field.
const MOTION_INK: [f32; 4] = [0.78, 0.62, 0.36, 1.0];
/// The centre-cell reticle's ink — the bold outline on the cell the camera
/// faces. Instrument ink like the graticule's, not UI chrome.
const RETICLE_INK: [f32; 4] = [1.0, 0.85, 0.30, 1.0];
/// The reticle's two rings' radii, as radius scales: both above the tile shell
/// (1.0) and below the graticule (1.022), doubled because two strokes read BOLD
/// where one reads like a stray grid line.
const RETICLE_RINGS: [f32; 2] = [1.008, 1.016];
/// The reticle RIDES its column (Aaron 2026-08-27: the highlight was getting
/// buried once the era grew real mountains): the rings lift by the cell's own
/// ground height plus this many tile-widths of clear margin, and corner POSTS
/// run from the column top up to the rings so the mark reads from low angles
/// and out of valleys.
const RETICLE_LIFT: f32 = 0.6;
/// A vein body's FIELD OUTLINE (Aaron 2026-08-27: "a field outline around a
/// cluster of rubies or coal or calcium… making the node easier to see"):
/// the boundary ring drawn in the kind's own ink, this many tile-widths
/// above each boundary cell's own top.
const VEIN_RING_LIFT: f32 = 0.35;
// ── THE RIVERS (Aaron 2026-08-27: "highlighted in green water, to
// distinguish from ocean blue") — every LAND tile carrying a live channel
// wears a thin translucent green film on its column top; the ramp deepens
// with the catchment so trunks read stronger than headwaters. ──
const RIVER_COLOR: [f32; 3] = [0.24, 0.66, 0.32];
const RIVER_DEEP_COLOR: [f32; 3] = [0.08, 0.42, 0.22];
/// The film's thickness in tile-widths — a skin of water, not a cell.
const RIVER_FILM: f32 = 0.1;
const RIVER_ALPHA: f32 = 0.6;
/// Discharge at which the river ink saturates to its deep end, as a
/// multiple of the channel-live floor.
const RIVER_FULL: f32 = 8.0;

// ── the stack's provisional layer proportions (per cell width `w`) ──
/// The molten cell's height (Aaron 2026-08-25: "significantly shorter,
/// 1/6th height perhaps" — the as-tall-as-wide first cut read as too thick).
const MOLTEN_H_FRAC: f32 = 1.0 / 6.0;
/// The deep-crust bedrock cell's height — the THICK layer: as tall as the
/// cell is wide (the height the molten cell used to have).
const BEDROCK_H_FRAC: f32 = 1.0;
/// A hair of daylight between stacked cells, so the shared face between two
/// closed columns never z-fights and the stack reads as CELLS, not one slab.
const STACK_GAP_FRAC: f32 = 0.04;

// ── the crust map's inks (instrument data colours, like godmode's rock) ──
/// Bedrock — the deep crust's lid: most of the crust map is this brown.
const BEDROCK_COLOR: [f32; 3] = [0.36, 0.28, 0.19];
/// Lava — a vent where the molten heat pushed through the bedrock: the dots
/// on the crust map, and the whole bedrock cell of a vented column.
const LAVA_COLOR: [f32; 3] = [0.88, 0.16, 0.05];
/// Lava, EMISSIVE — the over-unit red drives the direct-colour glow bit
/// (flicker-globe: radiance past 1 is emission), so a vent BURNS on the
/// surface instead of sitting matte under the light.
const LAVA_GLOW: [f32; 3] = [1.25, 0.22, 0.07];
/// **The geological drift's cadence** (Aaron 2026-08-25: the upwelling seams
/// and volcanic dots SLOWLY shift — seams grow and shrink, volcanoes go
/// dormant and new ones form, over much longer timelines): every this many
/// era ticks the molten field's phases advance by the amount, the crust's
/// vents re-derive on the breathed field, and the plates' derived motion
/// follows. The seams/crust/plates tabs re-bake LAZILY on next entry.
const DRIFT_EVERY: u32 = 12;
const DRIFT_AMOUNT: f32 = 0.06;

/// How strongly the molten heat below WARMS the crust's bedrock — the subtle
/// brown→lava shading around the hot zones (Aaron 2026-08-25: "still bedrock
/// brown, just shade it a little from the heat map below"). Applied on
/// heat², so interiors stay clean brown and only the real heat blushes.
const CRUST_SHADE_GAIN: f32 = 0.55;
/// The generation ZONE's base ink — the clearly-bounded ember band over every
/// tile above the upwell floor: where the crust actually makes material.
const UPWELL_ZONE_COLOR: [f32; 3] = [0.52, 0.20, 0.07];
/// A continental plate — this is LAND (there is no water yet), so it reads
/// brown (Aaron 2026-08-25: "the ocean beds are probably more grey and land
/// is more on the brown side").
const CONTINENT_COLOR: [f32; 3] = [0.42, 0.32, 0.21];
/// An oceanic plate — a bare rock bed, grey, not sea-blue: nothing has filled
/// it with water yet.
const OCEAN_BED_COLOR: [f32; 3] = [0.33, 0.34, 0.36];
/// Young volcanic rock — the basalt the evolution era piles up.
const ROCK_COLOR: [f32; 3] = [0.17, 0.15, 0.14];
/// Loose SEDIMENT — the pale wash riding every column's top: the softest
/// material, the water cycle's currency.
const SEDIMENT_COLOR: [f32; 3] = [0.62, 0.57, 0.47];
/// What a fully water-compacted consolidated cell shades toward — indurated
/// marine rock: darker, denser, colder than fresh strata.
const COMPACT_COLOR: [f32; 3] = [0.28, 0.29, 0.33];
/// A consolidated stratum — rock that became a LAYER.
const STRATA_COLOR: [f32; 3] = [0.48, 0.44, 0.40];
// ── the motion-arrow FIELD (the God Mode read, replicated): a stippled
// sample of columns across the WHOLE globe, each arrow coloured by its
// plate and FILLING toward its next one-hex step ──
/// How many headings the field aims to draw, whatever the resolution.
const MOTION_ARROWS: usize = 2600;
/// Below this step-progress a column is not going anywhere worth drawing.
const MOTION_FLOOR: f32 = 0.02;
/// Legibility gain on arrow length — a true one-hex shaft is ~1% of the
/// radius and unreadable, so the shafts draw longer.
const MOTION_GAIN: f32 = 3.2;
/// Arrowhead barb length, as a fraction of the shaft.
const MOTION_BARB: f32 = 0.34;
/// Radius the headings draw at — clear of the ground, under the graticule.
/// The arrows ride ABOVE the tallest column, like the reference frame does
/// (Aaron 2026-08-29: "they get buried after a while — draw on top of the
/// visible hex stack"), below the graticule's 1.2 so the frames stay layered.
const R_MOTION: f32 = 1.16;

// ── WATER (Aaron 2026-08-25: coverage dial; three layers — deep, shallow,
// surface/shelf — whose temperature + circulation arrive with the erosion
// era; today the LEVEL and the three static bands) ──
/// The coverage dial's range and its Earth-like default: ~71% of Earth's
/// surface is ocean (75 was Aaron's guess; 71 is the standard figure).
const MIN_WATER: u32 = 0;
const MAX_WATER: u32 = 100;
/// The era opens as a WATER WORLD: the dial is a live coverage GAUGE now
/// (Aaron 2026-08-26) — it reads how much of the surface IS water (starting
/// ~100 and falling as the land grows); a hand on it re-pours to that share.
const DEFAULT_WATER: u32 = 100;
/// A RUN's per-frame compute budget: the engine ticks flat-out inside it and
/// the UI keeps the rest of the frame. Once a tick costs more than this the
/// loop admits one per frame, so throughput ≈ frame rate — keeping frames
/// fast IS the fast path.
const RUN_FRAME_MS: u128 = 12;
/// How many ticks of climate the history sparkline remembers. A BOUNDED ring, so a
/// million-tick era costs exactly these floats and never grows; sized well past the
/// well's pixel width because the plot downsamples to one segment per column — a
/// longer memory costs storage, never draw calls.
const CLIMATE_HISTORY: usize = 1024;
// (The climate dial's MIN/MAX_TEMP range died with the dial — the climate
// readout is a 0..1 gauge now and the baseline stays the era default.)
/// Ice caps — the ink the frozen ground shades toward, in the view and stack.
const ICE_COLOR: [f32; 3] = [0.86, 0.90, 0.94];
/// The band depths, in tile-width units below the sea level: shallower than
/// SURFACE_DEPTH is the surface/shelf layer (lakes, inland seas, drowned
/// shelf); deeper than DEEP_DEPTH is the deep ocean; between is shallow sea.
const SURFACE_DEPTH: f32 = 0.2;
const DEEP_DEPTH: f32 = 0.7;
// ── the first two ATMOSPHERIC layers (Aaron 2026-08-25: volumetric-fog
// cells, mainly transparent — the clouds at their densest should barely
// occlude; the point is only to SHOW the moisture the erosion drinks) ──
/// The two layers' altitudes above the sea line and their cell thickness,
/// in tile-width units — haze low and broad, clouds at the condensation deck.
const ATMO_THICK: f32 = 0.3;
/// The weather decks' fog cells: below AIR_FLOOR a cell is absent, at
/// AIR_FULL it reads fully bright; per-deck alphas stay near-nothing
/// (Aaron: the clouds barely occlude — they indicate).
const AIR_FLOOR: f32 = 0.06;
const AIR_FULL: f32 = 0.9;
const DECK_ALPHA: [f32; 3] = [0.07, 0.14, 0.10];
// ── THE RAIN TINT (Aaron 2026-08-28): wet land GREENS (a muted olive — the
// river films keep the bright translucent green), dry land reads SANDY
// where loose sediment leads its surface or STONY where rock and strata
// do — plains, prairies and deserts at a glance. ──
const GREEN_WET_COLOR: [f32; 3] = [0.22, 0.45, 0.16];
const DRY_SAND_COLOR: [f32; 3] = [0.66, 0.50, 0.28];
const DRY_STONE_COLOR: [f32; 3] = [0.51, 0.50, 0.48];
/// The rain EMA that reads as FULLY watered (measured land-rain p90 ≈
/// 0.016, p99 ≈ 0.054 — the first calibration, 0.1, greened nothing).
const RAIN_TINT_SCALE: f32 = 33.0;
/// The three water layers' inks, deep to surface.
const SHALLOW_WATER_COLOR: [f32; 3] = [0.10, 0.22, 0.38];
const SURFACE_WATER_COLOR: [f32; 3] = [0.20, 0.38, 0.52];
// ── THE OPEN OCEAN (Aaron 2026-08-28: "significantly more blue… close to
// the saturation of the green river knobs and the red lava knobs… anything
// on the deep ocean outside of shelf region should be much darker, earth
// blue"): water standing over a SHELF-class bed keeps the pale inks above;
// water over open sea floor wears these — bold, saturated, dark from
// orbit — with a lighter hand on the temperature wash so the blue holds. ──
const OPEN_DEEP_COLOR: [f32; 3] = [0.010, 0.075, 0.30];
const OPEN_SHALLOW_COLOR: [f32; 3] = [0.035, 0.22, 0.58];
const OPEN_SURFACE_COLOR: [f32; 3] = [0.07, 0.33, 0.68];
/// The temperature wash on shelf water (the old hand) and open ocean (the
/// lighter one that keeps the blue saturated).
const SHELF_TEMP_WASH: f32 = 0.45;
const OPEN_TEMP_WASH: f32 = 0.22;
/// Open-ocean alphas, band by band (deep, shallow, surface) — bolder than
/// the shelf's 0.62 / 0.5 / 0.38.
const OPEN_ALPHA: [f32; 3] = [0.78, 0.64, 0.50];
/// DRY-land reclassification levels, in tile-width units of TOTAL height
/// (Aaron: material that rises above the shelf and plate levels must take
/// the correct colour). LIVE classification (Aaron 2026-08-28: shorelines
/// and shelves must reclassify as the sim advances): drowned ground is
/// SHELF only while it stays in the shallow band AND under the induration
/// grade — pushed deep, or pressed and sedimented hard enough, it is BED.
const SHELF_DEPTH: f32 = 0.55;
const SHELF_BED_GRADE: f32 = 1.6;

/// What a tile's GROUND is, judged by its CURRENT total height — never by
/// what it was rolled as: a seamount that grows past the levels becomes
/// shelf, then land (the reclassification Aaron asked for). The ground is
/// ROCK wet or dry; the water above it is its own transparent cells, split
/// geometrically at the band depths.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ground {
    Bed,
    Shelf,
    Land,
}

/// Classify one tile LIVE against the standing sea and its own marine
/// grade: above the line is LAND; drowned ground is SHELF while shallow
/// and young; deep water, or a bed the water has pressed hard, is BED.
fn ground_class(total: f32, sea: f32, bed_hard: f32) -> Ground {
    if total >= sea {
        Ground::Land
    } else if sea - total < SHELF_DEPTH && bed_hard < SHELF_BED_GRADE {
        Ground::Shelf
    } else {
        Ground::Bed
    }
}

// (The sea level lives with the era now — `Evolution::sea_level`, the
// percentile of the era's own heights — so the dial has one derivation.)

/// CONTINENTAL SHELF — the transitional zone where a bed meets a continent,
/// the ONE edge the surface marks (plate joins between two beds or two
/// continents paint nothing). A sandy tone between the two kinds.
const SHELF_COLOR: [f32; 3] = [0.56, 0.49, 0.35];
// (The plate shell's per-kind BASE heights live with the scheme —
// `plates::{CONTINENT_H_FRAC, OCEAN_BED_H_FRAC}` — because the era's ground
// ledger is seeded from them too.)
// (The heat-elevation constant is the ERA's law now — `evolve::ELEV_H_FRAC` —
// so ground height has one owner.)
/// How far below the nominal surface a plate column's walls reach (per cell
/// width) — the root that keeps neighbouring cells of different heights
/// reading as EXTRUDED solids, never floating caps.
const PLATE_ROOT_FRAC: f32 = 0.25;

/// Which data the world's tile shell is painted with. `Authored` is the
/// stage's own look (the MAP tab). Every view is a BAKED mesh set in the one
/// world (`GlobeWorld::bake`), rebuilt when its DATA changes — so a tab
/// switch is a free `show`, never a rebuild holding the old picture on
/// screen (Aaron's stale-flash report, 2026-08-25).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorldView {
    Authored,
    Heat,
    Crust,
    Evolve,
}

impl WorldView {
    /// The baked set this view draws from.
    fn key(self) -> &'static str {
        match self {
            WorldView::Authored => "authored",
            WorldView::Heat => "heat",
            WorldView::Crust => "crust",
            WorldView::Evolve => "evolve",
        }
    }
}

/// The bench's Lua ORCHESTRATION script — `arrange()` decides which tab-specific
/// components are shown from the two-way-bound page/tab selection. Embedded (the
/// same `include_str!` pattern the shell scenes use), so the bench ships with it.
const POPULOUS_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/populous.lua");

/// **Populous Bench** — the DEFAULT PAGE, and the surface the world will be
/// authored from.
pub struct PopulousBench {
    // ── navigation (indices into the roster, never a hardcoded count) ──
    sel_page: usize,
    /// Each PAGE's remembered tab (indexed by page): leaving a page and
    /// returning restores the tab you were on (Aaron 2026-08-25 — the PTT
    /// retains the tab between page swaps; a fresh page opens on its first).
    page_tabs: Vec<usize>,

    // ── the surface (the scene as DATA) ──
    /// The static component tree, parsed ONCE from `populous.scene.json`:
    /// every component declared, each tab's slice gated on a `shown_p0_t*` bind.
    /// Never rebuilt — the walker redraws it each frame against a fresh Model.
    tree: UiNode,
    /// The Lua ORCHESTRATION host (`populous.lua`): `arrange()` reads the bound
    /// page/tab and returns which slice is lit. Held for the bench's whole life.
    script: ScriptHost,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    /// The screen's declared signal->result intents, read off the static tree ONCE.
    ui_intents: UiIntents,
    hud_commands: Vec<HudCommand>,
    theme: Option<Theme>,
    /// The theme's engine textures in ID ORDER — white 0, muse 1, pad_glyphs 2.
    /// Registered whole so a component added to the page later (text, sprites,
    /// pad glyphs) draws without re-plumbing the atlas hand-off.
    textures: Vec<TextureHandle>,

    // ── input (input-P3, 0569DA9B): the scene owns NO resolver/bindings. The central
    //    PUMP resolves this frame's World-context events for the scene's `input_context()`
    //    (the World default); the walker consumes the edges and the globe reads continuous
    //    look/zoom from `signals.axis`. ──

    // ── the globe as an instrument ──
    map: HexMap,
    /// **The hex world, whole.** Meshes, offscreen target, authored stage and
    /// camera all live in the one component; this bench hands it the tiling and
    /// the rect the walker reserved, and nothing else. No colour, no radius, no
    /// mesh, no camera state, no device read.
    world: GlobeWorld,

    // ── the molten heat field, and the two views of it ──
    /// **The molten layer's first fact** — N convection cells and the per-tile
    /// heat their boundaries induce. SHARED world state like the map itself
    /// (Aaron's ruling): the seams tab paints it and the hex page reads a
    /// column from it; neither owns a copy.
    seams: SeamField,
    /// **The deep crust's consequence of it** — the vents where that heat
    /// pushes through the bedrock. Derived from `seams`, re-derived whenever
    /// it changes; the crust tab's lava dots and the stack's lava columns.
    crust: CrustField,
    /// **The evolution era's living state** — poles, loose rock, formed
    /// strata. Ticks only while the evolve view is watched and running.
    evolve: Evolution,
    /// Whether the era's clock runs.
    evolve_running: bool,
    /// Era ticks since the last geological drift.
    drift_unticked: u32,
    /// The molten-fed views (heat/crust/plates) have drifted since their last
    /// bake — re-baked lazily on next entry, never mid-era.
    molten_views_stale: bool,
    /// PLAY-N's goal and start (0/0 = none): while the era's clock is under
    /// the goal the run loop computes flat-out with no baking, the progress
    /// bar filling from `roll_from`; arrival bakes once and stops. PLAY/PAUSE
    /// runs with NO window (both zero) until the pause click bakes.
    roll_until: u64,
    roll_from: u64,
    /// PLAY-N's count field, as typed (digits only, "1200" fresh) — the tick
    /// contract (Aaron 2026-08-29): enter a number, click, run THAT many.
    tick_count: String,
    /// The world view's fluid lenses (Aaron 2026-08-29): atmosphere decks,
    /// ocean bands (+ the shelf-ice stand-in), river films — each hides so
    /// the ground reads bare. Lenses, never resets; a flip rebakes once.
    show_air: bool,
    show_water: bool,
    show_rivers: bool,
    /// Planetary water coverage, percent of surface flooded — the sea-level
    /// dial. DISPLAY + classification level today; the three water layers'
    /// temperature and circulation arrive with the erosion era. Changing it
    /// never resets the era (it is a lens, not a roll).
    /// Whether the plate MOTION ARROWS draw on the evolve view — a lens, like
    /// the water dial: toggling it resets nothing.
    show_arrows: bool,
    /// The last COMMIT's result line — the staged `.epoch`'s path, or the
    /// error. Published on [`ui::COMMIT_STATUS_BIND`]; empty until the first
    /// commit (the footer text takes no ink).
    commit_status: String,
    /// **One column of the world, up close** — the HEX page's view: the same
    /// component as `world`, framing the centre cell's molten column instead of
    /// the planet. A second VIEW, never a second world.
    hex: GlobeWorld,
    /// **The world laid out FLAT** — the shared [`WorldMap`] component behind the
    /// map modal (contract FF8A575D): the hex sphere cut on a seam, pole caps
    /// trimmed at full zoom, locally re-projected once zoomed to a region. Its
    /// cell colours are folded from the SAME shell list the globe bakes with.
    worldmap: WorldMap<HexSphereMap>,
    /// Whether the map modal stands. Rust owns the flag; `populous.lua`'s
    /// `arrange()` reads it off the model and lights the modal's slice.
    map_open: bool,
    /// The CENTRE cell — the fixed reticle: on the seams tab, whichever tile
    /// faces the camera; the hex page shows this cell's column.
    focus_tile: TileId,
    /// The reticle ring currently drawn over the globe (`None` off the seams
    /// tab) — kept so the outline is rebuilt only when the faced cell changes.
    highlight: Option<TileId>,
    /// Which data the published tile shell is painted with — kept so a tab
    /// change rebuilds the world's colours exactly once, not per frame.
    shown_view: WorldView,
    /// **The era's CLIMATE HISTORY** — the ice-age runner's live temperature
    /// (`Evolution::climate`, an O(1) reading the engine refreshes inside the tick)
    /// sampled ONCE PER TICK into a bounded ring. Plotted against the gauge's own
    /// `0..1` range so the curve does not re-scale itself every time the world warms
    /// a little, and emptied whenever the era restarts — the history belongs to the
    /// era, not to the bench.
    climate_history: PlotSeries,
    /// The readout that draws it: the shared [`Plot`] filler seated on the
    /// [`ui::TEMP_PLOT_SLOT`] `surface` under the climate gauge — the walker
    /// reserves the rect, the scene seats and layers what the filler draws over the
    /// HUD. Its ink is read ONCE out of the scene's own `plot` style block; the
    /// filler owns no palette.
    climate_plot: Plot,
}

impl PopulousBench {
    pub fn new(def: &SceneDef) -> Self {
        let map = HexMap::new(DEFAULT_FREQ);
        tracing::info!("populous: {} tiles at freq {}", map.len(), map.freq());
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        // Open with the world FILLING the square viewport (Aaron: ~85%), not a
        // distant marble; the camera belongs to the viewport PANE, which is what
        // hands it the look signals.
        let world =
            GlobeWorld::new(ui::STAGE_SOURCE, &ui_styles, Some(0.85)).in_panel(ui::VIEW_PANE);

        // The scene is DATA: the kernel's manifest parsed the authored scene-def and
        // handed it here (this bench is the BEHAVIOUR that plays it). Its tree names
        // component KINDS directly (the template tier is gone — 201F4F51).
        let tree = def
            .tree
            .clone()
            .expect("populous.scene.json declares a tree");
        // The declared signal->result intents live in the static tree — read ONCE.
        let ui_intents = UiIntents::of(&tree);
        // The Lua ORCHESTRATION host, held for the bench's life so `arrange()` runs
        // each frame the selection may have moved.
        let script = ScriptHost::new(POPULOUS_SCRIPT, "populous.lua")
            .expect("populous.lua loads (it ships with the crate)");

        // The hex-stack view: the SAME component as the planet view, told to
        // frame one column instead of one world. It shares the viewport pane's
        // camera gate — only one of the two is ever on screen at a time.
        let hex = GlobeWorld::new(ui::HEX_STAGE_SOURCE, &ui_styles, None).in_panel(ui::VIEW_PANE);
        // The flat map: the SAME tiling the globe draws, plugged into the shared
        // WorldMap view; its authored look (seam ink, tile base + inset, the
        // graticule) comes off its own stage exactly as the globe's does.
        let mut worldmap = WorldMap::new(
            ui::MAP_STAGE_SOURCE,
            &ui_styles,
            HexSphereMap::from_tiling(&map.grid().dirs, map.outlines()),
        );
        let look = worldmap.authored_look();
        worldmap.content_mut().set_look(look);
        // THE CLIMATE HISTORY readout (the `plot` filler's first consumer, B05B3D09
        // §4d): the ice-age curve behind the live gauge. The RING is the scene's —
        // the filler samples nothing itself — and the INK comes off the scene's own
        // `plot` style block, token-resolved like every other colour in the bench.
        let climate_plot = Plot::new(
            PlotKind::Curve,
            PlotStyle {
                line: style_rgba(&ui_styles, "plot.line"),
                fill: style_rgba(&ui_styles, "plot.fill"),
                baseline: style_rgba(&ui_styles, "plot.baseline"),
                grid: style_rgba(&ui_styles, "plot.grid"),
                ..Default::default()
            },
        );
        let seams = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, fastrand::u64(..));
        let crust = CrustField::derive(&map, &seams);
        let mut evolve = Evolution::new(&map, &seams);
        evolve.set_water(DEFAULT_WATER as f32);

        let mut bench = Self {
            sel_page: 0,
            page_tabs: vec![0; ui::PAGES.len()],
            tree,
            script,
            ui_styles,
            ui_state: UiState::default(),
            ui_intents,
            hud_commands: Vec::new(),
            theme: None,
            textures: Vec::new(),
            map,
            world,
            seams,
            crust,
            evolve,
            evolve_running: false,
            drift_unticked: 0,
            molten_views_stale: false,
            roll_until: 0,
            roll_from: 0,
            tick_count: flicker_worldengine::BOOTSTRAP_TICKS.to_string(),
            show_air: true,
            show_water: true,
            show_rivers: true,
            show_arrows: true,
            commit_status: String::new(),
            hex,
            worldmap,
            map_open: false,
            focus_tile: 0,
            highlight: None,
            shown_view: WorldView::Authored,
            // The gauge's own range, fixed: a history that re-scaled itself would
            // make every era look identically dramatic.
            climate_history: PlotSeries::new(CLIMATE_HISTORY).fixed_range(0.0, 1.0),
            climate_plot,
        };
        // The opening reticle: whatever the default camera faces.
        bench.focus_tile = bench.world.facing(&bench.map.grid().dirs).unwrap_or(0) as TileId;
        // Bake EVERY view up front — data changes re-bake theirs, and a tab
        // switch is then a free swap.
        bench.bake_view(WorldView::Authored);
        bench.bake_molten_views();
        bench.world.show(bench.shown_view.key());
        bench.publish_hex();
        bench
    }

    /// **One tick of the era, recorded.** Every path that advances the clock
    /// flat-out (PLAY, PLAY-N) or one at a time (the TICK button) runs it here, so
    /// the climate history cannot drift from the era: ONE site pushes, and it pushes
    /// the O(1) reading the engine just refreshed — never a scan of the world (a
    /// per-tick `coverage()` would re-resolve the sea over every tile, 405F7034).
    fn tick_era(&mut self) {
        let Self {
            map,
            seams,
            crust,
            evolve,
            climate_history,
            ..
        } = &mut *self;
        let sea = evolve.resolve_sea();
        evolve.tick(map, seams, crust, sea);
        climate_history.push(evolve.climate());
        self.drift_fields();
    }

    /// **Return the era to its bare shell**, and take its history with it: the
    /// engine's own reset, the default water, and the ring emptied. Every restart —
    /// the RESET button, a seams re-roll, a cells/spots re-pour, a new tiling —
    /// goes through here, so a curve can never outlive the era it measured.
    fn reset_era(&mut self) {
        self.evolve.reset(&self.map, &self.seams);
        self.evolve.set_water(DEFAULT_WATER as f32);
        self.climate_history.clear();
    }

    /// The CURRENT page's tab — its remembered selection, clamped to the
    /// page's own roster (pages have different tab counts, and the memory of
    /// a bigger page must not index off the end of a smaller one).
    fn sel_tab(&self) -> usize {
        let tabs = ui::page(self.sel_page).tabs.len();
        self.page_tabs[self.sel_page.min(self.page_tabs.len() - 1)].min(tabs.saturating_sub(1))
    }

    /// Which data the world's tile shell wears for the CURRENT selection: the
    /// SEAMS tab paints the molten heat field, the CRUST tab paints bedrock
    /// with the lava vents, every other selection shows the authored look. A
    /// tab owns its VIEW — the fields are shared state either way.
    fn world_view(&self) -> WorldView {
        let page = ui::page(self.sel_page);
        if page.id != "world" {
            return WorldView::Authored;
        }
        match page.tabs[self.sel_tab()].id {
            "seams" => WorldView::Heat,
            "crust" => WorldView::Crust,
            "evolve" => WorldView::Evolve,
            _ => WorldView::Authored,
        }
    }

    /// Whether the current tab carries the centre-cell reticle — the two
    /// LAYER tabs, where which cell you are aimed at is the question.
    fn reticle_view(&self) -> bool {
        self.world_view() != WorldView::Authored
    }

    /// Whether the HEX page is the selected one — which of the two centre-pane
    /// views (the planet or the column) owns the camera and the pointer.
    fn hex_view(&self) -> bool {
        ui::page(self.sel_page).id == "hex"
    }

    /// Bake ONE view's mesh set into the world. WHAT the world looks like —
    /// the two static shells' radii, insets and colours — is authored in
    /// `stages.populous_globe` and read by the world itself; the bench
    /// supplies only the geometry. On a LAYER view the ONE data override: the
    /// topmost (tile) shell is painted with that layer's field — the molten
    /// heat through the shared ramp, the crust's bedrock dotted with its lava
    /// vents, or the plate shell's kinds EXTRUDED by the seam heat below —
    /// the same pattern God Mode paints its fields with, radii and insets
    /// still the stage's. Baking never changes which view SHOWS.
    fn bake_view(&mut self, view: WorldView) {
        {
            let lenses = (self.show_air, self.show_water, self.show_rivers);
            let Self {
                map,
                world,
                seams,
                crust,
                evolve,
                ..
            } = self;
            let shells = Self::view_shell_list(view, map, world, seams, crust, evolve, lenses);
            world.bake(view.key(), shells);
        }
        // The flat map paints with the SAME shell list (contract FF8A575D):
        // while it is open, a re-bake of the SHOWN view refreshes its cell
        // colours too, so the map and the globe can never disagree.
        if self.map_open && view == self.shown_view {
            self.refresh_map_colors();
        }
    }

    /// The one shell list a view is drawn from — built here for BOTH consumers:
    /// [`Self::bake_view`] hands it to the globe, [`Self::refresh_map_colors`]
    /// folds it flat for the world map. `lenses` is (air, water, rivers) —
    /// the evolve view's fluid toggles; a false drops that family of shells
    /// so the ground reads bare (the shelf ice rides the water lens: it
    /// stands in for the sea it froze).
    fn view_shell_list<'a>(
        view: WorldView,
        map: &'a HexMap,
        world: &GlobeWorld,
        seams: &'a SeamField,
        crust: &'a CrustField,
        evolve: &'a Evolution,
        lenses: (bool, bool, bool),
    ) -> Vec<ShellSpec<'a>> {
        let (show_air, show_water, show_rivers) = lenses;
        let mut shells = world.authored_shells(&map.grid().dirs, map.outlines());
        if let Some(top) = shells.last_mut() {
            match view {
                WorldView::Authored => {}
                WorldView::Heat => {
                    top.color = Box::new(|i| Some(temp_color(seams.heat(i as TileId))));
                }
                WorldView::Crust => {
                    let seams: &SeamField = seams;
                    top.color = Box::new(move |i| {
                        let t = i as TileId;
                        let h = seams.heat(t);
                        Some(if crust.is_vent(t) {
                            LAVA_GLOW // over-unit: the vent EMITS
                        } else if h >= flicker_worldengine::UPWELL_HEAT {
                            // THE GENERATION ZONE (Aaron 2026-08-25: the
                            // crust tab defines the places material is
                            // generated): everything above the upwell floor
                            // wears a clearly-bounded ember band, deepening
                            // toward lava as the heat climbs — the seam
                            // zones read as zones, the vents burn inside
                            // them.
                            let z = (h - flicker_worldengine::UPWELL_HEAT)
                                / (1.0 - flicker_worldengine::UPWELL_HEAT);
                            lerp3(UPWELL_ZONE_COLOR, LAVA_COLOR, z)
                        } else {
                            // Bedrock, blushed by the sub-floor heat: brown
                            // in the interiors, warming as the zone nears —
                            // heat², so only real heat shows.
                            lerp3(BEDROCK_COLOR, LAVA_COLOR, CRUST_SHADE_GAIN * h * h)
                        })
                    });
                }
                WorldView::Evolve => {
                    // The living view. Colour is CLASSIFIED, never inherited:
                    // each tile's current total height against the sea level
                    // decides what it IS — deep/shallow/surface water below
                    // the line, bed/shelf/land above it (grown material that
                    // crosses a level takes the level's colour — Aaron's
                    // reclassification) — with the era's basalt, strata and
                    // vent glow shading the dry ground, and vents glowing
                    // faintly through shallow water (submarine volcanoes).
                    let crust: &CrustField = crust;
                    let evolve: &Evolution = evolve;
                    let w = map
                        .tiles()
                        .next()
                        .map(|t| tile_width(map.direction(t), map.outline(t), RADIUS))
                        .unwrap_or(1.0);
                    // The sea level that floods the asked share of the world.
                    // Heights come from the ERA's OWN ground ledger — the
                    // base rides the conveyor, so the land visibly drifts;
                    // the heat elevation stays in the mantle's frame beneath.
                    let era_h = move |t: TileId| evolve.ground(t);
                    let sea = evolve.resolve_sea();
                    // The GROUND is ROCK, wet or dry (Aaron 2026-08-25: the
                    // depth recolouring was wrong — brown, grey and dark are
                    // the crust's colours; the water goes ON TOP as its own
                    // transparent cells). Height still reclassifies rock that
                    // grows through the shelf and land levels.
                    top.color = Box::new(move |i| {
                        let t = i as TileId;
                        let total = era_h(t);
                        // LIVE classification: the shoreline and the shelf
                        // follow the standing sea and the marine press —
                        // an old shelf pushed deep or indurated reclasses
                        // (and recolours) as BED.
                        let mut c = match ground_class(total, sea, evolve.bed_hardness(t)) {
                            Ground::Land => CONTINENT_COLOR,
                            Ground::Shelf => SHELF_COLOR,
                            Ground::Bed => OCEAN_BED_COLOR,
                        };
                        c = lerp3(c, ROCK_COLOR, (evolve.rock(t) * 1.2).clamp(0.0, 0.8));
                        let (n, _) = evolve.strata(t);
                        c = lerp3(c, STRATA_COLOR, (f32::from(n) / 3.0).clamp(0.0, 0.7));
                        // THE GREENING (land only): VEGETATION paints the
                        // watered country green — strongly, it is the land's
                        // cover — while unwatered, uncovered land reads
                        // sandy where sediment leads its surface, stony
                        // where rock and strata do: desertification you can
                        // see. Rivers keep their brighter translucent film.
                        if total >= sea {
                            let v = evolve.vegetation(t);
                            let rain = (evolve.rainfall(t) * RAIN_TINT_SCALE).clamp(0.0, 1.0);
                            let dry = (1.0 - v.max(rain)).clamp(0.0, 1.0);
                            let dry_c = if evolve.sediment(t) >= evolve.rock(t) {
                                DRY_SAND_COLOR
                            } else {
                                DRY_STONE_COLOR
                            };
                            c = lerp3(c, dry_c, dry * 0.55);
                            // THE TINT MATCHES THE METRIC (Aaron 2026-08-28:
                            // 70% on the dial read as bare tan): a tile that
                            // COUNTS as greened (cover ≥ GREEN_COVER) reads
                            // clearly green — the ramp lands 0.5 blend AT the
                            // counted threshold and saturates from there,
                            // instead of the old linear cover × 0.85 that
                            // left threshold-green land 70% sand.
                            let cover = flicker_worldengine::GREEN_COVER;
                            let g = if v < cover {
                                0.5 * (v / cover)
                            } else {
                                0.5 + 0.35 * ((v - cover) / (1.0 - cover))
                            };
                            c = lerp3(c, GREEN_WET_COLOR, g);
                        }
                        if crust.is_vent(t) {
                            c = lerp3(c, LAVA_COLOR, 0.5);
                        }
                        // THE VEINS glow through like the lava nodes do — the
                        // bench's x-ray on the buried ore bodies, inked per
                        // kind (gold amber, coal black, calcite chalk…).
                        if let Some(k) = evolve.vein(t) {
                            c = lerp3(c, flicker_worldengine::vein_kinds()[k as usize].ink, 0.5);
                        }
                        // THE CAPS: standing ice whitens the ground toward
                        // frozen-through — the ice ages read at a glance.
                        let ice = evolve.ice(t);
                        if ice > 0.02 {
                            c = lerp3(
                                c,
                                ICE_COLOR,
                                (ice / flicker_worldengine::ICE_SOLID).clamp(0.0, 1.0) * 0.92,
                            );
                        }
                        Some(c)
                    });
                    top.cell_radius = Some(Box::new(move |i| RADIUS + era_h(i as TileId) * w));
                    top.depth = Some(Box::new(move |i| {
                        era_h(i as TileId) * w + w * PLATE_ROOT_FRAC
                    }));
                    // THE WATER — the liquid layer's three cells, standing
                    // ABOVE the rock as their own TRANSPARENT shells (deep,
                    // shallow, surface — bottom-up, because blending has no
                    // sort and draw order is list order). Each is sparse:
                    // a tile carries a band's cell only where its ground
                    // lies below that band's ceiling. The ocean's top is
                    // FLAT — the one level the dial set.
                    let ground = era_h;
                    // FIVE water shells now (Aaron 2026-08-28): the deep
                    // band is always OPEN OCEAN — bold, dark, earth-blue —
                    // and the shallow/surface bands each split per cell into
                    // an open-ocean variant and a SHELF variant that keeps
                    // the old pale inks, so coastal water reads as the
                    // shallows it stands over. `over_shelf`: None = any bed,
                    // Some(want) = only cells whose LIVE class matches.
                    type Band = (usize, f32, f32, Option<bool>, [f32; 3], f32, f32);
                    let bands: [Band; 5] = [
                        // (band, ceiling, floor, over_shelf, colour, alpha, gloss)
                        (
                            0,
                            sea - DEEP_DEPTH,
                            f32::NEG_INFINITY,
                            None,
                            OPEN_DEEP_COLOR,
                            OPEN_ALPHA[0],
                            0.1,
                        ),
                        (
                            1,
                            sea - SURFACE_DEPTH,
                            sea - DEEP_DEPTH,
                            Some(false),
                            OPEN_SHALLOW_COLOR,
                            OPEN_ALPHA[1],
                            0.2,
                        ),
                        (
                            1,
                            sea - SURFACE_DEPTH,
                            sea - DEEP_DEPTH,
                            Some(true),
                            SHALLOW_WATER_COLOR,
                            0.5,
                            0.2,
                        ),
                        (
                            2,
                            sea,
                            sea - SURFACE_DEPTH,
                            Some(false),
                            OPEN_SURFACE_COLOR,
                            OPEN_ALPHA[2],
                            0.45,
                        ),
                        (
                            2,
                            sea,
                            sea - SURFACE_DEPTH,
                            Some(true),
                            SURFACE_WATER_COLOR,
                            0.38,
                            0.45,
                        ),
                    ];
                    if show_water {
                        for (band, ceil, floor, over_shelf, colour, alpha, gloss) in bands {
                            shells.push(ShellSpec {
                                dirs: &map.grid().dirs,
                                outlines: map.outlines(),
                                radius: RADIUS + ceil * w,
                                inset: 0.0,
                                color: Box::new(move |i| {
                                    let t = i as TileId;
                                    // Frozen through: thick ice REPLACES its
                                    // water column — the cap is solid, not sea.
                                    if ground(t) >= ceil
                                        || evolve.ice(t) >= flicker_worldengine::ICE_SOLID
                                    {
                                        return None;
                                    }
                                    if let Some(want) = over_shelf {
                                        let shelf = matches!(
                                            ground_class(ground(t), sea, evolve.bed_hardness(t)),
                                            Ground::Shelf
                                        );
                                        if shelf != want {
                                            return None;
                                        }
                                    }
                                    // THE OCEAN'S OWN HEAT tints each band by ITS
                                    // temperature — surface tracked per tile,
                                    // deep the one global reservoir, shallow the
                                    // mix (bands bottom-up: 0 deep, 2 surface) —
                                    // with a lighter wash on the open ocean so
                                    // the bold blue holds its saturation.
                                    let (sst, mid, deep) = evolve.ocean_temps(t);
                                    let bt = [deep, mid, sst][band];
                                    let wash = if over_shelf == Some(true) {
                                        SHELF_TEMP_WASH
                                    } else {
                                        OPEN_TEMP_WASH
                                    };
                                    Some(lerp3(colour, water_temp_color(bt), wash))
                                }),
                                cell_radius: None,
                                depth: Some(Box::new(move |i| {
                                    let g = ground(i as TileId).max(floor);
                                    (ceil - g) * w
                                })),
                                opts: MeshDrawOptions {
                                    tint: [1.0, 1.0, 1.0, alpha],
                                    gloss,
                                    ..MeshDrawOptions::default()
                                },
                            });
                        }
                        // THE SHELF ICE: a frozen-through sea tile dropped its
                        // water columns above ("the cap is solid, not sea"), so
                        // the cap itself must stand in — one cell at sea level
                        // closing down to the bed, or the view shows the seabed
                        // as a pit ringed by ocean cliffs. The stack draws this
                        // same cap; here it keeps the ocean's surface unbroken.
                        shells.push(ShellSpec {
                            dirs: &map.grid().dirs,
                            outlines: map.outlines(),
                            radius: RADIUS + sea * w,
                            inset: 0.0,
                            color: Box::new(move |i| {
                                let t = i as TileId;
                                if ground(t) < sea
                                    && evolve.ice(t) >= flicker_worldengine::ICE_SOLID
                                {
                                    Some(ICE_COLOR)
                                } else {
                                    None
                                }
                            }),
                            cell_radius: None,
                            depth: Some(Box::new(move |i| {
                                (sea - ground(i as TileId)).max(0.0) * w
                            })),
                            opts: MeshDrawOptions {
                                tint: [1.0, 1.0, 1.0, 0.94],
                                gloss: 0.5,
                                ..MeshDrawOptions::default()
                            },
                        });
                    }
                    if show_rivers {
                        // THE RIVERS (Aaron 2026-08-27: "highlighted in green
                        // water, to distinguish from ocean blue"): every LAND
                        // tile carrying a live channel wears a thin translucent
                        // green film on its column top — the discharge network
                        // made visible, trunks inked deeper than headwaters.
                        // Frozen-solid ground shows ice, not running water.
                        shells.push(ShellSpec {
                            dirs: &map.grid().dirs,
                            outlines: map.outlines(),
                            radius: RADIUS,
                            inset: 0.0,
                            color: Box::new(move |i| {
                                let t = i as TileId;
                                let d = evolve.discharge(t);
                                if d < flicker_worldengine::CHANNEL_LIVE
                                    || ground(t) < sea
                                    || evolve.ice(t) >= flicker_worldengine::ICE_SOLID
                                {
                                    return None;
                                }
                                let full = flicker_worldengine::CHANNEL_LIVE * RIVER_FULL;
                                let s = ((d - flicker_worldengine::CHANNEL_LIVE) / full)
                                    .clamp(0.0, 1.0);
                                Some(lerp3(RIVER_COLOR, RIVER_DEEP_COLOR, s))
                            }),
                            cell_radius: Some(Box::new(move |i| {
                                RADIUS + (ground(i as TileId) + RIVER_FILM) * w
                            })),
                            depth: Some(Box::new(move |_| RIVER_FILM * w)),
                            opts: MeshDrawOptions {
                                tint: [1.0, 1.0, 1.0, RIVER_ALPHA],
                                gloss: 0.5,
                                ..MeshDrawOptions::default()
                            },
                        });
                    }
                    if show_air {
                        // THE ATMOSPHERE — the weather engine's decks as
                        // volumetric fog cells (Aaron 2026-08-28: "volumes of
                        // moisture drawn as concentrations in various layers"):
                        // each deck's cell present only where IN-FLIGHT moisture
                        // actually stands, brightness riding the concentration —
                        // cloud banks over the convergence zones, streets along
                        // the winds, clear skies over the deserts. A mountain
                        // taller than a deck pokes through. Drawn last: the most
                        // transparent thing in the scene.
                        for (l, &alpha) in DECK_ALPHA.iter().enumerate() {
                            let deck = sea + flicker_worldengine::DECK_ALT[l];
                            shells.push(ShellSpec {
                                dirs: &map.grid().dirs,
                                outlines: map.outlines(),
                                radius: RADIUS + deck * w,
                                inset: 0.0,
                                color: Box::new(move |i| {
                                    let t = i as TileId;
                                    let m = evolve.air_layer(l, t);
                                    (m >= AIR_FLOOR && ground(t) < deck).then(|| {
                                        // Denser moisture = whiter, thicker fog.
                                        let b = 0.5 + 0.5 * (m / AIR_FULL).min(1.0);
                                        [b, b, b + 0.02]
                                    })
                                }),
                                cell_radius: None,
                                depth: Some(Box::new(move |_| ATMO_THICK * w)),
                                opts: MeshDrawOptions {
                                    tint: [1.0, 1.0, 1.0, alpha],
                                    gloss: 0.0,
                                    ..MeshDrawOptions::default()
                                },
                            });
                        }
                    }
                }
            }
        }
        shells
    }

    /// Fold the SHOWN view's shell list into ONE colour per cell for the flat
    /// map: each shell's colour lands where it answers, blended by its draw
    /// alpha (the water films and fog decks), later shells over earlier — the
    /// same order the globe draws them. Runs only while the map is open, on the
    /// bake cadence, never per frame.
    fn refresh_map_colors(&mut self) {
        let colors = {
            let Self {
                map,
                world,
                seams,
                crust,
                evolve,
                shown_view,
                show_air,
                show_water,
                show_rivers,
                ..
            } = self;
            let shells = Self::view_shell_list(
                *shown_view,
                map,
                world,
                seams,
                crust,
                evolve,
                (*show_air, *show_water, *show_rivers),
            );
            // Loud-wrong magenta until a shell answers — an unpainted cell is a
            // visible defect, never a quiet stand-in (rule 4BB12A75).
            let mut colors = vec![[1.0f32, 0.0, 1.0]; map.len()];
            for s in &shells {
                let a = s.opts.tint[3];
                for (i, c) in colors.iter_mut().enumerate() {
                    if let Some(rgb) = (s.color)(i) {
                        let rgb = rgb.map(|v| v.clamp(0.0, 1.0));
                        *c = if a >= 0.999 { rgb } else { lerp3(*c, rgb, a) };
                    }
                }
            }
            colors
        };
        self.worldmap.content_mut().set_colors(colors);
    }

    /// Bake the views whose DATA the molten field feeds: the heat map, the
    /// crust's vents — and the plate shell too, whose RELIEF is the seam heat
    /// (Aaron: "this tab should calculate plates after seams change"). The
    /// authored view reads only the map and is not touched.
    fn bake_molten_views(&mut self) {
        self.bake_view(WorldView::Heat);
        self.bake_view(WorldView::Crust);
        self.bake_view(WorldView::Evolve);
    }

    /// One era tick's share of the GEOLOGICAL DRIFT: on the cadence, breathe
    /// the molten field, re-derive the vents on it (dormancy and birth), let
    /// the plates' motion follow the shifted push, and mark the molten-fed
    /// tabs for a lazy re-bake. The era reads the drifted fields next tick.
    fn drift_fields(&mut self) {
        self.drift_unticked += 1;
        if self.drift_unticked < DRIFT_EVERY {
            return;
        }
        self.drift_unticked = 0;
        self.seams.drift(&self.map, DRIFT_AMOUNT);
        self.crust = CrustField::derive(&self.map, &self.seams);
        self.evolve.derive_motion(&self.map, &self.seams);
        self.molten_views_stale = true;
    }

    /// The selection moved — SHOW its baked set. A swap, not a rebuild: the
    /// meshes were baked when their data changed, so nothing stale lingers on
    /// screen while a 92k-tile view rebuilds (the flash Aaron reported).
    fn refresh_world_view(&mut self) {
        if self.world_view() != self.shown_view {
            self.shown_view = self.world_view();
            if self.molten_views_stale
                && matches!(self.shown_view, WorldView::Heat | WorldView::Crust)
            {
                // The era's drift moved the fields since these were baked —
                // catch the whole molten-fed family up once, on entry.
                self.molten_views_stale = false;
                self.bake_molten_views();
            }
            self.world.show(self.shown_view.key());
            self.apply_overlays();
            // The open map follows the view swap — its colours are the shown
            // view's, refreshed here because a tab switch bakes nothing.
            if self.map_open {
                self.refresh_map_colors();
            }
        }
    }

    /// **One column of the shared world, standing on the hex page — now two
    /// cells of it.** The centre cell's outline rotated upright
    /// ([`column_frame`]), each layer a CLOSED column (`ShellSpec::depth`) at
    /// the planet's true radii so the side walls keep the true radial taper,
    /// framed small and low: the bottom of the ~50-cell stack the view leaves
    /// room for. Bottom to top: the thin MOLTEN cell (its own heat on the
    /// shared ramp), then the thick BEDROCK cell of the deep crust — bedrock
    /// brown, or a red LAVA column where this cell is one of the crust's
    /// vents. Layer heights are the provisional fractions above until the
    /// ledger authors real depths.
    fn publish_hex(&mut self) {
        let Self {
            map,
            hex,
            seams,
            crust,
            evolve,
            focus_tile,
            ..
        } = self;
        let tile = (*focus_tile).min(map.len().saturating_sub(1) as TileId);
        let dir = map.direction(tile);
        let ring = column_frame(dir, map.outline(tile));
        let w = tile_width(dir, map.outline(tile), RADIUS);
        // The stack reads the ERA's ground — bare floor at tick zero, and
        // whatever the upwelling built after. LAND is a height fact now, not
        // a plate kind: whatever stands above the resolved sea.
        let continent = evolve.ground(tile) >= evolve.resolve_sea();
        let h_plate = evolve.base(tile) * w;
        let h_bed = w * BEDROCK_H_FRAC;
        let h_molten = w * MOLTEN_H_FRAC;
        let gap = w * STACK_GAP_FRAC;
        let frame = HEX_FRAME_TILES * w;
        hex.set_frame(frame, Some(HEX_FILL));
        // The stack's base sits at the BOTTOM of the framed region: the orbit
        // goes around the point one frame-radius above it — where the rest of
        // the stack will stand.
        let base = RADIUS - h_plate - gap - h_bed - gap - h_molten;
        hex.aim(Vec3::Y * (base + frame));
        let molten = temp_color(seams.heat(tile));
        let bedrock = if crust.is_vent(tile) {
            LAVA_GLOW // the vent cell burns in the stack too
        } else {
            let h = seams.heat(tile);
            lerp3(BEDROCK_COLOR, LAVA_COLOR, CRUST_SHADE_GAIN * h * h)
        };
        // The marine grade shades every CONSOLIDATED cell of this column:
        // 1.0 = fresh, the cap = indurated sea-pressed rock.
        let compact = ((evolve.bed_hardness(tile) - 1.0)
            / (flicker_worldengine::MARINE_HARD_CAP - 1.0))
            .clamp(0.0, 1.0);
        let plate = lerp3(
            if continent {
                CONTINENT_COLOR
            } else {
                OCEAN_BED_COLOR
            },
            COMPACT_COLOR,
            compact * 0.75,
        );
        let dirs = [Vec3::Y];
        let outlines = [ring];
        let mut shells = vec![
            ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: RADIUS - h_plate - gap - h_bed - gap,
                inset: 0.0,
                color: Box::new(move |_| Some(molten)),
                cell_radius: None,
                depth: Some(Box::new(move |_| h_molten)),
                opts: MeshDrawOptions::default(),
            },
            ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: RADIUS - h_plate - gap,
                inset: 0.0,
                color: Box::new(move |_| Some(bedrock)),
                cell_radius: None,
                depth: Some(Box::new(move |_| h_bed)),
                opts: MeshDrawOptions::default(),
            },
            ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: RADIUS,
                inset: 0.0,
                color: Box::new(move |_| Some(plate)),
                cell_radius: None,
                depth: Some(Box::new(move |_| h_plate)),
                opts: MeshDrawOptions::default(),
            },
        ];
        // The era's GROWN layers stand above the plate cell: each formed
        // stratum its own cell, the loose rock a thin working cell on top —
        // the stack the ticks are building.
        // The crust sub-group's formed slots as REAL cells (Aaron
        // 2026-08-26): L3 the vein layer — wearing its ore's ink when a vein
        // lives here — then L4 the volcanic layer above it. L5 is reserved
        // and draws nothing.
        let mut top = RADIUS;
        let l3 = evolve.layer3(tile) * w;
        if l3 > 0.005 {
            let ink = match evolve.vein(tile) {
                Some(k) => lerp3(
                    lerp3(STRATA_COLOR, COMPACT_COLOR, compact),
                    flicker_worldengine::vein_kinds()[k as usize].ink,
                    0.65,
                ),
                None => lerp3(STRATA_COLOR, COMPACT_COLOR, compact),
            };
            top += gap + l3;
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some(ink)),
                cell_radius: None,
                depth: Some(Box::new(move |_| l3)),
                opts: MeshDrawOptions::default(),
            });
        }
        let l4 = evolve.layer4(tile) * w;
        if l4 > 0.005 {
            let ink = lerp3(
                lerp3(STRATA_COLOR, ROCK_COLOR, 0.35),
                COMPACT_COLOR,
                compact * 0.5,
            );
            top += gap + l4;
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some(ink)),
                cell_radius: None,
                depth: Some(Box::new(move |_| l4)),
                opts: MeshDrawOptions::default(),
            });
        }
        let rock_h = evolve.rock(tile) * w;
        if rock_h > 0.005 {
            top += gap + rock_h;
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some(ROCK_COLOR)),
                cell_radius: None,
                depth: Some(Box::new(move |_| rock_h)),
                opts: MeshDrawOptions::default(),
            });
        }
        // The loose SEDIMENT riding the column's top — the cell the stack
        // was missing: the pale soft wash the erosion is moving, the very
        // material the standing water below presses into the bed.
        let sed_h = evolve.sediment(tile) * w;
        if sed_h > 0.005 {
            top += gap + sed_h;
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some(SEDIMENT_COLOR)),
                cell_radius: None,
                depth: Some(Box::new(move |_| sed_h)),
                opts: MeshDrawOptions::default(),
            });
        }
        // The WATER above a submerged column, deep to surface — the three
        // layers' static faces (their temperature and circulation arrive with
        // the erosion era). Depth = the sea level minus this column's total
        // height, split at the band depths.
        let sea = evolve.resolve_sea();
        let total = evolve.ground(tile);
        let depth = (sea - total).max(0.0);
        // Build upward from the ground: deep first, then shallow, then the
        // surface layer.
        // Each band wears ITS OWN temperature (the ocean's heat by depth:
        // surface tracked, deep the one global reservoir, shallow the mix).
        let (b_sst, b_mid, b_deep) = evolve.ocean_temps(tile);
        // The stack's water wears the same class-aware inks as the globe:
        // pale over a SHELF column, bold open-ocean blue elsewhere.
        let shelfy = matches!(
            ground_class(total, sea, evolve.bed_hardness(tile)),
            Ground::Shelf
        );
        let (c_sh, c_sf, wash, a_sh, a_sf) = if shelfy {
            (
                SHALLOW_WATER_COLOR,
                SURFACE_WATER_COLOR,
                SHELF_TEMP_WASH,
                0.5,
                0.38,
            )
        } else {
            (
                OPEN_SHALLOW_COLOR,
                OPEN_SURFACE_COLOR,
                OPEN_TEMP_WASH,
                OPEN_ALPHA[1],
                OPEN_ALPHA[2],
            )
        };
        let bands = [
            (
                lerp3(OPEN_DEEP_COLOR, water_temp_color(b_deep), OPEN_TEMP_WASH),
                (depth - DEEP_DEPTH).max(0.0),
                OPEN_ALPHA[0],
                0.1,
            ),
            (
                lerp3(c_sh, water_temp_color(b_mid), wash),
                (depth.min(DEEP_DEPTH) - SURFACE_DEPTH).max(0.0),
                a_sh,
                0.2,
            ),
            (
                lerp3(c_sf, water_temp_color(b_sst), wash),
                depth.min(SURFACE_DEPTH),
                a_sf,
                0.45,
            ),
        ];
        for (color, h, alpha, gloss) in bands {
            if h <= 0.01 {
                continue;
            }
            let hw = h * w;
            top += gap + hw;
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some(color)),
                cell_radius: None,
                depth: Some(Box::new(move |_| hw)),
                // Water is TRANSPARENT cells over the rock (Aaron
                // 2026-08-25), with a wet sheen strongest at the surface.
                opts: MeshDrawOptions {
                    tint: [1.0, 1.0, 1.0, alpha],
                    gloss,
                    ..MeshDrawOptions::default()
                },
            });
        }
        // THE CAP: standing ice rides above the water (or bare ground) —
        // the frozen cell of the column, present whenever the ice ledger
        // holds anything real on this tile.
        let ice_h = evolve.ice(tile) * w;
        if ice_h > 0.01 {
            top += gap + ice_h;
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some(ICE_COLOR)),
                cell_radius: None,
                depth: Some(Box::new(move |_| ice_h)),
                opts: MeshDrawOptions {
                    tint: [1.0, 1.0, 1.0, 0.94],
                    gloss: 0.5,
                    ..MeshDrawOptions::default()
                },
            });
        }
        // The ATMOSPHERE's cells over the column — one per weather deck,
        // present only where that deck actually holds in-flight moisture:
        // the stack shows the VOLUMES, layer by layer.
        for (l, &alpha) in DECK_ALPHA.iter().enumerate() {
            let m = evolve.air_layer(l, tile);
            if m < AIR_FLOOR {
                continue;
            }
            let hw = ATMO_THICK * w;
            top += gap + hw;
            let b = 0.5 + 0.5 * (m / AIR_FULL).min(1.0);
            shells.push(ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: top,
                inset: 0.0,
                color: Box::new(move |_| Some([b, b, b + 0.02])),
                cell_radius: None,
                depth: Some(Box::new(move |_| hw)),
                opts: MeshDrawOptions {
                    tint: [1.0, 1.0, 1.0, alpha],
                    gloss: 0.0,
                    ..MeshDrawOptions::default()
                },
            });
        }
        hex.set_shells(shells);
    }

    /// A run CAME TO REST — PLAY-N arrived, PAUSE was clicked, or TICK/STEP
    /// finished its cycle work: the clock stops and the ONE bake of the tick
    /// contract lands (Aaron 2026-08-29: "no bake, just run... when pause is
    /// clicked, then bake"). Running never bakes; resting always does.
    fn run_complete(&mut self) {
        self.evolve_running = false;
        self.roll_until = 0;
        self.roll_from = 0;
        self.bake_view(WorldView::Evolve);
        self.publish_hex();
        self.apply_overlays();
    }

    /// The stepping caveat (Aaron 2026-08-29): a run button clicked while a
    /// stepped tick stands open first COMPLETES that tick — the cycle always
    /// closes before the next begins, so no button ever forks a half-run
    /// pipeline. A no-op when the cursor already rests between cycles.
    fn complete_open_tick(&mut self) {
        let mut closed = false;
        while self.evolve.current_phase() != flicker_worldengine::PHASES[0] {
            let Self {
                map,
                seams,
                crust,
                evolve,
                ..
            } = &mut *self;
            let sea = evolve.resolve_sea();
            closed = evolve.tick_phase(map, seams, crust, sea);
        }
        if closed {
            self.drift_fields(); // the closed cycle counts like any other tick
        }
    }

    /// The line OVERLAYS on the globe, composed fresh each change: the
    /// centre-cell reticle (two raised bold rings) on any layer tab, and on
    /// the EVOLVE view the LOCAL motion arrows — each sampled tile's own
    /// seam-push, radiating from the sources. Drawn over the stage's own
    /// reference frame (set_arrows re-lays the graticule under them).
    fn apply_overlays(&mut self) {
        let Self {
            map,
            world,
            evolve,
            highlight,
            shown_view,
            show_arrows,
            ..
        } = self;
        let mut overlays: Arrows = Vec::new();
        let w = map
            .tiles()
            .next()
            .map(|t| tile_width(map.direction(t), map.outline(t), RADIUS))
            .unwrap_or(1.0);
        if let Some(tile) = *highlight {
            let ring = map.outline(tile);
            let n = ring.len();
            // The reticle RIDES its column: the rings lift by the cell's own
            // grown height plus a clear margin, and corner posts run from
            // the column top to the rings — never buried under a mountain.
            let top = evolve.ground(tile) * w;
            let lift = top + RETICLE_LIFT * w;
            let mut segs = Vec::with_capacity(n * (RETICLE_RINGS.len() + 1));
            for scale in RETICLE_RINGS {
                for k in 0..n {
                    segs.push((
                        ring[k] * (RADIUS * scale + lift),
                        ring[(k + 1) % n] * (RADIUS * scale + lift),
                    ));
                }
            }
            for v in ring {
                segs.push((*v * (RADIUS + top), *v * (RADIUS * RETICLE_RINGS[1] + lift)));
            }
            overlays.push((RETICLE_INK, segs));
        }
        if *shown_view == WorldView::Evolve {
            // THE VEIN FIELD OUTLINES (Aaron 2026-08-27): every ore body
            // wears its boundary ring in its own ink, lifted above its
            // columns — rubies, coal, calcite each announce their field.
            // Boundary = an edge whose far side is not the same KIND; the
            // edge is found as the outline segment facing that neighbour.
            let kinds = flicker_worldengine::vein_kinds();
            for t in 0..map.len() as TileId {
                let Some(k) = evolve.vein(t) else {
                    continue;
                };
                let ring = map.outline(t);
                let m = ring.len();
                let p = map.direction(t);
                let lift = (evolve.ground(t) + VEIN_RING_LIFT) * w;
                for nb in map.neighbours(t) {
                    if evolve.vein(*nb) == Some(k) {
                        continue; // interior: the body continues
                    }
                    // The outline edge FACING this neighbour: the segment
                    // whose midpoint leans furthest toward it.
                    let toward = (map.direction(*nb) - p).normalize_or_zero();
                    let e = (0..m)
                        .max_by(|a, b| {
                            let mid_a = (ring[*a] + ring[(*a + 1) % m]) * 0.5 - p;
                            let mid_b = (ring[*b] + ring[(*b + 1) % m]) * 0.5 - p;
                            mid_a
                                .normalize_or_zero()
                                .dot(toward)
                                .total_cmp(&mid_b.normalize_or_zero().dot(toward))
                        })
                        .unwrap_or(0);
                    let ink = kinds[k as usize].ink;
                    let color = [ink[0], ink[1], ink[2], 1.0];
                    let slot = match overlays.iter_mut().find(|(c, _)| *c == color) {
                        Some(s) => &mut s.1,
                        None => {
                            overlays.push((color, Vec::new()));
                            &mut overlays.last_mut().expect("just pushed").1
                        }
                    };
                    slot.push((
                        ring[e] * (RADIUS + lift),
                        ring[(e + 1) % m] * (RADIUS + lift),
                    ));
                }
            }
        }
        if *shown_view == WorldView::Evolve && *show_arrows {
            // THE LOCAL FIELD (Aaron 2026-08-25: the seams drive the crust —
            // arrows RADIATE from the vents and seams, and cold ground shows
            // none): ~MOTION_ARROWS tiles sampled by the stable stipple hash,
            // one arrow per sampled tile along its OWN push, shaft filling
            // toward the tile's next one-hex step. A rigid per-plate field
            // marched arrows straight over volcanoes; this one cannot.
            let n = map.len();
            let step_len = RADIUS * (4.0 * std::f32::consts::PI / n as f32).sqrt() * MOTION_GAIN;
            let coverage = (MOTION_ARROWS as f64 / n as f64).min(1.0);
            for t in 0..n as TileId {
                let i = t as usize;
                if !stippled(i, 977, coverage) {
                    continue;
                }
                let progress = evolve.drift_progress(t);
                if progress < MOTION_FLOOR {
                    continue;
                }
                let d = map.direction(t);
                let heading = evolve.push_at(t).normalize_or_zero();
                if heading.length_squared() < 0.5 {
                    continue;
                }
                let from = d * (RADIUS * R_MOTION);
                let shaft = heading * (step_len * progress);
                let to = from + shaft;
                let side = d.cross(heading).normalize_or_zero() * (shaft.length() * MOTION_BARB);
                let back = heading * (shaft.length() * MOTION_BARB);
                // One ink: there are no plates in the era — the field is
                // the seams' push, radiating from the sources.
                let color = MOTION_INK;
                let slot = match overlays.iter_mut().find(|(k, _)| *k == color) {
                    Some(s) => &mut s.1,
                    None => {
                        overlays.push((color, Vec::new()));
                        &mut overlays.last_mut().expect("just pushed").1
                    }
                };
                slot.push((from, to));
                slot.push((to, to - back + side));
                slot.push((to, to - back - side));
            }
        }
        world.set_arrows(overlays);
    }

    /// The map in the viewport.
    pub fn map(&self) -> &HexMap {
        &self.map
    }

    /// The molten heat field — the seams tab's data, for tests and future
    /// layers.
    pub fn seams(&self) -> &SeamField {
        &self.seams
    }

    /// The deep crust's vent set — the crust tab's data, derived from the
    /// seam field.
    pub fn crust(&self) -> &CrustField {
        &self.crust
    }

    /// The evolution era's living state — the evolve tab's data.
    pub fn evolve(&self) -> &Evolution {
        &self.evolve
    }

    /// The water-coverage percentage — the sea-level dial's committed value.
    /// The LIVE water coverage the gauge shows, percent of surface flooded.
    pub fn water_coverage_pct(&self) -> u32 {
        (self.evolve.coverage() * 100.0).round() as u32
    }

    /// Whether the motion arrows are shown — the checkbox's committed value.
    pub fn show_arrows(&self) -> bool {
        self.show_arrows
    }

    /// Whether the era's clock is running.
    pub fn evolve_running(&self) -> bool {
        self.evolve_running
    }

    /// The live fast-roll window `(from, until)` — `None` when no roll is
    /// queued. For gates and tools.
    pub fn roll_window(&self) -> Option<(u64, u64)> {
        (self.roll_until > 0).then_some((self.roll_from, self.roll_until))
    }

    /// The centre cell — the reticle's tile, whose column the hex page shows.
    pub fn focus_tile(&self) -> TileId {
        self.focus_tile
    }

    /// The scene's static component tree — a clone for a gate or a test to walk. It is
    /// authored DATA now (`populous.scene.json`), parsed ONCE in `new`, so
    /// this is no longer per-(page, tab): which slice shows is decided by `arrange()`'s
    /// visibility binds, not by rebuilding structure.
    pub fn build_tree(&self) -> UiNode {
        self.tree.clone()
    }

    /// Which page and tab the rails are on — the roster indices, for tests.
    pub fn selection(&self) -> (usize, usize) {
        (self.sel_page, self.sel_tab())
    }

    /// Which node holds the walker's ONE focus — for tests. The panel cursor,
    /// the d-pad and the pointer all write this same id; the scene never does.
    pub fn focused(&self) -> Option<&str> {
        self.ui_state.focused()
    }

    /// Stand the walker's cursor on `id` — the test-side stand-in for a left-stick
    /// press or a click, so a gate can exercise the focus-gated channels without
    /// building a whole input frame. **Not a scene path**: nothing in `update`
    /// writes focus, which is the point.
    #[cfg(test)]
    fn focus_for_test(&mut self, id: &str) {
        self.ui_state.request_focus(id);
    }

    /// **What the components read.** The rail selections as NUMBERS (an index is
    /// a number end to end — rule 1B64FF03), whether the selected page has tabs
    /// at all (a condition on the ROSTER, so a future page with none collapses
    /// the tab rail by itself), the world's COMMITTED size, and the three stat
    /// readouts already formatted.
    ///
    /// Formatting happens HERE and rides a bind, never in the tree: a node
    /// carries a `$token` caption or a bind name, never a composed string. The
    /// dial publishes the map's own frequency — the control owns its display
    /// while a gesture is in flight (rule 3A04B4CE), so the scene has nothing
    /// live to publish.
    fn model(&self) -> ValueMap {
        let mut m = ValueMap::default();
        // CONFIRM = APPLY (Aaron 2026-09-04): every dial stages its pad steps until a
        // Confirm commits them; the footer's "Press {Confirm} to apply" legend entry
        // lights on this flag, read straight off the walker. The legend's affordance is
        // device-adaptive, so the bench publishes the Confirm binding + device exactly as
        // Clayworks does for its footer.
        m.set("ui_staged", self.ui_state.staged_any());
        flicker_shell::publish_signal_bindings(
            &mut m,
            &flicker_shell::current_world_map(),
            [ActionSignal::Confirm],
        );
        m.set(ui::PAGE_BIND, self.sel_page as f64);
        m.set(ui::TAB_BIND, self.sel_tab() as f64);
        m.set(ui::TABS_SHOWN, !ui::page(self.sel_page).tabs.is_empty());
        m.set(ui::FREQ_BIND, f64::from(self.map.freq()));
        m.set(ui::CELLS_BIND, f64::from(self.seams.cells()));
        m.set(ui::SPOTS_BIND, f64::from(self.seams.spots()));
        m.set(ui::HEXES_BIND, group_thousands(self.map.len() as u64));
        m.set(
            ui::DIAMETER_BIND,
            group_thousands(flicker_worldengine::diameter_mi(self.map.freq()).round() as u64),
        );
        m.set(
            ui::TILE_BIND,
            format!("{:.2}", flicker_worldengine::TILE_MI),
        );
        // The bootstrap roll's progress bar: visible while the era is running
        // toward the horizon, filled by the tick fraction.
        let booting =
            self.evolve_running && self.roll_until > 0 && self.evolve.ticks() < self.roll_until;
        m.set("pop_booting", booting);
        let span = self.roll_until.saturating_sub(self.roll_from).max(1);
        m.set(
            "pop_boot",
            (self.evolve.ticks().saturating_sub(self.roll_from) as f64 / span as f64).min(1.0),
        );
        // The two LIVE READOUTS (Aaron 2026-08-27, godmode-gauge style —
        // right panel, never interactive): each is a 0..1 gauge fill plus a
        // pre-formatted percent.
        let coverage = self.evolve.coverage();
        m.set(ui::WATER_BIND, f64::from(coverage));
        m.set(
            ui::WATER_VAL_BIND,
            format!("{}%", (coverage * 100.0).round()),
        );
        let climate = self.evolve.climate();
        m.set(ui::TEMP_BIND, f64::from(climate));
        m.set(ui::TEMP_VAL_BIND, format!("{}%", (climate * 100.0).round()));
        // The MEASURED green share — what the target dial is actually
        // steering: the eye can now check the ask against the world.
        let green = self.evolve.green_share();
        m.set(ui::GREEN_BIND, f64::from(green));
        m.set(ui::GREEN_VAL_BIND, format!("{}%", (green * 100.0).round()));
        m.set(
            ui::WATER_TARGET_BIND,
            f64::from((self.evolve.water_target() * 100.0).round() as u32),
        );
        m.set(
            ui::VEG_TARGET_BIND,
            f64::from((self.evolve.veg_target() * 100.0).round() as u32),
        );
        m.set(ui::ARROWS_BIND, self.show_arrows);
        m.set(ui::SHOW_AIR_BIND, self.show_air);
        m.set(ui::SHOW_WATER_BIND, self.show_water);
        m.set(ui::SHOW_RIVERS_BIND, self.show_rivers);
        // The PLAY-N count rides its field bind as typed; the parse-or-1200
        // fallback lives on the click, so a half-typed field never fights.
        m.set(ui::TICK_COUNT_BIND, self.tick_count.clone());
        // The map modal's runtime flag: `arrange()` reads it and lights the
        // modal's slice (`shown_map`) — the one visibility path.
        m.set(ui::MAP_OPEN_BIND, self.map_open);
        m.set(ui::TICKS_BIND, group_thousands(self.evolve.ticks()));
        // The nav footer's commit result: a path or an error, pre-formatted
        // here like every readout; empty until the first commit.
        m.set(ui::COMMIT_STATUS_BIND, self.commit_status.clone());
        // The material census TABLE: two columns per row (label | hexes),
        // most-common first — labels are registry notation, counts formatted
        // here like every readout. Rows past the roster fold into a final
        // "+K" row carrying the remaining hexes; unused rows publish empty
        // strings and take no ink.
        let census = self.evolve.vein_census();
        let kinds = flicker_worldengine::vein_kinds();
        for i in 0..ui::CENSUS_ROWS {
            let (name, count) = if census.len() > ui::CENSUS_ROWS && i == ui::CENSUS_ROWS - 1 {
                let rest = &census[ui::CENSUS_ROWS - 1..];
                (
                    format!("+{}", rest.len()),
                    group_thousands(rest.iter().map(|(_, c)| u64::from(*c)).sum()),
                )
            } else if let Some((k, c)) = census.get(i) {
                (
                    kinds[*k as usize].label.clone(),
                    group_thousands(u64::from(*c)),
                )
            } else {
                (String::new(), String::new())
            };
            m.set(ui::census_name_bind(i), name);
            m.set(ui::census_count_bind(i), count);
        }
        m.set(ui::PHASE_BIND, phase_token(self.evolve.current_phase()));
        m.set(ui::STRATA_BIND, group_thousands(self.evolve.strata_total()));
        // THE HEX INSPECTOR (Aaron 2026-08-27): the focused column itemized —
        // materials on the left pane, fluids on the right, every value
        // pre-formatted here like the other readouts. "—" is the empty
        // value; heights ride tile-width units; grades ride ×N.NN.
        {
            let t = self
                .focus_tile
                .min(self.map.len().saturating_sub(1) as TileId);
            let e = &self.evolve;
            let dash = || "—".to_string();
            let opt = |v: f32| if v > 1e-3 { format!("{v:.2}") } else { dash() };
            let graded = |h: f32, g: f32| {
                if h > 1e-3 {
                    format!("{h:.2} ×{g:.2}")
                } else {
                    dash()
                }
            };
            m.set(ui::HEX_SED_BIND, opt(e.sediment(t)));
            m.set(ui::HEX_ROCK_BIND, graded(e.rock(t), e.rock_hardness(t)));
            let (g3, g4) = e.strata_hardness(t);
            m.set(ui::HEX_L4_BIND, graded(e.layer4(t), g4));
            m.set(ui::HEX_L3_BIND, graded(e.layer3(t), g3));
            m.set(
                ui::HEX_VEIN_BIND,
                match e.vein(t) {
                    Some(k) => flicker_worldengine::vein_kinds()[k as usize].label.clone(),
                    None => dash(),
                },
            );
            m.set(
                ui::HEX_BASE_BIND,
                format!("{:.2} ×{:.2}", e.base(t), e.bed_hardness(t)),
            );
            // The deep crust's provisional authored thickness (w units) —
            // the honest value until the ledger authors real depths.
            m.set(ui::HEX_BEDROCK_BIND, format!("{BEDROCK_H_FRAC:.2}"));
            let sea = e.resolve_sea();
            let depth = sea - e.ground(t);
            m.set(
                ui::HEX_MOIST_BIND,
                format!("{}%", (e.moisture(t) * 100.0).round()),
            );
            m.set(
                ui::HEX_RAIN_BIND,
                format!("{:.0}", (e.rainfall(t) * 1000.0).round()),
            );
            m.set(
                ui::HEX_RIVER_BIND,
                if e.discharge(t) >= flicker_worldengine::CHANNEL_LIVE {
                    format!("{:.1}", e.discharge(t))
                } else {
                    dash()
                },
            );
            m.set(ui::HEX_ICE_BIND, opt(e.ice(t)));
            m.set(
                ui::HEX_WATER_BIND,
                if depth > 0.0 {
                    format!("{depth:.2}")
                } else {
                    dash()
                },
            );
            let (sst, _, deep) = e.ocean_temps(t);
            m.set(
                ui::HEX_WTEMP_BIND,
                if depth > 0.0 {
                    format!("{:.0} / {:.0}", sst * 100.0, deep * 100.0)
                } else {
                    dash()
                },
            );
            m.set(
                ui::HEX_HEAT_BIND,
                format!("{}%", (self.seams.heat(t) * 100.0).round()),
            );
        }
        m
    }

    /// Rebuild the ONE shared world at a new size, clamped by the offered range.
    /// A no-op at the current size. The heat field re-derives over the new
    /// tiling from the SAME roll — the world's seams do not move when its map
    /// does — and both views republish.
    fn resize(&mut self, freq: u32) {
        let freq = freq.clamp(MIN_FREQ, MAX_FREQ);
        if freq == self.map.freq() {
            return;
        }
        self.map = HexMap::new(freq);
        self.seams.rebuild(&self.map);
        self.crust = CrustField::derive(&self.map, &self.seams);
        // The centre cell is SHARED state — keep it a tile the new map has;
        // the old reticle outlined tiles that no longer exist, so it comes
        // down and the next frame on the seams tab re-faces it.
        self.focus_tile = self
            .focus_tile
            .min(self.map.len().saturating_sub(1) as TileId);
        self.highlight = None;
        self.apply_overlays();
        // A new tiling: the era restarts over it, and every view's geometry
        // moved — the flat map's atlas re-lays-out over the same new tiling.
        let mut content = HexSphereMap::from_tiling(&self.map.grid().dirs, self.map.outlines());
        content.set_look(self.worldmap.authored_look());
        self.worldmap.replace_content(content);
        self.reset_era();
        self.bake_view(WorldView::Authored);
        self.bake_molten_views();
        if self.map_open {
            self.refresh_map_colors();
        }
        self.publish_hex();
        tracing::info!(
            "populous: {} tiles at freq {}",
            self.map.len(),
            self.map.freq()
        );
    }

    /// **The one dispatcher.** A click and a pad press arrive here identically,
    /// because both were folded into `results` before it was called.
    ///
    /// Four arms. Three of them read a value a CONTROL wrote — two rail indices
    /// and the dial's committed size — and the fourth is an action name a button
    /// fired. The rails step themselves (each strip owns its
    /// `next_action`/`prev_action`), the dial commits on release and owns its
    /// own display in between (rules B694F6B1 + 3A04B4CE), the panel cursor is
    /// the walker's, and activation arrives on the walker's one drain — so there
    /// is no stepper, no pane cursor, no enter/exit state and no live size copy
    /// left in this bench.
    pub fn apply_results(&mut self, results: &ValueMap) {
        let pages = ui::PAGES.len();
        // The strip writes its bind with the selected entry's roster index —
        // from a click on a cell, or from its own ±1 step. The bind also ECHOES
        // the resting value every frame (the echo contract), so only a CHANGED
        // value moves anything; in particular the page echo must not re-fire the
        // page arm, or its tab reset would pin the tab rail to 0 forever.
        let mut page_changed = false;
        if let Some(v) = results.number(ui::PAGE_BIND) {
            let want = (v.round().max(0.0) as usize).min(pages.saturating_sub(1));
            if want != self.sel_page {
                self.sel_page = want;
                // The new page opens on ITS remembered tab — the PTT retains
                // each page's tab between swaps (Aaron 2026-08-25).
                page_changed = true;
                self.refresh_world_view();
            }
        }
        // THE CLOBBER GUARD (the regression Aaron caught twice): the tab bind
        // ECHOES every frame, and on the page-change dispatch that echo still
        // carries the OUTGOING page's tab — folding it in would overwrite the
        // new page's memory (via the hex page's single tab it clamps to 0:
        // "page return resets to tab 0"). A tab write counts only on a
        // dispatch whose page stood still.
        if !page_changed {
            if let Some(v) = results.number(ui::TAB_BIND) {
                let tabs = ui::page(self.sel_page).tabs.len();
                let want = (v.round().max(0.0) as usize).min(tabs.saturating_sub(1));
                if want != self.sel_tab() {
                    self.page_tabs[self.sel_page] = want;
                    self.refresh_world_view();
                }
            }
        }
        // The dial's ONE write: the number it committed on release (or on a pad
        // step). `resize` is a no-op at the current size, so the every-frame
        // echo of the resting value rebuilds nothing.
        if let Some(v) = results.number(ui::FREQ_BIND) {
            self.resize(v.round() as u32);
        }
        // The cells dial: how many convection cells the heat field is rolled
        // with. `set_cells` is a no-op at the current count, so the resting
        // echo re-rolls nothing.
        if let Some(v) = results.number(ui::CELLS_BIND) {
            let before = self.seams.cells();
            self.seams.set_cells(&self.map, v.round().max(0.0) as u32);
            if self.seams.cells() != before {
                self.crust = CrustField::derive(&self.map, &self.seams);
                self.reset_era();
                self.bake_molten_views();
                self.publish_hex();
            }
        }
        // The spots dial: how many mantle plumes burn through. Same contract
        // as the cells dial, on the spots' own stream.
        if let Some(v) = results.number(ui::SPOTS_BIND) {
            let before = self.seams.spots();
            self.seams.set_spots(&self.map, v.round().max(0.0) as u32);
            if self.seams.spots() != before {
                self.crust = CrustField::derive(&self.map, &self.seams);
                self.reset_era();
                self.bake_molten_views();
                self.publish_hex();
            }
        }
        // THE ONE WATER CONTROL (Aaron 2026-08-27): the TARGET dial —
        // horizontal on the left pane, d-pad left/right nudges it, up/down
        // walks on to the buttons. The coverage and climate READOUTS moved
        // to the right pane as godmode-style gauges: display only, no
        // handler — the ice-age runner owns the climate number now and the
        // baseline stays the era default (`Evolution::set_climate` remains
        // the engine's own lever). A plain dial: committed number lands,
        // echo inert, wild clamps, nothing resets.
        if let Some(v) = results.number(ui::WATER_TARGET_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(MIN_WATER, MAX_WATER);
            if want != (self.evolve.water_target() * 100.0).round() as u32 {
                self.evolve.set_water_target(want as f32 / 100.0);
            }
        }
        // The GREEN TARGET dial: same plain-dial contract — the flora's
        // thirst walks toward the committed share; nothing repaints.
        if let Some(v) = results.number(ui::VEG_TARGET_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(MIN_WATER, MAX_WATER);
            if want != (self.evolve.veg_target() * 100.0).round() as u32 {
                self.evolve.set_veg_target(want as f32 / 100.0);
            }
        }
        // The motion-arrows checkbox: a display lens — the overlays recompose,
        // nothing resets.
        if let Some(flicker::script::Value::Bool(v)) = results.get(ui::ARROWS_BIND) {
            if *v != self.show_arrows {
                self.show_arrows = *v;
                self.apply_overlays();
            }
        }
        // The three fluid lenses (Aaron 2026-08-29): atmosphere, water,
        // rivers each hide so the ground reads bare. A flip changes which
        // shells the list carries, so the evolve view rebakes once — a lens
        // with a heavier arm than the arrows', still never a reset.
        let mut lens_flipped = false;
        for (bind, flag) in [
            (ui::SHOW_AIR_BIND, &mut self.show_air),
            (ui::SHOW_WATER_BIND, &mut self.show_water),
            (ui::SHOW_RIVERS_BIND, &mut self.show_rivers),
        ] {
            if let Some(flicker::script::Value::Bool(v)) = results.get(bind) {
                if *v != *flag {
                    *flag = *v;
                    lens_flipped = true;
                }
            }
        }
        if lens_flipped {
            self.bake_view(WorldView::Evolve);
        }
        // THE TICK CONTRACT (Aaron 2026-08-29): a tick is one complete run of
        // the procedure list — the bake is NOT part of it. PLAY/PAUSE runs
        // flat-out with no baking and bakes once on pause; PLAY-N runs the
        // typed count the same way and bakes at arrival; TICK plays one cycle
        // then bakes; STEP plays one procedure then bakes. Every run button
        // first completes a tick STEP left open.
        // The PLAY-N count field: digits only, remembered as typed; anything
        // unparseable falls back to the 1200 default on use.
        if let Some(t) = results.text(ui::TICK_COUNT_BIND) {
            if t != self.tick_count {
                self.tick_count = t.to_string();
            }
        }
        if results.is_on(ui::EVOLVE_RUN_ACTION) {
            if self.evolve_running {
                self.run_complete(); // PAUSE: the run's one bake lands here
            } else {
                self.complete_open_tick();
                self.roll_until = 0; // PLAY: no horizon — run until paused
                self.roll_from = 0;
                self.evolve_running = true;
            }
        }
        if results.is_on(ui::EVOLVE_ROLL_ACTION) {
            self.complete_open_tick();
            let count = self
                .tick_count
                .parse::<u64>()
                .ok()
                .filter(|c| *c > 0)
                .unwrap_or(flicker_worldengine::BOOTSTRAP_TICKS);
            self.roll_from = self.evolve.ticks();
            self.roll_until = self.evolve.ticks() + count;
            self.evolve_running = true;
        }
        if results.is_on(ui::EVOLVE_TICK_ACTION) {
            self.complete_open_tick();
            self.tick_era();
            self.run_complete();
        }
        if results.is_on(ui::EVOLVE_STEP_ACTION) {
            let Self {
                map,
                seams,
                crust,
                evolve,
                climate_history,
                ..
            } = &mut *self;
            let sea = evolve.resolve_sea();
            // A STEP is one PROCEDURE, not one tick — the history advances only on
            // the step that CLOSES the cycle, exactly as `drift_fields` does.
            if evolve.tick_phase(map, seams, crust, sea) {
                climate_history.push(evolve.climate());
                self.drift_fields();
            }
            self.run_complete(); // one procedure, then the bake shows it
        }
        if results.is_on(ui::EVOLVE_RESET_ACTION) {
            self.roll_until = 0;
            self.roll_from = 0;
            self.reset_era();
            self.evolve_running = false;
            self.bake_view(WorldView::Evolve);
            self.publish_hex();
        }
        // The randomize button: a new roll of the same count — the seams move,
        // both views repaint.
        if results.is_on(ui::SEAMS_ACTION) {
            self.seams.randomize(&self.map);
            self.crust = CrustField::derive(&self.map, &self.seams);
            self.reset_era();
            self.bake_molten_views();
            self.publish_hex();
        }
        // The nav footer's COMMIT — the bench's OUTPUT: the planet epoch,
        // staged for the Content Manager's review.
        if results.is_on(ui::COMMIT_ACTION) {
            let staging = flicker_content::roots().staging();
            self.commit_to(&staging);
        }
        // THE MAP MODAL (contract FF8A575D): the footer's MAP button toggles
        // it, the modal's own Close button closes it. Opening paints the flat
        // map with the shown view's colours — the one moment they could be
        // stale (the closed map skips every bake).
        if results.is_on(ui::MAP_TOGGLE_ACTION) {
            self.map_open = !self.map_open;
            if self.map_open {
                self.refresh_map_colors();
            }
        }
        if results.is_on(ui::MAP_CLOSE_ACTION) {
            self.map_open = false;
        }
    }

    /// The walker's scene-level Cancel (always scene-level under the implied panel
    /// context — there is no pane to back out of): the scene pops its topmost
    /// modal, and the map is the one this bench owns.
    fn apply_cancel(&mut self, cancelled: bool) {
        if cancelled && self.map_open {
            self.map_open = false;
        }
    }

    /// **COMMIT the planet** — capture the world into a v2 `.epoch`
    /// ([`flicker_worldengine::PlanetEpoch`]) and write it under
    /// `<staging_root>/worlds/`. An ingest bench writes to `staging/` and
    /// STOPS (the sablework contract): nothing is visible to the running
    /// game until the Content Manager reviews and promotes it.
    ///
    /// The era pauses and any queued leap stands down first — a committed
    /// planet is a world at rest — and an open procedure cycle is run to its
    /// close so the capture stands BETWEEN cycles (mid-cycle carry state has
    /// no place in the file). The result line lands on
    /// [`ui::COMMIT_STATUS_BIND`] either way: the staged path, or the error
    /// — never silence.
    fn commit_to(&mut self, staging_root: &std::path::Path) {
        self.evolve_running = false;
        self.roll_until = 0;
        self.roll_from = 0;
        let mid_cycle = self.evolve.current_phase() != flicker_worldengine::PHASES[0];
        while self.evolve.current_phase() != flicker_worldengine::PHASES[0] {
            let Self {
                map,
                seams,
                crust,
                evolve,
                ..
            } = &mut *self;
            let sea = evolve.resolve_sea();
            evolve.tick_phase(map, seams, crust, sea);
        }
        if mid_cycle {
            // The close-out moved material: the views and the hex column
            // follow the data, same as the step button's tick.
            self.drift_fields();
            self.bake_view(WorldView::Evolve);
            self.publish_hex();
        }
        let comment = format!(
            "Populous world: freq {}, seed {:#018x}, {} ticks.",
            self.map.freq(),
            self.seams.seed(),
            self.evolve.ticks()
        );
        let file = self.evolve.capture(&self.map, &self.seams, comment);
        // Identity in the name — recipe + clock — so successive commits of a
        // world stand side by side instead of silently overwriting.
        let name = format!(
            "planet_f{}_s{:016x}_t{}.epoch.gz",
            self.map.freq(),
            self.seams.seed(),
            self.evolve.ticks()
        );
        let path = staging_root.join("worlds").join(name);
        self.commit_status = match file.save(&path) {
            Ok(()) => path.display().to_string(),
            Err(e) => e.to_string(),
        };
    }
}

/// The PROCEDURE label's stringtable token for a pipeline phase — published
/// on a bind, resolved by the walker like any `$token` (the godmode pattern).
fn phase_token(p: flicker_worldengine::Phase) -> &'static str {
    use flicker_worldengine::Phase;
    match p {
        Phase::Climate => "$pop_phase_climate",
        Phase::Weather => "$pop_phase_weather",
        Phase::Upwell => "$pop_phase_upwell",
        Phase::Spread => "$pop_phase_spread",
        Phase::Collide => "$pop_phase_collide",
        Phase::Push => "$pop_phase_push",
        Phase::Form => "$pop_phase_form",
        Phase::Erode => "$pop_phase_erode",
        Phase::Compact => "$pop_phase_compact",
        Phase::Weld => "$pop_phase_weld",
    }
}

/// One AUTHORED colour out of the scene's own styles: a dotted path (`plot.line`)
/// into the token-resolved style tree, as the rgba a surface filler draws with.
/// This is the five-line split at the seam — `ui_theme.json` holds the palette, the
/// scene's `styles` block names which token each element wears, and the Rust filler
/// receives finished numbers and owns no colour of its own. A path that does not
/// resolve warns and comes back TRANSPARENT: the element then draws nothing, which
/// is the loud answer — never a stand-in colour nobody authored.
fn style_rgba(styles: &serde_json::Value, path: &str) -> [f32; 4] {
    let mut cur = styles;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => {
                tracing::warn!("populous: no style at `{path}` — that ink stays unset");
                return [0.0; 4];
            }
        }
    }
    match cur.as_array() {
        Some(a) if a.len() >= 4 => {
            let mut out = [0.0f32; 4];
            for (i, c) in a.iter().take(4).enumerate() {
                out[i] = c.as_f64().unwrap_or(0.0) as f32;
            }
            out
        }
        _ => {
            tracing::warn!(
                "populous: style `{path}` is not a resolved rgba — that ink stays unset"
            );
            [0.0; 4]
        }
    }
}

/// `92162` → `92,162` — thousands separators for the stat readouts. Formatting
/// lives in the scene and rides a Model bind (docs/ui-authoring: pre-format in
/// Rust, publish on a bind); a node never carries a composed number.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Scene for PopulousBench {
    /// The PLAY-N count field owns the keyboard while its session is open: the pump then
    /// resolves only the text exits and every other key reaches the field as text.
    fn input_context(&self) -> Option<InputContext> {
        self.ui_state
            .text_entry()
            .then_some(InputContext::TextEntry)
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
        let theme = Theme::build(renderer);
        // The same texture list feeds the script host (names → ids) and
        // `render_hud` (ids → handles); registering one without the other is how
        // a sprite resolves to a name that draws nothing.
        let entries = theme.lua_textures();
        self.textures = entries.iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.world.free(renderer);
        self.hex.free(renderer);
        self.worldmap.free(renderer);
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        if let Some((_map, look, _gp)) = flicker_shell::take_pending_input() {
            // The pump owns the key rebind now (S1c, non-draining); this scene keeps only
            // the player's LOOK controls (sensitivity + invert) for its globe camera — they
            // used to be discarded here while the globe turned at a private rate. The pad
            // deadzone rides the pump's `signals.axis`, so the gamepad is no longer ours.
            self.world.set_controls(look);
            self.hex.set_controls(look);
        }

        let screen = renderer.size();
        // The scene is DATA: walk the STATIC tree (built once in `new`). `arrange()`
        // reads the two-way-bound page/tab from the model and returns which slice is
        // lit; fold those visibility binds in so the walker draws the right one.
        // `ui_intents` was read off the static tree once, in `new`.
        let mut model = self.model();
        if let Err(e) = self.script.set_model(&model) {
            tracing::error!("populous: publishing the model to the script failed: {e}");
        }
        match self.script.arrange() {
            Ok(Some(arrangement)) => model.extend(arrangement.to_model()),
            Ok(None) => {}
            Err(e) => tracing::error!("populous: arrange() failed: {e}"),
        }
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen,
            // The OS text stream rides through every frame; `fold_typed` is
            // focus-gated, so it lands only while a text_field holds the
            // walker's keyboard focus (the PLAY-N count) and is dropped
            // otherwise. No TextEntry choreography: this bench binds no
            // letter keys, so typing collides with nothing.
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(
            &self.tree,
            &model,
            &self.ui_styles,
            &snap,
            &mut self.ui_state,
        );
        let over_hud = frame.results.is_on("hud_hit");
        // The walker RESERVES the viewport's rect and never fills it — hand it to the
        // world here, which declares its surface into that rect in `render`. The two
        // centre-pane views seat from their own gated surfaces: the page that is dark
        // reserved nothing, so its world seats `None` and costs nothing.
        self.world.seat(frame.surface(ui::VIEW_SLOT));
        self.hex.seat(frame.surface(ui::HEX_SLOT));
        // The map modal's surface: reserved only while the modal's slice is lit,
        // so a closed map seats `None` and costs nothing.
        self.worldmap.seat(frame.surface(ui::MAP_SLOT));
        // The CLIMATE HISTORY readout: the same hand-off, for a 2D filler — the
        // walker reserves the well under the climate gauge (and reserves NOTHING
        // while the evolve slice is dark, so the plot is unseated and costs nothing
        // on every other tab), and the filler draws the ring into it below.
        self.climate_plot.seat(frame.surface(ui::TEMP_PLOT_SLOT));
        let map_pointer = frame.surface_pointer(ui::MAP_SLOT).cloned();
        // The pointer SAMPLE for the globe's surface — the walker's barrier (A8C9F02B
        // §4b): present while the cursor is over the planet with no UI over it, or while
        // a press that began there is still held. The scene reads no device for it.
        let pointer = frame.surface_pointer(ui::VIEW_SLOT).cloned();
        let hex_pointer = frame.surface_pointer(ui::HEX_SLOT).cloned();
        self.hud_commands = frame.commands;
        // The era's climate curve, layered over the walker's own draw at the slot's
        // band — clip-safe by construction, so it needs no scissor of its own.
        self.hud_commands
            .extend(self.climate_plot.commands(&self.climate_history));
        // THE ELEMENT BILLBOARDS (Aaron 2026-08-26): a super-tiny label a
        // hair above each vein node's column — the EXTRACTED element's symbol
        // (bauxite digs as Al, per canon A4), projected through the globe's
        // own camera into the seated viewport, culled to the facing side.
        // Chemical symbols are notation, not prose — no stringtable token.
        if self.shown_view == WorldView::Evolve && !self.hex_view() {
            if let Some(rect) = self.world.rect() {
                let cam = self.world.camera();
                let aspect = (rect.size.x / rect.size.y.max(1.0)).max(0.01);
                let vp = cam.view_projection(aspect);
                let toward = cam.position.normalize_or_zero();
                let w = self
                    .map
                    .tiles()
                    .next()
                    .map(|t| tile_width(self.map.direction(t), self.map.outline(t), RADIUS))
                    .unwrap_or(1.0);
                for (t, k) in self.evolve.vein_nodes() {
                    let dir = self.map.direction(t);
                    if dir.dot(toward) < 0.3 {
                        continue; // the far side keeps its secrets
                    }
                    let lift = (self.evolve.ground(t) + 1.2) * w;
                    let ndc = vp.project_point3(dir * (RADIUS + lift));
                    if !(-1.0..=1.0).contains(&ndc.x)
                        || !(-1.0..=1.0).contains(&ndc.y)
                        || !(0.0..=1.0).contains(&ndc.z)
                    {
                        continue;
                    }
                    let kind = &flicker_worldengine::vein_kinds()[k as usize];
                    self.hud_commands.push(HudCommand::Text {
                        x: rect.pos.x + (ndc.x * 0.5 + 0.5) * rect.size.x,
                        y: rect.pos.y + (0.5 - ndc.y * 0.5) * rect.size.y,
                        text: kind.label.clone(),
                        size: 9.0,
                        color: [kind.ink[0], kind.ink[1], kind.ink[2], 0.95],
                        layer: 0.9,
                        align: flicker::script::TextAlign::Center,
                        font: flicker::script::FontRole::Label,
                        italic: false,
                        bold: true,
                        tracking: 0.0,
                        wrap: None,
                    });
                }
            }
        }

        // ── The input seam (input-P3, 0569DA9B): the PUMP resolved this frame's
        // World-context events — the scene owns no Resolver. Dispatch `signals.events`
        // through the [walker, world] chain via `signals.route`; the walker layer consumes
        // the screen's declared `on_menu` intent (so the pause never reads a raw key), and
        // the globe below it takes what is left of the look/zoom edges while its panel
        // holds the cursor. The runner reconciles the route's context requests after
        // `update`; the walker writes focus directly during dispatch, so there is nothing
        // to apply here. ──

        // UNCONDITIONAL: the walker owns the focus graph on every frame, so the
        // left stick can reach every panel and the d-pad can reach every control
        // inside the focused one. A scene that switched navigability on and off
        // would be deciding what a signal means, which is the walker's job.
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&self.tree, &model)
            .with_intents(&self.ui_intents);
        {
            // The worlds sit BELOW the walker: navigation is decided first, and
            // what is left of the look/zoom signals belongs to whichever view is
            // on screen while its panel holds the cursor (the dark page's view
            // never owns the camera, so it passes).
            let mut chain: [&mut dyn InputHandler; 3] =
                [&mut walker, &mut self.world, &mut self.hex];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }

        // Fold the fired intent names in beside the click results, so both
        // channels reach the ONE dispatcher identically.
        let mut results = frame.results;
        for name in walker.take_fired() {
            results.set(name, true);
        }
        // Cancel with NO pane entered is the walker's "pop the scene's modal"
        // seam (`WalkerHandler::cancelled`) — with the map open, that modal is
        // the map, so the pad backs out of it exactly as Close does. (Read out
        // before the walker's `ui_state` borrow ends with it.)
        let cancelled = walker.cancelled();
        drop(walker);
        self.apply_cancel(cancelled);
        self.apply_results(&results);

        // One camera line, and it is the WORLD's: the look and zoom come from the PUMP's
        // continuous queries (`signals.axis`, never a device), and the globe answers them
        // only while its own panel holds the walker's cursor, taking the pointer only
        // through the walker's surface capture. The six Look/Zoom signals stay the camera's
        // (`look_from`).
        let dtf = dt.as_secs_f32();
        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        // The globe answers look/zoom while its pane is the FOCUSED pane (the implied
        // panel context, Aaron 2026-09-02): the pane wearing the sapphire rim IS the
        // context — the left stick switches it, no Confirm to enter, no Cancel to leave.
        // The focused pane's `tab_group` IS the gate the globe matches against
        // (`in_panel`); the walker owns it, the scene only reads it, never a second focus
        // system (F2). Focusing a DIFFERENT pane yields that pane's group, so the globe
        // correctly stays quiet. Both centre-pane views name the same pane — the
        // SELECTED PAGE decides which of the two the focused pane hands the camera to,
        // and the dark one holds still.
        let look_gate = self.ui_state.focused_pane();
        let still = ((0.0, 0.0, 0.0), None::<&str>);
        // The OPEN MAP owns the look channel (contract FF8A575D): the modal
        // stands over the panes, so the globes hold still under it and the same
        // stick that flies the planet pans the flat sheet.
        let ((w_look, w_gate), (h_look, h_gate)) = if self.map_open {
            (still, still)
        } else if self.hex_view() {
            (still, (look, look_gate))
        } else {
            ((look, look_gate), still)
        };
        self.world.update(dtf, pointer.as_ref(), w_look, w_gate);
        self.hex.update(dtf, hex_pointer.as_ref(), h_look, h_gate);
        self.worldmap.update(
            dtf,
            map_pointer.as_ref(),
            if self.map_open { look } else { (0.0, 0.0, 0.0) },
            self.map_open,
        );
        // A map PICK moves the shared centre cell: the hex inspector follows
        // the tile clicked on the sheet, exactly as it follows the reticle. A
        // pick outside the tiling is a content/consumer disagreement — warned
        // and dropped, never clamped to a cell nobody chose (rule 4BB12A75).
        if let Some(picked) = self.worldmap.take_pick() {
            if picked >= self.map.len() as u64 {
                tracing::warn!(
                    "populous: map pick {picked} is outside the tiling ({} cells) — ignored",
                    self.map.len()
                );
            } else {
                let t = picked as TileId;
                if t != self.focus_tile {
                    self.focus_tile = t;
                    if self.reticle_view() {
                        self.highlight = Some(t);
                        self.apply_overlays();
                    }
                    self.publish_hex();
                }
            }
        }

        // The fixed reticle: on the LAYER tabs (seams + crust), the cell
        // facing the camera is the CENTRE cell — outlined bold on the globe,
        // and the hex page's column follows it. Off those tabs the ring comes
        // down.
        if self.reticle_view() {
            if let Some(f) = self.world.facing(&self.map.grid().dirs) {
                let f = f as TileId;
                if self.highlight != Some(f) {
                    self.highlight = Some(f);
                    self.focus_tile = f;
                    self.apply_overlays();
                    self.publish_hex();
                }
            }
        } else if self.highlight.take().is_some() {
            self.apply_overlays();
        }

        // The era's clock (the 2026-08-29 tick contract): while RUNNING the
        // engine computes full cycles flat-out under a per-frame budget and
        // draws NOTHING new — no bake, no overlays, no hex publish, no other
        // visual work in the procedural loop; the readouts (tick counter,
        // gauges, the PLAY-N bar) stay live off the per-frame Model. The one
        // bake lands when the run RESTS: PLAY-N's arrival here, or the pause
        // click in `apply_results`.
        if self.shown_view == WorldView::Evolve && self.evolve_running {
            let start = std::time::Instant::now();
            while (self.roll_until == 0 || self.evolve.ticks() < self.roll_until)
                && start.elapsed().as_millis() < RUN_FRAME_MS
            {
                self.tick_era();
            }
            if self.roll_until > 0 && self.evolve.ticks() >= self.roll_until {
                self.run_complete(); // PLAY-N arrived: the run's one bake
            }
        }

        // Menu opens the shell's pause overlay — quit, settings, back to the
        // menu. The screen DECLARED `on_menu`; the arm lives here rather than in
        // `apply_results` because it returns a Transition.
        if results.is_on("pause_open") {
            if let Some(theme) = self.theme {
                // The scene owns no bindings; take the World map from the shared profile
                // for the pause overlay (it binds Menu, so Esc resumes).
                let pause_map = flicker_shell::input_profile()
                    .context_map("World")
                    .cloned()
                    .unwrap_or_else(InputMap::wasd_and_mouse);
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    &pause_map,
                    &AbstractControls::default(),
                    &GamepadConfig::default(),
                )));
            }
        }
        // The stick tried to leave a pane holding pad-staged dial values: the shared
        // Apply / Revert prompt (Aaron 2026-09-04) — one call, the answer folds through
        // `modal_closed` below.
        if let Some(t) = flicker_shell::stage_prompt(self.theme, &mut self.ui_state) {
            return t;
        }
        Transition::None
    }

    /// The stage prompt's answer (Apply / Revert / Keep editing) folds into the walker
    /// state; nothing else opens a modal on this bench yet.
    fn modal_closed(&mut self, _modal: &str, result: &str, _payload: Option<&str>) {
        flicker_shell::stage_prompt_closed(&mut self.ui_state, result);
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // Declare-only: the globe goes into the rect the walker reserved, and the HUD
        // replay is the screen surface's final 2D — one overlay, run after the composites.
        // The destructure splits the disjoint borrows (`world` &mut, `hud_commands` /
        // `textures` shared) so both survive into the graph until the manager's `execute`.
        let Self {
            world,
            hex,
            worldmap,
            hud_commands,
            textures,
            ..
        } = self;
        let layer = fg.base_layer();
        world.render(renderer, fg, layer);
        hex.render(renderer, fg, layer);
        // The map composites at its own slot layer — the modal's lifted band —
        // so the sheet lands inside the popup's well, over the HUD scrim.
        worldmap.render(renderer, fg, layer);
        if let Some(&white) = textures.first() {
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
        }
    }
}

/// Build the bench as a boxed `Scene` — the CLIENT BEHAVIOUR the roster registers;
/// the manifest resolves `populous.scene.json` and hands its def here.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(PopulousBench::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::script::{UiNode, Value};
    // Test-only: the gates load the raw theme without a scene-styles overlay;
    // the lib itself always merges through `load_shared_styles`.
    use flicker::ui::load_shared_styles;
    // Test-only now: the tests synthesize events with a real resolver to drive the walker
    // chain end-to-end; the scene itself owns none of this any more (input-P3).
    use flicker_input_core::{
        ActionSignal, ContextualBindings, EventKind, Fired, InputContext, Resolver,
    };
    use flicker_input_router::{InputEvent, RouteCtx};

    fn flatten(n: &UiNode) -> Vec<&UiNode> {
        let mut out = vec![n];
        let mut i = 0;
        while i < out.len() {
            let node = out[i];
            for c in &node.children {
                out.push(c);
            }
            i += 1;
        }
        out
    }

    /// **The whole surface resolves.** Every component kind is registered and no raw
    /// English reaches a draw — the two drift gates every migrated bench carries (the
    /// template tier that once needed a third gate is gone — 201F4F51).
    #[test]
    fn the_screen_names_known_kinds_with_no_raw_strings() {
        let tree = test_bench().build_tree();
        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(unknown.is_empty(), "{unknown:?}");
        let raw = flicker::ui::raw_display_literals(&tree);
        assert!(raw.is_empty(), "{raw:?}");
    }

    /// **THE BENCH IS EXACTLY THE CATALOG, and nothing else.** Aaron's list —
    /// Frame, PTT, Slider, Button, Multi-View, RTT Panel, UI Panel, localized
    /// strings — is the whole vocabulary this screen may draw, and this gate
    /// asserts the inverse too: every component kind in the expanded tree is on
    /// the list, on BOTH tabs. A stray `cell` column, a hand-built readout or a
    /// borrowed bench widget fails here, which is what makes "the entire
    /// Populous Bench should be that" a build result rather than a wish.
    ///
    /// The list, and why each name is on it: `screen` is the root the
    /// `default_page` proto emits; `grid` / `cell` / `row` / `stack` are what
    /// the FRAME (Aaron's own "Frame (9-grid)" — the frame builder emits a
    /// three-column `grid` of cells) and the PTT are MADE of, structure rather
    /// than content (the window stack's corner runes ride its `runes` flag,
    /// decoration not a kind); `tabs` + `pill_toggle` + `button` are
    /// the PTT's two rails and its four glyph hints; `panel` is the UI Panel and
    /// the RTT Panel (one component, two protos); `surface` is the root screen
    /// AND the hex world's nested viewport (one kind at two depths);
    /// `slider` is the size dial; `text` and `option` carry the localized
    /// strings.
    #[test]
    fn the_bench_is_exactly_the_catalog_and_nothing_else() {
        /// Aaron's catalog, expanded to the component kinds it is built from.
        const CATALOG: &[&str] = &[
            "surface", // the root screen AND the hex world's viewport — one kind
            "cell",
            "row",
            "stack",
            // (corner runes are the `runes` FLAG on the window stack now — a
            // decoration, not a kind — so they no longer appear in the census)
            "paged_menu", // the PTT — a native Component (rails/hints/rule drawn by Rust)
            "tabs",
            "pill_toggle",    // the PTT's authored page + tab rails
            "panel",          // UI Panel and RTT Panel
            "slider",         // the size dial
            "checkbox",       // the evolve tab's lenses (arrows + the three fluids)
            "button",         // the seams action
            "text_field",     // PLAY-N's tick-count entry (the tick contract)
            "resource_gauge", // the bootstrap roll's progress bar (the loading bar component)
            "nav_footer",     // the bench nav band (map/menu/back/next — commit on evolve)
            "popup_panel",    // the world-map modal's carved slab (contract FF8A575D)
            "text",
            "option", // localized strings
        ];
        for tab in [0.0, 1.0] {
            let mut bench = test_bench();
            let mut r = ValueMap::default();
            r.set(ui::TAB_BIND, tab);
            bench.apply_results(&r);
            let tree = bench.build_tree();
            let mut kinds: Vec<&str> = flatten(&tree)
                .iter()
                .map(|n| n.component.as_str())
                .collect();
            kinds.sort_unstable();
            kinds.dedup();
            for k in &kinds {
                assert!(
                    CATALOG.contains(k),
                    "tab {tab}: `{k}` is not in the catalog"
                );
            }
        }
        // …and the catalog is not aspirational: on the MAP tab every interactive
        // member of it is actually PRESENT.
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        let count = |kind: &str| all.iter().filter(|n| n.component == kind).count();
        assert_eq!(count("paged_menu"), 1, "the PTT is ONE Component");
        assert_eq!(count("tabs"), 1, "the PTT's page rail");
        assert_eq!(
            count("pill_toggle"),
            ui::PAGES.len(),
            "one tab rail per page, gated apart (the catalog's per-page pattern)"
        );
        assert_eq!(count("panel"), 3, "two UI Panels and one RTT Panel");
        assert_eq!(
            count("surface"),
            5,
            "the root surface + the globe's viewport + the hex stack's viewport + \
             the map modal's sheet + the climate history's plot well"
        );
        assert_eq!(
            count("slider"),
            5,
            "the size, cells, spots, water-target and green-target dials (coverage + climate are right-pane gauges now)"
        );
        // The rail hints (lt/rt/lb/rb) and the rule are now drawn BY the `paged_menu`
        // Component, not authored tree nodes — so NOTHING on the surface wears a glyph.
        // The hint→step behaviour is gated in flicker-widgets (`draw_paged_menu` /
        // `hit_paged_menu`); here we assert only the stray-node inverse.
        assert!(
            !all.iter().any(|n| n.props.contains_key("glyph")),
            "the PTT hints are component-internal, not authored glyph buttons"
        );
        // The three panes, each the ONE panel component, and the viewport wired
        // to its authored stage (the default light source).
        for id in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE] {
            let p = all
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("{id} is placed"));
            assert_eq!(p.component, "panel", "{id} is the ONE panel component");
        }
        let view = all
            .iter()
            .find(|n| n.id == ui::VIEW_SLOT)
            .expect("the viewport is placed");
        assert_eq!(view.component, "surface", "the centre pane is the viewport");
        assert_eq!(
            view.props.get("source"),
            Some(&Value::Text(ui::STAGE_SOURCE.to_string())),
            "the viewport names the stage that lights it"
        );
        // EVERY word on the surface is a `$token`, and NOT ONE number is
        // composed into a node: the three stat values arrive on Model binds
        // (`text_bind`), which is why there is no literal left to check.
        let texts: Vec<&&UiNode> = all.iter().filter(|n| n.component == "text").collect();
        for t in &texts {
            if let Some(Value::Text(s)) = t.props.get("text") {
                assert!(s.starts_with('$'), "`{s}` must be a stringtable token");
            }
        }
        let mut bound: Vec<String> = texts
            .iter()
            .filter_map(|n| match n.props.get("text_bind") {
                Some(Value::Text(t)) => Some(t.clone()),
                _ => None,
            })
            .collect();
        bound.sort_unstable();
        // The expected roster: the fixed readouts plus the census table's
        // authored two-column rows, generated from the one row count.
        let mut want: Vec<String> = [
            ui::DIAMETER_BIND,
            ui::HEXES_BIND,
            ui::PHASE_BIND,
            ui::STRATA_BIND,
            ui::TICKS_BIND,
            ui::TILE_BIND,
        ]
        .iter()
        .map(|b| b.to_string())
        .collect();
        for i in 0..ui::CENSUS_ROWS {
            want.push(ui::census_name_bind(i));
            want.push(ui::census_count_bind(i));
        }
        // …plus the hex inspector's two panes, from the one shared roster,
        // and the two right-pane readout gauges' percent texts.
        for b in ui::HEX_MAT_BINDS.iter().chain(&ui::HEX_FLUID_BINDS) {
            want.push((*b).to_string());
        }
        want.push(ui::WATER_VAL_BIND.to_string());
        want.push(ui::TEMP_VAL_BIND.to_string());
        want.push(ui::GREEN_VAL_BIND.to_string());
        // …plus the nav footer's commit result line.
        want.push(ui::COMMIT_STATUS_BIND.to_string());
        want.sort_unstable();
        assert_eq!(
            bound, want,
            "every readout is a bind the scene pre-formats, never a node's own string"
        );
        // The size dial: bound over the offered range, focusable (the pad
        // channel's gate), and wearing the label the proto carries.
        let dial = all
            .iter()
            .find(|n| n.component == "slider")
            .expect("the dial");
        assert_eq!(
            dial.props.get("vertical"),
            Some(&Value::Bool(true)),
            "upright"
        );
        assert_eq!(dial.bind.as_deref(), Some(ui::FREQ_BIND));
        assert_eq!(dial.props.get("min"), Some(&Value::Number(48.0)));
        assert_eq!(dial.props.get("max"), Some(&Value::Number(120.0)));
        assert_eq!(
            dial.props.get("label"),
            Some(&Value::Text("$pop_size".into()))
        );
        assert_eq!(dial.props.get("step"), Some(&Value::Number(1.0)));
        assert_eq!(dial.props.get("step_coarse"), Some(&Value::Number(10.0)));
        assert!(
            !dial.tab_group.is_empty(),
            "walker-focusable — the pad channel's gate"
        );
        // NOT ONE style string reaches a node from this crate: every skin the
        // surface wears is named by a proto, in data.
        for n in &all {
            if let Some(Value::Text(s)) = n.props.get("style") {
                assert!(
                    !s.contains("sablework") && !s.contains("assetpipeline"),
                    "`{s}` is another bench's palette"
                );
            }
        }
        // The right pane displays and never interacts: its whole subtree is the
        // pane plus the readout rows — focusing it changes no control, because
        // there are none to change.
        fn subtree<'a>(n: &'a UiNode, out: &mut Vec<&'a UiNode>) {
            out.push(n);
            for c in &n.children {
                subtree(c, out);
            }
        }
        let right = all
            .iter()
            .find(|n| n.id == ui::RIGHT_PANE)
            .expect("right pane");
        let mut nodes = Vec::new();
        subtree(right, &mut nodes);
        assert!(
            nodes.iter().all(|n| matches!(
                n.component.as_str(),
                // `surface` joins the list as the climate history's well: a
                // reserved RECT, not a control — it takes no focus and answers no
                // signal, so the pane is still display-only with it in.
                "panel" | "cell" | "row" | "text" | "resource_gauge" | "surface"
            )),
            "the stats pane is display-only (the gauge shows, never interacts)"
        );
        // ONE set of corner runes: the page chrome's — the `runes` DECORATION
        // FLAG on the window stack (the standalone kind is gone, 2026-08-14).
        // The inner content frame stays bare, so the corners never stack.
        let flagged: Vec<&&UiNode> = all
            .iter()
            .filter(|n| matches!(n.props.get("runes"), Some(Value::Bool(true))))
            .collect();
        assert_eq!(flagged.len(), 1, "exactly one slab wears the corner runes");
        assert_eq!(
            flagged[0].component, "stack",
            "…and it is the page chrome's window stack"
        );
    }

    /// **The panes are PANELS, and the walker owns which one has the cursor.**
    /// Three `panel` nodes on the NESTED-pane convention (Aaron 2026-08-15): a
    /// container carries NO self-membership marker — its members claim it (the
    /// dial claims the left pane) or it authors `pane: true` (the viewport and
    /// stats panes, which have no focusable interior) — and its `nav_ordinal` is
    /// the AUTHORED stick-stop order. The wrapping, the rim and the enter/exit
    /// live in the walker and the `panel` component.
    #[test]
    fn the_panes_are_panels_the_walker_can_cycle() {
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        let panes: Vec<&&UiNode> = all.iter().filter(|n| n.component == "panel").collect();
        assert_eq!(panes.len(), 3, "three panes, all the ONE panel component");
        for (i, id) in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE]
            .iter()
            .enumerate()
        {
            let p = panes[i];
            assert_eq!(&p.id, id, "panes keep authoring order");
            assert!(
                p.tab_group.is_empty(),
                "a container carries no self-membership marker"
            );
            assert_eq!(
                p.nav_ordinal,
                i as u32 + 1,
                "the stick-stop order is authored"
            );
            assert!(
                !p.props
                    .keys()
                    .any(|k| k == "focused" || k.ends_with("_style")),
                "{id}: the scene passes no rim and no focus flag"
            );
        }
        // The two panes with no focusable interior author the explicit marker;
        // the left pane is claimed by its dial and needs none.
        for id in [ui::VIEW_PANE, ui::RIGHT_PANE] {
            let p = all.iter().find(|n| n.id == id).expect("pane");
            assert!(
                matches!(p.props.get("pane"), Some(Value::Bool(true))),
                "{id} authors `pane: true` (nothing claims it)"
            );
        }
        // …and the ONE interactive control inside a pane sits ABOVE the pane in
        // its own group, so the panel cursor lands on the pane and the d-pad
        // reaches the control. This asserts the COMPOSED result only — what the
        // dial itself is (its range, its two pad step sizes, its skin, its own
        // default ordinal) is the proto's contract and is gated beside the proto:
        // flicker-widgets `the_size_dial_proto_carries_the_whole_control_contract`.
        let dial = all
            .iter()
            .find(|n| n.component == "slider")
            .expect("the dial");
        assert_eq!(dial.tab_group, ui::LEFT_PANE);
        assert!(
            dial.nav_ordinal > 0,
            "the control follows its panel in the group"
        );
    }

    /// **Both rails carry the roster.** One entry per page and one per tab of
    /// the selected page — read from [`ui::PAGES`], never a written-down count.
    #[test]
    fn the_rails_are_built_from_the_roster() {
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        let rail = |id: &str| {
            all.iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("{id} is placed"))
                .children
                .len()
        };
        assert_eq!(rail("paged_pages"), ui::PAGES.len(), "one entry per page");
        assert_eq!(
            rail("paged_tabs"),
            ui::PAGES[0].tabs.len(),
            "one entry per world tab"
        );
        assert_eq!(
            rail("paged_tabs_p1"),
            ui::PAGES[1].tabs.len(),
            "one entry per hex tab, on that page's own gated rail"
        );
    }

    /// **The rails step THEMSELVES, and the scene only reads the result.** Each
    /// strip authors the `next_action` / `prev_action` naming the very result the
    /// neighbouring hint button fires, so a click, a shoulder press and a pad
    /// Confirm all converge on one numeric index write — which is the only thing
    /// this bench dispatches. (The stepping itself, over the real `paged_menu`,
    /// is gated in flicker-widgets: `a_rail_hint_press_steps_the_strip_by_one_and_clamps`.)
    #[test]
    fn the_rails_own_their_stepping_and_the_scene_reads_the_index() {
        let mut bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        for (id, next, prev) in [
            ("paged_pages", "page_next", "page_prev"),
            ("paged_tabs", "tab_next", "tab_prev"),
        ] {
            let rail = all
                .iter()
                .find(|n| n.id == id)
                .unwrap_or_else(|| panic!("{id} is placed"));
            assert_eq!(
                rail.props.get("next_action"),
                Some(&Value::Text(next.into()))
            );
            assert_eq!(
                rail.props.get("prev_action"),
                Some(&Value::Text(prev.into()))
            );
        }
        // The scene has NO stepper: a fired step name it happens to see moves
        // nothing — only the index the strip wrote does.
        let mut named = ValueMap::default();
        named.set("tab_next", true);
        bench.apply_results(&named);
        assert_eq!(bench.selection(), (0, 0), "the scene owns no stepping");
        let mut wrote = ValueMap::default();
        wrote.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&wrote);
        assert_eq!(
            bench.selection(),
            (0, 1),
            "the strip's index is what moves the tab"
        );
    }

    /// **Every page and tab's slices live in ONE static tree, over the SAME panes,
    /// gated apart.** A selection no longer rebuilds structure — `build_tree` is
    /// identical whatever the selection; the MAP slice (the size dial + stats), the
    /// SEAMS slice (the cells dial + randomize button), and the HEX page (its column
    /// viewport + placeholder panes) coexist, each gated on its `shown_*` key over
    /// the SAME three panes. WHICH slice shows is `arrange()`'s job
    /// (`arrange_lights_the_selected_tabs_slice`), never the tree's — the point of
    /// the shared-panes arrangement.
    #[test]
    fn every_slice_shares_the_panes_and_is_gated_apart() {
        let bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        assert!(flicker::ui::unknown_kinds(&tree).is_empty());
        assert!(flicker::ui::raw_display_literals(&tree).is_empty());

        // The WORLD page's viewport, wired to its stage — shared by that page's
        // two tabs, gated at PAGE level (the hex page shows its own view there).
        let view = all
            .iter()
            .find(|n| n.id == ui::VIEW_SLOT)
            .expect("the viewport is placed");
        assert_eq!(view.component, "surface", "the centre pane is the viewport");
        assert_eq!(
            view.props.get("source"),
            Some(&Value::Text(ui::STAGE_SOURCE.to_string())),
            "the one authored stage lights it"
        );
        // The HEX page's viewport beside it, wired to the hex stage.
        let hex = all
            .iter()
            .find(|n| n.id == ui::HEX_SLOT)
            .expect("the hex viewport is placed");
        assert_eq!(hex.component, "surface", "the hex view is a surface too");
        assert_eq!(
            hex.props.get("source"),
            Some(&Value::Text(ui::HEX_STAGE_SOURCE.to_string())),
            "the hex stage lights it"
        );

        // The one three-pane arrangement, all the `panel` component, left/view/right.
        let panes: Vec<&&UiNode> = all.iter().filter(|n| n.component == "panel").collect();
        assert_eq!(
            panes.len(),
            3,
            "one three-pane arrangement, shared by every page and tab"
        );
        for (i, id) in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE]
            .iter()
            .enumerate()
        {
            assert_eq!(&panes[i].id, id, "the panes keep left/view/right order");
        }

        // ALL slices are DECLARED in the one tree — the selection lights one, never
        // adds or removes structure. The map slice's dial, the seams slice's dial +
        // button, and the hex page's inspector rows coexist.
        assert!(
            all.iter()
                .any(|n| n.component == "slider" && n.bind.as_deref() == Some(ui::FREQ_BIND)),
            "the map slice's size dial is declared"
        );
        assert!(
            all.iter()
                .any(|n| n.component == "slider" && n.bind.as_deref() == Some(ui::CELLS_BIND)),
            "the seams slice's cells dial is declared"
        );
        assert!(
            all.iter()
                .any(|n| n.action.as_deref() == Some(ui::SEAMS_ACTION)),
            "the seams slice's randomize button is declared"
        );
        let placeholders = all
            .iter()
            .filter(
                |n| matches!(n.props.get("text"), Some(Value::Text(t)) if t == "$ui_pane_empty"),
            )
            .count();
        assert_eq!(
            placeholders, 3,
            "the seams stats pane and both crust panes rest on placeholders (the hex page's side panes are the inspector now)"
        );

        // The slices are gated on DIFFERENT keys, so a selection lights exactly one
        // of each pane's interiors — and the two centre views split at page level.
        let gates: std::collections::HashSet<&str> = all
            .iter()
            .filter_map(|n| n.visible_bind.as_deref())
            .collect();
        for key in [
            "shown_page0",
            "shown_page1",
            "shown_p0_t0",
            "shown_p0_t1",
            "shown_p0_t2",
            "shown_p0_t3",
            "shown_p1_t0",
            // The nav footer's NEXT/COMMIT swap rides the same mechanism.
            "shown_ft_next",
            "shown_ft_commit",
        ] {
            assert!(gates.contains(key), "`{key}` gates its slice");
        }
    }

    /// **The planet size is ONE world, shared by every tab** (Aaron's ruling).
    /// The dial is shown on the map tab alone, but its committed number writes
    /// the scene's one `HexMap` — so a size set there IS the size the seams tab
    /// renders, and the value the dial reads back on return. A tab owns its
    /// view, never a copy of the world; this gate is what makes forking it per
    /// tab a failing build instead of a quiet refactor.
    ///
    /// The dial's own GESTURE is not tested here and no longer can be: the
    /// bench holds no live size and no stick reader. Mid-gesture behaviour —
    /// the knob tracking the hand while `results` still report the resting
    /// value, and the single write on release — belongs to the walker and is
    /// gated there (rules B694F6B1 commit-on-release + 3A04B4CE local display
    /// ownership; `slider_drag_captures_and_commits_on_release`). This scene
    /// sees one number, once, and rebuilds.
    #[test]
    fn the_planet_size_is_one_world_shared_by_every_tab() {
        let mut bench = test_bench();
        let tab = |i: f64| {
            let mut r = ValueMap::default();
            r.set(ui::TAB_BIND, i);
            r
        };

        // The dial's COMMITTED number on the MAP tab rebuilds the one world...
        let mut write = ValueMap::default();
        write.set(ui::FREQ_BIND, 72.0);
        bench.apply_results(&write);
        assert_eq!(bench.map().freq(), 72, "the dial's committed number lands");

        // ...and the SEAMS tab renders that same world — same map, same size;
        // there is no per-tab copy to drift.
        bench.apply_results(&tab(1.0));
        assert_eq!(bench.selection(), (0, 1), "on the seams tab");
        assert_eq!(
            bench.map().freq(),
            72,
            "the seams tab shows the resized world"
        );

        // Returning, the dial reads the SHARED value back out of the model —
        // the size did not reset with the tab.
        bench.apply_results(&tab(0.0));
        assert_eq!(bench.selection(), (0, 0));
        assert_eq!(
            bench.model().number(ui::FREQ_BIND),
            Some(72.0),
            "the dial shows the world's one size, not a per-tab memory"
        );

        // The bind ECHOES its resting value every frame; `resize` is a no-op at
        // the current size, so the echo rebuilds nothing. An out-of-range number
        // clamps into the offered range rather than reaching `HexMap`.
        bench.apply_results(&write);
        assert_eq!(bench.map().freq(), 72, "the resting echo is not a rebuild");
        let mut wild = ValueMap::default();
        wild.set(ui::FREQ_BIND, 9_000.0);
        bench.apply_results(&wild);
        assert_eq!(
            bench.map().freq(),
            MAX_FREQ,
            "a wild number clamps into the range"
        );
    }

    /// **A rail click jumps the selection directly** — the strip writes its
    /// bind with the clicked entry's index and the dispatcher consumes it. The
    /// every-frame echo is inert: only a CHANGED value moves anything, so the
    /// page echo can never re-fire the arm whose side effect resets the tab.
    #[test]
    fn a_rail_click_write_jumps_the_selection_and_echoes_are_inert() {
        let mut bench = test_bench();
        // A wild write clamps into the roster and lands — one click, no steps.
        let mut wild = ValueMap::default();
        wild.set(ui::TAB_BIND, 9.0);
        bench.apply_results(&wild);
        let last = ui::PAGES[0].tabs.len() - 1;
        assert_eq!(
            bench.selection(),
            (0, last),
            "clamped into the roster and applied"
        );
        // The resting echo (both binds, current values) changes nothing.
        let mut echo = ValueMap::default();
        echo.set(ui::PAGE_BIND, 0.0);
        echo.set(ui::TAB_BIND, last as f64);
        bench.apply_results(&echo);
        assert_eq!(bench.selection(), (0, last), "echoes are inert");
        // A clamp-equal page write is NOT a page change — the tab survives.
        let mut pw = ValueMap::default();
        pw.set(ui::PAGE_BIND, 0.0);
        bench.apply_results(&pw);
        assert_eq!(bench.selection(), (0, last), "no page change, no tab reset");
        // A wild page write clamps into the roster and LANDS. The hex page
        // has one tab, so the world page's remembered tab CLAMPS to it while
        // there — and is RESTORED whole on return: the PTT retains each
        // page's tab between swaps (Aaron 2026-08-25).
        let remembered = bench.selection().1;
        let mut wild_page = ValueMap::default();
        wild_page.set(ui::PAGE_BIND, 9.0);
        bench.apply_results(&wild_page);
        assert_eq!(
            bench.selection(),
            (ui::PAGES.len() - 1, 0),
            "clamped to the last page, whose one tab is 0"
        );
        let mut home = ValueMap::default();
        home.set(ui::PAGE_BIND, 0.0);
        bench.apply_results(&home);
        assert_eq!(
            bench.selection(),
            (0, remembered),
            "back on the world page, ON THE TAB IT REMEMBERS"
        );
        let mut seams_tab = ValueMap::default();
        seams_tab.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&seams_tab);
        // A real click on the first pill jumps back.
        let mut back = ValueMap::default();
        back.set(ui::TAB_BIND, 0.0);
        bench.apply_results(&back);
        assert_eq!(bench.selection(), (0, 0));
    }

    /// **The PTT retains each page's tab between page swaps.** Stand the
    /// world page on a non-default tab, visit the hex page (tab 0 — its only
    /// one), come back: the world page is exactly where it was left. Each
    /// page's memory is its OWN — the hex page's selection never bleeds into
    /// the world page's — and the tab arm always writes the CURRENT page's
    /// memory, never another's.
    #[test]
    fn the_ptt_remembers_each_pages_tab_across_swaps() {
        let mut bench = test_bench();
        let go = |b: &mut PopulousBench, key: &str, v: f64| {
            let mut r = ValueMap::default();
            r.set(key, v);
            b.apply_results(&r);
        };
        go(&mut bench, ui::TAB_BIND, 3.0); // world → plates
        assert_eq!(bench.selection(), (0, 3));
        go(&mut bench, ui::PAGE_BIND, 1.0); // to the hex page
        assert_eq!(bench.selection(), (1, 0), "the hex page's own (only) tab");
        go(&mut bench, ui::PAGE_BIND, 0.0); // and back
        assert_eq!(bench.selection(), (0, 3), "the world page kept PLATES");
        // Moving the tab while on hex writes HEX's memory, not the world's.
        go(&mut bench, ui::PAGE_BIND, 1.0);
        go(&mut bench, ui::TAB_BIND, 0.0);
        go(&mut bench, ui::PAGE_BIND, 0.0);
        assert_eq!(
            bench.selection(),
            (0, 3),
            "another page's writes never bleed"
        );
    }

    /// **A mouse click on the tab rail's second pill switches to the seams
    /// tab** — the full pointer contract through the REAL component: the strip
    /// under the real styles, the Lua hit verdict carrying the entry's numeric
    /// value, the bind write in `results`, and the dispatcher's jump. The
    /// numeric click path was dead once (string-only gate); this drives it end
    /// to end so it cannot go quiet again.
    #[test]
    fn a_click_on_the_tab_rail_switches_to_the_seams_tab() {
        use flicker::render::Vec2;

        let mut bench = test_bench();
        let tree = bench.build_tree();
        let model = bench.model();
        let styles = load_shared_styles(None);
        let mut state = UiState::default();
        let snap = |mouse: Vec2, clicked: bool, down: bool| UiInput {
            mouse,
            clicked,
            down,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };

        // The rail is PAGE-gated now (one rail per page): light the world
        // page's, as arrange() does on page 0.
        let mut model = model;
        model.set("shown_page0", true);
        // Resolve the rail's rect, then click the centre of its SECOND pill.
        let idle = run_ui(
            &tree,
            &model,
            &styles,
            &snap(Vec2::ZERO, false, false),
            &mut state,
        );
        let rail = idle.rect("paged_tabs").expect("the tab rail resolves");
        // Four pills now — the SEAMS pill is the second quarter.
        let click = Vec2::new(
            rail.pos.x + rail.size.x * 0.375,
            rail.pos.y + rail.size.y * 0.5,
        );
        let f = run_ui(&tree, &model, &styles, &snap(click, true, true), &mut state);
        assert_eq!(
            f.results.number(ui::TAB_BIND),
            Some(1.0),
            "the pill's numeric value rides the bind write"
        );
        bench.apply_results(&f.results);
        assert_eq!(
            bench.selection(),
            (0, 1),
            "the click switched to the seams tab"
        );
    }

    /// **Five intents declared, every one with an arm — and NOT ONE the walker
    /// owns.** `on_menu` rides in from the `default_page` proto; the four rail
    /// intents through its optional params. The inverse is the load-bearing half:
    /// Confirm, Cancel, `Nav*` and `Panel*` are the walker's on every screen in
    /// Prism, and this bench declared four of them until 2026-08-09 — which,
    /// because the walker consumes a declared intent and returns BEFORE the
    /// activation path, statically killed every button on the screen (F1).
    #[test]
    fn the_screen_declares_only_what_it_owns_and_every_one_has_an_arm() {
        let tree = test_bench().build_tree();
        let mut declared: Vec<(String, String)> = tree
            .props
            .iter()
            .filter(|(k, _)| k.starts_with("on_"))
            .filter_map(|(k, v)| match v {
                Value::Text(t) => Some((k.clone(), t.clone())),
                _ => None,
            })
            .collect();
        declared.sort();
        assert_eq!(
            declared,
            [
                ("on_menu", "pause_open"),
                ("on_page_next", "page_next"),
                ("on_page_prev", "page_prev"),
                ("on_tab_next", "tab_next"),
                ("on_tab_prev", "tab_prev"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string())),
            "the declaration is exactly the set the scene owns"
        );
        // THE INVERSE — the channel the F1 defect travelled. Not one walker-owned
        // signal may be named, at the root or anywhere beneath it.
        for signal in [
            ActionSignal::Confirm,
            ActionSignal::Cancel,
            ActionSignal::NavUp,
            ActionSignal::NavDown,
            ActionSignal::NavLeft,
            ActionSignal::NavRight,
            ActionSignal::PanelNext,
            ActionSignal::PanelPrev,
            ActionSignal::ChordBegin,
        ] {
            assert_eq!(
                UiIntents::of(&tree).result_for(signal),
                None,
                "{signal:?} is the walker's — declaring it kills it on this screen"
            );
        }
        assert!(
            !flatten(&tree)
                .iter()
                .any(|n| n.props.keys().any(|k| matches!(
                    k.as_str(),
                    "on_confirm"
                        | "on_cancel"
                        | "on_nav_up"
                        | "on_nav_down"
                        | "on_nav_left"
                        | "on_nav_right"
                        | "on_panel_next"
                        | "on_panel_prev"
                        | "on_chord_begin"
                ))),
            "no node anywhere in the tree names a walker-owned signal"
        );
        assert_eq!(
            UiIntents::of(&tree).result_for(ActionSignal::Menu),
            Some("pause_open"),
            "the menu declaration names the signal the resolver actually fires"
        );
        for (_, name) in declared {
            if name == "pause_open" {
                continue; // handled in `update`, which returns a Transition
            }
            // Every rail intent is a STRIP's own step name — the control that
            // dispatches it, so a fired name can never fail to nothing.
            assert!(
                flatten(&tree).iter().any(|n| {
                    n.props.get("next_action") == Some(&Value::Text(name.clone()))
                        || n.props.get("prev_action") == Some(&Value::Text(name.clone()))
                }),
                "`{name}` is declared but no strip steps on it"
            );
        }
    }

    /// **The seams button's authored name reaches its ARM: a re-roll.** The
    /// button fires the name, the dispatcher answers it by re-rolling the
    /// molten heat field — the seams MOVE — while the world's size and the
    /// selection stand still. An authored name that failed to NOTHING is the
    /// difference between authorable and not (rule 4BB12A75).
    #[test]
    fn the_seams_button_fires_a_name_the_dispatcher_answers() {
        let mut bench = test_bench();
        let mut seams = ValueMap::default();
        seams.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&seams);

        // The button is in the tree, on the seams tab, firing exactly that name.
        let tree = bench.build_tree();
        let button = flatten(&tree)
            .into_iter()
            .find(|n| n.action.as_deref() == Some(ui::SEAMS_ACTION))
            .expect("the seams button fires the authored action");
        assert_eq!(
            button.component, "button",
            "a plain button, not a bench widget"
        );
        assert_eq!(
            button.tab_group,
            ui::LEFT_PANE,
            "it lives in the left pane's focus group"
        );

        // …and the dispatcher's arm re-rolls the field: a new seed, moved
        // seams — and NOTHING else moves underneath it.
        let before_freq = bench.map().freq();
        let before_seed = bench.seams().seed();
        let before_heat = bench.seams().heats().to_vec();
        let before_vents = bench.crust().vents().to_vec();
        let mut fired = ValueMap::default();
        fired.set(ui::SEAMS_ACTION, true);
        bench.apply_results(&fired);
        assert_ne!(bench.seams().seed(), before_seed, "a new roll");
        assert_ne!(
            bench.seams().heats(),
            &before_heat[..],
            "and the seams moved"
        );
        assert_ne!(
            bench.crust().vents(),
            &before_vents[..],
            "and the crust's vents moved with them"
        );
        assert_eq!(bench.map().freq(), before_freq, "the world's size held");
        assert_eq!(bench.selection(), (0, 1), "…and it is not a navigation");
    }

    /// **The cells dial is the field's second control, and both are SHARED
    /// state.** Its committed number re-rolls the field at the new count (the
    /// same seed — dialing up grows the same world), the resting echo re-rolls
    /// nothing, and a wild number clamps into the offered 2..12 range. The
    /// field itself is one object every view reads: the hex page's column
    /// colour comes from the same heats the seams tab paints.
    #[test]
    fn the_cells_dial_rerolls_the_shared_field_and_echoes_are_inert() {
        let mut bench = test_bench();
        let seed = bench.seams().seed();
        assert_eq!(
            bench.seams().cells(),
            flicker_worldengine::DEFAULT_CELLS,
            "opens at the default count"
        );

        let write = |n: f64| {
            let mut r = ValueMap::default();
            r.set(ui::CELLS_BIND, n);
            r
        };
        bench.apply_results(&write(9.0));
        assert_eq!(bench.seams().cells(), 9, "the committed number lands");
        assert_eq!(bench.seams().seed(), seed, "same roll, more cells");
        let heats = bench.seams().heats().to_vec();
        bench.apply_results(&write(9.0));
        assert_eq!(
            bench.seams().heats(),
            &heats[..],
            "the resting echo re-rolls nothing"
        );
        bench.apply_results(&write(99.0));
        assert_eq!(
            bench.seams().cells(),
            flicker_worldengine::MAX_CELLS,
            "a wild number clamps into the dial's range"
        );
        // …and the dial's own declaration carries the same range.
        let tree = bench.build_tree();
        let dial = flatten(&tree)
            .into_iter()
            .find(|n| n.bind.as_deref() == Some(ui::CELLS_BIND))
            .expect("the cells dial is declared");
        assert_eq!(
            dial.props.get("min"),
            Some(&Value::Number(f64::from(flicker_worldengine::MIN_CELLS)))
        );
        assert_eq!(
            dial.props.get("max"),
            Some(&Value::Number(f64::from(flicker_worldengine::MAX_CELLS)))
        );
        assert_eq!(dial.tab_group, ui::LEFT_PANE, "walker-focusable");
    }

    /// **EVERY dial in the tree is accounted for, with its range pinned.**
    /// The inverse-membership gate: each `slider` the scene authors must have
    /// a row here pairing its bind with the code's own MIN/MAX constants — so
    /// a new layer's control added without a row FAILS THE BUILD, the way an
    /// unaccounted component already fails the catalog gate. (The spots dial
    /// shipped ungated once and the plates dial then reproduced the gap
    /// verbatim — a point fix per control does not close a loop that every
    /// future layer re-enters; this table does.)
    #[test]
    fn every_dial_in_the_tree_is_accounted_with_its_range() {
        let accounted: &[(&str, f64, f64)] = &[
            (ui::FREQ_BIND, f64::from(MIN_FREQ), f64::from(MAX_FREQ)),
            (
                ui::CELLS_BIND,
                f64::from(flicker_worldengine::MIN_CELLS),
                f64::from(flicker_worldengine::MAX_CELLS),
            ),
            (
                ui::SPOTS_BIND,
                f64::from(flicker_worldengine::MIN_SPOTS),
                f64::from(flicker_worldengine::MAX_SPOTS),
            ),
            (
                ui::WATER_TARGET_BIND,
                f64::from(MIN_WATER),
                f64::from(MAX_WATER),
            ),
            (
                ui::VEG_TARGET_BIND,
                f64::from(MIN_WATER),
                f64::from(MAX_WATER),
            ),
        ];
        let bench = test_bench();
        let tree = bench.build_tree();
        let model = bench.model();
        let all = flatten(&tree);
        let dials: Vec<&&UiNode> = all.iter().filter(|n| n.component == "slider").collect();
        assert_eq!(
            dials.len(),
            accounted.len(),
            "every authored dial has a row, every row has a dial"
        );
        for d in dials {
            let bind = d.bind.as_deref().expect("a dial is bound");
            let (_, min, max) = accounted
                .iter()
                .find(|(b, _, _)| *b == bind)
                .unwrap_or_else(|| panic!("`{bind}` has no row — add (bind, MIN, MAX) here"));
            assert_eq!(
                d.props.get("min"),
                Some(&Value::Number(*min)),
                "{bind}: authored min IS the code constant"
            );
            assert_eq!(
                d.props.get("max"),
                Some(&Value::Number(*max)),
                "{bind}: authored max IS the code constant"
            );
            // …and the MODEL PUBLISHES the bind. An unpublished dial echoes
            // its authored MINIMUM, which the dispatcher then takes as a
            // committed write — the plates tab shipped exactly that: first
            // entry re-rolled the world down to 4 plates in front of Aaron.
            let v = model
                .number(bind)
                .unwrap_or_else(|| panic!("`{bind}` is not published by model()"));
            assert!(
                (*min..=*max).contains(&v),
                "{bind}: the published value {v} sits inside the dial's range"
            );
        }
    }

    /// **The era's three controls drive their arms** (rule F50B97A5 — a
    /// binding lands WITH its gate): RUN toggles the clock, STEP advances
    /// exactly one tick (data changes, the readouts follow), RESET stops the
    /// clock and returns the bare shell — and all three are declared in the
    /// tree, in the evolve slice, in the left pane's focus group.
    #[test]
    fn the_evolve_controls_drive_their_arms() {
        let mut bench = test_bench();
        let fire = |name: &str| {
            let mut r = ValueMap::default();
            r.set(name, true);
            r
        };
        assert!(!bench.evolve_running(), "the era opens paused");
        bench.apply_results(&fire(ui::EVOLVE_RUN_ACTION));
        assert!(bench.evolve_running(), "RUN starts the clock");
        bench.apply_results(&fire(ui::EVOLVE_RUN_ACTION));
        assert!(!bench.evolve_running(), "…and toggles it off");

        assert_eq!(bench.evolve().ticks(), 0);
        bench.apply_results(&fire(ui::EVOLVE_TICK_ACTION));
        assert_eq!(bench.evolve().ticks(), 1, "TICK plays exactly one cycle");
        let grown: f32 = (0..bench.map().len() as TileId)
            .map(|t| bench.evolve().rock(t))
            .sum();
        assert!(grown > 0.0, "…and the world grew material");

        bench.apply_results(&fire(ui::EVOLVE_RUN_ACTION));
        bench.apply_results(&fire(ui::EVOLVE_RESET_ACTION));
        assert_eq!(bench.evolve().ticks(), 0, "RESET returns the bare shell");
        assert!(!bench.evolve_running(), "…and stops the clock");

        // All three are authored in the tree, walker-reachable.
        let tree = bench.build_tree();
        for action in [
            ui::EVOLVE_RUN_ACTION,
            ui::EVOLVE_STEP_ACTION,
            ui::EVOLVE_RESET_ACTION,
        ] {
            let b = flatten(&tree)
                .into_iter()
                .find(|n| n.action.as_deref() == Some(action))
                .unwrap_or_else(|| panic!("`{action}` is declared"));
            assert_eq!(b.component, "button");
            assert_eq!(b.tab_group, ui::LEFT_PANE);
        }
    }

    /// **COMMIT stages the planet epoch** — the bench's OUTPUT contract, end
    /// to end: a running, MID-CYCLE era commits; the clock pauses, the open
    /// procedure cycle is run to its close (never captured half-done), and a
    /// v2 `.epoch` lands under `<staging>/worlds/` that the format's own
    /// loader validates. The result line is the staged path; a failure would
    /// land the error on the same bind — never silence. Touches ONLY the
    /// staging root it was given (the sablework rule: an ingest bench writes
    /// to staging and stops).
    ///
    /// The footer's own wiring rides the tree: NEXT/COMMIT are gated
    /// opposites over the same footer (the gates test covers the keys), and
    /// BACK/NEXT fire the rail's own step names, which
    /// `the_rails_own_their_stepping_and_the_scene_reads_the_index` already
    /// proves step the strip from ANY source.
    #[test]
    fn commit_stages_a_valid_planet_epoch() {
        let mut bench = test_bench();
        let fire = |name: &str| {
            let mut r = ValueMap::default();
            r.set(name, true);
            r
        };
        // A real stretch of era, left RUNNING and MID-CYCLE.
        bench.apply_results(&fire(ui::EVOLVE_STEP_ACTION));
        bench.apply_results(&fire(ui::EVOLVE_RUN_ACTION));
        {
            let PopulousBench {
                map,
                seams,
                crust,
                evolve,
                ..
            } = &mut bench;
            let sea = evolve.resolve_sea();
            evolve.tick_phase(map, seams, crust, sea);
            assert_ne!(
                evolve.current_phase(),
                flicker_worldengine::PHASES[0],
                "the era stands mid-cycle"
            );
        }

        let dir = std::env::temp_dir().join("flicker_populous_commit_test");
        let _ = std::fs::remove_dir_all(&dir);
        bench.commit_to(&dir);

        assert!(!bench.evolve_running(), "commit pauses the era");
        assert_eq!(
            bench.evolve().current_phase(),
            flicker_worldengine::PHASES[0],
            "the open cycle was run to its close"
        );
        let expected = dir.join("worlds").join(format!(
            "planet_f{}_s{:016x}_t{}.epoch.gz",
            bench.map().freq(),
            bench.seams().seed(),
            bench.evolve().ticks()
        ));
        assert_eq!(
            bench.commit_status,
            expected.display().to_string(),
            "the result line is the staged path"
        );
        assert!(
            bench.model().text(ui::COMMIT_STATUS_BIND).is_some(),
            "the result line is published on its bind"
        );
        let file = flicker_worldengine::PlanetEpoch::load(&expected)
            .expect("the staged file is a valid planet epoch");
        assert_eq!(file.recipe.freq, bench.map().freq());
        assert_eq!(file.era.ticks, bench.evolve().ticks());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A tile is WHAT ITS HEIGHT SAYS, against the sea the dial asked for.**
    /// The classification ladder end to end: deep, shallow and surface water
    /// by depth below the line; dry ground re-reads as bed, shelf or LAND as
    /// it grows through the levels — a seamount that outgrows the shelf level
    /// stops being painted a bed (Aaron's reclassification). And the sea
    /// level itself floods the asked share of an equal-area world.
    #[test]
    fn height_reclassifies_the_surface_and_the_dial_floods_its_share() {
        // The rock ladder: bed → shelf → land as the material grows — wet or
        // dry, the GROUND is rock (the water is its own transparent cells).
        // LIVE classification: the sea line and the marine press decide.
        let sea = 2.0;
        assert_eq!(ground_class(sea + 0.1, sea, 1.0), Ground::Land);
        assert_eq!(
            ground_class(sea - SHELF_DEPTH * 0.5, sea, 1.0),
            Ground::Shelf,
            "shallow young drowned ground is shelf"
        );
        assert_eq!(
            ground_class(sea - SHELF_DEPTH - 0.2, sea, 1.0),
            Ground::Bed,
            "a shelf pushed deep reclasses as bed"
        );
        assert_eq!(
            ground_class(sea - SHELF_DEPTH * 0.5, sea, SHELF_BED_GRADE + 0.1),
            Ground::Bed,
            "enough pressure and sedimentation makes bed even in the shallows"
        );

        // The percentile over the era's own heights. At TICK ZERO the crust
        // is three flat tiers (bed / shelf / continent), so exact coverage is
        // impossible — the strict-below count stops at a tier edge below the
        // ask. The gate: the flood never EXCEEDS the ask, and it reaches at
        // least the bed tier (the world's majority floor). As the era runs
        // and injection + weathering diversify the heights, coverage
        // converges on the dial.
        let bench = test_bench();
        let sea = bench.evolve().sea_level(71.0);
        let under = (0..bench.map().len() as TileId)
            .filter(|t| bench.evolve().ground(*t) < sea)
            .count() as f32;
        let frac = under / bench.map().len() as f32;
        // Water fills WHOLE tiers, so the achievable coverages at tick zero
        // are the tier boundaries — assert the line landed on the one CLOSEST
        // to the ask, whatever this roll's kind mix turned out to be.
        let mut heights: Vec<f32> = (0..bench.map().len() as TileId)
            .map(|t| bench.evolve().ground(t))
            .collect();
        heights.sort_by(f32::total_cmp);
        let n = heights.len() as f32;
        let mut achievable = vec![0.0f32];
        let mut i = 0usize;
        while i < heights.len() {
            let v = heights[i];
            let mut j = i;
            while j < heights.len() && heights[j] <= v + 1e-5 {
                j += 1;
            }
            achievable.push(j as f32 / n);
            i = j;
        }
        let closest = achievable
            .iter()
            .copied()
            .min_by(|a, b| (a - 0.71).abs().total_cmp(&(b - 0.71).abs()))
            .unwrap();
        assert!(
            (frac - closest).abs() < 1e-3,
            "the flood lands on the closest achievable tier: {frac} vs {closest}"
        );
    }

    /// **TICK 1200 queues a bootstrap-sized leap from the standing clock**
    /// (Aaron 2026-08-26: 1200 looks amazing — what does 4800 look like?
    /// Four presses). The arm starts the run with a roll window
    /// [now, now+1200); reset clears it; and the first Run on a fresh world
    /// queues the bootstrap window itself.
    #[test]
    fn the_hex_inspector_itemizes_the_focused_column() {
        // **The hex page reads as an INSPECTOR** (Aaron 2026-08-27): the
        // model publishes every roster bind, pre-formatted — materials on
        // the left, fluids on the right — and the values follow the ledger:
        // an empty layer is "—", a planted ore body names itself.
        let mut bench = test_bench();
        let m = bench.model();
        for b in ui::HEX_MAT_BINDS.iter().chain(&ui::HEX_FLUID_BINDS) {
            assert!(
                matches!(m.get(b), Some(Value::Text(_))),
                "{b}: the inspector publishes a formatted value"
            );
        }
        let text = |m: &ValueMap, b: &str| match m.get(b) {
            Some(Value::Text(t)) => t.clone(),
            _ => unreachable!(),
        };
        // A bare world: no ore, no strata at the focused cell.
        let t = bench.focus_tile();
        assert_eq!(text(&m, ui::HEX_VEIN_BIND), "—");
        assert_eq!(text(&m, ui::HEX_L4_BIND), "—");
        // Plant an ore body at the focus: the composition row names it.
        bench.evolve.plant_vein(t, 3);
        let m = bench.model();
        assert_eq!(
            text(&m, ui::HEX_VEIN_BIND),
            flicker_worldengine::vein_kinds()[3].label,
            "the ore body announces its registry label"
        );
        // The molten row reads the seam field at the focus, as a percent.
        assert!(
            text(&m, ui::HEX_HEAT_BIND).ends_with('%'),
            "heat is a percent readout"
        );
    }

    /// **The tick contract (Aaron 2026-08-29).** PLAY runs with no horizon;
    /// PAUSE stops it. PLAY-N runs the typed count. TICK plays one complete
    /// cycle and rests. STEP plays ONE procedure — and any run button after a
    /// step first completes the open tick before its own work.
    #[test]
    fn the_run_buttons_honour_the_tick_contract() {
        let mut bench = test_bench();
        let fire = |name: &str| {
            let mut r = ValueMap::default();
            r.set(name, true);
            r
        };
        // PLAY: running, no window — until PAUSE, which stops the clock.
        bench.apply_results(&fire(ui::EVOLVE_RUN_ACTION));
        assert!(bench.evolve_running(), "PLAY runs");
        assert_eq!(bench.roll_window(), None, "PLAY has no horizon");
        bench.apply_results(&fire(ui::EVOLVE_RUN_ACTION));
        assert!(!bench.evolve_running(), "PAUSE stops");

        // PLAY-N: the typed count opens the window from the standing clock.
        let mut n = ValueMap::default();
        n.set(ui::TICK_COUNT_BIND, "2500".to_string());
        n.set(ui::EVOLVE_ROLL_ACTION, true);
        let now = bench.evolve().ticks();
        bench.apply_results(&n);
        assert!(bench.evolve_running(), "PLAY-N runs");
        assert_eq!(
            bench.roll_window(),
            Some((now, now + 2500)),
            "the window is the typed count"
        );

        // An unparseable count falls back to the 1200 default.
        let mut junk = ValueMap::default();
        junk.set(ui::TICK_COUNT_BIND, "".to_string());
        junk.set(ui::EVOLVE_ROLL_ACTION, true);
        let now = bench.evolve().ticks();
        bench.apply_results(&junk);
        assert_eq!(
            bench.roll_window(),
            Some((now, now + flicker_worldengine::BOOTSTRAP_TICKS)),
            "empty field means the default leap"
        );

        // Reset clears the world AND the window.
        bench.apply_results(&fire(ui::EVOLVE_RESET_ACTION));
        assert!(!bench.evolve_running());
        assert_eq!(bench.roll_window(), None, "reset clears the goal");
        assert_eq!(bench.evolve().ticks(), 0);
    }

    /// STEP plays exactly one procedure; TICK plays one complete cycle; and a
    /// run button clicked mid-step first COMPLETES the open tick (the cycle
    /// always closes before the next begins).
    #[test]
    fn step_is_one_procedure_and_open_ticks_complete_first() {
        let mut bench = test_bench();
        let fire = |name: &str| {
            let mut r = ValueMap::default();
            r.set(name, true);
            r
        };
        let phases = flicker_worldengine::PHASES.len();

        // One STEP: the cursor moved one procedure, the tick has not counted.
        bench.apply_results(&fire(ui::EVOLVE_STEP_ACTION));
        assert_eq!(bench.evolve().ticks(), 0, "a step is less than a tick");
        assert_ne!(
            bench.evolve().current_phase(),
            flicker_worldengine::PHASES[0],
            "the tick stands open"
        );

        // TICK mid-step: the open tick completes FIRST, then one full cycle
        // runs — two ticks on the clock, the cursor at rest between cycles.
        bench.apply_results(&fire(ui::EVOLVE_TICK_ACTION));
        assert_eq!(
            bench.evolve().ticks(),
            2,
            "complete the open tick, then play one"
        );
        assert_eq!(
            bench.evolve().current_phase(),
            flicker_worldengine::PHASES[0],
            "the cursor rests between cycles"
        );

        // Stepping a whole cycle by hand counts exactly one tick.
        let before = bench.evolve().ticks();
        for _ in 0..phases {
            bench.apply_results(&fire(ui::EVOLVE_STEP_ACTION));
        }
        assert_eq!(bench.evolve().ticks(), before + 1, "N steps = one tick");
    }

    /// **The evolve pane NAVIGATES up/down and the target rides left/right**
    /// (Aaron 2026-08-27, QOL): the left pane's evolve slice is checkbox →
    /// ONE horizontal target slider → the four buttons, each on its own
    /// ascending ordinal — up/down walks the chain, left/right nudges only
    /// the slider (the walker's SliderH contract). The coverage and climate
    /// left as READOUT gauges on the right pane: resource_gauge fills on
    /// 0..1 fractions beside pre-formatted percents.
    #[test]
    fn the_evolve_pane_navigates_and_the_readouts_moved_right() {
        let bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        // The one slider in the evolve slice is the TARGET, horizontal.
        let sliders: Vec<&&UiNode> = all
            .iter()
            .filter(|n| {
                n.component == "slider"
                    && matches!(n.bind.as_deref(), Some(b) if b.starts_with("pop_water") || b == "pop_temp")
            })
            .collect();
        assert_eq!(sliders.len(), 1, "one water control stands, not three");
        let target = sliders[0];
        assert_eq!(target.bind.as_deref(), Some(ui::WATER_TARGET_BIND));
        assert!(
            !matches!(target.props.get("vertical"), Some(Value::Bool(true))),
            "the target dial lies HORIZONTAL: left/right nudges it"
        );
        // The buttons are reachable on their own ascending ordinals.
        let mut ords: Vec<u32> = all
            .iter()
            .filter(|n| n.action.as_deref().map(|a| a.starts_with("pop_evolve")) == Some(true))
            .map(|n| n.nav_ordinal)
            .collect();
        assert_eq!(ords.len(), 5, "all five era buttons carry ordinals");
        let mut sorted = ords.clone();
        sorted.sort_unstable();
        sorted.dedup();
        ords.sort_unstable();
        assert_eq!(ords, sorted, "every button has its OWN step in the walk");
        // The readouts stand as right-pane gauges on 0..1 fractions.
        for (gauge, frac) in [
            ("pop_water_gauge", ui::WATER_BIND),
            ("pop_temp_gauge", ui::TEMP_BIND),
            ("pop_green_gauge", ui::GREEN_BIND),
        ] {
            let g = all
                .iter()
                .find(|n| n.id == gauge)
                .unwrap_or_else(|| panic!("{gauge} is declared"));
            assert_eq!(g.component, "resource_gauge", "a READOUT, not a control");
            assert_eq!(g.bind.as_deref(), Some(frac));
        }
        let m = bench.model();
        for b in [ui::WATER_BIND, ui::TEMP_BIND, ui::GREEN_BIND] {
            match m.get(b) {
                Some(Value::Number(v)) => {
                    assert!((0.0..=1.0).contains(v), "{b} is a gauge fraction: {v}")
                }
                other => panic!("{b} publishes a fraction: {other:?}"),
            }
        }
    }

    /// **The GAUGE shows, the TARGET controls** (Aaron 2026-08-26; re-cut
    /// 2026-08-27: the gauge is a right-pane godmode-style READOUT now, no
    /// handler at all). A write on the readout bind changes NOTHING. The
    /// target dial is a plain control: its committed number lands on the
    /// era's coverage target, its echo is inert, a wild number clamps — and
    /// none of it touches the clock or any roll: the IN-FALL, not a
    /// re-pour, walks the world toward the target.
    #[test]
    fn the_water_gauge_shows_and_the_target_dial_controls() {
        let mut bench = test_bench();
        assert!(
            bench.water_coverage_pct() >= 99,
            "opens a water world: {}%",
            bench.water_coverage_pct()
        );
        assert_eq!(
            (bench.evolve().water_target() * 100.0).round() as u32,
            70,
            "the target opens at the 70% ocean share"
        );
        let mut step = ValueMap::default();
        step.set(ui::EVOLVE_STEP_ACTION, true);
        bench.apply_results(&step);
        let ticks = bench.evolve().ticks();
        let molten_seed = bench.seams().seed();
        let sea = bench.evolve().resolve_sea();

        // The GAUGE is display: a hand on it changes nothing at all.
        let mut gauge = ValueMap::default();
        gauge.set(ui::WATER_BIND, 15.0);
        bench.apply_results(&gauge);
        assert_eq!(
            bench.evolve().resolve_sea(),
            sea,
            "the gauge is not a control"
        );

        // The TARGET dial is a plain control on the era's target.
        let write = |n: f64| {
            let mut r = ValueMap::default();
            r.set(ui::WATER_TARGET_BIND, n);
            r
        };
        bench.apply_results(&write(55.0));
        assert_eq!(
            (bench.evolve().water_target() * 100.0).round() as u32,
            55,
            "the committed target lands"
        );
        bench.apply_results(&write(55.0));
        assert_eq!(
            (bench.evolve().water_target() * 100.0).round() as u32,
            55,
            "the echo is inert"
        );
        bench.apply_results(&write(900.0));
        assert_eq!(
            (bench.evolve().water_target() * 100.0).round() as u32,
            u32::from(MAX_WATER as u8),
            "a wild number clamps"
        );
        assert_eq!(bench.evolve().resolve_sea(), sea, "a target is not a pour");
        assert_eq!(bench.evolve().ticks(), ticks, "the era's clock held");
        assert_eq!(bench.seams().seed(), molten_seed, "no molten re-roll");
    }

    /// **The reticle RIDES its column** — rings lifted by the focused
    /// cell's own ground plus the margin, posts rooted at the column top.
    #[test]
    fn the_reticle_rides_its_column() {
        // **The reticle RIDES its column** (Aaron 2026-08-27: the highlight
        // was getting buried once the era grew mountains): the rings lift by
        // the focused cell's own ground plus the margin, and the posts root
        // at the column top — pinned to the ledger, whatever the height.
        let mut bench = test_bench();
        bench.shown_view = WorldView::Evolve;
        let t: TileId = 42;
        bench.highlight = Some(t);
        bench.apply_overlays();
        let w = tile_width(bench.map.direction(0), bench.map.outline(0), RADIUS);
        let top = bench.evolve.ground(t) * w;
        let want = RADIUS * RETICLE_RINGS[1] + top + RETICLE_LIFT * w;
        let ring = bench
            .world
            .arrows()
            .iter()
            .find(|(c, _)| *c == RETICLE_INK)
            .expect("the reticle group stands");
        let peak = ring
            .1
            .iter()
            .map(|(a, b)| a.length().max(b.length()))
            .fold(0.0f32, f32::max);
        assert!(
            (peak - want).abs() < w * 0.05,
            "the upper ring rides the column: {peak} vs {want}"
        );
        let foot = ring
            .1
            .iter()
            .map(|(a, b)| a.length().min(b.length()))
            .fold(f32::MAX, f32::min);
        assert!(
            (foot - (RADIUS + top)).abs() < w * 0.05,
            "the posts root at the column top: {foot} vs {}",
            RADIUS + top
        );
    }

    #[test]
    fn the_vein_bodies_wear_their_field_outlines() {
        // **A vein body wears its field outline** in its own ink on the
        // evolve view — the boundary ring that makes rubies, coal and
        // calcium findable at a glance (Aaron 2026-08-27).
        let mut bench = test_bench();
        bench.shown_view = WorldView::Evolve;
        let t: TileId = 100;
        bench.evolve.plant_vein(t, 3);
        bench.apply_overlays();
        let ink = flicker_worldengine::vein_kinds()[3].ink;
        let color = [ink[0], ink[1], ink[2], 1.0];
        let outline = bench
            .world
            .arrows()
            .iter()
            .find(|(c, _)| *c == color)
            .expect("the body's ink group stands");
        // A single-cell body is ALL boundary: its whole hex ring draws.
        assert!(
            outline.1.len() >= 5,
            "the outline surrounds the node: {} edges",
            outline.1.len()
        );
    }

    /// **The motion-arrows checkbox is a LENS with an arm.** Its committed
    /// bool lands, its echo is inert, and toggling it resets nothing — not
    /// the era's clock, not any roll. (The arrows draw only on the evolve
    /// view AND while this is on; the overlay recomposes on toggle.)
    #[test]
    fn the_arrows_checkbox_is_a_lens_with_an_arm() {
        let mut bench = test_bench();
        assert!(bench.show_arrows(), "opens showing the motion");
        let mut step = ValueMap::default();
        step.set(ui::EVOLVE_STEP_ACTION, true);
        bench.apply_results(&step);
        let ticks = bench.evolve().ticks();
        let write = |b: &mut PopulousBench, v: bool| {
            let mut r = ValueMap::default();
            r.set(ui::ARROWS_BIND, v);
            b.apply_results(&r);
        };
        write(&mut bench, false);
        assert!(!bench.show_arrows(), "the committed toggle lands");
        write(&mut bench, false);
        assert!(!bench.show_arrows(), "the echo is inert");
        write(&mut bench, true);
        assert!(bench.show_arrows());
        assert_eq!(bench.evolve().ticks(), ticks, "a lens resets nothing");
    }

    /// **The spots dial is the field's third control, on the same contract.**
    /// Its committed number re-rolls the plumes at the new count (same roll —
    /// the shared prefix survives), the resting echo re-rolls nothing, a wild
    /// number clamps into the offered range, the crust's vents follow the
    /// change — and the AUTHORED min/max are pinned to the code's own
    /// constants, so the two sides of the range cannot drift apart unnoticed
    /// (the cells dial carries the identical pin).
    #[test]
    fn the_spots_dial_rerolls_the_plumes_and_its_range_is_pinned() {
        let mut bench = test_bench();
        let seed = bench.seams().seed();
        assert_eq!(
            bench.seams().spots(),
            flicker_worldengine::DEFAULT_SPOTS,
            "opens at the default count"
        );

        let write = |n: f64| {
            let mut r = ValueMap::default();
            r.set(ui::SPOTS_BIND, n);
            r
        };
        let before_heat = bench.seams().heats().to_vec();
        bench.apply_results(&write(9.0));
        assert_eq!(bench.seams().spots(), 9, "the committed number lands");
        assert_eq!(bench.seams().seed(), seed, "same roll, more plumes");
        // The robust witness: the FIELD changed (new plumes burn in it). The
        // derived vent LIST may legitimately coincide on some rolls — the
        // two-scale founding separation can refuse every new field — so the
        // old vents-differ assertion was a seed-dependent flake.
        assert_ne!(
            bench.seams().heats(),
            &before_heat[..],
            "the plumes burn in the field the crust derives from"
        );
        let heats = bench.seams().heats().to_vec();
        bench.apply_results(&write(9.0));
        assert_eq!(
            bench.seams().heats(),
            &heats[..],
            "the resting echo re-rolls nothing"
        );
        bench.apply_results(&write(99.0));
        assert_eq!(
            bench.seams().spots(),
            flicker_worldengine::MAX_SPOTS,
            "a wild number clamps into the dial's range"
        );
        // …and the dial's own declaration carries the same range.
        let tree = bench.build_tree();
        let dial = flatten(&tree)
            .into_iter()
            .find(|n| n.bind.as_deref() == Some(ui::SPOTS_BIND))
            .expect("the spots dial is declared");
        assert_eq!(
            dial.props.get("min"),
            Some(&Value::Number(f64::from(flicker_worldengine::MIN_SPOTS)))
        );
        assert_eq!(
            dial.props.get("max"),
            Some(&Value::Number(f64::from(flicker_worldengine::MAX_SPOTS)))
        );
        assert_eq!(dial.tab_group, ui::LEFT_PANE, "walker-focusable");
    }

    /// **The PTT rails fire results this screen dispatches.** The glyph hints are now
    /// drawn BY the `paged_menu` Component and their click→step is gated in flicker-widgets
    /// (`hit_paged_menu` fires the neighbouring rail's step name). At the SCENE level the
    /// load-bearing property survives: each rail's `next_action`/`prev_action` is one of the
    /// screen's declared intents — so a hint click, a pad Confirm and the shoulder signal all
    /// converge on the ONE result the rail steps on. A rail stepping a result nothing
    /// declares would be a dead control.
    #[test]
    fn the_ptt_rails_fire_results_this_screen_dispatches() {
        let tree = test_bench().build_tree();
        let declared: Vec<String> = tree
            .props
            .iter()
            .filter(|(k, _)| k.starts_with("on_"))
            .filter_map(|(_, v)| match v {
                Value::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let all = flatten(&tree);
        let mut steps: Vec<&str> = Vec::new();
        for n in &all {
            for k in ["next_action", "prev_action"] {
                if let Some(Value::Text(t)) = n.props.get(k) {
                    steps.push(t.as_str());
                }
            }
        }
        steps.sort_unstable();
        steps.dedup();
        assert_eq!(
            steps,
            ["page_next", "page_prev", "tab_next", "tab_prev"],
            "both rails carry their four step names"
        );
        for want in steps {
            assert!(
                declared.iter().any(|d| d == want),
                "`{want}` is a declared intent, so pad / click / shoulder converge on the rail's step"
            );
        }
    }

    /// **The viewport actually gets its pixels.** The real tree through the
    /// real resolver at a bench-sized screen: the reserved rect must exist, be
    /// SQUARE (the aspect lock), and fill a substantial share of the height —
    /// the assertion that catches a collapsed layout chain. Both halves of the
    /// squashed-band bug lived below this line: the inner frame flowing at its
    /// measured ~100px instead of growing, and an anchored square inside a
    /// flow cell measuring to nothing and vanishing. Tree gates saw both trees
    /// as fine; only the resolved RECT tells the truth.
    #[test]
    fn the_viewport_resolves_to_a_substantial_square() {
        use flicker::render::Vec2;

        let bench = test_bench();
        let tree = bench.build_tree();
        // Light the WORLD page + MAP slice (what arrange() publishes on page 0,
        // tab 0) so the gated viewport and size dial are placed — both are dark
        // until the selection lights their keys.
        let mut model = ValueMap::default();
        model.set("shown_page0", true);
        model.set("shown_p0_t0", true);
        let styles = load_shared_styles(None);
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let mut state = UiState::default();
        let frame = run_ui(&tree, &model, &styles, &snap, &mut state);
        let rect = frame
            .surface_rect(ui::VIEW_SLOT)
            .expect("the viewport's rect is reserved by the walker");
        assert!(
            (rect.size.x - rect.size.y).abs() < 1.5,
            "the viewport is square: {}×{}",
            rect.size.x,
            rect.size.y
        );
        assert!(
            rect.size.y > 300.0,
            "the square fills a substantial share of a 900px screen, got {}",
            rect.size.y
        );
        // EVERY control a user must see and hit gets the same treatment — a
        // node can be perfect in the tree and still resolve to zero pixels
        // (the size dial shipped exactly that way once: present, bound,
        // unclickable). Presence AND extent, from the layout's own answer.
        for id in [ui::LEFT_PANE, ui::RIGHT_PANE] {
            let r = frame.rect(id).unwrap_or_else(|| panic!("{id} resolves"));
            assert!(r.size.x > 100.0 && r.size.y > 300.0, "{id} has real pixels");
        }
        let dial = frame.rect(ui::FREQ_BIND).expect("the size dial resolves");
        assert!(
            dial.size.y > 150.0,
            "the dial's track is tall enough to grab, got {}",
            dial.size.y
        );
        let left = frame.rect(ui::LEFT_PANE).unwrap();
        assert!(
            dial.pos.x >= left.pos.x && dial.pos.x + dial.size.x <= left.pos.x + left.size.x,
            "the dial sits inside the left pane"
        );
    }

    /// **A mouse click on the dial's track writes the bound value** — the full
    /// pointer contract through the REAL component: press inside the vertical
    /// track captures (top is max), and the release edge lands the one real
    /// write on `pop_freq`. This is the test for "I could not click on that":
    /// it drives the actual Lua slider, so a dial that draws but cannot be hit
    /// — or hits but writes nothing — fails here, not in Aaron's hands.
    #[test]
    fn a_click_on_the_dials_track_writes_the_bound_value() {
        use flicker::render::Vec2;

        let bench = test_bench();
        let tree = bench.build_tree();
        // Light the MAP slice so the gated dial is placed + hittable (tab 0).
        let mut model = bench.model();
        model.set("shown_p0_t0", true);
        let styles = load_shared_styles(None);
        let mut state = UiState::default();
        let snap = |mouse: Vec2, clicked: bool, down: bool| UiInput {
            mouse,
            clicked,
            down,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };

        // Find the dial, then press at the very BOTTOM of it — below-track
        // presses clamp to t = 0, so the landing value is EXACTLY the minimum,
        // independent of the label band's height above the track.
        let count_48 = |cmds: &[flicker::script::HudCommand]| {
            cmds.iter()
                .filter(
                    |c| matches!(c, flicker::script::HudCommand::Text { text, .. } if text == "48"),
                )
                .count()
        };
        let frame = run_ui(
            &tree,
            &model,
            &styles,
            &snap(Vec2::ZERO, false, false),
            &mut state,
        );
        let dial = frame.rect(ui::FREQ_BIND).expect("the dial resolves");
        assert_eq!(
            count_48(&frame.commands),
            1,
            "at rest: the min range mark alone reads 48"
        );
        let grab = Vec2::new(
            dial.pos.x + dial.size.x * 0.5,
            dial.pos.y + dial.size.y - 2.0,
        );

        let held = run_ui(&tree, &model, &styles, &snap(grab, true, true), &mut state);
        // The drag INDICATOR: while the hand holds the knob, the LIVE value
        // joins the range mark — two "48"s on screen, the number you are
        // setting riding the handle at the point of motion.
        assert_eq!(
            count_48(&held.commands),
            2,
            "mid-drag: the live readout joins the min mark beside the handle"
        );
        let released = run_ui(
            &tree,
            &model,
            &styles,
            &snap(grab, false, false),
            &mut state,
        );
        let v = released
            .results
            .number(ui::FREQ_BIND)
            .expect("the release edge writes the bind");
        assert!(
            (v - 48.0).abs() < 1.0,
            "a press at the track's bottom lands the minimum, got {v}"
        );
    }

    /// **A resize re-derives the heat field over the new tiling from the SAME
    /// roll.** The seam field is shared world state beside the map: the size
    /// dial rebuilds the tiling, the field follows it tile for tile, and the
    /// world's seams do not move — same seed, same cells, new derivation. The
    /// centre cell survives as a CLAMPED read, so the hex view cannot index a
    /// tile the smaller map no longer has.
    #[test]
    fn a_resize_rederives_the_field_from_the_same_roll() {
        let mut bench = test_bench();
        let seed = bench.seams().seed();
        let cells = bench.seams().cells();
        let mut shrink = ValueMap::default();
        shrink.set(ui::FREQ_BIND, f64::from(MIN_FREQ));
        bench.apply_results(&shrink);
        assert_eq!(
            bench.seams().heats().len(),
            bench.map().len(),
            "one heat per tile of the NEW tiling"
        );
        assert_eq!(bench.seams().seed(), seed, "the roll survives the resize");
        assert_eq!(bench.seams().cells(), cells, "and so does the count");
        assert!(
            (bench.focus_tile() as usize) < bench.map().len(),
            "the centre cell reads inside the new map"
        );
    }

    /// **The map is the standard 96.** Frequency 96 — 92,162 tiles, each the
    /// standard ~49.65 mi across on an Earth-sized body. The icosphere's own
    /// law (`10·f² + 2`) is the cross-check, so a drifted constant cannot pass
    /// as a rounding quirk.
    #[test]
    fn the_map_is_the_standard_ninety_six() {
        let bench = test_bench();
        assert_eq!(bench.map().freq(), 96);
        assert_eq!(
            bench.map().len(),
            (10 * 96 * 96 + 2) as usize,
            "92,162 tiles"
        );
    }

    /// **The whole look of this world is AUTHORED.** The bench names a stage
    /// and hands over a tiling; the stage says what is drawn — the near-black
    /// under-shell, the inset tile shell over it, and the shared reference
    /// frame above both. Nothing in this crate names a colour, a radius or an
    /// inset any more, and this gate is what keeps one from creeping back:
    /// the appearance must arrive through `stages.populous_globe`.
    ///
    /// (The layer parsing and the graticule's own shape are gated where they
    /// live, in flicker-globe: `the_stage_block_drives_the_shells_and_the_clear`
    /// and `the_world_draws_the_shared_graticule`.)
    #[test]
    fn the_worlds_appearance_comes_from_the_authored_stage() {
        use flicker::render::StageLayer;

        let bench = test_bench();
        let layers = &bench.world.stage().layers;
        let shells: Vec<&StageLayer> = layers
            .iter()
            .filter(|l| matches!(l, StageLayer::Shell { .. }))
            .collect();
        assert_eq!(
            shells.len(),
            2,
            "the under-shell and the tile shell, both authored"
        );
        assert!(
            layers
                .iter()
                .any(|l| matches!(l, StageLayer::Graticule { .. })),
            "the reference frame is a layer of the stage, not scene code"
        );
        assert!(
            !layers.iter().any(|l| matches!(l, StageLayer::Shells)),
            "this bench publishes no simulated shells — its world is the authored one"
        );
        // The tiling the world is drawn over is the map's, at every size.
        let shells = bench
            .world
            .authored_shells(&bench.map.grid().dirs, bench.map.outlines());
        assert_eq!(
            shells.len(),
            2,
            "one shell per authored layer, over the map's own tiling"
        );
        assert_eq!(
            shells[0].dirs.len(),
            bench.map.len(),
            "the world IS the map"
        );
    }

    /// **A key press steps the dial through the WHOLE component chain** — the
    /// real bindings → the resolver's edge → the walker's slider-axis nudge on
    /// the FOCUSED dial → `run_ui` stepping the bind by the node's own `step`.
    /// No scene wiring anywhere in the loop: this is the pad channel every
    /// slider now owns, driven end to end with the actual key the map binds.
    /// The dial once sat dead in-window while results-level gates were green —
    /// this chain is the one that must never go quiet again.
    #[test]
    fn a_key_press_steps_the_dial_through_the_component_chain() {
        use flicker::render::Vec2;
        use flicker_input_core::device::Key;
        use flicker_input_core::{EventKind, InputBinding};

        let bench = test_bench();
        let tree = bench.build_tree();
        // Light the MAP slice so the gated dial is placed + focusable (tab 0).
        let mut model = bench.model();
        model.set("shown_p0_t0", true);
        let styles = load_shared_styles(None);
        let intents = UiIntents::of(&tree);
        let bindings = ContextualBindings::new(InputMap::wasd_and_mouse());
        let cfg = GamepadConfig::default();
        let mut resolver = Resolver::default();
        let mut input = InputState::new();
        input.set_key(Key::Up, true);

        let mut ev: Vec<Fired> = Vec::new();
        resolver.resolve_frame(&bindings, &cfg, &input, 1, &mut ev);
        assert!(
            ev.iter()
                .any(|f| f.signal == ActionSignal::NavUp && f.kind == EventKind::Press),
            "the resolver edges the bound key into NavUp"
        );

        // The entered-left context: the dial holds the walker focus and the
        // pane's focus graph is live — exactly what the scene sets up.
        let ctx = bindings.active();
        let events: Vec<InputEvent> = ev
            .iter()
            .map(|f| InputEvent::from_fired(f, ctx, &input))
            .collect();
        let mut ui = UiState::default();
        ui.request_focus(ui::FREQ_BIND);
        let mut walker = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        assert!(
            walker.take_fired().is_empty(),
            "a nudge is not a declared intent"
        );
        drop(walker);

        // The next `run_ui` pass applies the nudge with the NODE's own step. The size
        // dial stages on Confirm (Aaron 2026-09-04, "all of them"): the step is HELD —
        // the bench keeps seeing the resting value — until a Confirm commits it.
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut ui);
        assert_eq!(
            frame.results.number(ui::FREQ_BIND),
            Some(96.0),
            "NavUp STAGED the step: the bench still sees the resting value"
        );
        assert!(ui.staged_any(), "the stage is held for Confirm");

        // Confirm on the dial commits the pane's stage; the next pass writes it.
        let confirm = Fired {
            signal: ActionSignal::Confirm,
            kind: EventKind::Press,
            control: InputBinding::Key(Key::Enter),
        };
        let events = vec![InputEvent::from_fired(&confirm, ctx, &input)];
        let mut walker = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        drop(walker);
        let frame = run_ui(&tree, &model, &styles, &snap, &mut ui);
        assert_eq!(
            frame.results.number(ui::FREQ_BIND),
            Some(97.0),
            "Confirm applied the staged step by the dial's own step"
        );
    }

    /// **The camera is the WORLD's, and the pane it lives in is its gate.** The
    /// bench keeps no orbit code and no device read: the world names the panel
    /// that hands it the look signals, and the walker's one focus id decides
    /// when that is. (The motion itself is gated in flicker-globe:
    /// `the_camera_moves_only_while_the_world_panel_holds_focus`.)
    #[test]
    fn the_camera_belongs_to_the_world_in_the_viewport_pane() {
        let mut bench = test_bench();
        let input = InputState::new();

        // A non-zero look tuple (the pump resolves this from `signals.axis` in-scene); the
        // world applies it ONLY while the viewport pane holds the walker's focus — the gate
        // this asserts. (The motion itself is gated in flicker-globe:
        // `the_camera_moves_only_while_the_world_panel_holds_focus`.)
        let turn = |b: &mut PopulousBench, _input: &InputState| {
            let before = b.world.camera().position;
            let focus = b.ui_state.focused().map(str::to_string);
            b.world.update(0.5, None, (1.0, 0.0, 0.0), focus.as_deref());
            (b.world.camera().position - before).length()
        };
        assert_eq!(
            turn(&mut bench, &input),
            0.0,
            "panel navigation owns the sticks"
        );
        bench.focus_for_test(ui::RIGHT_PANE);
        assert_eq!(
            turn(&mut bench, &input),
            0.0,
            "another pane owns its interior, never the camera"
        );
        bench.focus_for_test(ui::VIEW_PANE);
        assert!(
            turn(&mut bench, &input) > 0.0,
            "the focused viewport pane flies the planet"
        );
    }

    /// **A Menu press fires the pause intent** through the real chain: the
    /// screen's declared `on_menu` is consumed at the walker layer and comes
    /// back as the `pause_open` result `update` answers with the pause push.
    #[test]
    fn a_menu_press_fires_the_declared_pause_intent() {
        let bench = test_bench();
        let tree = bench.build_tree();
        let intents = UiIntents::of(&tree);

        let raw = InputState::new();
        let events = [InputEvent::new(
            ActionSignal::Menu,
            EventKind::Press,
            InputContext::World,
            &raw,
        )];
        let mut ui = UiState::default();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        assert_eq!(
            walker.take_fired(),
            vec!["pause_open".to_string()],
            "the declared intent reaches the scene as its result name"
        );
    }

    /// **THE WORLD MAP MODAL is wired end to end** (contract FF8A575D): the
    /// footer's MAP button toggles the engine flag, the model publishes it,
    /// `arrange()` lights the modal's slice from it, the modal subtree is the
    /// settings pattern (scrim cell → centred popup slab → the map's
    /// `surface` + a Close button), the surface names its authored stage, and
    /// the close arm shuts it. One flag, one visibility path, both
    /// directions.
    #[test]
    fn the_map_modal_is_wired_end_to_end() {
        use flicker::script::ScriptHost;

        // The tree: the modal subtree, gated and shaped as declared.
        let bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        let modal = all
            .iter()
            .find(|n| n.id == "pop_map_modal")
            .expect("the modal scrim cell is authored");
        assert_eq!(modal.visible_bind.as_deref(), Some("shown_map"));
        let popup = all
            .iter()
            .find(|n| n.id == "pop_map_popup")
            .expect("the modal slab is authored");
        assert_eq!(popup.component, "popup_panel");
        let sheet = all
            .iter()
            .find(|n| n.id == ui::MAP_SLOT)
            .expect("the map's surface is authored");
        assert_eq!(sheet.component, "surface");
        assert_eq!(
            sheet.props.get("source"),
            Some(&Value::Text(ui::MAP_STAGE_SOURCE.into())),
            "the sheet names its authored stage"
        );
        let footer_map = all
            .iter()
            .find(|n| n.id == "pop_nav_map")
            .expect("the footer's MAP button is authored");
        assert_eq!(
            footer_map.action.as_deref(),
            Some(ui::MAP_TOGGLE_ACTION),
            "the footer MAP button fires the toggle"
        );
        assert!(
            all.iter()
                .any(|n| n.action.as_deref() == Some(ui::MAP_CLOSE_ACTION)),
            "the modal carries its Close"
        );

        // The stage: the map's authored look resolves out of the scene styles.
        let def = SceneDef::parse("populous", POPULOUS_SCENE).expect("populous.scene.json loads");
        let styles = load_shared_styles(def.styles.as_ref());
        assert!(
            flicker::ui::stage_def(&styles, ui::MAP_STAGE_SOURCE).is_some(),
            "stages.populous_map compiles"
        );

        // The flag: toggle opens (and repaints), close shuts, model publishes.
        let mut bench = test_bench();
        assert!(!bench.map_open, "the bench opens with the map closed");
        let mut r = ValueMap::default();
        r.set(ui::MAP_TOGGLE_ACTION, true);
        bench.apply_results(&r);
        assert!(bench.map_open, "the footer MAP toggles it open");
        assert!(
            bench.model().is_on(ui::MAP_OPEN_BIND),
            "the model publishes the flag arrange() reads"
        );
        let mut r = ValueMap::default();
        r.set(ui::MAP_CLOSE_ACTION, true);
        bench.apply_results(&r);
        assert!(!bench.map_open, "Close shuts it");

        // Cancel is the pad's Close: a REAL Cancel dispatched with no pane
        // entered raises the walker's scene-level `cancelled`, and the bench
        // pops the map with it. A cancel while the map is closed pops nothing.
        let mut r = ValueMap::default();
        r.set(ui::MAP_TOGGLE_ACTION, true);
        bench.apply_results(&r);
        assert!(bench.map_open);
        let raw = InputState::new();
        let events = [InputEvent::new(
            ActionSignal::Cancel,
            EventKind::Press,
            InputContext::World,
            &raw,
        )];
        let nav_tree = bench.build_tree();
        let nav_model = ValueMap::default();
        let mut ui = UiState::default();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_nav(&nav_tree, &nav_model);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        assert!(walker.cancelled(), "no pane entered: Cancel is scene-level");
        bench.apply_cancel(walker.cancelled());
        assert!(!bench.map_open, "Cancel backs out of the map");
        bench.apply_cancel(true);
        assert!(!bench.map_open, "a cancel with no modal up pops nothing");

        // The Lua slice: `shown_map` follows the published flag, either way.
        let host = ScriptHost::new(POPULOUS_SCRIPT, "populous.lua").expect("populous.lua loads");
        let arrange_with = |open: bool| {
            let mut m = ValueMap::default();
            m.set(ui::PAGE_BIND, 0.0);
            m.set(ui::TAB_BIND, 0.0);
            m.set(ui::MAP_OPEN_BIND, open);
            host.set_model(&m).expect("model publishes");
            host.arrange()
                .expect("arrange runs")
                .expect("arrange is present")
                .to_model()
        };
        assert!(
            arrange_with(true).is_on("shown_map"),
            "open lights the modal"
        );
        assert!(!arrange_with(false).is_on("shown_map"), "closed darkens it");
    }

    /// **`arrange()` lights exactly the selected tab's slice.** The Lua
    /// orchestration reads the two-way-bound page/tab out of the Model and returns
    /// which selection-keyed slice is shown — the MAP tab lights `shown_p0_t0`, the
    /// SEAMS tab lights `shown_p0_t1`, and neither lights the other's. Gating is by
    /// SELECTION, not content; this is the whole of what Lua decides for this bench,
    /// and the values (the dial's number, the readouts) stay engine-side.
    #[test]
    fn arrange_lights_the_selected_tabs_slice() {
        use flicker::script::ScriptHost;

        let host = ScriptHost::new(POPULOUS_SCRIPT, "populous.lua").expect("populous.lua loads");
        let arrange_at = |page: f64, tab: f64| {
            let mut m = ValueMap::default();
            m.set(ui::PAGE_BIND, page);
            m.set(ui::TAB_BIND, tab);
            host.set_model(&m).expect("model publishes");
            host.arrange()
                .expect("arrange runs")
                .expect("arrange is present")
                .to_model()
        };

        // MAP tab (page 0, tab 0): its slice is lit, the seams slice is dark.
        let map = arrange_at(0.0, 0.0);
        assert!(map.is_on("shown_p0_t0"), "the map tab lights its slice");
        assert!(
            !map.is_on("shown_p0_t1"),
            "the map tab darkens the seams slice"
        );

        // SEAMS tab (page 0, tab 1): its slice is lit, the map slice is dark.
        let seams = arrange_at(0.0, 1.0);
        assert!(seams.is_on("shown_p0_t1"), "the seams tab lights its slice");
        assert!(
            !seams.is_on("shown_p0_t0"),
            "the seams tab darkens the map slice"
        );
        // …and the PAGE keys light the world page's rail + viewport, not the hex's.
        assert!(seams.is_on("shown_page0") && !seams.is_on("shown_page1"));

        // CRUST tab (page 0, tab 2): its slice alone.
        let crust = arrange_at(0.0, 2.0);
        assert!(crust.is_on("shown_p0_t2"), "the crust tab lights its slice");
        assert!(
            !crust.is_on("shown_p0_t0") && !crust.is_on("shown_p0_t1"),
            "the other world slices are dark"
        );

        // EVOLVE tab (page 0, tab 3): its slice alone.
        let evolve = arrange_at(0.0, 3.0);
        assert!(
            evolve.is_on("shown_p0_t3"),
            "the evolve tab lights its slice"
        );
        assert!(
            !evolve.is_on("shown_p0_t2") && !evolve.is_on("shown_p0_t0"),
            "the other world slices are dark"
        );

        // HEX page (page 1, tab 0): its page + slice light, the world page darkens.
        let hex = arrange_at(1.0, 0.0);
        assert!(hex.is_on("shown_page1"), "the hex page lights its view");
        assert!(hex.is_on("shown_p1_t0"), "and its molten tab's slice");
        assert!(
            !hex.is_on("shown_page0") && !hex.is_on("shown_p0_t0") && !hex.is_on("shown_p0_t1"),
            "the world page and both its slices are dark"
        );
    }

    /// **The static tree loads and declares BOTH tabs' slices gated.** The authored
    /// JSON parses to a `UiNode` tree of component KINDS (the template tier is gone —
    /// 201F4F51), every component kind is known, the three panes + the viewport + the
    /// size dial are all present at once, and each tab's slice is gated on its
    /// `shown_p0_t*` key — the data half of the Lua-arrange pattern.
    /// The shipped scene file, read exactly as the manifest reads it.
    const POPULOUS_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/populous.scene.json");

    /// A bench built the way the resolver builds it: from the shipped file's def.
    fn test_bench() -> PopulousBench {
        let def = SceneDef::parse("populous", POPULOUS_SCENE).expect("populous.scene.json loads");
        PopulousBench::new(&def)
    }

    /// **The seams tab is pad-operable end to end** (incident A0D3CE6A; Aaron's
    /// 2026-09-04 scheme): the stick lands the cursor on the first dial; Up/Down STAGE
    /// the vertical dial (nothing reaches the bench until a Confirm); Left/Right hop
    /// dial → dial → the randomize button (which carries an id now); Confirm on the
    /// button commits the staged value FIRST and fires the button in the pass that
    /// lands it. Every dial on the bench stages (Aaron: "all of them").
    #[test]
    fn the_seams_tab_is_pad_operable_and_stages_until_confirm() {
        use flicker::render::Vec2;
        use flicker_input_core::device::Key;
        use flicker_input_core::InputBinding;

        let bench = test_bench();
        let tree = bench.build_tree();
        let all = flatten(&tree);
        let dials: Vec<&&UiNode> = all.iter().filter(|n| n.component == "slider").collect();
        assert_eq!(dials.len(), 5, "five dials on the bench");
        assert!(
            dials
                .iter()
                .all(|n| n.props.get("apply") == Some(&Value::Text("confirm".into()))),
            "every dial stages on Confirm"
        );

        let mut model = bench.model();
        model.set("shown_page0", true);
        model.set("shown_p0_t1", true);
        assert!(!model.is_on("ui_staged"), "nothing staged at rest");
        let styles = load_shared_styles(None);
        let intents = UiIntents::of(&tree);
        let input = InputState::new();
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let press = |ui: &mut UiState, sig: ActionSignal| -> Vec<String> {
            let f = Fired {
                signal: sig,
                kind: EventKind::Press,
                control: InputBinding::Key(Key::Up),
            };
            let events = vec![InputEvent::from_fired(&f, InputContext::World, &input)];
            let mut walker = WalkerHandler::hud(ui, false)
                .with_nav(&tree, &model)
                .with_intents(&intents);
            let mut route = RouteCtx::default();
            {
                let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
                Router::dispatch(&events, &mut chain, &mut route);
            }
            walker.take_fired()
        };

        let mut ui = UiState::default();
        let _ = run_ui(&tree, &model, &styles, &snap, &mut ui);
        press(&mut ui, ActionSignal::PanelNext);
        assert_eq!(
            ui.focused(),
            Some("pop_cells"),
            "the stick descends onto the first dial"
        );
        assert_eq!(ui.focused_pane(), Some(ui::LEFT_PANE));

        press(&mut ui, ActionSignal::NavDown);
        let f = run_ui(&tree, &model, &styles, &snap, &mut ui);
        let cells = model.number(ui::CELLS_BIND).expect("cells published");
        assert_eq!(
            f.results.number(ui::CELLS_BIND),
            Some(cells),
            "a pad step STAGES: the bench keeps seeing the resting value"
        );
        assert!(
            ui.staged_any(),
            "…and `ui_staged` lights the footer's apply hint"
        );

        press(&mut ui, ActionSignal::NavRight);
        assert_eq!(
            ui.focused(),
            Some("pop_spots"),
            "Right hops to the next dial"
        );
        press(&mut ui, ActionSignal::NavRight);
        assert_eq!(
            ui.focused(),
            Some(ui::SEAMS_ACTION),
            "…and on to the randomize button, reachable now that it has an id"
        );

        let fired = press(&mut ui, ActionSignal::Confirm);
        assert!(fired.is_empty(), "the activation waits behind the commit");
        assert!(!ui.staged_any(), "Confirm committed the pane's stage");
        let f = run_ui(&tree, &model, &styles, &snap, &mut ui);
        assert_eq!(
            f.results.number(ui::CELLS_BIND),
            Some(cells - 1.0),
            "the commit lands in the next pass"
        );
        let mut walker = WalkerHandler::hud(&mut ui, false)
            .with_nav(&tree, &model)
            .with_intents(&intents);
        assert_eq!(
            walker.take_fired(),
            vec![ui::SEAMS_ACTION.to_string()],
            "…and the button fires in that same pass, after the values"
        );
    }

    #[test]
    fn the_static_tree_loads_and_gates_both_tabs() {
        let tree = SceneDef::parse("populous", POPULOUS_SCENE)
            .expect("populous.scene.json loads")
            .tree
            .expect("populous.scene.json declares a tree");
        let all = flatten(&tree);

        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(unknown.is_empty(), "unknown component kinds: {unknown:?}");

        // The whole layout is present at once — both tabs, gated (not a per-tab rebuild).
        for id in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE] {
            assert!(
                all.iter().any(|n| n.id == id),
                "the `{id}` pane is in the static tree"
            );
        }
        let view = all
            .iter()
            .find(|n| n.id == ui::VIEW_SLOT)
            .expect("the viewport is placed");
        assert_eq!(view.component, "surface", "the centre pane is the viewport");
        assert!(
            all.iter().any(|n| n.bind.as_deref() == Some(ui::FREQ_BIND)),
            "the size dial (bound to pop_freq) is present"
        );

        // Both tabs' slices are declared and gated on the keys `arrange()` lights.
        let gates: std::collections::HashSet<&str> = all
            .iter()
            .filter_map(|n| n.visible_bind.as_deref())
            .collect();
        assert!(
            gates.contains("shown_p0_t0"),
            "the map tab's slice is gated"
        );
        assert!(
            gates.contains("shown_p0_t1"),
            "the seams tab's slice is gated"
        );
    }
    /// **THE CLIMATE HISTORY ADVANCES WITH THE ERA — AND DIES WITH IT.** The ring
    /// the sparkline reads is the scene's, sampled once per SIM TICK (never per
    /// frame, never per procedure), and a restart empties it: a curve that outlived
    /// its era would be a readout of a world that no longer exists. The invariant
    /// the gate holds is the simplest one there is — one sample per tick run.
    #[test]
    fn the_climate_history_records_one_sample_per_tick_and_empties_with_the_era() {
        let mut bench = test_bench();
        let fire = |name: &str| {
            let mut r = ValueMap::default();
            r.set(name, true);
            r
        };
        assert!(
            bench.climate_history.is_empty(),
            "a fresh era has no history"
        );
        assert_eq!(
            bench.climate_history.capacity(),
            CLIMATE_HISTORY,
            "the ring is BOUNDED — a million-tick era costs these floats and no more"
        );
        // TICK: one complete cycle, one sample.
        for expect in 1..=3u64 {
            bench.apply_results(&fire(ui::EVOLVE_TICK_ACTION));
            assert_eq!(bench.evolve().ticks(), expect);
            assert_eq!(
                bench.climate_history.len() as u64,
                expect,
                "the ring tracks the era's own clock"
            );
        }
        // STEP: one PROCEDURE. The ring must not move until the step that closes
        // the cycle — the history counts ticks, not phases.
        let ticks = bench.evolve().ticks();
        let mut steps = 0;
        while bench.evolve().ticks() == ticks && steps < 64 {
            bench.apply_results(&fire(ui::EVOLVE_STEP_ACTION));
            steps += 1;
            assert_eq!(
                bench.climate_history.len() as u64,
                bench.evolve().ticks(),
                "a partial tick recorded a sample"
            );
        }
        assert!(steps > 1, "a tick really is several procedures ({steps})");
        assert_eq!(bench.evolve().ticks(), ticks + 1);
        // Every sample is the gauge's own fraction, so the fixed 0..1 range is honest.
        assert!(
            bench
                .climate_history
                .iter()
                .all(|v| (0.0..=1.05).contains(&v)),
            "the history holds the climate READING, not some other number"
        );
        // RESET returns the bare shell — and the history goes with the era.
        bench.apply_results(&fire(ui::EVOLVE_RESET_ACTION));
        assert_eq!(bench.evolve().ticks(), 0);
        assert!(
            bench.climate_history.is_empty(),
            "the curve must not outlive the era it measured"
        );
    }

    /// **The history readout actually gets its pixels** (rules 93B5000F): the plot's
    /// `surface` is reserved with real extent on the EVOLVE slice, is reserved on NO
    /// other slice (so the filler is unseated and free), and what the seated filler
    /// emits lands inside the well the walker gave it.
    #[test]
    fn the_climate_plot_is_seated_with_extent_on_the_evolve_slice_alone() {
        use flicker::render::Vec2;

        let mut bench = test_bench();
        let tree = bench.build_tree();
        let styles = bench.ui_styles.clone();
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let walk = |gate: &str| {
            let mut model = ValueMap::default();
            model.set("shown_page0", true);
            model.set(gate, true);
            let mut state = UiState::default();
            run_ui(&tree, &model, &styles, &snap, &mut state)
        };

        // The MAP slice reserves nothing for the plot — an unseated filler costs
        // nothing on every tab but its own.
        let frame = walk("shown_p0_t0");
        assert!(frame.surface(ui::TEMP_PLOT_SLOT).is_none());
        bench.climate_plot.seat(frame.surface(ui::TEMP_PLOT_SLOT));
        assert!(bench.climate_plot.rect().is_none());
        assert!(bench
            .climate_plot
            .commands(&bench.climate_history)
            .is_empty());

        // The EVOLVE slice reserves a real well under the climate gauge.
        let frame = walk("shown_p0_t3");
        let rect = frame
            .surface_rect(ui::TEMP_PLOT_SLOT)
            .expect("the history well is reserved on the evolve slice");
        assert!(
            rect.size.x > 100.0 && rect.size.y > 10.0,
            "the plot well has real pixels, got {}x{}",
            rect.size.x,
            rect.size.y
        );
        let gauge = frame
            .rect("pop_temp_gauge")
            .expect("the live gauge it reads under");
        assert!(
            rect.pos.y > gauge.pos.y,
            "the history sits UNDER the live gauge it belongs to"
        );
        // Seated, with a ring behind it, the filler draws — inside its own well.
        for i in 0..200 {
            bench
                .climate_history
                .push((i as f32 * 0.05).sin() * 0.5 + 0.5);
        }
        bench.climate_plot.seat(frame.surface(ui::TEMP_PLOT_SLOT));
        let cmds = bench.climate_plot.commands(&bench.climate_history);
        assert!(!cmds.is_empty(), "a seated plot over a full ring draws");
        for c in &cmds {
            let (x, y, w, h) = match c {
                HudCommand::Rect { x, y, w, h, .. } => (*x, *y, *w, *h),
                HudCommand::Line { from, to, .. } => (
                    from[0].min(to[0]),
                    from[1].min(to[1]),
                    (to[0] - from[0]).abs(),
                    (to[1] - from[1]).abs(),
                ),
                other => panic!("the plot emitted an unexpected command: {other:?}"),
            };
            assert!(
                x >= rect.pos.x - 0.01
                    && y >= rect.pos.y - 0.01
                    && x + w <= rect.pos.x + rect.size.x + 0.01
                    && y + h <= rect.pos.y + rect.size.y + 0.01,
                "the plot drew outside its well: {x},{y} {w}x{h} in {rect:?}"
            );
        }
    }

    /// **The plot's INK is authored, not invented** (the colour rule): the filler
    /// carries no palette, so every colour it draws with is a `plot.*` path in the
    /// scene's own styles resolving to a real token. A path that stops resolving is
    /// an INVISIBLE readout — loud in the log, silent on the screen — so the gate
    /// asserts the resolve rather than trusting it.
    #[test]
    fn the_plots_ink_resolves_from_the_scenes_own_style_block() {
        let bench = test_bench();
        for path in ["plot.line", "plot.fill", "plot.baseline", "plot.grid"] {
            let rgba = style_rgba(&bench.ui_styles, path);
            assert!(
                rgba[3] > 0.0,
                "`{path}` must resolve to a visible token, got {rgba:?}"
            );
        }
        // And an unauthored path is transparent rather than a stand-in colour.
        assert_eq!(style_rgba(&bench.ui_styles, "plot.nope"), [0.0; 4]);
    }
}
