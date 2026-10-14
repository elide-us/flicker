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
//! The globe draws the authored shells EXTRUDED by the composed height channel,
//! the tile shell tinted trough → mean → crest, under a translucent SEA sphere
//! at the recipe's sea; the HEAT tab paints the molten seam field instead.

use std::time::Duration;

use flicker::render::{FrameGraph, MeshDrawOptions, Renderer, TextureHandle};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{render_hud, run_ui, SceneDef, UiInput, UiIntents, UiState, WalkerHandler};
use flicker_globe::{lerp3, temp_color, GlobeWorld, ShellSpec, RADIUS};
use flicker_input_core::{
    AbstractControls, ActionSignal, GamepadConfig, InputContext, InputMap, InputState,
};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_worldengine::{
    compose, land_share, CoastBandRecipe, CrustField, HarmonicRecipe, HeatRecipe, HexMap,
    MaskRecipe, PlanetWave, Recipe, RidgeRecipe, SeamField, VolcanoShape, WaveRecipe, DEFAULT_AXES,
    DEFAULT_CELLS, DEFAULT_FREQ, DEFAULT_LOBE, DEFAULT_SPOTS, MAX_AXES, MAX_LOBE, MIN_AXES,
    MIN_LOBE,
};

use crate::ui;

const WORLDBENCH_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/worldbench.lua");

/// The baked view sets: the WAVE (relief + sea) and the HEAT (the seam field
/// painted on the relief, no sea).
const WAVE_SET: &str = "wave";
const HEAT_SET: &str = "heat";

// ── the dials' ranges and openings: (min, max, default) ──
const SEA_RANGE: (u32, u32, u32) = (0, 100, 49);
/// RELIEF, percent of radius (Aaron: useful caps at ~5).
const AMP_RANGE: (u32, u32, u32) = (0, 8, 4);
const LOBE_DIAL_SCALE: f32 = 10.0;
/// PLATEAU exponent ×10.
const PLATEAU_RANGE: (u32, u32, u32) = (3, 20, 10);
const PLATEAU_DIAL_SCALE: f32 = 0.1;
const MASK_SEED_RANGE: (u32, u32, u32) = (0, 99, 1);
/// The wave RIDGE: level (wave value ×100, signed), width (×100), height
/// (half-percents of radius).
const RIDGE_LEVEL_RANGE: (i32, i32, i32) = (-50, 80, 15);
const RIDGE_WIDTH_RANGE: (u32, u32, u32) = (2, 40, 12);
const RIDGE_HEIGHT_RANGE: (u32, u32, u32) = (0, 12, 4);
const HUNDREDTH: f32 = 0.01;
const HALF_PERCENT: f32 = 0.005;
/// The COAST band: level, width, height in thousandths of the radius.
const COAST_LEVEL_RANGE: (u32, u32, u32) = (0, 20, 4);
const COAST_WIDTH_RANGE: (u32, u32, u32) = (1, 20, 3);
const COAST_HEIGHT_RANGE: (u32, u32, u32) = (0, 60, 30);
const THOUSANDTH: f32 = 0.001;
/// The harmonics' RELIEF rides tenths of a percent; RATIO rides hundredths.
const HARMONIC_AMP_SCALE: f32 = 0.001;
const RATIO_RANGE: (u32, u32) = (5, 1600);
/// The emitters: reach in tiles, arms, rings, height in half-percents.
const RADIUS_RANGE: (u32, u32, u32) = (2, 16, 6);
const ARMS_RANGE: (u32, u32, u32) = (3, 8, 6);
const RINGS_RANGE: (u32, u32, u32) = (1, 5, 2);
const HEIGHT_RANGE: (u32, u32, u32) = (0, 12, 1);

/// The scene's own ink, read ONCE out of its `wave` style block.
struct WaveInk {
    trough: [f32; 3],
    mean: [f32; 3],
    crest: [f32; 3],
    sea: [f32; 3],
    sea_alpha: f32,
    sea_gloss: f32,
}

/// One harmonic field's dials and switches.
struct HarmonicSlot {
    seed: u64,
    /// Ratio ×100.
    ratio_dial: u32,
    axes: u32,
    /// Relief, tenths of a percent of the radius.
    amp: u32,
    mask_seed: u32,
    on: bool,
    mask_on: bool,
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
    plateau_dial: u32,
    mask_seed: u32,
    field_wave: bool,
    field_mask: bool,
    field_ridge: bool,
    ridge_level: i32,
    ridge_width: u32,
    ridge_height: u32,
    harmonics: Vec<HarmonicSlot>,
    field_coast: bool,
    coast_level: u32,
    coast_width: u32,
    coast_height: u32,
    heat_seed: u64,
    cells: u32,
    spots: u32,
    field_heat: bool,
    field_emit: bool,
    radius_tiles: u32,
    arms: u32,
    rings: u32,
    height: u32,

    // ── derived from the recipe ──
    /// The base wave, for its readouts (crests, roll).
    wave: PlanetWave,
    /// The heat field, for the HEAT view and its vents' count.
    seams: SeamField,
    crust: CrustField,
    /// The composed height channel and the sea — `compose(recipe)`.
    heights: Vec<f32>,
    sea: f32,
}

impl WorldBench {
    pub fn new(def: &SceneDef) -> Self {
        let map = HexMap::new(DEFAULT_FREQ);
        tracing::info!("worldbench: {} tiles at freq {}", map.len(), map.freq());
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let world =
            GlobeWorld::new(ui::STAGE_SOURCE, &ui_styles, Some(0.85)).in_panel(ui::VIEW_PANE);
        let tree = def
            .tree
            .clone()
            .expect("worldbench.scene.json declares a tree");
        let ui_intents = UiIntents::of(&tree);
        let script = ScriptHost::new(WORLDBENCH_SCRIPT, "worldbench.lua")
            .expect("worldbench.lua loads (it ships with the crate)");
        let ink = WaveInk {
            trough: rgb(&ui_styles, "wave.trough"),
            mean: rgb(&ui_styles, "wave.mean"),
            crest: rgb(&ui_styles, "wave.crest"),
            sea: rgb(&ui_styles, "wave.sea"),
            sea_alpha: style_number(&ui_styles, "wave.sea_alpha", 0.55),
            sea_gloss: style_number(&ui_styles, "wave.sea_gloss", 0.5),
        };
        let wave_seed = fastrand::u64(..);
        let heat_seed = fastrand::u64(..);
        let wave = PlanetWave::with_lobe(&map, wave_seed, DEFAULT_LOBE);
        let seams = SeamField::new(&map, DEFAULT_CELLS, DEFAULT_SPOTS, heat_seed);
        let crust = CrustField::derive(&map, &seams);
        let harmonics = ui::HARMONICS
            .iter()
            .map(|h| HarmonicSlot {
                seed: fastrand::u64(..),
                ratio_dial: (h.ratio * 100.0).round() as u32,
                axes: DEFAULT_AXES,
                amp: h.amp_range.1,
                mask_seed: MASK_SEED_RANGE.2,
                on: true,
                mask_on: true,
            })
            .collect();
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
            wave_seed,
            sea_percent: SEA_RANGE.2,
            amp_percent: AMP_RANGE.2,
            lobe_dial: (DEFAULT_LOBE * LOBE_DIAL_SCALE).round() as u32,
            plateau_dial: PLATEAU_RANGE.2,
            mask_seed: MASK_SEED_RANGE.2,
            field_wave: true,
            field_mask: true,
            field_ridge: false,
            ridge_level: RIDGE_LEVEL_RANGE.2,
            ridge_width: RIDGE_WIDTH_RANGE.2,
            ridge_height: RIDGE_HEIGHT_RANGE.2,
            harmonics,
            field_coast: true,
            coast_level: COAST_LEVEL_RANGE.2,
            coast_width: COAST_WIDTH_RANGE.2,
            coast_height: COAST_HEIGHT_RANGE.2,
            heat_seed,
            cells: DEFAULT_CELLS,
            spots: DEFAULT_SPOTS,
            field_heat: true,
            field_emit: true,
            radius_tiles: RADIUS_RANGE.2,
            arms: ARMS_RANGE.2,
            rings: RINGS_RANGE.2,
            height: HEIGHT_RANGE.2,
            wave,
            seams,
            crust,
            heights: Vec::new(),
            sea: 0.0,
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
                plateau: self.plateau_dial as f32 * PLATEAU_DIAL_SCALE,
                mask: MaskRecipe {
                    seed: u64::from(self.mask_seed),
                    on: self.field_mask,
                },
                on: self.field_wave,
                ridge: RidgeRecipe {
                    level: self.ridge_level as f32 * HUNDREDTH,
                    width: self.ridge_width as f32 * HUNDREDTH,
                    amp: self.ridge_height as f32 * HALF_PERCENT,
                    on: self.field_ridge,
                },
            },
            harmonics: self
                .harmonics
                .iter()
                .map(|h| HarmonicRecipe {
                    seed: h.seed,
                    ratio: h.ratio_dial as f32 * HUNDREDTH,
                    axes: h.axes,
                    amp: h.amp as f32 * HARMONIC_AMP_SCALE,
                    mask: MaskRecipe {
                        seed: u64::from(h.mask_seed),
                        on: h.mask_on,
                    },
                    on: h.on,
                })
                .collect(),
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
            coast: CoastBandRecipe {
                level: self.coast_level as f32 * THOUSANDTH,
                width: self.coast_width as f32 * THOUSANDTH,
                amp: self.coast_height as f32 * THOUSANDTH,
                on: self.field_coast,
            },
            sea_percent: self.sea_percent as f32,
        }
    }

    /// Compose the recipe into the heights and the sea — the engine's one way.
    fn derive(&mut self) {
        let c = compose(&self.map, &self.recipe());
        self.heights = c.heights;
        self.sea = c.sea;
    }

    fn sel_tab(&self) -> usize {
        let tabs = ui::page(self.sel_page).tabs.len();
        self.page_tabs[self.sel_page.min(self.page_tabs.len() - 1)].min(tabs.saturating_sub(1))
    }

    fn view_key(&self) -> &'static str {
        if self.sel_tab() == ui::TAB_HEAT {
            HEAT_SET
        } else {
            WAVE_SET
        }
    }

    /// Whether a field's tab is ENABLED — the field's switch, a child's
    /// inheriting its parent's.
    pub fn tab_enabled(&self, tab: usize) -> bool {
        match tab {
            ui::TAB_WAVE => self.field_wave,
            ui::TAB_RIDGE => self.field_wave && self.field_ridge,
            ui::TAB_COAST => self.field_coast,
            ui::TAB_HEAT => self.field_heat,
            ui::TAB_EMIT => self.field_heat && self.field_emit,
            t => ui::HARMONICS
                .iter()
                .position(|h| h.tab == t)
                .is_none_or(|i| self.harmonics[i].on),
        }
    }

    /// The root fields' switches: (wave, heat, mask, emitters, ridge, coast) — for tests.
    pub fn fields(&self) -> (bool, bool, bool, bool, bool, bool) {
        (
            self.field_wave,
            self.field_heat,
            self.field_mask,
            self.field_emit,
            self.field_ridge,
            self.field_coast,
        )
    }

    /// A harmonic's (seed, ratio, axes, on, mask on) — for tests.
    pub fn harmonic(&self, i: usize) -> (u64, f32, u32, bool, bool) {
        let h = &self.harmonics[i];
        (
            h.seed,
            h.ratio_dial as f32 * HUNDREDTH,
            h.axes,
            h.on,
            h.mask_on,
        )
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

    /// The standing sea, radius units.
    pub fn sea(&self) -> f32 {
        self.sea
    }

    /// The area-weighted share of the surface above the sea.
    pub fn land(&self) -> f32 {
        land_share(&self.map, |t| self.heights[t as usize], self.sea)
    }

    /// The composed heights — for tests.
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// The dials (sea %, relief %, lobe ×10) — for tests.
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

    /// **Bake both views** off the composed heights.
    fn bake(&mut self) {
        let Self {
            map,
            world,
            heights,
            ink,
            seams,
            ..
        } = self;
        let heights: &[f32] = heights;
        let seams: &SeamField = seams;
        let amp = if self.amp_percent == 0 || !self.field_wave {
            1.0
        } else {
            self.amp_percent as f32 / 100.0
        };
        let sea = self.sea;
        let extruded = || {
            let mut shells = world.authored_shells(&map.grid().dirs, map.outlines());
            for shell in shells.iter_mut() {
                let base = shell.radius;
                shell.cell_radius = Some(Box::new(move |i| base * (1.0 + heights[i])));
            }
            shells
        };
        let mut wave = extruded();
        if let Some(top) = wave.last_mut() {
            let (trough, mean, crest) = (ink.trough, ink.mean, ink.crest);
            top.color = Box::new(move |i| {
                let v = (heights[i] / amp).clamp(-1.0, 1.0);
                Some(if v < 0.0 {
                    lerp3(mean, trough, -v)
                } else {
                    lerp3(mean, crest, v)
                })
            });
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
        world.bake(WAVE_SET, wave);
        world.bake(HEAT_SET, heat);
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
        m.set(ui::TAB_BIND, self.sel_tab() as f64);
        m.set(ui::TABS_SHOWN, !ui::page(self.sel_page).tabs.is_empty());
        // The fields and their tabs.
        m.set(ui::FIELD_WAVE_BIND, self.field_wave);
        m.set(ui::FIELD_MASK_BIND, self.field_mask);
        m.set(ui::FIELD_RIDGE_BIND, self.field_ridge);
        m.set(ui::FIELD_COAST_BIND, self.field_coast);
        m.set(ui::FIELD_HEAT_BIND, self.field_heat);
        m.set(ui::FIELD_EMIT_BIND, self.field_emit);
        m.set(ui::TAB_WAVE_ENABLED, self.tab_enabled(ui::TAB_WAVE));
        m.set(ui::TAB_RIDGE_ENABLED, self.tab_enabled(ui::TAB_RIDGE));
        m.set(ui::TAB_COAST_ENABLED, self.tab_enabled(ui::TAB_COAST));
        m.set(ui::TAB_HEAT_ENABLED, self.tab_enabled(ui::TAB_HEAT));
        m.set(ui::TAB_EMIT_ENABLED, self.tab_enabled(ui::TAB_EMIT));
        // The wave's dials.
        m.set(ui::SEA_BIND, f64::from(self.sea_percent));
        m.set(ui::AMP_BIND, f64::from(self.amp_percent));
        m.set(ui::LOBE_BIND, f64::from(self.lobe_dial));
        m.set(ui::PLATEAU_BIND, f64::from(self.plateau_dial));
        m.set(ui::MASK_SEED_BIND, f64::from(self.mask_seed));
        m.set(ui::RIDGE_LEVEL_BIND, f64::from(self.ridge_level));
        m.set(ui::RIDGE_WIDTH_BIND, f64::from(self.ridge_width));
        m.set(ui::RIDGE_HEIGHT_BIND, f64::from(self.ridge_height));
        m.set(ui::COAST_LEVEL_BIND, f64::from(self.coast_level));
        m.set(ui::COAST_WIDTH_BIND, f64::from(self.coast_width));
        m.set(ui::COAST_HEIGHT_BIND, f64::from(self.coast_height));
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
        // The harmonics'.
        for (h, slot) in ui::HARMONICS.iter().zip(&self.harmonics) {
            m.set(h.field_bind, slot.on);
            m.set(h.mask_bind, slot.mask_on);
            m.set(h.tab_enabled, self.tab_enabled(h.tab));
            m.set(h.amp_bind, f64::from(slot.amp));
            m.set(h.ratio_bind, f64::from(slot.ratio_dial));
            m.set(h.axes_bind, f64::from(slot.axes));
            m.set(h.mask_seed_bind, f64::from(slot.mask_seed));
            m.set(
                h.ratio_val_bind,
                format!("{:.2}", slot.ratio_dial as f32 * HUNDREDTH),
            );
            m.set(h.seed_bind, format!("{:#018x}", slot.seed));
        }
        // The readouts.
        m.set(ui::HEXES_BIND, group_thousands(self.map.len() as u64));
        let land = self.land();
        m.set(ui::LAND_BIND, f64::from(land));
        m.set(ui::LAND_VAL_BIND, format!("{}%", (land * 100.0).round()));
        m.set(
            ui::WATER_VAL_BIND,
            format!("{}%", ((1.0 - land) * 100.0).round()),
        );
        m.set(ui::SEA_VAL_BIND, format!("{:+.2}%", self.sea * 100.0));
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
            (ui::FIELD_RIDGE_BIND, &mut self.field_ridge),
            (ui::FIELD_COAST_BIND, &mut self.field_coast),
            (ui::FIELD_HEAT_BIND, &mut self.field_heat),
            (ui::FIELD_EMIT_BIND, &mut self.field_emit),
        ] {
            if let Some(Value::Bool(v)) = results.get(bind) {
                if *v != *flag {
                    *flag = *v;
                    changed = true;
                }
            }
        }
        // The unsigned dials: (bind, dial, min, max).
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
                ui::PLATEAU_BIND,
                &mut self.plateau_dial,
                PLATEAU_RANGE.0,
                PLATEAU_RANGE.1,
            ),
            (
                ui::MASK_SEED_BIND,
                &mut self.mask_seed,
                MASK_SEED_RANGE.0,
                MASK_SEED_RANGE.1,
            ),
            (
                ui::RIDGE_WIDTH_BIND,
                &mut self.ridge_width,
                RIDGE_WIDTH_RANGE.0,
                RIDGE_WIDTH_RANGE.1,
            ),
            (
                ui::RIDGE_HEIGHT_BIND,
                &mut self.ridge_height,
                RIDGE_HEIGHT_RANGE.0,
                RIDGE_HEIGHT_RANGE.1,
            ),
            (
                ui::COAST_LEVEL_BIND,
                &mut self.coast_level,
                COAST_LEVEL_RANGE.0,
                COAST_LEVEL_RANGE.1,
            ),
            (
                ui::COAST_WIDTH_BIND,
                &mut self.coast_width,
                COAST_WIDTH_RANGE.0,
                COAST_WIDTH_RANGE.1,
            ),
            (
                ui::COAST_HEIGHT_BIND,
                &mut self.coast_height,
                COAST_HEIGHT_RANGE.0,
                COAST_HEIGHT_RANGE.1,
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
        // The one signed dial.
        if let Some(v) = results.number(ui::RIDGE_LEVEL_BIND) {
            let want = (v.round() as i32).clamp(RIDGE_LEVEL_RANGE.0, RIDGE_LEVEL_RANGE.1);
            if want != self.ridge_level {
                self.ridge_level = want;
                changed = true;
            }
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
        // THE HARMONICS.
        for (h, slot) in ui::HARMONICS.iter().zip(&mut self.harmonics) {
            for (bind, flag) in [
                (h.field_bind, &mut slot.on),
                (h.mask_bind, &mut slot.mask_on),
            ] {
                if let Some(Value::Bool(v)) = results.get(bind) {
                    if *v != *flag {
                        *flag = *v;
                        changed = true;
                    }
                }
            }
            for (bind, dial, lo, hi) in [
                (h.amp_bind, &mut slot.amp, 0, h.amp_range.0),
                (
                    h.ratio_bind,
                    &mut slot.ratio_dial,
                    RATIO_RANGE.0,
                    RATIO_RANGE.1,
                ),
                (h.axes_bind, &mut slot.axes, MIN_AXES, MAX_AXES),
                (
                    h.mask_seed_bind,
                    &mut slot.mask_seed,
                    MASK_SEED_RANGE.0,
                    MASK_SEED_RANGE.1,
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
            if results.is_on(h.reroll_action) {
                slot.seed = fastrand::u64(..);
                changed = true;
            }
        }
        self.clamp_tab_to_enabled();
        if changed {
            self.derive();
            self.bake();
        }
        self.world.show(self.view_key());
    }
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
        self.hud_commands = frame.commands;
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&self.tree, &model)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 2] = [&mut walker, &mut self.world];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        let mut results = frame.results;
        for name in walker.take_fired() {
            results.set(name, true);
        }
        drop(walker);
        self.apply_results(&results);
        let dtf = dt.as_secs_f32();
        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        let look_gate = self.ui_state.focused_pane();
        self.world.update(dtf, pointer.as_ref(), look, look_gate);
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
        let Self {
            world,
            hud_commands,
            textures,
            ..
        } = self;
        let layer = fg.base_layer();
        world.render(renderer, fg, layer);
        if let Some(&white) = textures.first() {
            fg.overlay(move |r| render_hud(r, hud_commands, white, textures));
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
    use flicker_worldengine::{metrics, SEA_CREST_PERCENT};

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

    /// Leave only the base WAVE expressed: mask, ridge, coast, emitters and every
    /// harmonic off.
    fn isolate_wave(bench: &mut WorldBench) {
        let mut r = ValueMap::default();
        for k in [
            ui::FIELD_MASK_BIND,
            ui::FIELD_RIDGE_BIND,
            ui::FIELD_COAST_BIND,
            ui::FIELD_EMIT_BIND,
        ] {
            r.set(k, false);
        }
        for h in ui::HARMONICS.iter() {
            r.set(h.field_bind, false);
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
    /// `compose`, IS the heights and the sea the globe draws; and that recipe
    /// round-trips through JSON (what a sweep writes, the bench could load).
    #[test]
    fn the_bench_composes_exactly_what_the_evaluator_composes() {
        let mut bench = test_bench();
        let check = |bench: &WorldBench| {
            let r = bench.recipe();
            let c = compose(bench.map(), &r);
            assert_eq!(c.heights, bench.heights(), "the heights are the recipe's");
            assert_eq!(c.sea, bench.sea(), "the sea is the recipe's");
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
        bench.apply_results(&set(ui::PLATEAU_BIND, Value::Number(5.0)));
        bench.apply_results(&set(ui::FIELD_RIDGE_BIND, Value::Bool(true)));
        bench.apply_results(&set(ui::RIDGE_LEVEL_BIND, Value::Number(-10.0)));
        bench.apply_results(&set(ui::HARMONICS[0].ratio_bind, Value::Number(330.0)));
        bench.apply_results(&set(ui::COAST_HEIGHT_BIND, Value::Number(40.0)));
        bench.apply_results(&set(ui::SEAMS_ACTION, Value::Bool(true)));
        bench.apply_results(&set(ui::RANDOMIZE_ACTION, Value::Bool(true)));
        check(&bench);
        let r = bench.recipe();
        assert!((r.wave.plateau - 0.5).abs() < 1e-6);
        assert!(r.wave.ridge.on && (r.wave.ridge.level + 0.1).abs() < 1e-6);
        assert!((r.harmonics[0].ratio - 3.3).abs() < 1e-6);
        assert!((r.coast.amp - 0.04).abs() < 1e-6);
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
    /// key; the FIELDS tree's switches; every tab's slice gated.
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
                ui::TAB_RIDGE_ENABLED,
                ui::HARMONICS[0].tab_enabled,
                ui::HARMONICS[1].tab_enabled,
                ui::HARMONICS[2].tab_enabled,
                ui::TAB_COAST_ENABLED,
                ui::TAB_HEAT_ENABLED,
                ui::TAB_EMIT_ENABLED
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
            ui::FIELD_RIDGE_BIND,
            ui::FIELD_COAST_BIND,
            ui::FIELD_HEAT_BIND,
            ui::FIELD_EMIT_BIND,
        ] {
            assert!(
                all.iter()
                    .any(|n| n.component == "checkbox" && n.bind.as_deref() == Some(bind)),
                "the `{bind}` switch is present"
            );
        }
    }

    /// **The viewport and the controls get their pixels** on the wave slice.
    #[test]
    fn the_viewport_and_the_dials_resolve_with_extent() {
        let bench = test_bench();
        let tree = bench.build_tree();
        let mut model = bench.model();
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
            ui::PLATEAU_BIND,
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
        // THE DATUM IS THE WAVE'S: stamps, bands and masks move the sea by nothing.
        bench.apply_results(&dial(49.0));
        let sea = bench.sea();
        for k in [
            ui::FIELD_EMIT_BIND,
            ui::FIELD_MASK_BIND,
            ui::FIELD_COAST_BIND,
            ui::FIELD_RIDGE_BIND,
        ] {
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
        assert_eq!(bench.fields(), (true, true, true, true, false, true));
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
        // The ridge is the wave's child: its tab needs both switches.
        assert_eq!(
            bench.model().get(ui::TAB_RIDGE_ENABLED),
            Some(&Value::Bool(false))
        );
        bench.apply_results(&set(ui::FIELD_RIDGE_BIND, Value::Bool(true)));
        assert_eq!(
            bench.model().get(ui::TAB_RIDGE_ENABLED),
            Some(&Value::Bool(true))
        );
        bench.apply_results(&set(ui::FIELD_WAVE_BIND, Value::Bool(false)));
        assert_eq!(
            bench.model().get(ui::TAB_RIDGE_ENABLED),
            Some(&Value::Bool(false))
        );
        bench.apply_results(&set(ui::FIELD_WAVE_BIND, Value::Bool(true)));
        bench.apply_results(&set(ui::FIELD_HEAT_BIND, Value::Bool(true)));
        // Every field off: a flat sphere.
        let before = bench.heights().to_vec();
        assert!(before.iter().any(|h| *h != 0.0));
        for k in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_COAST_BIND,
            ui::FIELD_HEAT_BIND,
        ] {
            bench.apply_results(&set(k, Value::Bool(false)));
        }
        for h in ui::HARMONICS.iter() {
            bench.apply_results(&set(h.field_bind, Value::Bool(false)));
        }
        assert!(
            bench.heights().iter().all(|h| *h == 0.0),
            "every field off is a flat sphere"
        );
        for k in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_COAST_BIND,
            ui::FIELD_HEAT_BIND,
        ] {
            bench.apply_results(&set(k, Value::Bool(true)));
        }
        for h in ui::HARMONICS.iter() {
            bench.apply_results(&set(h.field_bind, Value::Bool(true)));
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
        bench.apply_results(&set(ui::AMP_BIND, Value::Number(8.0)));
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

    /// **The harmonics superpose and each gates its own tab**; the ratio dial
    /// re-tunes one harmonic and no other.
    #[test]
    fn the_harmonics_superpose_and_each_gates_its_own_tab() {
        let mut bench = test_bench();
        for k in [
            ui::FIELD_WAVE_BIND,
            ui::FIELD_EMIT_BIND,
            ui::FIELD_COAST_BIND,
        ] {
            bench.apply_results(&set(k, Value::Bool(false)));
        }
        for h in ui::HARMONICS.iter() {
            bench.apply_results(&set(h.field_bind, Value::Bool(false)));
        }
        assert!(bench.heights().iter().all(|h| *h == 0.0));
        let mut sum = vec![0.0f32; bench.map().len()];
        for h in ui::HARMONICS.iter() {
            bench.apply_results(&set(h.field_bind, Value::Bool(true)));
            let alone = bench.heights().to_vec();
            assert!(alone.iter().any(|v| *v != 0.0));
            bench.apply_results(&set(h.mask_bind, Value::Bool(false)));
            let bare = bench.heights().to_vec();
            assert_ne!(alone, bare, "its mask filters it");
            for (a, b) in alone.iter().zip(&bare) {
                assert!(a.abs() <= b.abs() + 1e-6, "the mask only mutes");
            }
            bench.apply_results(&set(h.mask_bind, Value::Bool(true)));
            for (acc, a) in sum.iter_mut().zip(&alone) {
                *acc += a;
            }
            bench.apply_results(&set(h.field_bind, Value::Bool(false)));
        }
        for h in ui::HARMONICS.iter() {
            bench.apply_results(&set(h.field_bind, Value::Bool(true)));
        }
        for (t, (a, b)) in bench.heights().iter().zip(&sum).enumerate() {
            assert!((a - b).abs() < 1e-5, "tile {t}: the harmonics add");
        }
        let h = &ui::HARMONICS[1];
        bench.apply_results(&set(ui::TAB_BIND, Value::Number(h.tab as f64)));
        assert_eq!(bench.selection(), (0, h.tab));
        bench.apply_results(&set(h.field_bind, Value::Bool(false)));
        assert_eq!(
            bench.selection(),
            (0, ui::HARMONICS[0].tab),
            "off its dimmed tab"
        );
        bench.apply_results(&set(h.field_bind, Value::Bool(true)));
        let (s0, ..) = bench.harmonic(0);
        let (s1, ..) = bench.harmonic(1);
        bench.apply_results(&set(h.reroll_action, Value::Bool(true)));
        assert_ne!(bench.harmonic(1).0, s1, "its re-roll rolls it");
        assert_eq!(bench.harmonic(0).0, s0, "…and no other");
        bench.apply_results(&set(h.ratio_bind, Value::Number(330.0)));
        assert!(
            (bench.harmonic(1).1 - 3.3).abs() < 1e-6,
            "the ratio re-tunes it"
        );
        assert!(
            (bench.harmonic(0).1 - 8.0 / 9.0).abs() < 0.01,
            "…and no other"
        );
    }

    /// **The model publishes every bind the tree reads.**
    #[test]
    fn the_model_publishes_what_the_tree_binds() {
        let bench = test_bench();
        let m = bench.model();
        assert_eq!(m.number(ui::PAGE_BIND), Some(0.0));
        assert!(m.text(ui::HEXES_BIND).is_some_and(|s| s.contains(',')));
        assert!(m.text(ui::CRESTS_BIND).is_some_and(|s| s.starts_with("6 ")));
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
