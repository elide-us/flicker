//! The `Sim` scene: the cinematic camera flies in from outside a dissipating dust
//! cloud, which clears (inside-out + annular gaps at each orbit) to reveal the
//! fixed Prism system — the sun, eight planets, and Home's moon, all slowly
//! orbiting. A flicker-shell client: Esc opens the pause menu.

use std::time::Duration;

use flicker::app::{AbstractControls, Action, GamepadConfig, InputMap, InputState, Key};
use flicker::render::{
    Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, SceneLighting, Vec2, Vec3,
    VolumetricDisk, MAX_VOLUMETRIC_BODIES,
};
use flicker::scene::{Scene, Transition};
use flicker_shell::{PauseScene, Theme};
use flicker_flight::{Flight, FlightPlayer};

use crate::camera::OrbitCam;
use crate::system::{self, Planet, SYSTEM_INNER, SYSTEM_OUTER};

const TEXT: [f32; 4] = [0.90, 0.92, 0.97, 1.0];
const DIM: [f32; 4] = [0.62, 0.66, 0.78, 1.0];

/// The bundled intro cinematic (an authored `.flight`), loaded at runtime so it
/// can be retuned in the file without recompiling.
const INTRO_FLIGHT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Alpha/content/flights/intro.flight");

/// The moon orbits Home at this multiple of Home's radius, at this angular speed.
const MOON_ORBIT_MULT: f32 = 2.6;
const MOON_OMEGA: f32 = 0.9;
const MOON_INCL: f32 = 0.45;
const MOON_RADIUS: f32 = 0.11;
const MOON_COLOR: [f32; 3] = [0.66, 0.68, 0.72];

pub struct Sim {
    cam: OrbitCam,
    planets: Vec<Planet>,
    /// One sphere mesh per planet (index-aligned with `planets`), uploaded once.
    planet_meshes: Vec<MeshHandle>,
    moon_mesh: Option<MeshHandle>,
    ring_mesh: Option<MeshHandle>,
    /// The intro cinematic, played by the flicker-flight service — it drives the
    /// camera pose and the dust-clearing clock (its `progress()`).
    flight: FlightPlayer,
    /// While true the flight is choreographing the camera; the first drag hands
    /// manual orbit control back. Space re-arms it.
    cinematic: bool,
    /// Free-running clock (seconds) driving the planets' orbital motion.
    anim_time: f32,
    // Shell pause plumbing.
    bindings: InputMap,
    menu_prev: bool,
    space_prev: bool,
    theme: Option<Theme>,
}

impl Sim {
    pub fn new() -> Self {
        let flight = Flight::load(INTRO_FLIGHT)
            .unwrap_or_else(|e| panic!("loading bundled intro flight {INTRO_FLIGHT}: {e:#}"));
        Self {
            cam: OrbitCam::new(SYSTEM_OUTER),
            planets: system::roster(),
            planet_meshes: Vec::new(),
            moon_mesh: None,
            ring_mesh: None,
            flight: FlightPlayer::new(flight),
            cinematic: true,
            anim_time: 0.0,
            bindings: InputMap::wasd_and_mouse(),
            menu_prev: false,
            space_prev: false,
            theme: None,
        }
    }

    /// Restart the cinematic fly-in from the opening pose.
    fn replay(&mut self) {
        self.flight.restart();
        self.cinematic = true;
    }

    /// Configure the volumetric dust cloud for this frame: the disk geometry, the
    /// formation clock (inside-out dissipation), and an annular gap at each
    /// planet's orbit (the "clearing" — cosmetic, no accounting).
    fn set_dust(&self, renderer: &mut Renderer) {
        let mut gaps: Vec<(f32, f32)> = self
            .planets
            .iter()
            .map(|p| {
                let width = (0.5 + p.orbit * 0.06).min(1.6);
                (p.orbit, width)
            })
            .collect();
        gaps.truncate(MAX_VOLUMETRIC_BODIES);
        renderer.set_volumetric_disk(VolumetricDisk {
            inner: SYSTEM_INNER,
            outer: SYSTEM_OUTER,
            snow_line: 6.0, // a visual density feature, not a physics boundary here
            scale_height: 0.07,
            density: 2.7, // heavier → darker, more occluding dust
            formation: self.flight.progress(),
            time: self.flight.progress() * 10.0, // a few inner-disk rotations of swirl over the fly-in
            tint: Vec3::new(0.038, 0.033, 0.052), // darker dust
            glow: Vec3::new(0.70, 0.38, 0.20),    // dimmer warm centre
            gaps,
        });
    }

    /// The Home planet (and its live world position at `anim_time`), for the moon.
    fn home_pos(&self) -> Option<(f32, Vec3)> {
        self.planets
            .iter()
            .find(|p| p.moon)
            .map(|p| (p.radius, system::planet_pos(p, self.anim_time)))
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

/// A closed ring of `segs` line segments in the disk plane (XZ) at `radius` — the
/// faint orbit-reference circles.
fn orbit_circle(radius: f32, segs: usize) -> Vec<(Vec3, Vec3)> {
    use std::f32::consts::TAU;
    let p = |i: usize| {
        let a = i as f32 / segs as f32 * TAU;
        Vec3::new(radius * a.cos(), 0.0, radius * a.sin())
    };
    (0..segs).map(|i| (p(i), p(i + 1))).collect()
}

/// A gentle per-planet tilt for its ring plane so rings read as tilted discs.
fn ring_tilt() -> Mat4 {
    Mat4::from_rotation_x(0.42) * Mat4::from_rotation_z(0.14)
}

impl Scene for Sim {
    fn enter(&mut self, renderer: &mut Renderer) {
        renderer.clear_color = [0.006, 0.008, 0.014, 1.0]; // deep space

        // One sphere per planet, coloured by its school; the sun's point light
        // shades each from its own direction to the origin (correct terminators).
        self.planet_meshes = self
            .planets
            .iter()
            .map(|p| {
                let (v, i) = system::uv_sphere(p.color, 40, 24);
                renderer.upload_mesh(&v, MeshIndices::U32(&i))
            })
            .collect();

        let (mv, mi) = system::uv_sphere(MOON_COLOR, 24, 16);
        self.moon_mesh = Some(renderer.upload_mesh(&mv, MeshIndices::U32(&mi)));

        // A unit ring annulus (radii in planet-radii); tinted per planet at draw time.
        let (rv, ri) = system::ring_mesh(1.35, 2.05, 72, 9);
        self.ring_mesh = Some(renderer.upload_mesh(&rv, MeshIndices::U32(&ri)));

        // Gothic theme for the shell pause overlay we push on Esc.
        self.theme = Some(Theme::build(renderer));
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        // Esc / Menu → push the shell pause overlay (edge-detected).
        let menu_down = self.bindings.action_pressed(Action::Menu, input);
        let menu_pressed = menu_down && !self.menu_prev;
        self.menu_prev = menu_down;
        if menu_pressed {
            let theme = self.theme.expect("theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &self.bindings,
                &AbstractControls::default(),
                &GamepadConfig::default(),
            )));
        }

        // Space replays the fly-in from the top.
        let space = input.key_down(Key::Space);
        if space && !self.space_prev {
            self.replay();
        }
        self.space_prev = space;

        // Camera: the flight drives the pose until the first drag; a drag hands
        // manual orbit control back. The flight advances only while it's driving.
        let dts = dt.as_secs_f32();
        self.cam.update(input, !self.cinematic);
        if self.cinematic {
            if input.mouse_left {
                self.cinematic = false;
            } else {
                let p = self.flight.advance(dts);
                self.cam.set_pose(p.yaw, p.pitch, p.distance);
            }
        }

        // The planets orbit on their own free-running clock (independent of the
        // camera flight).
        self.anim_time += dts;
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&self.cam.camera());

        // Deep-space galactic background: the sky pass renders a Milky Way band +
        // star field at "night", so we push the sun *and* moon lights below the
        // horizon (no discs) and set a near-black gradient. The dust composites
        // over it — dense dust occludes the stars into dark lanes.
        renderer.draw_sky();
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.0, -1.0, 0.0),
            sun_color: Vec3::ZERO,
            moon_dir: Vec3::new(0.0, -1.0, 0.0),
            moon_color: Vec3::ZERO,
            // The sun is a **point light at the origin**: every planet mesh is
            // shaded per-fragment from its own direction to it, over a faint
            // ambient floor so night sides aren't pure black.
            ambient: Vec3::splat(0.07),
            point_pos: Vec3::ZERO,
            point_color: Vec3::new(1.0, 0.94, 0.84), // warm starlight
            sky_zenith: Vec3::new(0.004, 0.006, 0.014),
            sky_horizon: Vec3::new(0.007, 0.010, 0.022),
            ..SceneLighting::default()
        });

        // The dust cloud (the sun is rendered *inside* this pass so the dust
        // occludes it — no separate star billboard, which would draw on top).
        self.set_dust(renderer);

        // Faint orbit-reference circles.
        for p in &self.planets {
            renderer.draw_lines(&orbit_circle(p.orbit, 128), [0.30, 0.36, 0.52, 0.16]);
        }

        // The planets: each a school-coloured sphere on its circular orbit, lit by
        // the sun point light. Air wears a tinted ring; Death stays near-black
        // (occulted). Home's moon rides a tilted orbit around it.
        let ring_mesh = self.ring_mesh;
        let moon_mesh = self.moon_mesh;
        for (p, &mesh) in self.planets.iter().zip(self.planet_meshes.iter()) {
            let pos = system::planet_pos(p, self.anim_time);
            let model = Mat4::from_translation(pos) * Mat4::from_scale(Vec3::splat(p.radius));
            renderer.draw_mesh(mesh, model, MeshDrawOptions::default());

            if p.rings {
                if let Some(rh) = ring_mesh {
                    let tint = [0.85, 0.78, 0.42, 1.0]; // Air's warm ring
                    let rmodel =
                        Mat4::from_translation(pos) * ring_tilt() * Mat4::from_scale(Vec3::splat(p.radius));
                    renderer.draw_mesh(rh, rmodel, MeshDrawOptions { tint, ..Default::default() });
                }
            }
        }

        // Home's moon.
        if let (Some((home_r, home_pos)), Some(mmesh)) = (self.home_pos(), moon_mesh) {
            let a = self.anim_time * MOON_OMEGA;
            let orbit_r = home_r * MOON_ORBIT_MULT;
            let off = Vec3::new(
                orbit_r * a.cos(),
                orbit_r * MOON_INCL.sin() * a.sin(),
                orbit_r * MOON_INCL.cos() * a.sin(),
            );
            let model =
                Mat4::from_translation(home_pos + off) * Mat4::from_scale(Vec3::splat(MOON_RADIUS));
            renderer.draw_mesh(mmesh, model, MeshDrawOptions::default());
        }

        self.hud(renderer);
    }
}

impl Sim {
    fn hud(&self, renderer: &mut Renderer) {
        renderer.draw_text("flicker · solarbirth", Vec2::new(16.0, 16.0), 24.0, TEXT);
        let seg = self.flight.segment_name();
        let phase = if self.flight.progress() >= 1.0 {
            format!("{seg} · settled")
        } else {
            format!("{seg} · approaching {:.0}%", self.flight.progress() * 100.0)
        };
        renderer.draw_text(
            &format!("the Prism system · {phase}"),
            Vec2::new(16.0, 50.0),
            16.0,
            [0.78, 0.74, 0.92, 1.0],
        );
        renderer.draw_text(
            "drag rotate · wheel zoom · Space replay fly-in · Esc pause",
            Vec2::new(16.0, 74.0),
            13.0,
            DIM,
        );

        // Roster legend, inner → outer.
        let mut y = 104.0;
        renderer.draw_text("planets (inner → outer):", Vec2::new(16.0, y), 14.0, TEXT);
        y += 22.0;
        for p in &self.planets {
            let mut tags = Vec::new();
            if p.moon {
                tags.push("moon");
            }
            if p.rings {
                tags.push("rings");
            }
            if p.occulted {
                tags.push("occulted");
            }
            let suffix = if tags.is_empty() {
                String::new()
            } else {
                format!("  ({})", tags.join(", "))
            };
            renderer.draw_text(
                &format!("{}{suffix}", p.name),
                Vec2::new(24.0, y),
                13.0,
                [p.color[0].max(0.35), p.color[1].max(0.35), p.color[2].max(0.35), 1.0],
            );
            y += 18.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::INTRO_FLIGHT;

    /// The bundled intro cinematic must parse — guards the asset at test time so a
    /// typo doesn't surface only as a runtime panic when the scene starts.
    #[test]
    fn bundled_intro_flight_loads() {
        let f = flicker_flight::Flight::load(INTRO_FLIGHT).expect("intro.flight parses");
        assert_eq!(f.segments.len(), 2, "glide + coast");
        assert!(f.loops(), "the coast tail loops");
    }
}
