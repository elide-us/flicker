//! HexWorld — flat local-bubble viewer (per the Flat Neighbor Graph & Celestial
//! Orientation spec). **No sphere in the data or the render.**
//!
//! - Data: a flat [`HexMap`] graph with baked edge-neighbour refs ([`topology`]).
//! - Render: a flat bubble — the standpoint hex at the origin plus its 6
//!   graph-attached neighbours, drawn flat. Crossing an edge re-roots the bubble
//!   by graph adjacency (despawn ~3, spawn ~3); nothing assembles or reconciles.
//! - Celestial: the *only* sphere consumer — the standpoint's read-only
//!   orientation eases the sky on each crossing (compass lerp).
//!
//! Controls: W/S move forward/back, A/D turn, Esc quit. Cell index billboards
//! float above each hex.

mod planet_mesh;
mod topology;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, Bindings, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, Renderer, SceneLighting, Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};

use planet_mesh::{build_flat_hex, edge_offsets};
use topology::HexMap;

const RINGS: u32 = 5;
const MOVE_SPEED: f32 = 140.0;
const TURN_SPEED: f32 = 1.6;
/// Billboard height above the tile — clears the heightmap relief.
const LABEL_Y: f32 = 80.0;

struct HexWorld {
    map: HexMap,
    standpoint: u32,
    /// Player position in the bubble, relative to the standpoint at the origin.
    player: Vec2,
    /// Facing yaw: 0 = +Z. A/D turn it; W/S move along it.
    heading: f32,
    /// Bubble tiles: (hex index, flat placement, mesh).
    tiles: Vec<(u32, Vec2, MeshHandle)>,
    bindings: Bindings,
    /// Eased celestial sky direction (compass lerp).
    sky: Vec3,
}

impl HexWorld {
    fn new() -> Self {
        Self {
            map: HexMap::new(RINGS),
            standpoint: 0,
            player: Vec2::ZERO,
            heading: 0.0,
            tiles: Vec::new(),
            bindings: Bindings::wasd(),
            sky: Vec3::Y,
        }
    }

    /// Rebuild the bubble: standpoint at origin + its 6 graph neighbours at the
    /// fixed flat edge offsets, sampled with a per-standpoint seed.
    fn rebuild(&mut self, renderer: &mut Renderer) {
        for (_, _, h) in self.tiles.drain(..) {
            renderer.free_mesh(h);
        }
        let sx = self.standpoint as f32 * 173.0;
        let sz = self.standpoint as f32 * 313.0;
        self.push(renderer, self.standpoint, Vec2::ZERO, sx, sz);
        let refs = self.map.edge_refs(self.standpoint);
        for (e, (ox, oz)) in edge_offsets().into_iter().enumerate() {
            self.push(renderer, refs[e], Vec2::new(ox, oz), sx, sz);
        }
    }

    fn push(&mut self, renderer: &mut Renderer, idx: u32, place: Vec2, sx: f32, sz: f32) {
        let (v, i) = build_flat_hex(place.x, place.y, sx + place.x, sz + place.y);
        let h = renderer.upload_mesh(&v, MeshIndices::U32(&i));
        self.tiles.push((idx, place, h));
    }
}

impl Scene for HexWorld {
    fn enter(&mut self, renderer: &mut Renderer) {
        println!("HexWorld: {} hexes (R={RINGS}) — flat graph", self.map.total());
        self.standpoint = 0; // north pole
        self.player = Vec2::ZERO;
        self.rebuild(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let left = input.action_active(&self.bindings, Action::StrafeLeft);
        let right = input.action_active(&self.bindings, Action::StrafeRight);
        let fwd = input.action_active(&self.bindings, Action::MoveForward);
        let back = input.action_active(&self.bindings, Action::MoveBackward);

        let dt = dt.as_secs_f32();
        self.heading += (right as i32 - left as i32) as f32 * TURN_SPEED * dt;
        let dir = Vec2::new(self.heading.sin(), self.heading.cos());
        self.player += dir * ((fwd as i32 - back as i32) as f32 * MOVE_SPEED * dt);
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Re-root by graph adjacency: nearer a neighbour centre than the
        // standpoint (origin) ⇒ crossed that edge.
        let offs = edge_offsets();
        let mut best = (self.player.length_squared(), 6usize);
        for (e, &(ox, oz)) in offs.iter().enumerate() {
            let d = (self.player - Vec2::new(ox, oz)).length_squared();
            if d < best.0 {
                best = (d, e);
            }
        }
        if best.1 != 6 {
            self.standpoint = self.map.edge_refs(self.standpoint)[best.1];
            let (ox, oz) = offs[best.1];
            self.player -= Vec2::new(ox, oz);
            self.rebuild(renderer);
        }

        // Celestial (only sphere read): ease the sky toward the standpoint.
        let target = Vec3::from_array(self.map.celestial_dir(self.standpoint));
        self.sky = (self.sky + (target - self.sky) * 0.04).normalize_or(Vec3::Y);

        // Heading-relative chase camera.
        let p3 = Vec3::new(self.player.x, 0.0, self.player.y);
        let dir = Vec2::new(self.heading.sin(), self.heading.cos());
        let fwd3 = Vec3::new(dir.x, 0.0, dir.y);
        let camera = Camera {
            position: p3 - fwd3 * 230.0 + Vec3::new(0.0, 170.0, 0.0),
            target: p3 + fwd3 * 40.0,
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 8000.0,
        };
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: self.sky,
            ..SceneLighting::default()
        });
        renderer.draw_sky();
        for &(_, _, h) in &self.tiles {
            renderer.draw_mesh(h, Mat4::IDENTITY, MeshDrawOptions::default());
        }

        // Cell-index billboards, floating above each tile.
        let size = renderer.size();
        let vp = camera.view_projection(size.x / size.y);
        renderer.set_layer(50.0);
        for &(idx, place, _) in &self.tiles {
            let world = Vec3::new(place.x, LABEL_Y, place.y);
            if let Some(s) = project_to_screen(vp, world, size) {
                let color = if idx == self.standpoint {
                    [0.4, 1.0, 1.0, 1.0]
                } else {
                    [1.0, 1.0, 1.0, 0.95]
                };
                renderer.draw_text(&idx.to_string(), s, 18.0, color);
            }
        }
    }
}

/// Project a world point to screen pixels; `None` if behind the camera.
fn project_to_screen(vp: Mat4, world: Vec3, size: Vec2) -> Option<Vec2> {
    let clip = vp * world.extend(1.0);
    if clip.w <= 0.001 {
        return None;
    }
    Some(Vec2::new(
        (clip.x / clip.w * 0.5 + 0.5) * size.x,
        (1.0 - (clip.y / clip.w * 0.5 + 0.5)) * size.y,
    ))
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(HexWorld::new())))?;
    Ok(())
}
