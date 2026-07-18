//! flicker-packeditor — the instanced GPU-skinning **animation field / pack viewer**.
//! (Promoted from the old `examples/flicker-field-viewer` POC into Alpha.)
//!
//! A living 3D field of the model's whole animation library, drawn as **one instanced
//! draw call** (`flicker-render`'s `SkinnedMeshPipeline` via
//! [`Renderer::draw_skinned_instanced`]) — one GPU-skinned character per node, each
//! playing its clip.
//!
//! * **Slice 1/2** — the instanced-skinning pipeline proof, then per-instance animation
//!   (each clip loops at its own `tick_rate_hz`; per frame we re-sample all clips →
//!   `skin::palette` → re-submit — the real CPU-pose → GPU-skin path).
//! * **Slice 3a** — orbit + fly cameras (Tab toggles).
//! * **Slice 3b** — the **state graph** ([`graph`]): the pack's *states* become
//!   **connected** nodes laid out by a force-directed weighting (clustered like the HTML
//!   diagram, spread to read the wires); clips no state uses become **orphan** nodes in a
//!   separate zone. Transition **connections** are drawn as lines. Rings / glow / node
//!   selection / the below-ground cloud are later slices.
//!
//! Each node's model matrix is `translate(layout_pos) × Model::world` (source→engine, as
//! the single-character viewer; the graph is centred on the origin for the orbit camera).
//!
//! **Known edge case:** `RM/` (RootMotion) clips bake world translation into the pose, so
//! an orphan playing one (e.g. `RM/Slide`) walks out of its cell. In-place display (strip
//! the root translation) is the fix — deferred until it matters.
//!
//! Reuses the Katanami character bundle from the shared content tree
//! (`Alpha/content/characters/katanami`: the rig + 91-clip library + the
//! `Katanami.pack.json` state graph), the same source flicker-paperdoll loads.
//!
//! **Tab** toggles input mode: **Character** (WASD move · Shift run · C crouch · Space jump ·
//! F attack · H hit · X die drive the PiP character's state machine; camera orbits — drag
//! rotate, wheel zoom) ⇄ **Camera** (WASD flies over the field · R/F up-down · right-drag look ·
//! wheel speed). Click = select a node (reveals its connections). Esc = quit. The USER runs
//! the window and verifies; this crate never launches it.

mod graph;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, App, InputState, Key};
use flicker::render::{
    Camera, GroundFog, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, RenderTargetHandle,
    Renderer, SceneLighting, SkinnedMeshHandle, SkinnedVertex, Vec2, Vec3,
};
use glam::Mat4;

use flicker_skeletal::state::{ClipRef, Inputs, StateMachine, StateMachineDef};
use flicker_skeletal::{format, pose, skin, state};

use format::Model;
use graph::{BoxKind, Graph};

/// Mouse look sensitivity (radians per pixel) — mirrors `voxel-cluster`'s default.
const LOOK_SENS: f32 = 0.005;

/// A group-box outline: its floor line segments + the RGBA colour to draw them.
type BoxOutline = (Vec<(Vec3, Vec3)>, [f32; 4]);

/// Line-loop segments approximating a circle on the floor (`y`) — the ground ring under a
/// character. `segments` sides.
fn circle_segments(center: Vec2, radius: f32, y: f32, segments: usize) -> Vec<(Vec3, Vec3)> {
    use std::f32::consts::TAU;
    (0..segments)
        .map(|i| {
            let a0 = i as f32 / segments as f32 * TAU;
            let a1 = (i + 1) as f32 / segments as f32 * TAU;
            (
                Vec3::new(center.x + radius * a0.cos(), y, center.y + radius * a0.sin()),
                Vec3::new(center.x + radius * a1.cos(), y, center.y + radius * a1.sin()),
            )
        })
        .collect()
}

/// Night-sky lighting for the field: sun below the horizon, cool moonlight, dark starry
/// sky. Characters read as moonlit; the procedural sky pass draws the stars/Milky Way.
fn night_scene() -> SceneLighting {
    SceneLighting {
        sun_dir: Vec3::new(0.2, -0.5, 0.3).normalize(),
        sun_color: Vec3::ZERO,
        moon_dir: Vec3::new(-0.3, 0.7, 0.4).normalize(),
        moon_color: Vec3::new(0.42, 0.48, 0.62),
        ambient: Vec3::new(0.13, 0.15, 0.21),
        sky_zenith: Vec3::new(0.006, 0.010, 0.024),
        sky_horizon: Vec3::new(0.020, 0.030, 0.060),
        ..SceneLighting::default()
    }
}

/// Brighter studio lighting for the PiP portrait, so the character stays well-lit against
/// the night field.
fn pip_scene() -> SceneLighting {
    SceneLighting {
        sun_dir: Vec3::new(0.4, 0.8, 0.5).normalize(),
        sun_color: Vec3::splat(0.85),
        moon_dir: Vec3::new(-0.5, 0.3, -0.4).normalize(),
        moon_color: Vec3::new(0.20, 0.22, 0.30),
        ambient: Vec3::splat(0.35),
        ..SceneLighting::default()
    }
}

/// Segments for an arrow from `from` to `to` on the floor: a shaft that stops `stop_short`
/// before the target (so the head sits at the target's ring edge) + a two-line arrowhead.
fn arrow_segments(from: Vec3, to: Vec3, stop_short: f32, head_len: f32) -> Vec<(Vec3, Vec3)> {
    let delta = to - from;
    let len = delta.length();
    if len < 1e-4 {
        return Vec::new();
    }
    let dir = delta / len;
    let tip = to - dir * stop_short.min(len * 0.5);
    let back = tip - dir * head_len;
    let perp = Vec3::new(-dir.z, 0.0, dir.x) * head_len * 0.6; // horizontal ⟂ to dir
    vec![(from, tip), (tip, back + perp), (tip, back - perp)]
}

/// Outline colour for a group box, by what it holds.
fn box_color(kind: BoxKind) -> [f32; 4] {
    match kind {
        BoxKind::Pack => [0.40, 0.62, 1.0, 0.95], // the .pack box — prominent blue
        BoxKind::Movement => [0.34, 0.44, 0.62, 0.6], // subtle sub-boxes inside the pack
        BoxKind::Triggered => [0.38, 0.58, 0.50, 0.6],
        BoxKind::WaitingInPlace => [0.60, 0.55, 0.42, 0.70],
        BoxKind::WaitingRoot => [0.62, 0.46, 0.72, 0.75],
    }
}

/// Pack an RGB colour (0..1) into the lit-mesh pipeline's direct-RGB material escape: the
/// low 12 bits `0xFFF` mark a packed RGB666 in the upper bits (see `flicker-render`
/// `mesh.wgsl` `material_color`), so a flat colour renders through the existing lit
/// pipeline. Same helper as `flicker-paperdoll`.
fn pack_rgb666(r: f32, g: f32, b: f32) -> u32 {
    let q = |c: f32| ((c.clamp(0.0, 1.0) * 63.0).round() as u32) & 0x3F;
    0xFFF | (q(r) << 12) | (q(g) << 18) | (q(b) << 24)
}

/// Build a **double-sided** floor quad (both windings, same `+Y` normal) spanning
/// `min..max` at height `y`. Emitting both windings means the front-facing set always
/// draws and is lit from above regardless of which winding the rasteriser treats as
/// front — no winding guesswork for a one-off ground plane.
fn floor_quad(min: Vec2, max: Vec2, y: f32, material: u32) -> Vec<MeshVertex> {
    let v = |x: f32, z: f32| MeshVertex {
        position: [x, y, z],
        normal: [0.0, 1.0, 0.0],
        material,
    };
    let (a, b, c, d) = (
        v(min.x, min.y),
        v(max.x, min.y),
        v(max.x, max.y),
        v(min.x, max.y),
    );
    vec![a, b, c, a, c, d, a, c, b, a, d, c]
}

/// The four edge segments of an XZ rectangle at height `y` — a region boundary outline.
fn rect_outline(min: Vec2, max: Vec2, y: f32) -> Vec<(Vec3, Vec3)> {
    let p = |x: f32, z: f32| Vec3::new(x, y, z);
    let c = [
        p(min.x, min.y),
        p(max.x, min.y),
        p(max.x, max.y),
        p(min.x, max.y),
    ];
    vec![(c[0], c[1]), (c[1], c[2]), (c[2], c[3]), (c[3], c[0])]
}

/// What the WASD / action keys drive. `Camera` flies the camera over the field;
/// `Character` drives the PiP character's state machine (WASD move · Shift run · C crouch ·
/// Space jump · F attack · H hit · X die) while the camera orbits. Tab toggles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Camera,
    Character,
}

/// Orbit camera (drag rotates, wheel zooms) framing the whole field and looking at the
/// grid centre (the origin). Mirrors `flicker-paperdoll`'s `OrbitCam`, retuned for a
/// larger framing radius.
struct OrbitCam {
    /// The point the camera orbits / looks at (movable, so a fly→orbit switch stays put).
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    min_distance: f32,
    max_distance: f32,
    prev_mouse: Vec2,
    dragging: bool,
}

impl OrbitCam {
    fn new(radius: f32) -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.55,
            distance: radius * 1.6,
            min_distance: radius * 0.2,
            max_distance: radius * 6.0,
            prev_mouse: Vec2::ZERO,
            dragging: false,
        }
    }

    /// Reposition to match another camera's pose so a mode switch doesn't jump: put the
    /// orbit target in front of `cam` (at the current distance) and derive yaw/pitch from
    /// its look direction. Orbit is a constrained view (always looks at `target`), so this
    /// keeps the *view* continuous where an exact match isn't representable.
    fn sync_from_camera(&mut self, cam: &Camera) {
        let d = (cam.target - cam.position).normalize_or_zero();
        if d.length_squared() < 1e-6 {
            return;
        }
        self.pitch = (-d.y).clamp(-1.0, 1.0).asin();
        self.yaw = (-d.x).atan2(-d.z);
        self.target = cam.position + d * self.distance;
    }

    fn update(&mut self, input: &InputState) {
        let mouse = input.mouse_position;
        if input.mouse_left {
            if self.dragging {
                let delta = mouse - self.prev_mouse;
                self.yaw -= delta.x * 0.01;
                self.pitch = (self.pitch + delta.y * 0.01).clamp(-1.4, 1.4);
            }
            self.dragging = true;
        } else {
            self.dragging = false;
        }
        self.prev_mouse = mouse;

        let wheel = input.mouse_wheel_delta;
        if wheel != 0.0 {
            self.distance =
                (self.distance * (1.0 - wheel * 0.1)).clamp(self.min_distance, self.max_distance);
        }
    }

    fn camera(&self) -> Camera {
        Camera::orbit(self.target, self.distance, self.yaw, self.pitch)
    }
}

/// Free-fly camera: right-drag to look, WASD to move along the look direction, R/F (or
/// E/Q) for world up/down, Shift to sprint, wheel to scale fly speed. Look convention
/// mirrors `voxel-cluster` (`yaw -= dx·sens`, `pitch -= dy·sens`).
struct FlyCam {
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Base move speed (units/s), scaled to the field size; wheel adjusts it.
    speed: f32,
    /// Cursor at the previous look-frame while right-dragging; `None` when not looking.
    last_look: Option<Vec2>,
}

impl FlyCam {
    fn new(speed: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            speed,
            last_look: None,
        }
    }

    /// Seed position + look angles from an existing camera so a mode switch doesn't jump.
    fn sync_from(&mut self, cam: &Camera) {
        self.position = cam.position;
        let f = (cam.target - cam.position).normalize_or_zero();
        if f.length_squared() > 1e-6 {
            self.yaw = f.x.atan2(f.z);
            self.pitch = f.y.clamp(-1.0, 1.0).asin();
        }
        self.last_look = None;
    }

    fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos())
    }

    fn move_right(&self) -> Vec3 {
        let f = self.forward();
        Vec3::new(f.x, 0.0, f.z)
            .normalize_or_zero()
            .cross(Vec3::Y)
            .normalize_or_zero()
    }

    fn update(&mut self, dt: f32, input: &InputState) {
        // Right-drag look.
        if input.mouse_right {
            if let Some(prev) = self.last_look {
                let d = input.mouse_position - prev;
                self.yaw -= d.x * LOOK_SENS;
                self.pitch = (self.pitch - d.y * LOOK_SENS).clamp(-1.5, 1.5);
            }
            self.last_look = Some(input.mouse_position);
        } else {
            self.last_look = None;
        }

        // Move: W/S along look dir, A/D horizontal strafe, R/F (or E/Q) world up/down.
        let mut dir = Vec3::ZERO;
        if input.key_down(Key::W) {
            dir += self.forward();
        }
        if input.key_down(Key::S) {
            dir -= self.forward();
        }
        if input.key_down(Key::D) {
            dir += self.move_right();
        }
        if input.key_down(Key::A) {
            dir -= self.move_right();
        }
        if input.key_down(Key::R) || input.key_down(Key::E) {
            dir += Vec3::Y;
        }
        if input.key_down(Key::F) || input.key_down(Key::Q) {
            dir -= Vec3::Y;
        }
        let sprint = if input.key_down(Key::LeftShift) || input.key_down(Key::RightShift) {
            3.0
        } else {
            1.0
        };
        self.position += dir.normalize_or_zero() * self.speed * sprint * dt;

        let wheel = input.mouse_wheel_delta;
        if wheel != 0.0 {
            self.speed = (self.speed * (1.0 + wheel * 0.1)).clamp(0.05, 1.0e6);
        }
    }

    fn camera(&self) -> Camera {
        Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            ..Camera::default()
        }
    }
}

struct Field {
    /// The rig + clip library, retained so every frame can re-sample each node's clip.
    model: Model,
    /// The laid-out state graph (connected states + orphan clips + connections).
    graph: Graph,
    orbit: OrbitCam,
    fly: FlyCam,
    mode: InputMode,
    /// The combat/animation state machine driving the PiP character (from the pack). `None`
    /// if the pack didn't load. The active state's node is highlighted in the field.
    sm: Option<StateMachine>,
    /// The one bind-pose skinned mesh, uploaded once in `init`.
    mesh: Option<SkinnedMeshHandle>,
    /// Offscreen target for the paperdoll panel — the selected character rendered alone
    /// with its own camera (created in `init`).
    paperdoll: Option<RenderTargetHandle>,
    /// The floor plane everything stands on (uploaded once in `init`).
    floor_mesh: Option<MeshHandle>,
    /// Floor vertices (built in `new`, uploaded in `init`).
    floor_verts: Vec<MeshVertex>,
    /// Volumetric ground-fog slab bounds (world Y): between the glass floor and just below
    /// the feet. Drifts + self-composites in `flicker-render`'s ground-fog pass.
    fog_bottom: f32,
    fog_top: f32,
    /// Per-group-box outline segments + colour (drawn on the floor).
    box_outlines: Vec<BoxOutline>,
    /// Ground ring under every character (all circles concatenated, drawn each frame).
    rings: Vec<(Vec3, Vec3)>,
    /// Ring radius (metres) — reused to draw the selected node's highlight ring.
    ring_radius: f32,
    /// Per-node model→world: `translate(layout_pos) × Model::world`, computed once.
    models: Vec<Mat4>,
    /// Scratch flat palette buffer, rebuilt every frame: node `i` bone `b` at
    /// `i*bone_count + b`. `len == models.len() * bone_count`.
    palettes: Vec<Mat4>,
    /// Character bounding radius (metres) — sizes the pick threshold + selection marker.
    unit: f32,
    /// Height of the on-floor lines/markers (just above the floor plane).
    line_y: f32,
    /// The currently selected node (index into `graph.nodes`), or `None`. Clicking a node
    /// reveals *its* connections (the HTML "select to see wires" behaviour); clicking empty
    /// deselects.
    selected: Option<usize>,
    prev_mouse_left: bool,
    /// Hit-reaction state names (`Dame_*`) — H plays a random one. Collected in `new`.
    dame_states: Vec<String>,
    /// xorshift RNG state for picking a random hit reaction (driver-owned randomness, so
    /// the state machine stays deterministic).
    rng_state: u64,
    bone_count: u32,
    vert_count: usize,
    /// Shared elapsed seconds driving every field node's looping play-head.
    time: f32,
    /// Edge-detection for the state-machine trigger keys (jump/attack/hit/die).
    prev_jump: bool,
    prev_attack: bool,
    prev_hit: bool,
    prev_die: bool,
    prev_tab: bool,
    should_quit: bool,
}

impl Field {
    fn new(model: Model, pack: Option<StateMachineDef>, pack_name: &str) -> Self {
        let unit = model.orbit_radius.max(0.25);
        let graph = Graph::build(&model, pack.as_ref(), pack_name, unit);
        let bone_count = model.bones.len();
        let vert_count = model.mesh.vertices.len();

        // Hit-reaction states (Dame_*) for random selection on H.
        let dame_states: Vec<String> = graph.nodes[..graph.connected_count]
            .iter()
            .filter(|n| n.label.starts_with("Dame"))
            .map(|n| n.label.clone())
            .collect();

        // The state machine that drives the PiP character (clips referenced by name).
        let sm = pack.as_ref().and_then(|def| {
            let refs: Vec<ClipRef> = model
                .clips
                .iter()
                .map(|c| ClipRef { name: &c.name, duration_ticks: c.duration_ticks })
                .collect();
            match StateMachine::build(def, &refs) {
                Ok(sm) => Some(sm),
                Err(e) => {
                    tracing::warn!("state machine failed to build ({e}); PiP disabled");
                    None
                }
            }
        });

        // Each node's transform: shift the origin-centred model to its layout cell.
        let models: Vec<Mat4> = graph
            .nodes
            .iter()
            .map(|n| Mat4::from_translation(Vec3::new(n.pos.x, 0.0, n.pos.y)) * model.world)
            .collect();

        // Floor level = the lowest bind-mesh vertex in engine space (the characters'
        // feet), so the floor sits under their soles. Lines sit a hair above it.
        let foot_y = model
            .mesh
            .vertices
            .iter()
            .map(|v| model.world.transform_point3(Vec3::from(v.p)).y)
            .fold(f32::INFINITY, f32::min);
        let foot_y = if foot_y.is_finite() { foot_y } else { -unit };
        let line_y = foot_y + unit * 0.03;
        // Layers below the models (which stand at `foot_y`, with the rings/boxes/arrows):
        // a volumetric fog slab drifts *below the feet*, and the glass floor sits *below the
        // fog* — models over a fog layer over the glass (the fog never slices the feet).
        let floor_y = foot_y - unit * 0.6;
        let fog_bottom = floor_y + unit * 0.05;
        let fog_top = foot_y - unit * 0.05;

        // Floor mesh (built now, uploaded in `init`) + per-box outlines.
        let floor_verts = floor_quad(graph.floor_min, graph.floor_max, floor_y, pack_rgb666(0.15, 0.17, 0.22));
        let box_outlines: Vec<BoxOutline> = graph
            .boxes
            .iter()
            .map(|b| (rect_outline(b.min, b.max, line_y), box_color(b.kind)))
            .collect();

        // A ground ring under every character.
        let ring_radius = unit;
        let rings: Vec<(Vec3, Vec3)> = graph
            .nodes
            .iter()
            .flat_map(|n| circle_segments(n.pos, ring_radius, line_y, 24))
            .collect();

        // Frame the whole floor.
        let frame_radius = (0.5 * (graph.floor_max - graph.floor_min).length()).max(unit * 3.0);

        let orbit = OrbitCam::new(frame_radius);
        let mut fly = FlyCam::new(frame_radius * 0.5);
        fly.sync_from(&orbit.camera());

        let mut field = Self {
            model,
            graph,
            orbit,
            fly,
            mode: InputMode::Character,
            sm,
            mesh: None,
            paperdoll: None,
            floor_mesh: None,
            floor_verts,
            fog_bottom,
            fog_top,
            box_outlines,
            rings,
            ring_radius,
            palettes: Vec::new(),
            models,
            unit,
            line_y,
            selected: None,
            prev_mouse_left: false,
            dame_states,
            rng_state: 0x2545_F491_4F6C_DD1D,
            bone_count: bone_count as u32,
            vert_count,
            time: 0.0,
            prev_jump: false,
            prev_attack: false,
            prev_hit: false,
            prev_die: false,
            prev_tab: false,
            should_quit: false,
        };
        field.rebuild_palettes();
        field
    }

    /// The PiP character's current pose palette from the state machine (with crossfade
    /// blend), mirroring `flicker-paperdoll`'s Graph mode. `None` if no state machine.
    fn pip_palette(&self) -> Option<Vec<Mat4>> {
        let sm = self.sm.as_ref()?;
        let sample = |clip_idx: usize, tick: u32| -> Vec<Mat4> {
            match self.model.clips.get(clip_idx) {
                Some(c) => {
                    let t = tick.min(c.duration_ticks.saturating_sub(1));
                    pose::sample_local_poses(&self.model.bones, c, t, self.model.retarget)
                }
                None => self.model.bones.iter().map(|b| b.local).collect(),
            }
        };
        let incoming = sample(sm.current_clip(), sm.current_tick());
        let locals = match sm.blend() {
            Some(b) => {
                let outgoing = sample(b.from_clip, b.from_tick);
                pose::blend_local_poses(&outgoing, &incoming, b.weight)
            }
            None => incoming,
        };
        let globals = pose::global_transforms(&self.model.bones, &locals);
        Some(skin::palette(&self.model.bones, &globals))
    }

    /// The field node for the state machine's current state (highlighted as it plays).
    fn active_node(&self) -> Option<usize> {
        let name = self.sm.as_ref()?.current_state_name();
        self.graph.nodes[..self.graph.connected_count]
            .iter()
            .position(|n| n.label == name)
    }

    /// xorshift64 — a cheap PRNG for picking a random hit reaction.
    fn next_rand(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }

    /// Rebuild the flat palette buffer for the current `time`: each node's clip sampled
    /// at its own looping play-head (missing clip → rest pose). The per-frame CPU-pose
    /// work — cheap; the GPU does the vertex skinning.
    fn rebuild_palettes(&mut self) {
        self.palettes.clear();
        for node in &self.graph.nodes {
            let globals = match self.model.clips.get(node.clip_index) {
                Some(clip) => {
                    let tick = if clip.duration_ticks > 0 {
                        let ticks = self.time * clip.tick_rate_hz as f32;
                        (ticks.floor() as i64).rem_euclid(clip.duration_ticks as i64) as u32
                    } else {
                        0
                    };
                    let locals = pose::sample_local_poses(&self.model.bones, clip, tick, self.model.retarget);
                    pose::global_transforms(&self.model.bones, &locals)
                }
                None => {
                    let rest: Vec<Mat4> = self.model.bones.iter().map(|b| b.local).collect();
                    pose::global_transforms(&self.model.bones, &rest)
                }
            };
            self.palettes
                .extend_from_slice(&skin::palette(&self.model.bones, &globals));
        }
    }

    fn camera(&self) -> Camera {
        match self.mode {
            InputMode::Camera => self.fly.camera(),
            InputMode::Character => self.orbit.camera(),
        }
    }

    /// Pick the node nearest the cursor: unproject the cursor to a world ray (via the
    /// inverse view-projection — convention-proof), intersect the floor plane (y = 0, the
    /// node-position plane), and take the closest node within a threshold. `None` = the
    /// ray missed every node (a click on empty floor deselects).
    fn pick_node(&self, cursor: Vec2, viewport: Vec2) -> Option<usize> {
        if viewport.x <= 0.0 || viewport.y <= 0.0 {
            return None;
        }
        let aspect = viewport.x / viewport.y;
        let inv = self.camera().view_projection(aspect).inverse();
        let ndc = Vec2::new(
            2.0 * cursor.x / viewport.x - 1.0,
            1.0 - 2.0 * cursor.y / viewport.y,
        );
        // wgpu NDC z ∈ [0,1]: z=0 near plane, z=1 far plane.
        let p0 = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let p1 = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
        let dir = p1 - p0;
        if dir.y.abs() < 1e-6 {
            return None;
        }
        let t = -p0.y / dir.y;
        if t < 0.0 {
            return None;
        }
        let hit = p0 + dir * t; // point on the y=0 plane
        let hit_xz = Vec2::new(hit.x, hit.z);
        let mut best: Option<usize> = None;
        let mut best_d = self.unit * 1.3; // within ~a character's footprint
        for (i, n) in self.graph.nodes.iter().enumerate() {
            let d = (n.pos - hit_xz).length();
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    /// Label of the group box a node sits in (for the HUD readout).
    fn box_label_of(&self, node: usize) -> Option<&str> {
        let p = self.graph.nodes[node].pos;
        self.graph
            .boxes
            .iter()
            .find(|b| p.x >= b.min.x && p.x <= b.max.x && p.y >= b.min.y && p.y <= b.max.y)
            .map(|b| b.label.as_str())
    }
}

impl App for Field {
    fn init(&mut self, renderer: &mut Renderer) {
        let verts: Vec<SkinnedVertex> = self
            .model
            .mesh
            .vertices
            .iter()
            .map(|v| SkinnedVertex {
                position: v.p,
                normal: v.n,
                uv: v.uv,
                joints: v.joints,
                weights: v.weights,
            })
            .collect();
        let indices: Vec<u32> = if self.model.mesh.indices.is_empty() {
            (0..verts.len() as u32).collect()
        } else {
            self.model.mesh.indices.clone()
        };
        self.mesh = Some(renderer.upload_skinned_mesh(&verts, MeshIndices::U32(&indices)));
        // Offscreen target for the paperdoll panel (square).
        self.paperdoll = Some(renderer.create_render_target(512, 512));
        // The floor plane (one lit mesh, in world space → identity model matrix).
        if !self.floor_verts.is_empty() {
            let fidx: Vec<u32> = (0..self.floor_verts.len() as u32).collect();
            self.floor_mesh = Some(renderer.upload_mesh(&self.floor_verts, MeshIndices::U32(&fidx)));
        }
        tracing::info!(
            "field viewer: {} nodes ({} states + {} orphans) · {} links · {} bones · {} verts",
            self.graph.nodes.len(),
            self.graph.connected_count,
            self.graph.orphan_count,
            self.graph.edges.len(),
            self.bone_count,
            self.vert_count,
        );
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) {
        if input.key_down(Key::Escape) {
            self.should_quit = true;
        }
        // Left-click selects the node under the cursor (empty floor deselects), revealing
        // its connections. Independent of the input mode.
        let ml = input.mouse_left;
        if ml && !self.prev_mouse_left {
            self.selected = self.pick_node(input.mouse_position, renderer.size());
        }
        self.prev_mouse_left = ml;

        // Tab toggles Camera ↔ Character control; entering Camera seeds the fly cam.
        let tab = input.key_down(Key::Tab);
        if tab && !self.prev_tab {
            self.mode = match self.mode {
                InputMode::Character => {
                    self.fly.sync_from(&self.orbit.camera());
                    InputMode::Camera
                }
                InputMode::Camera => {
                    // Keep the flown view instead of snapping back to the centre orbit.
                    self.orbit.sync_from_camera(&self.fly.camera());
                    InputMode::Character
                }
            };
        }
        self.prev_tab = tab;

        // Edge-detect the state-machine trigger keys (tracked regardless of mode).
        let space = input.key_down(Key::Space);
        let jump = space && !self.prev_jump;
        self.prev_jump = space;
        let fkey = input.key_down(Key::F);
        let attack = fkey && !self.prev_attack;
        self.prev_attack = fkey;
        let hkey = input.key_down(Key::H);
        let hit = hkey && !self.prev_hit;
        self.prev_hit = hkey;
        let xkey = input.key_down(Key::X);
        let die = xkey && !self.prev_die;
        self.prev_die = xkey;

        // In Character mode, WASD + the triggers drive the state machine; in Camera mode
        // they're inert (the character idles) and WASD flies the camera instead.
        let inputs = if matches!(self.mode, InputMode::Character) {
            let (w, a, s, d) = (
                input.key_down(Key::W),
                input.key_down(Key::A),
                input.key_down(Key::S),
                input.key_down(Key::D),
            );
            Inputs {
                move_: w || a || s || d,
                left: a,
                right: d,
                back: s,
                run: input.key_down(Key::LeftShift) || input.key_down(Key::RightShift),
                crouch: input.key_down(Key::C),
                jump,
                attack,
                hit: false, // hit → a RANDOM Dame below, not the single any-edge
                die,
            }
        } else {
            Inputs::default()
        };

        // H (hit test) → force a **random** hit reaction from the Dame group. The driver
        // owns the randomness so the state machine stays deterministic.
        if matches!(self.mode, InputMode::Character) && hit && !self.dame_states.is_empty() {
            let idx = (self.next_rand() % self.dame_states.len() as u64) as usize;
            let name = self.dame_states[idx].clone();
            if let Some(sm) = self.sm.as_mut() {
                sm.force_state_by_name(&name);
            }
        }

        match self.mode {
            InputMode::Camera => self.fly.update(dt.as_secs_f32(), input),
            InputMode::Character => self.orbit.update(input),
        }
        if let Some(sm) = self.sm.as_mut() {
            sm.advance(dt.as_secs_f32(), &inputs);
        }
        // The field of clips always loops.
        self.time += dt.as_secs_f32();

    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Poses first — both the paperdoll and the field read them this frame.
        self.rebuild_palettes();

        // PiP paperdoll: the **interactive character**, driven by the state machine, rendered
        // alone into the offscreen panel target. MUST run before any main-frame draws
        // (render_to_texture resets the draw queues).
        if let (Some(pd), Some(mesh), Some(pose)) = (self.paperdoll, self.mesh, self.pip_palette()) {
            let bc = self.bone_count as usize;
            let model = self.model.world; // character centred at the origin
            let cam = Camera::orbit(Vec3::ZERO, self.unit * 2.3, 0.6, 0.15);
            renderer.render_to_texture(pd, [0.0, 0.0, 0.0, 0.0], |r| {
                r.set_scene(pip_scene()); // keep the portrait well-lit against the night field
                r.set_camera(&cam);
                r.draw_skinned_instanced(mesh, &[model], &pose, bc as u32);
            });
        }

        // Celestial night sky over the field.
        renderer.set_scene(night_scene());
        renderer.set_camera(&self.camera());
        renderer.draw_sky();

        // Glass floor (translucent, so the night sky shows through it).
        if let Some(floor) = self.floor_mesh {
            renderer.draw_mesh(
                floor,
                Mat4::IDENTITY,
                MeshDrawOptions {
                    tint: [0.75, 0.82, 1.0, 0.42],
                    ..Default::default()
                },
            );
        }
        // Group-box outlines on the floor (depth-tested, no depth write).
        for (segs, color) in &self.box_outlines {
            renderer.draw_lines(segs, *color);
        }
        // A gold ground ring under every character.
        if !self.rings.is_empty() {
            renderer.draw_lines(&self.rings, [0.82, 0.68, 0.34, 0.55]);
        }
        // Selection: the picked node's ring turns orange/rust and ONLY its connections
        // are revealed (the HTML "select a node to see its wires" behaviour).
        if let Some(sel) = self.selected {
            let p = self.graph.nodes[sel].pos;
            let ring = circle_segments(p, self.ring_radius, self.line_y, 24);
            renderer.draw_lines(&ring, [1.0, 0.45, 0.12, 1.0]);
            if sel < self.graph.connected_count {
                let segs: Vec<(Vec3, Vec3)> = self
                    .graph
                    .edges
                    .iter()
                    .filter(|&&(a, b)| a == sel || b == sel)
                    .map(|&(a, b)| {
                        let pa = self.graph.nodes[a].pos;
                        let pb = self.graph.nodes[b].pos;
                        (Vec3::new(pa.x, self.line_y, pa.y), Vec3::new(pb.x, self.line_y, pb.y))
                    })
                    .collect();
                if !segs.is_empty() {
                    renderer.draw_lines(&segs, [0.55, 0.85, 1.0, 0.95]);
                }
            }
        }

        // Active state: the node the state machine is currently playing (as shown in the
        // PiP) glows green, with **arrows to every state reachable from it** — the graph
        // lights up as you drive the character through its cycles.
        if let Some(ai) = self.active_node() {
            let p = self.graph.nodes[ai].pos;
            let ring = circle_segments(p, self.ring_radius * 1.18, self.line_y, 28);
            renderer.draw_lines(&ring, [0.35, 1.0, 0.45, 1.0]);

            let mut targets = self.graph.out_edges[ai].clone();
            targets.extend(self.graph.any_targets.iter().copied()); // hit/die from anywhere
            targets.sort_unstable();
            targets.dedup();
            let from = Vec3::new(p.x, self.line_y, p.y);
            let mut arrows: Vec<(Vec3, Vec3)> = Vec::new();
            for &t in &targets {
                if t == ai {
                    continue;
                }
                let q = self.graph.nodes[t].pos;
                let to = Vec3::new(q.x, self.line_y, q.y);
                arrows.extend(arrow_segments(from, to, self.ring_radius * 1.2, self.unit * 0.8));
            }
            if !arrows.is_empty() {
                renderer.draw_lines(&arrows, [0.70, 1.0, 0.35, 0.95]);
            }
        }

        // Draw the whole field in one instanced call (poses already rebuilt above).
        if let (Some(mesh), false) = (self.mesh, self.models.is_empty()) {
            renderer.draw_skinned_instanced(mesh, &self.models, &self.palettes, self.bone_count);
        }

        // Volumetric ground fog — a raymarched drifting slab between the feet and the glass.
        // Correctly self-composites (integrated along the ray) and fades continuously.
        renderer.set_ground_fog(GroundFog {
            color: Vec3::new(0.42, 0.48, 0.60), // dimmer cool mist (was washing the models out)
            bottom: self.fog_bottom,
            top: self.fog_top,
            density: 0.5, // calmer (transparency/softness dialed in — leave it)
            noise_scale: 1.0 / (self.unit * 4.0), // blobs ~4 character-radii across
            coverage: 0.72, // more clouds (amount up; density unchanged)
            wind: Vec2::new(self.unit * 0.5, self.unit * 0.12),
            time: self.time,
            height_power: 1.6,
            // Localise the fog to the field floor, fading out over a soft outer margin.
            bounds_min: self.graph.floor_min,
            bounds_max: self.graph.floor_max,
            edge_fade: self.unit * 6.0,
            // Spill off the edges: a falling curtain that pours over the rim, drifting down.
            fall_depth: self.unit * 3.5,
            flow: self.unit * 1.5,
        });

        let sm_state = self.sm.as_ref().map(|s| s.current_state_name()).unwrap_or("—");
        let controls = match self.mode {
            InputMode::Character => "Tab: CHARACTER — WASD move · Shift run/still · C crouch · Space jump · F attack · H hit(random) · X die · click select · Esc",
            InputMode::Camera => "Tab: CAMERA — WASD fly · R/F up-down · Shift sprint · right-drag look · wheel speed · click select · Esc",
        };
        renderer.draw_text(
            &format!(
                "[FIELD] {} states · {} orphans   ·   PiP state: {sm_state}",
                self.graph.connected_count, self.graph.orphan_count,
            ),
            Vec2::new(16.0, 16.0),
            20.0,
            [1.0, 1.0, 1.0, 1.0],
        );
        renderer.draw_text(controls, Vec2::new(16.0, 42.0), 14.0, [0.80, 0.86, 0.95, 1.0]);
        let sel_line = match self.selected {
            Some(i) => {
                let label = self.graph.nodes[i].label.clone();
                let bx = self.box_label_of(i).unwrap_or("—").to_string();
                if i < self.graph.connected_count {
                    let links = self.graph.edges.iter().filter(|&&(a, b)| a == i || b == i).count();
                    format!("selected: {label} [{bx}]  ·  {links} connection(s)")
                } else {
                    format!("selected: {label} [{bx}]  ·  waiting clip")
                }
            }
            None => "click a node to reveal its connections".to_string(),
        };
        renderer.draw_text(&sel_line, Vec2::new(16.0, 62.0), 14.0, [0.95, 0.82, 0.55, 1.0]);

        // PiP panel (bottom-right) — the interactive character, rendered offscreen. Shown
        // whenever a state machine is driving it.
        if self.sm.is_some() {
            if let Some(pd) = self.paperdoll {
                if let Some(tex) = renderer.target_texture(pd) {
                    let screen = renderer.size();
                    let side = (screen.y * 0.32).clamp(160.0, 320.0);
                    let margin = 16.0;
                    let pos = Vec2::new(screen.x - side - margin, screen.y - side - margin);
                    // Dark backdrop quad behind the (transparent-background) portrait.
                    let tr = Vec2::new(pos.x + side, pos.y);
                    let br = Vec2::new(pos.x + side, pos.y + side);
                    let bl = Vec2::new(pos.x, pos.y + side);
                    let bg = [0.05, 0.06, 0.09, 0.88];
                    renderer.draw_triangle(pos, tr, br, bg);
                    renderer.draw_triangle(pos, br, bl, bg);
                    renderer.draw_sprite(tex, pos, Vec2::new(side, side), [1.0, 1.0, 1.0, 1.0]);
                    // A small label above the panel.
                    renderer.draw_text(
                        &format!("PiP · {sm_state}"),
                        Vec2::new(pos.x + 6.0, pos.y - 20.0),
                        16.0,
                        [0.85, 0.95, 0.80, 1.0],
                    );
                }
            }
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "flicker_packeditor=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    // Reuse the Katanami character bundle from the shared content tree (rig +
    // 91-clip library + state pack) — the same source flicker-paperdoll loads.
    let assets = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../content/characters/katanami"
    ));
    let model = format::load_dir(&assets)?;
    let pack = state::load_pack(&assets.join("Katanami.pack.json")).ok();
    tracing::info!(
        "loaded rig: {} bones, {} clips, mesh {} verts · state pack {}",
        model.bones.len(),
        model.clips.len(),
        model.mesh.vertices.len(),
        if pack.is_some() { "loaded" } else { "absent" },
    );
    run(Field::new(model, pack, "Katanami.pack"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets_dir() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../content/characters/katanami"
        ))
    }

    fn load() -> Option<(Model, Option<StateMachineDef>)> {
        let model = format::load_dir(&assets_dir()).ok()?;
        if model.mesh.vertices.is_empty() {
            return None;
        }
        let pack = state::load_pack(&assets_dir().join("Katanami.pack.json")).ok();
        Some((model, pack))
    }

    /// One instance per node, and the flat palette buffer is exactly `nodes × bones`
    /// finite matrices — the shape `draw_skinned_instanced` requires.
    #[test]
    fn field_builds_full_palettes_for_every_node() {
        let Some((model, pack)) = load() else {
            return;
        };
        let bones = model.bones.len();
        let field = Field::new(model, pack, "test.pack");
        let nodes = field.graph.nodes.len();
        assert!(nodes > 0, "the field has nodes");
        assert_eq!(field.models.len(), nodes, "one model matrix per node");
        assert_eq!(field.bone_count as usize, bones);
        assert_eq!(
            field.palettes.len(),
            nodes * bones,
            "flat palette = nodes × bones (draw_skinned_instanced contract)"
        );
        assert!(field.palettes.iter().all(|m| m.is_finite()));
    }

    /// The graph splits the library into connected states + orphan clips, wired by real
    /// transitions, and every layout position is finite.
    #[test]
    fn graph_has_states_orphans_and_links() {
        let Some((model, pack)) = load() else {
            return;
        };
        // This test only makes sense with the pack present.
        if pack.is_none() {
            return;
        }
        let clips = model.clips.len();
        let field = Field::new(model, pack, "test.pack");
        let g = &field.graph;
        assert!(g.connected_count >= 15, "expected ~19 states, got {}", g.connected_count);
        assert!(g.orphan_count > 0, "unused clips should be waiting nodes");
        // Reaction states live in the graph (their own Reactions box), not the waiting table.
        assert!(
            g.nodes[..g.connected_count].iter().any(|n| n.label == "Death_01"),
            "reaction states should be connected nodes"
        );
        assert_eq!(
            g.connected_count + g.orphan_count,
            clips,
            "every clip is either a state's clip or an orphan"
        );
        assert!(!g.edges.is_empty(), "the state graph has transition links");
        assert!(g.edges.iter().all(|&(a, b)| a < g.connected_count && b < g.connected_count));
        assert!(g.nodes.iter().all(|n| n.pos.is_finite()), "layout positions finite");
        // Everything stands on the floor container.
        assert!(
            g.nodes.iter().all(|n| n.pos.x >= g.floor_min.x
                && n.pos.x <= g.floor_max.x
                && n.pos.y >= g.floor_min.y
                && n.pos.y <= g.floor_max.y),
            "every node sits within the floor bounds"
        );
    }

    /// The scene is organised into labelled group boxes (a Movement box + at least one
    /// waiting box), and every node sits within the floor container.
    #[test]
    fn scene_has_group_boxes_and_everything_on_floor() {
        let Some((model, pack)) = load() else {
            return;
        };
        if pack.is_none() {
            return;
        }
        let field = Field::new(model, pack, "test.pack");
        let g = &field.graph;
        assert!(g.boxes.len() >= 4, "expected several group boxes, got {}", g.boxes.len());
        assert!(
            g.boxes.iter().any(|b| b.kind == BoxKind::Movement),
            "a Movement box"
        );
        assert!(
            g.boxes
                .iter()
                .any(|b| matches!(b.kind, BoxKind::WaitingInPlace | BoxKind::WaitingRoot)),
            "a waiting box"
        );
        assert!(
            g.nodes.iter().all(|n| n.pos.x >= g.floor_min.x
                && n.pos.x <= g.floor_max.x
                && n.pos.y >= g.floor_min.y
                && n.pos.y <= g.floor_max.y),
            "every node sits within the floor"
        );
    }

    /// Advancing the clock re-poses the field: palette shape preserved, stays finite, and
    /// at least one matrix changes.
    #[test]
    fn advancing_time_repositions_and_stays_finite() {
        let Some((model, pack)) = load() else {
            return;
        };
        let mut field = Field::new(model, pack, "test.pack");
        let at_zero = field.palettes.clone();
        field.time = 0.5;
        field.rebuild_palettes();
        assert_eq!(field.palettes.len(), at_zero.len());
        assert!(field.palettes.iter().all(|m| m.is_finite()));
        assert!(field.palettes.iter().zip(&at_zero).any(|(a, b)| *a != *b));
    }

    /// Swapping to the fly camera seeds it from the orbit view — no jump on Tab.
    #[test]
    fn fly_cam_syncs_from_orbit_without_jumping() {
        let Some((model, pack)) = load() else {
            return;
        };
        let mut field = Field::new(model, pack, "test.pack");
        let orbit_cam = field.orbit.camera();
        field.fly.sync_from(&orbit_cam);
        assert!((field.fly.position - orbit_cam.position).length() < 1e-3);
        let orbit_dir = (orbit_cam.target - orbit_cam.position).normalize_or_zero();
        assert!(field.fly.forward().dot(orbit_dir) > 0.999);
    }
}
