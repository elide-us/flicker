//! HexWorld — **the whole flat hex graph as a living world** (POC).
//!
//! The entire hex array from [`topology`] laid out flat — no sphere projection.
//! Each hemisphere is a spiral of concentric rings: the **north pole** sits at
//! the centre of the left cluster spiralling out to its equator ring, and the
//! **south pole** at the centre of the right cluster spiralling in — exactly the
//! two-disc layout from the notebook sketch. (Cross-hemisphere "virtual"
//! adjacency lives in the graph's `edge_refs`, not in this flat picture.)
//!
//! Every hex owns a [`LayerStack`] and runs the full water cycle + climate; the
//! surface is drawn biome-coloured (the realized composite) with a drifting
//! cloud deck. Terrain is continuous within a hemisphere because each tile
//! samples its own world window of the shared heightmap.
//!
//! **Fly controls:** W/S forward/back, A/D left/right, R/F up/down, hold
//! **right mouse** to look (pitch/yaw). Esc quits.
//!
//! Reusable nucleus is in [`layers`]; the flat graph is in [`topology`].

mod layers;
#[allow(dead_code)] // parked graph: only layout uses it here; adjacency kept for later.
mod topology;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};

use layers::{LayerStack, G, HEX_HALF_W, HEX_SIZE};
use topology::{HexCoord, HexMap, Hemisphere};

const WORLD_OFFSET: Vec2 = Vec2::new(1234.0, 5678.0);
const SIM_DT: f32 = 0.05;
/// Rings per hemisphere (R=2 → 38 hexes total, matching the sketch's 0..37).
const RINGS: u32 = 2;
/// Meshes rebuild on this real-time cadence, decoupled from the 20 Hz sim, so a
/// world of tiles doesn't re-upload every frame.
const REBUILD_INTERVAL: f32 = 0.15;

const MOVE_SPEED: f32 = 950.0;
const LOOK_SENS: f32 = 0.005;
const WY_CLOUD: f32 = 120.0;

/// One hex's data and its flat-layout placement.
struct HexTile {
    place: Vec2,
    stack: LayerStack,
}

/// One uploaded mesh: handle, draw tint, model transform (places it in the map).
struct Sheet {
    handle: MeshHandle,
    tint: [f32; 4],
    model: Mat4,
}

#[inline]
fn at(field: &[f32], i: usize, j: usize) -> f32 {
    field[j * G + i]
}

/// Flat XZ offset of ring `pos` within ring `ring`, by walking the hex ring at
/// radius `ring`. Axial coords map to **flat-top** pixel space via the (NE, N)
/// basis — flat-top hexes have no E/W neighbour (those are the points), so the
/// two basis directions are NE and the vertical N.
fn ring_offset(ring: u32, pos: u32) -> Vec2 {
    if ring == 0 {
        return Vec2::ZERO;
    }
    const DIRS: [(i32, i32); 6] = [(1, 0), (0, 1), (-1, 1), (-1, 0), (0, -1), (1, -1)];
    let (mut q, mut r) = (DIRS[4].0 * ring as i32, DIRS[4].1 * ring as i32);
    let mut cells = Vec::with_capacity(6 * ring as usize);
    for &(dq, dr) in &DIRS {
        for _ in 0..ring {
            cells.push((q, r));
            q += dq;
            r += dr;
        }
    }
    let (cq, cr) = cells[pos as usize % cells.len()];
    let aq = Vec2::new(1.5 * HEX_SIZE, HEX_HALF_W); // flat-top NE (axial +q)
    let ar = Vec2::new(0.0, 2.0 * HEX_HALF_W); // flat-top N (axial +r)
    aq * cq as f32 + ar * cr as f32
}

/// Flat position of a hex: its hemisphere's cluster centre plus its ring offset.
/// North cluster sits left, south cluster right, separated so equators meet.
fn hex_flat_pos(coord: HexCoord) -> Vec2 {
    // Clusters one column-pitch apart so the equator rings meet along a vertical
    // seam. The south is shifted half a hex (one apothem) vertically so its
    // flat-top E/W points interlock with the north's like teeth — per the spec's
    // "southern hemisphere rotated half a hex".
    let sep = 1.5 * HEX_SIZE * (RINGS as f32 + 0.5);
    let center = match coord.hemi {
        Hemisphere::North => Vec2::new(-sep, 0.0),
        Hemisphere::South => Vec2::new(sep, HEX_HALF_W),
    };
    center + ring_offset(coord.ring, coord.pos)
}

fn mk(renderer: &mut Renderer, mesh: (Vec<MeshVertex>, Vec<u32>), tint: [f32; 4], model: Mat4) -> Option<Sheet> {
    let (verts, inds) = mesh;
    if inds.is_empty() {
        return None;
    }
    Some(Sheet {
        handle: renderer.upload_mesh(&verts, MeshIndices::U32(&inds)),
        tint,
        model,
    })
}

/// Every tile's biome-coloured surface + cloud deck, placed across the map.
fn build_world(tiles: &[HexTile], renderer: &mut Renderer) -> Vec<Sheet> {
    let mut out = Vec::new();
    for t in tiles {
        let s = &t.stack;
        let model = Mat4::from_translation(Vec3::new(t.place.x, 0.0, t.place.y));
        let realized = layers::build_sheet(0.0, |i, j| s.realized(i, j).0, |i, j| s.realized(i, j).1, |_, _| true);
        out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], model));
        let cloud = layers::build_sheet(WY_CLOUD, |i, j| at(&s.cloud, i, j) * 40.0, |_, _| layers::M_CLOUD, |i, j| at(&s.cloud, i, j) > 0.15);
        out.extend(mk(renderer, cloud, [1.0, 1.0, 1.0, 0.5], model));
    }
    out
}

struct World {
    tiles: Vec<HexTile>,
    sheets: Vec<Sheet>,
    sim_accum: f32,
    rebuild_accum: f32,
    dirty: bool,
    // Fly camera.
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    prev_mouse: Vec2,
    dragging: bool,
}

impl World {
    fn new() -> Self {
        let map = HexMap::new(RINGS);
        let tiles = (0..map.total())
            .map(|i| {
                let place = hex_flat_pos(map.coord(i));
                HexTile {
                    place,
                    stack: LayerStack::generate(WORLD_OFFSET + place),
                }
            })
            .collect();
        Self {
            tiles,
            sheets: Vec::new(),
            sim_accum: 0.0,
            rebuild_accum: REBUILD_INTERVAL,
            dirty: true,
            pos: Vec3::new(0.0, 1700.0, -2900.0),
            yaw: 0.0,
            pitch: -0.5,
            prev_mouse: Vec2::ZERO,
            dragging: false,
        }
    }

    fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(sy * cp, sp, cy * cp)
    }

    fn rebuild(&mut self, renderer: &mut Renderer) {
        for s in std::mem::take(&mut self.sheets) {
            renderer.free_mesh(s.handle);
        }
        self.sheets = build_world(&self.tiles, renderer);
        self.dirty = false;
    }
}

impl Scene for World {
    fn enter(&mut self, _renderer: &mut Renderer) {
        println!(
            "HexWorld flat graph: {} hexes (R={RINGS}). Fly: WASD + Space/Shift, RMB look, Esc quit.",
            self.tiles.len()
        );
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let dt = dt.as_secs_f32();

        // Look: hold right mouse, drag to pitch/yaw.
        if input.mouse_right {
            if self.dragging {
                let d = input.mouse_position - self.prev_mouse;
                self.yaw -= d.x * LOOK_SENS;
                self.pitch = (self.pitch - d.y * LOOK_SENS).clamp(-1.5, 1.5);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = input.mouse_position;

        // Move: 6-DOF fly.
        let fwd = self.forward();
        let (sy, cy) = self.yaw.sin_cos();
        let right = Vec3::new(-cy, 0.0, sy);
        let speed = MOVE_SPEED * dt;
        let mut v = Vec3::ZERO;
        if input.key_down(Key::W) {
            v += fwd;
        }
        if input.key_down(Key::S) {
            v -= fwd;
        }
        if input.key_down(Key::D) {
            v += right;
        }
        if input.key_down(Key::A) {
            v -= right;
        }
        if input.key_down(Key::R) {
            v += Vec3::Y;
        }
        if input.key_down(Key::F) {
            v -= Vec3::Y;
        }
        self.pos += v * speed;

        // Sim every tile at fixed step (capped so a hitch can't spiral).
        self.sim_accum += dt;
        let mut steps = 0;
        while self.sim_accum >= SIM_DT && steps < 4 {
            for t in &mut self.tiles {
                t.stack.tick(SIM_DT);
            }
            self.sim_accum -= SIM_DT;
            steps += 1;
        }
        if self.sim_accum > SIM_DT {
            self.sim_accum = 0.0;
        }

        // Rebuild meshes on a slower, decoupled cadence.
        self.rebuild_accum += dt;
        if self.rebuild_accum >= REBUILD_INTERVAL {
            self.rebuild_accum = 0.0;
            self.dirty = true;
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if self.dirty {
            self.rebuild(renderer);
        }

        let camera = Camera {
            position: self.pos,
            target: self.pos + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 1.0,
            far: 30000.0,
        };
        renderer.set_camera(&camera);
        renderer.set_scene(SceneLighting {
            sun_dir: Vec3::new(0.4, 0.9, 0.35).normalize(),
            ..SceneLighting::default()
        });
        renderer.draw_sky();
        for s in &self.sheets {
            renderer.draw_mesh(
                s.handle,
                s.model,
                MeshDrawOptions {
                    wireframe: false,
                    tint: s.tint,
                },
            );
        }

        renderer.set_layer(50.0);
        renderer.draw_text(
            "WASD move · R/F up/down · hold RMB to look · Esc quit",
            Vec2::new(12.0, 12.0),
            18.0,
            [1.0, 1.0, 1.0, 0.8],
        );
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    run(SceneManager::new(Box::new(World::new())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_layout_tessellates_without_overlap() {
        let map = HexMap::new(RINGS);
        let places: Vec<Vec2> = (0..map.total()).map(|i| hex_flat_pos(map.coord(i))).collect();
        // Adjacent hex centres are ~2·HEX_HALF_W apart; any closer means two
        // hexes collapsed onto the same spot (a ring-walk bug).
        for a in 0..places.len() {
            for b in a + 1..places.len() {
                let d = (places[a] - places[b]).length();
                assert!(d > HEX_HALF_W, "hexes {a},{b} overlap (d = {d:.1})");
            }
        }
    }

    #[test]
    fn hemispheres_interlock_at_the_equator() {
        // The closest north↔south hexes should be ~one hex pitch apart: touching
        // and interlocking (teeth), neither overlapping nor leaving a gap.
        let map = HexMap::new(RINGS);
        let mut min_cross = f32::INFINITY;
        for a in 0..map.total() {
            for b in 0..map.total() {
                if map.coord(a).hemi != map.coord(b).hemi {
                    let d = (hex_flat_pos(map.coord(a)) - hex_flat_pos(map.coord(b))).length();
                    min_cross = min_cross.min(d);
                }
            }
        }
        let pitch = 2.0 * HEX_HALF_W; // one hex centre-to-centre
        assert!(
            min_cross > HEX_HALF_W && min_cross < 1.3 * pitch,
            "equator join distance {min_cross:.1} (pitch {pitch:.1}) — gapped or overlapping"
        );
    }

    #[test]
    fn each_ring_is_full_and_separated_by_hemisphere() {
        let map = HexMap::new(RINGS);
        let (mut north, mut south) = (0u32, 0u32);
        for i in 0..map.total() {
            let c = map.coord(i);
            match c.hemi {
                Hemisphere::North => north += 1,
                Hemisphere::South => south += 1,
            }
        }
        // Symmetric hemispheres; total = 2 + 6·R·(R+1).
        assert_eq!(north, south);
        assert_eq!(north + south, 2 + 6 * RINGS * (RINGS + 1));
    }
}
