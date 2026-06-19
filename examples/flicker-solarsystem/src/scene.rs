//! The viewer scene: an oblique orbit-camera look at a forming system, **playing back
//! a recorded formation run**. Embryos orbit, scatter, collide (with impact flashes),
//! merge or shatter, and a few settle into protoplanets; the disk's leftover dust
//! fades as it accretes. The HUD lists the survivors with a habitability verdict and,
//! for the selected one, the element-abundance vector it would hand to Epoch 1.

use std::f32::consts::TAU;
use std::time::Duration;

use flicker::app::{InputState, Key};
use flicker::render::{
    Renderer, SceneLighting, TextureHandle, Vec2, Vec3, VolumetricDisk, MAX_VOLUMETRIC_BODIES,
};
use flicker::scene::{Scene, Transition};
use flicker_materials::Tables;

use crate::astro::{earth_masses, G, M_EARTH, M_STAR};
use crate::body::{cleared_neighborhood, Body, BodyKind};
use crate::camera::OrbitCam;
use crate::disk::{self, DISK_INNER, DISK_OUTER, HZ_INNER, HZ_OUTER, SNOW_LINE};
use crate::habitability::assess;
use crate::material::{load_tables, MaterialClass};
use crate::sim::{self, Timeline};

const TEXT: [f32; 4] = [0.90, 0.92, 0.97, 1.0];
const DIM: [f32; 4] = [0.62, 0.66, 0.78, 1.0];
const GOOD: [f32; 4] = [0.55, 0.92, 0.60, 1.0];
/// The time slider's `0..1` maps to this many millions of years (cosmetic — see `sim`).
/// ~150 Myr spans a realistic terrestrial giant-impact phase.
const RUN_MYR: f32 = 150.0;
/// Physical radii are ~10⁻⁵ AU; scale them up so bodies are visible at disk scale.
const VISUAL_INFLATION: f64 = 6000.0;
const MIN_BODY: f32 = 0.18;
const MAX_BODY: f32 = 2.4;
/// Below this mass a survivor is a leftover planetesimal / belt object, not a planet.
const BELT_MASS: f64 = 0.03 * M_EARTH;

pub struct SolarSystem {
    cam: OrbitCam,
    tables: Tables,
    timeline: Timeline,
    seed: u64,
    /// Seeding-supernova size `0..1` — the master initial-condition dial.
    supernova: f64,
    /// Normalised playback time `0..1`.
    t: f32,
    play: bool,
    speed: f32,
    selected: usize,
    /// While true the camera is choreographed by the formation clock; the first drag hands
    /// manual control back, a reseed re-arms it.
    cinematic: bool,
    disc: Option<TextureHandle>,
    ring: Option<TextureHandle>,
    prev_space: bool,
    prev_r: bool,
    prev_up: bool,
    prev_down: bool,
    prev_lbracket: bool,
    prev_rbracket: bool,
}

impl SolarSystem {
    pub fn new() -> Self {
        let seed = 0xACC2_E71D;
        let supernova = disk::random_supernova(seed);
        Self {
            cam: OrbitCam::new(DISK_OUTER as f32),
            tables: load_tables(),
            timeline: sim::run(seed, supernova),
            seed,
            supernova,
            t: 0.0,
            play: true,
            speed: 0.032, // a slow, ~30 s cinematic pass
            selected: 0,
            cinematic: true,
            disc: None,
            ring: None,
            prev_space: false,
            prev_r: false,
            prev_up: false,
            prev_down: false,
            prev_lbracket: false,
            prev_rbracket: false,
        }
    }

    /// Re-run the formation sim for the current seed + supernova size and reset playback.
    fn rerun(&mut self) {
        self.timeline = sim::run(self.seed, self.supernova);
        self.t = 0.0;
        self.selected = 0;
        self.cinematic = true; // re-arm the choreographed camera for the new run
    }

    /// New random system: fresh seed *and* a fresh random supernova size.
    fn reseed(&mut self) {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.supernova = disk::random_supernova(self.seed);
        self.rerun();
    }

    /// Nudge the supernova-size dial and re-form the same seed's disk with it.
    fn dial_supernova(&mut self, delta: f64) {
        self.supernova = (self.supernova + delta).clamp(0.0, 1.0);
        self.rerun();
    }
}

impl Default for SolarSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// A closed ring of `segs` line segments in the disk plane (XZ) at `radius`.
fn ring(radius: f32, segs: usize) -> Vec<(Vec3, Vec3)> {
    let p = |i: usize| {
        let a = i as f32 / segs as f32 * TAU;
        Vec3::new(radius * a.cos(), 0.0, radius * a.sin())
    };
    (0..segs).map(|i| (p(i), p(i + 1))).collect()
}

/// The body's closed orbital ellipse as line segments (world AU), reconstructed from its
/// state vector: angular momentum `h = r×v` fixes the orbit plane, the eccentricity vector
/// points to periapsis, and `r(θ) = p/(1+e·cosθ)` traces the conic. Empty for an unbound
/// (hyperbolic) body — there's no closed orbit to draw.
fn orbit_ellipse(body: &Body) -> Vec<(Vec3, Vec3)> {
    use glam::DVec3;
    let mu = G * (M_STAR + body.mass);
    let r_vec = body.pos;
    let v_vec = body.vel;
    let r = r_vec.length();
    if r <= 0.0 || mu <= 0.0 {
        return Vec::new();
    }
    let v2 = v_vec.length_squared();
    if 0.5 * v2 - mu / r >= 0.0 {
        return Vec::new(); // unbound
    }
    let h_vec = r_vec.cross(v_vec);
    let h2 = h_vec.length_squared();
    if h2 <= 0.0 {
        return Vec::new();
    }
    let p = h2 / mu; // semi-latus rectum
    let e_vec = ((v2 - mu / r) * r_vec - r_vec.dot(v_vec) * v_vec) / mu;
    let e = e_vec.length();
    if e >= 1.0 {
        return Vec::new();
    }
    let n_hat = h_vec.normalize();
    let p_hat = if e > 1e-6 {
        e_vec / e
    } else {
        // Circular orbit: periapsis is arbitrary — pick any in-plane axis.
        let aref = if n_hat.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
        (aref - n_hat * aref.dot(n_hat)).normalize()
    };
    let q_hat = n_hat.cross(p_hat);
    const N: usize = 96;
    let point = |theta: f64| -> Vec3 {
        let rr = p / (1.0 + e * theta.cos());
        ((p_hat * theta.cos() + q_hat * theta.sin()) * rr).as_vec3()
    };
    (0..N)
        .map(|i| {
            let t0 = i as f64 / N as f64 * std::f64::consts::TAU;
            let t1 = (i + 1) as f64 / N as f64 * std::f64::consts::TAU;
            (point(t0), point(t1))
        })
        .collect()
}

/// The choreographed camera pose `(yaw, pitch, distance)` at formation time `t ∈ [0,1]`.
/// It opens **just below the disk plane, edge-on and well out** — looking *through* the dust
/// toward the star (which the cloud occludes, with rays leaking through the gaps) — then
/// gently **rises above** the plane and **glides inward** as the cloud clears and the planets
/// settle. A slow, languid Star-Trek-titles pass. Eased so the motion is gentle.
fn cinematic_pose(t: f32) -> (f32, f32, f32) {
    let e = {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t) // smoothstep
    };
    let lerp = |a: f32, b: f32| a + (b - a) * e;
    let outer = DISK_OUTER as f32;
    let yaw = lerp(0.15, 1.05); // slow drift for parallax
    let pitch = lerp(-0.12, 0.66); // just below the plane (edge-on) → above it
    let distance = lerp(outer * 3.2, outer * 0.52); // well out, looking through the cloud → in
    (yaw, pitch, distance)
}

/// Visible billboard size (AU) for a body of the given physical radius and kind.
fn body_size(radius_au: f64, kind: BodyKind) -> f32 {
    let base = (radius_au * VISUAL_INFLATION) as f32;
    let lo = if kind == BodyKind::Debris { 0.07 } else { MIN_BODY };
    base.clamp(lo, MAX_BODY)
}

/// Which bodies the protoplanet list shows (indices into `bodies`): the most massive few,
/// **plus** every currently-habitable world — so every blue-ringed planet is also listed
/// (no "two rings, one row" mismatch). Sorted largest-first.
fn displayed(bodies: &[Body]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..bodies.len()).collect();
    idx.sort_by(|&a, &b| bodies[b].mass.total_cmp(&bodies[a].mass));
    let mut shown: Vec<usize> = idx.iter().copied().take(8).collect();
    for &i in &idx {
        if shown.len() >= 12 {
            break;
        }
        if !shown.contains(&i) && assess(&bodies[i]).playable {
            shown.push(i);
        }
    }
    shown
}

impl Scene for SolarSystem {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.006, 0.008, 0.014, 1.0]; // deep space
        self.disc = Some(renderer.load_texture(&disk::disc_texture(), 16, 16));
        self.ring = Some(renderer.load_texture(&disk::ring_texture(), 32, 32));
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        // Camera: while the cinematic is armed it's driven by the formation clock; the first
        // drag hands manual control back.
        self.cam.update(input, !self.cinematic);
        if self.cinematic {
            if input.mouse_left {
                self.cinematic = false; // user grabbed the camera
            } else {
                let (yaw, pitch, distance) = cinematic_pose(self.t);
                self.cam.set_pose(yaw, pitch, distance);
            }
        }

        let space = input.key_down(Key::Space);
        if space && !self.prev_space {
            self.play = !self.play;
        }
        self.prev_space = space;

        let r = input.key_down(Key::R);
        if r && !self.prev_r {
            self.reseed();
        }
        self.prev_r = r;

        // [ / ] dial the seeding-supernova size down/up and re-form this seed's disk.
        let lb = input.key_down(Key::LeftBracket);
        if lb && !self.prev_lbracket {
            self.dial_supernova(-0.08);
        }
        self.prev_lbracket = lb;
        let rb = input.key_down(Key::RightBracket);
        if rb && !self.prev_rbracket {
            self.dial_supernova(0.08);
        }
        self.prev_rbracket = rb;

        // Up/Down cycle the selected body in the (live) protoplanet list.
        let n = displayed(self.current_bodies()).len().max(1);
        let up = input.key_down(Key::Up);
        if up && !self.prev_up {
            self.selected = (self.selected + n - 1) % n;
        }
        self.prev_up = up;
        let down = input.key_down(Key::Down);
        if down && !self.prev_down {
            self.selected = (self.selected + 1) % n;
        }
        self.prev_down = down;

        // ←/→ scrub; Space-driven playback otherwise.
        let scrub = dt.as_secs_f32() * 0.4;
        if input.key_down(Key::Left) {
            self.t = (self.t - scrub).max(0.0);
        }
        if input.key_down(Key::Right) {
            self.t = (self.t + scrub).min(1.0);
        }
        if self.play {
            self.t += dt.as_secs_f32() * self.speed;
            if self.t >= 1.0 {
                self.t = 1.0;
                self.play = false; // hold on the final system
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&self.cam.camera());
        let Some(disc) = self.disc else { return };
        let t = self.t;

        // Deep-space galactic background: the sky pass renders a Milky Way band + star field
        // at "night", so we push the sun *and* moon below the horizon (full night, no discs)
        // and set a near-black sky gradient. The dust then composites over it — dense dust
        // occludes the stars into dark lanes (the galactic-core look).
        renderer.draw_sky();
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.0, -1.0, 0.0),
            sun_color: Vec3::ZERO,
            moon_dir: Vec3::new(0.0, -1.0, 0.0),
            moon_color: Vec3::ZERO,
            sky_zenith: Vec3::new(0.004, 0.006, 0.014),
            sky_horizon: Vec3::new(0.007, 0.010, 0.022),
            ..SceneLighting::default()
        });

        // (The star is rendered *inside* the volumetric pass so the dust occludes it — no
        // separate star billboard here, or it'd draw on top and never be blocked.)

        // The protoplanetary disk — a real **volumetric** raymarched dust cloud (a shader,
        // not sprites). Its density dissipates inside-out with the formation clock and carves
        // annular gaps at the forming planets' orbits, so the cloud *is* the disk being
        // consumed, driven by the sim — not a decorative field on a timer.
        self.set_disk_cloud(renderer);

        // The bodies at the current playback instant.
        if let Some(snap) = self.timeline.sample(t as f64) {
            for b in &snap.bodies {
                let pos = b.pos.as_vec3();
                let c = b.dominant().map(MaterialClass::color).unwrap_or([0.7, 0.7, 0.72]);
                let size = if b.kind == BodyKind::Giant {
                    // Gas giant: a faint envelope halo behind the core.
                    let size = body_size(b.physical_radius(), b.kind);
                    renderer.draw_billboard_additive(disc, pos, Vec2::splat(size * 1.9), Vec2::ZERO, Vec2::ONE, [c[0], c[1], c[2], 0.18]);
                    renderer.draw_billboard_additive(disc, pos, Vec2::splat(size), Vec2::ZERO, Vec2::ONE, [c[0], c[1], c[2], 1.0]);
                    size
                } else if b.mass < BELT_MASS {
                    // Leftover planetesimal / belt object — small, dim, faintly icy-grey.
                    renderer.draw_billboard_additive(disc, pos, Vec2::splat(0.06), Vec2::ZERO, Vec2::ONE, [0.60, 0.62, 0.68, 0.55]);
                    0.06
                } else {
                    // A planet.
                    let size = body_size(b.physical_radius(), b.kind);
                    renderer.draw_billboard_additive(disc, pos, Vec2::splat(size), Vec2::ZERO, Vec2::ONE, [c[0], c[1], c[2], 1.0]);
                    size
                };
                self.draw_moons(renderer, disc, pos, size, b.moons.len() as u32);
            }
        }

        self.flashes(renderer, disc);

        // Orbit ellipses for the actual planets/giants (skip debris + tiny belt objects),
        // reconstructed from each body's state vector — the real shapes, incl. eccentric ones.
        for b in self.current_bodies() {
            if b.kind == BodyKind::Debris || b.mass < BELT_MASS {
                continue;
            }
            let col = if b.kind == BodyKind::Giant {
                [0.65, 0.55, 0.42, 0.22]
            } else {
                [0.45, 0.55, 0.78, 0.22]
            };
            renderer.draw_lines(&orbit_ellipse(b), col);
        }

        // Reference rings: snow line + habitable-zone band.
        renderer.draw_lines(&ring(SNOW_LINE as f32, 128), [0.55, 0.80, 0.95, 0.45]);
        renderer.draw_lines(&ring(HZ_INNER as f32, 128), [0.45, 0.85, 0.50, 0.5]);
        renderer.draw_lines(&ring(HZ_OUTER as f32, 128), [0.45, 0.85, 0.50, 0.5]);

        // As the system settles, ring the viable starter worlds in blue.
        self.mark_viable_worlds(renderer);

        self.hud(renderer);
    }
}

impl SolarSystem {
    /// The bodies at the current playback instant — what the render, the list, the rings and
    /// the export all read, so they always agree and update live as the system evolves.
    fn current_bodies(&self) -> &[Body] {
        self.timeline
            .sample(self.t as f64)
            .map(|s| s.bodies.as_slice())
            .unwrap_or(&[])
    }

    /// Configure the volumetric dust cloud for this frame: disk geometry + the formation
    /// clock + annular gaps at the current planets'/giants' orbits (wider for more massive
    /// bodies, via the Hill radius). The renderer raymarches it behind the bodies.
    fn set_disk_cloud(&self, renderer: &mut Renderer) {
        // Only **giants** carve gaps — they're the bodies that actually open a wide annular
        // gap (the ALMA look). Carving one per embryo would shred the whole cloud to nothing.
        let mut gaps: Vec<(f32, f32)> = self
            .current_bodies()
            .iter()
            .filter(|b| b.kind == BodyKind::Giant)
            .map(|b| {
                let (a, _) = b.orbital_elements();
                let r_hill = a * (b.mass / (3.0 * M_STAR)).cbrt();
                let width = (r_hill * 9.0).clamp(0.6, DISK_OUTER * 0.3);
                (a as f32, width as f32)
            })
            .collect();
        gaps.truncate(MAX_VOLUMETRIC_BODIES);
        renderer.set_volumetric_disk(VolumetricDisk {
            inner: DISK_INNER as f32,
            outer: DISK_OUTER as f32,
            snow_line: SNOW_LINE as f32,
            scale_height: 0.07,
            density: 2.2, // enough opacity to occlude the starfield into dark dust lanes
            formation: self.t,
            time: self.t * 10.0, // a few inner-disk rotations of swirl over the run
            tint: Vec3::new(0.07, 0.06, 0.09), // dark dust — visible by occluding the stars
            glow: Vec3::new(1.0, 0.55, 0.26),  // warm star-lit centre
            gaps,
        });
    }

    /// Little satellites orbiting a body, animated by playback time. Capped so a giant's
    /// moon system reads without clutter.
    fn draw_moons(&self, renderer: &mut Renderer, disc: TextureHandle, center: Vec3, body_size: f32, count: u32) {
        let n = count.min(6);
        for i in 0..n {
            let orbit_r = body_size * (1.3 + 0.22 * i as f32) + 0.10;
            let phase = self.t * TAU * 5.0 + i as f32 / n.max(1) as f32 * TAU;
            let off = Vec3::new(orbit_r * phase.cos(), 0.0, orbit_r * phase.sin());
            renderer.draw_billboard_additive(disc, center + off, Vec2::splat(0.05), Vec2::ZERO, Vec2::ONE, [0.85, 0.86, 0.92, 0.95]);
        }
    }

    /// Draw a pulsing blue ring around each **currently** habitable world — the same bodies
    /// the list marks PLAYABLE, so the rings and the list always agree as the system evolves.
    fn mark_viable_worlds(&self, renderer: &mut Renderer) {
        let Some(ringtex) = self.ring else { return };
        let pulse = 0.55 + 0.25 * (self.t * 8.0).sin().abs();
        for body in self.current_bodies() {
            if !assess(body).playable {
                continue;
            }
            let size = (body_size(body.physical_radius(), body.kind) * 2.6).max(0.9);
            renderer.draw_billboard_additive(
                ringtex,
                body.pos.as_vec3(),
                Vec2::splat(size),
                Vec2::ZERO,
                Vec2::ONE,
                [0.35, 0.65, 1.0, pulse],
            );
        }
    }

    /// Bright expanding billboards for collisions near the current playback time.
    fn flashes(&self, renderer: &mut Renderer, disc: TextureHandle) {
        let now = self.t as f64 * self.timeline.span_year;
        let window = (self.timeline.span_year / 90.0).max(1.0e-3);
        for ev in &self.timeline.events {
            let age = now - ev.t_year;
            if age < 0.0 || age > window {
                continue;
            }
            let k = 1.0 - (age / window) as f32; // 1 at impact → 0 fading out
            let size = 0.6 + (1.0 - k) * 2.4;
            let col = match ev.regime {
                crate::collide::Regime::Disruption => [1.0, 0.97, 0.9],
                crate::collide::Regime::Erosion => [1.0, 0.8, 0.5],
                crate::collide::Regime::HitAndRun => [0.8, 0.9, 1.0],
                crate::collide::Regime::Merge => [1.0, 0.88, 0.6],
                crate::collide::Regime::Capture => [0.7, 0.8, 1.0], // a gentle moon capture
            };
            renderer.draw_billboard_additive(
                disc,
                ev.site.as_vec3(),
                Vec2::splat(size),
                Vec2::ZERO,
                Vec2::ONE,
                [col[0], col[1], col[2], 0.7 * k],
            );
        }
    }

    fn hud(&self, renderer: &mut Renderer) {
        renderer.draw_text("flicker · solar-system formation", Vec2::new(16.0, 16.0), 24.0, TEXT);
        let myr = self.t * RUN_MYR;
        renderer.draw_text(
            &format!("T = {myr:.1} Myr (scaled)   {}", if self.play { "▶ playing" } else { "❚❚ paused" }),
            Vec2::new(16.0, 50.0),
            18.0,
            [0.96, 0.86, 0.60, 1.0],
        );
        renderer.draw_text(
            "Space play/pause · ←/→ scrub · ↑/↓ select · [ ] supernova · R reseed · drag · wheel · Esc",
            Vec2::new(16.0, 74.0),
            13.0,
            DIM,
        );

        let tl = &self.timeline;
        let neb = &tl.nebula;
        let bodies = self.current_bodies();
        // Live IAU classification of the bodies *at this instant*: giant, a planet that has
        // cleared its orbital band, or a dwarf/belt object that hasn't. These evolve as you
        // play — early on a packed disk is mostly dwarfs, settling into a few planets.
        let giants = bodies.iter().filter(|b| b.kind == BodyKind::Giant).count();
        let planets = (0..bodies.len())
            .filter(|&i| bodies[i].kind != BodyKind::Giant && cleared_neighborhood(i, bodies))
            .count();
        let dwarfs = bodies.len() - giants - planets;
        let impacts_so_far = tl
            .events
            .iter()
            .filter(|e| e.t_year <= self.t as f64 * tl.span_year)
            .count();
        renderer.draw_text(
            &format!(
                "supernova {:.2} → disk {:.1}× MMSN · metallicity {:.1}× · {} embryos",
                neb.supernova_size,
                neb.sigma_1au / disk::SIGMA_MMSN,
                neb.metallicity,
                tl.seeded,
            ),
            Vec2::new(16.0, 98.0),
            13.0,
            [0.78, 0.74, 0.92, 1.0],
        );
        renderer.draw_text(
            &format!(
                "→ {} planets · {} giants · {} dwarf/belt · {} ejected · {} consumed · {} impacts",
                planets, giants, dwarfs, tl.ejected, tl.consumed, impacts_so_far
            ),
            Vec2::new(16.0, 116.0),
            13.0,
            DIM,
        );
        renderer.draw_text("snow line (blue) · habitable zone (green)", Vec2::new(16.0, 134.0), 12.0, DIM);

        // Protoplanet panel — the *current* bodies (largest-first + any habitable world),
        // updating live as the system evolves.
        let mut y = 158.0;
        renderer.draw_text("protoplanets (a · mass · water · verdict):", Vec2::new(16.0, y), 14.0, TEXT);
        y += 24.0;
        let shown = displayed(bodies);
        let sel = self.selected.min(shown.len().saturating_sub(1));
        for (row, &bi) in shown.iter().enumerate() {
            let body = &bodies[bi];
            let (a, _e) = body.orbital_elements();
            let mass_e = earth_masses(body.mass);
            let water = body.comp.water_fraction() * 100.0;
            let dom = body.dominant().map(MaterialClass::label).unwrap_or("—");
            let v = assess(body);
            let marker = if row == sel { "▶ " } else { "  " };
            let kind = if body.kind == BodyKind::Giant {
                "giant"
            } else if cleared_neighborhood(bi, bodies) {
                dom
            } else {
                "dwarf"
            };
            let moons = match body.moons.len() {
                0 => String::new(),
                1 => " ☾1".to_string(),
                n => format!(" ☾{n}"),
            };
            let line = format!(
                "{marker}{a:>5.2} AU  {mass_e:>6.2} M⊕  {water:>4.1}%  {kind:<6}{moons}  {}",
                v.summary()
            );
            let color = if v.playable { GOOD } else { DIM };
            renderer.draw_text(&line, Vec2::new(16.0, y), 13.0, color);
            y += 19.0;
        }

        self.export_panel(renderer, y + 8.0, shown.get(sel).map(|&bi| &bodies[bi]));
    }

    /// The selected survivor's element-abundance vector — the Epoch-1 hand-off.
    fn export_panel(&self, renderer: &mut Renderer, mut y: f32, body: Option<&Body>) {
        let Some(body) = body else {
            return;
        };
        renderer.draw_text(
            &format!(
                "Epoch-1 composition of selected ({:.2} M⊕, {:.1} g/cm³):",
                earth_masses(body.mass),
                body.comp.density()
            ),
            Vec2::new(16.0, y),
            14.0,
            TEXT,
        );
        y += 22.0;
        let ab = body.comp.to_epoch1_abundance(&self.tables);
        let mut rows: Vec<(String, f64)> = ab.into_iter().collect();
        // Sort by abundance, breaking ties by symbol. The abundance map is a HashMap with
        // a randomized per-instance iteration order, and it's rebuilt every frame — without
        // a deterministic tie-break, equal-valued elements (e.g. several at 6.8) reshuffle
        // every frame and the row visibly thrashes.
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        // Fixed-width tokens (symbol padded to 2 so single- and two-letter symbols like
        // `Mg` line up) with a clear separator, wrapped to 5 per line so a long mix stays
        // inside the panel rather than crowding into the next symbol.
        let tokens: Vec<String> = rows
            .iter()
            .take(10)
            .map(|(s, v)| format!("{s:<2} {v:>4.1}"))
            .collect();
        for chunk in tokens.chunks(5) {
            renderer.draw_text(&chunk.join("     "), Vec2::new(16.0, y), 13.0, DIM);
            y += 18.0;
        }

        // Captured moons of the selected body (mass + type), if any.
        if !body.moons.is_empty() {
            y += 4.0;
            let moons: Vec<String> = body
                .moons
                .iter()
                .map(|m| {
                    let kind = m.comp.dominant().map(MaterialClass::label).unwrap_or("—");
                    format!("{:.3} M⊕ {kind}", earth_masses(m.mass))
                })
                .collect();
            renderer.draw_text(
                &format!("moons: {}", moons.join(" · ")),
                Vec2::new(16.0, y),
                13.0,
                [0.78, 0.80, 0.90, 1.0],
            );
        }
    }
}
