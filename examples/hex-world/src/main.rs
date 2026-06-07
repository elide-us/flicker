//! HexWorld — **layered water-cycle viewer** (POC).
//!
//! Two views, toggled with **E**:
//!
//! - **Exploded** (one hex) — the full field stack floating layer-by-layer:
//!   substance as relief **meshes** (ground/water/cloud), influence fields as
//!   flat **heatmaps** (temperature/humidity), and the star-driven upper
//!   atmosphere (stratosphere/ozone, thermosphere) on top.
//! - **World** (a few rings of hexes joined) — the realized surface tiled across
//!   the hex patch into one continuous map, with a temperature underlay and a
//!   drifting cloud deck. Terrain joins seamlessly because the heightmap is a
//!   continuous function; the sun sweeps in world coordinates so the day/night
//!   terminator runs unbroken across tiles.
//!
//! The sim ticks every frame on every tile. Hold **Space** to pause it; the
//! camera auto-orbits (A/D nudge, W/S zoom, Esc quits). Per-hex tiles still sim
//! independently — cross-hex flow (clouds/water drifting over seams) is the next
//! step (graph halo exchange); terrain and the world-positioned heat field
//! already join.
//!
//! Reusable nucleus is in [`layers`]; this file is just the viewer.

mod layers;

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, Bindings, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, SceneLighting,
    Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};

use layers::{LayerStack, G, HEX_HALF_W, HEX_SIZE, VSCALE};

const WORLD_OFFSET: Vec2 = Vec2::new(1234.0, 5678.0);
const SIM_DT: f32 = 0.05;
/// Hex rings around the centre in World view (0 = 1 hex, 1 = 7, 2 = 19).
const RINGS: i32 = 1;

// Exploded-view display heights per layer.
const PY_GROUND: f32 = 240.0;
const PY_TEMP: f32 = 360.0;
const PY_HUMID: f32 = 470.0;
const PY_WATER: f32 = 590.0;
const PY_CLOUD: f32 = 710.0;
const PY_STRATO: f32 = 830.0;
const PY_THERMO: f32 = 950.0;
// World-view layer separation.
const WY_TEMP: f32 = -130.0;
const WY_CLOUD: f32 = 160.0;

const SPIN_RATE: f32 = 0.22;
const TURN_SPEED: f32 = 1.4;
const ZOOM_SPEED: f32 = 500.0;

#[derive(Clone, Copy, PartialEq)]
enum View {
    Exploded,
    World,
}

/// One hex's data plus its flat-layout position in the joined map.
struct HexTile {
    place: Vec2,
    stack: LayerStack,
}

/// One uploaded mesh: its handle, draw tint, and model transform (XZ placement).
struct Sheet {
    handle: MeshHandle,
    tint: [f32; 4],
    model: Mat4,
}

fn mk(
    renderer: &mut Renderer,
    mesh: (Vec<MeshVertex>, Vec<u32>),
    tint: [f32; 4],
    model: Mat4,
) -> Option<Sheet> {
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

#[inline]
fn at(field: &[f32], i: usize, j: usize) -> f32 {
    field[j * G + i]
}

#[inline]
fn nrm(v: f32, lo: f32, hi: f32) -> f32 {
    if hi > lo {
        (v - lo) / (hi - lo)
    } else {
        0.5
    }
}

/// Concentric hex rings, flat-laid in axial coordinates (the proven bubble
/// spacing). Each tile samples its own world window, so terrain joins seamlessly.
fn hex_tiles() -> Vec<HexTile> {
    let ax = Vec2::new(2.0 * HEX_HALF_W, 0.0); // E neighbour
    let ay = Vec2::new(HEX_HALF_W, 1.5 * HEX_SIZE); // NE neighbour
    let mut tiles = Vec::new();
    for q in -RINGS..=RINGS {
        for r in -RINGS..=RINGS {
            let s = -q - r;
            if q.abs().max(r.abs()).max(s.abs()) <= RINGS {
                let place = ax * q as f32 + ay * r as f32;
                tiles.push(HexTile {
                    place,
                    stack: LayerStack::generate(WORLD_OFFSET + place),
                });
            }
        }
    }
    tiles
}

/// Exploded view: the centre tile's full stack, each layer at its own height.
fn build_exploded(s: &LayerStack, renderer: &mut Renderer) -> Vec<Sheet> {
    let mut out = Vec::new();
    let m = Mat4::IDENTITY;
    let snow = s.snow_line;

    // ground (mesh relief)
    let ground = layers::build_sheet(
        PY_GROUND,
        |i, j| (at(&s.ground, i, j) - 128.0) * VSCALE,
        |i, j| if at(&s.ground, i, j) > snow { layers::M_STONE } else { layers::M_LAND },
        |_, _| true,
    );
    out.extend(mk(renderer, ground, [1.0, 1.0, 1.0, 0.7], m));

    // realized composite (mesh, opaque, at the base)
    let realized = layers::build_sheet(0.0, |i, j| s.realized(i, j).0, |i, j| s.realized(i, j).1, |_, _| true);
    out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], m));

    // temperature (heatmap)
    let (tlo, thi) = layers::minmax(&s.temperature);
    let temp = layers::build_sheet(
        PY_TEMP,
        |_, _| 0.0,
        |i, j| layers::pack_ramp(layers::M_ICE, layers::M_LAVA, nrm(at(&s.temperature, i, j), tlo, thi)),
        |_, _| true,
    );
    out.extend(mk(renderer, temp, [1.0, 1.0, 1.0, 0.92], m));

    // humidity (heatmap)
    let (hlo, hhi) = layers::minmax(&s.humidity);
    let humid = layers::build_sheet(
        PY_HUMID,
        |_, _| 0.0,
        |i, j| layers::pack_ramp(layers::M_FOAM, layers::M_WATER_DEEP, nrm(at(&s.humidity, i, j), hlo, hhi)),
        |_, _| true,
    );
    out.extend(mk(renderer, humid, [1.0, 1.0, 1.0, 0.92], m));

    // water (mesh, footprint)
    let water = layers::build_sheet(
        PY_WATER,
        |i, j| at(&s.water, i, j) * VSCALE,
        |i, j| {
            let d = at(&s.water, i, j);
            if d > 28.0 { layers::M_WATER_DEEP } else if d > 10.0 { layers::M_WATER_MID } else { layers::M_WATER_SHALLOW }
        },
        |i, j| at(&s.water, i, j) > 0.5,
    );
    out.extend(mk(renderer, water, [1.0, 1.0, 1.0, 0.6], m));

    // cloud (mesh, footprint)
    let cloud = layers::build_sheet(PY_CLOUD, |i, j| at(&s.cloud, i, j) * 45.0, |_, _| layers::M_CLOUD, |i, j| at(&s.cloud, i, j) > 0.15);
    out.extend(mk(renderer, cloud, [1.0, 1.0, 1.0, 0.45], m));

    // stratosphere / ozone (heatmap)
    let (slo, shi) = layers::minmax(&s.stratosphere);
    let strato = layers::build_sheet(
        PY_STRATO,
        |_, _| 0.0,
        |i, j| layers::pack_ramp(layers::M_ICE, layers::M_UV, nrm(at(&s.stratosphere, i, j), slo, shi)),
        |_, _| true,
    );
    out.extend(mk(renderer, strato, [1.0, 1.0, 1.0, 0.9], m));

    // thermosphere (heatmap)
    let (xlo, xhi) = layers::minmax(&s.thermosphere);
    let thermo = layers::build_sheet(
        PY_THERMO,
        |_, _| 0.0,
        |i, j| layers::pack_ramp(layers::M_VOID, layers::M_AURORA, nrm(at(&s.thermosphere, i, j), xlo, xhi)),
        |_, _| true,
    );
    out.extend(mk(renderer, thermo, [1.0, 1.0, 1.0, 0.92], m));

    out
}

/// World view: every tile's realized surface joined, with a global temperature
/// underlay and a cloud deck above. Temperature uses one global ramp so the
/// heatmap doesn't seam between tiles.
fn build_world(tiles: &[HexTile], renderer: &mut Renderer) -> Vec<Sheet> {
    let (mut tlo, mut thi) = (f32::INFINITY, f32::NEG_INFINITY);
    for t in tiles {
        let (l, h) = layers::minmax(&t.stack.temperature);
        tlo = tlo.min(l);
        thi = thi.max(h);
    }

    let mut out = Vec::new();
    for t in tiles {
        let s = &t.stack;
        let model = Mat4::from_translation(Vec3::new(t.place.x, 0.0, t.place.y));

        let temp = layers::build_sheet(
            WY_TEMP,
            |_, _| 0.0,
            |i, j| layers::pack_ramp(layers::M_ICE, layers::M_LAVA, nrm(at(&s.temperature, i, j), tlo, thi)),
            |_, _| true,
        );
        out.extend(mk(renderer, temp, [1.0, 1.0, 1.0, 0.95], model));

        let realized = layers::build_sheet(0.0, |i, j| s.realized(i, j).0, |i, j| s.realized(i, j).1, |_, _| true);
        out.extend(mk(renderer, realized, [1.0, 1.0, 1.0, 1.0], model));

        let cloud = layers::build_sheet(WY_CLOUD, |i, j| at(&s.cloud, i, j) * 40.0, |_, _| layers::M_CLOUD, |i, j| at(&s.cloud, i, j) > 0.15);
        out.extend(mk(renderer, cloud, [1.0, 1.0, 1.0, 0.5], model));
    }
    out
}

struct LayeredHex {
    tiles: Vec<HexTile>,
    view: View,
    sheets: Vec<Sheet>,
    labels: Vec<(Vec3, &'static str, [f32; 4])>,
    dirty: bool,
    sim_accum: f32,
    yaw: f32,
    distance: f32,
    toggle_was_down: bool,
    bindings: Bindings,
}

impl LayeredHex {
    fn new() -> Self {
        Self {
            tiles: hex_tiles(),
            view: View::Exploded,
            sheets: Vec::new(),
            labels: Vec::new(),
            dirty: true,
            sim_accum: 0.0,
            yaw: 0.0,
            distance: 1550.0,
            toggle_was_down: false,
            bindings: Bindings::wasd(),
        }
    }

    fn center(&self) -> &HexTile {
        self.tiles
            .iter()
            .min_by(|a, b| a.place.length_squared().partial_cmp(&b.place.length_squared()).unwrap())
            .unwrap()
    }

    /// Labels and camera defaults change with the view.
    fn enter_view(&mut self) {
        let lx = -HEX_HALF_W - 55.0;
        match self.view {
            View::Exploded => {
                self.distance = 1550.0;
                self.labels = vec![
                    (Vec3::new(lx, 95.0, 0.0), "realized", [1.0, 1.0, 1.0, 1.0]),
                    (Vec3::new(lx, PY_GROUND, 0.0), "ground", [0.6, 0.8, 0.5, 1.0]),
                    (Vec3::new(lx, PY_TEMP, 0.0), "temperature", [1.0, 0.6, 0.3, 1.0]),
                    (Vec3::new(lx, PY_HUMID, 0.0), "humidity", [0.5, 0.7, 1.0, 1.0]),
                    (Vec3::new(lx, PY_WATER, 0.0), "water", [0.4, 0.7, 1.0, 1.0]),
                    (Vec3::new(lx, PY_CLOUD, 0.0), "cloud", [0.85, 0.87, 0.9, 1.0]),
                    (Vec3::new(lx, PY_STRATO, 0.0), "stratosphere (ozone)", [0.7, 0.5, 0.95, 1.0]),
                    (Vec3::new(lx, PY_THERMO, 0.0), "thermosphere", [0.3, 0.95, 0.6, 1.0]),
                ];
            }
            View::World => {
                self.distance = 700.0 + RINGS as f32 * 620.0;
                let wx = -(RINGS as f32 + 1.4) * 2.0 * HEX_HALF_W;
                self.labels = vec![
                    (Vec3::new(wx, WY_CLOUD, 0.0), "clouds", [0.85, 0.87, 0.9, 1.0]),
                    (Vec3::new(wx, 60.0, 0.0), "terrain", [0.7, 0.85, 0.7, 1.0]),
                    (Vec3::new(wx, WY_TEMP, 0.0), "temperature", [1.0, 0.6, 0.3, 1.0]),
                ];
            }
        }
        self.dirty = true;
    }

    fn rebuild(&mut self, renderer: &mut Renderer) {
        for s in std::mem::take(&mut self.sheets) {
            renderer.free_mesh(s.handle);
        }
        self.sheets = match self.view {
            View::Exploded => build_exploded(&self.center().stack, renderer),
            View::World => build_world(&self.tiles, renderer),
        };
        self.dirty = false;
    }
}

impl Scene for LayeredHex {
    fn enter(&mut self, _renderer: &mut Renderer) {
        println!(
            "LayeredHex: {} hex(es), {}×{} grid each, H2O total {:.0} (conserved). E: view, Space: pause",
            self.tiles.len(),
            G,
            G,
            self.center().stack.total_water()
        );
        self.enter_view();
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) -> Transition {
        if input.key_down(Key::Escape) {
            return Transition::Quit;
        }
        let dt = dt.as_secs_f32();

        // E toggles the view on key-down edge.
        let toggle = input.key_down(Key::E);
        if toggle && !self.toggle_was_down {
            self.view = match self.view {
                View::Exploded => View::World,
                View::World => View::Exploded,
            };
            self.enter_view();
        }
        self.toggle_was_down = toggle;

        // Camera.
        let left = input.action_active(&self.bindings, Action::StrafeLeft);
        let right = input.action_active(&self.bindings, Action::StrafeRight);
        let fwd = input.action_active(&self.bindings, Action::MoveForward);
        let back = input.action_active(&self.bindings, Action::MoveBackward);
        self.yaw += (SPIN_RATE + (right as i32 - left as i32) as f32 * TURN_SPEED) * dt;
        self.distance =
            (self.distance + (back as i32 - fwd as i32) as f32 * ZOOM_SPEED * dt).clamp(450.0, 4000.0);

        // Sim (hold Space to pause), capped so a hitch can't spiral.
        if !input.key_down(Key::Space) {
            self.sim_accum += dt;
            let mut steps = 0;
            while self.sim_accum >= SIM_DT && steps < 4 {
                for t in &mut self.tiles {
                    t.stack.tick(SIM_DT);
                }
                self.sim_accum -= SIM_DT;
                self.dirty = true;
                steps += 1;
            }
            if self.sim_accum > SIM_DT {
                self.sim_accum = 0.0;
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if self.dirty {
            self.rebuild(renderer);
        }

        let (target, pitch) = match self.view {
            View::Exploded => (Vec3::new(0.0, PY_WATER, 0.0), 0.45),
            View::World => (Vec3::ZERO, 0.85),
        };
        let camera = Camera::orbit(target, self.distance, self.yaw, pitch);
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

        let size = renderer.size();
        let vp = camera.view_projection(size.x / size.y);
        renderer.set_layer(50.0);
        for &(pos, text, color) in &self.labels {
            if let Some(p) = project_to_screen(vp, pos, size) {
                renderer.draw_text(text, p, 20.0, color);
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
    run(SceneManager::new(Box::new(LayeredHex::new())))?;
    Ok(())
}
