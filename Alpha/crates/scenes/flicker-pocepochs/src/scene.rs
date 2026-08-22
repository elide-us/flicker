//! The world-gen scene, stripped to **Epoch 1 (seed) + Epoch 2 (molten convection)**.
//!
//! # The scene is a PAIR (five-line architecture)
//!
//! `pocepochs.scene.json` authors the HUD tree + this bench's style blocks;
//! `pocepochs.lua` derives every display string (readout, transport words, axis
//! statuses, the verdict footer) from the RAW model this behaviour publishes; the
//! Rust component kinds draw. The condition-axis rows are REFILLED into the
//! authored `pe_axis_rows` container at construction — the observer's bands are
//! runtime data, and authoring them in the scene file would fork the observer's
//! numbers. The seven raw keyboard verbs became the TRANSPORT row's controls
//! (play/reset/reseed · size − + · view cycle · the Layers slice toggle) plus the
//! declared `on_mode_next`/`on_mode_prev` view-cycle intents; the scene owns no
//! resolver and no bindings — the PUMP hands it resolved signals. The planet
//! itself stays Rust-drawn under the HUD (the shared `GlobeWorld`, handed the
//! whole window), and the surface-view legend + element-distribution panels stay
//! scene-drawn (their per-row swatch colours are DATA — the sanctioned exception).

use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flicker::render::{FrameGraph, Renderer, TextureHandle, Vec2, Vec3};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, Value, ValueMap};
use flicker::ui::{
    render_hud, run_ui, strings, SceneDef, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{AbstractControls, GamepadConfig, InputMap, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_worldengine::{observe, LayerKind, Simulation, MY_PER_TICK};

use flicker_globe::{GlobeWorld, ShellSpec, RADIUS};

use crate::appearance::{self, ViewMode};

/// The pair script — the scene's LOGIC half, by name (five-line architecture).
const PE_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/pocepochs.lua");
/// The shipped scene file — the tests' copy of the authored tree (the runtime
/// receives the same file through the manifest `SceneDef`).
#[cfg(test)]
const PE_SCENE: &str = include_str!("../../../../content/sensorium/scenes/pocepochs.scene.json");

/// The globe's authored stage — `stages.pocepochs_globe`: the light the planet is
/// seen by, the backdrop it sits on, and the fact that its shells come from the
/// simulation rather than the style sheet.
const STAGE_SOURCE: &str = "pocepochs_globe";

/// The condition-axis row's authored heights (the refilled rows' geometry —
/// name/status line · gauge bar · gap · end captions = 42px total).
const AXIS_ROW_H: f32 = 42.0;
const AXIS_BAR_H: f32 = 12.0;

/// Default planet size (grid frequency, ~49.65 mi/hex). ½ Earth — snappy but planet-scale.
const PLANET_FREQ: u32 = 48;
/// Planet-size range + step for the size − / + controls (grid frequency; 96 ≈ full Earth).
const SIZE_MIN: u32 = 12;
const SIZE_MAX: u32 = 96;
const SIZE_STEP: u32 = 6;
/// Sim ticks advanced per second while playing.
const PLAY_TICKS_PER_SEC: f32 = 6.0;

/// A fresh random base seed each launch (the Epoch-1 distribution differs per run); reset
/// returns to tick 0 of this same seed.
fn clock_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The readout's cached census: the cooling clock and which layers have emerged
/// from it across the planet, counted once per tick move.
#[derive(Default)]
struct Census {
    temp_k: f32,
    cells: usize,
    core: usize,
    crust: usize,
    ocean: usize,
    atm: usize,
}

pub struct WorldScene {
    sim: Simulation,
    /// **The planet, whole** — the shell meshes, the authored stage, the offscreen target and
    /// the orbit camera. This bench kept its own copy of every one of those; it keeps none now.
    world: GlobeWorld,
    tick: u64,
    /// Current world seed (Reseed rolls a new one).
    seed: u64,
    /// Current planet size (grid frequency; the size − / + controls change it).
    freq: u32,
    /// The seed's global element distribution: `(atomic number, symbol, percent)`, sorted
    /// descending, only the relevant (non-negligible) elements. Conserved across ticks, so it
    /// is a property of the Epoch-1 seed; recomputed on reseed / resize.
    element_dist: Vec<(u8, String, f32)>,
    /// The cached census the readout composes from — recomputed on refresh (a
    /// tick move), never per frame.
    census: Census,
    theme: Option<Theme>,
    white: Option<TextureHandle>,
    /// Surface colouring (the view control cycles material → heat → layer stack).
    mode: ViewMode,
    /// Whether the layer stack is sliced open (the Layers-view checkbox) — a wedge cut
    /// through the shells.
    cutaway: bool,
    playing: bool,
    play_accum: f32,
    /// The AUTHORED tree off the manifest's def (the five-line split); its
    /// `pe_axis_rows` container is REFILLED at construction. `take`n around the
    /// walk so the walker can borrow it beside the mutable UI state.
    authored: Option<UiNode>,
    /// The pair script (`pocepochs.lua`) — derives every display string from the
    /// raw Model each frame. `None` only if it failed to load.
    script: Option<ScriptHost>,
    /// The screen's declared bindings (S9), read off the authored root ONCE.
    ui_intents: UiIntents,
    /// Token-resolved styles (dotted `style` paths resolve here).
    ui_styles: serde_json::Value,
    /// Retained walker interaction state.
    ui_state: UiState,
    /// Draw commands stashed by `update`'s walker pass, blitted in `render`.
    hud_commands: Vec<HudCommand>,
    /// Intent names fired last frame — republished ONCE into the next Model as
    /// the transient `sig_<name>` mirror (S9 ruling), then dropped.
    fired_sigs: Vec<String>,
}

/// Find the first descendant (or self) with `id`, mutably — the seam the bench refills
/// its axis-row container through (the sablework Rust-fills-the-container pattern;
/// there is no shared helper, so this is the local one).
fn find_by_id_mut<'a>(node: &'a mut UiNode, id: &str) -> Option<&'a mut UiNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter_mut().find_map(|c| find_by_id_mut(c, id))
}

fn node(component: &str) -> UiNode {
    UiNode {
        component: component.to_string(),
        ..Default::default()
    }
}

fn prop(mut n: UiNode, key: &str, value: Value) -> UiNode {
    n.props.insert(key.to_string(), value);
    n
}

fn text_val(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

/// A bound `text` node for a refilled axis row. `color_is_bind` picks the colour
/// channel: a Model key holding a dotted path (`color_bind`) vs a static path
/// (`color`) — stated explicitly so the two can never be confused.
fn bind_text(bind: &str, size: f32, color: &str, color_is_bind: bool) -> UiNode {
    let mut t = node("text");
    t = prop(t, "text_bind", text_val(bind));
    t = prop(t, "text_size", Value::Number(size as f64));
    prop(
        t,
        if color_is_bind { "color_bind" } else { "color" },
        text_val(color),
    )
}

impl WorldScene {
    /// The runtime constructor — the manifest hands in the authored `SceneDef`
    /// (the five-line split): the tree + this bench's style blocks come from
    /// `pocepochs.scene.json`.
    pub fn new(def: &SceneDef) -> Self {
        Self::from_parts(def.tree.clone(), def.styles.clone())
    }

    /// A bench on the SHIPPED scene file — the seam a test drives without an
    /// app, exercising the same authored tree the runtime gets.
    #[cfg(test)]
    pub fn shipped() -> Self {
        let def = SceneDef::parse("pocepochs", PE_SCENE)
            .expect("the shipped pocepochs.scene.json parses");
        Self::from_parts(def.tree, def.styles)
    }

    #[cfg(not(test))]
    pub fn shipped() -> Self {
        // Outside tests the manifest is the only construction path; a def-less
        // bench would be a blank screen, so `Default` routes here loudly.
        unreachable!("WorldScene is built from the manifest's SceneDef")
    }

    fn from_parts(authored: Option<UiNode>, scene_styles_json: Option<serde_json::Value>) -> Self {
        if authored.is_none() {
            tracing::error!("pocepochs: the scene def declares no `tree` — no HUD will draw");
        }
        let seed = clock_seed();
        let sim = Simulation::from_repo_seeded(PLANET_FREQ, seed)
            .expect("tick sim loads from Alpha/content/data");
        // The styles are read HERE, not in `enter`, because the world is built FROM them: a
        // globe's look is authored, and the object that owns the look has to exist before the
        // first frame asks it to draw.
        let ui_styles = flicker::ui::load_shared_styles(scene_styles_json.as_ref());
        let world = GlobeWorld::new(STAGE_SOURCE, &ui_styles, None);
        let ui_intents = authored.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(PE_SCRIPT, "pocepochs_pair.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("pocepochs.lua failed to load — raw HUD values only: {e}");
                None
            }
        };
        let mut scene = Self {
            sim,
            world,
            tick: 0,
            seed,
            freq: PLANET_FREQ,
            element_dist: Vec::new(),
            census: Census::default(),
            theme: None,
            white: None,
            mode: ViewMode::Material, // Epoch 1 = the material cloud
            cutaway: false,
            playing: false, // start paused at the Epoch-1 seed; Play begins Epoch 2
            play_accum: 0.0,
            authored,
            script,
            ui_intents,
            ui_styles,
            ui_state: UiState::new(),
            hud_commands: Vec::new(),
            fired_sigs: Vec::new(),
        };
        scene.element_dist = scene.compute_element_dist();
        scene.refresh();
        // The observer's bands are runtime data: the axis rows refill from them,
        // never from authored literals that could fork the observer's numbers.
        scene.refill_axis_rows();
        scene
    }

    /// Rebuild the condition-axis rows from the observer's bands — at construction
    /// (the bands are constants of the observer, not of the seed). Each row: the
    /// name/status line, the gauge (its band baked from the observer), and the two
    /// end captions — every label riding a bind.
    fn refill_axis_rows(&mut self) {
        self.sim.ensure(0);
        let bands: Vec<(f32, f32)> = self
            .sim
            .world(0)
            .map(|w| observe(w).axes.iter().map(|ax| (ax.lo, ax.hi)).collect())
            .unwrap_or_default();
        let Some(cell) = self
            .authored
            .as_mut()
            .and_then(|t| find_by_id_mut(t, "pe_axis_rows"))
        else {
            return;
        };
        cell.children = bands
            .iter()
            .enumerate()
            .map(|(i, (lo, hi))| {
                let n = i + 1;
                let mut row = node("cell");
                row.size = Some(AXIS_ROW_H);

                let mut head = node("row");
                head.size = Some(17.0);
                let mut name = bind_text(
                    &format!("a{n}_name"),
                    13.0,
                    &format!("a{n}_name_color"),
                    true,
                );
                name.grow = Some(1.0);
                let mut status = bind_text(
                    &format!("a{n}_status"),
                    11.0,
                    &format!("a{n}_status_color"),
                    true,
                );
                status.grow = Some(1.0);
                status = prop(status, "align", text_val("right"));
                head.children = vec![name, status];

                let mut gauge = node("gauge");
                gauge.size = Some(AXIS_BAR_H);
                gauge.bind = Some(format!("a{n}_v"));
                gauge = prop(gauge, "lo", Value::Number(f64::from(*lo)));
                gauge = prop(gauge, "hi", Value::Number(f64::from(*hi)));
                gauge = prop(gauge, "style", text_val("pocepochs.hab.gauge"));

                let mut gap = node("stack");
                gap.size = Some(2.0);

                let mut foot = node("row");
                foot.size = Some(11.0);
                let mut lolab = bind_text(
                    &format!("a{n}_lolab"),
                    10.0,
                    "pocepochs.hab.caption.color",
                    false,
                );
                lolab.grow = Some(1.0);
                let mut hilab = bind_text(
                    &format!("a{n}_hilab"),
                    10.0,
                    "pocepochs.hab.caption.color",
                    false,
                );
                hilab.grow = Some(1.0);
                hilab = prop(hilab, "align", text_val("right"));
                foot.children = vec![lolab, hilab];

                row.children = vec![head, gauge, gap, foot];
                row
            })
            .collect();
    }

    /// Rebuild the sim at the current seed + size, back to the Epoch-1 seed, paused.
    fn rebuild(&mut self) {
        self.sim = Simulation::from_repo_seeded(self.freq, self.seed).expect("rebuild tick sim");
        self.tick = 0;
        self.playing = false;
        self.play_accum = 0.0;
        self.element_dist = self.compute_element_dist();
        self.refresh();
    }

    /// Reseed Epoch 1: roll a new random seed → a fresh planet (same size).
    fn reseed(&mut self) {
        self.seed = clock_seed();
        self.rebuild();
    }

    /// Grow / shrink the planet by `delta` steps of grid frequency (same seed).
    fn resize(&mut self, delta: i32) {
        let f =
            (self.freq as i32 + delta * SIZE_STEP as i32).clamp(SIZE_MIN as i32, SIZE_MAX as i32);
        if f as u32 != self.freq {
            self.freq = f as u32;
            self.rebuild();
        }
    }

    /// The seed's global element distribution — `(number, symbol, percent)` for the relevant
    /// (≥ 0.5 %) elements, sorted descending. Summed over the Epoch-1 seed cells; convection
    /// conserves element mass, so this is stable across ticks (an Epoch-1 property).
    fn compute_element_dist(&mut self) -> Vec<(u8, String, f32)> {
        self.sim.ensure(0);
        let cells = &self.sim.world(0).expect("tick-0 seed").cells;
        let tables = self.sim.tables();
        let mut totals: std::collections::BTreeMap<u8, f64> = std::collections::BTreeMap::new();
        for c in cells {
            for (el, amt) in c.composition.iter() {
                *totals.entry(el).or_insert(0.0) += amt;
            }
        }
        let total: f64 = totals.values().sum();
        if total <= 0.0 {
            return Vec::new();
        }
        let mut list: Vec<(u8, String, f32)> = totals
            .into_iter()
            .map(|(el, amt)| {
                let sym = tables
                    .element_by_number(el)
                    .map(|e| e.symbol.clone())
                    .unwrap_or_else(|| el.to_string());
                (el, sym, (amt / total * 100.0) as f32)
            })
            .filter(|(_, _, pct)| *pct >= 0.5) // the relevant (non-defaulted) elements
            .collect();
        list.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        list
    }

    /// Move the shown tick (re-simming + refreshing if it changed).
    fn go_to_tick(&mut self, tick: u64) {
        if tick != self.tick {
            self.tick = tick;
            self.refresh();
        }
    }

    /// Ensure the current tick is computed, cache the census, flag the mesh for rebuild.
    fn refresh(&mut self) {
        self.sim.ensure(self.tick);
        // The readout is the condition state, not an epoch number: the cooling clock `T` and
        // which layers have emerged from it across the planet. Counted HERE (a tick move),
        // never per frame; the pair script composes the display line from these raws.
        self.census = {
            let w = self.sim.world(self.tick).expect("ensured this tick");
            let count = |k: LayerKind| {
                w.cells
                    .iter()
                    .filter(|c| c.column.find(k).is_some())
                    .count()
            };
            Census {
                temp_k: w.temp,
                cells: w.cells.len(),
                core: count(LayerKind::Core),
                crust: count(LayerKind::Crust),
                ocean: count(LayerKind::Ocean),
                atm: count(LayerKind::Atmosphere),
            }
        };
        self.publish_shells();
    }

    /// Publish what the planet is made of, for the current tick and view — the ONE hand-off
    /// from the simulation to the world. Called wherever the picture's inputs move; there is
    /// no stale flag, because the world is told the moment its data changes.
    fn publish_shells(&mut self) {
        self.sim.ensure(self.tick);
        let Self {
            sim,
            world,
            tick,
            mode,
            cutaway,
            ..
        } = self;
        world.set_shells(shells_for(sim, *tick, *mode, *cutaway));
    }

    /// The per-frame RAW model: the sim clock + census numbers, the transport
    /// state, the observer's per-axis reading (signal + live/in-band booleans +
    /// resolved metadata words), the aggregate verdict inputs, and the resolved
    /// WORD variables the pair script composes with (localization stays
    /// stringtable-resolved engine-side). Pure read; the observer encodes no
    /// causal rule. Presentation strings belong to `pocepochs.lua`'s `derive()`.
    fn hud_model(&self) -> ValueMap {
        let r = |t: &str| strings::resolve(t).into_owned();
        let mut m = ValueMap::new();

        // ── the readout raws ──
        m.set("tick", self.tick as f64);
        m.set("my", f64::from(self.tick as f32 * MY_PER_TICK));
        m.set("temp_k", f64::from(self.census.temp_k));
        m.set("cells_n", self.census.cells as f64);
        m.set("core_n", self.census.core as f64);
        m.set("crust_n", self.census.crust as f64);
        m.set("ocean_n", self.census.ocean as f64);
        m.set("atm_n", self.census.atm as f64);
        m.set("playing", self.playing);
        // The view cursor (a NUMBER — 1B64FF03) + the two-way slice toggle.
        m.set(
            "view",
            match self.mode {
                ViewMode::Material => 0.0,
                ViewMode::Heat => 1.0,
                ViewMode::Layers => 2.0,
            },
        );
        m.set("cut", self.cutaway);
        m.set("freq", f64::from(self.freq));

        // Resolved WORDS the pair script composes with (never raw English).
        m.set("w_tick", r("$pe_tick"));
        m.set("w_cells", r("$pe_cells"));
        m.set("w_core", r("$pe_core"));
        m.set("w_crust", r("$pe_crust"));
        m.set("w_ocean", r("$pe_ocean"));
        m.set("w_atm", r("$pe_atm"));
        m.set("w_playing", r("$pe_playing"));
        m.set("w_paused", r("$pe_paused"));
        m.set("w_play", r("$pe_play"));
        m.set("w_pause", r("$pe_pause"));
        m.set("w_view", r("$pe_view"));
        m.set("w_size", r("$pe_size"));
        m.set("w_view_0", r("$pe_view_material"));
        m.set("w_view_1", r("$pe_view_heat"));
        m.set("w_view_2", r("$pe_view_layers"));
        m.set("w_in_band", r("$pe_in_band"));
        m.set("w_out_of_band", r("$pe_out_of_band"));
        m.set("w_no_signal", r("$pe_no_signal"));
        m.set("w_life_supporting", r("$pe_life_supporting"));
        m.set("w_axes_in_band", r("$pe_axes_in_band"));
        m.set("w_observed", r("$pe_observed"));
        m.set("w_air", r("$pe_air"));

        // ── life-supporting conditions (the observer's reading, raw) ──
        if let Some(w) = self.sim.world(self.tick) {
            let h = observe(w);
            for (i, ax) in h.axes.iter().enumerate() {
                let n = i + 1;
                m.set(format!("a{n}_v"), f64::from(ax.signal.unwrap_or(-1.0))); // −1 = no signal yet
                m.set(format!("a{n}_live"), ax.signal.is_some());
                m.set(format!("a{n}_in_band"), ax.in_band());
                // The observer's display metadata ships as `$token`s — resolved here,
                // the bench's publish site.
                m.set(format!("a{n}_name"), r(ax.name));
                m.set(format!("a{n}_lolab"), r(ax.low_label));
                m.set(format!("a{n}_hilab"), r(ax.high_label));
            }
            m.set("axes_total", h.axes.len() as f64);
            m.set("axes_live", h.axes_live as f64);
            m.set("axes_in_band", h.axes_in_band as f64);
            m.set("life", h.life_supporting);
            m.set("no_life", !h.life_supporting);
            m.set("air_kind", r(h.atmosphere_kind));
        }
        m
    }

    /// The frame's full model: the raw variables plus the pair script's derived
    /// display strings folded over them, and the transient `sig_<name>` mirror
    /// (S9) riding the same ONE publish.
    fn model(&mut self) -> ValueMap {
        let raw = self.hud_model();
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("pocepochs: publishing the model to the script failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => m.extend(derived),
                Ok(None) => {}
                Err(e) => tracing::error!("pocepochs.lua derive() failed: {e}"),
            }
        }
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    /// Advance the convection run while playing ([`PLAY_TICKS_PER_SEC`] ticks/sec).
    fn advance_play(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        self.play_accum += dt * PLAY_TICKS_PER_SEC;
        if self.play_accum < 1.0 {
            return;
        }
        let steps = self.play_accum.floor() as u64;
        self.play_accum -= steps as f32;
        self.go_to_tick(self.tick + steps);
    }
}

/// The shell list this bench publishes for a tick, a view and the cutaway.
///
/// Material and Heat are ONE surface shell coloured per cell. The layer stack is one shell per
/// layer kind, each standing at that column's OWN accumulated height — the shared builder
/// answering a per-cell radius instead of a sphere's ([`ShellSpec::cell_radius`]), which is the
/// only thing this bench's private globe module ever did differently. The cutaway drops a
/// longitude wedge from everything above the mantle so the stack shows in section.
///
/// A free function, and inspectable by a gate: what the world is handed is the whole of what
/// the bench decides about the picture, and it can be asserted without a GPU.
fn shells_for(sim: &Simulation, tick: u64, mode: ViewMode, cut: bool) -> Vec<ShellSpec<'_>> {
    let Some(w) = sim.world(tick) else {
        return Vec::new();
    };
    let cells = &w.cells;
    let dirs = &sim.sphere().dirs;
    let outlines = sim.outlines();
    // One surface sphere over the whole tiling — the Material / Heat reads.
    fn sphere<'a>(
        dirs: &'a [Vec3],
        outlines: &'a [Vec<Vec3>],
        color: Box<dyn Fn(usize) -> Option<[f32; 3]> + 'a>,
    ) -> ShellSpec<'a> {
        ShellSpec {
            dirs,
            outlines,
            radius: RADIUS,
            inset: 0.0,
            color,
            cell_radius: None,
        }
    }
    match mode {
        ViewMode::Material => {
            vec![sphere(
                dirs,
                outlines,
                Box::new(|i| Some(appearance::material_color(&cells[i]))),
            )]
        }
        ViewMode::Heat => {
            vec![sphere(
                dirs,
                outlines,
                Box::new(|i| Some(appearance::cell_heat_color(&cells[i]))),
            )]
        }
        ViewMode::Layers => {
            // Each cell's layers are CLASSIFIED (composition + temp + pressure → what they ARE)
            // and stacked OUTWARD at their PHYSICAL thickness (volume = mass ÷ density). The
            // mantle IS the base ball (core = data inside it, never a nested sphere).
            let tables = sim.tables();
            let stacks: Vec<Vec<appearance::StackLayer>> = cells
                .iter()
                .map(|c| appearance::cell_stack(c, tables))
                .collect();
            [
                LayerKind::Mantle,
                LayerKind::Crust,
                LayerKind::Ocean,
                LayerKind::Atmosphere,
            ]
            .into_iter()
            .map(|kind| {
                // ONE resolved row per cell — its drawn top and its ink — so the radius
                // answer and the colour answer cannot disagree about a column.
                let rows: Rc<Vec<(f32, Option<[f32; 3]>)>> = Rc::new(
                    stacks
                        .iter()
                        .enumerate()
                        .map(|(i, s)| {
                            let hit = s.iter().find(|l| l.kind == kind);
                            let sliced = cut
                                && kind != LayerKind::Mantle
                                && flicker_globe::in_wedge(dirs[i]);
                            let ink = if sliced { None } else { hit.map(|l| l.color) };
                            (hit.map_or(RADIUS, |l| l.outer_r), ink)
                        })
                        .collect(),
                );
                let radii = Rc::clone(&rows);
                ShellSpec {
                    dirs,
                    outlines,
                    radius: RADIUS,
                    inset: 0.0,
                    color: Box::new(move |i| rows[i].1),
                    cell_radius: Some(Box::new(move |i| radii[i].0)),
                }
            })
            .collect()
        }
    }
}

impl Default for WorldScene {
    fn default() -> Self {
        Self::shipped()
    }
}

impl Scene for WorldScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0]; // deep space
        let theme = Theme::build(renderer);
        self.white = Some(theme.lua_textures()[0].1); // id 0 = "white"
        self.theme = Some(theme);
        // The tree, styles, pair script and the refilled axis rows were all built
        // in `from_parts` — the five-line split leaves `enter` with the GPU only.
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
        let Some(tree) = self.authored.take() else {
            return Transition::None;
        };
        // The scene is DATA: walk the AUTHORED tree (its axis rows refilled at
        // construction) with the raw model + the pair script's derived strings.
        let model = self.model();
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen: renderer.size(),
            typed: String::new(),
            backspace: false,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        let over_hud = frame.results.is_on("hud_hit");
        // The planet is the ROOT surface: its pointer sample is the walker's root pointer —
        // present only when no UI claims the cursor, so a drag on the HUD never flies the
        // planet (the barrier, A8C9F02B §4b).
        let pointer = frame.root_pointer().cloned();
        let mut results = frame.results.clone();
        self.hud_commands = frame.commands;

        // ── The input seam (input-P3): the PUMP resolved this frame's events — the
        // scene owns no Resolver. One dispatch through the walker, which owns the
        // focus graph, consumes the pointer while it is over the HUD, and fires the
        // screen's DECLARED intents (`on_menu` / `on_mode_*`) as result names. ──
        let mut walker = WalkerHandler::hud(&mut self.ui_state, over_hud)
            .with_nav(&tree, &model)
            .with_rects(&frame.rects)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }
        self.fired_sigs = walker.take_fired();
        self.authored = Some(tree);
        for name in &self.fired_sigs {
            results.set(name.clone(), true);
        }

        // ── The ONE dispatch: a click and a pad press arrive here identically.
        // The seven raw keyboard verbs died with the migration — these are their
        // controls (KBM reach is the pointer; pad reach is the nav ring + the
        // declared `on_mode_*` view cycle). ──
        if results.is_on("toggle_play") {
            self.playing = !self.playing;
            self.play_accum = 0.0;
        }
        if results.is_on("reset") {
            self.playing = false;
            self.go_to_tick(0);
        }
        if results.is_on("reseed") {
            self.reseed();
        }
        if results.is_on("size_down") {
            self.resize(-1);
        }
        if results.is_on("size_up") {
            self.resize(1);
        }
        let cycled = i32::from(results.is_on("view_next")) - i32::from(results.is_on("view_prev"));
        if cycled != 0 {
            self.mode = if cycled > 0 {
                appearance::cycle_view(self.mode)
            } else {
                appearance::cycle_view_back(self.mode)
            };
            self.publish_shells();
        }
        // The slice toggle is a two-way checkbox that only EXISTS on the Layers
        // view (its row gates on `layers_view`), so the read is gated the same
        // way — off that view `is_on` reads false and would clear it.
        if self.mode == ViewMode::Layers {
            let cut = results.is_on("cut");
            if cut != self.cutaway {
                self.cutaway = cut;
                self.publish_shells();
            }
        }

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer
        // consumed the Menu press and fired the name; the ONE dispatch maps it
        // onto the shell pause overlay. The overlay shows the PROFILE's map —
        // the pump owns bindings now (input-P3), the scene holds none.
        if results.is_on("pause_open") {
            let theme = self.theme.expect("theme built in enter");
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

        // The planet is the ROOT surface — the whole window — and the root is ENTERED by
        // default (there is no pane to lock into): the stick look/zoom flow whenever no
        // pane is entered, and the pointer reaches the camera only through the walker's
        // root sample. Drawing goes through the frame graph's root pass, not a target.
        let look = GlobeWorld::look_from(|s| signals.axis(s, input));
        self.world.update(
            dt.as_secs_f32(),
            pointer.as_ref(),
            look,
            self.ui_state.entered_group(),
        );

        self.advance_play(dt.as_secs_f32());
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let base = renderer.layer();
        // The planet is the ROOT surface's element: declared as the frame graph's root
        // pass, straight into the swapchain — no full-window target, no blit. `execute`
        // orders it after any offscreen pass, so the shared draw queues are never reset
        // under it. Everything the planet needs — camera, stage, meshes — is inside it.
        {
            let mut fg = FrameGraph::new();
            self.world.render_root(renderer, &mut fg);
            fg.execute(renderer);
        }

        // ── The HUD: the walker commands stashed by `update` (readout text +
        // the life-supporting gauge panel), then the two panels still
        // scene-drawn (FLAGGED, S10): the surface-view legend and the
        // element-distribution readout — their per-row swatch colours are DATA
        // (legend entries / element_rgb), and the walker's colour channel is
        // dotted style paths by design. One layer above the root element, RELATIVE to
        // the scene's band — never an absolute layer. ──
        renderer.set_layer(base + 1.0);
        if let Some(white) = self.white {
            render_hud(renderer, &self.hud_commands, white, &[]);
        }
        let gold = [0.722, 0.592, 0.353, 1.0]; // Prism bronze (structural accent)
        let text = [0.85, 0.87, 0.92, 1.0];

        // Surface-view legend (top-right) — display only, no controls.
        if let Some(white) = self.white {
            let entries = appearance::legend_entries(self.mode);
            let (pad, sw, row_h, panel_w) = (10.0f32, 14.0f32, 20.0f32, 210.0f32);
            let panel_h = pad + 22.0 + entries.len() as f32 * row_h + pad;
            let px = renderer.size().x - panel_w - 16.0;
            let py = 20.0;
            renderer.draw_sprite(
                white,
                Vec2::new(px, py),
                Vec2::new(panel_w, panel_h),
                [0.05, 0.06, 0.08, 0.85],
            );
            let title = strings::resolve(self.mode.label()).to_uppercase();
            renderer.draw_text(&title, Vec2::new(px + pad, py + pad), 14.0, gold);
            let mut ry = py + pad + 24.0;
            for (label, c) in &entries {
                renderer.draw_sprite(
                    white,
                    Vec2::new(px + pad, ry + 2.0),
                    Vec2::new(sw, sw),
                    [c[0], c[1], c[2], 1.0],
                );
                renderer.draw_text(
                    &strings::resolve(label),
                    Vec2::new(px + pad + sw + 8.0, ry),
                    13.0,
                    text,
                );
                ry += row_h;
            }
        }

        // Element-distribution readout (left) — the Epoch-1 seed composition (relevant,
        // non-default elements), each with its material swatch + share.
        if let Some(white) = self.white {
            if !self.element_dist.is_empty() {
                let (pad, sw, row_h, panel_w) = (10.0f32, 12.0f32, 18.0f32, 200.0f32);
                let panel_h = pad + 22.0 + self.element_dist.len() as f32 * row_h + pad;
                let (px, py) = (16.0f32, 120.0f32);
                renderer.draw_sprite(
                    white,
                    Vec2::new(px, py),
                    Vec2::new(panel_w, panel_h),
                    [0.05, 0.06, 0.08, 0.85],
                );
                renderer.draw_text(
                    &strings::resolve("$pe_element_distribution"),
                    Vec2::new(px + pad, py + pad),
                    14.0,
                    gold,
                );
                let mut ry = py + pad + 24.0;
                for (num, sym, pct) in &self.element_dist {
                    let c = appearance::element_rgb(*num);
                    renderer.draw_sprite(
                        white,
                        Vec2::new(px + pad, ry + 2.0),
                        Vec2::new(sw, sw),
                        [c[0], c[1], c[2], 1.0],
                    );
                    renderer.draw_text(
                        &format!("{sym}   {pct:.1}%"),
                        Vec2::new(px + pad + sw + 8.0, ry),
                        13.0,
                        text,
                    );
                    ry += row_h;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Load the shipped stringtable (en-us) into the process-wide table, so tests
    /// asserting composed copy read FINAL text.
    fn load_shipped_strings() {
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");
    }

    #[test]
    fn plays_epoch2_convection_and_resets_to_epoch1() {
        let mut scene = WorldScene::shipped();
        assert!(!scene.playing, "starts paused at the Epoch-1 seed");
        assert_eq!(scene.tick, 0);

        // Play advances the tick — Epoch-2 convection runs.
        scene.playing = true;
        scene.advance_play(5.0);
        assert!(scene.tick > 0, "playback advanced the convection sim");

        // Reset returns to the Epoch-1 seed.
        scene.go_to_tick(0);
        assert_eq!(scene.tick, 0, "reset to Epoch 1");
    }

    /// The shipped scene file IS the bench: it parses, names this behaviour,
    /// declares the pause + view-cycle intents, and carries the axis-row refill
    /// container and every transport control.
    #[test]
    fn the_shipped_scene_file_authors_the_bench() {
        use flicker_input_core::ActionSignal;
        let def = SceneDef::parse("pocepochs", PE_SCENE).expect("scene file parses");
        assert_eq!(def.behaviour, "pocepochs");
        let tree = def.tree.expect("the scene file carries the HUD tree");
        let intents = UiIntents::of(&tree);
        assert_eq!(intents.result_for(ActionSignal::Menu), Some("pause_open"));
        let mut t = tree.clone();
        assert!(
            find_by_id_mut(&mut t, "pe_axis_rows").is_some(),
            "the axis-row refill container is authored"
        );
        fn ids(n: &UiNode, out: &mut Vec<String>) {
            if !n.id.is_empty() {
                out.push(n.id.clone());
            }
            for c in &n.children {
                ids(c, out);
            }
        }
        let mut all = Vec::new();
        ids(&tree, &mut all);
        for b in [
            "pe_play",
            "pe_reset",
            "pe_reseed",
            "pe_size_down",
            "pe_size_up",
            "pe_view",
            "pe_cut",
        ] {
            assert!(all.iter().any(|i| i == b), "{b} authored");
        }
    }

    /// THE PAIR-SCRIPT + REFILL GATE: build the bench exactly as the resolver does
    /// (real def, real pocepochs.lua) and run the REAL model path — the raw
    /// numbers must come back as the DERIVED display strings the tree binds, the
    /// refilled axis rows must carry the observer's real bands, and the walked
    /// surface must draw the readout, the transport, and the gauges.
    #[test]
    fn hud_tree_is_well_formed_and_draws_from_the_observer_model() {
        load_shipped_strings();
        let def = SceneDef::parse("pocepochs", PE_SCENE).expect("scene file parses");
        let tree = def.tree.clone().expect("scene defines a tree");
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "pocepochs.scene.json names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "pocepochs.scene.json ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The MODEL-CHANNEL strings gate (S10's blind side).
        let flags = strings::raw_model_publish_literals(include_str!("scene.rs"));
        assert!(
            flags.is_empty(),
            "raw display copy published into the Model: {flags:?}"
        );

        let mut scene = WorldScene::shipped();
        assert!(
            scene.script.is_some(),
            "pocepochs.lua loads (the pair script)"
        );

        // The refilled axis rows carry the observer's REAL bands, one row per axis.
        scene.sim.ensure(0);
        let bands: Vec<(f32, f32)> = scene
            .sim
            .world(0)
            .map(|w| observe(w).axes.iter().map(|ax| (ax.lo, ax.hi)).collect())
            .unwrap_or_default();
        assert!(!bands.is_empty(), "the observer exposes the condition axes");
        {
            let rows = find_by_id_mut(scene.authored.as_mut().unwrap(), "pe_axis_rows")
                .expect("refill container present");
            assert_eq!(
                rows.children.len(),
                bands.len(),
                "one refilled row per axis"
            );
            for (row, (lo, hi)) in rows.children.iter().zip(&bands) {
                let gauge = row
                    .children
                    .iter()
                    .find(|c| c.component == "gauge")
                    .expect("each axis row carries its gauge");
                assert_eq!(
                    gauge.props.get("lo"),
                    Some(&Value::Number(f64::from(*lo))),
                    "the gauge's band rides the OBSERVER's numbers"
                );
                assert_eq!(gauge.props.get("hi"), Some(&Value::Number(f64::from(*hi))));
            }
        }

        // The pair script derives the display strings over the raw publish.
        let m = scene.model();
        for key in [
            "stats_val",
            "play_state",
            "view_line",
            "verdict",
            "observed",
            "air",
        ] {
            assert!(
                m.text(key).is_some(),
                "derive() must yield display TEXT for '{key}'"
            );
        }
        assert_eq!(m.text("play_state"), Some("PAUSED"));

        // Walk the real (refilled) tree with the real model.
        let tree = scene.authored.clone().expect("authored tree held");
        let snap = UiInput {
            mouse: Vec2::new(-9.0, -9.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &m, &scene.ui_styles, &snap, &mut UiState::new());
        let has = |s: &str| {
            frame
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text == s))
        };
        assert!(has("FLICKER · PLANET SIMULATION"), "readout title renders");
        assert!(has("PAUSED"), "the state word rides its bind");
        assert!(
            has("LIFE-SUPPORTING CONDITIONS"),
            "habitability panel renders"
        );
        assert!(has("Reset"), "the transport buttons render");
        assert!(
            frame
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Rect { .. })),
            "the gauges emitted their bars"
        );
    }

    /// The declared pause intent through the scene's real chain (the re-pointed
    /// half of the retired route.rs tests).
    #[test]
    fn the_declared_pause_intent_fires_through_the_authored_tree() {
        use flicker_input_core::{ActionSignal, EventKind, InputContext};
        use flicker_input_router::{InputEvent, RouteCtx};

        let def = SceneDef::parse("pocepochs", PE_SCENE).expect("scene file parses");
        let tree = def.tree.expect("scene defines a tree");
        let intents = UiIntents::of(&tree);

        let raw = InputState::new();
        let events = [InputEvent::new(
            ActionSignal::Menu,
            EventKind::Press,
            InputContext::World,
            &raw,
        )];
        let mut ui = UiState::new();
        let mut walker = WalkerHandler::hud(&mut ui, false).with_intents(&intents);
        let mut rc = RouteCtx::new();
        let report = {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut rc)
        };
        assert!(
            report.consumed_by(0, ActionSignal::Menu),
            "the walker layer consumed the declared Menu"
        );
        assert_eq!(walker.take_fired(), vec!["pause_open".to_string()]);
    }

    #[test]
    fn epoch1_element_distribution_is_populated_and_sensible() {
        let scene = WorldScene::shipped();
        let dist = &scene.element_dist;
        assert!(
            !dist.is_empty(),
            "the Epoch-1 element distribution readout is empty"
        );
        // Sorted descending, every share is a real percentage, and the total is bounded.
        for w in dist.windows(2) {
            assert!(w[0].2 >= w[1].2, "distribution not sorted descending");
        }
        let sum: f32 = dist.iter().map(|(_, _, p)| p).sum();
        assert!(
            sum > 50.0 && sum <= 100.5,
            "shares should cover most of the planet, got {sum:.1}%"
        );
    }

    /// **The planet is the SHARED world, and the stack rides a per-cell radius.**
    ///
    /// This bench carried its own `globe.rs` (a second `RADIUS`, a second `in_wedge`, a second
    /// shell builder) and its own `camera.rs` (a second `OrbitCam`) until they were folded back
    /// into `flicker-globe`. The one thing that copy did differently was ask for the radius PER
    /// COLUMN, so the layer stack could stand at each cell's own accumulated height — absorbed
    /// as [`ShellSpec::cell_radius`]. This is the named home of that migration's proof:
    ///
    /// * the stage the bench names resolves (an authored name that resolves to nothing is the
    ///   failure rule 4BB12A75 exists for),
    /// * a surface read is ONE sphere — no per-cell radius, because there is nothing to stack,
    /// * the layer stack is one shell per kind, every one of them answering per column, with
    ///   the mantle at the base ball and the beds above standing proud of it,
    /// * the cutaway takes the wedge out of everything above the mantle and nothing else, and
    /// * the geometry budget is unchanged: the per-column framing emits exactly the vertices
    ///   and triangles the sphere framing does, which is what "the picture did not change"
    ///   means when it cannot be verified by eye.
    #[test]
    fn the_planet_is_the_shared_world_and_the_stack_rides_a_per_cell_radius() {
        use flicker_globe::{build, StageLayer};

        let scene = WorldScene::shipped();
        assert_eq!(
            scene.world.stage().layers,
            vec![StageLayer::Shells],
            "`stages.{STAGE_SOURCE}` is authored, and it says the SIMULATION publishes the shells"
        );

        let sim = &scene.sim;
        for mode in [ViewMode::Material, ViewMode::Heat] {
            let shells = shells_for(sim, 0, mode, false);
            assert_eq!(shells.len(), 1, "a surface read is one shell");
            assert!(
                shells[0].cell_radius.is_none(),
                "and a sphere — nothing to stack"
            );
            assert_eq!(shells[0].radius, RADIUS);
            assert!(
                (shells[0].color)(0).is_some(),
                "every cell is coloured; no holes"
            );
        }

        // A cell that actually carries a mantle and something above it — the stack is emergent,
        // so the gate finds one rather than assuming cell 0 has one.
        let cells = &sim.world(0).expect("tick 0").cells;
        let tables = sim.tables();
        let stacked: Vec<Vec<appearance::StackLayer>> = cells
            .iter()
            .map(|c| appearance::cell_stack(c, tables))
            .collect();
        let base = appearance::R_BASE;

        let shells = shells_for(sim, 0, ViewMode::Layers, false);
        assert_eq!(shells.len(), 4, "one shell per drawn layer kind");
        for s in &shells {
            assert!(
                s.cell_radius.is_some(),
                "every stack shell answers per COLUMN"
            );
        }
        let mantle = &shells[0];
        let mantle_cell = stacked
            .iter()
            .position(|s| s.iter().any(|l| l.kind == LayerKind::Mantle))
            .expect("the tick-0 seed has a mantle somewhere");
        let r = (mantle.cell_radius.as_ref().unwrap())(mantle_cell);
        assert!(
            (r - base).abs() < 1e-3,
            "the mantle IS the base ball at {base}, got {r}"
        );
        assert!(
            (mantle.color)(mantle_cell).is_some(),
            "and it is drawn there"
        );
        // Anything above the mantle stands proud of it, at its own physical thickness.
        for (i, stack) in stacked.iter().enumerate() {
            if let Some(top) = stack.iter().find(|l| l.kind != LayerKind::Mantle) {
                assert!(
                    top.outer_r > base,
                    "cell {i}: {:?} sits above the ball",
                    top.kind
                );
                break;
            }
        }

        // The cutaway: a wedge cell loses every layer ABOVE the mantle, and only those.
        let cut = shells_for(sim, 0, ViewMode::Layers, true);
        let dirs = &sim.sphere().dirs;
        let wedge = (0..cells.len())
            .find(|&i| flicker_globe::in_wedge(dirs[i]) && !stacked[i].is_empty())
            .expect("some cell lies in the cutaway wedge");
        assert_eq!(
            (cut[0].color)(wedge),
            (shells[0].color)(wedge),
            "the innermost shell is never cut — there is nothing beneath it to reveal"
        );
        for s in &cut[1..] {
            assert_eq!((s.color)(wedge), None, "the wedge is open above the mantle");
        }

        // …and the absorbed framing costs nothing: same vertices, same triangles as the sphere
        // it replaced. (The builder-tier twin of this lives in `flicker-globe`.)
        let outlines = sim.outlines();
        let per_column = build(
            dirs,
            outlines,
            |i| (mantle.cell_radius.as_ref().unwrap())(i),
            0.0,
            |i| (mantle.color)(i),
        );
        let sphere = build(dirs, outlines, |_| base, 0.0, |i| (mantle.color)(i));
        assert_eq!(
            per_column.0.len(),
            sphere.0.len(),
            "same vertex count as before the absorption"
        );
        assert_eq!(per_column.1, sphere.1, "same triangles, same winding");
    }

    #[test]
    fn resize_changes_the_planet_and_reseed_changes_the_composition() {
        let mut scene = WorldScene::shipped();
        let f0 = scene.freq;
        scene.resize(-1); // shrink
        assert!(scene.freq < f0, "resize should shrink the planet");
        assert_eq!(scene.tick, 0, "resize returns to the Epoch-1 seed");
        let seed0 = scene.seed;
        scene.reseed();
        assert_ne!(scene.seed, seed0, "reseed rolls a new seed");
        assert_eq!(
            scene.freq,
            f0 - SIZE_STEP,
            "reseed keeps the current planet size"
        );
    }
}
