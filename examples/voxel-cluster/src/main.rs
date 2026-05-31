//! voxel-cluster: the minimal voxel scene — one bare cluster, drawn
//! as a wireframe boundary box, flown through with MMO-style
//! first-person controls.
//!
//! This is the stripped foundation the QEF contouring work builds on.
//! It exercises the data-context wiring (a `Cluster` held in a
//! `ClusterMap`, addressed by `ClusterId`) and the render/controls/HUD
//! infrastructure, with no contouring, meshing, or procedural content.
//! The cluster is empty: there is nothing to contour yet, so nothing
//! is drawn but its extent.
//!
//! Controls (rebindable via `Bindings`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * Escape: quit.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, App, Bindings, ControlConfig, InputState};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, TextureHandle,
    Vec2, Vec3,
};
use flicker_voxel::{contour, ClusterId, ClusterMap, Material, Scene, CLUSTER_DIM};

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
        // One cluster at the origin, contoured from the analytic-primitive
        // gallery — six SDF shapes (sphere, cube, cylinder, cone, dome,
        // half-cylinder) unioned in a single contour pass. The mesh-regen
        // stage reads the contoured cluster back out as triangles.
        let material = Material::new(1, 1, 0).expect("grey material is in-range");
        let cluster = contour(&Scene::gallery(), material);

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
        // is here so adding more clusters is a one-line change.)
        if let Some(mesh) = self.mesh {
            let o = ClusterId::new(0, 0, 0, 0).world_offset();
            let model = Mat4::from_translation(Vec3::new(o[0], o[1], o[2]));
            renderer.draw_mesh(mesh, model, MeshDrawOptions::default());
        }

        // HUD text.
        renderer.draw_text(
            "voxel cluster — primitive gallery — WASD move, R/F up/down, right-drag look",
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
            "press Escape to quit",
            Vec2::new(16.0, 104.0),
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
