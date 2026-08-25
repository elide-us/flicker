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

use flicker::render::{FrameGraph, Renderer, TextureHandle, Vec3};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_globe::{column_frame, temp_color, tile_width, GlobeWorld, ShellSpec, RADIUS};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};

use crate::crust::CrustField;
use crate::map::{HexMap, TileId, DEFAULT_FREQ, MAX_FREQ, MIN_FREQ};
use crate::plates::{PlateField, DEFAULT_PLATES};
use crate::seams::{SeamField, DEFAULT_CELLS, DEFAULT_SPOTS};
use crate::ui;

/// The hex-stack view's opening framing — the framed region fills this share of
/// the viewport, which puts the stack's columns at about a tenth of the panel's
/// width: small at the bottom, with the room the ~50-cell stack above will need.
const HEX_FILL: f32 = 0.85;
/// How many tile-widths across the hex view frames. Five tiles across ⇒ the
/// column is ~1/10 of the viewport at the opening fill.
const HEX_FRAME_TILES: f32 = 5.0;
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
/// A continental plate — this is LAND (there is no water yet), so it reads
/// brown (Aaron 2026-08-25: "the ocean beds are probably more grey and land
/// is more on the brown side").
const CONTINENT_COLOR: [f32; 3] = [0.42, 0.32, 0.21];
/// An oceanic plate — a bare rock bed, grey, not sea-blue: nothing has filled
/// it with water yet.
const OCEAN_BED_COLOR: [f32; 3] = [0.33, 0.34, 0.36];
/// CONTINENTAL SHELF — the transitional zone where a bed meets a continent,
/// the ONE edge the surface marks (plate joins between two beds or two
/// continents paint nothing). A sandy tone between the two kinds.
const SHELF_COLOR: [f32; 3] = [0.56, 0.49, 0.35];
/// The plate cell's BASE heights, per kind (per cell width `w`): a continent
/// is THICK crust riding high, an ocean bed is a thin veneer — the difference
/// the erosion era will carve against.
const CONTINENT_H_FRAC: f32 = 0.5;
const OCEAN_BED_H_FRAC: f32 = 0.125;
/// How much ELEVATION the molten seams push into the plate shell, per cell
/// width, at full heat (Aaron 2026-08-25: heat = pressure = volume — the
/// hotter the seam below, the taller this layer's cell). Rides ON TOP of the
/// kind base, for both kinds: a hot seam under an ocean bed is a ridge.
const ELEV_H_FRAC: f32 = 2.0;
/// How far below the nominal surface a plate column's walls reach (per cell
/// width) — the root that keeps neighbouring cells of different heights
/// reading as EXTRUDED solids, never floating caps.
const PLATE_ROOT_FRAC: f32 = 0.25;

/// **The plate shell's per-cell height** — the composition Aaron specified:
/// the KIND's base thickness (thin bed / thick shelf, the plates' own data)
/// plus the seam-derived elevation (the molten layer's data, a SEPARATE
/// channel: colour stays the plate's, geometry carries the heat below).
fn plate_height(plates: &PlateField, seams: &SeamField, tile: TileId, w: f32) -> f32 {
    let base = if plates.is_continent(tile) {
        CONTINENT_H_FRAC
    } else {
        OCEAN_BED_H_FRAC
    };
    w * (base + ELEV_H_FRAC * seams.heat(tile))
}

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
    Plates,
}

impl WorldView {
    /// The baked set this view draws from.
    fn key(self) -> &'static str {
        match self {
            WorldView::Authored => "authored",
            WorldView::Heat => "heat",
            WorldView::Crust => "crust",
            WorldView::Plates => "plates",
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
    sel_tab: usize,

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
    /// **The tectonic shell** — continents, ocean beds and plate boundaries.
    /// Its OWN roll, deliberately unrelated to the molten layers below
    /// (Aaron 2026-08-25): the erosion era starts from this scheme.
    plates: PlateField,
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
        let plates = PlateField::new(&map, DEFAULT_PLATES, fastrand::u64(..));

        let mut bench = Self {
            sel_page: 0,
            sel_tab: 0,
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
            plates,
            hex,
            focus_tile: 0,
            highlight: None,
            shown_view: WorldView::Authored,
        };
        // The opening reticle: whatever the default camera faces.
        bench.focus_tile = bench
            .world
            .facing(&bench.map.grid().dirs)
            .unwrap_or(0) as TileId;
        // Bake EVERY view up front — data changes re-bake theirs, and a tab
        // switch is then a free swap.
        bench.bake_view(WorldView::Authored);
        bench.bake_molten_views();
        bench.world.show(bench.shown_view.key());
        bench.publish_hex();
        bench
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
        match page.tabs[self.sel_tab.min(page.tabs.len() - 1)].id {
            "seams" => WorldView::Heat,
            "crust" => WorldView::Crust,
            "plates" => WorldView::Plates,
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
            plates,
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
                    top.color = Box::new(|i| {
                        Some(if crust.is_vent(i as TileId) {
                            LAVA_COLOR
                        } else {
                            BEDROCK_COLOR
                        })
                    });
                }
                WorldView::Plates => {
                    // Reborrow the destructured `&mut`s as SHARED refs (Copy),
                    // so all three closures can carry their own copy.
                    let plates: &PlateField = plates;
                    let seams: &SeamField = seams;
                    top.color = Box::new(move |i| {
                        let t = i as TileId;
                        Some(if plates.is_shelf(t) {
                            SHELF_COLOR
                        } else if plates.is_continent(t) {
                            CONTINENT_COLOR
                        } else {
                            OCEAN_BED_COLOR
                        })
                    });
                    // The RELIEF channel, separate from the colour: each cell
                    // stands at its own height — kind base + the heat of the
                    // seam beneath — as an EXTRUDED column whose walls root
                    // below the surface, so uneven neighbours read as solids
                    // sticking out of the shell, not floating caps.
                    let w = map
                        .tiles()
                        .next()
                        .map(|t| tile_width(map.direction(t), map.outline(t), RADIUS))
                        .unwrap_or(0.0);
                    top.cell_radius = Some(Box::new(move |i| {
                        RADIUS + plate_height(plates, seams, i as TileId, w)
                    }));
                    top.depth = Some(Box::new(move |i| {
                        plate_height(plates, seams, i as TileId, w) + w * PLATE_ROOT_FRAC
                    }));
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
        self.bake_view(WorldView::Plates);
    }

    /// The selection moved — SHOW its baked set. A swap, not a rebuild: the
    /// meshes were baked when their data changed, so nothing stale lingers on
    /// screen while a 92k-tile view rebuilds (the flash Aaron reported).
    fn refresh_world_view(&mut self) {
        if self.world_view() != self.shown_view {
            self.shown_view = self.world_view();
            self.world.show(self.shown_view.key());
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
            plates,
            focus_tile,
            ..
        } = self;
        let tile = (*focus_tile).min(map.len().saturating_sub(1) as TileId);
        let dir = map.direction(tile);
        let ring = column_frame(dir, map.outline(tile));
        let w = tile_width(dir, map.outline(tile), RADIUS);
        let continent = plates.is_continent(tile);
        // The plate cell's DERIVED height: kind base + the seam heat below —
        // the same composition the plates view extrudes on the globe.
        let h_plate = plate_height(plates, seams, tile, w);
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
            LAVA_COLOR
        } else {
            BEDROCK_COLOR
        };
        let plate = if plates.is_shelf(tile) {
            SHELF_COLOR
        } else if continent {
            CONTINENT_COLOR
        } else {
            OCEAN_BED_COLOR
        };
        let dirs = [Vec3::Y];
        let outlines = [ring];
        hex.set_shells(vec![
            ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: RADIUS - h_plate - gap - h_bed - gap,
                inset: 0.0,
                color: Box::new(move |_| Some(molten)),
                cell_radius: None,
                depth: Some(Box::new(move |_| h_molten)),
            },
            ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: RADIUS - h_plate - gap,
                inset: 0.0,
                color: Box::new(move |_| Some(bedrock)),
                cell_radius: None,
                depth: Some(Box::new(move |_| h_bed)),
            },
            ShellSpec {
                dirs: &dirs,
                outlines: &outlines,
                radius: RADIUS,
                inset: 0.0,
                color: Box::new(move |_| Some(plate)),
                cell_radius: None,
                depth: Some(Box::new(move |_| h_plate)),
            },
        ]);
    }

    /// The reticle over the globe: the centre cell's outline, twice, slightly
    /// raised — a BOLD ring on the cell the camera faces. Drawn over the stage's
    /// own reference frame (set_arrows re-lays the graticule under it).
    fn apply_highlight(&mut self) {
        let Self {
            map,
            world,
            highlight,
            ..
        } = self;
        let Some(tile) = *highlight else {
            world.set_arrows(Vec::new());
            return;
        };
        let ring = map.outline(tile);
        let n = ring.len();
        let mut segs = Vec::with_capacity(n * RETICLE_RINGS.len());
        for scale in RETICLE_RINGS {
            for k in 0..n {
                segs.push((ring[k] * RADIUS * scale, ring[(k + 1) % n] * RADIUS * scale));
            }
        }
        world.set_arrows(vec![(RETICLE_INK, segs)]);
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

    /// The tectonic shell — the plates tab's data, its own independent roll.
    pub fn plates(&self) -> &PlateField {
        &self.plates
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
        (self.sel_page, self.sel_tab)
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
        m.set(ui::TAB_BIND, self.sel_tab as f64);
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
        self.plates.rebuild(&self.map);
        // The centre cell is SHARED state — keep it a tile the new map has;
        // the old reticle outlined tiles that no longer exist, so it comes
        // down and the next frame on the seams tab re-faces it.
        self.focus_tile = self.focus_tile.min(self.map.len().saturating_sub(1) as TileId);
        self.highlight = None;
        self.apply_highlight();
        // A new tiling: every view's geometry moved.
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
        if let Some(v) = results.number(ui::PAGE_BIND) {
            let want = (v.round().max(0.0) as usize).min(pages.saturating_sub(1));
            if want != self.sel_page {
                self.sel_page = want;
                self.sel_tab = 0; // a new page's tabs are its own
                self.refresh_world_view();
            }
        }
        if let Some(v) = results.number(ui::TAB_BIND) {
            let tabs = ui::page(self.sel_page).tabs.len();
            let want = (v.round().max(0.0) as usize).min(tabs.saturating_sub(1));
            if want != self.sel_tab {
                self.sel_tab = want;
                self.refresh_world_view();
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
                self.bake_molten_views();
                self.publish_hex();
            }
        }
        // The plates dial: how many plates the tectonic shell tiles into. Its
        // own field, its own arm — the molten layers never hear about it.
        if let Some(v) = results.number(ui::PLATES_BIND) {
            let before = self.plates.plates();
            self.plates.set_plates(&self.map, v.round().max(0.0) as u32);
            if self.plates.plates() != before {
                self.bake_view(WorldView::Plates);
                self.publish_hex();
            }
        }
        // The plates randomize: a new roll of the SHELL alone — the molten
        // seams and spots stand exactly where they were.
        if results.is_on(ui::PLATES_ACTION) {
            self.plates.randomize(&self.map);
            self.bake_view(WorldView::Plates);
            self.publish_hex();
        }
        // The randomize button: a new roll of the same count — the seams move,
        // both views repaint.
        if results.is_on(ui::SEAMS_ACTION) {
            self.seams.randomize(&self.map);
            self.crust = CrustField::derive(&self.map, &self.seams);
            self.bake_molten_views();
            self.publish_hex();
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
        self.world
            .update(dtf, pointer.as_ref(), w_look, w_gate);
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
                    self.apply_highlight();
                    self.publish_hex();
                }
            }
        } else if self.highlight.take().is_some() {
            self.apply_highlight();
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
            "pill_toggle", // the PTT's authored page + tab rails
            "panel",       // UI Panel and RTT Panel
            "slider",      // the size dial
            "button",      // the seams action
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
            4,
            "the size, cells, spots and plates dials"
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
        let mut bound: Vec<&str> = texts
            .iter()
            .filter_map(|n| match n.props.get("text_bind") {
                Some(Value::Text(t)) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        bound.sort_unstable();
        assert_eq!(
            bound,
            [ui::DIAMETER_BIND, ui::HEXES_BIND, ui::TILE_BIND],
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
            nodes
                .iter()
                .all(|n| matches!(n.component.as_str(), "panel" | "cell" | "row" | "text")),
            "the stats pane is display-only (its slice gate is a plain `cell`)"
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
            placeholders, 6,
            "the seams stats pane, both crust panes, the plates stats pane and the hex page's two side panes rest on placeholders"
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
        // A wild page write clamps into the roster and LANDS — and a real page
        // change resets the tab, because a new page's tabs are its own.
        let mut wild_page = ValueMap::default();
        wild_page.set(ui::PAGE_BIND, 9.0);
        bench.apply_results(&wild_page);
        assert_eq!(
            bench.selection(),
            (ui::PAGES.len() - 1, 0),
            "clamped to the last page, tab reset"
        );
        let mut home = ValueMap::default();
        home.set(ui::PAGE_BIND, 0.0);
        bench.apply_results(&home);
        assert_eq!(bench.selection(), (0, 0), "back on the world page");
        let mut seams_tab = ValueMap::default();
        seams_tab.set(ui::TAB_BIND, 1.0);
        bench.apply_results(&seams_tab);
        // A real click on the first pill jumps back.
        let mut back = ValueMap::default();
        back.set(ui::TAB_BIND, 0.0);
        bench.apply_results(&back);
        assert_eq!(bench.selection(), (0, 0));
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
            (
                ui::PLATES_BIND,
                f64::from(crate::plates::MIN_PLATES),
                f64::from(crate::plates::MAX_PLATES),
            ),
        ];
        let tree = test_bench().build_tree();
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
        }
    }

    /// **The plates dial + randomize drive their arms, and the shell's roll is
    /// INDEPENDENT at the dispatcher level.** The committed count lands at the
    /// same roll, the echo is inert, a wild number clamps — and the plates
    /// randomize moves the continents while the molten seams, spots and the
    /// crust's vents all stand exactly where they were (and vice versa: the
    /// molten randomize leaves the plates alone). Aaron's independence
    /// mandate, asserted where the buttons actually fire.
    #[test]
    fn the_plates_controls_drive_their_arms_independently() {
        let mut bench = test_bench();
        let write = |n: f64| {
            let mut r = ValueMap::default();
            r.set(ui::PLATES_BIND, n);
            r
        };
        let seed = bench.plates().seed();
        bench.apply_results(&write(20.0));
        assert_eq!(bench.plates().plates(), 20, "the committed count lands");
        assert_eq!(bench.plates().seed(), seed, "same roll, more plates");
        bench.apply_results(&write(20.0)); // the resting echo
        assert_eq!(bench.plates().plates(), 20);
        bench.apply_results(&write(99.0));
        assert_eq!(
            bench.plates().plates(),
            crate::plates::MAX_PLATES,
            "a wild number clamps"
        );

        // The plates re-roll moves ONLY the plates…
        let molten_seed = bench.seams().seed();
        let heats = bench.seams().heats().to_vec();
        let vents = bench.crust().vents().to_vec();
        let mut fired = ValueMap::default();
        fired.set(ui::PLATES_ACTION, true);
        bench.apply_results(&fired);
        assert_ne!(bench.plates().seed(), seed, "the shell re-rolled");
        assert_eq!(bench.seams().seed(), molten_seed, "the molten roll held");
        assert_eq!(bench.seams().heats(), &heats[..], "the heat stood still");
        assert_eq!(bench.crust().vents(), &vents[..], "the vents stood still");

        // …and the molten re-roll leaves the plates alone.
        let plate_seed = bench.plates().seed();
        let mut molten = ValueMap::default();
        molten.set(ui::SEAMS_ACTION, true);
        bench.apply_results(&molten);
        assert_eq!(bench.plates().seed(), plate_seed, "the shell held");
        assert_ne!(bench.seams().seed(), molten_seed, "the molten re-rolled");
    }

    /// **The plate shell's height composes the two channels Aaron separated:**
    /// the KIND's base (thick shelf / thin bed — the plates' own data) plus
    /// the seam-derived elevation (the molten heat below; heat = pressure =
    /// volume). At equal heat a continent stands taller than a bed; on either
    /// kind a hotter seam pushes the cell higher; and the heat term is the
    /// SAME for both kinds — a hot seam under an ocean bed is a ridge.
    #[test]
    fn the_plate_height_composes_kind_base_and_seam_heat() {
        let bench = test_bench();
        let (plates, seams) = (bench.plates(), bench.seams());
        let w = 2.5;
        // Find one continental and one oceanic tile.
        let cont = (0..bench.map().len() as TileId)
            .find(|t| plates.is_continent(*t))
            .expect("a continent exists");
        let bed = (0..bench.map().len() as TileId)
            .find(|t| !plates.is_continent(*t))
            .expect("an ocean bed exists");
        let base = |t: TileId| plate_height(plates, seams, t, w) - w * ELEV_H_FRAC * seams.heat(t);
        assert!(
            (base(cont) - w * CONTINENT_H_FRAC).abs() < 1e-4,
            "a continent's base is the thick shelf"
        );
        assert!(
            (base(bed) - w * OCEAN_BED_H_FRAC).abs() < 1e-4,
            "a bed's base is the thin veneer"
        );
        assert!(base(cont) > base(bed), "shelves outstand beds at equal heat");
        // The heat term: strictly monotone, kind-independent.
        let lift = |t: TileId| plate_height(plates, seams, t, w) - base(t);
        assert!(
            (lift(cont) - w * ELEV_H_FRAC * seams.heat(cont)).abs() < 1e-4
                && (lift(bed) - w * ELEV_H_FRAC * seams.heat(bed)).abs() < 1e-4,
            "the elevation channel is the seam heat alone, on either kind"
        );
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
        let before_vents = bench.crust().vents().to_vec();
        bench.apply_results(&write(9.0));
        assert_eq!(bench.seams().spots(), 9, "the committed number lands");
        assert_eq!(bench.seams().seed(), seed, "same roll, more plumes");
        assert_ne!(
            bench.crust().vents(),
            &before_vents[..],
            "the crust's vents follow the plumes"
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

        // PLATES tab (page 0, tab 3): its slice alone.
        let plates = arrange_at(0.0, 3.0);
        assert!(plates.is_on("shown_p0_t3"), "the plates tab lights its slice");
        assert!(
            !plates.is_on("shown_p0_t2") && !plates.is_on("shown_p0_t0"),
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
