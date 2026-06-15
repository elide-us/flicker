//! The celestial cycle for the world viewer — a sun (and a tracked moon) orbiting
//! the planet's polar axis, producing a **day/night terminator that sweeps across
//! the globe** as time advances.
//!
//! Adapted from `examples/voxel-cluster`'s day/night system, but **reframed for the
//! orbit-camera globe view**. That model is for an observer standing on the surface
//! (the sky is a dome wheeling over a local horizon, and `sun_dir.y` means "how high
//! is the sun"). Here the camera looks *at* the planet from outside, so:
//!
//! - The sun orbits the planet's polar axis (`+Y`) — `time_of_day` is its longitude
//!   around that axis, `year_month` lifts it off the equatorial plane by the axial
//!   tilt (the season). The planet stays put; the sun moves around it.
//! - The directional sun lights the sphere directly: the terminator is **emergent**
//!   from `dot(surface_normal, sun_dir)` in the mesh shader (the globe carries radial
//!   per-cell normals), so the sun is always "on" — there is no whole-planet night to
//!   fade it to.
//!
//! The sun lights the globe and the dark "space" sky carries the procedural
//! starfield. The moon is a dim cool fill light whose brightness tracks its phase,
//! and when its disc overlaps the sun's (a new moon aligned with the sun, only
//! possible near an equinox) the same disc-overlap geometry the sky uses for its
//! corona sinks the whole scene into an eclipse "blood-shadow". All five knobs
//! (time / speed / moon / season / tilt) are driven from the HUD panel.

use std::f32::consts::TAU;
use std::time::Duration;

use flicker::render::{Mat4, SceneLighting, Vec3};

/// Real-seconds for one full simulated day at the default sim speed — paced so the
/// terminator is clearly moving but not dizzying when you first open the window.
const DEFAULT_DAY_SECONDS: f32 = 24.0;

/// Earth-like obliquity in degrees — the sun's maximum declination off the
/// equatorial plane, reached at the solstices.
const DEFAULT_AXIAL_TILT_DEG: f32 = 23.5;

/// Angular radii of the sun and moon discs, **mirrored from `flicker-render`'s
/// `sky.wgsl`** so the eclipse darkening here lines up with the sky's corona —
/// ground and sky go dark in step.
const SUN_DISC_R: f32 = 0.038;
const MOON_DISC_R: f32 = 0.047;

/// The animated celestial state. Pure data: advanced by [`Self::update`] and read by
/// [`Self::lighting`]. Owned by the `World` scene and stepped each frame.
pub struct CelestialState {
    /// Time of day, hours in `0..24`. Sets the sun's longitude around the axis.
    pub time_of_day: f32,
    /// Season, months in `0..12`. Sets the sun's declination via the axial tilt.
    pub year_month: f32,
    /// Lunar phase, weeks in `0..4`. Offsets the moon from the sun around the axis.
    pub moon_phase: f32,
    /// Obliquity in radians — the solstice declination amplitude.
    pub axial_tilt: f32,
    /// Whole years elapsed — bumped on each year rollover so the planets' orbital
    /// clock stays continuous (no snap when `year_month` wraps).
    pub epoch: f32,
    /// Auto-advance rate, simulated minutes per real second. `0.0` = paused.
    pub sim_speed: f32,
}

impl Default for CelestialState {
    fn default() -> Self {
        Self {
            time_of_day: 8.0, // morning light raking across one face to start
            year_month: 3.0,  // a tilted (solstice-ish) season so the poles read
            moon_phase: 2.0,  // full moon to start (lights the night side)
            axial_tilt: DEFAULT_AXIAL_TILT_DEG.to_radians(),
            epoch: 0.0,
            sim_speed: 24.0 * 60.0 / DEFAULT_DAY_SECONDS, // one day per DEFAULT_DAY_SECONDS
        }
    }
}

impl CelestialState {
    /// Advance the cycle by real-time `dt` (a no-op while paused). The day spins
    /// fastest; the moon phase and the season drift slowly off the same clock.
    pub fn update(&mut self, dt: Duration) {
        if self.sim_speed <= 0.0 {
            return;
        }
        let d_minutes = dt.as_secs_f32() * self.sim_speed;
        let d_hours = d_minutes / 60.0;
        let d_days = d_minutes / (24.0 * 60.0);
        self.time_of_day = (self.time_of_day + d_hours).rem_euclid(24.0);
        self.moon_phase = (self.moon_phase + d_days / 7.0).rem_euclid(4.0);
        let next_year = (self.year_month + d_days / 30.0).rem_euclid(12.0);
        if next_year < self.year_month {
            self.epoch += 1.0; // year rolled over — keep the planet clock continuous
        }
        self.year_month = next_year;
    }

    /// The sun's declination off the equatorial plane for the current season:
    /// `0` at the equinoxes (`year_month` 0 / 6), `±axial_tilt` at the solstices.
    fn declination(&self) -> f32 {
        self.axial_tilt * ((self.year_month / 12.0) * TAU).sin()
    }

    /// Direction *toward* the sun, in planet space.
    pub fn sun_dir(&self) -> Vec3 {
        axis_orbit(self.time_of_day, self.declination())
    }

    /// Direction toward the moon: the sun's orbit offset by the lunar phase around
    /// the axis, with a slight opposite declination lean (so at new moon — phase
    /// `0`/`4` — it coincides with the sun, the future-eclipse alignment).
    pub fn moon_dir(&self) -> Vec3 {
        let lon = self.time_of_day + (self.moon_phase / 4.0) * 24.0;
        axis_orbit(lon, -self.declination() * 0.5)
    }

    /// The illuminated fraction of the moon's disc for the current phase: `0` at
    /// the new moon (`moon_phase` 0 / 4), `1` at the full moon (phase 2).
    fn moon_illumination(&self) -> f32 {
        0.5 - 0.5 * ((self.moon_phase / 4.0) * TAU).cos()
    }

    /// How completely the moon's disc covers the sun's, from the angular
    /// separation of the two directions — the eclipse driver. Reaches `1` only when
    /// they nearly coincide, which (given the moon's opposite seasonal lean) needs
    /// both a new moon *and* a near-equinox season; the same geometry `sky.wgsl`
    /// uses for the corona.
    fn eclipse(&self) -> f32 {
        let separation = self.sun_dir().dot(self.moon_dir()).clamp(-1.0, 1.0).acos();
        1.0 - smoothstep(MOON_DISC_R - SUN_DISC_R, MOON_DISC_R + SUN_DISC_R, separation)
    }

    /// Build this frame's [`SceneLighting`]: a constant warm-white sun (the
    /// day/night terminator is emergent in the mesh shader from the globe's radial
    /// normals), a dim phase-lit moon fill, a low ambient floor so the night side
    /// stays legible, and a dark "space" sky for the procedural starfield. At an
    /// eclipse the direct sun is killed and ambient + sky sink into a desaturated
    /// blood-shadow, in step with the sky pass.
    pub fn lighting(&self) -> SceneLighting {
        let sun_dir = self.sun_dir();
        let moon_dir = self.moon_dir();
        let eclipse = self.eclipse();

        // Sun: full and always on (directional), killed under the eclipse shadow.
        let sun_color = Vec3::new(1.0, 0.97, 0.92) * (1.0 - eclipse);
        // Moon: a dim cool fill, brightness = lit fraction; its near face also
        // darkens as it slides in front of the sun.
        let moon_color = Vec3::new(0.34, 0.42, 0.66) * (self.moon_illumination() * 0.35 * (1.0 - eclipse));

        // Night-side fill + deep-space sky, both sinking toward blood-shadow at the
        // eclipse so the lit planet and the starry backdrop dim together.
        let ambient = Vec3::new(0.06, 0.07, 0.10).lerp(Vec3::new(0.05, 0.018, 0.022), eclipse);
        let sky_zenith = Vec3::new(0.004, 0.006, 0.012).lerp(Vec3::new(0.030, 0.012, 0.018), eclipse);
        let sky_horizon = Vec3::new(0.010, 0.014, 0.028).lerp(Vec3::new(0.060, 0.020, 0.028), eclipse);

        SceneLighting {
            sun_dir,
            sun_color,
            moon_dir,
            moon_color,
            ambient,
            sky_zenith,
            sky_horizon,
            // Stars fixed in world space as a stable backdrop — it's the sun that
            // orbits the (stationary) planet, not the camera frame.
            star_rotation: Mat4::IDENTITY,
            ..SceneLighting::default()
        }
    }

    // --- Phase 3 overlays: orbital paths, the ecliptic, and the other worlds. ---

    /// The sun's daily orbital path — a closed ring of segments at `radius` around
    /// the planet (a small circle at the current season's declination).
    pub fn sun_ring(&self, radius: f32) -> Vec<(Vec3, Vec3)> {
        orbit_ring(self.declination(), radius)
    }

    /// The moon's daily orbital path at `radius` (its own slightly-leaned circle).
    pub fn moon_ring(&self, radius: f32) -> Vec<(Vec3, Vec3)> {
        orbit_ring(-self.declination() * 0.5, radius)
    }

    /// A planet's current direction from the planet: a point on the tilted ecliptic
    /// whose longitude advances with the orbital clock — whole `epoch` years plus the
    /// within-year season, divided by the planet's period, so it never snaps at the
    /// year wrap.
    pub fn planet_dir(&self, p: &Planet) -> Vec3 {
        let lon = p.phase + (self.epoch + self.year_month / 12.0) / p.period * TAU;
        ecliptic_dir(lon, self.axial_tilt)
    }

    /// The ecliptic ring (the tilted plane the planets share) at `radius`.
    pub fn ecliptic_ring(&self, radius: f32) -> Vec<(Vec3, Vec3)> {
        let mut segs = Vec::with_capacity(RING_STEPS);
        let mut prev = ecliptic_dir(0.0, self.axial_tilt) * radius;
        for i in 1..=RING_STEPS {
            let p = ecliptic_dir(i as f32 / RING_STEPS as f32 * TAU, self.axial_tilt) * radius;
            segs.push((prev, p));
            prev = p;
        }
        segs
    }
}

/// Hermite smoothstep: `0` at/below `e0`, `1` at/above `e1`, smooth between.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Segments per full orbital ring.
const RING_STEPS: usize = 96;

/// A unit direction on the ecliptic plane — the equatorial (XZ) circle tilted about
/// the X axis by `obliquity` — at longitude `lon`.
fn ecliptic_dir(lon: f32, obliquity: f32) -> Vec3 {
    let (s, c) = lon.sin_cos();
    let (so, co) = obliquity.sin_cos();
    Vec3::new(c, s * so, s * co)
}

/// A closed daily orbital ring at declination `decl` and `radius`, centered on the
/// planet (the origin).
fn orbit_ring(decl: f32, radius: f32) -> Vec<(Vec3, Vec3)> {
    let mut segs = Vec::with_capacity(RING_STEPS);
    let mut prev = axis_orbit(0.0, decl) * radius;
    for i in 1..=RING_STEPS {
        let p = axis_orbit(i as f32 / RING_STEPS as f32 * 24.0, decl) * radius;
        segs.push((prev, p));
        prev = p;
    }
    segs
}

/// A small 3-axis cross marking a point — a body's current position on its ring, or
/// a star in a constellation.
pub fn cross_marker(center: Vec3, half: f32) -> [(Vec3, Vec3); 3] {
    [
        (center - Vec3::X * half, center + Vec3::X * half),
        (center - Vec3::Y * half, center + Vec3::Y * half),
        (center - Vec3::Z * half, center + Vec3::Z * half),
    ]
}

/// One of the other worlds, riding the ecliptic. Geocentric here — they orbit the
/// planet at the origin. `orbit` is a multiple of the planet's render radius;
/// `period` (years) sets the angular speed; `phase` offsets the start; plus a
/// billboard `color` and `size` (world units).
pub struct Planet {
    pub orbit: f32,
    pub period: f32,
    pub phase: f32,
    pub color: [f32; 4],
    pub size: f32,
}

/// Six neighbour worlds — inner rocky → outer gas-giant hues and sizes.
pub const PLANETS: [Planet; 6] = [
    Planet { orbit: 1.6, period: 0.24, phase: 0.0, color: [0.80, 0.72, 0.62, 1.0], size: 10.0 },
    Planet { orbit: 1.95, period: 0.62, phase: 1.1, color: [0.98, 0.90, 0.66, 1.0], size: 12.0 },
    Planet { orbit: 2.35, period: 1.88, phase: 2.4, color: [0.90, 0.38, 0.24, 1.0], size: 11.0 },
    Planet { orbit: 2.9, period: 11.9, phase: 3.9, color: [0.86, 0.74, 0.56, 1.0], size: 18.0 },
    Planet { orbit: 3.45, period: 29.5, phase: 5.0, color: [0.90, 0.82, 0.54, 1.0], size: 16.0 },
    Planet { orbit: 4.1, period: 84.0, phase: 0.7, color: [0.58, 0.86, 0.88, 1.0], size: 14.0 },
];

/// Integer hash → `[0, 1)`. Deterministic and stateless — a stand-in RNG so the
/// generated star map is identical every run.
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28).wrapping_add(4))) ^ x).wrapping_mul(277_803_737);
    x = (x >> 22) ^ x;
    (x & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// Deterministically generate 14 constellations of bright stars on the unit sphere —
/// each a cluster of 5–7 stars returned in draw order (connect consecutive stars to
/// trace the figure). ~84 stars total. Generate once and cache; the viewer projects
/// each direction onto a distant sphere around the planet.
pub fn generate_constellations() -> Vec<Vec<Vec3>> {
    let mut out = Vec::with_capacity(14);
    for c in 0..14u32 {
        let theta = hash01(c * 13 + 1) * TAU;
        let z = hash01(c * 13 + 2) * 1.6 - 0.8;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let center = Vec3::new(r * theta.cos(), r * theta.sin(), z);
        let aux = if center.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let tx = aux.cross(center).normalize();
        let ty = center.cross(tx);
        let n = 5 + (hash01(c * 13 + 3) * 3.0) as usize; // 5..7 stars
        let mut stars = Vec::with_capacity(n);
        for s in 0..n {
            let h1 = hash01(c * 211 + s as u32 * 3 + 7);
            let h2 = hash01(c * 211 + s as u32 * 3 + 8);
            let spread = 0.22;
            let p = center + tx * ((h1 - 0.5) * spread) + ty * ((h2 - 0.5) * spread);
            stars.push(p.normalize());
        }
        out.push(stars);
    }
    out
}

/// A small white soft-disc RGBA texture (radial alpha falloff), tinted per planet to
/// draw the pinhead planet billboards.
pub fn disc_texture() -> Vec<u8> {
    const S: usize = 16;
    let mut px = vec![0u8; S * S * 4];
    let c = (S as f32 - 1.0) * 0.5;
    for y in 0..S {
        for x in 0..S {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt() / c; // 0 centre … 1 edge
            let a = 1.0 - smoothstep(0.65, 1.0, d); // soft round edge
            let i = (y * S + x) * 4;
            px[i] = 255;
            px[i + 1] = 255;
            px[i + 2] = 255;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}

/// A unit direction orbiting the polar axis (`+Y`): `hour` (`0..24`) is the longitude
/// around the equatorial (XZ) plane, lifted toward the pole by `decl` radians of
/// declination. The result is always normalized (`cos²decl·(s²+c²) + sin²decl = 1`).
fn axis_orbit(hour: f32, decl: f32) -> Vec3 {
    let (s, c) = ((hour / 24.0) * TAU).sin_cos();
    let cd = decl.cos();
    Vec3::new(c * cd, decl.sin(), s * cd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// The headline: advancing the day rotates the sun about the polar axis, so the
    /// terminator sweeps. The direction stays unit and its declination (Y) is fixed
    /// by the season, not the time of day.
    #[test]
    fn day_sweeps_the_sun_around_the_axis() {
        let mut c = CelestialState {
            year_month: 0.0, // equinox: sun rides the equatorial plane
            ..CelestialState::default()
        };
        c.time_of_day = 0.0;
        let a = c.sun_dir();
        c.time_of_day = 6.0; // quarter day → quarter turn
        let b = c.sun_dir();

        assert!(approx(a.length(), 1.0) && approx(b.length(), 1.0), "sun dir is unit");
        assert!(approx(a.y, 0.0) && approx(b.y, 0.0), "equinox sun stays on the equatorial plane");
        // A quarter turn about Y: x and z swap roles, so the directions are distinct
        // and (here) orthogonal.
        assert!(a.dot(b).abs() < 1e-3, "a quarter day is a quarter turn");
    }

    /// The season tilts the sun toward a pole: equinox at the equator, solstices at
    /// `±axial_tilt`.
    #[test]
    fn season_sets_the_declination() {
        let base = CelestialState::default();
        let tilt = base.axial_tilt;

        let equinox = CelestialState { year_month: 0.0, ..CelestialState::default() };
        assert!(approx(equinox.sun_dir().y, 0.0), "equinox sun on the equator");

        let summer = CelestialState { year_month: 3.0, ..CelestialState::default() };
        assert!(approx(summer.sun_dir().y, tilt.sin()), "midsummer sun lifted +tilt");

        let winter = CelestialState { year_month: 9.0, ..CelestialState::default() };
        assert!(approx(winter.sun_dir().y, -tilt.sin()), "midwinter sun dipped -tilt");
    }

    /// `update` advances the clock at the sim speed and wraps the day at 24h.
    #[test]
    fn update_advances_and_wraps_the_day() {
        let mut c = CelestialState { time_of_day: 23.0, sim_speed: 60.0, ..CelestialState::default() };
        // 60 sim-min/s × 2s = 120 sim-min = 2 sim-hours: 23 → 1 (wrapped).
        c.update(Duration::from_secs(2));
        assert!(approx(c.time_of_day, 1.0), "day wraps past midnight, got {}", c.time_of_day);

        let mut paused = CelestialState { sim_speed: 0.0, ..CelestialState::default() };
        let before = paused.time_of_day;
        paused.update(Duration::from_secs(10));
        assert!(approx(paused.time_of_day, before), "paused clock does not move");
    }

    /// The frame lighting is finite, the sun is on, and both bodies are normalized.
    #[test]
    fn lighting_is_finite_with_the_sun_on() {
        let s = CelestialState::default().lighting();
        assert!(approx(s.sun_dir.length(), 1.0), "sun dir normalized");
        assert!(approx(s.moon_dir.length(), 1.0), "moon dir normalized");
        assert!(s.sun_color.length() > 0.5, "sun is on");
        for v in [s.sun_dir, s.sun_color, s.moon_dir, s.moon_color, s.ambient, s.sky_zenith, s.sky_horizon] {
            assert!(v.is_finite(), "lighting vectors finite");
        }
    }

    /// The moon brightens from new (dark) to full (bright) with its phase.
    #[test]
    fn moon_brightens_from_new_to_full() {
        let new = CelestialState { moon_phase: 0.0, ..CelestialState::default() };
        let full = CelestialState { moon_phase: 2.0, ..CelestialState::default() };
        assert!(new.moon_illumination() < 1e-3, "new moon is unlit");
        assert!(approx(full.moon_illumination(), 1.0), "full moon fully lit");
    }

    /// At a true alignment — a new moon (phase 0) on an equinox (season 0), where the
    /// moon's disc sits over the sun's — the eclipse kills the direct sun; away from
    /// that alignment the sun is full.
    #[test]
    fn eclipse_only_at_a_new_moon_equinox() {
        let advent = CelestialState { year_month: 0.0, moon_phase: 0.0, ..CelestialState::default() };
        assert!(advent.eclipse() > 0.99, "new moon at equinox covers the sun");
        assert!(advent.lighting().sun_color.length() < 1e-3, "eclipse kills the direct sun");

        let full = CelestialState { year_month: 0.0, moon_phase: 2.0, ..CelestialState::default() };
        assert!(full.eclipse() < 1e-3, "a full moon sits opposite the sun — no eclipse");
        assert!(full.lighting().sun_color.length() > 0.5, "sun is full away from alignment");

        // A new moon off the equinox: the moon's seasonal lean pulls the discs apart.
        let solstice_new = CelestialState { year_month: 3.0, moon_phase: 0.0, ..CelestialState::default() };
        assert!(solstice_new.eclipse() < 1e-3, "no eclipse when the season tilts the discs apart");
    }

    /// Orbital rings are closed loops of unit-direction points scaled to the radius.
    #[test]
    fn orbital_rings_sit_on_their_radius() {
        let c = CelestialState::default();
        for ring in [c.sun_ring(230.0), c.moon_ring(270.0), c.ecliptic_ring(500.0)] {
            assert!(!ring.is_empty(), "ring has segments");
            for &(a, b) in &ring {
                assert!(a.is_finite() && b.is_finite(), "ring points finite");
            }
            // First/last points lie on the sphere of the requested radius.
            let r = ring[0].0.length();
            assert!(r > 1.0, "ring scaled out from the origin ({r})");
        }
        assert!(approx(c.sun_ring(230.0)[0].0.length(), 230.0), "sun ring on its radius");
    }

    /// The planets ride the ecliptic (unit directions) and sweep as the clock turns.
    #[test]
    fn planets_orbit_and_advance() {
        assert_eq!(PLANETS.len(), 6, "six neighbour worlds");
        let mut c = CelestialState::default();
        let p = &PLANETS[3];
        let a = c.planet_dir(p);
        assert!(approx(a.length(), 1.0), "planet direction is unit");
        c.epoch += 5.0; // five years on
        let b = c.planet_dir(p);
        assert!(a.distance(b) > 1e-3, "the planet moved along its orbit");
    }

    /// The year rollover bumps the continuous epoch clock (so planet orbits don't snap).
    #[test]
    fn year_rollover_bumps_the_epoch() {
        let mut c = CelestialState { year_month: 11.9, epoch: 0.0, sim_speed: 6000.0, ..CelestialState::default() };
        let before = c.epoch;
        c.update(Duration::from_secs(2)); // enough sim-days to wrap the year
        assert!(c.epoch > before, "crossing the year boundary advances the epoch");
    }

    /// The constellation map is deterministic: 14 figures of unit-direction stars.
    #[test]
    fn constellations_are_unit_figures() {
        let cons = generate_constellations();
        assert_eq!(cons.len(), 14, "fourteen figures");
        let total: usize = cons.iter().map(|f| f.len()).sum();
        assert!(total >= 70, "enough stars to read as figures, got {total}");
        for fig in &cons {
            assert!(fig.len() >= 5, "each figure has several stars");
            for s in fig {
                assert!(approx(s.length(), 1.0), "stars are unit directions");
            }
        }
    }
}
