//! Minimal celestial model for the epoch viewer: a sun that **orbits the planet** to
//! drive the day/night terminator — i.e. the heat map sweeping across the surface.
//!
//! Nothing more lives here on purpose. The richer solar-system work (heliocentric
//! orbits, sibling worlds, accretion-disk formation) is its own POC
//! (`examples/flicker-solarsystem`); flicker-world is the focused epoch viewer.

use std::f32::consts::TAU;
use std::time::Duration;

use flicker::render::{Mat4, SceneLighting, Vec3};

/// Real-seconds for one full simulated day at the default sim speed.
const DEFAULT_DAY_SECONDS: f32 = 24.0;
/// Default obliquity (degrees) — tilts the sun's seasonal path off the equator.
const DEFAULT_AXIAL_TILT_DEG: f32 = 23.5;

/// The sun's cycle around the planet. `time_of_day` is its longitude around the polar
/// axis; `year_month` (via `axial_tilt`) its seasonal declination. Advanced by
/// [`Self::update`], read by [`Self::lighting`].
pub struct CelestialState {
    /// Time of day, hours `0..24` — the sun's longitude.
    pub time_of_day: f32,
    /// Season, months `0..12` — the sun's declination via the axial tilt.
    pub year_month: f32,
    /// Obliquity in radians.
    pub axial_tilt: f32,
    /// Auto-advance rate, simulated minutes per real second. `0.0` = paused.
    pub sim_speed: f32,
}

impl Default for CelestialState {
    fn default() -> Self {
        Self {
            time_of_day: 8.0,
            year_month: 3.0,
            axial_tilt: DEFAULT_AXIAL_TILT_DEG.to_radians(),
            sim_speed: 24.0 * 60.0 / DEFAULT_DAY_SECONDS,
        }
    }
}

impl CelestialState {
    /// Advance the sun's cycle by real-time `dt` (no-op while paused).
    pub fn update(&mut self, dt: Duration) {
        if self.sim_speed <= 0.0 {
            return;
        }
        let d_min = dt.as_secs_f32() * self.sim_speed;
        self.time_of_day = (self.time_of_day + d_min / 60.0).rem_euclid(24.0);
        self.year_month = (self.year_month + d_min / (24.0 * 60.0) / 30.0).rem_euclid(12.0);
    }

    /// The sun's declination off the equatorial plane for the season: `0` at the
    /// equinoxes (`year_month` 0 / 6), `±axial_tilt` at the solstices.
    fn declination(&self) -> f32 {
        self.axial_tilt * ((self.year_month / 12.0) * TAU).sin()
    }

    /// Direction *toward* the sun — orbits the polar axis (`+Y`) with the day, lifted
    /// off the equator by the season. The mesh shader's `dot(normal, sun_dir)` makes
    /// the terminator (the heat band) sweep the surface as this advances.
    pub fn sun_dir(&self) -> Vec3 {
        let (s, c) = (((self.time_of_day - 6.0) / 24.0) * TAU).sin_cos();
        let decl = self.declination();
        let cd = decl.cos();
        Vec3::new(c * cd, decl.sin(), s * cd)
    }

    /// This frame's [`SceneLighting`]: the orbiting sun, a low ambient floor so the
    /// night side stays legible, and a dark "space" sky for the procedural backdrop.
    pub fn lighting(&self) -> SceneLighting {
        SceneLighting {
            sun_dir: self.sun_dir(),
            sun_color: Vec3::new(1.0, 0.97, 0.92),
            ambient: Vec3::new(0.06, 0.07, 0.10),
            sky_zenith: Vec3::new(0.012, 0.016, 0.030),
            sky_horizon: Vec3::new(0.030, 0.040, 0.085),
            star_rotation: Mat4::IDENTITY,
            ..SceneLighting::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Advancing the day rotates the sun about the polar axis (the terminator sweeps);
    /// at an equinox it stays on the equatorial plane.
    #[test]
    fn sun_orbits_with_the_day() {
        let mut c = CelestialState { year_month: 0.0, ..CelestialState::default() };
        c.time_of_day = 0.0;
        let a = c.sun_dir();
        c.time_of_day = 6.0;
        let b = c.sun_dir();
        assert!(approx(a.length(), 1.0) && approx(b.length(), 1.0), "sun dir is unit");
        assert!(approx(a.y, 0.0) && approx(b.y, 0.0), "equinox sun on the equatorial plane");
        assert!(a.dot(b).abs() < 1e-3, "a quarter day is a quarter turn");
    }

    /// The season lifts the sun off the equator by the obliquity.
    #[test]
    fn season_sets_the_declination() {
        let tilt = DEFAULT_AXIAL_TILT_DEG.to_radians();
        let equinox = CelestialState { year_month: 0.0, ..CelestialState::default() };
        assert!(approx(equinox.sun_dir().y, 0.0), "equinox on the equator");
        let summer = CelestialState { year_month: 3.0, ..CelestialState::default() };
        assert!(approx(summer.sun_dir().y, tilt.sin()), "midsummer lifted +tilt");
        let winter = CelestialState { year_month: 9.0, ..CelestialState::default() };
        assert!(approx(winter.sun_dir().y, -tilt.sin()), "midwinter dipped -tilt");
    }

    /// `update` advances the clock at the sim speed and wraps the day at 24h.
    #[test]
    fn update_advances_and_wraps_the_day() {
        let mut c = CelestialState { time_of_day: 23.0, sim_speed: 60.0, ..CelestialState::default() };
        c.update(Duration::from_secs(2)); // 2 sim-hours: 23 → 1
        assert!(approx(c.time_of_day, 1.0), "day wraps past midnight, got {}", c.time_of_day);

        let mut paused = CelestialState { sim_speed: 0.0, ..CelestialState::default() };
        let before = paused.time_of_day;
        paused.update(Duration::from_secs(10));
        assert!(approx(paused.time_of_day, before), "paused clock does not move");
    }

    /// The frame lighting is finite with the sun on.
    #[test]
    fn lighting_is_finite_with_the_sun_on() {
        let s = CelestialState::default().lighting();
        assert!(approx(s.sun_dir.length(), 1.0), "sun dir normalized");
        assert!(s.sun_color.length() > 0.5, "sun is on");
        for v in [s.sun_dir, s.sun_color, s.ambient, s.sky_zenith, s.sky_horizon] {
            assert!(v.is_finite(), "lighting vectors finite");
        }
    }
}
