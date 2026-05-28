//! voxel-cluster: contour a stepped island-world fixture and fly
//! through it with MMO-style first-person controls. Includes a
//! clickable wireframe-toggle checkbox in the HUD as a smoke test of
//! the just-pressed mouse-button edge.
//!
//! Visual content (unchanged from the prior orbit-camera version):
//!   * A central island dome rising from a sea-floor base.
//!   * A recessed lake to one side with stepped interior walls.
//!   * A taller volcano cone — fly over it to see the crater.
//!   * Several puffy clouds floating above the terrain.
//!   * The magenta/black checker tiles every surface, one cell per
//!     voxel. A toggleable green wireframe overlay traces every face.
//!   * A white wireframe cube outlines the cluster's `(0,0,0)` to
//!     `(256,256,256)` extent.
//!
//! Controls (rebindable via [`Bindings`]):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch (sensitivity + invert
//!     configurable via [`ControlConfig`]).
//!   * Mouse: free cursor. Click the wireframe checkbox in the HUD to
//!     toggle the line-list overlay on/off.
//!   * Escape: quit.
//!
//! Rebinding example: change `Bindings::wasd()` to `Bindings::esdf()`
//! in the struct default, or set `config.invert_pitch = true`, and the
//! camera updates without any other code changes — the example only
//! reads `Action`s and `config.look_delta(...)`.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, App, Bindings, ControlConfig, InputState};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, Renderer, TextureHandle, Vec2, Vec3,
};
use flicker_voxel::{contour_cluster, generators, Material, CLUSTER_DIM};

/// Axis-aligned rectangle in HUD pixel space.
#[derive(Copy, Clone, Debug)]
struct Rect {
    top_left: Vec2,
    size: Vec2,
}

impl Rect {
    fn contains(self, p: Vec2) -> bool {
        p.x >= self.top_left.x
            && p.y >= self.top_left.y
            && p.x < self.top_left.x + self.size.x
            && p.y < self.top_left.y + self.size.y
    }
}

/// Where the wireframe-toggle checkbox lives in screen space.
fn wireframe_checkbox_rect() -> Rect {
    Rect {
        top_left: Vec2::new(16.0, 168.0),
        size: Vec2::new(18.0, 18.0),
    }
}

struct VoxelCluster {
    mesh: Option<MeshHandle>,
    /// A 1×1 white pixel uploaded once at `init`. The sprite shader
    /// multiplies it by a tint, so this serves as the "solid colored
    /// quad" for HUD widgets like the checkbox.
    white: Option<TextureHandle>,
    vertex_count: usize,
    triangle_count: usize,

    /// First-person camera state.
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Last cursor position while right-dragging, so we can compute a
    /// per-frame delta. `None` when right is not held.
    last_look_cursor: Option<Vec2>,

    bindings: Bindings,
    config: ControlConfig,

    show_wireframe: bool,
    should_quit: bool,
}

impl Default for VoxelCluster {
    fn default() -> Self {
        Self {
            mesh: None,
            white: None,
            vertex_count: 0,
            triangle_count: 0,
            // Start outside and a bit above the cluster, looking
            // toward its center.
            position: Vec3::new(
                CLUSTER_DIM as f32 * 0.5 - 220.0,
                CLUSTER_DIM as f32 * 0.5 + 80.0,
                CLUSTER_DIM as f32 * 0.5 - 220.0,
            ),
            yaw: 0.785,
            pitch: -0.25,
            last_look_cursor: None,
            bindings: Bindings::wasd(),
            config: ControlConfig::default(),
            show_wireframe: true,
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
        tracing::info!("generating island-world cluster …");
        let material = Material::new(7, 7, 7).expect("valid material");
        let cluster = generators::island_world(0xBEEF_F00D, material);
        tracing::info!(
            "cluster has {} overrides; contouring …",
            cluster.override_count()
        );
        let mesh = contour_cluster(&cluster);
        self.vertex_count = mesh.metadata().vertex_count;
        self.triangle_count = mesh.metadata().triangle_count;
        tracing::info!(
            "mesh: {} vertices, {} triangles",
            self.vertex_count,
            self.triangle_count
        );
        self.mesh = Some(renderer.upload_voxel_mesh(&mesh));

        // 1×1 white pixel — tinted in `draw_sprite` to build solid
        // colored HUD quads.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
    }

    fn update(&mut self, dt: Duration, input: &InputState, _r: &Renderer) {
        if input.action_active(&self.bindings, Action::Quit) {
            self.should_quit = true;
            return;
        }
        let dt_s = dt.as_secs_f32();

        // Look: right-drag, with invert/sensitivity applied by config.
        // Yaw is negated here so a rightward cursor drag rotates the
        // camera to the right (matches the corrected strafe basis).
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

        // UI: checkbox toggle on the just-pressed edge.
        if input.mouse_left_pressed && wireframe_checkbox_rect().contains(input.mouse_position) {
            self.show_wireframe = !self.show_wireframe;
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(mesh) = self.mesh else {
            return;
        };
        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
        });
        renderer.draw_mesh(mesh, Mat4::IDENTITY, MeshDrawOptions::default());
        if self.show_wireframe {
            renderer.draw_mesh(
                mesh,
                Mat4::IDENTITY,
                MeshDrawOptions {
                    wireframe: true,
                    ..MeshDrawOptions::default()
                },
            );
        }
        renderer.draw_bounding_box(
            Vec3::ZERO,
            Vec3::splat(CLUSTER_DIM as f32),
            [1.0, 1.0, 1.0, 1.0],
        );

        // HUD text.
        renderer.draw_text(
            "voxel cluster — WASD move, R/F up/down, right-drag look",
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
                "vertices: {}  triangles: {}",
                self.vertex_count, self.triangle_count
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

        // Wireframe checkbox.
        if let Some(white) = self.white {
            let rect = wireframe_checkbox_rect();
            // Box outline (white border).
            renderer.draw_sprite(white, rect.top_left, rect.size, [1.0, 1.0, 1.0, 1.0]);
            // Inner fill: green when on, dark grey when off, inset 2px.
            let inset = 2.0_f32;
            let inner_color = if self.show_wireframe {
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
        renderer.draw_text(
            "wireframe overlay (click to toggle)",
            Vec2::new(40.0, 170.0),
            14.0,
            [0.75, 0.85, 0.95, 1.0],
        );
    }
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
