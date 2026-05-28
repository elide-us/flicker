//! mesh-smoke: validates the 3D mesh pipeline end-to-end.
//!
//! Visual expectations when running:
//!   * A shaded cube sits at the origin, rotating slowly under a slow
//!     orbiting camera.
//!   * Faces are tiled with the procedural magenta/black missing-
//!     texture checker (one cell per world-space unit), modulated by
//!     simple Lambertian lighting — the lit faces should appear
//!     distinctly brighter than the back-facing ones.
//!   * A bright-green wireframe overlay sits on top of the fill, drawn
//!     by re-issuing `draw_mesh` with `wireframe: true`. Edges should
//!     appear as one-pixel-wide lines along every triangle edge of the
//!     cube, including the diagonal of each face (each face is two
//!     triangles). The wireframe is a separate line-list draw against
//!     an edge index buffer the pipeline builds at upload time, so it
//!     works on shared-vertex meshes too.
//!   * Debug text in the top-left reads `fill + wireframe smoke check,
//!     frame N`.
//!   * No z-fighting between fill and wireframe: the depth pipeline
//!     uses `LessEqual`, so the line-list draw lands on top of the
//!     just-written fill depth values cleanly.
//!   * Escape exits cleanly.
//!
//! The cube uses per-face duplicated vertices, but that's no longer a
//! requirement for the wireframe to render correctly — the line-list
//! pipeline doesn't care whether triangles share vertices. The cube
//! keeps its duplicated vertices to make per-face material ids well-
//! defined.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, App, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, Vec2, Vec3,
};

struct MeshSmoke {
    mesh: Option<MeshHandle>,
    frame: u64,
    should_quit: bool,
}

impl App for MeshSmoke {
    fn init(&mut self, renderer: &mut Renderer) {
        let (vertices, indices) = cube_mesh(80.0);
        self.mesh = Some(renderer.upload_mesh(&vertices, MeshIndices::U16(&indices)));
        tracing::info!("mesh-smoke initialized");
    }

    fn update(&mut self, _dt: Duration, input: &InputState, _renderer: &Renderer) {
        if input.key_down(Key::Escape) {
            self.should_quit = true;
        }
        self.frame = self.frame.wrapping_add(1);
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(mesh) = self.mesh else {
            return;
        };

        // Camera orbits slowly around the origin at a fixed altitude.
        let angle = (self.frame as f32) * 0.005;
        let radius = 220.0;
        let camera = Camera {
            position: Vec3::new(angle.cos() * radius, 120.0, angle.sin() * radius),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 1000.0,
        };
        renderer.set_camera(&camera);

        // Slight spin on the cube itself so the wireframe and fill both
        // move; helps eyeball any z-fighting.
        let spin = (self.frame as f32) * 0.01;
        let model = Mat4::from_rotation_y(spin);

        // Pass 1: filled, no tint.
        renderer.draw_mesh(mesh, model, MeshDrawOptions::default());
        // Pass 2: wireframe overlay using the same mesh handle.
        renderer.draw_mesh(
            mesh,
            model,
            MeshDrawOptions {
                wireframe: true,
                ..MeshDrawOptions::default()
            },
        );

        renderer.draw_text(
            &format!("fill + wireframe smoke check, frame {}", self.frame),
            Vec2::new(16.0, 16.0),
            22.0,
            [1.0, 1.0, 1.0, 1.0],
        );
        renderer.draw_text(
            "press Escape to quit",
            Vec2::new(16.0, 44.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
    }
}

/// Build a unit cube of the given half-extent. Vertices are duplicated
/// per face (not shared) so the barycentric wireframe technique gives
/// crisp edges along every triangle — including the per-face diagonal.
fn cube_mesh(half: f32) -> (Vec<MeshVertex>, Vec<u16>) {
    // 6 faces × 4 vertices = 24 vertices.
    // 6 faces × 2 triangles = 12 triangles = 36 indices.
    let mut vertices: Vec<MeshVertex> = Vec::with_capacity(24);
    let mut indices: Vec<u16> = Vec::with_capacity(36);

    // For each face: (normal, four corner positions in CCW order viewed
    // from outside the cube, material id).
    let faces: [([f32; 3], [[f32; 3]; 4], u32); 6] = [
        // +X face
        (
            [1.0, 0.0, 0.0],
            [
                [half, -half, half],
                [half, -half, -half],
                [half, half, -half],
                [half, half, half],
            ],
            1,
        ),
        // -X face
        (
            [-1.0, 0.0, 0.0],
            [
                [-half, -half, -half],
                [-half, -half, half],
                [-half, half, half],
                [-half, half, -half],
            ],
            2,
        ),
        // +Y face
        (
            [0.0, 1.0, 0.0],
            [
                [-half, half, half],
                [half, half, half],
                [half, half, -half],
                [-half, half, -half],
            ],
            3,
        ),
        // -Y face
        (
            [0.0, -1.0, 0.0],
            [
                [-half, -half, -half],
                [half, -half, -half],
                [half, -half, half],
                [-half, -half, half],
            ],
            4,
        ),
        // +Z face
        (
            [0.0, 0.0, 1.0],
            [
                [-half, -half, half],
                [half, -half, half],
                [half, half, half],
                [-half, half, half],
            ],
            5,
        ),
        // -Z face
        (
            [0.0, 0.0, -1.0],
            [
                [half, -half, -half],
                [-half, -half, -half],
                [-half, half, -half],
                [half, half, -half],
            ],
            6,
        ),
    ];

    for (normal, corners, material) in &faces {
        let base = vertices.len() as u16;
        for pos in corners {
            vertices.push(MeshVertex {
                position: *pos,
                normal: *normal,
                material: *material,
            });
        }
        // Two triangles, CCW from outside.
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    (vertices, indices)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mesh_smoke=info,flicker_app=info,flicker_render=warn".into()),
        )
        .init();

    let app = MeshSmoke {
        mesh: None,
        frame: 0,
        should_quit: false,
    };
    run(app)?;
    Ok(())
}
