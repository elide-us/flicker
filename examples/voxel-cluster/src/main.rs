//! voxel-cluster: a 3×3 cluster field contoured from the split-scene
//! world. Each cluster contours its own region against the shared
//! global primitive; meshing closes the four internal seams (and the
//! interior cluster's all four faces) via the low-side-owns convention
//! in `flicker_voxel::mesh`.
//!
//! Pipeline: 3×3 `ClusterId`s → `contour` per cluster → `ClusterMap`
//! → per-cluster `NeighborContext` → `mesh` → upload one mesh handle
//! per cluster, drawn at its `world_offset()`. The cluster boundary is
//! drawn as a white wireframe box; two debug toggles let the user
//! inspect the meshes interactively (see controls below).
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
use flicker_voxel::{
    contour, ClusterId, ClusterMap, CornerVector, Lod, Material, NeighborContext, Scene,
    CLUSTER_DIM,
};

/// Side length of the cluster field, in clusters. A 3×3 row in XZ
/// gives one fully-interior cluster (all four lateral neighbors
/// present), which is what actually exercises seam tangent stitching
/// on every face simultaneously.
const FIELD_DIM: u16 = 3;

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
    /// The cluster map — populated with `FIELD_DIM × FIELD_DIM`
    /// clusters at LOD 0, each contoured against the shared world
    /// primitive at its own world offset.
    map: ClusterMap,

    /// A 1×1 white pixel uploaded once at `init`. The sprite shader
    /// multiplies it by a tint, so this is the "solid colored quad"
    /// primitive for HUD widgets. Retained sprite-UI capability — no
    /// active widgets draw it in this minimal build.
    #[allow(dead_code)]
    white: Option<TextureHandle>,

    /// One mesh handle per cluster, paired with the cluster's id so
    /// `render` can draw each at its world offset.
    meshes: Vec<(ClusterId, MeshHandle)>,

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
    /// Cached line segments: from each stored voxel's world grid coord
    /// to its decoded `CornerVector` tip, across all clusters in the
    /// field.
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
            meshes: Vec::new(),
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
        let material = Material::new(1, 1, 0).expect("grey material is in-range");

        // Contour `FIELD_DIM × FIELD_DIM` clusters at y=0. Each cluster
        // gets its own `Scene::world_at(offset)` so the heightmap cache
        // covers that cluster's XZ footprint; the analytic gallery
        // primitives sit at fixed world coordinates and naturally
        // appear only in the cluster that contains them. The heightmap
        // is globally continuous because OOB cache hits fall through
        // to the procedural sampler.
        let ids: Vec<ClusterId> = (0..FIELD_DIM)
            .flat_map(|x| (0..FIELD_DIM).map(move |z| ClusterId::new(0, x, 0, z)))
            .collect();
        for id in &ids {
            let scene = Scene::world_at(id.world_offset());
            self.map.insert(*id, contour(&scene, material, *id));
        }

        // Build per-cluster neighbor contexts and mesh each. Looking up
        // by id rather than reordered index keeps this readable when
        // FIELD_DIM grows. All references stay borrowed from
        // `self.map`, which is alive for the whole `init` scope.
        let lookup = |id: ClusterId| self.map.get(id);
        for id in &ids {
            let x = id.x();
            let z = id.z();
            let neg_x = if x > 0 {
                lookup(ClusterId::new(0, x - 1, 0, z)).map(|c| (c, Lod::ZERO))
            } else {
                None
            };
            let pos_x = if x + 1 < FIELD_DIM {
                lookup(ClusterId::new(0, x + 1, 0, z)).map(|c| (c, Lod::ZERO))
            } else {
                None
            };
            let neg_z = if z > 0 {
                lookup(ClusterId::new(0, x, 0, z - 1)).map(|c| (c, Lod::ZERO))
            } else {
                None
            };
            let pos_z = if z + 1 < FIELD_DIM {
                lookup(ClusterId::new(0, x, 0, z + 1)).map(|c| (c, Lod::ZERO))
            } else {
                None
            };
            let neighbors = NeighborContext {
                neg_x,
                pos_x,
                neg_z,
                pos_z,
                ..NeighborContext::none()
            };

            let cluster = lookup(*id).expect("just inserted");
            let cm = flicker_voxel::mesh(cluster, &neighbors, Lod::ZERO);
            let verts: Vec<MeshVertex> = cm
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.position,
                    normal: v.normal,
                    material: v.material,
                })
                .collect();
            let handle = renderer.upload_mesh(&verts, MeshIndices::U32(&cm.indices));
            self.meshes.push((*id, handle));
        }

        // Corner-vector arrows: across the whole field, every stored
        // voxel with a non-default corner contributes one segment. The
        // arrow base is the voxel's WORLD grid coord (so adjacent
        // clusters' arrows don't pile on top of each other).
        let mut arrows: Vec<(Vec3, Vec3)> = Vec::new();
        for id in &ids {
            let off = id.world_offset();
            let origin_world = Vec3::new(off[0], off[1], off[2]);
            let cluster = lookup(*id).expect("just inserted");
            for (coord, voxel) in cluster.overrides() {
                if voxel.corner() == CornerVector::DEFAULT {
                    continue;
                }
                let base = origin_world
                    + Vec3::new(coord.x() as f32, coord.y() as f32, coord.z() as f32);
                let [dx, dy, dz] = voxel.corner().to_components();
                let tip = base + Vec3::new(dx, dy, dz);
                arrows.push((base, tip));
            }
        }
        self.corner_arrows = arrows;

        // Frame the whole field from outside its -Z face, angled down.
        let field_extent = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        let center_x = field_extent * 0.5;
        self.position = Vec3::new(center_x, field_extent * 1.1, -field_extent * 0.5);
        self.yaw = 0.0; // face +Z, into the field.
        self.pitch = -0.55; // look down at it.

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

        // Draw each cluster's mesh at its world offset.
        for (id, handle) in &self.meshes {
            let o = id.world_offset();
            let model = Mat4::from_translation(Vec3::new(o[0], o[1], o[2]));
            renderer.draw_mesh(*handle, model, MeshDrawOptions::default());
            if self.wireframe_on {
                renderer.draw_mesh(
                    *handle,
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
            &format!(
                "voxel cluster — {}×{} field — WASD move, R/F up/down, right-drag look",
                FIELD_DIM, FIELD_DIM
            ),
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
                "clusters: {}   extent: {}³ voxels each",
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
