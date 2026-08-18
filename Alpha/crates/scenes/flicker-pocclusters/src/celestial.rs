//! From-Home heliocentric sky for the Cluster Editor.
//!
//! Recovered from the `examples/voxel-cluster` reference, with one upgrade: the
//! seven other worlds are placed by a **geocentric solve over the shared
//! [`flicker_orrery`] roster** (`normalize(planet − Home)`), not an ad-hoc circular
//! table — so the sky and the solar-birth cinematic read the same layout (single
//! source of truth). Sun/moon discs, the Milky-Way "galactic cloud", and the eclipse
//! corona are drawn by the engine sky pass (`draw_sky` + `sky.wgsl`) from the
//! [`SceneLighting`] this module computes; the planets, the ecliptic, and the
//! constellations are overlays drawn here.
//!
//! Art may lie for beauty (the arcs, the glow), but the *body layout* is the
//! roster's truth, and BookV's **"equal apparent sizes"** rule lives HERE: the seven
//! render as equal-size discs (unlike the cinematic's class-sized bodies).

use std::time::Duration;

use flicker::render::{Mat4, Renderer, SceneLighting, TextureHandle, Vec2, Vec3};
use flicker_orrery as orrery;

/// How far out on the sky dome overlays sit (world units). Large, so they read as
/// "fixed to the sky" and are still depth-clipped by nearer terrain.
const SKY_R: f32 = 4200.0;
/// Ecliptic obliquity — Home's axial tilt of the orbital plane vs. the celestial
/// equator (~23.5°). A rendering choice (art), matching the reference.
const OBLIQUITY: f32 = 0.41;
/// Every planet renders at this apparent (angular-ish) disc size — BookV's "equal
/// apparent sizes" from Home's sky. Tunable.
const PLANET_DISC: f32 = 46.0;
/// Sun/moon disc radii — **must stay in sync with `sky.wgsl`** (the eclipse coverage
/// geometry is mirrored there).
const SUN_DISC_R: f32 = 0.038;
const MOON_DISC_R: f32 = 0.047;

// --- small math helpers -----------------------------------------------------

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A cheap deterministic hash → `0..1` (PCG-flavoured); seeds the placeholder
/// constellation figures so they're fixed, not random.
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28).wrapping_add(4))) ^ x).wrapping_mul(277_803_737);
    x = (x >> 22) ^ x;
    (x & 0x00ff_ffff) as f32 / 0x00ff_ffff as f32
}

// --- the celestial frame (sun/moon/stars share one rotation) ----------------

/// Seasonal declination of the sun arc, driven by the month (0..12).
fn season_tilt(year_month: f32) -> f32 {
    use std::f32::consts::TAU;
    0.25 * -((year_month / 12.0) * TAU).cos()
}

/// The daily rotation angle from the time of day (hours, 0..24). Sunrise ~06:00.
fn day_angle(time_of_day: f32) -> f32 {
    use std::f32::consts::TAU;
    ((time_of_day - 6.0) / 24.0) * TAU
}

/// Tilt the equatorial frame to the observer's latitude (degrees passed as radians).
fn latitude_mat(latitude: f32) -> Mat4 {
    Mat4::from_rotation_x(-latitude)
}

/// World-space direction toward the sun. `+Y` is up (altitude).
fn sun_direction(time_of_day: f32, year_month: f32, latitude: f32) -> Vec3 {
    let a = day_angle(time_of_day);
    let eq = Vec3::new(a.cos(), a.sin(), season_tilt(year_month)).normalize();
    latitude_mat(latitude).transform_vector3(eq)
}

/// World-space direction toward the moon: the sun's arc offset by the phase.
fn moon_direction(time_of_day: f32, moon_phase: f32, year_month: f32, latitude: f32) -> Vec3 {
    use std::f32::consts::TAU;
    let a = day_angle(time_of_day) + (moon_phase / 4.0) * TAU;
    let eq = Vec3::new(a.cos(), a.sin(), -season_tilt(year_month) * 0.5).normalize();
    latitude_mat(latitude).transform_vector3(eq)
}

/// The full lighting/sky state for one frame — the engine sky pass reads this.
/// Sun/moon direction + colour, ambient, the sky gradient, fog, the star-field
/// rotation, and the Advent **eclipse** darkening when the discs align.
fn compute_scene(
    time_of_day: f32,
    moon_phase: f32,
    year_month: f32,
    fog: f32,
    latitude: f32,
) -> SceneLighting {
    use std::f32::consts::TAU;

    let sun_dir = sun_direction(time_of_day, year_month, latitude);
    let sun_up = sun_dir.y.max(0.0);
    let sun_amt = (sun_dir.y * 3.0).clamp(0.0, 1.0);
    let warmth = (1.0 - sun_up).clamp(0.0, 1.0);
    let sun_hue = Vec3::new(1.0, 0.98, 0.92).lerp(Vec3::new(1.0, 0.52, 0.22), warmth * 0.85);
    let sun_color = sun_hue * (sun_amt * 0.95);

    let moon_dir = moon_direction(time_of_day, moon_phase, year_month, latitude);
    let moon_amt = (moon_dir.y * 3.0).clamp(0.0, 1.0);
    let illum = 0.5 - 0.5 * ((moon_phase / 4.0) * TAU).cos();
    let moon_color = Vec3::new(0.34, 0.42, 0.66) * (moon_amt * illum * 0.55);

    let ambient = Vec3::new(0.05, 0.06, 0.09).lerp(Vec3::new(0.30, 0.33, 0.40), sun_amt);
    let sky_zenith = Vec3::new(0.012, 0.016, 0.030).lerp(Vec3::new(0.09, 0.15, 0.30), sun_amt);
    let sky_horizon = Vec3::new(0.030, 0.040, 0.085).lerp(Vec3::new(0.42, 0.49, 0.58), sun_amt);

    // The Advent: when the moon disc covers the sun disc (with the sun up), kill the
    // key light and sink sky + ambient into a blood shadow. (Coverage geometry is
    // mirrored in sky.wgsl for the corona.)
    let separation = sun_dir.dot(moon_dir).clamp(-1.0, 1.0).acos();
    let coverage = 1.0
        - smoothstep(
            MOON_DISC_R - SUN_DISC_R,
            MOON_DISC_R + SUN_DISC_R,
            separation,
        );
    let eclipse = coverage * smoothstep(-0.02, 0.05, sun_dir.y);
    let sun_color = sun_color * (1.0 - eclipse);
    let ambient = ambient.lerp(Vec3::new(0.07, 0.022, 0.028), eclipse);
    let sky_zenith = sky_zenith.lerp(Vec3::new(0.035, 0.014, 0.022), eclipse);
    let sky_horizon = sky_horizon.lerp(Vec3::new(0.10, 0.030, 0.042), eclipse);

    let fog_curve = 0.65 + 0.35 * (1.0 - sun_amt);
    let fog_color = sky_horizon;
    let fog_density = fog.clamp(0.0, 1.0) * 0.0020 * fog_curve;

    let star_rotation =
        (latitude_mat(latitude) * Mat4::from_rotation_z(day_angle(time_of_day))).inverse();

    SceneLighting {
        sun_dir,
        sun_color,
        moon_dir,
        moon_color,
        ambient,
        sky_zenith,
        sky_horizon,
        fog_color,
        fog_density,
        star_rotation,
        ..SceneLighting::default()
    }
}

/// A 16×16 soft white RGBA disc — uploaded once, tinted per-planet by the billboard
/// colour multiply. (White-on-alpha so the tint becomes the planet's hue.)
pub fn build_disc_texture() -> (Vec<u8>, u32, u32) {
    const N: usize = 16;
    let mut px = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) * 0.5;
    for y in 0..N {
        for x in 0..N {
            let d = (((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt()) / c;
            let a = (1.0 - smoothstep(0.65, 1.0, d)).clamp(0.0, 1.0);
            let i = (y * N + x) * 4;
            px[i] = 255;
            px[i + 1] = 255;
            px[i + 2] = 255;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    (px, N as u32, N as u32)
}

/// A soft star **glow** sprite — a tight bright core + a wide exponential halo + a
/// faint 4-point diffraction glint (the sparkle) — white RGBA, tinted per-star by the
/// additive billboard colour. Gives the stars a glossy bloom look through the ordinary
/// billboard pipeline (no post-process pass needed).
pub fn build_star_glow_texture() -> (Vec<u8>, u32, u32) {
    const N: usize = 48;
    let mut px = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) * 0.5;
    for y in 0..N {
        for x in 0..N {
            let dx = (x as f32 - c) / c;
            let dy = (y as f32 - c) / c;
            let d = (dx * dx + dy * dy).sqrt();
            let core = smoothstep(0.16, 0.0, d); // tight bright centre
            let halo = 0.55 * (-d * 3.6).exp(); // soft wide bloom
                                                // Thin bright cross fading radially — the diffraction glint.
            let glint =
                0.30 * ((-dy.abs() * 24.0).exp() + (-dx.abs() * 24.0).exp()) * (-d * 2.4).exp();
            let a = (core + halo + glint).clamp(0.0, 1.0);
            let i = (y * N + x) * 4;
            px[i] = 255;
            px[i + 1] = 255;
            px[i + 2] = 255;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    (px, N as u32, N as u32)
}

// --- constellations ---------------------------------------------------------

/// A named star figure on the celestial sphere: an ordered chain of unit directions
/// (consecutive stars connected). Authored in the *celestial* frame so it co-rotates
/// with the shader's Milky-Way band.
pub struct Constellation {
    /// Canon/placeholder identity — kept for the constellation labels + selection a
    /// later HUD pass will surface (and to record which figure is the Chalice). Not
    /// yet read at render time.
    #[allow(dead_code)]
    pub name: &'static str,
    /// Star positions (unit directions in the celestial frame).
    pub stars: Vec<Vec3>,
    /// Connections as index pairs into `stars` — an explicit edge list, so any figure
    /// (open path, star-polygon, branching weave) is representable. This is the target
    /// of the SVG-authoring pipeline: circles → `stars`, lines → `edges`.
    pub edges: Vec<(usize, usize)>,
    /// The one canonical figure (the Chalice) is drawn brighter/gold.
    pub canonical: bool,
}

/// The galactic-plane pole in the celestial frame — **mirrors `sky.wgsl`** (the
/// Milky-Way band is the great circle perpendicular to this). The Chalice sits on
/// that band, at "galactic centre".
fn galactic_pole() -> Vec3 {
    Vec3::new(0.20, 0.46, 0.86).normalize()
}

/// A tangent frame `(tx, ty)` spanning the plane perpendicular to `axis`.
fn tangent_frame(axis: Vec3) -> (Vec3, Vec3) {
    let aux = if axis.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let tx = aux.cross(axis).normalize();
    let ty = axis.cross(tx);
    (tx, ty)
}

/// The sky's constellations: the **Chalice / septisigil** (canon — 7 evenly-spaced
/// stars in a ring at galactic centre) plus 12 **placeholder** figures whose shapes
/// are not yet ruled (only the Chalice is decided; the rest are trivially replaced
/// as canon lands — see the module/plan notes).
pub fn constellations() -> Vec<Constellation> {
    use std::f32::consts::TAU;
    let mut out = Vec::with_capacity(13);

    // The Chalice = the septisigil (Prism/septisigil.svg): the seven schools set at
    // the vertices of a regular heptagon at galactic centre (a direction in the
    // galactic plane, so it lands on the Milky-Way band), laced in the ruled "angelic
    // weave" — six lines forming the open path Red → Green → Black → White → Orange →
    // Yellow → Blue. Slot 0 = White at the top, then Yellow, Red, Orange, Black, Blue,
    // Green (the SVG's 360/7° layout).
    let g_pole = galactic_pole();
    let gc = tangent_frame(g_pole).0; // "galactic centre" — a direction in the band.
    let (tx, ty) = tangent_frame(gc);
    let ring_r: f32 = 0.16; // angular radius of the heptagon
    let mut stars = Vec::with_capacity(7);
    for slot in 0..7 {
        let a = slot as f32 / 7.0 * TAU;
        let p = gc * (1.0 - ring_r * ring_r).sqrt() + (tx * a.cos() + ty * a.sin()) * ring_r;
        stars.push(p.normalize());
    }
    // The weave, verbatim from septisigil.svg (slots: White 0 · Yellow 1 · Red 2 ·
    // Orange 3 · Black 4 · Blue 5 · Green 6):
    //   White→Black, White→Orange, Black→Green, Orange→Yellow, Green→Red, Yellow→Blue.
    let edges = vec![(0, 4), (0, 3), (4, 6), (3, 1), (6, 2), (1, 5)];
    out.push(Constellation {
        name: "The Chalice (septisigil)",
        stars,
        edges,
        canonical: true,
    });

    // Twelve placeholder figures — deterministic scatter around the sphere, kept off
    // the poles. SHAPES ARE NOT CANON: stand-ins until each ruled figure is authored
    // via the SVG pipeline (star circles → `stars`, connection lines → `edges`, exactly
    // like the Chalice above). Each is a simple open chain for now.
    for c in 0..12u32 {
        let theta = hash01(c * 13 + 1) * TAU;
        let z = hash01(c * 13 + 2) * 1.6 - 0.8;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let center = Vec3::new(r * theta.cos(), r * theta.sin(), z);
        let (ctx, cty) = tangent_frame(center);
        let n = 5 + (hash01(c * 13 + 3) * 3.0) as usize; // 5..7 stars
        let spread = 0.22;
        let mut stars = Vec::with_capacity(n);
        for s in 0..n {
            let h1 = hash01(c * 211 + s as u32 * 3 + 7);
            let h2 = hash01(c * 211 + s as u32 * 3 + 8);
            let p = center + ctx * ((h1 - 0.5) * spread) + cty * ((h2 - 0.5) * spread);
            stars.push(p.normalize());
        }
        let edges: Vec<(usize, usize)> = (0..stars.len() - 1).map(|i| (i, i + 1)).collect();
        out.push(Constellation {
            name: "placeholder",
            stars,
            edges,
            canonical: false,
        });
    }
    out
}

// --- the panel-driven state -------------------------------------------------

/// The live celestial controls (the Celestial Cycle panel writes these). Held in the
/// math's native units; the panel converts + formats.
pub struct CelestialState {
    /// Time of day, hours 0..24.
    pub time_of_day: f32,
    /// Moon phase, weeks 0..4 (0/4 = new, 2 = full).
    pub moon_phase: f32,
    /// Month of the year, 0..12 (season).
    pub year_month: f32,
    /// Sim minutes per real second (0 = paused).
    pub sim_speed: f32,
    /// Fog, 0..1.
    pub fog: f32,
    /// Observer latitude, degrees −90..90.
    pub latitude: f32,
    /// Orbital epoch, whole years (the continuous planet clock).
    pub epoch: f32,
    /// Overlay toggles.
    pub show_planets: bool,
    pub show_constellations: bool,
    pub show_paths: bool,
    /// Real-time cosmetic clock (seconds) for the star twinkle — advances every frame
    /// regardless of sim speed, so the stars shimmer even when time is paused.
    clock: f32,
    /// Built once (cheap); the fixed figures.
    figures: Vec<Constellation>,
}

impl Default for CelestialState {
    fn default() -> Self {
        Self {
            time_of_day: 9.5,
            moon_phase: 1.2,
            year_month: 5.0,
            sim_speed: 30.0,
            fog: 0.22,
            latitude: 35.0,
            epoch: 3.4,
            show_planets: true,
            show_constellations: true,
            show_paths: false,
            clock: 0.0,
            figures: constellations(),
        }
    }
}

impl CelestialState {
    /// Auto-advance the clock by `sim_speed` (sim-minutes per real second); paused at
    /// 0. Moon over ~28 days (4 wk), year over 360 days (12 mo); the orbital epoch
    /// ticks each time the year wraps.
    pub fn update(&mut self, dt: Duration) {
        // Cosmetic twinkle clock — always ticks (even paused), so stars shimmer.
        self.clock += dt.as_secs_f32();
        if self.sim_speed <= 0.0 {
            return;
        }
        let d_min = self.sim_speed * dt.as_secs_f32();
        let d_days = d_min / (24.0 * 60.0);
        self.time_of_day = (self.time_of_day + d_min / 60.0).rem_euclid(24.0);
        self.moon_phase = (self.moon_phase + d_days / 7.0).rem_euclid(4.0);
        let prev = self.year_month;
        self.year_month = (self.year_month + d_days / 30.0).rem_euclid(12.0);
        if self.year_month < prev {
            self.epoch += 1.0;
        }
    }

    /// The frame's [`SceneLighting`] (sun/moon/sky/fog/eclipse/star-rotation).
    pub fn lighting(&self) -> SceneLighting {
        // NB: the frame helpers take latitude in **radians** (they feed
        // `Mat4::from_rotation_x`); `latitude` is stored in degrees, so convert here —
        // exactly as `draw` does. (Passing degrees rotated the whole sky by ~57× and
        // desynced the Milky-Way band from the constellations.)
        compute_scene(
            self.time_of_day,
            self.moon_phase,
            self.year_month,
            self.fog,
            self.latitude.to_radians(),
        )
    }

    /// Draw the sky overlays from the observer at `eye`: the seven worlds on the
    /// ecliptic (geocentric, from the shared roster, equal apparent size), the
    /// ecliptic track, and — at night — the constellations (Chalice + placeholders).
    /// Sun/moon/stars/Milky-Way come from the engine sky pass, not here.
    pub fn draw(
        &self,
        renderer: &mut Renderer,
        eye: Vec3,
        disc: Option<TextureHandle>,
        star_tex: Option<TextureHandle>,
    ) {
        use std::f32::consts::TAU;
        let lat = self.latitude.to_radians();
        let m = latitude_mat(lat) * Mat4::from_rotation_z(day_angle(self.time_of_day));
        let to_sky = |d: Vec3| eye + m.transform_vector3(d) * SKY_R;
        let (se, ce) = OBLIQUITY.sin_cos();
        let ecl = |lon: f32| Vec3::new(lon.cos(), lon.sin() * ce, lon.sin() * se);
        let night = 1.0
            - smoothstep(
                -0.12,
                0.06,
                sun_direction(self.time_of_day, self.year_month, lat).y,
            );

        // Night sky: the connecting figures (lines) — placeholders blue, the Chalice
        // gold — then the stars themselves as soft glowing dots.
        if self.show_constellations && night > 0.02 {
            let mut placeholder: Vec<(Vec3, Vec3)> = Vec::new();
            let mut chalice: Vec<(Vec3, Vec3)> = Vec::new();
            for fig in &self.figures {
                let dst = if fig.canonical {
                    &mut chalice
                } else {
                    &mut placeholder
                };
                for &(i, j) in &fig.edges {
                    dst.push((to_sky(fig.stars[i]), to_sky(fig.stars[j])));
                }
            }
            renderer.draw_lines(&placeholder, [0.50, 0.58, 0.82, 0.42 * night]);
            renderer.draw_lines(&chalice, [0.92, 0.80, 0.42, 0.85 * night]); // the septisigil, in gold

            // Stars = soft glowing dots of varying intensity, gently twinkling, on the
            // glow sprite (bright core + halo + glint). Additive (drawn as light, not
            // geometry), depth-tested so the horizon still occludes them.
            if let Some(tex) = star_tex {
                let mut sid = 0u32;
                for fig in &self.figures {
                    for &s in &fig.stars {
                        let base = 0.65 + 0.5 * hash01(sid * 4 + 1); // various intensity
                        let freq = 1.5 + 2.5 * hash01(sid * 4 + 2); // twinkle rate
                        let phase = hash01(sid * 4 + 3) * TAU;
                        let twinkle = 0.82 + 0.18 * (self.clock * freq + phase).sin();
                        let size = 34.0 + 22.0 * hash01(sid * 4 + 4); // slight size variety
                        let it = (base * twinkle * night).clamp(0.0, 1.0);
                        renderer.draw_billboard_additive(
                            tex,
                            to_sky(s),
                            Vec2::splat(size),
                            Vec2::ZERO,
                            Vec2::ONE,
                            [it * 0.92, it * 0.96, it, 1.0], // faintly cool-white
                        );
                        sid += 1;
                    }
                }
            }
        }

        // The ecliptic + the seven worlds — geocentric from the shared orrery.
        if self.show_planets {
            // The ecliptic highway.
            let mut track: Vec<(Vec3, Vec3)> = Vec::with_capacity(72);
            let mut prev = to_sky(ecl(0.0));
            for i in 1..=72 {
                let p = to_sky(ecl(i as f32 / 72.0 * TAU));
                track.push((prev, p));
                prev = p;
            }
            renderer.draw_lines(&track, [0.42, 0.40, 0.52, 0.32]);

            if let Some(tex) = disc {
                let roster = orrery::roster();
                // Home's heliocentric position at the epoch+season clock.
                let clock = self.epoch + self.year_month / 12.0;
                let t = clock * orrery::HOME_YEAR_SECONDS;
                let home = roster
                    .iter()
                    .find(|p| p.moon)
                    .expect("Home carries the moon");
                let home_pos = orrery::planet_pos(home, t);
                // Ecliptic pole (in the same celestial frame `ecl` lives in) — latitude
                // tilts toward it, lifting inclined bodies off the ecliptic line.
                let ecl_pole = Vec3::new(0.0, -se, ce);
                for p in roster.iter().filter(|p| !p.moon) {
                    let d = orrery::planet_pos(p, t) - home_pos; // 3-D apparent direction (orrery frame)
                    let dl = d.length().max(1e-6);
                    let lon = d.z.atan2(d.x); // ecliptic longitude
                    let beta = (d.y / dl).clamp(-1.0, 1.0).asin(); // ecliptic latitude (the out-of-plane tilt)
                    let (sb, cb) = beta.sin_cos();
                    // Carry the FULL apparent direction: the longitude circle tilted toward the
                    // ecliptic pole by its latitude, so inclined orbits (Death ~10°, the inner
                    // rockies a few°) leave the ecliptic line instead of riding it.
                    let pos = to_sky(ecl(lon) * cb + ecl_pole * sb);
                    // Equal apparent size (BookV from-Home rule); Death rides dim
                    // (occulted); the ringed giant reads a touch brighter.
                    let dim = if p.occulted { 0.28 } else { 1.0 };
                    let boost = if p.rings { 1.12 } else { 1.0 };
                    let color = [
                        (p.color[0] * dim * boost).min(1.0),
                        (p.color[1] * dim * boost).min(1.0),
                        (p.color[2] * dim * boost).min(1.0),
                        1.0,
                    ];
                    renderer.draw_billboard(
                        tex,
                        pos,
                        Vec2::splat(PLANET_DISC),
                        Vec2::ZERO,
                        Vec2::ONE,
                        color,
                    );
                }
            }
        }

        // The Advent alignment overlay: the sun's + moon's full daily arcs as rings.
        if self.show_paths {
            let sun_ring = arc(eye, |t| sun_direction(t, self.year_month, lat));
            let moon_ring = arc(eye, |t| {
                moon_direction(t, self.moon_phase, self.year_month, lat)
            });
            renderer.draw_lines(&sun_ring, [0.95, 0.72, 0.35, 0.5]);
            renderer.draw_lines(&moon_ring, [0.45, 0.55, 0.85, 0.5]);
        }
    }
}

/// A body's full daily arc (over 24 h) as a ring of segments at `SKY_R`.
fn arc(eye: Vec3, dir_at: impl Fn(f32) -> Vec3) -> Vec<(Vec3, Vec3)> {
    const STEPS: usize = 96;
    let mut segs = Vec::with_capacity(STEPS);
    let mut prev = eye + dir_at(0.0) * SKY_R;
    for i in 1..=STEPS {
        let p = eye + dir_at(i as f32 / STEPS as f32 * 24.0) * SKY_R;
        segs.push((prev, p));
        prev = p;
    }
    segs
}

// --- panel readout formatters (Rust owns the strings; the walker has no printf) ---
// These match the Celestial Cycle mockup's display, over the native math units.

/// `HH:MM` from hours in `0..24`.
pub fn fmt_clock(hours: f32) -> String {
    let h = (hours.floor() as i32).rem_euclid(24);
    let m = ((hours - hours.floor()) * 60.0).floor() as i32;
    format!("{h:02}:{m:02}")
}

/// Moon phase name + week, from weeks in `0..4`.
pub fn fmt_moon(weeks: f32) -> String {
    const PH: [&str; 8] = [
        "New",
        "Waxing Crescent",
        "First Quarter",
        "Waxing Gibbous",
        "Full",
        "Waning Gibbous",
        "Last Quarter",
        "Waning Crescent",
    ];
    let frac = (weeks / 4.0).rem_euclid(1.0);
    let idx = (frac * 8.0).round() as usize % 8;
    format!("{} \u{00b7} wk {:.1}", PH[idx], weeks)
}

/// Month name from `0..12`.
pub fn fmt_month(m: f32) -> String {
    const MO: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MO[(m.floor() as i32).rem_euclid(12) as usize].to_string()
}

/// Sim speed (`min/s`), or "paused".
pub fn fmt_speed(v: f32) -> String {
    if v <= 0.0 {
        "paused".to_string()
    } else {
        format!("{v:.0} min/s")
    }
}

/// Fog percent, or "clear".
pub fn fmt_fog(v: f32) -> String {
    if v <= 0.0 {
        "clear".to_string()
    } else {
        format!("{:.0}%", v * 100.0)
    }
}

/// Latitude in degrees → equator / poles / `N`·`S`.
pub fn fmt_lat(d: f32) -> String {
    if d.abs() < 0.5 {
        "equator".to_string()
    } else if d >= 89.5 {
        "north pole".to_string()
    } else if d <= -89.5 {
        "south pole".to_string()
    } else {
        format!("{:.0}\u{00b0}{}", d.abs(), if d > 0.0 { "N" } else { "S" })
    }
}

/// Orbital epoch, years.
pub fn fmt_epoch(v: f32) -> String {
    format!("yr {v:.1}")
}
