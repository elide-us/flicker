//! Scenes: the app flow is `Loading → World`.
//!
//! `Loading` shows a title for a frame, then generates the planet and replaces
//! itself with `World`. `World` owns the generated planet (all epoch layers), an
//! orbit camera, the selected epoch + view field, and the tunable [`WorldParams`].
//! It drives a Lua HUD (`scripts/world_ui.lua` + `ui_elements.json`): stats, a
//! field/grid/epoch control row, and a per-epoch knob panel. Dragging a knob
//! updates the slider live; the planet regenerates on release. Keyboard
//! shortcuts mirror the panel.

use std::time::Duration;

use flicker::app::{AbstractControls, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, TextureHandle, Vec2,
};
use flicker::scene::{Scene, Transition};
use flicker::script::{ScriptHost, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};
use flicker_materials::Tables;
use flicker_shell::{PauseScene, Theme};

use crate::camera::OrbitCam;
use crate::celestial::CelestialState;
use crate::color::ViewMode;
use crate::globe;
use flicker_worldgen::{FieldSampler, HexState};

use crate::world::{
    generate, generate_phase_layer, generate_with_seeds, load_tables, mutate_epoch_params,
    next_seed, WorldData, WorldParams, ABUNDANCE_DEFS, EPOCH_LABELS, MAX_FREQ, MIN_FREQ, PARAM_DEFS,
};

const FREQ_STEP: u32 = 4;
/// Real-seconds for the evolution timeline to play through every epoch at speed 1.
const TIMELINE_SECONDS: f32 = 60.0;
/// The duration knob (shared 1..10 clock) that sets each epoch's weight on the
/// timeline. Epoch 1 has none — it's the snapshot the clock starts from.
const EPOCH_DURATION_IDS: [&str; 6] =
    ["", "e2_duration", "e3_duration", "e4_duration", "e5_duration", "e6_duration"];
/// Timeline weight of the Epoch-1 snapshot — short but visible.
const SNAPSHOT_WEIGHT: f32 = 2.0;
/// Sub-steps each phase is iterated through (stop-motion granularity): the in-phase
/// layer is re-derived when the playhead crosses one of these within an epoch.
const PHASE_STEPS: u32 = 14;
const TEXT: [f32; 4] = [0.92, 0.94, 0.98, 1.0];
const DIM: [f32; 4] = [0.70, 0.75, 0.85, 1.0];

const HUD_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/world_ui.lua");
const UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/resources/ui_elements.json");

/// Splash + first generation. Renders one frame, then builds the world.
pub struct Loading {
    freq: u32,
    seed: u64,
    shown: bool,
}

impl Loading {
    pub fn new(freq: u32, seed: u64) -> Self {
        Self {
            freq,
            seed,
            shown: false,
        }
    }
}

impl Scene for Loading {
    fn update(&mut self, _dt: Duration, _input: &InputState, _renderer: &Renderer) -> Transition {
        if !self.shown {
            self.shown = true; // let the splash render once before the (blocking) gen
            return Transition::None;
        }
        let tables = load_tables().expect("material tables load from Alpha/content/data");
        let params = WorldParams::default();
        let data = generate(&tables, &params, self.freq, self.seed);
        Transition::Replace(Box::new(World::new(tables, params, data)))
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let size = renderer.size();
        let title = "flicker · world";
        let tw = renderer.measure_text(title, 32.0);
        renderer.draw_text(title, Vec2::new((size.x - tw.x) * 0.5, size.y * 0.42), 32.0, TEXT);
        let sub = "generating planet…";
        let sw = renderer.measure_text(sub, 18.0);
        renderer.draw_text(sub, Vec2::new((size.x - sw.x) * 0.5, size.y * 0.42 + 46.0), 18.0, DIM);
    }
}

/// Edge-detect state for the discrete keyboard shortcuts.
#[derive(Default)]
struct KeyEdges {
    view: bool,
    reseed: bool,
    freq_down: bool,
    freq_up: bool,
    epoch_up: bool,
    epoch_down: bool,
}

/// The world view.
pub struct World {
    tables: Tables,
    data: WorldData,
    params: WorldParams,
    cam: OrbitCam,
    celestial: CelestialState,
    /// Evolution timeline — the source of truth is `(epoch, phase_t)`: which epoch
    /// and how far *through* it (`0..1`). The global scrubber position is derived
    /// from these via the weighted bounds, so a duration change keeps the marker in
    /// its epoch. `timeline_speed` is the top-right "Speed" (playback rate; 0 =
    /// paused). `phase_layer` is the in-progress layer rendered at this sub-time,
    /// re-derived (throttled by `phase_key`) as the playhead advances.
    phase_t: f32,
    timeline_speed: f32,
    phase_layer: Vec<HexState>,
    phase_key: (usize, u32),
    mode: ViewMode,
    epoch: usize,
    mesh: Option<MeshHandle>,
    dirty: bool,
    /// Pending regeneration: `(grid frequency, per-epoch seeds)`.
    regen: Option<(u32, Vec<u64>)>,
    params_dirty: bool,
    prev: KeyEdges,
    script: Option<ScriptHost>,
    white: Option<TextureHandle>,
    ui_capture: bool,
    esc_prev: bool,
    /// The shared Prism UI theme (baked once in `enter`), handed to each pushed pause overlay.
    theme: Option<Theme>,
}

impl World {
    pub fn new(tables: Tables, params: WorldParams, data: WorldData) -> Self {
        let epoch = 0; // start on Epoch 1 — the composition foundation
        let phase_layer = data.layers[epoch].clone();
        Self {
            tables,
            data,
            params,
            cam: OrbitCam::new(globe::RADIUS),
            celestial: CelestialState::default(),
            phase_t: 0.0, // marker at the very start of Epoch 1 (the snapshot)
            timeline_speed: 0.0, // paused — drag the SKY "Speed" up to play the movie
            phase_layer,
            phase_key: (epoch, 0),
            mode: natural_view(epoch),
            epoch,
            mesh: None,
            dirty: true,
            regen: None,
            params_dirty: false,
            prev: KeyEdges::default(),
            script: None,
            white: None,
            ui_capture: false,
            esc_prev: false,
            theme: None,
        }
    }

    fn rebuild_mesh(&mut self, renderer: &mut Renderer) {
        if let Some(handle) = self.mesh.take() {
            renderer.free_mesh(handle);
        }
        let sampler = FieldSampler::new(&self.tables, self.data.seeds[0]);
        let (verts, indices) =
            globe::build(&self.data, &self.phase_layer, self.mode, globe::RADIUS, &sampler);
        self.mesh = Some(renderer.upload_mesh(&verts, MeshIndices::U32(&indices)));
        self.dirty = false;
    }

    /// Re-derive the in-progress layer for the current epoch at the quantised
    /// sub-progress (`phase_key`), and flag the mesh for rebuild. Cheap — runs only
    /// the one epoch on the layer below it (see [`generate_phase_layer`]).
    fn refresh_phase(&mut self) {
        let progress = self.phase_key.1 as f32 / PHASE_STEPS as f32;
        self.phase_layer =
            generate_phase_layer(&self.tables, &self.data, &self.params, self.epoch, progress);
        self.dirty = true;
    }

    /// The global scrubber position `0..1`, derived from `(epoch, phase_t)` against
    /// the current weighted bounds — so changing a duration keeps the marker inside
    /// its own (now resized) segment.
    fn playhead(&self) -> f32 {
        let b = self.epoch_bounds();
        let e = self.epoch.min(self.data.epochs() - 1);
        b[e] + self.phase_t.clamp(0.0, 1.0) * (b[e + 1] - b[e])
    }

    /// Each epoch's weight on the evolution timeline: its duration knob (the shared
    /// 1..10 clock) for Epochs 2–6, a fixed short cap for the Epoch-1 snapshot — so
    /// a long molten era visibly widens its segment and the movie lingers there.
    fn epoch_weights(&self) -> Vec<f32> {
        (0..self.data.epochs())
            .map(|e| match EPOCH_DURATION_IDS.get(e) {
                Some(id) if !id.is_empty() => (self.params.get(id) as f32).max(0.1),
                _ => SNAPSHOT_WEIGHT,
            })
            .collect()
    }

    /// Cumulative normalised segment boundaries `[0.0, …, 1.0]` (len = epochs + 1),
    /// proportional to [`Self::epoch_weights`]. Drives both the playhead↔epoch
    /// mapping and the HUD segment widths.
    fn epoch_bounds(&self) -> Vec<f32> {
        let weights = self.epoch_weights();
        let total = weights.iter().sum::<f32>().max(1e-3);
        let mut bounds = Vec::with_capacity(weights.len() + 1);
        let mut acc = 0.0;
        bounds.push(0.0);
        for w in weights {
            acc += w;
            bounds.push(acc / total);
        }
        bounds
    }

    /// The epoch whose weighted segment contains playhead position `t`.
    fn epoch_at(&self, t: f32) -> usize {
        let bounds = self.epoch_bounds();
        let last = self.data.epochs() - 1;
        (0..=last).find(|&e| t < bounds[e + 1]).unwrap_or(last)
    }

    /// Engine values published to the HUD script each frame, including the
    /// selected epoch's current knob values (keyed by param id).
    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::new()
            .with("cells", self.data.sphere.len())
            .with("freq", self.data.freq)
            .with("plates", self.data.plates.len())
            .with("basins", self.data.watersheds.len())
            .with("seed", format!("{:#010x}", self.data.seeds[self.epoch.min(self.data.seeds.len() - 1)]))
            .with("view_mode", self.mode.index())
            .with("view_label", self.mode.label())
            .with("epoch", (self.epoch + 1) as u32)
            .with("epoch_label", EPOCH_LABELS[self.epoch])
            // Sun-for-heat cycle — published every frame so the sliders track the
            // auto-advancing clock; harvested back in `apply_ui`.
            .with("time_of_day", self.celestial.time_of_day as f64)
            .with("year_month", self.celestial.year_month as f64)
            .with("axial_tilt_deg", self.celestial.axial_tilt.to_degrees() as f64)
            // Evolution timeline: the marker position (derived from epoch+phase) +
            // its playback speed (the "Speed" slider).
            .with("timeline", self.playhead() as f64)
            .with("play_speed", self.timeline_speed as f64);
        // Duration-weighted timeline segment boundaries (`tl_b1`..`tl_b{n-1}`).
        let bounds = self.epoch_bounds();
        for (i, b) in bounds.iter().enumerate().take(bounds.len() - 1).skip(1) {
            m = m.with(format!("tl_b{i}"), *b as f64);
        }
        for (ep, id, _, _, _) in PARAM_DEFS {
            if *ep == self.epoch {
                m = m.with(*id, self.params.get(id));
            }
        }
        // The element mix is Epoch 1's tuning — publish it on the Composition epoch.
        if self.epoch == 0 {
            for (sym, _) in ABUNDANCE_DEFS {
                let id = format!("ab_{sym}");
                let v = self.params.get(&id);
                m = m.with(id, v);
            }
        }
        m
    }

    /// Apply the HUD script's returned results to engine state.
    fn apply_ui(&mut self, results: &ValueMap) {
        if let Some(v) = results.number("view_mode") {
            let mode = ViewMode::from_index(v.round() as u32);
            if mode != self.mode {
                self.mode = mode;
                self.dirty = true;
            }
        }
        if let Some(e) = results.number("epoch") {
            let e = (e.round() as i64 - 1).clamp(0, self.data.epochs() as i64 - 1) as usize;
            if e != self.epoch {
                self.set_epoch(e); // jump to that epoch, fully formed
            }
        }
        if let Some(f) = results.number("freq") {
            let f = (f.round() as u32).clamp(MIN_FREQ, MAX_FREQ);
            if f != self.data.freq {
                self.regen = Some((f, self.data.seeds.clone()));
            }
        }
        // Evolution timeline: dragging the bottom scrubber sets the epoch *and* the
        // sub-position within it (so it scrubs through a phase, not just between
        // phases). Idempotent when not dragging (the slider echoes the marker).
        if let Some(v) = results.number("timeline") {
            let t = (v as f32).clamp(0.0, 1.0);
            // Only act on a *real* drag. When the scrubber isn't being dragged it
            // echoes back the published marker (`playhead()`); re-deriving from that
            // echo flips `phase_t==1.0` (an epoch's right edge) into the next epoch
            // at phase 0 — the startup "Epoch 2, solid color" bug.
            if (t - self.playhead()).abs() > 1e-3 {
                let e = self.epoch_at(t);
                let b = self.epoch_bounds();
                let span = (b[e + 1] - b[e]).max(1e-4);
                if e != self.epoch {
                    self.epoch = e;
                    self.mode = natural_view(e);
                    self.dirty = true;
                }
                self.phase_t = ((t - b[e]) / span).clamp(0.0, 1.0);
            }
        }
        if let Some(v) = results.number("play_speed") {
            self.timeline_speed = v as f32;
        }
        // Sun-for-heat sliders — always live (no regen; just move the sun).
        if let Some(v) = results.number("time_of_day") {
            self.celestial.time_of_day = v as f32;
        }
        if let Some(v) = results.number("year_month") {
            self.celestial.year_month = v as f32;
        }
        if let Some(v) = results.number("axial_tilt_deg") {
            self.celestial.axial_tilt = (v as f32).to_radians();
        }
        if results.is_on("reseed") {
            // Reseed wholesale-replaces params with a mutated set. The slider
            // `results` below were read from the *pre-reseed* model (the old/default
            // values), so applying them would immediately revert the mutation —
            // skip the feedback this frame; next frame the sliders read the new model.
            self.reseed();
            self.ui_capture = results.is_on("ui_capture");
            return;
        }
        // Knob sliders for the selected epoch — update params; regen on release.
        for (ep, id, _, _, _) in PARAM_DEFS {
            if *ep == self.epoch {
                if let Some(v) = results.number(id) {
                    if (v - self.params.get(id)).abs() > 1e-9 {
                        self.params.set(id, v);
                        self.params_dirty = true;
                    }
                }
            }
        }
        // Element-mix sliders (Epoch 1 / Composition).
        if self.epoch == 0 {
            for (sym, _) in ABUNDANCE_DEFS {
                let id = format!("ab_{sym}");
                if let Some(v) = results.number(&id) {
                    if (v - self.params.get(&id)).abs() > 1e-9 {
                        self.params.set(&id, v);
                        self.params_dirty = true;
                    }
                }
            }
        }
        self.ui_capture = results.is_on("ui_capture");
    }

    /// Keyboard shortcuts mirroring the panel (rising-edge detected).
    fn apply_keys(&mut self, input: &InputState) {
        let edges = KeyEdges {
            view: input.key_down(Key::V),
            reseed: input.key_down(Key::R),
            freq_down: input.key_down(Key::LeftBracket),
            freq_up: input.key_down(Key::RightBracket),
            epoch_up: input.key_down(Key::Up),
            epoch_down: input.key_down(Key::Down),
        };
        if edges.view && !self.prev.view {
            self.mode = self.mode.next();
            self.dirty = true;
        }
        if edges.reseed && !self.prev.reseed {
            self.reseed();
        }
        if edges.freq_down && !self.prev.freq_down {
            self.regen = Some((self.data.freq.saturating_sub(FREQ_STEP).max(MIN_FREQ), self.data.seeds.clone()));
        }
        if edges.freq_up && !self.prev.freq_up {
            self.regen = Some(((self.data.freq + FREQ_STEP).min(MAX_FREQ), self.data.seeds.clone()));
        }
        // ↑/↓ step one epoch (shown fully formed).
        let segs = self.data.epochs();
        if edges.epoch_up && !self.prev.epoch_up && self.epoch + 1 < segs {
            self.set_epoch(self.epoch + 1);
        }
        if edges.epoch_down && !self.prev.epoch_down && self.epoch > 0 {
            self.set_epoch(self.epoch - 1);
        }
        self.prev = edges;
    }

    /// Select an epoch *and* switch to the field that epoch is most meaningfully
    /// read in, so every phase shows its own character (the foundations on
    /// Epoch 1, continents on Tectonics, climate on Hydrosphere, …).
    fn set_epoch(&mut self, epoch: usize) {
        self.epoch = epoch.min(self.data.epochs() - 1);
        self.mode = natural_view(self.epoch);
        self.phase_t = 1.0; // stepping/keying to an epoch shows it fully formed
        self.dirty = true;
    }

    /// Reseed **the currently-viewed layer only**: advance that epoch's seed,
    /// re-derive the seeds of the layers built on it, and re-roll just that epoch's
    /// knobs — so the upstream layers stay byte-identical (a locked-in Epoch 1 keeps
    /// its look) while this phase gets a fresh variation. Queued for regen; the HUD
    /// sliders pick up the new values next frame.
    fn reseed(&mut self) {
        let e = self.epoch;
        let mut seeds = self.data.seeds.clone();
        seeds[e] = next_seed(seeds[e]);
        for k in (e + 1)..seeds.len() {
            seeds[k] = next_seed(seeds[k - 1]);
        }
        mutate_epoch_params(&mut self.params, e, seeds[e]);
        self.regen = Some((self.data.freq, seeds));
    }
}

/// The field each epoch is most meaningfully read in.
fn natural_view(epoch: usize) -> ViewMode {
    match epoch {
        0 => ViewMode::Composition, // foundations — the material mix
        1 => ViewMode::Crust,       // differentiation — molten-thin vs solid crust
        2 => ViewMode::Elevation,   // tectonics — continents & mountains
        3 => ViewMode::Elevation,   // hydrosphere — oceans flood the basins (Temperature is a tab away)
        4 => ViewMode::Life,        // mineralization — microbial life at the vents
        5 => ViewMode::Biome,       // erosion — biomes
        _ => ViewMode::Elevation,
    }
}

impl Scene for World {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.theme = Some(Theme::build(renderer));
        match ScriptHost::from_file(HUD_SCRIPT) {
            Ok(script) => {
                load_ui_json(&script, UI_ELEMENTS);
                load_widgets(&script);
                self.script = Some(script);
            }
            Err(e) => tracing::warn!("HUD script load failed ({HUD_SCRIPT}): {e} — using text HUD"),
        }
        self.rebuild_mesh(renderer);
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        if let Some(handle) = self.mesh.take() {
            renderer.free_mesh(handle);
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // Esc opens the pause overlay (the world freezes beneath it).
        let esc = input.key_down(Key::Escape);
        let esc_edge = esc && !self.esc_prev;
        self.esc_prev = esc;
        if esc_edge {
            if let Some(theme) = self.theme {
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    &InputMap::default(),
                    &AbstractControls::default(),
                    &GamepadConfig::default(),
                )));
            }
        }

        let size = renderer.size();
        let model = self.hud_model();
        let mut results = ValueMap::new();
        if let Some(script) = &self.script {
            let _ = script.set_model(&model);
            match script.update(input, size.x, size.y) {
                Ok(r) => results = r,
                Err(e) => tracing::warn!("HUD update error: {e}"),
            }
        }
        self.apply_ui(&results);
        self.apply_keys(input);

        // Regenerate from tuned knobs once the slider is released (dragging a
        // full planet regen every frame would stutter).
        if self.params_dirty && !input.mouse_left {
            self.regen = Some((self.data.freq, self.data.seeds.clone()));
            self.params_dirty = false;
        }

        self.cam.update(input, !self.ui_capture);
        self.celestial.update(dt); // sweep the sun → the day/night terminator moves

        // Evolution playback: advance *through* the current phase, then roll into the
        // next when it completes. Each phase's real-time is proportional to its
        // duration weight, so the marker sweeps the bar at a steady visual pace and
        // the whole movie runs ~TIMELINE_SECONDS at speed 1.
        if self.timeline_speed > 0.0 {
            let weights = self.epoch_weights();
            let total = weights.iter().sum::<f32>().max(1e-3);
            let last = self.data.epochs() - 1;
            loop {
                let w = weights[self.epoch.min(last)].max(0.1);
                self.phase_t += dt.as_secs_f32() * self.timeline_speed * total / (w * TIMELINE_SECONDS);
                if self.phase_t < 1.0 {
                    break;
                }
                if self.epoch < last {
                    self.epoch += 1;
                    self.mode = natural_view(self.epoch);
                    self.phase_t -= 1.0;
                } else {
                    self.phase_t = 1.0;
                    break;
                }
            }
        }

        // Re-derive the in-progress layer only when the quantised sub-step changes
        // (stop-motion), so playback doesn't regenerate every frame.
        let step = (self.phase_t * PHASE_STEPS as f32).round() as u32;
        let key = (self.epoch, step);
        if key != self.phase_key {
            self.phase_key = key;
            self.refresh_phase();
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some((freq, seeds)) = self.regen.take() {
            self.data = generate_with_seeds(&self.tables, &self.params, freq, &seeds);
            self.epoch = self.epoch.min(self.data.epochs() - 1);
            self.refresh_phase(); // re-derive the in-progress layer for the new world
        }
        if self.dirty {
            self.rebuild_mesh(renderer);
        }

        renderer.set_camera(&self.cam.camera());
        renderer.set_scene(self.celestial.lighting()); // orbiting sun → heat-map terminator
        renderer.draw_sky(); // sun + stars on the skybox
        if let Some(handle) = self.mesh {
            renderer.draw_mesh(handle, Mat4::IDENTITY, MeshDrawOptions::default());
        }

        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            let _ = script.set_model(&self.hud_model());
            let size = renderer.size();
            match script.draw(size.x, size.y) {
                Ok(cmds) => render_hud(renderer, &cmds, white, &[]),
                Err(e) => tracing::warn!("HUD draw error: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HUD script loads against the real layout JSON + widgets and runs an
    /// update/draw cycle for an epoch with knobs — catches Lua/contract breakage
    /// headlessly (no window needed).
    #[test]
    fn hud_script_loads_and_runs() {
        let script = ScriptHost::from_file(HUD_SCRIPT).expect("world_ui.lua loads");
        load_ui_json(&script, UI_ELEMENTS);
        load_widgets(&script);

        let mut model = ValueMap::new()
            .with("cells", 5762usize)
            .with("freq", 24u32)
            .with("seed", "0x0ec0de01")
            .with("view_mode", 1u32)
            .with("view_label", "elevation")
            .with("epoch", 3u32) // Tectonics — has four knobs
            .with("epoch_label", "Tectonics");
        for (ep, id, def, _, _) in PARAM_DEFS {
            if *ep == 2 {
                model = model.with(*id, *def);
            }
        }
        script.set_model(&model).expect("publish model");

        let input = InputState::new();
        let results = script.update(&input, 1280.0, 720.0).expect("update runs");
        assert!(results.is_on("ui_capture"), "capture rect should report a hit");
        // The tectonics knobs round-trip through the slider widgets.
        assert!(results.number("e3_plates").is_some(), "e3_plates slider value returned");

        let cmds = script.draw(1280.0, 720.0).expect("draw runs");
        assert!(!cmds.is_empty(), "HUD should emit draw commands");
    }

    /// On the Composition epoch the element-mix grid renders and round-trips an
    /// `ab_<symbol>` slider.
    #[test]
    fn element_grid_runs_on_composition_epoch() {
        let script = ScriptHost::from_file(HUD_SCRIPT).expect("world_ui.lua loads");
        load_ui_json(&script, UI_ELEMENTS);
        load_widgets(&script);

        let mut model = ValueMap::new()
            .with("cells", 5762usize)
            .with("freq", 24u32)
            .with("seed", "0x0ec0de01")
            .with("view_mode", 5u32)
            .with("view_label", "composition")
            .with("epoch", 1u32)
            .with("epoch_label", "Composition");
        for (sym, def) in ABUNDANCE_DEFS {
            model = model.with(format!("ab_{sym}"), *def);
        }
        script.set_model(&model).expect("publish model");

        let input = InputState::new();
        let results = script.update(&input, 1280.0, 720.0).expect("update runs");
        assert!(results.number("ab_O").is_some(), "O abundance slider returned");

        let cmds = script.draw(1280.0, 720.0).expect("draw runs");
        assert!(!cmds.is_empty(), "element grid should emit draw commands");
    }

    /// The always-on SKY panel renders and round-trips a celestial slider, so the
    /// JSON + Lua celestial wiring is caught headlessly.
    #[test]
    fn celestial_panel_runs_and_round_trips() {
        let script = ScriptHost::from_file(HUD_SCRIPT).expect("world_ui.lua loads");
        load_ui_json(&script, UI_ELEMENTS);
        load_widgets(&script);

        let model = ValueMap::new()
            .with("cells", 5762usize)
            .with("freq", 24u32)
            .with("seed", "0x0ec0de01")
            .with("view_mode", 1u32)
            .with("view_label", "elevation")
            .with("epoch", 4u32)
            .with("epoch_label", "Hydrosphere")
            .with("time_of_day", 8.0_f64)
            .with("play_speed", 1.0_f64)
            .with("moon_phase", 2.0_f64)
            .with("year_month", 3.0_f64)
            .with("axial_tilt_deg", 23.5_f64)
            .with("timeline", 0.5_f64);
        script.set_model(&model).expect("publish model");

        let input = InputState::new();
        let results = script.update(&input, 1280.0, 720.0).expect("update runs");
        assert!(results.number("time_of_day").is_some(), "time slider value returned");
        assert!(results.number("axial_tilt_deg").is_some(), "tilt slider value returned");
        // The bottom timeline scrubber round-trips its playhead value.
        assert!(results.number("timeline").is_some(), "timeline scrubber value returned");

        let cmds = script.draw(1280.0, 720.0).expect("draw runs");
        assert!(!cmds.is_empty(), "SKY panel should emit draw commands");
    }
}
