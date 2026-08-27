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
//! The centre pane hands the bench's data core ([`crate::map::HexMap`], the hex
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
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_globe::{
    column_frame, lerp3, stippled, temp_color, tile_width, water_temp_color, Arrows, GlobeWorld,
    ShellSpec, RADIUS,
};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

use crate::crust::CrustField;
use crate::evolve::Evolution;
use crate::map::{HexMap, TileId, DEFAULT_FREQ, MAX_FREQ, MIN_FREQ};
use crate::plates::CONTINENT_H_FRAC;
use crate::seams::{SeamField, DEFAULT_CELLS, DEFAULT_SPOTS};
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
const R_MOTION: f32 = 1.012;

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
/// The bootstrap fast-forward's per-frame compute budget.
const BOOTSTRAP_FRAME_MS: u128 = 12;
/// The climate gauge's range (percent of the 0..1 climate scale) — the
/// ice-age runner wanders around the baseline a write to it sets.
const MIN_TEMP: u32 = 0;
const MAX_TEMP: u32 = 100;
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
const HAZE_ALT: f32 = 1.1;
const CLOUD_ALT: f32 = 2.1;
const ATMO_THICK: f32 = 0.3;
/// Presence thresholds on the moisture field: haze forms easily, a CLOUD is a
/// condensation zone (the uplift's own).
const HAZE_WET: f32 = 0.18;
const CLOUD_WET: f32 = 0.5;
/// The near-nothing alphas — see the ground THROUGH the weather, always.
const HAZE_ALPHA: f32 = 0.08;
const CLOUD_ALPHA: f32 = 0.15;

/// The three water layers' inks, deep to surface.
const DEEP_WATER_COLOR: [f32; 3] = [0.05, 0.10, 0.22];
const SHALLOW_WATER_COLOR: [f32; 3] = [0.10, 0.22, 0.38];
const SURFACE_WATER_COLOR: [f32; 3] = [0.20, 0.38, 0.52];
/// DRY-land reclassification levels, in tile-width units of TOTAL height
/// (Aaron: material that rises above the shelf and plate levels must take
/// the correct colour): below SHELF_LEVEL a dry tile is still bare bed;
/// between, it reads as shelf; above LAND_LEVEL it is land.
const SHELF_LEVEL: f32 = 0.30;
const LAND_LEVEL: f32 = CONTINENT_H_FRAC;

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

/// Classify one tile's rock: `total` in tile-width units.
fn ground_class(total: f32) -> Ground {
    if total >= LAND_LEVEL {
        Ground::Land
    } else if total >= SHELF_LEVEL {
        Ground::Shelf
    } else {
        Ground::Bed
    }
}

// (The sea level lives with the era now — `Evolution::sea_level`, the
// percentile of the era's own heights — so the dial has one derivation.)
/// The era's clock: ticks per second while running, and how many ticks pass
/// between REBAKES of the evolve view (the mesh is the heavy part, so the sim
/// steps faster than the picture).
const EVOLVE_HZ: f32 = 3.0;
const EVOLVE_BAKE_TICKS: u64 = 3;

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
    /// The clock's accumulator, and ticks since the view was last rebaked.
    evolve_accum: f32,
    evolve_unbaked: u64,
    /// The FAST ROLL's goal and start (0/0 = none): while the era's clock is
    /// under the goal the run loop computes flat-out with no baking, the
    /// progress bar filling from `roll_from`; arrival bakes once and pauses.
    roll_until: u64,
    roll_from: u64,
    /// Planetary water coverage, percent of surface flooded — the sea-level
    /// dial. DISPLAY + classification level today; the three water layers'
    /// temperature and circulation arrive with the erosion era. Changing it
    /// never resets the era (it is a lens, not a roll).
    /// Whether the plate MOTION ARROWS draw on the evolve view — a lens, like
    /// the water dial: toggling it resets nothing.
    show_arrows: bool,
    /// **One column of the world, up close** — the HEX page's view: the same
    /// component as `world`, framing the centre cell's molten column instead of
    /// the planet. A second VIEW, never a second world.
    hex: GlobeWorld,
    /// The CENTRE cell — the fixed reticle: on the seams tab, whichever tile
    /// faces the camera; the hex page shows this cell's column.
    focus_tile: TileId,
    /// The reticle ring currently drawn over the globe (`None` off the seams
    /// tab) — kept so the outline is rebuilt only when the faced cell changes.
    highlight: Option<TileId>,
    /// Which data the published tile shell is painted with — kept so a tab
    /// change rebuilds the world's colours exactly once, not per frame.
    shown_view: WorldView,
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
            evolve_accum: 0.0,
            evolve_unbaked: 0,
            roll_until: 0,
            roll_from: 0,
            show_arrows: true,
            hex,
            focus_tile: 0,
            highlight: None,
            shown_view: WorldView::Authored,
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
        let Self {
            map,
            world,
            seams,
            crust,
            evolve,
            ..
        } = self;
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
                        } else if h >= crate::crust::UPWELL_HEAT {
                            // THE GENERATION ZONE (Aaron 2026-08-25: the
                            // crust tab defines the places material is
                            // generated): everything above the upwell floor
                            // wears a clearly-bounded ember band, deepening
                            // toward lava as the heat climbs — the seam
                            // zones read as zones, the vents burn inside
                            // them.
                            let z =
                                (h - crate::crust::UPWELL_HEAT) / (1.0 - crate::crust::UPWELL_HEAT);
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
                        let mut c = match ground_class(total) {
                            Ground::Land => CONTINENT_COLOR,
                            Ground::Shelf => SHELF_COLOR,
                            Ground::Bed => OCEAN_BED_COLOR,
                        };
                        c = lerp3(c, ROCK_COLOR, (evolve.rock(t) * 1.2).clamp(0.0, 0.8));
                        let (n, _) = evolve.strata(t);
                        c = lerp3(c, STRATA_COLOR, (f32::from(n) / 3.0).clamp(0.0, 0.7));
                        if crust.is_vent(t) {
                            c = lerp3(c, LAVA_COLOR, 0.5);
                        }
                        // THE VEINS glow through like the lava nodes do — the
                        // bench's x-ray on the buried ore bodies, inked per
                        // kind (gold amber, coal black, calcite chalk…).
                        if let Some(k) = evolve.vein(t) {
                            c = lerp3(c, crate::evolve::vein_kinds()[k as usize].ink, 0.5);
                        }
                        // THE CAPS: standing ice whitens the ground toward
                        // frozen-through — the ice ages read at a glance.
                        let ice = evolve.ice(t);
                        if ice > 0.02 {
                            c = lerp3(
                                c,
                                ICE_COLOR,
                                (ice / crate::evolve::ICE_SOLID).clamp(0.0, 1.0) * 0.92,
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
                    let bands: [(f32, f32, [f32; 3], f32, f32); 3] = [
                        // (ceiling below sea, floor marker, colour, alpha, gloss)
                        (
                            sea - DEEP_DEPTH,
                            f32::NEG_INFINITY,
                            DEEP_WATER_COLOR,
                            0.62,
                            0.1,
                        ),
                        (
                            sea - SURFACE_DEPTH,
                            sea - DEEP_DEPTH,
                            SHALLOW_WATER_COLOR,
                            0.5,
                            0.2,
                        ),
                        (sea, sea - SURFACE_DEPTH, SURFACE_WATER_COLOR, 0.38, 0.45),
                    ];
                    for (band, (ceil, floor, colour, alpha, gloss)) in bands.into_iter().enumerate()
                    {
                        shells.push(ShellSpec {
                            dirs: &map.grid().dirs,
                            outlines: map.outlines(),
                            radius: RADIUS + ceil * w,
                            inset: 0.0,
                            color: Box::new(move |i| {
                                let t = i as TileId;
                                // Frozen through: thick ice REPLACES its
                                // water column — the cap is solid, not sea.
                                if ground(t) >= ceil || evolve.ice(t) >= crate::evolve::ICE_SOLID {
                                    return None;
                                }
                                // THE OCEAN'S OWN HEAT tints each band by ITS
                                // temperature — surface tracked per tile,
                                // deep the one global reservoir, shallow the
                                // mix (bands are bottom-up: 0 deep, 2 surface).
                                let (sst, mid, deep) = evolve.ocean_temps(t);
                                let bt = [deep, mid, sst][band];
                                Some(lerp3(colour, water_temp_color(bt), 0.45))
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
                    // THE ATMOSPHERE — the first two layers, as near-nothing
                    // fog cells: a low HAZE and the condensation-deck CLOUDS,
                    // brightness riding the moisture field the weathering
                    // drinks. A mountain taller than a deck pokes through it
                    // (its cell simply is not there). Drawn last: the most
                    // transparent thing in the scene.
                    let atmo: [(f32, f32, f32); 2] = [
                        (HAZE_ALT, HAZE_WET, HAZE_ALPHA),
                        (CLOUD_ALT, CLOUD_WET, CLOUD_ALPHA),
                    ];
                    for (alt, wet_floor, alpha) in atmo {
                        let deck = sea + alt;
                        shells.push(ShellSpec {
                            dirs: &map.grid().dirs,
                            outlines: map.outlines(),
                            radius: RADIUS + deck * w,
                            inset: 0.0,
                            color: Box::new(move |i| {
                                let t = i as TileId;
                                let m = evolve.moisture(t);
                                (m >= wet_floor && ground(t) < deck).then(|| {
                                    // Denser moisture = whiter fog.
                                    let b = 0.55 + 0.45 * m;
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
        world.bake(view.key(), shells);
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
        let compact = ((evolve.bed_hardness(tile) - 1.0) / (crate::evolve::MARINE_HARD_CAP - 1.0))
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
                    crate::evolve::vein_kinds()[k as usize].ink,
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
        let bands = [
            (
                lerp3(DEEP_WATER_COLOR, water_temp_color(b_deep), 0.45),
                (depth - DEEP_DEPTH).max(0.0),
                0.62,
                0.1,
            ),
            (
                lerp3(SHALLOW_WATER_COLOR, water_temp_color(b_mid), 0.45),
                (depth.min(DEEP_DEPTH) - SURFACE_DEPTH).max(0.0),
                0.5,
                0.2,
            ),
            (
                lerp3(SURFACE_WATER_COLOR, water_temp_color(b_sst), 0.45),
                depth.min(SURFACE_DEPTH),
                0.38,
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
        // The ATMOSPHERE's two cells over the column — near-nothing fog,
        // present only where this tile's moisture reaches each layer's floor.
        let m = evolve.moisture(tile);
        for (wet_floor, alpha) in [(HAZE_WET, HAZE_ALPHA), (CLOUD_WET, CLOUD_ALPHA)] {
            if m < wet_floor {
                continue;
            }
            let hw = ATMO_THICK * w;
            top += gap + hw;
            let b = 0.55 + 0.45 * m;
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
        if let Some(tile) = *highlight {
            let ring = map.outline(tile);
            let n = ring.len();
            let mut segs = Vec::with_capacity(n * RETICLE_RINGS.len());
            for scale in RETICLE_RINGS {
                for k in 0..n {
                    segs.push((ring[k] * RADIUS * scale, ring[(k + 1) % n] * RADIUS * scale));
                }
            }
            overlays.push((RETICLE_INK, segs));
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
        m.set(ui::PAGE_BIND, self.sel_page as f64);
        m.set(ui::TAB_BIND, self.sel_tab() as f64);
        m.set(ui::TABS_SHOWN, !ui::page(self.sel_page).tabs.is_empty());
        m.set(ui::FREQ_BIND, f64::from(self.map.freq()));
        m.set(ui::CELLS_BIND, f64::from(self.seams.cells()));
        m.set(ui::SPOTS_BIND, f64::from(self.seams.spots()));
        m.set(ui::HEXES_BIND, group_thousands(self.map.len() as u64));
        m.set(
            ui::DIAMETER_BIND,
            group_thousands(crate::map::diameter_mi(self.map.freq()).round() as u64),
        );
        m.set(ui::TILE_BIND, format!("{:.2}", crate::map::TILE_MI));
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
        m.set(
            ui::WATER_BIND,
            f64::from((self.evolve.coverage() * 100.0).round() as u32),
        );
        m.set(
            ui::WATER_TARGET_BIND,
            f64::from((self.evolve.water_target() * 100.0).round() as u32),
        );
        m.set(
            ui::TEMP_BIND,
            f64::from((self.evolve.climate() * 100.0).round() as u32),
        );
        m.set(ui::ARROWS_BIND, self.show_arrows);
        m.set(ui::TICKS_BIND, group_thousands(self.evolve.ticks()));
        // The material census TABLE: two columns per row (label | hexes),
        // most-common first — labels are registry notation, counts formatted
        // here like every readout. Rows past the roster fold into a final
        // "+K" row carrying the remaining hexes; unused rows publish empty
        // strings and take no ink.
        let census = self.evolve.vein_census();
        let kinds = crate::evolve::vein_kinds();
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
        // moved.
        self.evolve.reset(&self.map, &self.seams);
        self.evolve.set_water(DEFAULT_WATER as f32);
        self.bake_view(WorldView::Authored);
        self.bake_molten_views();
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
                self.evolve.reset(&self.map, &self.seams);
                self.evolve.set_water(DEFAULT_WATER as f32);
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
                self.evolve.reset(&self.map, &self.seams);
                self.evolve.set_water(DEFAULT_WATER as f32);
                self.bake_molten_views();
                self.publish_hex();
            }
        }
        // The water dial: the sea level's coverage. A LENS on the evolve
        // view — it rebakes the picture and resets NOTHING: an hour of era is
        // never the price of trying a different ocean.
        // TWO SLIDERS (Aaron 2026-08-26): `pop_water` is the LIVE coverage
        // GAUGE — display only, its knob rides the world and a hand on it
        // changes nothing — and `pop_water_target` is the CONTROL: the
        // coverage share the in-fall pursues. A plain dial: committed number
        // lands, echo inert, wild clamps.
        if let Some(v) = results.number(ui::WATER_TARGET_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(MIN_WATER, MAX_WATER);
            if want != (self.evolve.water_target() * 100.0).round() as u32 {
                self.evolve.set_water_target(want as f32 / 100.0);
            }
        }
        // The climate gauge: the model publishes the runner's LIVE reading
        // every frame (the knob moves with the glacials), so the echo returns
        // that same number — only a write that DISAGREES with the live
        // reading is a hand on the dial, and it sets the BASELINE the runner
        // wanders around. A lens on the era's weather: nothing resets.
        if let Some(v) = results.number(ui::TEMP_BIND) {
            let live = f64::from((self.evolve.climate() * 100.0).round());
            if (v - live).abs() > 0.6 {
                let pct = (v.round().max(0.0) as u32).clamp(MIN_TEMP, MAX_TEMP);
                self.evolve.set_climate(pct as f32 / 100.0);
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
        // The era's three controls: run/pause the clock, one deliberate step,
        // and back to the bare shell.
        if results.is_on(ui::EVOLVE_RUN_ACTION) {
            self.evolve_running = !self.evolve_running;
            self.evolve_accum = 0.0;
            // The FIRST click on a fresh world queues the bootstrap roll;
            // pausing mid-roll keeps the goal, so Run resumes it.
            if self.evolve_running && self.evolve.ticks() == 0 && self.roll_until == 0 {
                self.roll_from = 0;
                self.roll_until = crate::evolve::BOOTSTRAP_TICKS;
            }
        }
        // TICK 1200 (Aaron 2026-08-26: "what will it look like at 4500?
        // Let's FIND OUT!"): queue another bootstrap-sized leap from wherever
        // the clock stands — computed flat-out, no baking, progress barred,
        // baked once and paused on arrival.
        if results.is_on(ui::EVOLVE_ROLL_ACTION) {
            self.roll_from = self.evolve.ticks();
            self.roll_until = self.evolve.ticks() + crate::evolve::BOOTSTRAP_TICKS;
            self.evolve_running = true;
            self.evolve_accum = 0.0;
        }
        if results.is_on(ui::EVOLVE_STEP_ACTION) {
            let Self {
                map,
                seams,
                crust,
                evolve,
                ..
            } = &mut *self;
            let sea = evolve.resolve_sea();
            evolve.tick(map, seams, crust, sea);
            self.drift_fields();
            self.bake_view(WorldView::Evolve);
            self.publish_hex();
        }
        if results.is_on(ui::EVOLVE_RESET_ACTION) {
            self.roll_until = 0;
            self.roll_from = 0;
            self.evolve.reset(&self.map, &self.seams);
            self.evolve.set_water(DEFAULT_WATER as f32);
            self.evolve_running = false;
            self.bake_view(WorldView::Evolve);
            self.publish_hex();
        }
        // The randomize button: a new roll of the same count — the seams move,
        // both views repaint.
        if results.is_on(ui::SEAMS_ACTION) {
            self.seams.randomize(&self.map);
            self.crust = CrustField::derive(&self.map, &self.seams);
            self.evolve.reset(&self.map, &self.seams);
            self.evolve.set_water(DEFAULT_WATER as f32);
            self.bake_molten_views();
            self.publish_hex();
        }
    }
}

/// The PROCEDURE label's stringtable token for a pipeline phase — published
/// on a bind, resolved by the walker like any `$token` (the godmode pattern).
fn phase_token(p: crate::evolve::Phase) -> &'static str {
    use crate::evolve::Phase;
    match p {
        Phase::Climate => "$pop_phase_climate",
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
            typed: String::new(),
            backspace: false,
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
        // The pointer SAMPLE for the globe's surface — the walker's barrier (A8C9F02B
        // §4b): present while the cursor is over the planet with no UI over it, or while
        // a press that began there is still held. The scene reads no device for it.
        let pointer = frame.surface_pointer(ui::VIEW_SLOT).cloned();
        let hex_pointer = frame.surface_pointer(ui::HEX_SLOT).cloned();
        self.hud_commands = frame.commands;
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
                    let kind = &crate::evolve::vein_kinds()[k as usize];
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
        self.apply_results(&results);

        // One camera line, and it is the WORLD's: the look and zoom come from the PUMP's
        // continuous queries (`signals.axis`, never a device), and the globe answers them
        // only while its own panel holds the walker's cursor, taking the pointer only
        // through the walker's surface capture. The six Look/Zoom signals stay the camera's
        // (`look_from`).
        let dtf = dt.as_secs_f32();
        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        // The globe answers look/zoom only while its pane is ENTERED (nav-tier contract
        // 1B5F6BB8): merely highlighting the viewport pane no longer feeds the camera —
        // Confirm locks into it, Cancel backs out. The LOCKED pane's `tab_group` IS the
        // gate the globe matches against (`in_panel`); the walker owns it, the scene only
        // reads it, never a second focus system (F2). Entering a DIFFERENT pane yields that
        // pane's group, so the globe correctly stays quiet. Both centre-pane views name
        // the same pane — the SELECTED PAGE decides which of the two the entered pane
        // hands the camera to, and the dark one holds still.
        let look_gate = self.ui_state.entered_group();
        let still = ((0.0, 0.0, 0.0), None::<&str>);
        let ((w_look, w_gate), (h_look, h_gate)) = if self.hex_view() {
            (still, (look, look_gate))
        } else {
            ((look, look_gate), still)
        };
        self.world.update(dtf, pointer.as_ref(), w_look, w_gate);
        self.hex.update(dtf, hex_pointer.as_ref(), h_look, h_gate);

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

        // The era's clock: ticks only while its view is WATCHED and running —
        // a slow iteration you see grow. The sim steps at EVOLVE_HZ; the mesh
        // rebakes every few ticks (the picture is the heavy part).
        if self.shown_view == WorldView::Evolve && self.evolve_running {
            self.evolve_accum += dtf;
            let mut ticked = false;
            if self.roll_until > 0 && self.evolve.ticks() < self.roll_until {
                // THE BOOTSTRAP ROLL (Aaron 2026-08-26): from the first
                // click the era runs FLAT-OUT to the horizon before the
                // first weld and display — full cycles under a per-frame
                // compute budget, no baking, the tick and procedure
                // readouts spinning so the roll is visible. Arrival falls
                // through to the normal cadence below, which bakes.
                let start = std::time::Instant::now();
                while self.evolve.ticks() < self.roll_until
                    && start.elapsed().as_millis() < BOOTSTRAP_FRAME_MS
                {
                    let Self {
                        map,
                        seams,
                        crust,
                        evolve,
                        ..
                    } = &mut *self;
                    let sea = evolve.resolve_sea();
                    evolve.tick(map, seams, crust, sea);
                    self.drift_fields();
                }
                if self.evolve.ticks() >= self.roll_until {
                    ticked = true; // the weld: bake and show the rolled world
                    self.evolve_unbaked = EVOLVE_BAKE_TICKS;
                    // …and the sim STOPS at the goal (Aaron 2026-08-26): the
                    // world stands for inspection; Run resumes, TICK 1200
                    // leaps again.
                    self.evolve_running = false;
                    self.roll_until = 0;
                    self.roll_from = 0;
                }
                self.evolve_accum = 0.0;
            } else {
                // The engine steps PROCEDURES: one pipeline phase per step,
                // at a rate that keeps the completed-cycle (tick) cadence at
                // EVOLVE_HZ — the phase label is a real live readout, and
                // the sim's throughput is unchanged.
                let step = 1.0 / (EVOLVE_HZ * crate::evolve::PHASES.len() as f32);
                // At most two CYCLES' worth of steps a frame — a long frame
                // never spirals.
                for _ in 0..(2 * crate::evolve::PHASES.len()) {
                    if self.evolve_accum < step {
                        break;
                    }
                    self.evolve_accum -= step;
                    let Self {
                        map,
                        seams,
                        crust,
                        evolve,
                        ..
                    } = &mut *self;
                    let sea = evolve.resolve_sea();
                    if evolve.tick_phase(map, seams, crust, sea) {
                        ticked = true;
                        self.drift_fields();
                    }
                }
            }
            if ticked {
                self.apply_overlays(); // the arrows fill toward the next step
                self.evolve_unbaked += 1;
                if self.evolve_unbaked >= EVOLVE_BAKE_TICKS {
                    self.evolve_unbaked = 0;
                    self.bake_view(WorldView::Evolve);
                    self.publish_hex();
                }
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
        Transition::None
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // Declare-only: the globe goes into the rect the walker reserved, and the HUD
        // replay is the screen surface's final 2D — one overlay, run after the composites.
        // The destructure splits the disjoint borrows (`world` &mut, `hud_commands` /
        // `textures` shared) so both survive into the graph until the manager's `execute`.
        let Self {
            world,
            hex,
            hud_commands,
            textures,
            ..
        } = self;
        let layer = fg.base_layer();
        world.render(renderer, fg, layer);
        hex.render(renderer, fg, layer);
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
            "checkbox",       // the evolve tab's motion-arrows lens
            "button",         // the seams action
            "resource_gauge", // the bootstrap roll's progress bar (the loading bar component)
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
            3,
            "the root surface + the globe's viewport + the hex stack's viewport"
        );
        assert_eq!(
            count("slider"),
            6,
            "the size, cells, spots, water-gauge, water-target and climate dials"
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
                "panel" | "cell" | "row" | "text" | "resource_gauge"
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
        // button, and the hex page's placeholders coexist.
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
            placeholders, 5,
            "the seams stats pane, both crust panes and the hex page's two side panes rest on placeholders"
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
            typed: String::new(),
            backspace: false,
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
            crate::seams::DEFAULT_CELLS,
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
            crate::seams::MAX_CELLS,
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
            Some(&Value::Number(f64::from(crate::seams::MIN_CELLS)))
        );
        assert_eq!(
            dial.props.get("max"),
            Some(&Value::Number(f64::from(crate::seams::MAX_CELLS)))
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
                f64::from(crate::seams::MIN_CELLS),
                f64::from(crate::seams::MAX_CELLS),
            ),
            (
                ui::SPOTS_BIND,
                f64::from(crate::seams::MIN_SPOTS),
                f64::from(crate::seams::MAX_SPOTS),
            ),
            (ui::WATER_BIND, f64::from(MIN_WATER), f64::from(MAX_WATER)),
            (
                ui::WATER_TARGET_BIND,
                f64::from(MIN_WATER),
                f64::from(MAX_WATER),
            ),
            (ui::TEMP_BIND, f64::from(MIN_TEMP), f64::from(MAX_TEMP)),
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
        bench.apply_results(&fire(ui::EVOLVE_STEP_ACTION));
        assert_eq!(bench.evolve().ticks(), 1, "STEP advances exactly one tick");
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
        assert_eq!(ground_class(SHELF_LEVEL - 0.05), Ground::Bed);
        assert_eq!(ground_class(SHELF_LEVEL + 0.05), Ground::Shelf);
        assert_eq!(ground_class(LAND_LEVEL + 0.05), Ground::Land);

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
    fn tick_1200_queues_a_leap_and_reset_clears_it() {
        let mut bench = test_bench();
        assert_eq!(bench.roll_window(), None);
        // First Run queues the bootstrap roll.
        let mut run = ValueMap::default();
        run.set(ui::EVOLVE_RUN_ACTION, true);
        bench.apply_results(&run);
        assert!(bench.evolve_running());
        assert_eq!(
            bench.roll_window(),
            Some((0, crate::evolve::BOOTSTRAP_TICKS)),
            "the first click queues the bootstrap window"
        );
        // Pausing keeps the goal (Run resumes the roll).
        bench.apply_results(&run);
        assert!(!bench.evolve_running());
        assert!(bench.roll_window().is_some(), "pausing keeps the goal");

        // A step moves the clock; TICK 1200 leaps from wherever it stands.
        let mut step = ValueMap::default();
        step.set(ui::EVOLVE_STEP_ACTION, true);
        bench.apply_results(&step);
        let now = bench.evolve().ticks();
        let mut roll = ValueMap::default();
        roll.set(ui::EVOLVE_ROLL_ACTION, true);
        bench.apply_results(&roll);
        assert!(bench.evolve_running(), "the leap runs");
        assert_eq!(
            bench.roll_window(),
            Some((now, now + crate::evolve::BOOTSTRAP_TICKS)),
            "the window opens at the standing clock"
        );

        // Reset clears the world AND the roll.
        let mut reset = ValueMap::default();
        reset.set(ui::EVOLVE_RESET_ACTION, true);
        bench.apply_results(&reset);
        assert_eq!(bench.roll_window(), None, "reset clears the goal");
        assert_eq!(bench.evolve().ticks(), 0);
    }

    /// **Two sliders: the GAUGE shows, the TARGET controls** (Aaron
    /// 2026-08-26). The live gauge opens at the water world and a hand on it
    /// changes NOTHING — display only. The target dial is a plain control:
    /// its committed number lands on the era's coverage target, its echo is
    /// inert, a wild number clamps — and none of it touches the clock or any
    /// roll: the IN-FALL, not a re-pour, walks the world toward the target.
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
            crate::seams::DEFAULT_SPOTS,
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
            crate::seams::MAX_SPOTS,
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
            Some(&Value::Number(f64::from(crate::seams::MIN_SPOTS)))
        );
        assert_eq!(
            dial.props.get("max"),
            Some(&Value::Number(f64::from(crate::seams::MAX_SPOTS)))
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
            typed: String::new(),
            backspace: false,
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
            typed: String::new(),
            backspace: false,
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
        use flicker_input_core::EventKind;

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

        // The next `run_ui` pass applies the nudge with the NODE's own step and
        // writes the bind — the component's committed write, one step up.
        let snap = UiInput {
            mouse: Vec2::ZERO,
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1600.0, 900.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &model, &styles, &snap, &mut ui);
        assert_eq!(
            frame.results.number(ui::FREQ_BIND),
            Some(97.0),
            "NavUp stepped the focused vertical dial by its own step"
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
}
