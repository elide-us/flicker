//! **The WorldBench V5 screen** — the Populous frame around a V5 world.
//!
//! # The UI is DATA
//!
//! The whole surface is authored in `worldbench.scene.json`; `worldbench.lua`'s
//! `arrange()` lights the one slice that shows. This scene composes nothing:
//! it parses the tree once, publishes a Model each frame, walks the tree, and
//! answers the results.
//!
//! # The world IS a recipe
//!
//! Every dial on the bench is one number of a [`Recipe`] — the V5 field stack
//! as data — and the heights the globe draws come from the engine's one
//! [`compose`]. The headless evaluator (`resonance_eval`) composes the same
//! recipe the same way, so a world found by a sweep lands in the window
//! unchanged, and a world shaped in the window measures identically. The gate
//! `the_bench_composes_exactly_what_the_evaluator_composes` holds the two
//! together.
//!
//! **A field is a frequency across the whole planet and an alpha mask — that is
//! it** (Aaron 2026-10-15). The stack: the planetary wave (frequency 1 on the
//! pentagons) plus a list of resonance SLOTS, each a frequency the game master
//! edits, an ON switch and a MASK switch; relief, tilt, wave count and mask
//! seed are the stack's dials. Superposition is the only op.
//!
//! The globe draws the authored shells EXTRUDED by the composed height channel,
//! the tile shell tinted by the engine's relief [`Ramp`] against the sea (deep →
//! shallow below it, lowland → upland → summit above, the stops authored in the
//! scene's `relief` style block) times the engine's per-tile [`hill_shade`] —
//! the same shade the headless evaluator's globe is drawn with, so what the
//! agent judges off its renders is what Aaron sees on the ball — under a
//! translucent SEA sphere at the recipe's sea; the HEAT tab paints the molten
//! seam field instead, and the EPOCHS tab's VIEW pill shows the HARDNESS
//! ledger (soft → hard, the stops in the `hardness` style block).
//!
//! # The pipeline on the ball
//!
//! Composed → EPOCHS (the stone ocean moving) → EROSION (the era's water,
//! air and weathering, `Evolution` adopted over the epoched planet with its
//! hardness). What the globe draws is the LAST stage ticked: a change
//! upstream starts every stage below it over.
//!
//! # The hex at 2K
//!
//! The TILE tab MATERIALIZES the hex the camera faces through the pixel tier
//! (`flicker_worldtile::materialize` over the V5 source: the one recipe's
//! fields evaluated exactly at every pixel, the fine layers included) on its
//! own thread, and shows the tile's own hill-shaded picture in the centre
//! pane as a HUD sprite — the same picture `ground_eval` writes — or, the
//! VIEW pill at FIELD, the hex as the ENGINE'S FIELD OF VECTORS: one LOD8
//! vector per pixel of the mask (the pixel's composite, the heightmap as it
//! is) and the surface contoured between them as the cluster system
//! contours everything — a quad across every four vectors, smooth-shaded
//! by its own normals — wearing the picture, flown over with the orbit
//! camera through a second `GlobeWorld` seated in the same pane (Aaron
//! 2026-10-19: *"a single continuous field of quads with each quad
//! intersection defined by the pixel height"*; the first field's cubes
//! with drop walls were a voxel engine and are gone).

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use flicker::render::{FrameGraph, MeshDrawOptions, MeshVertex, Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_globe::{
    temp_color, BakedSet, GlobeWorld, HexSphereMap, MapContent, ShellSpec, WorldMap, DEFAULT_SET,
    RADIUS,
};
use flicker_input_core::{
    AbstractControls, ActionSignal, GamepadConfig, InputContext, InputMap, InputState,
};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_worldengine::{
    compose, hill_shade, land_share, BubbleRecipe, Composed, CrustField, EpochRecipe, Epochs,
    Evolution, HeatRecipe, HexMap, MaskKind, MaskRecipe, PlanetWave, Ramp, Recipe, SeamField,
    SlotRecipe, StackRecipe, VolcanoShape, WaveRecipe, DEFAULT_FREQ, LOOP_CYCLES, LOOP_EPOCHS,
    MAX_AXES, MAX_LOBE, MIN_AXES, MIN_LOBE,
};
use flicker_worldtile::{quad_field, ResonanceSource, TileSource, TILE_SPAN_M, V5_RELIEF_SCALE};

use crate::cache::TileCache;
use crate::ui;

const WORLDBENCH_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/worldbench.lua");

/// The baked view sets: the WAVE (relief + sea), the HEAT (the seam field
/// painted on the relief, no sea) and the HARDNESS ledger (soft → hard on the
/// relief, no sea — the floor's hardness shows).
const WAVE_SET: &str = "wave";
const HEAT_SET: &str = "heat";
const HARD_SET: &str = "hard";

// ── the dials' ranges (min, max); the openings are the recipe's own ──
const SEA_RANGE: (u32, u32) = (0, 100);
/// RELIEF, percent of radius (Aaron: useful caps at ~5).
const AMP_RANGE: (u32, u32) = (0, 8);
const LOBE_DIAL_SCALE: f32 = 10.0;
const MASK_SEED_RANGE: (u32, u32) = (0, 99);
/// The resonance stack: SPREAD in tenths of an octave, SEAM in rings (tiles)
/// from a converging plate edge.
const RES_SPREAD_RANGE: (u32, u32) = (0, 20);
const RES_SEAM_RANGE: (u32, u32) = (1, 10);
const HUNDREDTH: f32 = 0.01;
/// A slot's amplitude, hundredths of a percent of radius (the stepper speaks
/// percent to one decimal).
const SLOT_AMP_RANGE: (u32, u32) = (0, 600);
/// A slot's height dial counts hundredths of a percent of the radius — fine
/// enough for the layers under the hex (a half-mile hummock is 0.01%).
const HUNDREDTH_PERCENT: f32 = 0.0001;
const TENTH: f32 = 0.1;
/// The spare slots after the layers: OFF, unmasked, low, waiting for a
/// frequency.
const SPARE_FREQUENCIES: [f32; 5] = [5.0, 20.0, 60.0, 120.0, 240.0];
const SPARE_AMP: f32 = 0.005;
/// The epochs: RATE in hundredths of a tile per tick, UPLIFT the isostatic
/// share in tenths (2 = a fifth of the crust that goes under stands up).
/// A RUN ticks for this much of each frame, and re-bakes the globe this often.
const EPOCH_FRAME_BUDGET: Duration = Duration::from_millis(8);
const BAKE_EVERY: u64 = 25;
/// The era's RUN: a surface cycle is dearer than an epoch (the whole
/// atmosphere every tick), so the globe re-bakes every few cycles.
const ERA_BAKE_EVERY: u64 = 5;
/// The LOOP re-bakes every this many work units (epochs and cycles alike).
const LOOP_BAKE_EVERY: u64 = 5;

/// Where a loop step stands: idle between steps; the epochs it still owes
/// (the era written back and stood down, the crust moving); the cycles it
/// still owes (the era adopted anew, cutting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopPhase {
    Idle,
    Epochs(u32),
    Cycles(u32),
}
/// The tile picture is the 2K map reduced this many times — 512² for the pane.
const TILE_PREVIEW_SCALE: u32 = 4;

/// The loader thread's channels: cells to read (with their paths) out, and
/// pictures (cell, rgba, dim) back.
type LoaderRequests = mpsc::Sender<(u32, PathBuf)>;
type LoadedPictures = mpsc::Receiver<(u32, Vec<u8>, u32)>;

/// **One materialized hex**, as the thread hands it back: the picture and
/// its readings.
pub struct TileReport {
    pub cell: u32,
    pub rgba: Vec<u8>,
    pub dim: u32,
    /// The hex's column height above the datum, metres, and the pixels'
    /// span and roughness (mean |Δh| between neighbouring pixels).
    pub column_m: f64,
    pub lo_m: f32,
    pub hi_m: f32,
    pub rough_m: f64,
    /// The passes of rain run over it so far, and what they carried out, kg.
    pub cut: u32,
    pub exported_kg: f64,
    /// Whether this is the thread's last word (the cut has run its count).
    pub done: bool,
    /// The planet's radius the tile frame was built with, metres — what
    /// places the picture on a map (`TILE_SPAN_M / radius_m`).
    pub radius_m: f64,
    /// THE FIELD, with the thread's last word: the hex as the engine's quad
    /// field — its region meshes, ready to bake, and its counts.
    pub field: Option<FieldLanding>,
}

/// **The materialized hex as the engine's field of vectors**, as the tile
/// thread hands it back: the surface's region meshes (world voxels, smooth-
/// shaded by the surface's own normals, the tile's picture as their
/// colour), the vectors (the mask's count) and the quads spanned between
/// them, and the framing — the field's centre and half-diagonal in voxels —
/// the orbit camera opens on.
pub struct FieldLanding {
    pub meshes: BakedSet,
    pub vectors: usize,
    pub quads: usize,
    pub mask: usize,
    pub centre: [f32; 3],
    pub radius: f32,
}
const EPOCH_RATE_RANGE: (u32, u32) = (5, 100);
const EPOCH_UPLIFT_RANGE: (u32, u32) = (0, 30);
/// RIFT in tenths of the rate.
const EPOCH_RIFT_RANGE: (u32, u32) = (0, 20);
/// IMPACTS in strikes a thousand epochs.
const EPOCH_IMPACTS_RANGE: (u32, u32) = (0, 20);
/// The emitters: reach in tiles, arms, rings, height in half-percents.
const RADIUS_RANGE: (u32, u32) = (2, 16);
const ARMS_RANGE: (u32, u32) = (3, 8);
const RINGS_RANGE: (u32, u32) = (1, 5);
const HEIGHT_RANGE: (u32, u32) = (0, 12);
const HALF_PERCENT: f32 = 0.005;

/// The scene's own ink, read ONCE out of its `relief` and `wave` style blocks:
/// the relief RAMP the tile shell is tinted by against the sea, and the sea
/// sphere's colour, blend and sheen.
struct WaveInk {
    ramp: Ramp,
    sea: [f32; 3],
    sea_alpha: f32,
    sea_gloss: f32,
    /// The hardness ledger's two stops.
    soft: [f32; 3],
    hard: [f32; 3],
}

/// **WorldBench V5** — the screen.
pub struct WorldBench {
    // ── navigation ──
    sel_page: usize,
    page_tabs: Vec<usize>,

    // ── the surface (the scene as DATA) ──
    tree: UiNode,
    script: ScriptHost,
    ui_styles: serde_json::Value,
    ui_state: UiState,
    ui_intents: UiIntents,
    hud_commands: Vec<HudCommand>,
    theme: Option<Theme>,
    textures: Vec<TextureHandle>,

    // ── the world ──
    map: HexMap,
    world: GlobeWorld,
    ink: WaveInk,

    // ── THE RECIPE, as dials ──
    wave_seed: u64,
    sea_percent: u32,
    amp_percent: u32,
    lobe_dial: u32,
    mask_seed: u32,
    field_wave: bool,
    field_mask: bool,
    /// The resonance stack — the recipe's own record, held as is.
    stack: StackRecipe,
    /// The heat and its emitters.
    heat_seed: u64,
    cells: u32,
    spots: u32,
    field_heat: bool,
    field_emit: bool,
    radius_tiles: u32,
    arms: u32,
    rings: u32,
    height: u32,
    /// BUBBLES ON BUBBLES: the field's roll and dials; its switch is the
    /// FIELDS tree's BUBBLES.
    bubbles: BubbleRecipe,

    /// The epochs: their dials, and their state once ticked (None = the
    /// composed world stands); the RUN box as typed; the epochs a RUN still
    /// owes, ticked across frames so the window stays live.
    epoch_recipe: EpochRecipe,
    field_epochs: bool,
    epochs: Option<Epochs>,
    epoch_count: String,
    epochs_pending: u32,
    baked_at: u64,
    /// What the globe shows on the epochs tab: relief or the hardness ledger.
    ep_view: u32,
    /// THE EROSION: the era adopted over the shown world once ticked (None
    /// = the epochs' or composed world stands); the RUN box; the cycles a
    /// RUN still owes; where the globe was last baked.
    field_erosion: bool,
    era: Option<Evolution>,
    era_count: String,
    era_pending: u32,
    era_baked_at: u64,
    /// The CUT box as typed — the passes of rain the next MATERIALIZE runs.
    cut_count: String,
    /// THE FOREVER LOOP: the box as typed; the steps a LOOP still owes;
    /// where the live step stands; steps done since the epochs stood up;
    /// the work units (epochs and cycles) done, and where the globe was
    /// last baked, so a step plays out on the ball.
    loop_count: String,
    loop_pending: u32,
    loop_phase: LoopPhase,
    loops_done: u64,
    loop_work: u64,
    loop_baked_at: u64,

    // ── derived from the recipe ──
    wave: PlanetWave,
    seams: SeamField,
    crust: CrustField,
    /// The composed world — the shape era's planet: heights, sea, hardness.
    composed: Composed,
    /// What the globe draws: the composed relief and hardness, the epochs'
    /// once ticked, the era's once it has run — and the sea that stands.
    heights: Vec<f32>,
    hardness: Vec<f32>,
    shown_sea: f32,

    // ── the hex at 2K ──
    /// The materialization in flight (its receiver), the last report, the
    /// HUD texture it is uploaded to and whether that upload is owed.
    tile_job: Option<mpsc::Receiver<TileReport>>,
    /// The hex the thread is materializing and the cut it runs — the MAP
    /// page's per-hex progress reads the last report against them.
    tile_cell: Option<u32>,
    tile_cut_total: u32,
    tile: Option<TileReport>,
    tile_texture: Option<TextureHandle>,
    tile_dirty: bool,
    /// THE FIELD: a second world seated in the centre pane (its own stage,
    /// the orbit camera over the hex's quads), the VIEW pill's choice, and
    /// whether a field has been baked into it.
    field: GlobeWorld,
    tile_view: u32,
    field_ready: bool,
    field_quads: (usize, usize),
    /// THE MAP page: the world laid flat (the shared WorldMap over the same
    /// tiling the globe draws), where every hex the thread materializes is
    /// drawn IN THE WORLD as its picture; the pictures that landed and await
    /// the renderer (cell, rgba, dim, span over radius); the resident
    /// pictures in landing order (a pool of textures reused oldest-first);
    /// the last pick; the hexes a pick or RENDER WORLD still owes the
    /// thread; the job's count and whether it runs.
    worldmap: WorldMap<HexSphereMap>,
    map_pending: Vec<(u32, Vec<u8>, u32, f32)>,
    map_resident: VecDeque<(u32, TextureHandle)>,
    map_pick: Option<u32>,
    /// The hex RENDER HEX owes the thread: queued at the front, started even
    /// while the world walk rests.
    hex_owed: Option<u32>,
    map_queue: VecDeque<u32>,
    render_total: usize,
    render_done: usize,
    rendering: bool,
    /// THE TILE CACHE of the shown world (opened when the map needs it; a
    /// changed world is another cache and the sheet starts bare); the loader
    /// thread that reads cached pictures as the view reaches them (requests
    /// out, pictures back); the cells it is reading; textures freed by a
    /// world change, reused before any new one is made; the hex the view is
    /// centred on; when the render job started and how many it had done.
    cache: Option<TileCache>,
    loader: Option<(LoaderRequests, LoadedPictures)>,
    loading: HashSet<u32>,
    map_free: Vec<TextureHandle>,
    map_centre: Option<u32>,
    job_started: Option<Instant>,
    job_done_at_start: usize,
}

impl WorldBench {
    pub fn new(def: &SceneDef) -> Self {
        let map = HexMap::new(DEFAULT_FREQ);
        tracing::info!("worldbench: {} tiles at freq {}", map.len(), map.freq());
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let world =
            GlobeWorld::new(ui::STAGE_SOURCE, &ui_styles, Some(0.85)).in_panel(ui::VIEW_PANE);
        // THE FIELD's world: the same seat, stage and camera machinery over
        // the hex's quads instead of the planet's shells; framed when a
        // field lands.
        let field =
            GlobeWorld::new(ui::FIELD_STAGE_SOURCE, &ui_styles, None).in_panel(ui::VIEW_PANE);
        // The MAP page's sheet: the SAME tiling, plugged into the shared
        // WorldMap; its look comes off its own stage as the globe's does; it
        // zooms to a hex so a picture fills the view.
        let mut worldmap = WorldMap::new(
            ui::MAP_STAGE_SOURCE,
            &ui_styles,
            HexSphereMap::from_tiling(&map.grid().dirs, map.outlines()),
        );
        let map_look = worldmap.authored_look();
        worldmap.content_mut().set_look(map_look);
        worldmap
            .content_mut()
            .set_zoom_floor(ui::MAP_ZOOM_FLOOR_CELLS);
        let tree = def
            .tree
            .clone()
            .expect("worldbench.scene.json declares a tree");
        let ui_intents = UiIntents::of(&tree);
        let script = ScriptHost::new(WORLDBENCH_SCRIPT, "worldbench.lua")
            .expect("worldbench.lua loads (it ships with the crate)");
        let ink = WaveInk {
            ramp: Ramp {
                deep: rgb(&ui_styles, "relief.deep"),
                shallow: rgb(&ui_styles, "relief.shallow"),
                lowland: rgb(&ui_styles, "relief.lowland"),
                upland: rgb(&ui_styles, "relief.upland"),
                summit: rgb(&ui_styles, "relief.summit"),
            },
            sea: rgb(&ui_styles, "wave.sea"),
            sea_alpha: style_number(&ui_styles, "wave.sea_alpha", 0.55),
            sea_gloss: style_number(&ui_styles, "wave.sea_gloss", 0.5),
            soft: rgb(&ui_styles, "hardness.soft"),
            hard: rgb(&ui_styles, "hardness.hard"),
        };
        // THE OPENING DIALS ARE THE RECIPE'S OWN, AT THE SEED THE SWEEP FOUND —
        // the evaluator opens on the same preset at the same seed, so the
        // window opens on the world the agent judged (the mandate: the window
        // is the judge); RANDOMIZE rolls another.
        let opening = Recipe::default_at(map.freq(), Recipe::OPENING_SEED);
        let wave = PlanetWave::with_lobe(&map, opening.wave.seed, opening.wave.lobe);
        let seams = SeamField::new(
            &map,
            opening.heat.cells,
            opening.heat.spots,
            opening.heat.seed,
        );
        let crust = CrustField::derive(&map, &seams);
        // The slots: the layers, then the spare slots off.
        let mut stack = opening.stack.clone();
        for f in SPARE_FREQUENCIES {
            if stack.slots.len() >= ui::RESONANCE_SLOTS {
                break;
            }
            stack.slots.push(SlotRecipe {
                frequency: f,
                amp: SPARE_AMP,
                axes: 1,
                on: false,
                mask: MaskKind::Off,
            });
        }
        stack.slots.truncate(ui::RESONANCE_SLOTS);
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
            ink,
            wave_seed: opening.wave.seed,
            sea_percent: opening.sea_percent.round() as u32,
            amp_percent: (opening.wave.amp * 100.0).round() as u32,
            lobe_dial: (opening.wave.lobe * LOBE_DIAL_SCALE).round() as u32,
            mask_seed: opening.wave.mask.seed as u32,
            field_wave: opening.wave.on,
            field_mask: opening.wave.mask.on,
            stack,
            heat_seed: opening.heat.seed,
            cells: opening.heat.cells,
            spots: opening.heat.spots,
            field_heat: opening.heat.on,
            field_emit: opening.heat.emitters_on,
            radius_tiles: opening.heat.emitters.radius_tiles.round() as u32,
            arms: opening.heat.emitters.arms,
            rings: opening.heat.emitters.rings.round() as u32,
            height: (opening.heat.emitters.amp / HALF_PERCENT).round() as u32,
            bubbles: opening.bubbles,
            epoch_recipe: EpochRecipe::default(),
            field_epochs: true,
            epochs: None,
            epoch_count: ui::EPOCH_RUN_DEFAULT.to_string(),
            epochs_pending: 0,
            baked_at: 0,
            ep_view: ui::EPOCH_VIEW_RELIEF,
            field_erosion: true,
            era: None,
            era_count: ui::ERA_RUN_DEFAULT.to_string(),
            era_pending: 0,
            era_baked_at: 0,
            cut_count: ui::CUT_DEFAULT.to_string(),
            loop_count: ui::LOOP_RUN_DEFAULT.to_string(),
            loop_pending: 0,
            loop_phase: LoopPhase::Idle,
            loops_done: 0,
            loop_work: 0,
            loop_baked_at: 0,
            wave,
            seams,
            crust,
            composed: Composed {
                heights: Vec::new(),
                sea: 0.0,
                hardness: Vec::new(),
            },
            heights: Vec::new(),
            hardness: Vec::new(),
            shown_sea: 0.0,
            tile_job: None,
            tile_cell: None,
            tile_cut_total: 0,
            tile: None,
            tile_texture: None,
            tile_dirty: false,
            field,
            tile_view: ui::TILE_VIEW_PICTURE,
            field_ready: false,
            field_quads: (0, 0),
            worldmap,
            map_pending: Vec::new(),
            map_resident: VecDeque::new(),
            map_pick: None,
            hex_owed: None,
            map_queue: VecDeque::new(),
            render_total: 0,
            render_done: 0,
            rendering: false,
            cache: None,
            loader: None,
            loading: HashSet::new(),
            map_free: Vec::new(),
            map_centre: None,
            job_started: None,
            job_done_at_start: 0,
        };
        bench.derive();
        bench.bake();
        bench.world.show(bench.view_key());
        bench
    }

    /// **The recipe the dials spell** — the one representation the evaluator
    /// and the window share.
    pub fn recipe(&self) -> Recipe {
        Recipe {
            freq: self.map.freq(),
            wave: WaveRecipe {
                seed: self.wave_seed,
                amp: self.amp_percent as f32 / 100.0,
                lobe: self.lobe_dial as f32 / LOBE_DIAL_SCALE,
                mask: MaskRecipe {
                    seed: u64::from(self.mask_seed),
                    on: self.field_mask,
                },
                on: self.field_wave,
            },
            stack: self.stack.clone(),
            heat: HeatRecipe {
                seed: self.heat_seed,
                cells: self.cells,
                spots: self.spots,
                on: self.field_heat,
                emitters: VolcanoShape {
                    radius_tiles: self.radius_tiles as f32,
                    arms: self.arms,
                    rings: self.rings as f32,
                    amp: self.height as f32 * HALF_PERCENT,
                },
                emitters_on: self.field_emit,
            },
            sea_percent: self.sea_percent as f32,
            bubbles: self.bubbles,
        }
    }

    /// Compose the recipe into the heights and the sea — the engine's one way.
    /// A recomposed world is a new initial state: the epochs start over.
    fn derive(&mut self) {
        self.loops_done = 0;
        self.composed = compose(&self.map, &self.recipe());
        self.epochs = None;
        self.epochs_pending = 0;
        self.reset_era();
        self.refresh();
    }

    /// The era starts over: whatever it adopted is gone with the world it
    /// adopted — and so is a loop in flight.
    fn reset_era(&mut self) {
        self.era = None;
        self.era_pending = 0;
        self.stop_loop();
    }

    /// The loop stands down where it is: a live step keeps what it has done
    /// (the crust moved, or the era cutting), nothing more is owed.
    fn stop_loop(&mut self) {
        self.loop_pending = 0;
        self.loop_phase = LoopPhase::Idle;
    }

    /// What the globe draws: the last stage ticked — the era's relief, sea
    /// and surface hardness while the EROSION field is on and the era has
    /// run; else the epochs' while the EPOCHS field is on and ticked; else
    /// the composed world's.
    fn refresh(&mut self) {
        (self.heights, self.hardness, self.shown_sea) = match (
            &self.era,
            self.field_erosion,
            &self.epochs,
            self.field_epochs,
        ) {
            (Some(era), true, _, _) if era.ticks() > 0 => {
                (era.relief(), era.surface_hardnesses(), era.relief_sea())
            }
            (_, _, Some(e), true) => (
                e.heights().to_vec(),
                e.hardness().to_vec(),
                self.composed.sea,
            ),
            _ => (
                self.composed.heights.clone(),
                self.composed.hardness.clone(),
                self.composed.sea,
            ),
        };
    }

    /// The world the era ADOPTS: the stage before it — the epochs' relief
    /// and hardness once ticked (and on), else the composed world.
    fn world_before_erosion(&self) -> Composed {
        match (&self.epochs, self.field_epochs) {
            (Some(e), true) => Composed {
                heights: e.heights().to_vec(),
                sea: self.composed.sea,
                hardness: e.hardness().to_vec(),
            },
            _ => Composed {
                heights: self.composed.heights.clone(),
                sea: self.composed.sea,
                hardness: self.composed.hardness.clone(),
            },
        }
    }

    /// Run `count` surface cycles of the era over the world before it
    /// (adopting it on first use).
    fn tick_era(&mut self, count: u32) {
        if self.era.is_none() {
            let world = self.world_before_erosion();
            let mut era = Evolution::new(&self.map, &self.seams);
            era.adopt(&self.map, &self.seams, &world);
            self.era = Some(era);
        }
        let Self {
            map,
            seams,
            crust,
            era,
            ..
        } = self;
        if let Some(era) = era.as_mut() {
            for _ in 0..count {
                let sea = era.resolve_sea();
                era.tick(map, seams, crust, sea);
            }
        }
        self.refresh();
    }

    /// **Pay down the era's RUN** — like [`Self::pump_epochs`], a cycle a
    /// frame at least, the globe re-baked every [`ERA_BAKE_EVERY`] cycles.
    pub fn pump_era(&mut self, budget: Duration) {
        if self.era_pending == 0 {
            return;
        }
        let start = std::time::Instant::now();
        let mut ticked = false;
        while self.era_pending > 0 && (!ticked || start.elapsed() < budget) {
            self.tick_era(1);
            self.era_pending -= 1;
            ticked = true;
        }
        let ticks = self.era_ticks();
        if self.era_pending == 0 || ticks.saturating_sub(self.era_baked_at) >= ERA_BAKE_EVERY {
            self.bake();
            self.era_baked_at = ticks;
        }
    }

    /// **A loop step begins: the era is WRITTEN BACK into the crust.** The
    /// epochs stand up over the composed world if they have not; the era's
    /// relief and surface hardness go into the parcels ([`Epochs::absorb`]
    /// — the eroded rock is gone from the parcel that carried it, the
    /// sediment is part of it); the era stands down while the crust moves.
    fn begin_loop_step(&mut self) {
        if self.epochs.is_none() {
            self.epochs = Some(Epochs::new(
                &self.map,
                &self.recipe(),
                &self.composed,
                &self.epoch_recipe,
            ));
        }
        if let (Some(e), Some(era)) = (self.epochs.as_mut(), self.era.as_ref()) {
            if era.ticks() > 0 {
                e.absorb(&era.relief(), &era.surface_hardnesses());
            }
        }
        self.era = None;
        self.era_pending = 0;
        self.epochs_pending = 0;
        self.loop_phase = LoopPhase::Epochs(LOOP_EPOCHS);
    }

    /// **Pay down the LOOP** — write back, move, adopt, cut, a work unit at
    /// a time while `budget` lasts (one at least a frame): each step the
    /// crust moves [`LOOP_EPOCHS`] epochs (compression, drops, strikes
    /// over the eroded world), then the era adopts the moved world anew and
    /// cuts [`LOOP_CYCLES`] cycles. The globe re-bakes every
    /// [`LOOP_BAKE_EVERY`] units and when the loop completes.
    pub fn pump_loop(&mut self, budget: Duration) {
        if self.loop_pending == 0 && self.loop_phase == LoopPhase::Idle {
            return;
        }
        let start = std::time::Instant::now();
        let mut worked = false;
        loop {
            match self.loop_phase {
                LoopPhase::Idle => {
                    if self.loop_pending == 0 {
                        break;
                    }
                    self.begin_loop_step();
                }
                LoopPhase::Epochs(0) => {
                    // Adopt the moved world anew.
                    let world = match &self.epochs {
                        Some(e) => Composed {
                            heights: e.heights().to_vec(),
                            sea: self.composed.sea,
                            hardness: e.hardness().to_vec(),
                        },
                        None => self.world_before_erosion(),
                    };
                    let mut era = Evolution::new(&self.map, &self.seams);
                    era.adopt(&self.map, &self.seams, &world);
                    self.era = Some(era);
                    self.loop_phase = LoopPhase::Cycles(LOOP_CYCLES);
                }
                LoopPhase::Epochs(n) => {
                    if worked && start.elapsed() >= budget {
                        break;
                    }
                    let (map, r) = (&self.map, self.epoch_recipe);
                    if let Some(e) = self.epochs.as_mut() {
                        e.run(map, &r, 1);
                    }
                    self.loop_phase = LoopPhase::Epochs(n - 1);
                    self.loop_work += 1;
                    worked = true;
                }
                LoopPhase::Cycles(0) => {
                    self.loops_done += 1;
                    self.loop_pending = self.loop_pending.saturating_sub(1);
                    self.loop_phase = LoopPhase::Idle;
                }
                LoopPhase::Cycles(n) => {
                    if worked && start.elapsed() >= budget {
                        break;
                    }
                    let Self {
                        map,
                        seams,
                        crust,
                        era,
                        ..
                    } = self;
                    if let Some(era) = era.as_mut() {
                        let sea = era.resolve_sea();
                        era.tick(map, seams, crust, sea);
                    }
                    self.loop_phase = LoopPhase::Cycles(n - 1);
                    self.loop_work += 1;
                    worked = true;
                }
            }
        }
        self.refresh();
        let done = self.loop_pending == 0 && self.loop_phase == LoopPhase::Idle;
        if done || self.loop_work.saturating_sub(self.loop_baked_at) >= LOOP_BAKE_EVERY {
            self.bake();
            self.loop_baked_at = self.loop_work;
        }
    }

    /// Loop steps done since the epochs stood up — for tests.
    pub fn loops_done(&self) -> u64 {
        self.loops_done
    }

    /// The loop steps a LOOP still owes — for tests.
    pub fn loop_pending(&self) -> u32 {
        self.loop_pending
    }

    /// Where the live loop step stands — for tests.
    pub fn loop_phase(&self) -> LoopPhase {
        self.loop_phase
    }

    /// The era's cycles run — for tests.
    pub fn era_ticks(&self) -> u64 {
        self.era.as_ref().map_or(0, |e| e.ticks())
    }

    /// The cycles a RUN still owes — for tests.
    pub fn era_pending(&self) -> u32 {
        self.era_pending
    }

    /// The hex the camera faces — the TILE tab's hex.
    pub fn facing_hex(&self) -> u32 {
        self.world.facing(&self.map.grid().dirs).unwrap_or(0) as u32
    }

    /// **MATERIALIZE the facing hex** on its own thread: the world as it
    /// stands (the last stage ticked, with the era's sediment where it ran)
    /// goes to the thread with the recipe; the thread stands the map and the
    /// V5 source up, walks the hex through the migration, shades its picture
    /// and reports back. One job at a time; the report lands in `poll_tile`.
    fn start_tile(&mut self) {
        let cell = self.facing_hex();
        self.start_tile_for(cell);
    }

    /// MATERIALIZE `cell` (the facing hex, a map pick, the render job's next)
    /// on the tile thread; one job at a time.
    fn start_tile_for(&mut self, cell: u32) {
        if self.tile_job.is_some() {
            return;
        }
        let recipe = self.recipe();
        let world = Composed {
            heights: self.heights.clone(),
            sea: self.shown_sea,
            hardness: self.hardness.clone(),
        };
        let sediment: Option<Vec<f32>> = match (&self.era, self.field_erosion) {
            (Some(era), true) if era.ticks() > 0 => era.adopted().map(|frame| {
                (0..self.map.len() as u32)
                    .map(|t| era.sediment(t) * frame.scale)
                    .collect()
            }),
            _ => None,
        };
        let cut = self.cut_count.parse::<u32>().unwrap_or(ui::CUT_DEFAULT);
        self.tile_cell = Some(cell);
        self.tile_cut_total = cut;
        // The finished picture goes to the shown world's cache.
        self.open_cache();
        let cache_path = self.cache.as_ref().map(|c| c.path(cell));
        let (tx, rx) = mpsc::channel();
        self.tile_job = Some(rx);
        std::thread::spawn(move || {
            let map = HexMap::new(recipe.freq);
            let src =
                ResonanceSource::new(&map, &recipe, &world, sediment.as_deref(), V5_RELIEF_SCALE);
            let mut tile = src.materialize(cell as usize);
            // A picture of the ground as the fields made it, then THE WATER
            // over the hex — the pixel eroder by the hex's hardness — a
            // picture every few passes, so the cut is watched, not waited for.
            let report =
                |tile: &flicker_worldtile::Tile, cut: u32, exported_kg: f64, done: bool| {
                    let (mut lo, mut hi, mut rough, mut pairs) =
                        (f32::MAX, f32::MIN, 0.0f64, 0usize);
                    // THE FIELD rides the thread's last word: the hex as the
                    // engine's quad field, wearing its full 2K picture.
                    let field = done.then(|| {
                        let (picture, _) = tile.shaded_rgba(1, src.sea_level_m(), src.top_m());
                        field_landing(&quad_field(tile, &picture), tile.mask.count())
                    });
                    for (x, y) in tile.mask.iter() {
                        let h = tile.composite_m(x, y);
                        lo = lo.min(h);
                        hi = hi.max(h);
                        if x + 1 < flicker_worldtile::TILE_DIM && tile.mask.contains(x + 1, y) {
                            rough += f64::from((tile.composite_m(x + 1, y) - h).abs());
                            pairs += 1;
                        }
                    }
                    let (rgba, dim) =
                        tile.shaded_rgba(TILE_PREVIEW_SCALE, src.sea_level_m(), src.top_m());
                    TileReport {
                        cell,
                        rgba,
                        dim,
                        column_m: src.thickness_m(cell as usize),
                        lo_m: lo,
                        hi_m: hi,
                        rough_m: rough / pairs.max(1) as f64,
                        cut,
                        exported_kg,
                        done,
                        radius_m: src.radius_m(),
                        field,
                    }
                };
            // A receiver dropped (the bench closed) is not an error.
            let keep = |rep: &TileReport| {
                if let (true, Some(path)) = (rep.done, &cache_path) {
                    if let Err(e) = TileCache::write_picture(path, &rep.rgba, rep.dim) {
                        tracing::warn!(
                            "worldbench: tile {cell} not cached at {}: {e}",
                            path.display()
                        );
                    }
                }
            };
            let first = report(&tile, 0, 0.0, cut == 0);
            keep(&first);
            if tx.send(first).is_err() {
                return;
            }
            let mut exported = 0.0f64;
            let mut ran = 0u32;
            while ran < cut {
                let block = ui::CUT_PICTURE_EVERY.min(cut - ran);
                exported += src.cut(&mut tile, block).exported_kg;
                ran += block;
                let rep = report(&tile, ran, exported, ran == cut);
                keep(&rep);
                if tx.send(rep).is_err() {
                    return;
                }
            }
        });
    }

    /// Take a finished materialization, if one has landed.
    pub fn poll_tile(&mut self) -> bool {
        let Some(rx) = &self.tile_job else {
            return false;
        };
        let mut landed = false;
        let mut field = None;
        loop {
            match rx.try_recv() {
                Ok(mut report) => {
                    let done = report.done;
                    if let Some(landing) = report.field.take() {
                        field = Some(landing);
                    }
                    if done {
                        // Every hex the thread finishes is drawn in the world.
                        self.map_pending.push((
                            report.cell,
                            report.rgba.clone(),
                            report.dim,
                            (TILE_SPAN_M / report.radius_m) as f32,
                        ));
                        if let Some(cache) = self.cache.as_mut() {
                            cache.landed(report.cell);
                        }
                        if self.rendering {
                            self.render_done += 1;
                        }
                    }
                    self.tile = Some(report);
                    self.tile_dirty = true;
                    landed = true;
                    if done {
                        self.tile_job = None;
                        self.tile_cell = None;
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.tile_job = None;
                    self.tile_cell = None;
                    break;
                }
            }
        }
        if let Some(landing) = field {
            self.land_field(landing);
        }
        landed
    }

    /// Whether a materialization is in flight — for tests.
    pub fn tile_in_flight(&self) -> bool {
        self.tile_job.is_some()
    }

    /// **A field lands**: its region meshes go to the field world (uploaded
    /// at the next render, the set before them freed), and the orbit camera
    /// is framed on it — the whole hex in view, zoomable down to two quads'
    /// width, a right-drag sliding across the ground.
    fn land_field(&mut self, landing: FieldLanding) {
        let FieldLanding {
            meshes,
            vectors,
            quads,
            centre,
            radius,
            ..
        } = landing;
        tracing::info!("worldbench: the field: {vectors} vectors, {quads} quads");
        self.field.bake_meshes(DEFAULT_SET, meshes);
        self.field.show(DEFAULT_SET);
        self.field.aim(flicker::render::Vec3::from(centre));
        self.field.set_frame(radius, Some(0.85));
        self.field
            .set_range(2.0 * flicker_voxel::CLUSTER_DIM as f32, radius * 3.0);
        self.field.set_panning(true);
        self.field_ready = true;
        self.field_quads = (vectors, quads);
    }

    /// Whether a field has been baked — for tests.
    pub fn field_ready(&self) -> bool {
        self.field_ready
    }

    /// Whether the centre pane shows the FIELD this frame: the TILE tab lit,
    /// the VIEW pill at FIELD, a field baked.
    fn field_shown(&self) -> bool {
        self.sel_page == ui::PAGE_WORLD
            && self.sel_tab() == ui::TAB_TILE
            && self.tile_view == ui::TILE_VIEW_FIELD
            && self.field_ready
    }

    /// **A pick on the map**: the hex is selected and outlined on the sheet.
    /// Nothing renders until RENDER HEX asks — a click picks (the UX law
    /// D5054D29: a control's effect is what it says).
    pub fn pick_map(&mut self, cell: u32) {
        if cell as usize >= self.map.len() {
            tracing::warn!(
                "worldbench: map pick {cell} is outside the tiling ({} cells) — ignored",
                self.map.len()
            );
            return;
        }
        self.map_pick = Some(cell);
        self.worldmap.content_mut().set_picked(Some(cell));
    }

    /// The hex the sheet outlines — for tests.
    pub fn map_picked(&self) -> Option<u32> {
        self.worldmap.content().picked()
    }

    /// **RENDER HEX** — the picked hex goes to the thread next, ahead of the
    /// world walk's queue, and is drawn in the world when it lands.
    fn start_hex(&mut self) {
        if self.map_pick.is_none() {
            return;
        }
        self.open_cache();
        let Some(cell) = self.map_pick else { return };
        self.map_queue.retain(|c| *c != cell);
        self.map_queue.push_front(cell);
        self.hex_owed = Some(cell);
    }

    /// **STOP** — the materializer halts: the hex in flight is dropped (its
    /// thread's next report finds nobody listening and it exits) and goes
    /// back to the front of the queue; the world walk pauses, the queue
    /// keeps, RENDER WORLD resumes it.
    fn stop_materializer(&mut self) {
        self.tile_job = None;
        if let Some(cell) = self.tile_cell.take() {
            self.map_queue.retain(|c| *c != cell);
            self.map_queue.push_front(cell);
        }
        self.hex_owed = None;
        self.rendering = false;
    }

    /// **RENDER WORLD** — queue every hex not yet drawn, nearest the picked
    /// (else the facing) hex first, and run the thread through them; STOP
    /// pauses the walk (the queue keeps) and RENDER WORLD resumes it.
    fn start_world(&mut self) {
        if self.rendering {
            return;
        }
        self.open_cache();
        if self.map_queue.len() <= 1 {
            let from = self
                .map_pick
                .or(self.map_centre)
                .unwrap_or_else(|| self.facing_hex());
            let origin = self.map.direction(from);
            let cached: HashSet<u32> = self
                .cache
                .as_ref()
                .map(|c| c.cells().clone())
                .unwrap_or_default();
            let mut cells: Vec<u32> = self
                .map
                .tiles()
                .filter(|t| !cached.contains(t) && !self.map_queue.contains(t))
                .collect();
            cells.sort_by(|a, b| {
                self.map
                    .direction(*b)
                    .dot(origin)
                    .total_cmp(&self.map.direction(*a).dot(origin))
            });
            self.map_queue.extend(cells);
            self.render_total = self.map_queue.len();
            self.render_done = 0;
        }
        self.job_started = Some(Instant::now());
        self.job_done_at_start = self.render_done;
        self.rendering = true;
    }

    /// Hand the thread the next hex the map owes it: a pick at once, the
    /// render job's next while the job runs.
    fn pump_map(&mut self) {
        self.stream_cache();
        if self.tile_job.is_some() {
            return;
        }
        let next = match self.map_queue.front().copied() {
            Some(c) if self.rendering || Some(c) == self.hex_owed => Some(c),
            _ => None,
        };
        if let Some(cell) = next {
            self.map_queue.pop_front();
            if Some(cell) == self.hex_owed {
                self.hex_owed = None;
            }
            self.start_tile_for(cell);
        } else if self.rendering && self.map_queue.is_empty() {
            self.rendering = false;
        }
    }

    /// The map's readings — for tests: pictures landed and awaiting the
    /// renderer, hexes queued, whether RENDER WORLD runs.
    pub fn map_state(&self) -> (usize, usize, bool) {
        (self.map_pending.len(), self.map_queue.len(), self.rendering)
    }

    /// Hand the thread the map's next hex — for tests (the frame does it).
    pub fn pump_map_now(&mut self) {
        self.pump_map();
    }

    /// How many hexes the world has — for tests.
    pub fn hex_count(&self) -> usize {
        self.map.len()
    }

    /// How many of the world's hexes the cache holds — for tests.
    pub fn cached(&self) -> usize {
        self.cache.as_ref().map_or(0, |c| c.cells().len())
    }

    /// The stage the shown world stands at — with the recipe, what names its
    /// cache: a tick of the epochs, a cycle of the era, a loop step, another
    /// cut, a field switched, and the pictures are another world's.
    fn world_stage(&self) -> String {
        format!(
            "epochs{}:era{}:loops{}:cut{}:fields{}{}",
            self.epoch_ticks(),
            self.era_ticks(),
            self.loops_done,
            self.cut_count.parse::<u32>().unwrap_or(ui::CUT_DEFAULT),
            u8::from(self.field_epochs),
            u8::from(self.field_erosion)
        )
    }

    /// Open the shown world's tile cache (the loader thread with it, once).
    /// Another world: the sheet's pictures come off (their textures kept
    /// for reuse), the loads in flight are forgotten, the job stops.
    fn open_cache(&mut self) {
        let recipe_json = serde_json::to_string(&self.recipe()).unwrap_or_default();
        let stage = self.world_stage();
        let key = TileCache::key(&recipe_json, &stage);
        if self.cache.as_ref().is_some_and(|c| c.key_str() == key) {
            return;
        }
        if self.cache.is_some() {
            self.worldmap.content_mut().clear_pictures();
            for (_, handle) in self.map_resident.drain(..) {
                self.map_free.push(handle);
            }
            self.loading.clear();
            self.map_queue.clear();
            self.hex_owed = None;
            self.rendering = false;
            self.render_total = 0;
            self.render_done = 0;
            self.job_started = None;
        }
        self.cache = Some(TileCache::open(&key, &recipe_json, &stage));
        if self.loader.is_none() {
            let (req_tx, req_rx) = mpsc::channel::<(u32, PathBuf)>();
            let (res_tx, res_rx) = mpsc::channel::<(u32, Vec<u8>, u32)>();
            std::thread::spawn(move || {
                for (cell, path) in req_rx {
                    if let Some((rgba, dim)) = TileCache::read_picture(&path) {
                        if res_tx.send((cell, rgba, dim)).is_err() {
                            break;
                        }
                    }
                }
            });
            self.loader = Some((req_tx, res_rx));
        }
    }

    /// The tile's span over the planet's radius at this map — what places a
    /// cached picture on its hex (the thread reports the same for its own).
    fn span_over_radius(&self) -> f32 {
        (TILE_SPAN_M / flicker_worldtile::radius_for_freq(self.map.freq())) as f32
    }

    /// **The sheet reaches into the cache**: pictures the loader read land
    /// for the sheet; on the MAP page the cached hexes within the view, not
    /// yet on the sheet and not being read, are asked for nearest the view's
    /// centre first, a few a frame.
    fn stream_cache(&mut self) {
        let sor = self.span_over_radius();
        if let Some((_, rx)) = &self.loader {
            let mut landed = Vec::new();
            while let Ok((cell, rgba, dim)) = rx.try_recv() {
                landed.push((cell, rgba, dim));
            }
            for (cell, rgba, dim) in landed {
                self.loading.remove(&cell);
                self.map_pending.push((cell, rgba, dim, sor));
            }
        }
        if self.sel_page != ui::PAGE_MAP {
            return;
        }
        self.open_cache();
        let Some(frame) = self.worldmap.frame() else {
            return;
        };
        let Some(centre) = self.worldmap.content().pick(&frame, frame.center) else {
            return;
        };
        let centre = centre as u32;
        self.map_centre = Some(centre);
        let (Some(cache), Some((tx, _))) = (&self.cache, &self.loader) else {
            return;
        };
        let c = self.map.direction(centre);
        // The view's reach in arc: its half-diagonal on the sheet (plane
        // units per radian = the map's radius) and a hex to spare.
        let reach = (frame.span.length() * 0.5 / RADIUS + 2.0 * self.map.tile_angle())
            .min(std::f32::consts::PI);
        let min_dot = reach.cos();
        let resident: HashSet<u32> = self.map_resident.iter().map(|(t, _)| *t).collect();
        let pending: HashSet<u32> = self.map_pending.iter().map(|(t, _, _, _)| *t).collect();
        let mut near: Vec<(f32, u32)> = cache
            .cells()
            .iter()
            .filter(|t| !resident.contains(t) && !pending.contains(t) && !self.loading.contains(t))
            .map(|&t| (self.map.direction(t).dot(c), t))
            .filter(|(d, _)| *d >= min_dot)
            .collect();
        near.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, t) in near.into_iter().take(ui::MAP_LOADS_PER_FRAME) {
            if self.loading.len() >= ui::MAP_LOADS_PER_FRAME * 4 {
                break;
            }
            if tx.send((t, cache.path(t))).is_ok() {
                self.loading.insert(t);
            }
        }
    }

    /// The hex in flight: which, and the passes run of its cut — and the
    /// same as the share its gauge fills (the materialize itself is the
    /// first report; a cut of 0 is done when that lands).
    fn hex_reading(&self) -> (String, f64) {
        let Some(cell) = self.tile_cell else {
            return ("\u{2014}".to_string(), 0.0);
        };
        let ran = self
            .tile
            .as_ref()
            .filter(|t| t.cell == cell)
            .map_or(0, |t| t.cut);
        let share = if self.tile_cut_total == 0 {
            0.0
        } else {
            f64::from(ran) / f64::from(self.tile_cut_total)
        };
        (
            format!("{cell} \u{00b7} {ran} / {}", self.tile_cut_total),
            share,
        )
    }

    /// The world walk's reading: done of owed, and the time the rest will
    /// take at the measured pace.
    fn job_reading(&self) -> String {
        if !self.rendering && self.render_total == 0 {
            return "\u{2014}".to_string();
        }
        let mut s = format!("{} / {}", self.render_done, self.render_total);
        if self.rendering {
            let done_now = self.render_done.saturating_sub(self.job_done_at_start);
            match self.job_started {
                Some(started) if done_now > 0 => {
                    let per = started.elapsed().as_secs_f64() / done_now as f64;
                    let left = (self.render_total.saturating_sub(self.render_done)) as f64 * per;
                    let secs = left.round() as u64;
                    let eta = if secs >= 3600 {
                        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
                    } else if secs >= 60 {
                        format!("{}m {:02}s", secs / 60, secs % 60)
                    } else {
                        format!("{secs}s")
                    };
                    s.push_str(&format!(" \u{00b7} {eta}"));
                }
                _ => s.push_str(" \u{2026}"),
            }
        }
        s
    }

    /// The last materialized hex — for tests.
    pub fn tile(&self) -> Option<&TileReport> {
        self.tile.as_ref()
    }

    /// Run `count` epochs over the composed world (standing the epochs up on
    /// first use, off this recipe's seams). The era below starts over: its
    /// adopted world moved.
    fn tick_epochs(&mut self, count: u32) {
        self.reset_era();
        if self.epochs.is_none() {
            self.epochs = Some(Epochs::new(
                &self.map,
                &self.recipe(),
                &self.composed,
                &self.epoch_recipe,
            ));
        }
        let (map, r) = (&self.map, self.epoch_recipe);
        if let Some(e) = self.epochs.as_mut() {
            e.run(map, &r, count);
        }
        self.refresh();
    }

    /// **Pay down a RUN** — tick while `budget` lasts (at least one epoch a
    /// frame), re-baking the globe every [`BAKE_EVERY`] epochs and when the
    /// run completes, so a thousand epochs play out as a sequence on the ball
    /// instead of a stall.
    pub fn pump_epochs(&mut self, budget: Duration) {
        if self.epochs_pending == 0 {
            return;
        }
        let start = std::time::Instant::now();
        let mut ticked = false;
        while self.epochs_pending > 0 && (!ticked || start.elapsed() < budget) {
            self.tick_epochs(1);
            self.epochs_pending -= 1;
            ticked = true;
        }
        let ticks = self.epoch_ticks();
        if self.epochs_pending == 0 || ticks.saturating_sub(self.baked_at) >= BAKE_EVERY {
            self.bake();
            self.baked_at = ticks;
        }
    }

    /// The epochs a RUN still owes — for tests.
    pub fn epochs_pending(&self) -> u32 {
        self.epochs_pending
    }

    /// The epochs' readouts — for tests.
    pub fn epoch_ticks(&self) -> u64 {
        self.epochs.as_ref().map_or(0, |e| e.ticks())
    }

    /// The composed relief — for tests.
    pub fn composed(&self) -> &[f32] {
        &self.composed.heights
    }

    /// The hardness the globe's ledger view draws — for tests.
    pub fn hardness(&self) -> &[f32] {
        &self.hardness
    }

    fn sel_tab(&self) -> usize {
        let tabs = ui::page(self.sel_page).tabs.len();
        self.page_tabs[self.sel_page.min(self.page_tabs.len() - 1)].min(tabs.saturating_sub(1))
    }

    fn view_key(&self) -> &'static str {
        match self.sel_tab() {
            ui::TAB_HEAT => HEAT_SET,
            ui::TAB_EPOCHS if self.ep_view == ui::EPOCH_VIEW_HARDNESS => HARD_SET,
            _ => WAVE_SET,
        }
    }

    /// Whether a field's tab is ENABLED — the field's switch, a child's
    /// inheriting its parent's.
    pub fn tab_enabled(&self, tab: usize) -> bool {
        match tab {
            ui::TAB_WAVE => self.field_wave,
            ui::TAB_RES => self.stack.on,
            ui::TAB_HEAT => self.field_heat,
            ui::TAB_EMIT => self.field_heat && self.field_emit,
            ui::TAB_EPOCHS => self.field_epochs,
            ui::TAB_EROSION => self.field_erosion,
            _ => true,
        }
    }

    /// The fields' switches: (wave, heat, mask, emitters, resonance) — for tests.
    pub fn fields(&self) -> (bool, bool, bool, bool, bool) {
        (
            self.field_wave,
            self.field_heat,
            self.field_mask,
            self.field_emit,
            self.stack.on,
        )
    }

    /// A slot's (frequency, on, mask) — for tests.
    pub fn slot(&self, i: usize) -> (u32, bool, MaskKind) {
        let s = self.stack.slots[i];
        (s.frequency as u32, s.on, s.mask)
    }

    pub fn wave(&self) -> &PlanetWave {
        &self.wave
    }

    pub fn seams(&self) -> &SeamField {
        &self.seams
    }

    pub fn map(&self) -> &HexMap {
        &self.map
    }

    /// The standing sea, radius units — the era's once it has run, else the
    /// composed sea.
    pub fn sea(&self) -> f32 {
        self.shown_sea
    }

    /// The area-weighted share of the surface above the sea.
    pub fn land(&self) -> f32 {
        land_share(&self.map, |t| self.heights[t as usize], self.shown_sea)
    }

    /// The composed heights — for tests.
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// The wave's dials (sea %, relief %, lobe ×10) — for tests.
    pub fn dials(&self) -> (u32, u32, u32) {
        (self.sea_percent, self.amp_percent, self.lobe_dial)
    }

    pub fn build_tree(&self) -> UiNode {
        self.tree.clone()
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.sel_page, self.sel_tab())
    }

    /// A field switched off while its tab was selected: move to the first
    /// enabled tab.
    fn clamp_tab_to_enabled(&mut self) {
        let tabs = ui::page(self.sel_page).tabs.len();
        if self.tab_enabled(self.sel_tab()) {
            return;
        }
        if let Some(t) = (0..tabs).find(|t| self.tab_enabled(*t)) {
            self.page_tabs[self.sel_page] = t;
        }
    }

    /// **Bake the three views** off the heights the globe draws: the relief
    /// (tint × hill-shade under the sea sphere), the heat, the hardness ledger.
    fn bake(&mut self) {
        let Self {
            map,
            world,
            worldmap,
            heights,
            hardness,
            ink,
            seams,
            ..
        } = self;
        let heights: &[f32] = heights;
        let hardness: &[f32] = hardness;
        let seams: &SeamField = seams;
        let sea = self.shown_sea;
        // The shape's own floor and summit scale the ramp — robustly, so a
        // collision's tallest stacks do not paint every hill as lowland — never
        // one layer's amplitude, which the stack out-weighs.
        let (lo, hi) = Ramp::span(heights);
        // THE HILL-SHADE — the engine's per-tile shade, the one the evaluator's
        // globe render uses, so the ball in the window and the agent's picture
        // are lit the same.
        let shade: Vec<f32> = hill_shade(map, heights, sea);
        let shade: &[f32] = &shade;
        let extruded = || {
            let mut shells = world.authored_shells(&map.grid().dirs, map.outlines());
            for shell in shells.iter_mut() {
                let base = shell.radius;
                shell.cell_radius = Some(Box::new(move |i| base * (1.0 + heights[i])));
            }
            shells
        };
        let shaded = |c: [f32; 3], i: usize| [c[0] * shade[i], c[1] * shade[i], c[2] * shade[i]];
        // The MAP's cells: the relief's own ramp and shade, the sea its ink —
        // the same colours the globe's wave view wears, laid flat.
        {
            let ramp = ink.ramp;
            let sea_rgb = ink.sea;
            let colors: Vec<[f32; 3]> = (0..map.len())
                .map(|i| {
                    if heights[i] <= sea {
                        sea_rgb
                    } else {
                        shaded(ramp.tint(heights[i], sea, lo, hi), i)
                    }
                })
                .collect();
            worldmap.content_mut().set_colors(colors);
        }
        let mut wave = extruded();
        if let Some(top) = wave.last_mut() {
            let ramp = ink.ramp;
            top.color = Box::new(move |i| Some(shaded(ramp.tint(heights[i], sea, lo, hi), i)));
        }
        let sea_rgb = ink.sea;
        wave.push(ShellSpec {
            dirs: &map.grid().dirs,
            outlines: map.outlines(),
            radius: RADIUS * (1.0 + sea),
            inset: 0.0,
            color: Box::new(move |_| Some(sea_rgb)),
            cell_radius: None,
            depth: None,
            opts: MeshDrawOptions {
                tint: [1.0, 1.0, 1.0, ink.sea_alpha],
                gloss: ink.sea_gloss,
                ..MeshDrawOptions::default()
            },
        });
        let mut heat = extruded();
        if let Some(top) = heat.last_mut() {
            top.color = Box::new(move |i| Some(temp_color(seams.heat(i as u32))));
        }
        let mut hard = extruded();
        if let Some(top) = hard.last_mut() {
            let (soft, hard_c) = (ink.soft, ink.hard);
            top.color = Box::new(move |i| {
                let t = hardness.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                Some(shaded(flicker_globe::lerp3(soft, hard_c, t), i))
            });
        }
        world.bake(WAVE_SET, wave);
        world.bake(HEAT_SET, heat);
        world.bake(HARD_SET, hard);
    }

    /// **What the components read.**
    fn model(&self) -> ValueMap {
        let mut m = ValueMap::default();
        m.set("ui_staged", self.ui_state.staged_any());
        flicker_shell::publish_signal_bindings(
            &mut m,
            &flicker_shell::current_world_map(),
            [ActionSignal::Confirm],
        );
        m.set(ui::PAGE_BIND, self.sel_page as f64);
        // The MAP's readouts.
        m.set(
            ui::MAP_PICK_BIND,
            match self.map_pick {
                Some(c) => format!("{c} {}", where_on_the_ball(&self.map, c)),
                None => "\u{2014}".to_string(),
            },
        );
        // The materializer: what it is doing, as a reading and a gauge, and
        // which of its controls stand.
        let running = self.tile_job.is_some() || self.rendering;
        m.set(ui::MAT_RUNNING_BIND, running);
        m.set(ui::MAT_IDLE_BIND, !running);
        m.set(ui::HEX_READY_BIND, !running && self.map_pick.is_some());
        let (hex_reading, hex_share) = self.hex_reading();
        m.set(ui::MAP_HEX_BIND, hex_reading);
        m.set(ui::MAP_HEX_PROGRESS_BIND, hex_share);
        m.set(
            ui::MAP_TILES_BIND,
            format!("{} / {}", self.cached(), self.map.len()),
        );
        m.set(ui::MAP_JOB_BIND, self.job_reading());
        m.set(ui::TAB_BIND, self.sel_tab() as f64);
        m.set(ui::TABS_SHOWN, !ui::page(self.sel_page).tabs.is_empty());
        // The fields and their tabs.
        m.set(ui::FIELD_WAVE_BIND, self.field_wave);
        m.set(ui::FIELD_MASK_BIND, self.field_mask);
        m.set(ui::FIELD_RES_BIND, self.stack.on);
        m.set(ui::FIELD_HEAT_BIND, self.field_heat);
        m.set(ui::FIELD_EMIT_BIND, self.field_emit);
        m.set(ui::FIELD_BUBBLES_BIND, self.bubbles.on);
        m.set(ui::TAB_WAVE_ENABLED, self.tab_enabled(ui::TAB_WAVE));
        m.set(ui::TAB_RES_ENABLED, self.tab_enabled(ui::TAB_RES));
        m.set(ui::TAB_HEAT_ENABLED, self.tab_enabled(ui::TAB_HEAT));
        m.set(ui::TAB_EMIT_ENABLED, self.tab_enabled(ui::TAB_EMIT));
        m.set(ui::FIELD_EPOCHS_BIND, self.field_epochs);
        m.set(ui::TAB_EPOCHS_ENABLED, self.tab_enabled(ui::TAB_EPOCHS));
        m.set(ui::EPOCH_VIEW_BIND, f64::from(self.ep_view));
        m.set(ui::FIELD_EROSION_BIND, self.field_erosion);
        m.set(ui::TAB_EROSION_ENABLED, self.tab_enabled(ui::TAB_EROSION));
        m.set(ui::TAB_TILE_ENABLED, self.tab_enabled(ui::TAB_TILE));
        // The tile: what the centre pane shows, and the readouts.
        let on_tile = self.sel_tab() == ui::TAB_TILE;
        let on_field = self.field_shown();
        m.set(ui::GLOBE_SHOWN, !on_tile);
        m.set(ui::TILE_SHOWN, on_tile && self.tile.is_some() && !on_field);
        m.set(ui::FIELD_SHOWN, on_field);
        m.set(ui::TILE_VIEW_BIND, f64::from(self.tile_view));
        m.set(
            ui::TILE_QUADS_BIND,
            match &self.tile {
                Some(t) if t.done && self.field_ready => format!(
                    "{} \u{00b7} {}",
                    group_thousands(self.field_quads.0 as u64),
                    group_thousands(self.field_quads.1 as u64)
                ),
                Some(_) => "\u{2026}".to_string(),
                None => "\u{2014}".to_string(),
            },
        );
        let dim = self.tile.as_ref().map_or(1.0, |t| f64::from(t.dim));
        m.set(ui::IMG_W_BIND, dim);
        m.set(ui::IMG_H_BIND, dim);
        let facing = self.facing_hex();
        let whereabouts = where_on_the_ball(&self.map, facing);
        m.set(
            ui::TILE_HEX_BIND,
            match &self.tile {
                Some(t) if t.cell == facing => format!("{facing} {whereabouts}"),
                Some(t) => format!("{facing} {whereabouts} ({})", t.cell),
                None if self.tile_job.is_some() => format!("{facing} {whereabouts} \u{2026}"),
                None => format!("{facing} {whereabouts}"),
            },
        );
        let (column, span, rough) = match &self.tile {
            Some(t) => (
                format!("{:.0} m", t.column_m),
                format!("{:.0}\u{2013}{:.0} m", t.lo_m, t.hi_m),
                format!("{:.2} m/px", t.rough_m),
            ),
            None => ("\u{2014}".into(), "\u{2014}".into(), "\u{2014}".into()),
        };
        m.set(ui::TILE_COLUMN_BIND, column);
        m.set(ui::TILE_SPAN_BIND, span);
        m.set(ui::TILE_ROUGH_BIND, rough);
        m.set(ui::CUT_COUNT_BIND, self.cut_count.clone());
        m.set(
            ui::TILE_CUT_BIND,
            match &self.tile {
                Some(t) if !t.done => {
                    format!("{} \u{00b7} {:.2} Mt \u{2026}", t.cut, t.exported_kg * 1e-9)
                }
                Some(t) => format!("{} \u{00b7} {:.2} Mt", t.cut, t.exported_kg * 1e-9),
                None => "\u{2014}".to_string(),
            },
        );
        // The era's box and readouts.
        m.set(ui::ERA_COUNT_BIND, self.era_count.clone());
        let era_ticks = self.era_ticks();
        m.set(
            ui::ERA_TICKS_BIND,
            if self.era_pending > 0 {
                format!("{era_ticks} +{}", self.era_pending)
            } else {
                era_ticks.to_string()
            },
        );
        let (sea_delta, texture) = match &self.era {
            Some(era) if era.ticks() > 0 => (
                (era.relief_sea() - self.composed.sea) * 100.0,
                era.texture_ratio(&self.map).unwrap_or(1.0),
            ),
            _ => (0.0, 1.0),
        };
        m.set(ui::ERA_SEA_BIND, format!("{sea_delta:+.3}%"));
        m.set(ui::ERA_TEXTURE_BIND, format!("{texture:.2}"));
        // The loop's box and readout: steps done, +owed, where the live
        // step stands (△ epochs owed, ▽ cycles owed).
        m.set(ui::LOOP_COUNT_BIND, self.loop_count.clone());
        let (done, owed) = (self.loops_done, self.loop_pending);
        m.set(
            ui::ERA_LOOPS_BIND,
            match self.loop_phase {
                LoopPhase::Idle if owed == 0 => done.to_string(),
                LoopPhase::Idle => format!("{done} +{owed}"),
                LoopPhase::Epochs(n) => format!("{done} +{owed} \u{25b3}{n}"),
                LoopPhase::Cycles(n) => format!("{done} +{owed} \u{25bd}{n}"),
            },
        );
        // The epochs' dials and readouts.
        m.set(
            ui::EPOCH_RATE_BIND,
            f64::from(dial(self.epoch_recipe.rate, HUNDREDTH)),
        );
        m.set(
            ui::EPOCH_UPLIFT_BIND,
            f64::from(dial(self.epoch_recipe.uplift, TENTH)),
        );
        m.set(
            ui::EPOCH_RIFT_BIND,
            f64::from(dial(self.epoch_recipe.rift, TENTH)),
        );
        m.set(
            ui::EPOCH_IMPACTS_BIND,
            f64::from(self.epoch_recipe.impacts.round().max(0.0) as u32),
        );
        let st = self.epochs.as_ref().map(|e| e.stats()).unwrap_or_default();
        m.set(ui::EPOCH_COUNT_BIND, self.epoch_count.clone());
        m.set(
            ui::EPOCH_TICKS_BIND,
            if self.epochs_pending > 0 {
                format!("{} +{}", st.ticks, self.epochs_pending)
            } else {
                st.ticks.to_string()
            },
        );
        m.set(ui::EPOCH_RAFTS_BIND, st.rafts.to_string());
        m.set(
            ui::EPOCH_CRUST_BIND,
            format!("{} \u{25bc} \u{00b7} {} \u{25b3}", st.consumed, st.opening),
        );
        m.set(
            ui::EPOCH_UPLIFT_VAL_BIND,
            format!("{:+.3}%", st.uplifted * 100.0),
        );
        let (eruptions, strikes) = self.epochs.as_ref().map_or((0, 0), Epochs::events_so_far);
        m.set(
            ui::EPOCH_EVENTS_BIND,
            format!("{eruptions} \u{00b7} {strikes}"),
        );
        // The wave's dials.
        m.set(ui::SEA_BIND, f64::from(self.sea_percent));
        m.set(ui::AMP_BIND, f64::from(self.amp_percent));
        m.set(ui::LOBE_BIND, f64::from(self.lobe_dial));
        m.set(ui::MASK_SEED_BIND, f64::from(self.mask_seed));
        // The stack's dials and its slots.
        m.set(
            ui::RES_SPREAD_BIND,
            f64::from(dial(self.stack.spread, TENTH)),
        );
        m.set(ui::RES_SEAM_BIND, f64::from(dial(self.stack.seam, 1.0)));
        m.set(ui::RES_MASK_SEED_BIND, self.stack.mask_seed as f64);
        for (i, s) in self.stack.slots.iter().enumerate() {
            m.set(ui::slot_on_bind(i), s.on);
            m.set(ui::slot_freq_bind(i), f64::from(s.frequency));
            // The amplitude stepper speaks percent of radius.
            m.set(ui::slot_amp_bind(i), f64::from(s.amp) * 100.0);
            m.set(ui::slot_axes_bind(i), f64::from(s.axes));
            m.set(ui::slot_mask_bind(i), s.mask.index() as f64);
        }
        m.set(ui::RES_ACTIVE_BIND, self.stack.active().to_string());
        m.set(ui::RES_SEED_BIND, format!("{:#018x}", self.stack.seed));
        // The heat's and the emitters'.
        m.set(ui::CELLS_BIND, f64::from(self.cells));
        m.set(ui::SPOTS_BIND, f64::from(self.spots));
        m.set(ui::CELLS_VAL_BIND, self.cells.to_string());
        m.set(ui::SPOTS_VAL_BIND, self.spots.to_string());
        m.set(ui::SEAMS_SEED_BIND, format!("{:#018x}", self.heat_seed));
        m.set(ui::RADIUS_BIND, f64::from(self.radius_tiles));
        m.set(ui::ARMS_BIND, f64::from(self.arms));
        m.set(ui::RINGS_BIND, f64::from(self.rings));
        m.set(ui::HEIGHT_BIND, f64::from(self.height));
        m.set(ui::EMITTERS_VAL_BIND, self.crust.vents().len().to_string());
        // The readouts.
        m.set(ui::HEXES_BIND, group_thousands(self.map.len() as u64));
        let land = self.land();
        m.set(ui::LAND_BIND, f64::from(land));
        m.set(ui::LAND_VAL_BIND, format!("{}%", (land * 100.0).round()));
        m.set(
            ui::WATER_VAL_BIND,
            format!("{}%", ((1.0 - land) * 100.0).round()),
        );
        m.set(ui::SEA_VAL_BIND, format!("{:+.2}%", self.shown_sea * 100.0));
        let (up, down) = self.wave.crests();
        m.set(
            ui::CRESTS_BIND,
            format!("{up} \u{25b2} \u{00b7} {down} \u{25bc}"),
        );
        m.set(ui::SEED_BIND, format!("{:#018x}", self.wave_seed));
        m
    }

    /// **The ONE dispatcher.** Every bind ECHOES its resting value each frame,
    /// so only a CHANGED value does anything; a change re-composes the recipe
    /// once and re-bakes.
    pub fn apply_results(&mut self, results: &ValueMap) {
        let pages = ui::PAGES.len();
        let mut page_changed = false;
        if let Some(v) = results.number(ui::PAGE_BIND) {
            let want = (v.round().max(0.0) as usize).min(pages.saturating_sub(1));
            if want != self.sel_page {
                self.sel_page = want;
                page_changed = true;
            }
        }
        if !page_changed {
            if let Some(v) = results.number(ui::TAB_BIND) {
                let tabs = ui::page(self.sel_page).tabs.len();
                let want = (v.round().max(0.0) as usize).min(tabs.saturating_sub(1));
                if want != self.sel_tab() && self.tab_enabled(want) {
                    self.page_tabs[self.sel_page] = want;
                }
            }
        }
        let mut changed = false;
        // THE FIELDS' switches.
        for (bind, flag) in [
            (ui::FIELD_WAVE_BIND, &mut self.field_wave),
            (ui::FIELD_MASK_BIND, &mut self.field_mask),
            (ui::FIELD_RES_BIND, &mut self.stack.on),
            (ui::FIELD_HEAT_BIND, &mut self.field_heat),
            (ui::FIELD_EMIT_BIND, &mut self.field_emit),
            (ui::FIELD_BUBBLES_BIND, &mut self.bubbles.on),
        ] {
            if let Some(Value::Bool(v)) = results.get(bind) {
                if *v != *flag {
                    *flag = *v;
                    changed = true;
                }
            }
        }
        // The dials: (bind, dial, min, max).
        let lobe_lo = (MIN_LOBE * LOBE_DIAL_SCALE).round() as u32;
        let lobe_hi = (MAX_LOBE * LOBE_DIAL_SCALE).round() as u32;
        for (bind, dial, lo, hi) in [
            (
                ui::SEA_BIND,
                &mut self.sea_percent,
                SEA_RANGE.0,
                SEA_RANGE.1,
            ),
            (
                ui::AMP_BIND,
                &mut self.amp_percent,
                AMP_RANGE.0,
                AMP_RANGE.1,
            ),
            (ui::LOBE_BIND, &mut self.lobe_dial, lobe_lo, lobe_hi),
            (
                ui::MASK_SEED_BIND,
                &mut self.mask_seed,
                MASK_SEED_RANGE.0,
                MASK_SEED_RANGE.1,
            ),
            (
                ui::RADIUS_BIND,
                &mut self.radius_tiles,
                RADIUS_RANGE.0,
                RADIUS_RANGE.1,
            ),
            (ui::ARMS_BIND, &mut self.arms, ARMS_RANGE.0, ARMS_RANGE.1),
            (
                ui::RINGS_BIND,
                &mut self.rings,
                RINGS_RANGE.0,
                RINGS_RANGE.1,
            ),
            (
                ui::HEIGHT_BIND,
                &mut self.height,
                HEIGHT_RANGE.0,
                HEIGHT_RANGE.1,
            ),
        ] {
            if let Some(v) = results.number(bind) {
                let want = (v.round().max(0.0) as u32).clamp(lo, hi);
                if want != *dial {
                    *dial = want;
                    changed = true;
                }
            }
        }
        // THE STACK's dials: SPREAD waits for the next roll; SEAM, WAVES and
        // MASK SEED are direct.
        if let Some(v) = results.number(ui::RES_SPREAD_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(RES_SPREAD_RANGE.0, RES_SPREAD_RANGE.1);
            if want != dial(self.stack.spread, TENTH) {
                self.stack.spread = want as f32 * TENTH;
                changed = true;
            }
        }
        if let Some(v) = results.number(ui::RES_SEAM_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(RES_SEAM_RANGE.0, RES_SEAM_RANGE.1);
            if want != dial(self.stack.seam, 1.0) {
                self.stack.seam = want as f32;
                changed = true;
            }
        }
        if let Some(v) = results.number(ui::RES_MASK_SEED_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(MASK_SEED_RANGE.0, MASK_SEED_RANGE.1);
            if u64::from(want) != self.stack.mask_seed {
                self.stack.mask_seed = u64::from(want);
                changed = true;
            }
        }
        // THE SLOTS: each row's switch, frequency, amplitude and mask.
        for (i, slot) in self.stack.slots.iter_mut().enumerate() {
            if let Some(Value::Bool(v)) = results.get(&ui::slot_on_bind(i)) {
                if *v != slot.on {
                    slot.on = *v;
                    changed = true;
                }
            }
            if let Some(v) = results.number(&ui::slot_mask_bind(i)) {
                let want = MaskKind::from_index(v.round().max(0.0) as usize);
                if want != slot.mask {
                    slot.mask = want;
                    changed = true;
                }
            }
            if let Some(v) = results.number(&ui::slot_freq_bind(i)) {
                let want = v
                    .round()
                    .clamp(f64::from(ui::FREQ_RANGE.0), f64::from(ui::FREQ_RANGE.1))
                    as f32;
                if want != slot.frequency {
                    slot.frequency = want;
                    changed = true;
                }
            }
            if let Some(v) = results.number(&ui::slot_axes_bind(i)) {
                let want = (v.round().max(0.0) as u32).clamp(MIN_AXES, MAX_AXES);
                if want != slot.axes {
                    slot.axes = want;
                    changed = true;
                }
            }
            if let Some(v) = results.number(&ui::slot_amp_bind(i)) {
                // Percent to two decimals → hundredths of a percent, the dial's grain.
                let want = (v * 100.0)
                    .round()
                    .clamp(f64::from(SLOT_AMP_RANGE.0), f64::from(SLOT_AMP_RANGE.1))
                    as u32;
                if want != dial(slot.amp, HUNDREDTH_PERCENT) {
                    slot.amp = want as f32 * HUNDREDTH_PERCENT;
                    changed = true;
                }
            }
        }
        if results.is_on(ui::RES_REROLL_ACTION) {
            // Every slot's waves, and how far each is heard from its dial.
            self.stack.seed = fastrand::u64(..);
            changed = true;
        }
        if results.is_on(ui::RANDOMIZE_ACTION) {
            self.wave_seed = fastrand::u64(..);
            changed = true;
        }
        if changed {
            // The readout wave follows the roll and the lobe.
            self.wave = PlanetWave::with_lobe(
                &self.map,
                self.wave_seed,
                self.lobe_dial as f32 / LOBE_DIAL_SCALE,
            );
        }
        // THE HEAT dials and its re-roll: the seams and the vents follow.
        let mut seams_moved = false;
        for (bind, dial, lo, hi) in [
            (ui::CELLS_BIND, &mut self.cells, 2u32, 12u32),
            (ui::SPOTS_BIND, &mut self.spots, 0, 12),
        ] {
            if let Some(v) = results.number(bind) {
                let want = (v.round().max(0.0) as u32).clamp(lo, hi);
                if want != *dial {
                    *dial = want;
                    seams_moved = true;
                }
            }
        }
        if results.is_on(ui::SEAMS_ACTION) {
            self.heat_seed = fastrand::u64(..);
            seams_moved = true;
        }
        if seams_moved {
            self.seams = SeamField::new(&self.map, self.cells, self.spots, self.heat_seed);
            self.crust = CrustField::derive(&self.map, &self.seams);
            changed = true;
        }
        // THE EPOCHS: the field switch shows or hides them; a dial change
        // starts them over; TICK / RUN advance, RESET returns to the composed
        // world, REROLL rolls another plate scheme. The RUN box is remembered
        // as typed.
        if let Some(t) = results.text(ui::EPOCH_COUNT_BIND) {
            if t != self.epoch_count {
                self.epoch_count = t.to_string();
            }
        }
        let mut view_moved = false;
        if let Some(Value::Bool(v)) = results.get(ui::FIELD_EPOCHS_BIND) {
            if *v != self.field_epochs {
                self.field_epochs = *v;
                view_moved = true;
            }
        }
        if let Some(v) = results.number(ui::EPOCH_VIEW_BIND) {
            let want = (v.round().max(0.0) as u32).min(ui::EPOCH_VIEW_HARDNESS);
            if want != self.ep_view {
                self.ep_view = want;
            }
        }
        // THE EROSION: the field switch shows or hides the era; TICK / RUN
        // advance it over the world before it, RESET returns to that world.
        if let Some(t) = results.text(ui::ERA_COUNT_BIND) {
            if t != self.era_count {
                self.era_count = t.to_string();
            }
        }
        if let Some(Value::Bool(v)) = results.get(ui::FIELD_EROSION_BIND) {
            if *v != self.field_erosion {
                self.field_erosion = *v;
                view_moved = true;
            }
        }
        let era_reset = results.is_on(ui::ERA_RESET_ACTION);
        let era_tick = results.is_on(ui::ERA_TICK_ACTION);
        let era_run = results.is_on(ui::ERA_RUN_ACTION);
        // THE LOOP: the box is remembered as typed; LOOP plays the steps in
        // it across frames (LOOP again pauses); the era's own buttons take
        // the era back from a loop in flight.
        if let Some(t) = results.text(ui::LOOP_COUNT_BIND) {
            if t != self.loop_count {
                self.loop_count = t.to_string();
            }
        }
        let loop_run = results.is_on(ui::LOOP_RUN_ACTION);
        if let Some(t) = results.text(ui::CUT_COUNT_BIND) {
            if t != self.cut_count {
                self.cut_count = t.to_string();
            }
        }
        if results.is_on(ui::TILE_MAKE_ACTION) {
            self.start_tile();
        }
        if let Some(v) = results.number(ui::TILE_VIEW_BIND) {
            let want = (v.round().max(0.0) as u32).min(ui::TILE_VIEW_FIELD);
            if want != self.tile_view {
                self.tile_view = want;
            }
        }
        if results.is_on(ui::RENDER_HEX_ACTION) {
            self.start_hex();
        }
        if results.is_on(ui::RENDER_WORLD_ACTION) {
            self.start_world();
        }
        if results.is_on(ui::STOP_ACTION) {
            self.stop_materializer();
        }
        let mut epochs_reset = false;
        if let Some(v) = results.number(ui::EPOCH_RATE_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(EPOCH_RATE_RANGE.0, EPOCH_RATE_RANGE.1);
            if want != dial(self.epoch_recipe.rate, HUNDREDTH) {
                self.epoch_recipe.rate = want as f32 * HUNDREDTH;
                epochs_reset = true;
            }
        }
        if let Some(v) = results.number(ui::EPOCH_UPLIFT_BIND) {
            let want =
                (v.round().max(0.0) as u32).clamp(EPOCH_UPLIFT_RANGE.0, EPOCH_UPLIFT_RANGE.1);
            if want != dial(self.epoch_recipe.uplift, TENTH) {
                self.epoch_recipe.uplift = want as f32 * TENTH;
                epochs_reset = true;
            }
        }
        if let Some(v) = results.number(ui::EPOCH_RIFT_BIND) {
            let want = (v.round().max(0.0) as u32).clamp(EPOCH_RIFT_RANGE.0, EPOCH_RIFT_RANGE.1);
            if want != dial(self.epoch_recipe.rift, TENTH) {
                self.epoch_recipe.rift = want as f32 * TENTH;
                epochs_reset = true;
            }
        }
        if let Some(v) = results.number(ui::EPOCH_IMPACTS_BIND) {
            let want =
                (v.round().max(0.0) as u32).clamp(EPOCH_IMPACTS_RANGE.0, EPOCH_IMPACTS_RANGE.1);
            if want != self.epoch_recipe.impacts.round().max(0.0) as u32 {
                self.epoch_recipe.impacts = want as f32;
                epochs_reset = true;
            }
        }
        if results.is_on(ui::EPOCH_RESET_ACTION) {
            epochs_reset = true;
        }
        self.clamp_tab_to_enabled();
        if changed {
            self.derive();
            self.bake();
        } else if epochs_reset {
            // The crust starts over, and with it the era it fed and the loop.
            self.epochs = None;
            self.loops_done = 0;
            self.reset_era();
            self.refresh();
            self.bake();
        } else if results.is_on(ui::EPOCH_TICK_ACTION) {
            self.tick_epochs(1);
            self.bake();
        } else if results.is_on(ui::EPOCH_RUN_ACTION) {
            // RUN plays the count in the box across frames; RUN again pauses.
            self.reset_era();
            self.epochs_pending = if self.epochs_pending > 0 {
                0
            } else {
                self.epoch_count
                    .parse::<u32>()
                    .ok()
                    .filter(|c| *c > 0)
                    .unwrap_or(ui::EPOCH_RUN_DEFAULT)
            };
            self.baked_at = self.epoch_ticks();
        } else if era_reset {
            self.reset_era();
            self.refresh();
            self.bake();
        } else if era_tick {
            self.stop_loop();
            self.tick_era(1);
            self.bake();
        } else if loop_run {
            // LOOP plays the steps in the box across frames; LOOP again pauses.
            if self.loop_pending > 0 || self.loop_phase != LoopPhase::Idle {
                self.stop_loop();
            } else {
                self.loop_pending = self
                    .loop_count
                    .parse::<u32>()
                    .ok()
                    .filter(|c| *c > 0)
                    .unwrap_or(ui::LOOP_RUN_DEFAULT);
                self.loop_baked_at = self.loop_work;
            }
        } else if era_run {
            self.stop_loop();
            self.era_pending = if self.era_pending > 0 {
                0
            } else {
                self.era_count
                    .parse::<u32>()
                    .ok()
                    .filter(|c| *c > 0)
                    .unwrap_or(ui::ERA_RUN_DEFAULT)
            };
            self.era_baked_at = self.era_ticks();
        } else if view_moved {
            self.refresh();
            self.bake();
        }
        self.world.show(self.view_key());
    }
}

/// Where a tile is on the ball, as a reader says it: `23°N 45°W`.
/// **The quad field as the engine draws it**: the field's region meshes
/// (`FIELD_REGION` clusters a side) turned into the renderer's vertices —
/// field for field, as the voxel bench converts a cluster's mesh — drawn
/// FLAT (each face's normal from its own geometry, so a quad's four corners
/// serve its top and the walls that meet there), with the field's framing.
fn field_landing(field: &flicker_voxel::QuadField, mask: usize) -> FieldLanding {
    // The contoured surface, smooth-shaded by its own normals — never the
    // flat-shaded cubes with walls of the first field (Aaron 2026-10-19:
    // "what you have built... is a voxel engine").
    let opts = MeshDrawOptions::default();
    let regions = field.surface_meshes(ui::FIELD_REGION);
    let quads: usize = regions.iter().map(|r| r.quads).sum();
    let meshes: BakedSet = regions
        .into_iter()
        .map(|r| {
            let verts: Vec<MeshVertex> = r
                .mesh
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.position,
                    normal: v.normal,
                    material: v.material,
                })
                .collect();
            (verts, r.mesh.indices, opts)
        })
        .collect();
    let (centre, radius) = field.framing();
    FieldLanding {
        meshes,
        vectors: field.quads.len(),
        quads,
        mask,
        centre,
        radius,
    }
}

fn where_on_the_ball(map: &HexMap, tile: u32) -> String {
    let d = map.direction(tile);
    let lat = d.y.clamp(-1.0, 1.0).asin().to_degrees();
    let lon = d.z.atan2(d.x).to_degrees();
    format!(
        "{:.0}\u{00b0}{} {:.0}\u{00b0}{}",
        lat.abs(),
        if lat < 0.0 { "S" } else { "N" },
        lon.abs(),
        if lon < 0.0 { "W" } else { "E" }
    )
}

/// One AUTHORED colour out of the scene's own styles, as rgb; an unresolved
/// path warns and comes back BLACK.
fn rgb(styles: &serde_json::Value, path: &str) -> [f32; 3] {
    let v = style_value(styles, path);
    match v.and_then(|v| v.as_array()) {
        Some(a) if a.len() >= 3 => {
            let mut out = [0.0f32; 3];
            for (i, c) in a.iter().take(3).enumerate() {
                out[i] = c.as_f64().unwrap_or(0.0) as f32;
            }
            out
        }
        _ => {
            tracing::warn!("worldbench: style `{path}` is not a resolved colour — black");
            [0.0; 3]
        }
    }
}

fn style_number(styles: &serde_json::Value, path: &str, default: f32) -> f32 {
    style_value(styles, path)
        .and_then(|v| v.as_f64())
        .map_or(default, |v| v as f32)
}

fn style_value<'a>(styles: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = styles;
    for seg in path.split('.') {
        match cur.get(seg) {
            Some(v) => cur = v,
            None => {
                tracing::warn!("worldbench: no style at `{path}`");
                return None;
            }
        }
    }
    Some(cur)
}

/// A dial's integer position for a value in `unit`s.
fn dial(value: f32, unit: f32) -> u32 {
    (value / unit).round().max(0.0) as u32
}

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

impl Scene for WorldBench {
    fn input_context(&self) -> Option<InputContext> {
        self.ui_state
            .text_entry()
            .then_some(InputContext::TextEntry)
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0];
        let theme = Theme::build(renderer);
        let entries = theme.lua_textures();
        self.textures = entries.iter().map(|(_, h)| *h).collect();
        self.theme = Some(theme);
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.world.free(renderer);
        self.worldmap.free(renderer);
        self.field.free(renderer);
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
        if let Some((_map, look, _gp)) = flicker_shell::take_pending_input() {
            self.world.set_controls(look);
            self.field.set_controls(look);
        }
        let screen = renderer.size();
        let mut model = self.model();
        if let Err(e) = self.script.set_model(&model) {
            tracing::error!("worldbench: publishing the model to the script failed: {e}");
        }
        match self.script.arrange() {
            Ok(Some(arrangement)) => model.extend(arrangement.to_model()),
            Ok(None) => {}
            Err(e) => tracing::error!("worldbench: arrange() failed: {e}"),
        }
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen,
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
        self.world.seat(frame.surface(ui::VIEW_SLOT));
        let pointer = frame.surface_pointer(ui::VIEW_SLOT).cloned();
        // The MAP's sheet: seated only while its page is lit (a dark page
        // seats `None` and costs nothing).
        self.worldmap.seat(frame.surface(ui::MAP_SLOT));
        let map_pointer = frame.surface_pointer(ui::MAP_SLOT).cloned();
        // THE FIELD's seat: only while the TILE tab's VIEW shows it.
        self.field.seat(frame.surface(ui::FIELD_SLOT));
        let field_pointer = frame.surface_pointer(ui::FIELD_SLOT).cloned();
        self.hud_commands = frame.commands;
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&self.tree, &model)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 3] =
                [&mut walker, &mut self.world, &mut self.field];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let mut results = frame.results;
        for name in walker.take_fired() {
            results.set(name, true);
        }
        drop(walker);
        self.apply_results(&results);
        self.pump_epochs(EPOCH_FRAME_BUDGET);
        self.pump_era(EPOCH_FRAME_BUDGET);
        self.pump_loop(EPOCH_FRAME_BUDGET);
        self.poll_tile();
        self.pump_map();
        let dtf = dt.as_secs_f32();
        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        let look_gate = self.ui_state.focused_pane();
        // On the MAP page the same stick that flies the globe pans the sheet
        // and the globe holds still; on the FIELD it flies the field.
        let on_map = self.sel_page == ui::PAGE_MAP;
        let on_field = self.field_shown();
        let still = (0.0, 0.0, 0.0);
        self.world.update(
            dtf,
            pointer.as_ref(),
            if on_map || on_field { still } else { look },
            if on_map || on_field { None } else { look_gate },
        );
        self.field.update(
            dtf,
            field_pointer.as_ref(),
            if on_field { look } else { still },
            if on_field { look_gate } else { None },
        );
        self.worldmap.update(
            dtf,
            map_pointer.as_ref(),
            if on_map { look } else { still },
            on_map,
        );
        if let Some(picked) = self.worldmap.take_pick() {
            self.pick_map(picked as u32);
        }
        if results.is_on("pause_open") {
            if let Some(theme) = self.theme {
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
        if let Some(t) = flicker_shell::stage_prompt(self.theme, &mut self.ui_state) {
            return t;
        }
        Transition::None
    }

    fn modal_closed(&mut self, _modal: &str, result: &str, _payload: Option<&str>) {
        flicker_shell::stage_prompt_closed(&mut self.ui_state, result);
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // The hex picture reaches the HUD here, where the renderer is ours to
        // write: uploaded once, updated in place when a new hex lands, and
        // appended to the theme's textures at the slot the tree's sprite names.
        // The map's pictures: a landed hex becomes a texture (a pool of
        // MAP_PICTURES_RESIDENT, reused oldest-first) laid on its cell.
        let centre = self.map_centre.map(|c| self.map.direction(c));
        for (cell, rgba, dim, span_over_radius) in self.map_pending.drain(..) {
            let handle = if let Some(handle) = self.map_free.pop() {
                // A texture another world's picture left: reused.
                if !renderer.update_texture(handle, &rgba) {
                    continue;
                }
                handle
            } else if self.map_resident.len() < ui::MAP_PICTURES_RESIDENT {
                renderer.load_texture(&rgba, dim, dim)
            } else {
                // The pool is full: the picture farthest from the view's
                // centre gives its texture up (the oldest, with no view).
                let victim = match centre {
                    Some(c) => self
                        .map_resident
                        .iter()
                        .enumerate()
                        .min_by(|(_, (a, _)), (_, (b, _))| {
                            self.map
                                .direction(*a)
                                .dot(c)
                                .total_cmp(&self.map.direction(*b).dot(c))
                        })
                        .map(|(i, _)| i),
                    None => Some(0),
                };
                let Some((old, handle)) = victim.and_then(|i| self.map_resident.remove(i)) else {
                    continue;
                };
                self.worldmap.content_mut().clear_picture(old);
                if !renderer.update_texture(handle, &rgba) {
                    continue;
                }
                handle
            };
            self.map_resident.retain(|(c, _)| *c != cell);
            self.map_resident.push_back((cell, handle));
            self.worldmap
                .content_mut()
                .set_picture(cell, handle, span_over_radius);
        }
        if self.tile_dirty {
            if let Some(t) = &self.tile {
                match self.tile_texture {
                    Some(handle) if renderer.update_texture(handle, &t.rgba) => {}
                    _ => self.tile_texture = Some(renderer.load_texture(&t.rgba, t.dim, t.dim)),
                }
            }
            self.tile_dirty = false;
        }
        let Self {
            world,
            worldmap,
            field,
            hud_commands,
            textures,
            tile_texture,
            ..
        } = self;
        let layer = fg.base_layer();
        world.render(renderer, fg, layer);
        worldmap.render(renderer, fg, layer);
        field.render(renderer, fg, layer);
        if let Some(&white) = textures.first() {
            let mut all: Vec<TextureHandle> = textures.clone();
            if let Some(t) = tile_texture {
                debug_assert_eq!(all.len() as u32, ui::TILE_TEXTURE);
                all.push(*t);
            }
            fg.overlay(move |r| render_hud(r, hud_commands, white, &all));
        }
    }
}

/// Build the bench as a boxed `Scene` — the CLIENT BEHAVIOUR the roster registers.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(WorldBench::new(def))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker::render::Vec2;
    use flicker::ui::load_shared_styles;
    use flicker_worldengine::{metrics, HarmonicWave, WaveMask, DEFAULT_LAYERS, SEA_CREST_PERCENT};

    const WORLDBENCH_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/worldbench.scene.json");

    fn test_bench() -> WorldBench {
        let def =
            SceneDef::parse("worldbench", WORLDBENCH_SCENE).expect("worldbench.scene.json loads");
        WorldBench::new(&def)
    }

    fn set(k: &str, v: Value) -> ValueMap {
        let mut r = ValueMap::default();
        r.set(k, v);
        r
    }

    /// Leave only the base WAVE expressed: mask, resonance, emitters and
    /// bubbles off.
    fn isolate_wave(bench: &mut WorldBench) {
        let mut r = ValueMap::default();
        for k in [
            ui::FIELD_MASK_BIND,
            ui::FIELD_RES_BIND,
            ui::FIELD_EMIT_BIND,
            ui::FIELD_BUBBLES_BIND,
        ] {
            r.set(k, false);
        }
        bench.apply_results(&r);
    }

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

    /// **THE BENCH COMPOSES EXACTLY WHAT THE EVALUATOR COMPOSES** — the one
    /// representation: the recipe the dials spell, composed by the engine's
    /// `compose`, IS the heights and the sea the globe draws; the recipe
    /// round-trips through JSON; the readouts equal the metrics.
    #[test]
    fn the_bench_composes_exactly_what_the_evaluator_composes() {
        let mut bench = test_bench();
        let check = |bench: &WorldBench| {
            let r = bench.recipe();
            let c = compose(bench.map(), &r);
            assert_eq!(c.heights, bench.heights(), "the heights are the recipe's");
            assert_eq!(c.sea, bench.sea(), "the sea is the recipe's");
            assert_eq!(c.hardness, bench.hardness(), "the hardness is the recipe's");
            let json = serde_json::to_string(&r).unwrap();
            let back: Recipe = serde_json::from_str(&json).unwrap();
            assert_eq!(back, r, "the recipe round-trips as data");
            let m = metrics(bench.map(), &c);
            assert!(
                (m.land_share - bench.land()).abs() < 1e-6,
                "the readouts measure the same"
            );
        };
        check(&bench);
        bench.apply_results(&set(ui::RES_SEAM_BIND, Value::Number(5.0)));
        bench.apply_results(&set(&ui::slot_freq_bind(3), Value::Number(31.0)));
        bench.apply_results(&set(&ui::slot_mask_bind(2), Value::Number(0.0)));
        bench.apply_results(&set(ui::RES_REROLL_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::SEAMS_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::RANDOMIZE_ACTION, Value::Bool(true)));
        check(&bench);
        let r = bench.recipe();
        assert_eq!(r.stack.slots.len(), ui::RESONANCE_SLOTS);
        assert_eq!(r.stack.slots[3].frequency, 31.0);
        assert_eq!(r.stack.slots[2].mask, MaskKind::Off);
        assert!((r.stack.seam - 5.0).abs() < 1e-6);
        // THE BENCH OPENS ON THE EVALUATOR'S PRESET AT THE SWEEP'S SEED — the
        // recipe's own dials and the layers exactly, every roll the opening
        // seed's, the spare slots off.
        let fresh = test_bench().recipe();
        let preset = Recipe::default_at(fresh.freq, Recipe::OPENING_SEED);
        assert_eq!(
            fresh.wave.seed, preset.wave.seed,
            "the window opens on the sweep's world"
        );
        assert_eq!(fresh.heat.seed, preset.heat.seed);
        assert_eq!(fresh.stack.seed, preset.stack.seed);
        assert_eq!(fresh.sea_percent, preset.sea_percent);
        assert!((fresh.wave.amp - preset.wave.amp).abs() < 1e-6);
        assert!((fresh.wave.lobe - preset.wave.lobe).abs() < 1e-6);
        assert!((fresh.stack.spread - preset.stack.spread).abs() < 1e-6);
        assert!((fresh.stack.seam - preset.stack.seam).abs() < 1e-6);
        let n = preset.stack.slots.len();
        assert_eq!(&fresh.stack.slots[..n], &preset.stack.slots[..]);
        assert!(
            fresh.stack.slots[n..].iter().all(|s| !s.on),
            "the spare slots open off"
        );
        assert_eq!(fresh.heat.emitters, preset.heat.emitters);
    }

    /// **The globe's ink resolves**: every relief stop and the sea are authored
    /// colours, no stop is black, the sea stops are blue, the land stops are not.
    #[test]
    fn the_globe_ink_resolves_from_the_scene_styles() {
        let bench = test_bench();
        let r = bench.ink.ramp;
        for (name, c) in [
            ("deep", r.deep),
            ("shallow", r.shallow),
            ("lowland", r.lowland),
            ("upland", r.upland),
            ("summit", r.summit),
            ("sea", bench.ink.sea),
            ("soft", bench.ink.soft),
            ("hard", bench.ink.hard),
        ] {
            assert!(c.iter().any(|x| *x > 0.0), "relief.{name} resolves");
        }
        assert!(
            bench.ink.hard.iter().sum::<f32>() > bench.ink.soft.iter().sum::<f32>(),
            "hard crust draws brighter than soft"
        );
        assert!(
            r.deep[2] > r.deep[0] && r.shallow[2] > r.shallow[0],
            "the sea is blue"
        );
        assert!(
            r.lowland[1] > r.lowland[2],
            "the lowland is green, not blue"
        );
        assert!(r.summit.iter().all(|x| *x > 0.7), "the summit is pale");
    }

    #[test]
    fn the_screen_names_known_kinds_with_no_raw_strings() {
        let tree = test_bench().build_tree();
        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(unknown.is_empty(), "{unknown:?}");
        let raw = flicker::ui::raw_display_literals(&tree);
        assert!(raw.is_empty(), "{raw:?}");
    }

    /// **The static tree is the Populous frame around a V5 world**: panes,
    /// viewport, footer; one rail option per tab gated on its field's enabled
    /// key; the FIELDS tree's switches; every tab's slice gated; one row per
    /// resonance slot with its three controls.
    #[test]
    fn the_static_tree_carries_the_bench_frame_and_every_slice() {
        let tree = test_bench().build_tree();
        let all = flatten(&tree);
        for id in [ui::LEFT_PANE, ui::VIEW_PANE, ui::RIGHT_PANE, ui::NAV_PANE] {
            assert!(all.iter().any(|n| n.id == id), "`{id}` is in the tree");
        }
        let view = all
            .iter()
            .find(|n| n.id == ui::VIEW_SLOT)
            .expect("the viewport");
        assert_eq!(view.component, "surface");
        // THE FIELD's surface shares the pane, shown only by the model's
        // FIELD_SHOWN, authored by its own stage; the TILE tab's VIEW pill
        // chooses it.
        let field = all
            .iter()
            .find(|n| n.id == ui::FIELD_SLOT)
            .expect("the field's surface");
        assert_eq!(field.component, "surface");
        assert_eq!(field.visible_bind.as_deref(), Some(ui::FIELD_SHOWN));
        assert_eq!(
            field.props.get("source"),
            Some(&Value::Text(ui::FIELD_STAGE_SOURCE.into()))
        );
        assert!(
            all.iter().any(|n| n.component == "pill_toggle"
                && n.bind.as_deref() == Some(ui::TILE_VIEW_BIND)
                && n.children.len() == 2),
            "the TILE tab's VIEW pill: PICTURE | FIELD"
        );
        assert!(all.iter().any(|n| n.component == "paged_menu"));
        assert!(all.iter().any(|n| n.component == "nav_footer"));
        let rail = all
            .iter()
            .find(|n| n.component == "pill_toggle")
            .expect("the tab rail");
        let gated: Vec<&str> = rail
            .children
            .iter()
            .filter_map(|o| o.enabled_bind.as_deref())
            .collect();
        assert_eq!(
            gated,
            [
                ui::TAB_WAVE_ENABLED,
                ui::TAB_RES_ENABLED,
                ui::TAB_HEAT_ENABLED,
                ui::TAB_EMIT_ENABLED,
                ui::TAB_EPOCHS_ENABLED,
                ui::TAB_EROSION_ENABLED,
                ui::TAB_TILE_ENABLED,
            ]
        );
        assert_eq!(rail.children.len(), ui::PAGES[0].tabs.len());
        let gates: std::collections::HashSet<&str> = all
            .iter()
            .filter_map(|n| n.visible_bind.as_deref())
            .collect();
        for t in 0..ui::PAGES[0].tabs.len() {
            assert!(
                gates.contains(format!("shown_p0_t{t}").as_str()),
                "tab {t} is gated"
            );
        }
        for bind in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_MASK_BIND,
            ui::FIELD_RES_BIND,
            ui::FIELD_HEAT_BIND,
            ui::FIELD_EMIT_BIND,
            ui::FIELD_BUBBLES_BIND,
            ui::FIELD_EPOCHS_BIND,
            ui::FIELD_EROSION_BIND,
        ] {
            assert!(
                all.iter()
                    .any(|n| n.component == "checkbox" && n.bind.as_deref() == Some(bind)),
                "the `{bind}` switch is present"
            );
        }
        for i in 0..ui::RESONANCE_SLOTS {
            for (bind, kind) in [
                (ui::slot_on_bind(i), "checkbox"),
                (ui::slot_freq_bind(i), "stepper"),
                (ui::slot_amp_bind(i), "stepper"),
                (ui::slot_axes_bind(i), "stepper"),
                (ui::slot_mask_bind(i), "pill_toggle"),
            ] {
                assert!(
                    all.iter()
                        .any(|n| n.component == kind && n.bind.as_deref() == Some(bind.as_str())),
                    "slot {i}'s `{bind}` is a {kind}"
                );
            }
        }
    }

    /// **The viewport and the controls get their pixels** on the wave slice, and
    /// the resonance slice's rows resolve inside the left pane.
    #[test]
    fn the_viewport_and_the_dials_resolve_with_extent() {
        let bench = test_bench();
        let tree = bench.build_tree();
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
        let mut model = bench.model();
        model.set("shown_page0", true);
        model.set("shown_p0_t0", true);
        let mut state = UiState::default();
        let frame = run_ui(&tree, &model, &styles, &snap, &mut state);
        let rect = frame
            .surface_rect(ui::VIEW_SLOT)
            .expect("the viewport's rect");
        assert!(
            (rect.size.x - rect.size.y).abs() < 1.5,
            "square: {}×{}",
            rect.size.x,
            rect.size.y
        );
        assert!(rect.size.y > 300.0);
        for id in [ui::LEFT_PANE, ui::RIGHT_PANE] {
            let r = frame.rect(id).unwrap_or_else(|| panic!("{id} resolves"));
            assert!(r.size.x > 100.0 && r.size.y > 300.0, "{id} has real pixels");
        }
        let left = frame.rect(ui::LEFT_PANE).unwrap();
        for bind in [
            ui::SEA_BIND,
            ui::AMP_BIND,
            ui::LOBE_BIND,
            ui::MASK_SEED_BIND,
        ] {
            let dial = frame
                .rect(bind)
                .unwrap_or_else(|| panic!("{bind} resolves"));
            assert!(
                dial.size.y > 100.0,
                "{bind}'s track is grabbable: {}",
                dial.size.y
            );
            assert!(
                dial.pos.x >= left.pos.x && dial.pos.x + dial.size.x <= left.pos.x + left.size.x,
                "{bind} sits inside the left pane"
            );
        }
        assert!(frame.rect(ui::NAV_PANE).expect("the footer").size.x > 600.0);
        // The resonance slice: every slot row's stepper has real pixels in the pane.
        let mut model = bench.model();
        model.set("shown_page0", true);
        model.set("shown_p0_t1", true);
        let mut state = UiState::default();
        let frame = run_ui(&tree, &model, &styles, &snap, &mut state);
        for i in 0..ui::RESONANCE_SLOTS {
            let bind = ui::slot_freq_bind(i);
            let amp = frame
                .rect(&ui::slot_amp_bind(i))
                .expect("the amplitude stepper resolves");
            assert!(amp.size.x > 60.0, "the amplitude stepper has pixels");
            let r = frame
                .rect(&bind)
                .unwrap_or_else(|| panic!("{bind} resolves"));
            assert!(
                r.size.x > 60.0 && r.size.y > 14.0,
                "{bind} has pixels: {}×{}",
                r.size.x,
                r.size.y
            );
            assert!(
                r.pos.y + r.size.y <= left.pos.y + left.size.y + 1.0,
                "{bind} stays inside the pane: row bottom {} vs pane bottom {} (pane {}..{} tall {})",
                r.pos.y + r.size.y,
                left.pos.y + left.size.y,
                left.pos.y,
                left.pos.y + left.size.y,
                left.size.y
            );
        }
        for bind in [
            ui::RES_SPREAD_BIND,
            ui::RES_SEAM_BIND,
            ui::RES_MASK_SEED_BIND,
        ] {
            let dial = frame
                .rect(bind)
                .unwrap_or_else(|| panic!("{bind} resolves"));
            assert!(
                dial.size.y > 80.0,
                "{bind}'s track is grabbable: {}",
                dial.size.y
            );
        }
    }

    /// **Six up, six down, and the sea runs the dial's law** on the isolated wave.
    #[test]
    fn the_wave_crests_six_and_the_sea_dial_floods_from_floor_to_crest() {
        let mut bench = test_bench();
        assert_eq!(bench.wave().crests(), (6, 6));
        isolate_wave(&mut bench);
        let dial = |pct: f64| set(ui::SEA_BIND, Value::Number(pct));
        bench.apply_results(&dial(0.0));
        assert!(
            (bench.land() - 1.0).abs() < 1e-3,
            "0% = all land: {}",
            bench.land()
        );
        let mut last = bench.land();
        for pct in [10.0, 30.0, 49.0, 70.0, 90.0] {
            bench.apply_results(&dial(pct));
            assert!(
                bench.land() <= last + 1e-6,
                "the land share falls as the sea rises"
            );
            last = bench.land();
        }
        bench.apply_results(&dial(49.0));
        assert!(
            (bench.land() - 0.5).abs() < 0.03,
            "the mean drowns half: {}",
            bench.land()
        );
        bench.apply_results(&dial(f64::from(SEA_CREST_PERCENT)));
        assert!(bench.land() < 1e-3, "the summit at the waterline");
        bench.apply_results(&dial(100.0));
        assert_eq!(bench.land(), 0.0, "the planet is drowned");
        // THE DATUM IS THE WAVE'S: the stack, the stamps and the mask move it by nothing.
        bench.apply_results(&dial(49.0));
        let sea = bench.sea();
        for k in [ui::FIELD_EMIT_BIND, ui::FIELD_MASK_BIND, ui::FIELD_RES_BIND] {
            bench.apply_results(&set(k, Value::Bool(true)));
            assert_eq!(bench.sea(), sea, "{k} does not move the sea");
        }
        bench.apply_results(&dial(500.0));
        assert_eq!(bench.dials().0, SEA_RANGE.1, "the dial has ends");
    }

    /// **The fields gate their tabs and their contribution**: a child's tab
    /// inherits its parent's enable; a switch off moves a selection standing on
    /// its tab; every field off is a flat sphere; back on, everything returns.
    #[test]
    fn the_fields_gate_their_tabs_and_their_contribution() {
        let mut bench = test_bench();
        assert_eq!(bench.fields(), (true, true, true, true, true));
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_EMIT as f64)));
        assert_eq!(bench.selection(), (0, ui::TAB_EMIT));
        bench.apply_results(&set(ui::FIELD_HEAT_BIND, Value::Bool(false)));
        assert_eq!(bench.selection(), (0, ui::TAB_WAVE), "off a dimmed tab");
        let m = bench.model();
        assert_eq!(m.get(ui::TAB_HEAT_ENABLED), Some(&Value::Bool(false)));
        assert_eq!(
            m.get(ui::TAB_EMIT_ENABLED),
            Some(&Value::Bool(false)),
            "no heat, no emitters"
        );
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_EMIT as f64)));
        assert_eq!(
            bench.selection(),
            (0, ui::TAB_WAVE),
            "a dimmed tab is refused"
        );
        bench.apply_results(&set(ui::FIELD_HEAT_BIND, Value::Bool(true)));
        // Every field off: a flat sphere; back on: exactly as before. The
        // BUBBLES are a field of their own: off alone, the relief moves and
        // the sea stands.
        let before = bench.heights().to_vec();
        let sea = bench.sea();
        assert!(before.iter().any(|h| *h != 0.0));
        bench.apply_results(&set(ui::FIELD_BUBBLES_BIND, Value::Bool(false)));
        assert_ne!(bench.heights(), &before[..], "the bubbles stood somewhere");
        assert_eq!(bench.sea(), sea, "the bubbles do not move the sea");
        bench.apply_results(&set(ui::FIELD_BUBBLES_BIND, Value::Bool(true)));
        assert_eq!(bench.heights(), &before[..]);
        for k in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_RES_BIND,
            ui::FIELD_HEAT_BIND,
            ui::FIELD_BUBBLES_BIND,
        ] {
            bench.apply_results(&set(k, Value::Bool(false)));
        }
        assert!(
            bench.heights().iter().all(|h| *h == 0.0),
            "every field off is a flat sphere"
        );
        for k in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_RES_BIND,
            ui::FIELD_HEAT_BIND,
            ui::FIELD_BUBBLES_BIND,
        ] {
            bench.apply_results(&set(k, Value::Bool(true)));
        }
        assert!(bench.heights() == &before[..], "back on, exactly as before");
    }

    /// **Relief, lobe and the re-roll each move one thing** on the isolated wave.
    #[test]
    fn the_relief_lobe_and_reroll_each_move_one_thing() {
        let mut bench = test_bench();
        isolate_wave(&mut bench);
        let seed = bench.wave().seed();
        let hi0 = bench.heights().iter().copied().fold(f32::MIN, f32::max);
        let twice = f64::from(bench.dials().1 * 2);
        bench.apply_results(&set(ui::AMP_BIND, Value::Number(twice)));
        let hi1 = bench.heights().iter().copied().fold(f32::MIN, f32::max);
        assert!((hi1 / hi0 - 2.0).abs() < 1e-3, "relief doubles the heights");
        assert_eq!(bench.wave().seed(), seed);
        bench.apply_results(&set(ui::LOBE_BIND, Value::Number(120.0)));
        assert!((bench.wave().lobe() - 12.0).abs() < 1e-6);
        assert_eq!(bench.wave().seed(), seed);
        bench.apply_results(&set(ui::RANDOMIZE_ACTION, Value::Bool(true)));
        assert_ne!(bench.wave().seed(), seed);
        assert_eq!(bench.wave().crests(), (6, 6));
        // The echo contract: the resting values re-applied move nothing.
        let before = bench.model();
        let mut echo = ValueMap::default();
        echo.set(ui::SEA_BIND, f64::from(bench.dials().0));
        echo.set(ui::AMP_BIND, f64::from(bench.dials().1));
        echo.set(ui::LOBE_BIND, f64::from(bench.dials().2));
        bench.apply_results(&echo);
        let after = bench.model();
        for key in [
            ui::SEA_VAL_BIND,
            ui::LAND_VAL_BIND,
            ui::SEED_BIND,
            ui::CRESTS_BIND,
        ] {
            assert_eq!(before.text(key), after.text(key), "{key} stood still");
        }
    }

    /// **The HEAT tab drives the seam field**, and the recipe carries its roll.
    #[test]
    fn the_heat_dials_drive_the_seam_field_and_the_emitters_follow() {
        let mut bench = test_bench();
        let wave_seed = bench.wave().seed();
        let seams_seed = bench.seams().seed();
        let mut r = ValueMap::default();
        r.set(ui::CELLS_BIND, 5.0);
        r.set(ui::SPOTS_BIND, 3.0);
        bench.apply_results(&r);
        assert_eq!((bench.seams().cells(), bench.seams().spots()), (5, 3));
        assert_eq!(bench.seams().seed(), seams_seed);
        bench.apply_results(&set(ui::SEAMS_ACTION, Value::Bool(true)));
        assert_ne!(bench.seams().seed(), seams_seed);
        assert_eq!(bench.wave().seed(), wave_seed, "the wave is its own field");
        assert_eq!(bench.recipe().heat.seed, bench.seams().seed());
        let m = bench.model();
        assert_eq!(m.text(ui::CELLS_VAL_BIND), Some("5"));
        assert_eq!(m.text(ui::SPOTS_VAL_BIND), Some("3"));
    }

    /// **A slot is a wave train with its own height and its own mask cut.**
    /// The stack opens on the LAYERS with the spare slots off; one slot alone
    /// adds exactly its frequency through its mask at its heard height; the
    /// `a` stepper turns one slot and no other; SPREAD 0 pins what is heard
    /// to the dial; the SEAM cut stands a layer only on the mask wave's nodal
    /// lines; the stack's re-roll rolls the waves and the stray but not the
    /// dials, and the planetary wave stands still; the stack's switch gates its
    /// tab and its contribution.
    #[test]
    fn a_slot_is_a_wave_train_with_its_own_height_and_mask_cut() {
        let mut bench = test_bench();
        for (i, (f, _, _, mask)) in DEFAULT_LAYERS.iter().enumerate() {
            assert_eq!(
                bench.slot(i),
                (*f as u32, true, *mask),
                "slot {i} opens on the layers"
            );
        }
        for i in DEFAULT_LAYERS.len()..ui::RESONANCE_SLOTS {
            assert!(!bench.slot(i).1, "spare slot {i} opens off");
        }
        // Isolate the stack, then one REGION-cut slot: the basins.
        for k in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_EMIT_BIND,
            ui::FIELD_BUBBLES_BIND,
        ] {
            bench.apply_results(&set(k, Value::Bool(false)));
        }
        for i in 0..ui::RESONANCE_SLOTS {
            bench.apply_results(&set(&ui::slot_on_bind(i), Value::Bool(false)));
        }
        assert!(bench.heights().iter().all(|h| *h == 0.0));
        let i = 2;
        bench.apply_results(&set(&ui::slot_on_bind(i), Value::Bool(true)));
        let stack = bench.recipe().stack;
        let slot = stack.slots[i];
        assert_eq!((slot.frequency, slot.mask), (6.0, MaskKind::Region));
        let wave = HarmonicWave::new(
            bench.map(),
            stack.field_seed(i, slot.frequency),
            slot.frequency,
            slot.axes,
        );
        let mask = WaveMask::new(bench.map(), stack.mask_seed_of(i, slot.frequency));
        let heard = stack.heard(i, &slot);
        for t in 0..bench.map().len() {
            let want = wave.value(t as u32) * heard * mask.value(t as u32);
            assert!(
                (bench.heights()[t] - want).abs() < 1e-7,
                "tile {t}: the slot is its frequency through its mask at its heard height"
            );
        }
        let masked = bench.heights().to_vec();
        bench.apply_results(&set(&ui::slot_mask_bind(i), Value::Number(0.0)));
        let bare = bench.heights().to_vec();
        assert_ne!(masked, bare, "its mask filters it");
        for (a, b) in masked.iter().zip(&bare) {
            assert!(a.abs() <= b.abs() + 1e-6, "the region cut only mutes");
        }
        // The SEAM cut: with the WAVE on (so there are land masses with margins)
        // the layer stands on a minority of the surface, as a lift on the wave.
        bench.apply_results(&set(ui::FIELD_WAVE_BIND, Value::Bool(true)));
        bench.apply_results(&set(&ui::slot_on_bind(i), Value::Bool(false)));
        let wave_only = bench.heights().to_vec();
        bench.apply_results(&set(&ui::slot_on_bind(i), Value::Bool(true)));
        bench.apply_results(&set(&ui::slot_mask_bind(i), Value::Number(2.0)));
        assert_eq!(bench.slot(i).2, MaskKind::Seam);
        let standing = bench
            .heights()
            .iter()
            .zip(&wave_only)
            .filter(|(h, w)| (*h - *w).abs() > 1e-6)
            .count();
        assert!(
            standing > 0 && standing < bench.map().len() / 2,
            "ridges along seams: {standing}"
        );
        bench.apply_results(&set(ui::FIELD_WAVE_BIND, Value::Bool(false)));
        bench.apply_results(&set(&ui::slot_mask_bind(i), Value::Number(0.0)));
        assert_eq!(bench.heights(), &bare[..], "back to the bare train");
        // Another slot on: the two add.
        let j = 0;
        bench.apply_results(&set(&ui::slot_on_bind(j), Value::Bool(true)));
        let both = bench.heights().to_vec();
        bench.apply_results(&set(&ui::slot_on_bind(i), Value::Bool(false)));
        let only_j = bench.heights().to_vec();
        for t in 0..both.len() {
            assert!(
                (both[t] - (bare[t] + only_j[t])).abs() < 1e-6,
                "tile {t}: slots add"
            );
        }
        // The steppers re-tune a slot — and no other: its frequency, its wave count.
        bench.apply_results(&set(&ui::slot_axes_bind(j), Value::Number(3.0)));
        assert_eq!(bench.recipe().stack.slots[j].axes, 3);
        assert_eq!(bench.recipe().stack.slots[i].axes, 2);
        bench.apply_results(&set(&ui::slot_axes_bind(j), Value::Number(1.0)));
        bench.apply_results(&set(&ui::slot_freq_bind(j), Value::Number(7.0)));
        assert_eq!(bench.slot(j).0, 7);
        assert_eq!(bench.slot(i).0, 6);
        assert_ne!(
            bench.heights(),
            &only_j[..],
            "another frequency, another field"
        );
        bench.apply_results(&set(&ui::slot_freq_bind(j), Value::Number(9_999_999.0)));
        assert_eq!(bench.slot(j).0, ui::FREQ_RANGE.1, "the stepper has ends");
        // At the stepper's end the slot is finer than the hex grid carries:
        // the globe composes none of it (the 2K hex evaluates it exactly) —
        // one list, two resolutions. Back to a frequency the grid carries.
        assert!(
            bench.heights().iter().all(|h| *h == 0.0),
            "a slot under the hex is the pixel tier's alone"
        );
        bench.apply_results(&set(&ui::slot_freq_bind(j), Value::Number(31.0)));
        assert!(bench.heights().iter().any(|h| *h != 0.0));
        // EACH WAVE HAS ITS OWN HEIGHT: the `a` stepper turns one slot's dial and
        // no other's; SPREAD 0 makes the heard height the dial exactly.
        let dials: Vec<f32> = bench.recipe().stack.slots.iter().map(|s| s.amp).collect();
        bench.apply_results(&set(&ui::slot_amp_bind(j), Value::Number(3.0)));
        let after: Vec<f32> = bench.recipe().stack.slots.iter().map(|s| s.amp).collect();
        assert!((after[j] - 0.03).abs() < 1e-7, "three percent of radius");
        for k in 0..after.len() {
            if k != j {
                assert_eq!(after[k], dials[k], "slot {k} keeps its dial");
            }
        }
        bench.apply_results(&set(ui::RES_SPREAD_BIND, Value::Number(0.0)));
        let pinned = bench.recipe().stack;
        for (k, s) in pinned.slots.iter().enumerate() {
            assert!(
                (pinned.heard(k, s) - s.amp).abs() < 1e-9,
                "spread 0 is the dial"
            );
        }
        let hi = bench.heights().iter().copied().fold(f32::MIN, f32::max);
        assert!(
            (hi - 0.03).abs() < 1e-4,
            "one unmasked train alone crests at its dial: {hi}"
        );
        // The stack's re-roll rolls the waves and the stray, not the dials; the
        // planetary wave stands still.
        bench.apply_results(&set(ui::RES_SPREAD_BIND, Value::Number(10.0)));
        let before = bench.heights().to_vec();
        let wave_seed = bench.wave().seed();
        bench.apply_results(&set(ui::RES_REROLL_ACTION, Value::Bool(true)));
        let rolled = bench.recipe().stack;
        assert_ne!(rolled.seed, pinned.seed);
        assert_eq!(bench.wave().seed(), wave_seed);
        assert_ne!(bench.heights(), &before[..], "another roll, another world");
        let dials_after: Vec<f32> = rolled.slots.iter().map(|s| s.amp).collect();
        assert_eq!(dials_after, after, "the dials stand");
        for (k, s) in rolled.slots.iter().enumerate() {
            let heard = rolled.heard(k, s);
            assert!(
                heard >= s.amp * 0.5 - 1e-9 && heard <= s.amp * 2.0 + 1e-9,
                "within an octave"
            );
        }
        // The stack's switch gates its tab and its contribution.
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_RES as f64)));
        assert_eq!(bench.selection(), (0, ui::TAB_RES));
        bench.apply_results(&set(ui::FIELD_RES_BIND, Value::Bool(false)));
        assert_eq!(
            bench.selection(),
            (0, ui::TAB_HEAT),
            "off its dimmed tab (the wave is off)"
        );
        assert!(bench.heights().iter().all(|h| *h == 0.0));
        assert_eq!(
            bench.model().get(ui::TAB_RES_ENABLED),
            Some(&Value::Bool(false))
        );
        assert_eq!(bench.model().text(ui::RES_ACTIVE_BIND), Some("1"));
    }

    /// **The epochs tick from the composed world and reset to it.** The
    /// composed relief is the initial state; TICK stands the epochs up on the
    /// recipe's seams and moves the relief; RUN advances ten; the readouts
    /// count; RESET returns the globe to the composed relief; a recipe change
    /// starts the epochs over; the EPOCHS switch hides them without losing
    /// them; off, its tab dims.
    #[test]
    fn the_epochs_tick_from_the_composed_world_and_reset_to_it() {
        let mut bench = test_bench();
        assert_eq!(bench.epoch_ticks(), 0);
        assert_eq!(bench.heights(), bench.composed());
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        assert_eq!(bench.epoch_ticks(), 1);
        // RUN plays the count in the box across frames.
        bench.apply_results(&set(ui::EPOCH_COUNT_BIND, Value::Text("7".into())));
        bench.apply_results(&set(ui::EPOCH_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.epochs_pending(), 7);
        assert_eq!(bench.model().text(ui::EPOCH_TICKS_BIND), Some("1 +7"));
        bench.pump_epochs(Duration::from_secs(600));
        assert_eq!((bench.epoch_ticks(), bench.epochs_pending()), (8, 0));
        assert_ne!(bench.heights(), bench.composed(), "the relief moved");
        assert_eq!(bench.model().text(ui::EPOCH_TICKS_BIND), Some("8"));
        // An unparseable box runs the default; RUN again pauses.
        bench.apply_results(&set(ui::EPOCH_COUNT_BIND, Value::Text("".into())));
        bench.apply_results(&set(ui::EPOCH_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.epochs_pending(), ui::EPOCH_RUN_DEFAULT);
        bench.apply_results(&set(ui::EPOCH_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.epochs_pending(), 0);
        assert_eq!(bench.model().text(ui::EPOCH_COUNT_BIND), Some(""));
        let ticked = bench.heights().to_vec();
        bench.apply_results(&set(ui::FIELD_EPOCHS_BIND, Value::Bool(false)));
        assert_eq!(
            bench.heights(),
            bench.composed(),
            "off, the composed world stands"
        );
        assert_eq!(
            bench.model().get(ui::TAB_EPOCHS_ENABLED),
            Some(&Value::Bool(false))
        );
        bench.apply_results(&set(ui::FIELD_EPOCHS_BIND, Value::Bool(true)));
        assert_eq!(bench.heights(), &ticked[..], "on again, the epochs resume");
        assert_eq!(bench.epoch_ticks(), 8);
        bench.apply_results(&set(ui::EPOCH_RESET_ACTION, Value::Bool(true)));
        assert_eq!(bench.epoch_ticks(), 0);
        assert_eq!(bench.heights(), bench.composed());
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::SEA_BIND, Value::Number(50.0)));
        assert_eq!(
            bench.epoch_ticks(),
            0,
            "a recomposed world starts the epochs over"
        );
        assert_eq!(bench.heights(), bench.composed());
        // The epoch dials have ends; a change starts the epochs over; another
        // heat roll moves the seams the composition stands.
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        let seams_before = bench.composed().to_vec();
        bench.apply_results(&set(ui::SEAMS_ACTION, Value::Bool(true)));
        assert_ne!(
            bench.composed(),
            &seams_before[..],
            "another heat roll moves the seams"
        );
        assert_eq!(
            bench.epoch_ticks(),
            0,
            "a recomposed world starts the epochs over"
        );
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::EPOCH_RATE_BIND, Value::Number(0.0)));
        assert_eq!(bench.epoch_ticks(), 0);
        assert_eq!(
            bench.model().number(ui::EPOCH_RATE_BIND),
            Some(f64::from(EPOCH_RATE_RANGE.0))
        );
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::EPOCH_RIFT_BIND, Value::Number(999.0)));
        assert_eq!(bench.epoch_ticks(), 0);
        assert_eq!(
            bench.model().number(ui::EPOCH_RIFT_BIND),
            Some(f64::from(EPOCH_RIFT_RANGE.1))
        );
        assert_eq!(bench.model().text(ui::EPOCH_RAFTS_BIND), Some("0"));
        // THE VIEW PILL: on the epochs tab, HARDNESS shows the ledger set;
        // RELIEF (and any other tab) the relief; the ledger follows the epochs.
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_EPOCHS as f64)));
        assert_eq!(bench.view_key(), WAVE_SET);
        bench.apply_results(&set(
            ui::EPOCH_VIEW_BIND,
            Value::Number(f64::from(ui::EPOCH_VIEW_HARDNESS)),
        ));
        assert_eq!(bench.view_key(), HARD_SET);
        assert_eq!(
            bench.model().number(ui::EPOCH_VIEW_BIND),
            Some(f64::from(ui::EPOCH_VIEW_HARDNESS))
        );
        assert_eq!(bench.hardness(), &bench.composed.hardness[..]);
        bench.apply_results(&set(ui::EPOCH_RUN_ACTION, Value::Bool(true)));
        bench.pump_epochs(Duration::from_secs(600));
        assert!(bench.epoch_ticks() > 0);
        assert!(
            bench
                .hardness()
                .iter()
                .any(|h| (h - flicker_worldengine::Rock::Basalt.resistance()).abs() < 1e-6),
            "young floor shows basalt-hard in the ledger"
        );
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_WAVE as f64)));
        assert_eq!(bench.view_key(), WAVE_SET);
        bench.apply_results(&set(
            ui::EPOCH_VIEW_BIND,
            Value::Number(f64::from(ui::EPOCH_VIEW_RELIEF)),
        ));
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_EPOCHS as f64)));
        assert_eq!(bench.view_key(), WAVE_SET);
    }

    /// **The erosion tab adopts the world before it and resets with it.** The
    /// era adopts the composed world (or the epochs' once ticked): TICK runs
    /// one surface cycle and the relief, sea and hardness the globe draws are
    /// the era's; RUN plays the box; the readouts count; RESET returns to the
    /// world before; an epoch tick or a recipe change starts the era over;
    /// the EROSION switch hides it without losing it and dims its tab.
    #[test]
    fn the_erosion_tab_adopts_the_world_before_it_and_resets_with_it() {
        let mut bench = test_bench();
        assert_eq!(bench.era_ticks(), 0);
        let composed = bench.composed().to_vec();
        let sea0 = bench.sea();
        bench.apply_results(&set(ui::ERA_TICK_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_ticks(), 1);
        assert_ne!(bench.heights(), &composed[..], "the water cut");
        assert!(bench.heights().iter().all(|h| h.is_finite()));
        assert!(
            (bench.sea() - sea0).abs() < 0.002,
            "the sea holds: {sea0} → {}",
            bench.sea()
        );
        assert!(bench.hardness().iter().all(|h| (0.0..=1.0).contains(h)));
        assert_eq!(bench.model().text(ui::ERA_TICKS_BIND), Some("1"));
        assert!(bench
            .model()
            .text(ui::ERA_TEXTURE_BIND)
            .is_some_and(|s| s.parse::<f32>().is_ok_and(|v| v > 0.5)));
        // RUN plays the count in the box across frames; RUN again pauses.
        bench.apply_results(&set(ui::ERA_COUNT_BIND, Value::Text("3".into())));
        bench.apply_results(&set(ui::ERA_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_pending(), 3);
        assert_eq!(bench.model().text(ui::ERA_TICKS_BIND), Some("1 +3"));
        bench.pump_era(Duration::from_secs(600));
        assert_eq!((bench.era_ticks(), bench.era_pending()), (4, 0));
        // An unparseable box runs the default; RUN again pauses.
        bench.apply_results(&set(ui::ERA_COUNT_BIND, Value::Text("".into())));
        bench.apply_results(&set(ui::ERA_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_pending(), ui::ERA_RUN_DEFAULT);
        bench.apply_results(&set(ui::ERA_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_pending(), 0);
        // The switch hides the era; off, its tab dims; on, it is back.
        let eroded = bench.heights().to_vec();
        bench.apply_results(&set(ui::FIELD_EROSION_BIND, Value::Bool(false)));
        assert_eq!(bench.heights(), &composed[..]);
        assert_eq!(
            bench.model().get(ui::TAB_EROSION_ENABLED),
            Some(&Value::Bool(false))
        );
        bench.apply_results(&set(ui::FIELD_EROSION_BIND, Value::Bool(true)));
        assert_eq!(bench.heights(), &eroded[..]);
        assert_eq!(bench.era_ticks(), 4);
        // RESET returns to the world before.
        bench.apply_results(&set(ui::ERA_RESET_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_ticks(), 0);
        assert_eq!(bench.heights(), &composed[..]);
        assert_eq!(bench.sea(), sea0);
        // The era adopts the EPOCHS' world once ticked, and an epoch tick
        // starts it over.
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        let epoched = bench.heights().to_vec();
        bench.apply_results(&set(ui::ERA_TICK_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_ticks(), 1);
        assert_ne!(bench.heights(), &epoched[..]);
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        assert_eq!(bench.era_ticks(), 0, "an epoch tick starts the era over");
        assert_eq!(bench.epoch_ticks(), 2);
        bench.apply_results(&set(ui::ERA_TICK_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::SEA_BIND, Value::Number(50.0)));
        assert_eq!(
            bench.era_ticks(),
            0,
            "a recomposed world starts the era over"
        );
        assert_eq!(bench.epoch_ticks(), 0);
    }

    /// **The forever loop writes back, moves, adopts and cuts.** LOOP plays
    /// the steps in its box across frames: each step the era's cut is
    /// written into the crust's parcels, the crust moves `LOOP_EPOCHS`
    /// epochs, the era adopts the moved world anew and cuts `LOOP_CYCLES`
    /// cycles; the readout counts steps and shows where the live one
    /// stands; the cut carried into the crust makes the epochs' world
    /// differ from the same epochs run alone; LOOP again pauses, an era
    /// RESET stops it; the IMPACTS dial starts the epochs over and the
    /// EVENTS readout counts what landed.
    #[test]
    fn the_forever_loop_writes_back_moves_adopts_and_cuts() {
        let mut bench = test_bench();
        assert_eq!(bench.model().text(ui::ERA_LOOPS_BIND), Some("0"));
        bench.apply_results(&set(ui::LOOP_COUNT_BIND, Value::Text("2".into())));
        bench.apply_results(&set(ui::LOOP_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.loop_pending(), 2);
        assert_eq!(bench.model().text(ui::ERA_LOOPS_BIND), Some("0 +2"));
        // One frame's worth: the step began — the crust is moving, the era
        // stood down for it.
        bench.pump_loop(Duration::ZERO);
        assert!(matches!(bench.loop_phase(), LoopPhase::Epochs(_)));
        assert_eq!(bench.era_ticks(), 0);
        assert!(bench
            .model()
            .text(ui::ERA_LOOPS_BIND)
            .is_some_and(|s| s.starts_with("0 +2 \u{25b3}")));
        bench.pump_loop(Duration::from_secs(600));
        assert_eq!((bench.loops_done(), bench.loop_pending()), (2, 0));
        assert_eq!(bench.loop_phase(), LoopPhase::Idle);
        assert_eq!(bench.epoch_ticks(), 2 * u64::from(LOOP_EPOCHS));
        assert_eq!(
            bench.era_ticks(),
            u64::from(LOOP_CYCLES),
            "the era adopts the moved world anew each step"
        );
        assert_eq!(bench.model().text(ui::ERA_LOOPS_BIND), Some("2"));
        assert!(bench.heights().iter().all(|h| h.is_finite()));
        assert!(bench.hardness().iter().all(|h| (0.0..=1.0).contains(h)));
        // THE WRITE-BACK: the crust carries the cut — the epochs' world
        // after the loop differs from the same epochs run alone.
        bench.apply_results(&set(ui::FIELD_EROSION_BIND, Value::Bool(false)));
        let carried = bench.heights().to_vec();
        let mut plain = test_bench();
        plain.apply_results(&set(
            ui::EPOCH_COUNT_BIND,
            Value::Text((2 * LOOP_EPOCHS).to_string()),
        ));
        plain.apply_results(&set(ui::EPOCH_RUN_ACTION, Value::Bool(true)));
        plain.pump_epochs(Duration::from_secs(600));
        assert_eq!(plain.epoch_ticks(), bench.epoch_ticks());
        assert_ne!(
            plain.heights(),
            &carried[..],
            "the cut was written into the crust"
        );
        bench.apply_results(&set(ui::FIELD_EROSION_BIND, Value::Bool(true)));
        // LOOP again pauses; an era RESET stops a loop in flight.
        bench.apply_results(&set(ui::LOOP_COUNT_BIND, Value::Text("".into())));
        bench.apply_results(&set(ui::LOOP_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.loop_pending(), ui::LOOP_RUN_DEFAULT);
        bench.apply_results(&set(ui::LOOP_RUN_ACTION, Value::Bool(true)));
        assert_eq!(bench.loop_pending(), 0);
        bench.apply_results(&set(ui::LOOP_RUN_ACTION, Value::Bool(true)));
        bench.pump_loop(Duration::ZERO);
        bench.apply_results(&set(ui::ERA_RESET_ACTION, Value::Bool(true)));
        assert_eq!(
            (bench.loop_pending(), bench.loop_phase()),
            (0, LoopPhase::Idle)
        );
        assert_eq!(bench.era_ticks(), 0);
        // IMPACTS is a dial of the crust: it starts the epochs over (and the
        // loop's count with them); EVENTS counts what landed.
        bench.apply_results(&set(ui::EPOCH_IMPACTS_BIND, Value::Number(999.0)));
        assert_eq!(
            bench.model().number(ui::EPOCH_IMPACTS_BIND),
            Some(f64::from(EPOCH_IMPACTS_RANGE.1))
        );
        assert_eq!(bench.epoch_ticks(), 0);
        assert_eq!(bench.model().text(ui::ERA_LOOPS_BIND), Some("0"));
        assert_eq!(
            bench.model().text(ui::EPOCH_EVENTS_BIND),
            Some("0 \u{00b7} 0")
        );
    }

    /// **The map page draws picked hexes in the world.** The roster has the
    /// MAP page with no tabs and the rail shows both pages; the tree carries
    /// the map's surface with its stage source; a pick only PICKS (the hex
    /// outlined, nothing rendered); RENDER HEX queues it ahead of everything,
    /// the thread materializes it, the readout and gauge follow, and the
    /// finished picture lands for the sheet (the renderer lays it on the cell
    /// at the next frame); a pick outside the tiling is dropped; RENDER WORLD
    /// queues every hex and runs the thread through them until STOP pauses
    /// the walk.
    #[test]
    fn the_map_page_draws_picked_hexes_in_the_world() {
        // The cache under a directory of the test's own.
        let tmp = std::env::temp_dir().join(format!("wb-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var(crate::cache::ROOT_ENV, &tmp);
        let mut bench = test_bench();
        assert_eq!(ui::PAGES.len(), 2);
        assert!(ui::page(ui::PAGE_MAP).tabs.is_empty());
        let tree = bench.build_tree();
        let all = flatten(&tree);
        let surface = all
            .iter()
            .find(|n| n.id == ui::MAP_SLOT)
            .expect("the map's surface");
        assert_eq!(surface.component, "surface");
        assert_eq!(
            surface.props.get("source"),
            Some(&Value::Text(ui::MAP_STAGE_SOURCE.into()))
        );
        let rail = all
            .iter()
            .find(|n| n.id == "paged_pages")
            .expect("the page rail");
        assert_eq!(rail.children.len(), ui::PAGES.len());
        bench.apply_results(&set(ui::PAGE_BIND, Value::Number(ui::PAGE_MAP as f64)));
        assert_eq!(bench.selection(), (ui::PAGE_MAP, 0));
        // A pick: the hex goes to the thread at once, the ground as the
        // fields made it (no cut, for the test's patience).
        bench.apply_results(&set(ui::CUT_COUNT_BIND, Value::Text("0".into())));
        let cell = 123u32;
        bench.pick_map(cell);
        assert_eq!(bench.map_state(), (0, 0, false), "a click only picks");
        bench.pump_map_now();
        assert!(!bench.tile_in_flight(), "a click starts no render");
        assert_eq!(
            bench.map_picked(),
            Some(cell),
            "the sheet outlines the pick"
        );
        let m = bench.model();
        assert!(m
            .text(ui::MAP_PICK_BIND)
            .is_some_and(|s| s.starts_with("123 ")));
        assert_eq!(m.get(ui::HEX_READY_BIND), Some(&Value::Bool(true)));
        assert_eq!(m.get(ui::MAT_RUNNING_BIND), Some(&Value::Bool(false)));
        // RENDER HEX: now the hex goes to the thread, and the controls flip.
        bench.apply_results(&set(ui::RENDER_HEX_ACTION, Value::Bool(true)));
        assert_eq!(bench.map_state(), (0, 1, false));
        bench.pump_map_now();
        assert!(bench.tile_in_flight());
        let m = bench.model();
        assert_eq!(m.get(ui::MAT_RUNNING_BIND), Some(&Value::Bool(true)));
        assert_eq!(m.get(ui::HEX_READY_BIND), Some(&Value::Bool(false)));
        assert!(m
            .text(ui::MAP_HEX_BIND)
            .is_some_and(|s| s.starts_with("123 ")));
        let started = std::time::Instant::now();
        while bench.tile_in_flight() {
            bench.poll_tile();
            assert!(
                started.elapsed() < Duration::from_secs(300),
                "the hex materializes in time"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            bench.map_state(),
            (1, 0, false),
            "the picture landed for the sheet"
        );
        assert_eq!(bench.tile().map(|t| t.cell), Some(cell));
        // ... and in the cache, on disk, under the world's key.
        assert_eq!(bench.cached(), 1);
        let cached_png = std::fs::read_dir(&tmp)
            .expect("the cache root")
            .filter_map(|e| e.ok())
            .map(|e| e.path().join(format!("{cell}.png")))
            .find(|p| p.exists());
        assert!(cached_png.is_some(), "the hex's picture is on disk");
        assert!(bench
            .model()
            .text(ui::MAP_TILES_BIND)
            .is_some_and(|s| s.starts_with("1 / ")));
        // A pick outside the tiling is dropped, not clamped to a hex nobody chose.
        bench.pick_map(u32::MAX);
        assert_eq!(bench.map_state().1, 0);
        // RENDER WORLD: every hex queued, the job running; RENDER again pauses
        // it and the queue keeps; nothing starts while paused.
        bench.apply_results(&set(ui::RENDER_WORLD_ACTION, Value::Bool(true)));
        let (_, queued, running) = bench.map_state();
        assert!(running);
        assert_eq!(
            queued,
            bench.hex_count() - 1,
            "every hex but the cached one is owed"
        );
        assert!(bench
            .model()
            .text(ui::MAP_JOB_BIND)
            .is_some_and(|s| s.ends_with('\u{2026}')));
        bench.apply_results(&set(ui::STOP_ACTION, Value::Bool(true)));
        assert!(!bench.map_state().2, "STOP pauses the walk");
        bench.pump_map_now();
        assert!(!bench.tile_in_flight(), "paused, the thread rests");
        assert_eq!(
            bench.map_state().1,
            bench.hex_count() - 1,
            "the queue keeps"
        );
        // Another world is another cache: the sheet starts bare, the job stops.
        bench.apply_results(&set(ui::EPOCH_TICK_ACTION, Value::Bool(true)));
        bench.pick_map(7);
        bench.apply_results(&set(ui::RENDER_HEX_ACTION, Value::Bool(true)));
        assert_eq!(bench.cached(), 0, "a ticked world has its own cache");
        assert_eq!(
            bench.map_state().1,
            1,
            "the old queue is gone; the pick stands"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **The tile tab materializes the hex the camera faces** on its own
    /// thread: the centre pane swaps the globe for the picture once a hex
    /// has landed and the tab is selected; the report carries the picture at
    /// the pane's scale and the readings; the tree's sprite names the HUD
    /// texture slot the scene appends the picture at.
    #[test]
    fn the_tile_tab_materializes_the_facing_hex() {
        let mut bench = test_bench();
        let tree = bench.build_tree();
        let sprite = flatten(&tree)
            .into_iter()
            .find(|n| n.component == "sprite")
            .expect("the tile sprite");
        assert_eq!(
            sprite.props.get("tex"),
            Some(&Value::Number(f64::from(ui::TILE_TEXTURE))),
            "the sprite names the slot after the theme's textures"
        );
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_TILE as f64)));
        let m = bench.model();
        assert_eq!(m.get(ui::GLOBE_SHOWN), Some(&Value::Bool(false)));
        assert_eq!(
            m.get(ui::TILE_SHOWN),
            Some(&Value::Bool(false)),
            "no hex yet"
        );
        let facing = bench.facing_hex();
        // Two passes of rain in the test's patience; the default is the box's.
        assert_eq!(
            bench.model().text(ui::CUT_COUNT_BIND),
            Some(ui::CUT_DEFAULT.to_string().as_str())
        );
        bench.apply_results(&set(ui::CUT_COUNT_BIND, Value::Text("2".into())));
        bench.apply_results(&set(ui::TILE_MAKE_ACTION, Value::Bool(true)));
        assert!(bench.tile_in_flight());
        // The ground lands first, then the water's pictures; the job is in
        // flight until the cut has run its count.
        let started = std::time::Instant::now();
        let mut first_cut = None;
        while bench.tile_in_flight() {
            if bench.poll_tile() && first_cut.is_none() {
                first_cut = bench.tile().map(|t| t.cut);
            }
            assert!(
                started.elapsed() < Duration::from_secs(300),
                "the hex materializes in time"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            first_cut,
            Some(0),
            "the fields' ground shows before the water"
        );
        let t = bench.tile().expect("a hex landed");
        assert!(t.done);
        assert_eq!(t.cell, facing);
        assert_eq!(t.dim, flicker_worldtile::TILE_DIM / TILE_PREVIEW_SCALE);
        assert_eq!(t.rgba.len(), (t.dim * t.dim * 4) as usize);
        assert!(t.column_m > 0.0 && t.hi_m >= t.lo_m && t.rough_m >= 0.0);
        assert_eq!(t.cut, 2, "the water ran the box");
        assert!(bench
            .model()
            .text(ui::TILE_CUT_BIND)
            .is_some_and(|s| s.starts_with("2 ")));
        let m = bench.model();
        assert_eq!(m.get(ui::TILE_SHOWN), Some(&Value::Bool(true)));
        assert_eq!(m.number(ui::IMG_W_BIND), Some(f64::from(t.dim)));
        assert!(m
            .text(ui::TILE_COLUMN_BIND)
            .is_some_and(|s| s.ends_with(" m")));
        // THE FIELD landed with the last word: one vector a mask pixel, the
        // surface's quads counted, the meshes baked into the field world and
        // the camera framed; it shows when the VIEW pill says FIELD, the
        // picture standing down, and the globe stays dark while the tab is
        // lit.
        assert!(bench.field_ready(), "the field is baked");
        let (vectors, quads) = bench.field_quads;
        assert!(
            vectors > 3_000_000,
            "a hex is millions of clusters: {vectors}"
        );
        assert!(
            quads > vectors * 9 / 10 && quads < vectors,
            "a quad across every four vectors: {quads}"
        );
        assert!(m
            .text(ui::TILE_QUADS_BIND)
            .is_some_and(|s| s.contains(',') && s.contains('\u{00b7}')));
        assert_eq!(m.get(ui::FIELD_SHOWN), Some(&Value::Bool(false)));
        assert_eq!(
            m.number(ui::TILE_VIEW_BIND),
            Some(f64::from(ui::TILE_VIEW_PICTURE))
        );
        bench.apply_results(&set(
            ui::TILE_VIEW_BIND,
            Value::Number(f64::from(ui::TILE_VIEW_FIELD)),
        ));
        let m = bench.model();
        assert_eq!(m.get(ui::FIELD_SHOWN), Some(&Value::Bool(true)));
        assert_eq!(m.get(ui::TILE_SHOWN), Some(&Value::Bool(false)));
        assert_eq!(m.get(ui::GLOBE_SHOWN), Some(&Value::Bool(false)));
        assert!(bench.field_shown());
        let cam = bench.field.camera();
        assert!(
            cam.far > 1.0e6,
            "the field camera sees the whole hex: far {}",
            cam.far
        );
        assert!(
            (cam.target.y - cam.position.y).abs() < cam.far,
            "the camera stands over the field"
        );
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(ui::TAB_WAVE as f64)));
        let m = bench.model();
        assert_eq!(m.get(ui::GLOBE_SHOWN), Some(&Value::Bool(true)));
        assert_eq!(m.get(ui::TILE_SHOWN), Some(&Value::Bool(false)));
        assert_eq!(m.get(ui::FIELD_SHOWN), Some(&Value::Bool(false)));
        assert!(!bench.field_shown(), "the field needs the TILE tab");
    }

    /// **The field's counts are the tile's**: the thread's landing carries
    /// one vector per mask pixel (the gate the field module proves per pixel
    /// on a small tile; here the whole hex), the surface a quad across every
    /// four of them, one vertex a vector plus the aprons, smooth-shaded.
    #[test]
    fn the_field_landing_is_the_mask_in_vectors() {
        let mut bench = test_bench();
        bench.apply_results(&set(ui::CUT_COUNT_BIND, Value::Text("0".into())));
        bench.apply_results(&set(ui::TILE_MAKE_ACTION, Value::Bool(true)));
        let rx = bench.tile_job.take().expect("a job in flight");
        let report = rx
            .recv_timeout(Duration::from_secs(300))
            .expect("the hex lands in time");
        assert!(report.done, "no cut: the first word is the last");
        let landing = report.field.expect("the field rides the last word");
        assert_eq!(landing.vectors, landing.mask, "one vector a mask pixel");
        assert!(
            landing.quads > landing.vectors * 9 / 10 && landing.quads < landing.vectors,
            "a quad across every 2×2 block of vectors: {} of {}",
            landing.quads,
            landing.vectors
        );
        let (verts, idx): (usize, usize) = landing
            .meshes
            .iter()
            .fold((0, 0), |(v, i), (vs, is, _)| (v + vs.len(), i + is.len()));
        assert!(
            verts >= landing.vectors && verts < 2 * landing.vectors,
            "one vertex a vector, plus the aprons: {verts}"
        );
        assert_eq!(idx, 6 * landing.quads);
        assert!(
            landing.meshes.iter().all(|(_, _, o)| !o.flat),
            "the surface is smooth-shaded by its own normals, not flat cubes"
        );
        assert!(
            landing.radius > 200_000.0,
            "half a hex's diagonal in voxels"
        );
        assert!(landing.meshes.len() > 1, "a hex is several region meshes");
    }

    /// **The model publishes every bind the tree reads.**
    #[test]
    fn the_model_publishes_what_the_tree_binds() {
        let bench = test_bench();
        let m = bench.model();
        assert_eq!(m.number(ui::PAGE_BIND), Some(0.0));
        assert!(m.text(ui::HEXES_BIND).is_some_and(|s| s.contains(',')));
        assert!(m.text(ui::CRESTS_BIND).is_some_and(|s| s.starts_with("6 ")));
        assert_eq!(
            m.text(ui::RES_ACTIVE_BIND),
            Some(DEFAULT_LAYERS.len().to_string().as_str())
        );
        let tree = bench.build_tree();
        for n in flatten(&tree) {
            if let Some(b) = n.bind.as_deref() {
                assert!(
                    m.get(b).is_some(),
                    "the tree binds `{b}` and the model publishes it"
                );
            }
            if let Some(Value::Text(b)) = n.props.get("text_bind") {
                assert!(
                    m.get(b).is_some(),
                    "the tree reads `{b}` and the model publishes it"
                );
            }
        }
    }
}
