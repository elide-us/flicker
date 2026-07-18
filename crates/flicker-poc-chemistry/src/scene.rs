//! The chemistry scene — a **thin renderer** over the sim thread that draws the
//! world as a **stack of concentric layer meshes**.
//!
//! Each layer is its own mesh at its own radius, drawn where it exists and stacked
//! bottom→top: the **core** (inner sphere), the **mantle** shell above it (always
//! there, still convecting — never replaced), then each **crust bed** as a sparse
//! shell above the mantle (holes where the surface is still bare magma). Forming
//! crust *adds* an outer shell; it never recolours the mantle. The stack grows as
//! the chemistry produces layers (M3 water + atmosphere, M6 sediment beds, … each
//! just registers another shell). Occlusion shows the outermost shell that exists
//! at each spot; the cutaway wedge (next) slices the outer shells away to reveal
//! the interior.
//!
//! The sim runs on its own thread ([`crate::sim_thread`], spec §11); this scene
//! only sends commands (Space play/pause · R reseed · Down restart) and draws the
//! latest snapshot. **V** recolours the mantle shell (temperature / differentiation)
//! — an interior read, not a replacement of the stack.

use std::time::Duration;

use flicker::app::{AbstractControls, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, TextureHandle, Vec2, Vec3,
};
use flicker::scene::{Scene, Transition};
use flicker_shell::{PauseScene, Theme};

use crate::camera::OrbitCam;
use crate::globe::{self, RADIUS};
use crate::sim_thread::{SimCommand, SimHandle, Snapshot, BED_CONTINENTAL, BED_OCEANIC};
use flicker_poc_chemistry::PlateEvent;

/// Radii of the shells (exaggerated for legibility — real crust is a hair-thin
/// rind on the mantle; here the gaps are opened up so the stack is visible and the
/// cutaway can show it in section). Bottom → top.
const R_CORE: f32 = 0.50 * RADIUS;
const R_MANTLE: f32 = 0.960 * RADIUS;
const R_OCEANIC: f32 = 0.985 * RADIUS;
const R_CONTINENTAL: f32 = 1.000 * RADIUS;

/// Shell base colours (before lighting).
const CORE_COLOR: [f32; 3] = [0.95, 0.45, 0.20]; // molten metal
const OCEANIC_COLOR: [f32; 3] = [0.15, 0.22, 0.33]; // dark mafic sea floor
const CONTINENTAL_COLOR: [f32; 3] = [0.60, 0.54, 0.40]; // pale silicic land

/// A fresh seed from the wall clock — a new initial condition (spec §3.5). Launch
/// and **R** roll a new one; a given run stays deterministic.
fn clock_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// How the **mantle** shell is coloured (an interior read — the crust shells above
/// it are always drawn regardless).
#[derive(Copy, Clone, PartialEq)]
enum MantleView {
    Temperature,
    Differentiation,
    Plates,
    Seams,
}

impl MantleView {
    fn label(self) -> &'static str {
        match self {
            MantleView::Temperature => "temperature",
            MantleView::Differentiation => "differentiation",
            MantleView::Plates => "plates",
            MantleView::Seams => "seams",
        }
    }
    fn cycle(self) -> Self {
        match self {
            MantleView::Temperature => MantleView::Differentiation,
            MantleView::Differentiation => MantleView::Plates,
            MantleView::Plates => MantleView::Seams,
            MantleView::Seams => MantleView::Temperature,
        }
    }
}

pub struct ChemScene {
    // ── sim (on its own thread) ──
    sim: SimHandle,
    seed: u64,

    // ── static topology (received once) ──
    dirs: Vec<Vec3>,
    outlines: Vec<Vec<Vec3>>,
    budget_dist: Vec<(u8, String, f64)>,
    ready: bool,

    // ── latest published frame ──
    snap: Option<Snapshot>,
    last_gen: u64,

    // ── view ──
    cam: OrbitCam,
    /// The core sphere — static, built once.
    core_mesh: Option<MeshHandle>,
    /// The dynamic layer shells (mantle + crust beds), rebuilt on each new frame.
    shell_meshes: Vec<MeshHandle>,
    dirty: bool,
    mantle_view: MantleView,

    // ── shell ──
    theme: Option<Theme>,
    white: Option<TextureHandle>,

    // ── key edges ──
    prev_menu: bool,
    prev_play: bool,
    prev_down: bool,
    prev_view: bool,
    prev_reseed: bool,
}

impl ChemScene {
    pub fn new() -> Self {
        let seed = clock_seed();
        Self {
            sim: SimHandle::spawn(seed),
            seed,
            dirs: Vec::new(),
            outlines: Vec::new(),
            budget_dist: Vec::new(),
            ready: false,
            snap: None,
            last_gen: 0,
            cam: OrbitCam::new(RADIUS),
            core_mesh: None,
            shell_meshes: Vec::new(),
            dirty: false,
            mantle_view: MantleView::Temperature,
            theme: None,
            white: None,
            prev_menu: false,
            prev_play: false,
            prev_down: false,
            prev_view: false,
            prev_reseed: false,
        }
    }

    fn free_meshes(&mut self, renderer: &mut Renderer) {
        if let Some(h) = self.core_mesh.take() {
            renderer.free_mesh(h);
        }
        for h in self.shell_meshes.drain(..) {
            renderer.free_mesh(h);
        }
    }
}

impl Default for ChemScene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for ChemScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.02, 0.03, 0.05, 1.0]; // deep space
        let theme = Theme::build(renderer);
        self.white = Some(theme.lua_textures()[0].1); // id 0 = "white"
        self.theme = Some(theme);
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        self.free_meshes(renderer);
        // The sim thread shuts down when `self.sim` (SimHandle) drops.
    }

    fn update(&mut self, _dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        let menu = input.key_down(Key::Escape);
        if menu && !self.prev_menu {
            self.prev_menu = menu;
            let theme = self.theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &InputMap::default(),
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }
        self.prev_menu = menu;

        if !self.ready {
            if let Some(s) = self.sim.take_static() {
                self.dirs = s.dirs;
                self.outlines = s.outlines;
                self.budget_dist = s.budget_dist;
                self.ready = true;
            }
        }

        let play = input.key_down(Key::Space);
        let down = input.key_down(Key::Down);
        let view = input.key_down(Key::V);
        let reseed = input.key_down(Key::R);
        if play && !self.prev_play {
            self.sim.send(SimCommand::TogglePlay);
        }
        if down && !self.prev_down {
            self.sim.send(SimCommand::Reset);
        }
        if reseed && !self.prev_reseed {
            self.seed = clock_seed();
            self.sim.send(SimCommand::Reseed(self.seed));
        }
        if view && !self.prev_view {
            self.mantle_view = self.mantle_view.cycle();
            self.dirty = true;
        }
        self.prev_play = play;
        self.prev_down = down;
        self.prev_view = view;
        self.prev_reseed = reseed;

        self.cam.update(input, true);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if !self.ready {
            self.draw_loading(renderer);
            return;
        }

        // The core shell is static — build it once.
        if self.core_mesh.is_none() && !self.dirs.is_empty() {
            let (v, i) = globe::build(&self.dirs, &self.outlines, R_CORE, |_| Some(CORE_COLOR));
            self.core_mesh = Some(renderer.upload_mesh(&v, MeshIndices::U32(&i)));
        }

        // Pull the newest frame; rebuild the dynamic shells if it advanced.
        if let Some(s) = self.sim.latest_if_newer(self.last_gen) {
            self.last_gen = s.gen;
            self.snap = Some(s);
            self.dirty = true;
        }

        if self.dirty {
            for h in self.shell_meshes.drain(..) {
                renderer.free_mesh(h);
            }
            if let Some(snap) = self.snap.as_ref() {
                let view = self.mantle_view;
                let dirs = &self.dirs;
                let outlines = &self.outlines;
                let cells = &snap.cells;
                let (tmin, tmax) = cells
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(lo, hi), c| (lo.min(c.temp_k), hi.max(c.temp_k)));
                let tspan = (tmax - tmin).max(1.0);
                let light = Vec3::new(0.4, 0.7, 0.55).normalize();
                let lit = |i: usize, base: [f32; 3]| {
                    let l = (dirs[i].dot(light) * 0.5 + 0.5).clamp(0.25, 1.0);
                    [base[0] * l, base[1] * l, base[2] * l]
                };

                // Mantle shell — always present, coloured by the selected interior field.
                let (mv, mi) = globe::build(dirs, outlines, R_MANTLE, |i| {
                    let c = &cells[i];
                    let base = match view {
                        MantleView::Temperature => temp_color((c.temp_k - tmin) / tspan),
                        MantleView::Differentiation => diff_color(c.differentiation),
                        MantleView::Plates => plate_color(c.plate),
                        MantleView::Seams => seam_color(c.seam),
                    };
                    Some(lit(i, base))
                });
                self.shell_meshes.push(renderer.upload_mesh(&mv, MeshIndices::U32(&mi)));

                // Oceanic crust shell — sparse.
                let (ov, oi) = globe::build(dirs, outlines, R_OCEANIC, |i| {
                    (cells[i].beds & BED_OCEANIC != 0).then(|| lit(i, OCEANIC_COLOR))
                });
                if !oi.is_empty() {
                    self.shell_meshes.push(renderer.upload_mesh(&ov, MeshIndices::U32(&oi)));
                }

                // Continental crust shell — sparse, outermost.
                let (cv, ci) = globe::build(dirs, outlines, R_CONTINENTAL, |i| {
                    (cells[i].beds & BED_CONTINENTAL != 0).then(|| lit(i, CONTINENTAL_COLOR))
                });
                if !ci.is_empty() {
                    self.shell_meshes.push(renderer.upload_mesh(&cv, MeshIndices::U32(&ci)));
                }
            }
            self.dirty = false;
        }

        renderer.set_camera(&self.cam.camera());
        let opts = MeshDrawOptions::default();
        if let Some(h) = self.core_mesh {
            renderer.draw_mesh(h, Mat4::IDENTITY, opts);
        }
        for &h in &self.shell_meshes {
            renderer.draw_mesh(h, Mat4::IDENTITY, opts);
        }
        self.draw_hud(renderer);
    }
}

impl ChemScene {
    fn draw_loading(&self, renderer: &mut Renderer) {
        renderer.set_layer(10.0);
        let gold = [0.83, 0.67, 0.39, 1.0];
        let dim = [0.6, 0.63, 0.68, 1.0];
        renderer.draw_text("GENERATING PLANET…", Vec2::new(40.0, 60.0), 30.0, gold);
        renderer.draw_text(
            "freq 96 · 92,162 cells · bulk-accretion seed · building on the sim thread",
            Vec2::new(40.0, 104.0),
            16.0,
            dim,
        );
    }

    fn draw_hud(&self, renderer: &mut Renderer) {
        let Some(snap) = self.snap.as_ref() else {
            return;
        };
        let s = &snap.state;

        renderer.set_layer(10.0);
        let gold = [0.83, 0.67, 0.39, 1.0];
        let text = [0.85, 0.87, 0.92, 1.0];
        let dim = [0.6, 0.63, 0.68, 1.0];
        let green = [0.55, 0.85, 0.55, 1.0];

        renderer.draw_text("FLICKER · CHEMISTRY SIM (M2 · LAYER STACK)", Vec2::new(24.0, 24.0), 24.0, gold);
        let stats = format!("tick {}  ·  {:.0} My  ·  {} cells", snap.tick, snap.tick_myr, snap.swept_cells);
        renderer.draw_text(&stats, Vec2::new(24.0, 58.0), 17.0, text);

        let core_pct = s.core_mass_kg / s.planet_mass_kg.max(1.0) * 100.0;
        let interior = format!(
            "core {core_pct:.1}%  ·  differentiated {:.0}%  ·  mantle {:.0} K  ·  {} plates  ·  radiogenic {:.0} TW",
            s.differentiation_frac * 100.0,
            s.mean_mantle_temp_k,
            snap.plate_count,
            s.radiogenic_power_tw,
        );
        renderer.draw_text(&interior, Vec2::new(24.0, 84.0), 15.0, [0.72, 0.80, 0.92, 1.0]);

        let (word, col) = if snap.playing {
            ("PLAYING", green)
        } else {
            ("PAUSED", [0.92, 0.78, 0.42, 1.0])
        };
        renderer.draw_text(word, Vec2::new(24.0, 108.0), 14.0, col);
        renderer.draw_text(
            &format!(
                "·  Space play/pause  ·  R reseed  ·  Down reset  ·  V mantle: {}  ·  drag · wheel · Esc menu",
                self.mantle_view.label()
            ),
            Vec2::new(110.0, 108.0),
            14.0,
            dim,
        );

        let crust = format!(
            "crust {:.3}%  ·  continental {:.0}%  ·  mean elevation {:.0} m",
            s.crust_frac * 100.0,
            s.continental_frac * 100.0,
            s.mean_elevation_m,
        );
        renderer.draw_text(&crust, Vec2::new(24.0, 132.0), 15.0, [0.80, 0.86, 0.72, 1.0]);

        let Some(white) = self.white else {
            return;
        };

        // ── Conservation ledger (top-right). ──
        {
            let present = s.core_mass_kg
                + s.mantle_mass_kg
                + s.crust_mass_kg
                + s.atmosphere_mass_kg
                + s.ocean_mass_kg
                + s.escaped_mass_kg;
            let expected = s.planet_mass_kg + s.delivered_mass_kg;
            let balanced = (present - expected).abs() <= 1e-6 * expected.max(1.0);
            let total = expected.max(1.0);
            let pct = |m: f64| m / total * 100.0;

            let rows: [(&str, f64); 6] = [
                ("Mantle", s.mantle_mass_kg),
                ("Core", s.core_mass_kg),
                ("Crust", s.crust_mass_kg),
                ("Atmosphere", s.atmosphere_mass_kg),
                ("Ocean", s.ocean_mass_kg),
                ("Escaped", s.escaped_mass_kg),
            ];
            let (pad, row_h, panel_w) = (12.0f32, 18.0f32, 260.0f32);
            let panel_h = pad + 46.0 + rows.len() as f32 * row_h + pad;
            let px = renderer.size().x - panel_w - 16.0;
            let py = 20.0;
            renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.9]);
            renderer.draw_text("CONSERVATION LEDGER", Vec2::new(px + pad, py + pad), 14.0, gold);
            let status = if balanced { ("balanced ✓", green) } else { ("BROKEN ✗", [0.95, 0.4, 0.4, 1.0]) };
            renderer.draw_text(
                &format!("Σ {}  ·  {}", fmt_mass(expected), status.0),
                Vec2::new(px + pad, py + pad + 22.0),
                13.0,
                status.1,
            );
            let mut ry = py + pad + 46.0;
            for (label, mass) in rows {
                renderer.draw_text(&format!("{label:<11}{:>6.2}%", pct(mass)), Vec2::new(px + pad, ry), 13.0, text);
                ry += row_h;
            }
        }

        // ── Bulk-seed element distribution (left). ──
        if !self.budget_dist.is_empty() {
            let (pad, sw, row_h, panel_w) = (12.0f32, 12.0f32, 18.0f32, 210.0f32);
            let panel_h = pad + 24.0 + self.budget_dist.len() as f32 * row_h + pad;
            let (px, py) = (16.0f32, 158.0f32);
            renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.9]);
            renderer.draw_text("BULK ACCRETION SEED", Vec2::new(px + pad, py + pad), 14.0, gold);
            let mut ry = py + pad + 24.0;
            for (num, sym, pct) in &self.budget_dist {
                let c = element_rgb(*num);
                renderer.draw_sprite(white, Vec2::new(px + pad, ry + 2.0), Vec2::new(sw, sw), [c[0], c[1], c[2], 1.0]);
                renderer.draw_text(&format!("{sym}   {pct:.1}%"), Vec2::new(px + pad + sw + 8.0, ry), 13.0, text);
                ry += row_h;
            }
        }

        // ── Tectonic events (right, under the conservation ledger) — the observer's
        //    live read-out of plates being born, merging, splitting, dying. ──
        if !snap.recent_events.is_empty() {
            let (pad, row_h, panel_w) = (12.0f32, 16.0f32, 260.0f32);
            let panel_h = pad + 24.0 + snap.recent_events.len() as f32 * row_h + pad;
            let px = renderer.size().x - panel_w - 16.0;
            let py = 210.0;
            renderer.draw_sprite(white, Vec2::new(px, py), Vec2::new(panel_w, panel_h), [0.05, 0.06, 0.08, 0.9]);
            renderer.draw_text("TECTONIC EVENTS", Vec2::new(px + pad, py + pad), 14.0, gold);
            let mut ry = py + pad + 24.0;
            for (myr, ev) in snap.recent_events.iter().rev() {
                let (txt, col) = fmt_event(ev);
                renderer.draw_text(&format!("{myr:>6.0} My  {txt}"), Vec2::new(px + pad, ry), 13.0, col);
                ry += row_h;
            }
        }
    }
}

/// Linear blend of two RGB triples.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Temperature ramp over a normalised value: cool deep-blue → red → white-hot.
fn temp_color(x: f32) -> [f32; 3] {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        lerp3([0.10, 0.16, 0.55], [0.90, 0.35, 0.12], x * 2.0)
    } else {
        lerp3([0.90, 0.35, 0.12], [1.0, 0.95, 0.85], (x - 0.5) * 2.0)
    }
}

/// Core-formation progress: undifferentiated slate → differentiated gold.
fn diff_color(d: f32) -> [f32; 3] {
    lerp3([0.12, 0.13, 0.18], [0.95, 0.75, 0.30], d.clamp(0.0, 1.0))
}

/// A stable, distinct hue per persistent plate id (golden-ratio rotation). Because
/// the observer keeps a plate's id across ticks, its colour no longer flickers as it
/// drifts. Diffuse lithosphere (id 0) is neutral grey.
fn plate_color(id: u32) -> [f32; 3] {
    if id == 0 {
        return [0.22, 0.23, 0.26];
    }
    let h = (id as f32 * 0.618_034).fract() * std::f32::consts::TAU;
    [0.45 + 0.4 * h.cos(), 0.45 + 0.4 * (h + 2.094).cos(), 0.45 + 0.4 * (h + 4.188).cos()]
}

/// Seam class → colour: divergent ridge (blue), convergent trench (red), transform
/// (amber); interior / diffuse is dim.
fn seam_color(code: u8) -> [f32; 3] {
    match code {
        1 => [0.30, 0.55, 0.95], // divergent — spreading ridge
        2 => [0.90, 0.30, 0.25], // convergent — trench / collision
        3 => [0.95, 0.80, 0.30], // transform — strike-slip
        _ => [0.20, 0.21, 0.24], // interior / diffuse
    }
}

/// Muted colour per element (atomic number) for the distribution swatches.
fn element_rgb(number: u8) -> [f32; 3] {
    match number {
        26 => [0.56, 0.28, 0.18],
        8 => [0.55, 0.55, 0.60],
        14 => [0.72, 0.66, 0.52],
        12 => [0.58, 0.72, 0.55],
        16 => [0.86, 0.78, 0.36],
        28 => [0.70, 0.72, 0.74],
        20 => [0.80, 0.78, 0.72],
        13 => [0.66, 0.66, 0.70],
        x => {
            let h = (x as f32 * 0.137).fract();
            [0.40 + 0.30 * h, 0.35, 0.52 - 0.20 * h]
        }
    }
}

/// Format a mass in kg with a compact mantissa/exponent (e.g. `5.972e24 kg`).
fn fmt_mass(kg: f64) -> String {
    if kg <= 0.0 {
        return "0 kg".to_string();
    }
    let exp = kg.log10().floor() as i32;
    let mantissa = kg / 10f64.powi(exp);
    format!("{mantissa:.3}e{exp} kg")
}

/// Format a plate life-event for the events panel, with a colour cue.
fn fmt_event(e: &PlateEvent) -> (String, [f32; 4]) {
    match e {
        PlateEvent::Born(id) => (format!("born   P{id}"), [0.55, 0.85, 0.55, 1.0]),
        PlateEvent::Died(id) => (format!("died   P{id}"), [0.68, 0.70, 0.75, 1.0]),
        PlateEvent::Merged { from, into } => {
            (format!("merge  {}→P{into}", from.len() + 1), [0.55, 0.72, 0.95, 1.0])
        }
        PlateEvent::Split { from, into } => {
            (format!("split  P{from}→{}", into.len()), [0.95, 0.80, 0.40, 1.0])
        }
    }
}
