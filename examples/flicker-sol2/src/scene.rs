//! The simulation scene — a thin shell over [`flicker_system::System`], driven by a Lua HUD.
//!
//! Two modes over the same data. **Distribution view (Phase 1):** each Prism element is a colour
//! ring at its atomic-weight cast distance, clumpy + sheared, with overdensity **dots** (the
//! Phase-1 hot spots). **Collapse (Phase 2):** the cloud ignites into a planetary system — a
//! central star and an orbiting disk that accretes into planets, moons and rings, with the
//! habitable world highlighted.
//!
//! The scene holds no simulation state of its own — it feeds a `SystemConfig` (the dials) into the
//! `System` and renders what comes back. The simulation graphics are drawn here in Rust; every
//! control and readout lives in the Lua HUD (`scripts/sim_ui.lua` + `ui_elements.json`): a
//! bottom-right control panel (phase nav, dial sliders, toggles, buttons) and a top-right stats
//! readout. Esc opens the pause overlay.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

use flicker::app::{InputState, Key};
use flicker::render::{Renderer, TextureHandle, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{ScriptHost, Value, ValueMap};
use flicker::ui::{load_ui_json, load_widgets, render_hud};
use flicker_system::{BodyType, CastParams, MassParams, System, SystemConfig, EARTH_PER_SUN};

use crate::draw;
use crate::shell::Pause;
use crate::well;

const HUD_SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/sim_ui.lua");
const UI_ELEMENTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui_elements.json");

// ── tuning ──────────────────────────────────────────────────────────────────────
const DEFAULT_AU_AT_EDGE: f32 = 90.0;
const MIN_AU_AT_EDGE: f32 = 5.0;
const MAX_AU_AT_EDGE: f32 = 400.0;
const FALLOFF_MIN: f32 = 0.15;
const FALLOFF_MAX: f32 = 1.20;
const AU_TICKS: [f32; 13] = [1.0, 2.0, 3.0, 5.0, 10.0, 15.0, 20.0, 30.0, 40.0, 50.0, 75.0, 100.0, 150.0];

const FOCUS_INNER_FRAC: f32 = 0.40;
const FOCUS_OUTER_FRAC: f32 = 1.10;
const FOCUS_PEAK_ALPHA: f32 = 0.60;
const FOCUS_STEPS: usize = 36;
/// Fixed steepness of the focus-band radial gradient.
const FOCUS_SHARPNESS: f32 = 2.0;

/// Viewer-side clamp range for the clump dial (the default lives in `SystemConfig`).
const CLUMP_STRENGTH_DEFAULT: f32 = 0.6;
const CLUMP_STRENGTH_MAX: f32 = 1.2;

/// Default simulated years per real second while the collapse runs — the speed dial's start value.
const SIM_YEARS_PER_SEC: f32 = 6.0;
const SPEED_MAX: f32 = 60.0;
/// Body draw size: physical radius × this boost, floored so small worlds still read. (We draw
/// well above true scale on purpose — real planets would be invisible specks.)
const BODY_DRAW_BOOST: f32 = 1.6;
const BODY_DRAW_MIN_PX: f32 = 3.0;
/// Motion-vector arc length as a fraction of the body's orbital range (its pixel distance from
/// the star), so each arc reads as a consistent slice of its orbit — inner fast bodies don't
/// dwarf outer slow ones. Clamped readable.
const MOTION_ORBIT_FRAC: f32 = 0.16;
const MOTION_MIN_PX: f32 = 8.0;
const MOTION_MAX_PX: f32 = 60.0;
/// Largest angle (radians) a motion arc may sweep, so a tightly-curved body doesn't draw a loop.
const MOTION_ARC_MAX_SWEEP: f32 = 1.2;
/// Playable-world highlight: a pulsing green ring. Pulse rate (Hz) and the green tint.
const PLAYABLE_PULSE_HZ: f32 = 0.7;
const PLAYABLE_GREEN: [f32; 3] = [0.30, 1.00, 0.45];
/// Gravitational constant in AU³/(M☉·yr²) — `4π²`, matching the sim — for the arc's curvature.
const G_SIM: f32 = 39.478_418;

const DIM: [f32; 4] = [0.60, 0.64, 0.76, 1.0];
const GRID: [f32; 4] = [0.40, 0.46, 0.60, 0.18];

/// Advance a seed deterministically (a genuinely different cloud).
fn next_seed(seed: u32) -> u32 {
    seed.wrapping_mul(0x9E37_79B1).wrapping_add(0x6D2B_79F5)
}

/// Screen-space layout from window size + zoom. Shared by update (hover) and render.
struct Layout {
    center: Vec2,
    px_per_au: f32,
    view_radius_px: f32,
}

impl Layout {
    fn new(size: Vec2, au_at_edge: f32) -> Self {
        let view_radius_px = size.x.min(size.y) * 0.44;
        let center = Vec2::new((size.x * 0.40).max(view_radius_px + 24.0), size.y * 0.54);
        let px_per_au = view_radius_px / au_at_edge.max(0.001);
        Self { center, px_per_au, view_radius_px }
    }

    fn radius_px(&self, au: f32) -> f32 {
        au * self.px_per_au
    }
}

/// The simulation scene — a thin shell of view state over the `System` (which owns all sim state).
pub struct Sim {
    system: System,
    au_at_edge: f32,
    focus: usize,
    time: f32,
    /// Always-advancing clock (ignores pause) — drives the playable-world highlight pulse so it
    /// keeps breathing while the sim is paused for inspection.
    anim: f32,
    paused: bool,
    /// Simulated years per real second (the "Speed" dial).
    speed: f32,
    anchor_au: f32,
    show_bodies: bool,
    show_well: bool,
    last_candidates: usize,
    script: Option<ScriptHost>,
    white: Option<TextureHandle>,
    /// `true` when the cursor is over the Lua control panel — suppresses world hover/zoom so a
    /// slider drag doesn't also re-focus a ring or zoom the view.
    ui_capture: bool,
    esc_prev: bool,
}

impl Sim {
    pub fn new() -> Self {
        let system = System::new(SystemConfig::default());
        let focus = system.ejecta().elements.iter().position(|e| e.symbol == "Fe").unwrap_or(0);
        Self {
            system,
            au_at_edge: DEFAULT_AU_AT_EDGE,
            focus,
            time: 0.0,
            anim: 0.0,
            paused: false,
            speed: SIM_YEARS_PER_SEC,
            anchor_au: 1.0,
            show_bodies: true,
            show_well: false,
            last_candidates: 0,
            script: None,
            white: None,
            ui_capture: false,
            esc_prev: false,
        }
    }

    fn rot(&self, au: f32) -> f32 {
        self.system.cloud().omega(au, self.anchor_au) * self.time
    }

    fn nearest_ring(&self, cursor_au: f32, px_per_au: f32) -> Option<usize> {
        let cast = self.system.config().cast;
        let mut best: Option<(usize, f32)> = None;
        for (i, el) in self.system.ejecta().elements.iter().enumerate() {
            let d = (cast.distance_au(el.atomic_mass) - cursor_au).abs() * px_per_au;
            let better = match best {
                Some((_, b)) => d < b,
                None => true,
            };
            if better {
                best = Some((i, d));
            }
        }
        best.filter(|&(_, d)| d <= 8.0).map(|(i, _)| i)
    }

    /// Engine values published to the HUD script each frame: the dial positions (so the sliders
    /// track them), the conserved-mass readout, the focus element, and — once ignited — the
    /// collapse status.
    fn hud_model(&self) -> ValueMap {
        let cast = self.system.config().cast;
        let mass = self.system.config().mass;
        let ej = self.system.ejecta();
        let cm = self.system.cloud_mass();
        let total = cm.total();
        let metals = cm.metals(ej);
        let metals_pct = if total > 0.0 { metals / total * 100.0 } else { 0.0 };

        // Top tonnage line (Earth masses), with uranium always shown (the HZ-world trace).
        let mut order: Vec<usize> = (0..ej.elements.len()).collect();
        order.sort_by(|&a, &b| cm.tonnage[b].total_cmp(&cm.tonnage[a]));
        let mut show: Vec<usize> = order.into_iter().take(6).collect();
        if let Some(u) = ej.elements.iter().position(|e| e.symbol == "U") {
            if !show.contains(&u) {
                show.push(u);
            }
        }
        let mass_line = show
            .iter()
            .map(|&i| format!("{} {}", ej.elements[i].symbol, fmt_earth(cm.tonnage[i] * EARTH_PER_SUN)))
            .collect::<Vec<_>>()
            .join("  ·  ")
            + "  M_e";

        let el = &ej.elements[self.focus];
        let focus_sym = el.symbol.clone();
        let focus_name = el.name.clone();
        let focus_au = cast.distance_au(el.atomic_mass);
        let ignited = self.system.is_ignited();

        let mut m = ValueMap::new()
            .with("explosion", cast.explosion)
            .with("falloff", cast.falloff)
            .with("clump", self.system.cloud().strength)
            .with("mass", mass.total)
            .with("metallicity", mass.metallicity)
            .with("speed", self.speed)
            .with("view_edge", self.au_at_edge)
            .with("reach_au", cast.reach_au())
            .with("metals_pct", metals_pct)
            .with("cloud_sum", total)
            .with("focus_sym", focus_sym)
            .with("focus_name", focus_name)
            .with("focus_au", focus_au)
            .with("candidates", self.last_candidates)
            .with("mass_line", mass_line)
            .with("paused", self.paused)
            .with("dots", self.show_bodies)
            .with("well", self.show_well)
            .with("ignited", ignited)
            .with("phase", if ignited { 2u32 } else { 1u32 });

        if let Some(sim) = self.system.sim() {
            let (mut star, mut gg, mut ig, mut rock, mut small, mut ringed, mut playable) =
                (0, 0, 0, 0, 0, 0, 0);
            for i in 0..sim.mass.len() {
                if !sim.alive[i] {
                    continue;
                }
                if sim.ring_mass[i] > 0.0 {
                    ringed += 1;
                }
                if sim.is_playable(i) {
                    playable += 1;
                }
                match sim.classify(i) {
                    BodyType::Star => star += 1,
                    BodyType::GasGiant => gg += 1,
                    BodyType::IceGiant => ig += 1,
                    BodyType::RockyPlanet => rock += 1,
                    BodyType::IcyBody | BodyType::Asteroid => small += 1,
                }
            }
            let type_line = format!(
                "{star} star · {gg} gas · {ig} ice · {rock} rocky · {small} small · {ringed} ringed · {playable} playable"
            );
            m = m
                .with("sim_time", sim.time)
                .with("bodies", sim.live_count())
                .with("star_mass", sim.largest_mass())
                .with("sum_mass", sim.total_mass())
                .with("init_mass", sim.init_total())
                .with("type_line", type_line);
        }
        m
    }

    /// Apply the HUD script's returned results to the config + view state.
    fn apply_ui(&mut self, results: &ValueMap) {
        self.ui_capture = results.is_on("ui_capture");

        // Reset wholesale-restores defaults. The sliders this frame still echo the *old* model, so
        // skip harvesting them (next frame they read the reset values).
        if results.is_on("reset") {
            {
                let c = self.system.config_mut();
                c.cast = CastParams::default();
                c.mass = MassParams::default();
                c.clump = CLUMP_STRENGTH_DEFAULT;
            }
            self.system.sync_distribution();
            self.system.clear();
            self.au_at_edge = DEFAULT_AU_AT_EDGE;
            self.speed = SIM_YEARS_PER_SEC;
            self.time = 0.0;
            self.paused = false;
            return;
        }

        // Dial sliders → the config, then re-derive the Phase-1 distribution.
        {
            let mut c = *self.system.config();
            if let Some(v) = results.number("explosion") {
                c.cast.explosion = (v as f32).clamp(0.0, 1.0);
            }
            if let Some(v) = results.number("falloff") {
                c.cast.falloff = (v as f32).clamp(FALLOFF_MIN, FALLOFF_MAX);
            }
            if let Some(v) = results.number("clump") {
                c.clump = (v as f32).clamp(0.0, CLUMP_STRENGTH_MAX);
            }
            if let Some(v) = results.number("mass") {
                c.mass.total = (v as f32).clamp(0.1, 10.0);
            }
            if let Some(v) = results.number("metallicity") {
                c.mass.metallicity = (v as f32).clamp(0.0, 0.10);
            }
            *self.system.config_mut() = c;
        }
        self.system.sync_distribution();

        if let Some(v) = results.number("speed") {
            self.speed = (v as f32).clamp(0.0, SPEED_MAX);
        }
        if let Some(v) = results.number("view_edge") {
            self.au_at_edge = (v as f32).clamp(MIN_AU_AT_EDGE, MAX_AU_AT_EDGE);
        }

        // Toggles.
        if let Some(Value::Bool(b)) = results.get("pause") {
            self.paused = *b;
        }
        if let Some(Value::Bool(b)) = results.get("dots") {
            self.show_bodies = *b;
        }
        if let Some(Value::Bool(b)) = results.get("well") {
            self.show_well = *b;
        }

        // Action buttons: reseed (fresh cloud) / new system (reseed + ignite).
        if results.is_on("new_system") {
            let s = next_seed(self.system.config().seed);
            self.system.reseed(s);
            self.system.ignite();
        } else if results.is_on("reseed") {
            let s = next_seed(self.system.config().seed);
            self.system.reseed(s);
        }

        // Phase nav: Phase 1 = distribution (clear the collapse), Phase 2 = ignite the collapse.
        if results.is_on("phase1") {
            self.system.clear();
        }
        if results.is_on("phase2") {
            self.system.ignite();
        }
    }

    fn draw_reference_rings(&self, r: &mut Renderer, l: &Layout) {
        for &au in AU_TICKS.iter() {
            let radius = l.radius_px(au);
            if radius < 14.0 || radius > l.view_radius_px * 1.05 {
                continue;
            }
            draw::ring(r, l.center, radius, 1.0, GRID, ring_segs(radius));
            let label = format!("{au:.0} AU");
            let m = r.measure_text(&label, 11.0);
            r.draw_text(&label, Vec2::new(l.center.x - m.x * 0.5, l.center.y - radius - 14.0), 11.0, [DIM[0], DIM[1], DIM[2], 0.55]);
        }
    }

    fn draw_focus_band(&self, r: &mut Renderer, l: &Layout) {
        let el = &self.system.ejecta().elements[self.focus];
        let au = self.system.config().cast.distance_au(el.atomic_mass);
        let r_star = l.radius_px(au);
        if r_star < 3.0 || r_star > l.view_radius_px * 2.0 {
            return;
        }
        let r_in = r_star * FOCUS_INNER_FRAC;
        let r_out = r_star * FOCUS_OUTER_FRAC;
        let band_w = (r_out - r_in).max(1.0);
        let step_w = band_w / FOCUS_STEPS as f32;
        let p = FOCUS_SHARPNESS;
        let cloud = self.system.cloud();
        let i = self.focus;
        let col = el.color;
        let anchor = self.anchor_au;
        let time = self.time;
        let inv_px = 1.0 / l.px_per_au.max(1e-6);
        for s in 0..FOCUS_STEPS {
            let rad = r_in + (s as f32 + 0.5) * step_w;
            let base = (r_in / rad).powf(p);
            let inner = smoothstep(r_in, r_in + band_w * 0.15, rad);
            let outer = 1.0 - smoothstep(r_star, r_out, rad);
            let a_prof = FOCUS_PEAK_ALPHA * base * inner * outer;
            if a_prof <= 0.004 {
                continue;
            }
            let rot = cloud.omega(rad * inv_px, anchor) * time;
            draw::ring_sampled(
                r,
                l.center,
                step_w * 1.4,
                ring_segs(rad).min(96),
                |th| rad * (1.0 + cloud.wobble(i, th, rot)),
                |th| {
                    let a = (a_prof * cloud.density(i, th, rot)).min(0.95);
                    [col[0], col[1], col[2], a]
                },
            );
        }
    }

    fn draw_element_rings(&self, r: &mut Renderer, l: &Layout) {
        let ejecta = self.system.ejecta();
        let cast = self.system.config().cast;
        let n = ejecta.elements.len().max(1);
        let cloud = self.system.cloud();
        for (i, el) in ejecta.elements.iter().enumerate() {
            let au = cast.distance_au(el.atomic_mass);
            let radius = l.radius_px(au);
            if radius < 2.0 || radius > l.view_radius_px * 1.6 {
                continue;
            }
            let foc = self.focus == i;
            let base_a = if foc { 0.95 } else { 0.50 };
            let thick = if foc { 2.8 } else { 1.6 };
            let rot = self.rot(au);
            let col = el.color;
            draw::ring_sampled(
                r,
                l.center,
                thick,
                ring_segs(radius),
                |th| radius * (1.0 + cloud.wobble(i, th, rot)),
                |th| {
                    let a = (base_a * cloud.density(i, th, rot)).min(0.97);
                    [col[0], col[1], col[2], a]
                },
            );
            let ang = -FRAC_PI_2 + i as f32 / n as f32 * TAU;
            let lp = Vec2::new(l.center.x + radius * ang.cos(), l.center.y + radius * ang.sin());
            let m = r.measure_text(&el.symbol, 13.0);
            let lc = if foc {
                [1.0, 1.0, 1.0, 1.0]
            } else {
                [col[0] * 0.5 + 0.45, col[1] * 0.5 + 0.45, col[2] * 0.5 + 0.45, 0.92]
            };
            r.draw_text(&el.symbol, Vec2::new(lp.x - m.x * 0.5, lp.y - m.y * 0.5), 13.0, lc);
        }
    }

    /// Overdensity **dots** — the Phase-1 hot spots, where the sheared clumpy material
    /// concentrates (the seeds bodies aggregate from). Toggle with the Dots checkbox.
    fn draw_candidates(&mut self, r: &mut Renderer, l: &Layout) {
        let spots = self.system.hot_spots(self.time);
        let mut drawn = 0usize;
        for h in &spots {
            let rr = h.au * l.px_per_au;
            if rr < 2.0 || rr > l.view_radius_px * 1.7 {
                continue;
            }
            let pos = Vec2::new(l.center.x + rr * h.theta.cos(), l.center.y + rr * h.theta.sin());
            let s = (h.strength / 0.8).clamp(0.0, 1.0);
            let core = 2.0 + 5.0 * s;
            draw::disc(r, pos, core * 2.0, [0.95, 0.86, 0.62, 0.12 + 0.12 * s], 16);
            draw::disc(r, pos, core, [1.0, 0.95, 0.80, 0.7], 16);
            drawn += 1;
        }
        self.last_candidates = drawn;
    }

    fn draw_axes(&self, r: &mut Renderer, l: &Layout) {
        let len = l.view_radius_px * 0.28;
        let c = l.center;
        draw::arrow(r, c, Vec2::new(c.x + len, c.y), 2.0, 12.0, [0.95, 0.42, 0.42, 0.85]);
        draw::arrow(r, c, Vec2::new(c.x, c.y - len), 2.0, 12.0, [0.46, 0.90, 0.52, 0.85]);
        let zl = len * 0.66;
        draw::arrow(r, c, Vec2::new(c.x - zl, c.y - zl), 2.0, 12.0, [0.52, 0.62, 1.0, 0.85]);
        r.draw_text("+X", Vec2::new(c.x + len + 4.0, c.y - 7.0), 12.0, [0.95, 0.55, 0.55, 0.95]);
        r.draw_text("+Y", Vec2::new(c.x + 4.0, c.y - len - 16.0), 12.0, [0.58, 0.95, 0.62, 0.95]);
        r.draw_text("+Z", Vec2::new(c.x - zl - 24.0, c.y - zl - 14.0), 12.0, [0.62, 0.70, 1.0, 0.95]);
    }

    /// Each body's **motion vector** as a curved arc that bows around the gravity well (the
    /// osculating circle from the star's gravity): a forward arc with an arrowhead, plus a dotted
    /// trail behind. For a circular orbit the arc lies on the orbit ring; for an eccentric one it
    /// bends correctly. Captured moons draw nothing (their motion mirrors the host's).
    fn draw_motion(&self, r: &mut Renderer, l: &Layout) {
        let Some(sim) = self.system.sim() else {
            return;
        };
        let star_m = sim.mass[0];
        let to_px = |au: Vec2| Vec2::new(l.center.x + au.x * l.px_per_au, l.center.y + au.y * l.px_per_au);
        for i in 1..sim.mass.len() {
            // (body 0 is the pinned star — no vector)
            if !sim.alive[i] {
                continue;
            }
            // Captured moons get no arc: their absolute velocity just mirrors the host's orbit,
            // and their motion *around* the host is sub-pixel at system zoom — the moon disc beside
            // its planet already reads as a satellite.
            if sim.orbit_host(i) != 0 {
                continue;
            }
            let speed = sim.vel[i].length();
            let p = sim.pos[i];
            let r_au = p.length();
            if speed < 1e-4 || r_au < 1e-4 {
                continue;
            }
            let dir = sim.vel[i] / speed;
            // Arc length scales with orbital range so each reads as a consistent slice of its orbit.
            let len_px = (r_au * l.px_per_au * MOTION_ORBIT_FRAC).clamp(MOTION_MIN_PX, MOTION_MAX_PX);
            let len_au = len_px / l.px_per_au.max(1e-6);

            // Curvature from the star's gravity (the dominant well): the path bows around the star.
            let a = p * (-G_SIM * star_m / (r_au * r_au * r_au)); // accel toward the star
            let a_perp = a - dir * a.dot(dir);
            let a_perp_mag = a_perp.length();
            let c = sim.classify(i).color();
            let arrow_c = [c[0], c[1], c[2], 0.85];
            let trail_c = [c[0], c[1], c[2], 0.35];

            if a_perp_mag < 1e-6 {
                // Effectively straight (radial motion / negligible curvature) — fall back to a line.
                let tip = to_px(p) + dir * len_px;
                draw::arrow(r, to_px(p), tip, 1.5, 7.0, arrow_c);
                draw::dotted(r, to_px(p), to_px(p) - dir * len_px, 1.0, trail_c, 4.0, 4.0);
                continue;
            }
            let r_c = speed * speed / a_perp_mag; // radius of curvature (AU)
            let center = p + a_perp / a_perp_mag * r_c; // centre of the osculating circle (AU)
            let rel0 = p - center; // points from centre to the body
            // Sweep in the velocity's sense; cap the angle so a tight curve doesn't loop.
            let tang_ccw = Vec2::new(-rel0.y, rel0.x);
            let sign = if tang_ccw.dot(sim.vel[i]) >= 0.0 { 1.0 } else { -1.0 };
            let sweep = sign * (len_au / r_c).min(MOTION_ARC_MAX_SWEEP);
            let segs = ((len_px / 5.0) as usize).clamp(4, 14);
            let arc_pt = |phi: f32| {
                let (s, cs) = phi.sin_cos();
                center + Vec2::new(rel0.x * cs - rel0.y * s, rel0.x * s + rel0.y * cs)
            };
            // Forward arc → arrowhead at the tip.
            let mut prev = to_px(p);
            for k in 1..=segs {
                let pt = to_px(arc_pt(sweep * k as f32 / segs as f32));
                if k == segs {
                    draw::arrow(r, prev, pt, 1.5, 7.0, arrow_c);
                } else {
                    draw::line(r, prev, pt, 1.5, arrow_c);
                }
                prev = pt;
            }
            // Backward arc → dotted trail.
            let mut prevb = to_px(p);
            for k in 1..=segs {
                let pt = to_px(arc_pt(-sweep * k as f32 / segs as f32));
                draw::dotted(r, prevb, pt, 1.0, trail_c, 4.0, 4.0);
                prevb = pt;
            }
        }
    }

    /// Render the collapse: planets as discs sized by mass (boosted for legibility) and tinted by
    /// emergent type, with Roche rings and the pulsing playable-world highlight. The **star**
    /// (body 0) draws as a small fixed gold dot.
    fn draw_collapse(&self, r: &mut Renderer, l: &Layout) {
        let Some(sim) = self.system.sim() else {
            return;
        };
        for i in 0..sim.mass.len() {
            if !sim.alive[i] {
                continue;
            }
            let p = sim.pos[i];
            let sp = Vec2::new(l.center.x + p.x * l.px_per_au, l.center.y + p.y * l.px_per_au);
            if i == 0 {
                draw::disc(r, sp, 9.0, [1.0, 0.82, 0.20, 0.95], 30);
                draw::disc(r, sp, 5.0, [1.0, 0.98, 0.90, 1.0], 24);
                continue;
            }
            let rad = (sim.radius_au(i) * l.px_per_au * BODY_DRAW_BOOST).clamp(BODY_DRAW_MIN_PX, l.view_radius_px * 0.5);
            let c = sim.classify(i).color();
            // Tidally-shredded ring (Roche): a translucent icy annulus just outside the body,
            // its prominence set by how much of the body's mass is ring.
            if sim.ring_mass[i] > 0.0 {
                let frac = (sim.ring_mass[i] / sim.mass[i].max(1e-30)).clamp(0.0, 1.0);
                let segs = ((rad * 2.0 / 2.0) as usize).clamp(28, 96);
                let a = (0.20 + 0.5 * frac).min(0.7);
                draw::ring(r, sp, rad * 2.0, rad * 1.0, [0.82, 0.88, 0.95, a], segs);
            }
            draw::disc(r, sp, rad, [c[0], c[1], c[2], 0.92], 20);
            // Playable world: a pulsing green ring. It appears the frame a body enters the gate
            // and vanishes the frame it leaves (migration / growth / star change).
            if sim.is_playable(i) {
                let pulse = 0.5 + 0.5 * (self.anim * PLAYABLE_PULSE_HZ * TAU).sin();
                let ring_r = rad + 7.0 + 5.0 * pulse;
                let segs = ((ring_r / 2.0) as usize).clamp(24, 96);
                let g = PLAYABLE_GREEN;
                draw::ring(r, sp, ring_r, 2.0, [g[0], g[1], g[2], 0.30 + 0.45 * pulse], segs);
            }
        }
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for Sim {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.006, 0.008, 0.014, 1.0];
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        match ScriptHost::from_file(HUD_SCRIPT) {
            Ok(s) => {
                load_ui_json(&s, UI_ELEMENTS);
                load_widgets(&s);
                self.script = Some(s);
            }
            Err(e) => tracing::warn!("sim HUD script load failed ({HUD_SCRIPT}): {e}"),
        }
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // Esc opens the pause overlay (the sim freezes beneath it).
        let esc = input.key_down(Key::Escape);
        let esc_edge = esc && !self.esc_prev;
        self.esc_prev = esc;
        if esc_edge {
            return Transition::Push(Box::new(Pause::new()));
        }

        let dt = dt.as_secs_f32();
        let size = renderer.size();

        // Publish the model, run the HUD, and apply what it returns.
        let mut results = ValueMap::new();
        if let Some(script) = &self.script {
            let _ = script.set_model(&self.hud_model());
            match script.update(input, size.x, size.y) {
                Ok(r) => results = r,
                Err(e) => tracing::warn!("HUD update error: {e}"),
            }
        }
        self.apply_ui(&results);

        // Wheel zoom + hover-to-focus — only when the cursor isn't over the control panel.
        if !self.ui_capture {
            let factor = (-input.mouse_wheel_delta * 0.12).exp();
            self.au_at_edge = (self.au_at_edge * factor).clamp(MIN_AU_AT_EDGE, MAX_AU_AT_EDGE);

            let l = Layout::new(size, self.au_at_edge);
            let cursor_au = (input.mouse_position - l.center).length() / l.px_per_au.max(1e-6);
            if let Some(i) = self.nearest_ring(cursor_au, l.px_per_au) {
                self.focus = i;
            }
        }

        if !self.paused {
            self.time += dt;
        }
        self.anim += dt; // highlight pulse keeps breathing even while paused
        self.anchor_au = self.system.anchor_au();
        if !self.paused {
            self.system.step(dt * self.speed);
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let l = Layout::new(renderer.size(), self.au_at_edge);
        self.draw_reference_rings(renderer, &l);
        if let Some(sim) = self.system.sim() {
            if self.show_well {
                let bodies: Vec<(Vec2, f32)> = (0..sim.mass.len())
                    .filter(|&i| sim.alive[i])
                    .map(|i| (sim.pos[i], sim.mass[i]))
                    .collect();
                well::draw(renderer, l.center, l.px_per_au, self.au_at_edge, &bodies);
            }
            self.draw_motion(renderer, &l);
            self.draw_collapse(renderer, &l);
            self.last_candidates = 0; // dots are a distribution-only readout
        } else {
            self.draw_focus_band(renderer, &l);
            self.draw_element_rings(renderer, &l);
            if self.show_bodies {
                self.draw_candidates(renderer, &l);
            } else {
                self.last_candidates = 0;
            }
            draw::disc(renderer, l.center, 9.0, [1.0, 0.86, 0.55, 0.9], 30);
            draw::disc(renderer, l.center, 5.0, [1.0, 0.98, 0.92, 1.0], 24);
        }
        self.draw_axes(renderer, &l);

        // The Lua HUD (control panel + stats overlay) on top of the simulation.
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

fn ring_segs(radius: f32) -> usize {
    ((radius / 3.0) as usize).clamp(40, 128)
}

/// Format an Earth-mass tonnage: plain for sizeable budgets, scientific for traces (so
/// uranium reads as a small-but-nonzero number rather than rounding to zero).
fn fmt_earth(m_earth: f32) -> String {
    if m_earth >= 100.0 {
        format!("{m_earth:.0}")
    } else if m_earth >= 0.01 {
        format!("{m_earth:.2}")
    } else {
        format!("{m_earth:.1e}")
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if (b - a).abs() < 1e-6 {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sim HUD script loads against the real layout JSON + widgets and runs an update/draw
    /// cycle — catches Lua/contract breakage headlessly (no window needed).
    #[test]
    fn sim_hud_loads_and_runs() {
        let script = ScriptHost::from_file(HUD_SCRIPT).expect("sim_ui.lua loads");
        load_ui_json(&script, UI_ELEMENTS);
        load_widgets(&script);

        let model = ValueMap::new()
            .with("explosion", 0.5f32)
            .with("falloff", 0.6f32)
            .with("clump", 0.6f32)
            .with("mass", 1.0f32)
            .with("metallicity", 0.014f32)
            .with("speed", 6.0f32)
            .with("view_edge", 90.0f32)
            .with("reach_au", 90.0f32)
            .with("metals_pct", 1.4f32)
            .with("cloud_sum", 1.0f32)
            .with("focus_sym", "Fe")
            .with("focus_name", "Iron")
            .with("focus_au", 5.0f32)
            .with("candidates", 12usize)
            .with("mass_line", "H 1e3 · He 4e2  M_e")
            .with("paused", false)
            .with("dots", true)
            .with("well", false)
            .with("ignited", false)
            .with("phase", 1u32);
        script.set_model(&model).expect("publish model");

        let input = InputState::new();
        let results = script.update(&input, 1280.0, 720.0).expect("update runs");
        // A dial slider round-trips its value through the widget.
        assert!(results.number("explosion").is_some(), "explosion slider value returned");
        // The panel reports a capture flag (the cursor at 0,0 is outside it → false, but present).
        assert!(results.get("ui_capture").is_some(), "ui_capture reported");

        let cmds = script.draw(1280.0, 720.0).expect("draw runs");
        assert!(!cmds.is_empty(), "HUD should emit draw commands");
    }
}
