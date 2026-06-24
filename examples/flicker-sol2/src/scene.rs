//! The viewer scene — two modes over the same data. **Distribution view:** each Prism element is
//! a colour ring at its atomic-weight cast distance, clumpy + sheared (`crate::cloud`), with
//! overdensity **dots** marking where matter concentrates (`crate::detect`). **Collapse** (press
//! `Enter`): the cloud's clumps plus the conserved mass layer ignite into a planetary system —
//! a central star and an orbiting disk that accretes into planets (`crate::collapse`).
//!
//! Dials: `[`/`]` explosion (cast reach) · ↑/↓ falloff · `;`/`'` clump · `9`/`0` mass ·
//! `7`/`8` metallicity · ←/→ or hover focus a ring · wheel/`-`/`=` zoom · Enter ignite ·
//! Tab new system · Space pause · N reclump · B dots · G gravity well · R reset.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

use flicker::app::{InputState, Key};
use flicker::render::{Renderer, Vec2};
use flicker::scene::{Scene, Transition};

use crate::cloud::CloudField;
use crate::collapse::{BodyType, Sim};
use crate::detect::{self, View as DetectView};
use crate::draw;
use crate::mass::{CloudMass, MassParams, EARTH_PER_SUN};
use crate::model::{self, CastParams, Ejecta};
use crate::well;

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
/// Fixed steepness of the focus-band radial gradient (was the removed `,`/`.` dial).
const FOCUS_SHARPNESS: f32 = 2.0;

const CLUMP_STRENGTH_DEFAULT: f32 = 0.6;
const CLUMP_STRENGTH_MAX: f32 = 1.2;
const CLOUD_SEED0: u32 = 0xC10D_5EED;

/// Simulated years per real second while the collapse runs (a watch-speed dial).
const SIM_YEARS_PER_SEC: f32 = 6.0;
/// Motes sampled per element when igniting the collapse.
const MOTES_PER_EL: usize = 24;
/// Body draw size: physical radius × this boost, floored so small worlds still read. (We draw
/// well above true scale on purpose — real planets would be invisible specks.)
const BODY_DRAW_BOOST: f32 = 1.6;
const BODY_DRAW_MIN_PX: f32 = 3.0;
/// Motion-vector arrow length: screen px per (AU/yr) of speed, clamped readable.
const MOTION_SCALE: f32 = 10.0;
const MOTION_MIN_PX: f32 = 8.0;
const MOTION_MAX_PX: f32 = 48.0;

const TITLE: [f32; 4] = [0.92, 0.94, 0.99, 1.0];
const DIM: [f32; 4] = [0.60, 0.64, 0.76, 1.0];
const ACCENT: [f32; 4] = [0.96, 0.86, 0.60, 1.0];
const GRID: [f32; 4] = [0.40, 0.46, 0.60, 0.18];

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

/// The viewer.
pub struct CloudView {
    ejecta: Ejecta,
    params: CastParams,
    mass: MassParams,
    cloud_mass: CloudMass,
    au_at_edge: f32,
    focus: usize,
    cloud: CloudField,
    time: f32,
    paused: bool,
    anchor_au: f32,
    show_bodies: bool,
    show_well: bool,
    last_candidates: usize,
    sim: Option<Sim>,
    prev_r: bool,
    prev_enter: bool,
    prev_tab: bool,
    prev_g: bool,
    prev_left: bool,
    prev_right: bool,
    prev_space: bool,
    prev_n: bool,
    prev_b: bool,
}

impl CloudView {
    pub fn new() -> Self {
        let tables = model::load_tables();
        let ejecta = Ejecta::from_tables(&tables);
        let n = ejecta.elements.len();
        let focus = ejecta.elements.iter().position(|e| e.symbol == "Fe").unwrap_or(0);
        let mass = MassParams::default();
        let cloud_mass = CloudMass::derive(&ejecta, &mass);
        Self {
            ejecta,
            mass,
            cloud_mass,
            params: CastParams::default(),
            au_at_edge: DEFAULT_AU_AT_EDGE,
            focus,
            cloud: CloudField::new(n, CLOUD_SEED0, CLUMP_STRENGTH_DEFAULT),
            time: 0.0,
            paused: false,
            anchor_au: 1.0,
            show_bodies: true,
            show_well: false,
            last_candidates: 0,
            sim: None,
            prev_r: false,
            prev_enter: false,
            prev_tab: false,
            prev_g: false,
            prev_left: false,
            prev_right: false,
            prev_space: false,
            prev_n: false,
            prev_b: false,
        }
    }

    fn rot(&self, au: f32) -> f32 {
        self.cloud.omega(au, self.anchor_au) * self.time
    }

    fn nearest_ring(&self, cursor_au: f32, px_per_au: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, el) in self.ejecta.elements.iter().enumerate() {
            let d = (self.params.distance_au(el.atomic_mass) - cursor_au).abs() * px_per_au;
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
        let el = &self.ejecta.elements[self.focus];
        let au = self.params.distance_au(el.atomic_mass);
        let r_star = l.radius_px(au);
        if r_star < 3.0 || r_star > l.view_radius_px * 2.0 {
            return;
        }
        let r_in = r_star * FOCUS_INNER_FRAC;
        let r_out = r_star * FOCUS_OUTER_FRAC;
        let band_w = (r_out - r_in).max(1.0);
        let step_w = band_w / FOCUS_STEPS as f32;
        let p = FOCUS_SHARPNESS;
        let cloud = &self.cloud;
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
        let n = self.ejecta.elements.len().max(1);
        let cloud = &self.cloud;
        for (i, el) in self.ejecta.elements.iter().enumerate() {
            let au = self.params.distance_au(el.atomic_mass);
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

    /// Overdensity **dots** — where the sheared, clumpy material concentrates (the seeds a future
    /// formation sim would aggregate bodies from). Toggle with `B`.
    fn draw_candidates(&mut self, r: &mut Renderer, l: &Layout) {
        let view = DetectView {
            center: l.center,
            px_per_au: l.px_per_au,
            view_radius_px: l.view_radius_px,
        };
        let cands = detect::detect(&self.ejecta, &self.params, &self.cloud, self.time, self.anchor_au, &view);
        for c in &cands {
            let s = (c.strength / 0.8).clamp(0.0, 1.0);
            let core = 2.0 + 5.0 * s;
            draw::disc(r, c.pos, core * 2.0, [0.95, 0.86, 0.62, 0.12 + 0.12 * s], 16);
            draw::disc(r, c.pos, core, [1.0, 0.95, 0.80, 0.7], 16);
        }
        self.last_candidates = cands.len();
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

    fn draw_hud(&self, r: &mut Renderer) {
        r.draw_text("flicker · sol2 — supernova ejecta → system formation", Vec2::new(16.0, 16.0), 22.0, TITLE);
        let motion = if self.paused { "  ·  ⏸ paused" } else { "" };
        r.draw_text(
            &format!(
                "explosion {:.2}  ·  reach {:.0} AU  ·  falloff {:.2}  ·  clump {:.2}  ·  view edge {:.0} AU{}",
                self.params.explosion,
                self.params.reach_au(),
                self.params.falloff,
                self.cloud.strength,
                self.au_at_edge,
                motion,
            ),
            Vec2::new(16.0, 46.0),
            14.0,
            ACCENT,
        );
        r.draw_text(
            "[ ] explosion · ↑/↓ falloff · ;/' clump · ←/→ focus · wheel/-/= zoom",
            Vec2::new(16.0, 68.0),
            13.0,
            DIM,
        );
        r.draw_text(
            "Enter ignite · Tab new system · 9/0 mass · 7/8 metallicity · Space pause · N reclump · B dots · G well · R reset · Esc",
            Vec2::new(16.0, 86.0),
            13.0,
            DIM,
        );
        let el = &self.ejecta.elements[self.focus];
        let dots = if self.show_bodies { format!("  ·  {} density dots", self.last_candidates) } else { String::new() };
        r.draw_text(
            &format!("focus {} ({})  {:.1} AU{}", el.symbol, el.name, self.params.distance_au(el.atomic_mass), dots),
            Vec2::new(16.0, 108.0),
            14.0,
            [el.color[0] * 0.4 + 0.55, el.color[1] * 0.4 + 0.55, el.color[2] * 0.4 + 0.55, 1.0],
        );
    }

    /// The conserved **mass layer** readout: total tonnage, the metals fraction, a
    /// conservation check, and the per-element tonnages — so the cosmic-abundance shape
    /// (H/He bulk, iron peak, uranium trace) is verifiable at a glance.
    fn draw_mass_panel(&self, r: &mut Renderer) {
        let ej = &self.ejecta;
        let cm = &self.cloud_mass;
        let total = cm.total();
        let metals = cm.metals(ej);
        let zpct = if total > 0.0 { metals / total * 100.0 } else { 0.0 };
        r.draw_text(
            &format!(
                "cloud mass {:.2} M_sun  ·  metals {:.2}%  ·  sum {:.3} M_sun (conserved)",
                self.mass.total, zpct, total,
            ),
            Vec2::new(16.0, 136.0),
            14.0,
            [0.80, 0.86, 0.74, 1.0],
        );

        // Rank by tonnage; show the heaviest budgets, and always uranium (the HZ-world
        // trace) so its presence-but-tininess reads.
        let mut order: Vec<usize> = (0..ej.elements.len()).collect();
        order.sort_by(|&a, &b| cm.tonnage[b].total_cmp(&cm.tonnage[a]));
        let mut show: Vec<usize> = order.into_iter().take(11).collect();
        if let Some(u) = ej.elements.iter().position(|e| e.symbol == "U") {
            if !show.contains(&u) {
                show.push(u);
            }
        }
        for (row, chunk) in show.chunks(6).enumerate() {
            let line = chunk
                .iter()
                .map(|&i| format!("{} {}", ej.elements[i].symbol, fmt_earth(cm.tonnage[i] * EARTH_PER_SUN)))
                .collect::<Vec<_>>()
                .join("  ·  ");
            r.draw_text(&format!("{line}   M_earth"), Vec2::new(16.0, 158.0 + row as f32 * 18.0), 13.0, DIM);
        }
    }

    /// Each body's **motion vector**: a forward arrow in its direction of travel, plus a dotted
    /// trail behind it. This shows where each body is actually going — honest once orbits are
    /// eccentric or inclined, unlike a radius circle that just assumes a perfect ring.
    fn draw_motion(&self, r: &mut Renderer, l: &Layout) {
        let Some(sim) = &self.sim else {
            return;
        };
        for i in 0..sim.mass.len() {
            if !sim.alive[i] {
                continue;
            }
            let speed = sim.vel[i].length();
            if speed < 1e-4 {
                continue; // the pinned star isn't moving
            }
            let p = sim.pos[i];
            let pos_px = Vec2::new(l.center.x + p.x * l.px_per_au, l.center.y + p.y * l.px_per_au);
            let dir = sim.vel[i] / speed;
            let len = (speed * MOTION_SCALE).clamp(MOTION_MIN_PX, MOTION_MAX_PX);
            let step = Vec2::new(dir.x, dir.y) * len;
            let c = sim.classify(i).color();
            draw::arrow(r, pos_px, pos_px + step, 1.5, 7.0, [c[0], c[1], c[2], 0.85]);
            draw::dotted(r, pos_px, pos_px - step, 1.0, [c[0], c[1], c[2], 0.35], 4.0, 4.0);
        }
    }

    /// Render the collapse: planets as discs sized by mass (boosted for legibility) and tinted by
    /// emergent type. The **star** (body 0) draws as a small fixed dot — at true scale it's a
    /// ~1:1,000,000 speck, and its accretion/gravity reach is unchanged, so it stays bossy without
    /// swallowing the inner system on screen.
    fn draw_collapse(&self, r: &mut Renderer, l: &Layout) {
        let Some(sim) = &self.sim else {
            return;
        };
        for i in 0..sim.mass.len() {
            if !sim.alive[i] {
                continue;
            }
            let p = sim.pos[i];
            let sp = Vec2::new(l.center.x + p.x * l.px_per_au, l.center.y + p.y * l.px_per_au);
            if i == 0 {
                draw::disc(r, sp, 9.0, [1.0, 0.86, 0.55, 0.95], 30);
                draw::disc(r, sp, 5.0, [1.0, 0.98, 0.92, 1.0], 24);
                continue;
            }
            let rad = (sim.radius_au(i) * l.px_per_au * BODY_DRAW_BOOST).clamp(BODY_DRAW_MIN_PX, l.view_radius_px * 0.5);
            let c = sim.classify(i).color();
            draw::disc(r, sp, rad, [c[0], c[1], c[2], 0.92], 20);
        }
    }

    /// The collapse status line: elapsed time, body count, the largest (the star), and a
    /// running conservation check (live sum vs the starting tonnage).
    fn draw_sim_status(&self, r: &mut Renderer) {
        let Some(sim) = &self.sim else {
            return;
        };
        r.draw_text(
            &format!(
                "COLLAPSE  ·  t {:.0} yr  ·  {} bodies  ·  star {:.3} M_sun  ·  sum {:.3} / {:.3} M_sun",
                sim.time,
                sim.live_count(),
                sim.largest_mass(),
                sim.total_mass(),
                sim.init_total(),
            ),
            Vec2::new(16.0, 200.0),
            14.0,
            ACCENT,
        );
        // System makeup by emergent type.
        let (mut star, mut gg, mut ig, mut rock, mut small) = (0, 0, 0, 0, 0);
        for i in 0..sim.mass.len() {
            if !sim.alive[i] {
                continue;
            }
            match sim.classify(i) {
                BodyType::Star => star += 1,
                BodyType::GasGiant => gg += 1,
                BodyType::IceGiant => ig += 1,
                BodyType::RockyPlanet => rock += 1,
                BodyType::IcyBody | BodyType::Asteroid => small += 1,
            }
        }
        r.draw_text(
            &format!("{star} star · {gg} gas giant · {ig} ice giant · {rock} rocky · {small} small bodies"),
            Vec2::new(16.0, 220.0),
            13.0,
            DIM,
        );
    }

    /// Legend for the collapse view: what each body colour means.
    fn draw_type_legend(&self, r: &mut Renderer) {
        let size = r.size();
        let x = size.x - 176.0;
        let mut y = 74.0;
        r.draw_text("body types", Vec2::new(x, y), 14.0, TITLE);
        y += 22.0;
        for t in [
            BodyType::Star,
            BodyType::GasGiant,
            BodyType::IceGiant,
            BodyType::RockyPlanet,
            BodyType::IcyBody,
            BodyType::Asteroid,
        ] {
            let c = t.color();
            draw::rect(r, Vec2::new(x, y + 2.0), Vec2::new(11.0, 11.0), [c[0], c[1], c[2], 1.0]);
            r.draw_text(t.label(), Vec2::new(x + 18.0, y), 13.0, DIM);
            y += 17.0;
        }
    }

    fn draw_legend(&self, r: &mut Renderer) {
        let size = r.size();
        let x = size.x - 236.0;
        let mut y = 74.0;
        r.draw_text("ejecta rings (outer → inner)", Vec2::new(x, y), 14.0, TITLE);
        y += 24.0;
        for (i, el) in self.ejecta.elements.iter().enumerate() {
            let foc = self.focus == i;
            draw::rect(r, Vec2::new(x, y + 2.0), Vec2::new(11.0, 11.0), [el.color[0], el.color[1], el.color[2], 1.0]);
            let au = self.params.distance_au(el.atomic_mass);
            let line = format!("{:<2} {:>6.1} AU  {}", el.symbol, au, el.name);
            let col = if foc { [1.0, 1.0, 1.0, 1.0] } else { DIM };
            r.draw_text(&line, Vec2::new(x + 18.0, y), 13.0, col);
            y += 15.0;
        }
    }
}

impl Default for CloudView {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene for CloudView {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.006, 0.008, 0.014, 1.0];
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let dt = dt.as_secs_f32();
        let axis = |neg: Key, pos: Key| -> f32 {
            (input.key_down(pos) as i32 - input.key_down(neg) as i32) as f32
        };

        self.params.explosion =
            (self.params.explosion + axis(Key::LeftBracket, Key::RightBracket) * 0.5 * dt).clamp(0.0, 1.0);
        self.params.falloff =
            (self.params.falloff + axis(Key::Down, Key::Up) * 0.6 * dt).clamp(FALLOFF_MIN, FALLOFF_MAX);
        self.cloud.strength =
            (self.cloud.strength + axis(Key::Semicolon, Key::Apostrophe) * 0.6 * dt).clamp(0.0, CLUMP_STRENGTH_MAX);
        self.mass.total =
            (self.mass.total + axis(Key::Digit9, Key::Digit0) * 1.0 * dt).clamp(0.1, 10.0);
        self.mass.metallicity =
            (self.mass.metallicity + axis(Key::Digit7, Key::Digit8) * 0.03 * dt).clamp(0.0, 0.10);

        let zoom_keys = axis(Key::Minus, Key::Equal);
        let factor = (-(zoom_keys * 1.6 * dt) - input.mouse_wheel_delta * 0.12).exp();
        self.au_at_edge = (self.au_at_edge * factor).clamp(MIN_AU_AT_EDGE, MAX_AU_AT_EDGE);

        let r_down = input.key_down(Key::R);
        if r_down && !self.prev_r {
            self.params = CastParams::default();
            self.mass = MassParams::default();
            self.au_at_edge = DEFAULT_AU_AT_EDGE;
            self.cloud.strength = CLUMP_STRENGTH_DEFAULT;
            self.time = 0.0;
            self.sim = None;
        }
        self.prev_r = r_down;

        let n = self.ejecta.elements.len().max(1);
        let left = input.key_down(Key::Left);
        if left && !self.prev_left {
            self.focus = (self.focus + n - 1) % n;
        }
        self.prev_left = left;
        let right = input.key_down(Key::Right);
        if right && !self.prev_right {
            self.focus = (self.focus + 1) % n;
        }
        self.prev_right = right;

        let space = input.key_down(Key::Space);
        if space && !self.prev_space {
            self.paused = !self.paused;
        }
        self.prev_space = space;
        let nkey = input.key_down(Key::N);
        if nkey && !self.prev_n {
            let s = self.cloud.seed.wrapping_mul(0x9E37_79B1).wrapping_add(0x6D2B_79F5);
            self.cloud.reseed(s);
        }
        self.prev_n = nkey;
        let bkey = input.key_down(Key::B);
        if bkey && !self.prev_b {
            self.show_bodies = !self.show_bodies;
        }
        self.prev_b = bkey;
        let gkey = input.key_down(Key::G);
        if gkey && !self.prev_g {
            self.show_well = !self.show_well;
        }
        self.prev_g = gkey;
        let enter = input.key_down(Key::Enter);
        if enter && !self.prev_enter {
            // Ignite the collapse: sample the current conserved cloud into motes and let
            // them fall. R clears it back to the distribution view.
            self.sim = Some(Sim::from_cloud(
                &self.ejecta,
                &self.params,
                &self.cloud,
                &self.cloud_mass,
                MOTES_PER_EL,
            ));
        }
        self.prev_enter = enter;
        let tab = input.key_down(Key::Tab);
        if tab && !self.prev_tab {
            // New system: roll a fresh cloud seed and ignite the collapse from it.
            let s = self.cloud.seed.wrapping_mul(0x9E37_79B1).wrapping_add(0x6D2B_79F5);
            self.cloud.reseed(s);
            self.sim = Some(Sim::from_cloud(
                &self.ejecta,
                &self.params,
                &self.cloud,
                &self.cloud_mass,
                MOTES_PER_EL,
            ));
        }
        self.prev_tab = tab;

        if !self.paused {
            self.time += dt;
        }

        let (mut rmin, mut rmax) = (f32::MAX, 0.0_f32);
        for el in &self.ejecta.elements {
            let d = self.params.distance_au(el.atomic_mass);
            rmin = rmin.min(d);
            rmax = rmax.max(d);
        }
        self.anchor_au = (rmin.max(0.01) * rmax.max(0.01)).sqrt();
        self.cloud_mass = CloudMass::derive(&self.ejecta, &self.mass);

        let paused = self.paused;
        if let Some(sim) = self.sim.as_mut() {
            if !paused {
                sim.step(dt * SIM_YEARS_PER_SEC);
            }
        }

        // Hover-to-focus.
        let l = Layout::new(renderer.size(), self.au_at_edge);
        let cursor_au = (input.mouse_position - l.center).length() / l.px_per_au.max(1e-6);
        if let Some(i) = self.nearest_ring(cursor_au, l.px_per_au) {
            self.focus = i;
        }

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let l = Layout::new(renderer.size(), self.au_at_edge);
        self.draw_reference_rings(renderer, &l);
        if let Some(sim) = &self.sim {
            if self.show_well {
                let bodies: Vec<(Vec2, f32)> = (0..sim.mass.len())
                    .filter(|&i| sim.alive[i])
                    .map(|i| (sim.pos[i], sim.mass[i]))
                    .collect();
                well::draw(renderer, l.center, l.px_per_au, self.au_at_edge, &bodies);
            }
            self.draw_motion(renderer, &l);
            self.draw_collapse(renderer, &l);
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
        self.draw_hud(renderer);
        self.draw_mass_panel(renderer);
        self.draw_sim_status(renderer);
        if self.sim.is_some() {
            self.draw_type_legend(renderer);
        } else {
            self.draw_legend(renderer);
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
