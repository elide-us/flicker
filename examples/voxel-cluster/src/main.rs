//! voxel-cluster: contour a noisy heightfield cluster and render it
//! through the 3D mesh pipeline with a full orbital camera plus the
//! cluster's bounding box as a 3D gizmo.
//!
//! Visual expectations:
//!   * A bumpy terrain surface centered at world Y ≈ 128, with per-
//!     column height variation of ±12 voxels. The top is undulating
//!     (not flat), and the sides of the heightfield are visible from
//!     oblique angles — vertical columns at x=0, x=256, z=0, z=256
//!     reveal the heightmap from the side, and the cluster's y=0
//!     floor is visible from below.
//!   * The surface is tiled with the magenta/black checker, one cell
//!     per voxel, modulated by Lambertian shading.
//!   * A bright-green wireframe overlay drawn from the deduplicated
//!     edge index buffer reveals the underlying triangulation.
//!   * A white wireframe cube outlines the cluster's full `(0,0,0)` to
//!     `(256,256,256)` extent. The back edges are occluded by the
//!     mesh when the camera is outside the cluster; from inside, all
//!     12 edges are visible.
//!
//! Controls:
//!   * Left-drag: orbit the camera around the orbit target.
//!   * Right-drag: pan the orbit target in the camera's local right/
//!     up plane (scales by distance so pan speed feels constant at
//!     every zoom level).
//!   * Scroll wheel / two-finger scroll: zoom toward/away from the
//!     target. Clamped to `5..=5000` so the user can get close enough
//!     to see individual voxel-cell features.
//!   * Escape: quit.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, App, InputState, Key};
use flicker::render::{Camera, Mat4, MeshDrawOptions, MeshHandle, Renderer, Vec2, Vec3};
use flicker_voxel::{contour_cluster, generators, Material, CLUSTER_DIM};

struct VoxelCluster {
    mesh: Option<MeshHandle>,
    vertex_count: usize,
    triangle_count: usize,

    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,

    previous_mouse_position: Option<Vec2>,
    should_quit: bool,
}

impl Default for VoxelCluster {
    fn default() -> Self {
        Self {
            mesh: None,
            vertex_count: 0,
            triangle_count: 0,
            yaw: 0.785,
            pitch: 0.4,
            distance: 500.0,
            target: Vec3::new(
                CLUSTER_DIM as f32 * 0.5,
                CLUSTER_DIM as f32 * 0.5,
                CLUSTER_DIM as f32 * 0.5,
            ),
            previous_mouse_position: None,
            should_quit: false,
        }
    }
}

impl VoxelCluster {
    /// Current orbit-camera world position derived from yaw/pitch/distance.
    fn camera_position(&self) -> Vec3 {
        let pitch = self.pitch.clamp(-1.5, 1.5);
        self.target
            + Vec3::new(
                self.distance * pitch.cos() * self.yaw.sin(),
                self.distance * pitch.sin(),
                self.distance * pitch.cos() * self.yaw.cos(),
            )
    }
}

impl App for VoxelCluster {
    fn init(&mut self, renderer: &mut Renderer) {
        tracing::info!("generating noisy terrain cluster …");
        let material = Material::new(7, 7, 7).expect("valid material");
        let cluster = generators::noisy_terrain(128, 12, 0xDEAD_BEEF, material);
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
    }

    fn update(&mut self, _dt: Duration, input: &InputState, _renderer: &Renderer) {
        if input.key_down(Key::Escape) {
            self.should_quit = true;
        }

        // Left-drag → orbit. dx → yaw, dy → pitch (inverted because
        // screen Y points down).
        if input.mouse_left {
            if let Some(prev) = self.previous_mouse_position {
                let delta = input.mouse_position - prev;
                self.yaw += delta.x * 0.005;
                self.pitch = (self.pitch - delta.y * 0.005).clamp(-1.5, 1.5);
            }
        }

        // Right-drag → pan the orbit target in the camera's local
        // right/up plane. Pan speed scales with distance so the same
        // screen displacement produces roughly the same world
        // displacement at every zoom level.
        if input.mouse_right {
            if let Some(prev) = self.previous_mouse_position {
                let delta = input.mouse_position - prev;
                let camera_position = self.camera_position();
                let forward = (self.target - camera_position).normalize_or_zero();
                if forward.length_squared() > 0.0 {
                    let right = forward.cross(Vec3::Y).normalize_or_zero();
                    let up = right.cross(forward).normalize_or_zero();
                    let pan_scale = self.distance * 0.0015;
                    self.target -= right * delta.x * pan_scale;
                    self.target += up * delta.y * pan_scale;
                }
            }
        }

        self.previous_mouse_position = Some(input.mouse_position);

        // Scroll → zoom. One notch (~1.0) shrinks/grows distance by 10%.
        // Clamp at 5..=5000 so the user can get close enough to see
        // individual voxel-cell features without flying through the
        // mesh or losing it at infinity.
        if input.mouse_wheel_delta != 0.0 {
            let factor = 1.0 - input.mouse_wheel_delta * 0.1;
            self.distance = (self.distance * factor).clamp(5.0, 5000.0);
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(mesh) = self.mesh else {
            return;
        };
        renderer.set_camera(&Camera::orbit(
            self.target,
            self.distance,
            self.yaw,
            self.pitch,
        ));
        renderer.draw_mesh(mesh, Mat4::IDENTITY, MeshDrawOptions::default());
        renderer.draw_mesh(
            mesh,
            Mat4::IDENTITY,
            MeshDrawOptions {
                wireframe: true,
                ..MeshDrawOptions::default()
            },
        );
        // White wireframe outline around the cluster's full extent.
        renderer.draw_bounding_box(
            Vec3::ZERO,
            Vec3::splat(CLUSTER_DIM as f32),
            [1.0, 1.0, 1.0, 1.0],
        );

        renderer.draw_text(
            "voxel cluster — left=orbit, right=pan, wheel=zoom",
            Vec2::new(16.0, 16.0),
            22.0,
            [1.0, 1.0, 1.0, 1.0],
        );
        renderer.draw_text(
            &format!(
                "yaw: {:.2}, pitch: {:.2}, distance: {:.0}",
                self.yaw, self.pitch, self.distance
            ),
            Vec2::new(16.0, 44.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "target: ({:.0}, {:.0}, {:.0})",
                self.target.x, self.target.y, self.target.z
            ),
            Vec2::new(16.0, 64.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "vertices: {}, triangles: {}",
                self.vertex_count, self.triangle_count
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
