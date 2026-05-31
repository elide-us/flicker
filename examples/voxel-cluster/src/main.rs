//! voxel-cluster: the QEF-contouring playground — one cluster contoured
//! from the split-scene world (procedural heightmap in the lower band,
//! six analytic-primitive shapes floating above), flown through with
//! MMO-style first-person controls.
//!
//! Pipeline: `Scene::world()` → `contour` → `Cluster` → `mesh` → upload.
//! The cluster boundary is drawn as a white wireframe box. Two debug
//! toggles let the user inspect the QEF result interactively (see the
//! controls list below).
//!
//! Controls (rebindable via `Bindings`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * `1`: toggle wireframe overlay on top of the solid mesh.
//!   * `2`: toggle corner-vector arrows — for every stored voxel whose
//!     `CornerVector` differs from the default, draw a line from the
//!     voxel's grid coord to the decoded corner tip. Visualizes where
//!     the contour's QEF placed each active cell's dual vertex.
//!   * Escape: quit.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, App, Bindings, ControlConfig, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, TextureHandle,
    Vec2, Vec3,
};
use flicker_voxel::{contour, ClusterId, ClusterMap, CornerVector, Material, Scene, CLUSTER_DIM};

/// Axis-aligned rectangle in HUD pixel space. Retained as part of the
/// sprite-UI capability (see `draw_checkbox`); no active widgets use it
/// in this minimal build.
#[allow(dead_code)]
#[derive(Copy, Clone, Debug)]
struct Rect {
    top_left: Vec2,
    size: Vec2,
}

struct VoxelCluster {
    /// The cluster map — data-context wiring retained even though the
    /// scene holds a single empty cluster. The QEF work populates and
    /// contours these.
    map: ClusterMap,

    /// A 1×1 white pixel uploaded once at `init`. The sprite shader
    /// multiplies it by a tint, so this is the "solid colored quad"
    /// primitive for HUD widgets. Retained sprite-UI capability — no
    /// active widgets draw it in this minimal build.
    #[allow(dead_code)]
    white: Option<TextureHandle>,

    /// Uploaded mesh of the contoured cluster. Populated in `init` and
    /// drawn each frame at the cluster's world offset.
    mesh: Option<MeshHandle>,

    /// First-person camera state.
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Last cursor position while right-dragging, so we can compute a
    /// per-frame delta. `None` when right is not held.
    last_look_cursor: Option<Vec2>,

    bindings: Bindings,
    config: ControlConfig,

    /// `1` toggles a wireframe-overlay second pass on top of the solid
    /// mesh. Off by default.
    wireframe_on: bool,
    /// `2` toggles drawing corner-vector arrows (precomputed in `init`).
    /// Off by default.
    corner_arrows_on: bool,
    /// Held-state on the previous frame for `1`/`2` so the toggles flip
    /// on the press edge instead of every frame the key is down.
    prev_key1: bool,
    prev_key2: bool,
    /// Cached line segments: from each stored voxel's grid coord to its
    /// decoded `CornerVector` tip, for every voxel whose corner differs
    /// from the default. Computed once per cluster (re)build in `init`.
    corner_arrows: Vec<(Vec3, Vec3)>,

    should_quit: bool,
}

impl Default for VoxelCluster {
    fn default() -> Self {
        // Camera gets its real pose in `init`. The placeholders here
        // just satisfy the Default bound; nothing renders before init.
        Self {
            map: ClusterMap::new(),
            white: None,
            mesh: None,
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            last_look_cursor: None,
            bindings: Bindings::wasd(),
            config: ControlConfig::default(),
            wireframe_on: false,
            corner_arrows_on: false,
            prev_key1: false,
            prev_key2: false,
            corner_arrows: Vec::new(),
            should_quit: false,
        }
    }
}

impl VoxelCluster {
    /// Unit vector pointing where the camera is looking, derived from
    /// yaw/pitch. Right-handed Y-up.
    fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos())
    }

    /// Horizontal "right" relative to the camera's facing (ignores
    /// pitch so strafing stays in the world XZ plane).
    fn move_right(&self) -> Vec3 {
        let f = self.forward();
        let flat = Vec3::new(f.x, 0.0, f.z).normalize_or_zero();
        flat.cross(Vec3::Y).normalize_or_zero()
    }

    /// Horizontal forward (ignores pitch so WASD doesn't pitch into
    /// the ground or sky).
    fn move_forward(&self) -> Vec3 {
        let f = self.forward();
        Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
    }
}

impl App for VoxelCluster {
    fn init(&mut self, renderer: &mut Renderer) {
        // One cluster at the origin, contoured from the split-scene world:
        // the procedural heightmap fills the lower band and the six
        // analytic-primitive shapes float clear of the terrain above. The
        // mesh-regen stage reads the contoured cluster back out as
        // triangles.
        let material = Material::new(1, 1, 0).expect("grey material is in-range");
        let cluster = contour(&Scene::world(), material);

        let cm = flicker_voxel::mesh(&cluster);
        let verts: Vec<MeshVertex> = cm
            .vertices
            .iter()
            .map(|v| MeshVertex {
                position: v.position,
                normal: v.normal,
                material: v.material,
            })
            .collect();
        self.mesh = Some(renderer.upload_mesh(&verts, MeshIndices::U32(&cm.indices)));

        // Precompute corner-vector arrows: from each stored voxel's grid
        // coord (its integer min corner) to the decoded `CornerVector`
        // tip. Voxels at the default corner (e.g. interior solid voxels
        // whose cell isn't active) point at their own center; we skip
        // them since the arrow would just be a half-voxel stub and we
        // have ~8M solid voxels in the terrain. Active-cell min-corner
        // voxels carry the QEF-placed dual vertex and produce the
        // visually interesting arrows.
        let cluster_offset = ClusterId::new(0, 0, 0, 0).world_offset();
        let cluster_origin =
            Vec3::new(cluster_offset[0], cluster_offset[1], cluster_offset[2]);
        let mut arrows: Vec<(Vec3, Vec3)> = Vec::new();
        for (coord, voxel) in cluster.overrides() {
            if voxel.corner() == CornerVector::DEFAULT {
                continue;
            }
            let origin = cluster_origin
                + Vec3::new(coord.x() as f32, coord.y() as f32, coord.z() as f32);
            let [dx, dy, dz] = voxel.corner().to_components();
            let tip = origin + Vec3::new(dx, dy, dz);
            arrows.push((origin, tip));
        }
        self.corner_arrows = arrows;

        let mut map = ClusterMap::new();
        map.insert(ClusterId::new(0, 0, 0, 0), cluster);
        self.map = map;

        // Spawn outside the cluster looking back at it, angled down so
        // the whole 256³ box frames on the first frame.
        self.position = Vec3::new(128.0, 340.0, -180.0);
        self.yaw = 0.0; // face +Z, toward the box.
        self.pitch = -0.6; // look down at it.

        // 1×1 white pixel — tinted to build solid colored HUD quads.
        // Retained sprite-UI capability; no active widgets yet.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) {
        if input.action_active(&self.bindings, Action::Quit) {
            self.should_quit = true;
            return;
        }
        let dt_s = dt.as_secs_f32();

        // Debug toggles: flip on the press edge so holding the key
        // doesn't oscillate the bool every frame.
        let cur1 = input.key_down(Key::Digit1);
        if cur1 && !self.prev_key1 {
            self.wireframe_on = !self.wireframe_on;
        }
        self.prev_key1 = cur1;
        let cur2 = input.key_down(Key::Digit2);
        if cur2 && !self.prev_key2 {
            self.corner_arrows_on = !self.corner_arrows_on;
        }
        self.prev_key2 = cur2;

        // Look: right-drag, with invert/sensitivity applied by config.
        if input.mouse_right {
            if let Some(prev) = self.last_look_cursor {
                let (dyaw, dpitch) = self.config.look_delta(input.mouse_position - prev);
                self.yaw -= dyaw;
                self.pitch = (self.pitch + dpitch).clamp(-1.5, 1.5);
            }
            self.last_look_cursor = Some(input.mouse_position);
        } else {
            self.last_look_cursor = None;
        }

        // Movement: query actions, not keys.
        let mut motion = Vec3::ZERO;
        if input.action_active(&self.bindings, Action::MoveForward) {
            motion += self.move_forward();
        }
        if input.action_active(&self.bindings, Action::MoveBackward) {
            motion -= self.move_forward();
        }
        if input.action_active(&self.bindings, Action::StrafeRight) {
            motion += self.move_right();
        }
        if input.action_active(&self.bindings, Action::StrafeLeft) {
            motion -= self.move_right();
        }
        if input.action_active(&self.bindings, Action::MoveUp) {
            motion += Vec3::Y;
        }
        if input.action_active(&self.bindings, Action::MoveDown) {
            motion -= Vec3::Y;
        }
        if motion.length_squared() > 0.0 {
            self.position += motion.normalize() * self.config.move_speed * dt_s;
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
        });

        // Draw each cluster's extent as a white wireframe box.
        for (id, _cluster) in self.map.iter() {
            let offset = id.world_offset();
            let min = Vec3::new(offset[0], offset[1], offset[2]);
            let max = min + Vec3::splat(CLUSTER_DIM as f32);
            renderer.draw_bounding_box(min, max, [1.0, 1.0, 1.0, 1.0]);
        }

        // Draw the contoured cluster's mesh at its world offset. (For a
        // single cluster at (0,0,0,0) the offset is zero; the translation
        // is here so adding more clusters is a one-line change.) When
        // the wireframe toggle is on, re-issue the draw with
        // `wireframe: true` for a fill+wires overlay (see mesh-smoke).
        if let Some(mesh) = self.mesh {
            let o = ClusterId::new(0, 0, 0, 0).world_offset();
            let model = Mat4::from_translation(Vec3::new(o[0], o[1], o[2]));
            renderer.draw_mesh(mesh, model, MeshDrawOptions::default());
            if self.wireframe_on {
                renderer.draw_mesh(
                    mesh,
                    model,
                    MeshDrawOptions {
                        wireframe: true,
                        ..Default::default()
                    },
                );
            }
        }

        // Corner-vector arrows: every stored voxel with a non-default
        // corner contributes one segment from its grid coord to the
        // decoded tip. Orange so it reads against the grey mesh.
        if self.corner_arrows_on && !self.corner_arrows.is_empty() {
            renderer.draw_lines(&self.corner_arrows, [1.0, 0.6, 0.15, 1.0]);
        }

        // HUD text.
        renderer.draw_text(
            "voxel cluster — split scene — WASD move, R/F up/down, right-drag look",
            Vec2::new(16.0, 16.0),
            22.0,
            [1.0, 1.0, 1.0, 1.0],
        );
        renderer.draw_text(
            &format!(
                "pos: ({:.0}, {:.0}, {:.0})  yaw: {:.2}  pitch: {:.2}",
                self.position.x, self.position.y, self.position.z, self.yaw, self.pitch
            ),
            Vec2::new(16.0, 44.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "clusters: {}   extent: {}³ voxels",
                self.map.len(),
                CLUSTER_DIM
            ),
            Vec2::new(16.0, 64.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "config — speed: {:.0}  sens: {:.4}  invert-Y: {}  invert-X: {}",
                self.config.move_speed,
                self.config.look_sensitivity,
                self.config.invert_pitch,
                self.config.invert_yaw,
            ),
            Vec2::new(16.0, 84.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "[1] wireframe: {}    [2] corner arrows: {} ({} stored)",
                if self.wireframe_on { "ON " } else { "off" },
                if self.corner_arrows_on { "ON " } else { "off" },
                self.corner_arrows.len(),
            ),
            Vec2::new(16.0, 104.0),
            16.0,
            [0.85, 0.85, 0.7, 1.0],
        );
        renderer.draw_text(
            "press Escape to quit",
            Vec2::new(16.0, 124.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
    }
}

/// Render a checkbox at `rect`. Filled neon green when `checked`, dark
/// grey when not. Retained sprite-UI capability — the widget future
/// HUD controls reuse; no active widgets call it in this minimal build.
#[allow(dead_code)]
fn draw_checkbox(renderer: &mut Renderer, white: TextureHandle, rect: Rect, checked: bool) {
    renderer.draw_sprite(white, rect.top_left, rect.size, [1.0, 1.0, 1.0, 1.0]);
    let inset = 2.0_f32;
    let inner_color = if checked {
        [0.2, 0.9, 0.4, 1.0]
    } else {
        [0.15, 0.15, 0.18, 1.0]
    };
    renderer.draw_sprite(
        white,
        rect.top_left + Vec2::splat(inset),
        rect.size - Vec2::splat(inset * 2.0),
        inner_color,
    );
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "voxel_cluster=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    run(VoxelCluster::default())?;
    Ok(())
}
