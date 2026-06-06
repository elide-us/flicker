//! hex-world — a local-patch viewer for the continuous hex planet.
//!
//! Concrete embodiment of the local-only render model (see
//! `docs/hex-world-handoff.md`): it **never assembles the sphere.** It renders
//! only the focus hex plus its ring of neighbours (≤7 tiles, [`LOCAL_DEPTH`]),
//! every tile **gnomonic-flattened onto the focus hex's tangent plane** — the
//! "planar snap." The focus follows your gaze; crossing a hex boundary re-snaps
//! the whole patch onto the new plane and frees/builds tiles accordingly. The
//! sphere's curvature is paid across the *sequence* of snaps, never inside one
//! patch — which is why a purely local patch is all that's ever needed.
//!
//! An equirectangular minimap (right) shows where the resident patch sits on
//! the whole planet, with array indices on the live tiles.
//!
//! Controls (orbit, keyboard only):
//!   * A / D — swing around the planet (yaw)
//!   * W / S — tilt over the poles (pitch)
//!   * R / F — zoom in / out
//!   * Esc   — quit

mod planet_mesh;
mod topology;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, Bindings, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, SceneLighting, TextureHandle,
    Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};

use planet_mesh::PLANET_RADIUS;
use topology::{Hemisphere, HexCoord, Planet};

/// Planet size. `total = 2 + 6·R·(R+1)`; R=4 → 122 hexes.
const RINGS: u32 = 4;
/// How many neighbour rings around the focus hex to show. 1 = the focus plus
/// its immediate ring — at most 7 tiles, the gameplay-scale local patch.
const LOCAL_DEPTH: u32 = 1;

struct PlanetScene {
    planet: Planet,
    /// Currently-built local tiles, keyed by hex index — the streaming cache.
    local: HashMap<u32, MeshHandle>,
    /// Hex the camera is currently over; drives which tiles are resident.
    focus: u32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    bindings: Bindings,
    white: Option<TextureHandle>,
}

impl PlanetScene {
    fn new() -> Self {
        Self {
            planet: Planet::new(RINGS),
            local: HashMap::new(),
            focus: 0,
            yaw: 0.0,
            pitch: 0.0,
            distance: PLANET_RADIUS * 2.4,
            bindings: Bindings::wasd(),
            white: None,
        }
    }

    /// Rebuild the resident `LOCAL_DEPTH`-ring patch flattened onto the current
    /// focus plane. Every tile reprojects when the focus moves, so this frees
    /// all and rebuilds from scratch. Needs `&mut Renderer`, so it runs from
    /// `enter`/`render`, never `update`.
    fn reconcile(&mut self, renderer: &mut Renderer) {
        for (_, handle) in self.local.drain() {
            renderer.free_mesh(handle);
        }
        let frame = planet_mesh::focus_frame(&self.planet, self.planet.coord(self.focus));
        for h in local_set(&self.planet, self.focus, LOCAL_DEPTH) {
            let (verts, indices) =
                planet_mesh::build_tile(&self.planet, self.planet.coord(h), &frame);
            let handle = renderer.upload_mesh(&verts, MeshIndices::U32(&indices));
            self.local.insert(h, handle);
        }
    }

    /// Equirectangular god-map on the right: every hex a dot, the resident
    /// patch lit and index-labelled, the focus in cyan — so you watch hexes
    /// load and unload across the planet as you fly.
    fn draw_minimap(&self, renderer: &mut Renderer) {
        let Some(white) = self.white else {
            return;
        };
        let view = renderer.size();
        let (pw, ph) = (360.0_f32, 180.0_f32);
        let px = view.x - pw - 20.0;
        let py = (view.y - ph) * 0.5;

        renderer.set_layer(100.0);
        renderer.draw_sprite(
            white,
            Vec2::new(px - 6.0, py - 26.0),
            Vec2::new(pw + 12.0, ph + 46.0),
            [0.05, 0.06, 0.09, 0.85],
        );
        // equator reference line
        renderer.draw_sprite(
            white,
            Vec2::new(px, py + ph * 0.5),
            Vec2::new(pw, 1.0),
            [0.30, 0.32, 0.42, 0.8],
        );

        let fc = self.planet.coord(self.focus);
        let hemi = if matches!(fc.hemi, Hemisphere::North) {
            'N'
        } else {
            'S'
        };
        renderer.draw_text(
            &format!(
                "HEX MAP   focus #{}  {}r{}p{}   resident {}",
                self.focus,
                hemi,
                fc.ring,
                fc.pos,
                self.local.len()
            ),
            Vec2::new(px, py - 22.0),
            13.0,
            [0.90, 0.95, 1.0, 1.0],
        );

        for i in 0..self.planet.total_hexes() {
            let c = self.planet.coord(i);
            let lon = self.planet.longitude_center_deg(c).unwrap_or(180.0);
            let lat = self.planet.latitude_deg(c);
            let mx = px + (lon / 360.0) * pw;
            let my = py + (1.0 - (lat + 90.0) / 180.0) * ph;
            let resident = self.local.contains_key(&i);
            let (sz, color) = if i == self.focus {
                (8.0, [0.30, 1.0, 1.0, 1.0])
            } else if resident {
                (6.0, [1.0, 0.80, 0.35, 1.0])
            } else {
                (3.0, [0.35, 0.38, 0.45, 0.85])
            };
            renderer.draw_sprite(
                white,
                Vec2::new(mx - sz * 0.5, my - sz * 0.5),
                Vec2::splat(sz),
                color,
            );
            if resident {
                renderer.draw_text(
                    &i.to_string(),
                    Vec2::new(mx + 5.0, my - 6.0),
                    10.0,
                    [1.0, 1.0, 1.0, 0.95],
                );
            }
        }
    }
}

impl Scene for PlanetScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        // Start over a mid-latitude hex (off the poles and seams).
        self.focus = self.planet.index(HexCoord {
            hemi: Hemisphere::North,
            ring: 2,
            pos: 0,
        });
        let dir = Vec3::from_array(self.planet.unit_position(self.planet.coord(self.focus)));
        self.pitch = dir.y.clamp(-1.0, 1.0).asin();
        self.yaw = dir.x.atan2(dir.z);
        self.reconcile(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let sr = input.action_active(&self.bindings, Action::StrafeRight);
        let sl = input.action_active(&self.bindings, Action::StrafeLeft);
        let mf = input.action_active(&self.bindings, Action::MoveForward);
        let mb = input.action_active(&self.bindings, Action::MoveBackward);
        let mu = input.action_active(&self.bindings, Action::MoveUp);
        let md = input.action_active(&self.bindings, Action::MoveDown);
        let f = |b: bool| if b { 1.0 } else { 0.0 };

        let dt = dt.as_secs_f32();
        let rot = 1.0 * dt;
        self.yaw += (f(sr) - f(sl)) * rot;
        self.pitch = (self.pitch + (f(mf) - f(mb)) * rot).clamp(-1.4, 1.4);
        let zoom = 1.0 + (f(md) - f(mu)) * 1.5 * dt;
        self.distance = (self.distance * zoom).clamp(PLANET_RADIUS * 1.4, PLANET_RADIUS * 4.0);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let mut camera = Camera::orbit(Vec3::ZERO, self.distance, self.yaw, self.pitch);

        // The hex under the camera is the focus; reproject the patch if it moved.
        let focus = nearest_hex(&self.planet, camera.position.normalize());
        if focus != self.focus {
            self.focus = focus;
            self.reconcile(renderer);
        }

        // Flatten onto — and aim the camera at — the focus hex's plane.
        let frame = planet_mesh::focus_frame(&self.planet, self.planet.coord(self.focus));
        camera.target = frame.center;
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting::default());
        renderer.draw_sky();

        for (&h, &handle) in &self.local {
            let options = MeshDrawOptions {
                wireframe: false,
                tint: tile_tint(&self.planet, self.planet.coord(h)),
            };
            renderer.draw_mesh(handle, Mat4::IDENTITY, options);
        }

        // Hex outlines: cyan for the focus, amber for the rest.
        for &h in self.local.keys() {
            let corners = planet_mesh::hex_corners(&self.planet, self.planet.coord(h), &frame);
            let n = corners.len();
            let segments: Vec<(Vec3, Vec3)> =
                (0..n).map(|k| (corners[k], corners[(k + 1) % n])).collect();
            let color = if h == self.focus {
                [0.3, 1.0, 1.0, 1.0]
            } else {
                [1.0, 0.85, 0.4, 1.0]
            };
            renderer.draw_lines(&segments, color);
        }

        // Stamp each tile's array index on it (project its anchor to screen).
        let size = renderer.size();
        let view_proj = camera.view_projection(size.x / size.y);
        renderer.set_layer(50.0);
        for &h in self.local.keys() {
            let anchor = planet_mesh::tile_center(&self.planet, self.planet.coord(h), &frame);
            if let Some(s) = project_to_screen(view_proj, anchor, size) {
                let color = if h == self.focus {
                    [0.4, 1.0, 1.0, 1.0]
                } else {
                    [1.0, 1.0, 1.0, 0.95]
                };
                renderer.draw_text(&h.to_string(), s, 16.0, color);
            }
        }

        self.draw_minimap(renderer);
    }
}

/// Project a world point to screen pixels; `None` if behind the camera.
fn project_to_screen(view_proj: Mat4, world: Vec3, size: Vec2) -> Option<Vec2> {
    let clip = view_proj * world.extend(1.0);
    if clip.w <= 0.001 {
        return None;
    }
    let nx = clip.x / clip.w;
    let ny = clip.y / clip.w;
    Some(Vec2::new(
        (nx * 0.5 + 0.5) * size.x,
        (1.0 - (ny * 0.5 + 0.5)) * size.y,
    ))
}

/// BFS the topology out to `depth` rings from `focus`.
fn local_set(planet: &Planet, focus: u32, depth: u32) -> HashSet<u32> {
    let mut set = HashSet::from([focus]);
    let mut frontier = vec![focus];
    for _ in 0..depth {
        let mut next = Vec::new();
        for h in frontier {
            for n in planet.neighbors(h) {
                if set.insert(n) {
                    next.push(n);
                }
            }
        }
        frontier = next;
    }
    set
}

/// Hex whose center direction is closest to `dir` (the sub-camera point).
fn nearest_hex(planet: &Planet, dir: Vec3) -> u32 {
    let mut best = 0;
    let mut best_dot = f32::MIN;
    for i in 0..planet.total_hexes() {
        let d = Vec3::from_array(planet.unit_position(planet.coord(i))).dot(dir);
        if d > best_dot {
            best_dot = d;
            best = i;
        }
    }
    best
}

/// Per-hex colour: hemisphere sets the hue, ring brightens toward the equator,
/// and a parity checker keeps adjacent hexes visually distinct.
fn tile_tint(planet: &Planet, c: HexCoord) -> [f32; 4] {
    let base = match c.hemi {
        Hemisphere::North => [0.80, 0.66, 0.45],
        Hemisphere::South => [0.46, 0.60, 0.80],
    };
    let ring_f = 0.55 + 0.45 * (c.ring as f32 / planet.rings() as f32);
    let checker = if ((c.pos + c.ring) & 1) == 0 { 1.0 } else { 0.82 };
    let s = ring_f * checker;
    [base[0] * s, base[1] * s, base[2] * s, 1.0]
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(PlanetScene::new())))?;
    Ok(())
}
