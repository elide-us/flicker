//! voxel-cluster: contour a 3×3 field of clusters materialized from
//! the Navier-Stokes-inspired heightmap function, and fly through
//! them with MMO-style first-person controls. Three clickable HUD
//! checkboxes toggle a mesh-edge wireframe overlay, a centroid
//! wireframe (a line from each surface-boundary voxel's center to its
//! owned `+++` corner — the diagnostic for verifying that join
//! positions agree across cluster seams), and the choice of contour
//! algorithm: surface-first ([`contour_surface`]) by default, or
//! legacy cell-centroid dual contouring ([`contour_cluster`]) for
//! direct A/B comparison on the same scene.
//!
//! # Why a 3×3 field
//!
//! It's the smallest grid where the visual "is the seam correct?"
//! question makes sense from every direction at once: every interior
//! cluster has neighbors on all four XZ sides. Cross the seam, fly
//! over it, look down at it — the surface should remain continuous,
//! with no visible offset or jitter between adjacent clusters'
//! meshes. The architectural payoff of the rollback is that this
//! continuity comes from **upstream**: every cluster's generator
//! samples the same global heightmap function at its world
//! coordinates, so adjacent clusters' boundary columns sample the
//! heightmap at world coordinates one voxel apart and the heightmap
//! is Lipschitz-continuous, so the joins meet without coordination.
//! The contour pass never reads neighbor voxels.
//!
//! Visual content:
//!   * 3×3 = 9 clusters at LOD 0, each contoured independently. Total
//!     ground footprint 768×768 voxels.
//!   * The water surface is depth-banded: navy in the troughs, paling
//!     through blues to near-white at the crests. Each voxel carries
//!     a primary/secondary material index pair plus an 8-bit blend
//!     factor, and the shader interpolates between them — smooth
//!     gradients across the height band without per-pixel mixing.
//!   * About one in three clusters has a cumulonimbus formation
//!     drifting at altitude 200–240 (flat anvil base, dark underbelly,
//!     bright sunlit crown). About one in four has a thin cirrus wisp
//!     at altitude 250+.
//!   * The mesh-edge wireframe toggle adds a green per-triangle
//!     overlay.
//!   * The centroid wireframe (default OFF) adds a neon-green line
//!     from each surface-boundary voxel's center to its owned join.
//!     Adjacent clusters' lines should meet continuously at seams.
//!   * A white wireframe cube outlines every cluster's extent so the
//!     3×3 layout is visible.
//!   * Toggling the contour-algorithm checkbox re-contours every
//!     cluster live; voxel data is generated once at startup and
//!     reused across toggles (a few-second pause during the rebuild
//!     is expected).
//!
//! Controls (rebindable via `Bindings`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * Mouse: free cursor. Click the HUD checkboxes to toggle the
//!     mesh-edge wireframe, the centroid wireframe, or the contour
//!     algorithm (surface-first vs cell-centroid).
//!   * Escape: quit.

use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, App, Bindings, ControlConfig, InputState};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, Renderer, TextureHandle, Vec2, Vec3,
};
use flicker_voxel::{
    contour_cluster, contour_surface, generators, heightmap, surface_boundary_segments, ClusterId,
    ClusterMap, CLUSTER_DIM,
};

/// Edge length of the multi-cluster field, in clusters.
const FIELD_SIZE: u16 = 3;

/// Eye height above local ground when the camera spawns, in voxels.
/// Roughly the player's eye level — ~5'6" at 6-inch voxels.
const SPAWN_EYE_HEIGHT: f32 = 11.0;

/// Color for the centroid wireframe (lines from voxel centers to
/// their `+++` corner joins). Neon green for high contrast against
/// both the magenta/black checker and the green per-triangle
/// wireframe — they remain distinguishable when both are on.
const CENTROID_LINE_COLOR: [f32; 4] = [0.0, 1.0, 0.4, 1.0];

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

/// Mesh-edge wireframe-toggle checkbox position.
fn wireframe_checkbox_rect() -> Rect {
    Rect {
        top_left: Vec2::new(16.0, 168.0),
        size: Vec2::new(18.0, 18.0),
    }
}

/// Centroid wireframe-toggle checkbox position (below the mesh-edge
/// checkbox, with a one-row gap matching the HUD's existing rhythm).
fn centroid_checkbox_rect() -> Rect {
    Rect {
        top_left: Vec2::new(16.0, 196.0),
        size: Vec2::new(18.0, 18.0),
    }
}

/// Contour-algorithm-toggle checkbox position. Sits below the centroid
/// wireframe checkbox. ON = `contour_surface` (surface-first), OFF =
/// `contour_cluster` (legacy cell-centroid dual contouring).
fn contour_algo_checkbox_rect() -> Rect {
    Rect {
        top_left: Vec2::new(16.0, 224.0),
        size: Vec2::new(18.0, 18.0),
    }
}

/// Render data for one cluster: its id (for the world transform), its
/// uploaded mesh handle, and the pre-translated centroid wireframe
/// segments.
///
/// The centroid segments are computed once at startup and cached. They
/// total tens of thousands of lines per cluster; recomputing them per
/// frame would be wasteful, and they never change in this step (no
/// edits, no LOD swaps).
struct ClusterRender {
    id: ClusterId,
    mesh: MeshHandle,
    centroid_segments: Vec<(Vec3, Vec3)>,
}

struct VoxelCluster {
    clusters: Vec<ClusterRender>,
    /// A 1×1 white pixel uploaded once at `init`. The sprite shader
    /// multiplies it by a tint, so this serves as the "solid colored
    /// quad" for HUD widgets like the checkboxes.
    white: Option<TextureHandle>,
    /// Heightmap seed in use this run — surfaced in the HUD so the
    /// user can reproduce a layout via `FLICKER_SEED`.
    seed: u64,
    /// Sum of every cluster's mesh stats. Updated once after the
    /// startup generation.
    total_vertex_count: usize,
    total_triangle_count: usize,
    /// Sum of every cluster's centroid segment count. Updated once
    /// after the startup generation.
    total_segment_count: usize,

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
    show_centroid_wireframe: bool,
    /// Which contour algorithm to use. `true` = `contour_surface` (the
    /// surface-first algorithm); `false` = `contour_cluster` (the
    /// legacy cell-centroid dual contouring). Toggled by the HUD
    /// checkbox; flipping it schedules a re-build via
    /// [`Self::pending_rebuild`].
    use_surface_contour: bool,
    /// Set to `true` when the algorithm-toggle checkbox is clicked.
    /// The actual re-contour + upload runs at the top of the next
    /// `render` call (the only `App` callback that gets a mutable
    /// `&mut Renderer`).
    pending_rebuild: bool,
    /// The generated voxel data, kept around so the algorithm toggle
    /// can re-contour without re-generating clusters. `None` only
    /// before `init` runs.
    cluster_map: Option<ClusterMap>,
    should_quit: bool,
}

impl Default for VoxelCluster {
    fn default() -> Self {
        // Camera and seed get their real values in `init`. The
        // placeholders here just satisfy the Default bound; nothing
        // renders before init runs.
        Self {
            clusters: Vec::new(),
            white: None,
            seed: 0,
            total_vertex_count: 0,
            total_triangle_count: 0,
            total_segment_count: 0,
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            last_look_cursor: None,
            bindings: Bindings::wasd(),
            config: ControlConfig::default(),
            show_wireframe: true,
            show_centroid_wireframe: false,
            use_surface_contour: true,
            pending_rebuild: false,
            cluster_map: None,
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

    /// Contour every cluster in `map` with the currently selected
    /// algorithm, upload the meshes, and replace `self.clusters`. Also
    /// refreshes the cached centroid wireframe segments (they describe
    /// voxel-level data, not the mesh, so they don't depend on the
    /// algorithm choice — but we recompute on each rebuild rather
    /// than carrying duplicate state).
    ///
    /// # Mesh-handle leak (known follow-up)
    ///
    /// `flicker_render::Renderer` exposes `upload_voxel_mesh` but no
    /// matching `free_mesh` / `drop_mesh` — uploaded vertex and index
    /// buffers stay resident. Every algorithm-toggle leaks 9 meshes'
    /// worth of GPU memory. That's a few MB per toggle on this scene,
    /// fine for development; a `Renderer::free_mesh` addition is a
    /// follow-up.
    fn rebuild_meshes(&mut self, renderer: &mut Renderer, map: &ClusterMap) {
        let mut total_v = 0usize;
        let mut total_t = 0usize;
        let mut total_s = 0usize;
        let mut renders = Vec::with_capacity(map.len());
        let build_start = std::time::Instant::now();
        for (id, cluster) in map.iter() {
            let mesh = if self.use_surface_contour {
                contour_surface(cluster)
            } else {
                contour_cluster(cluster)
            };
            let meta = *mesh.metadata();
            let handle = renderer.upload_voxel_mesh(&mesh);

            let offset = id.world_offset();
            let world_translate =
                |p: [f32; 3]| Vec3::new(offset[0] + p[0], offset[1] + p[1], offset[2] + p[2]);
            let segments: Vec<(Vec3, Vec3)> = surface_boundary_segments(cluster)
                .into_iter()
                .map(|(a, b)| (world_translate(a), world_translate(b)))
                .collect();

            total_v += meta.vertex_count;
            total_t += meta.triangle_count;
            total_s += segments.len();

            renders.push(ClusterRender {
                id,
                mesh: handle,
                centroid_segments: segments,
            });
        }
        renders.sort_by_key(|r| r.id.bits());

        self.clusters = renders;
        self.total_vertex_count = total_v;
        self.total_triangle_count = total_t;
        self.total_segment_count = total_s;

        tracing::info!(
            algorithm = if self.use_surface_contour {
                "contour_surface"
            } else {
                "contour_cluster"
            },
            elapsed_ms = build_start.elapsed().as_millis(),
            vertices = self.total_vertex_count,
            triangles = self.total_triangle_count,
            segments = self.total_segment_count,
            "rebuild_meshes complete"
        );
    }
}

impl App for VoxelCluster {
    fn init(&mut self, renderer: &mut Renderer) {
        let seed = heightmap::seed_from_env();
        self.seed = seed;

        tracing::info!(
            seed = format!("0x{seed:016X}"),
            field = format!("{FIELD_SIZE}x{FIELD_SIZE}"),
            "generating multi-cluster Navier-Stokes wave-field …"
        );

        // Build the cluster map: 3×3 in XZ, all at y=0 and LOD 0.
        // Generation is sequential and slow (HashMap-backed storage,
        // ~7M overrides per cluster). The octree storage rewrite will
        // dramatically speed this up later; for this step we accept
        // the wait.
        //
        // Each cluster's surface is depth-banded water; selected
        // clusters also receive a cumulonimbus stamp at altitude 200–
        // 240, and one or two get cirrus wisps at 250+. The seed
        // selects which clusters get clouds via a hash, so the layout
        // is fully reproducible.
        let mut map = ClusterMap::new();
        let total_clusters = (FIELD_SIZE as usize).pow(2);
        let mut generated = 0;
        let gen_start = std::time::Instant::now();
        let seed32 = (seed as u32) ^ ((seed >> 32) as u32);
        for cz in 0..FIELD_SIZE {
            for cx in 0..FIELD_SIZE {
                generated += 1;
                let id = ClusterId::new(0, cx, 0, cz);
                let offset = id.world_offset();
                tracing::info!(
                    id = ?id,
                    "generating cluster {generated}/{total_clusters} at offset {offset:?} …"
                );
                let mut cluster =
                    generators::heightmap_terrain_at_with_depth_materials(seed, offset);
                stamp_atmospheric_features(&mut cluster, seed32, cx, cz);
                map.insert(id, cluster);
            }
        }
        tracing::info!(
            elapsed_ms = gen_start.elapsed().as_millis(),
            "all clusters generated; contouring + uploading meshes …"
        );

        // Stash the map for the algorithm-toggle path, then delegate
        // contour + upload to `rebuild_meshes`. We `take()` the map
        // out of the Option to avoid the simultaneous `&self.map` /
        // `&mut self` borrow conflict, then put it back. The contour
        // pass is single-cluster — no NeighborContext, no neighbor
        // reads. Cross-cluster correctness comes from upstream (every
        // cluster's generator sampled the same heightmap function).
        self.cluster_map = Some(map);
        let owned_map = self.cluster_map.take().expect("just inserted");
        self.rebuild_meshes(renderer, &owned_map);
        self.cluster_map = Some(owned_map);
        tracing::info!(clusters = self.clusters.len(), "ready");

        // Spawn at field center, eye height above local ground.
        // Field XZ extent is `[0, FIELD_SIZE * CLUSTER_DIM]`; center is
        // the midpoint.
        let field_extent = FIELD_SIZE as f32 * CLUSTER_DIM as f32;
        let spawn_x = field_extent * 0.5;
        let spawn_z = field_extent * 0.5;
        let ground = heightmap::world_height(spawn_x, spawn_z);
        self.position = Vec3::new(spawn_x, ground + SPAWN_EYE_HEIGHT, spawn_z);
        self.yaw = 0.0;
        self.pitch = 0.0;

        // 1×1 white pixel — tinted to build solid colored HUD quads.
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

        // UI: checkbox toggles on the just-pressed edge.
        if input.mouse_left_pressed {
            if wireframe_checkbox_rect().contains(input.mouse_position) {
                self.show_wireframe = !self.show_wireframe;
            }
            if centroid_checkbox_rect().contains(input.mouse_position) {
                self.show_centroid_wireframe = !self.show_centroid_wireframe;
            }
            if contour_algo_checkbox_rect().contains(input.mouse_position) {
                self.use_surface_contour = !self.use_surface_contour;
                // Re-contour every cluster with the newly chosen
                // algorithm. The `App::update` callback only gets an
                // immutable `&Renderer` reference, so we cannot upload
                // here — defer the rebuild to the top of the next
                // `render` call, which has `&mut Renderer`.
                self.pending_rebuild = true;
            }
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if self.clusters.is_empty() && !self.pending_rebuild {
            return;
        }

        // Deferred algorithm-toggle rebuild. The toggle was registered
        // in `update`, which only sees an immutable `&Renderer`; the
        // actual contour + upload happens here, where we have `&mut`.
        // Take the map out of the Option so the borrow checker accepts
        // `&mut self` alongside `&map`, then put it back. The rebuild
        // is synchronous and noticeably slow (~10 s on a 3×3 field at
        // these mesh sizes); a single frame stall on toggle is
        // acceptable for an A/B comparison demo.
        if self.pending_rebuild {
            self.pending_rebuild = false;
            if let Some(map) = self.cluster_map.take() {
                self.rebuild_meshes(renderer, &map);
                self.cluster_map = Some(map);
            }
        }

        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
        });

        // Per-cluster geometry: filled mesh + optional mesh-edge
        // wireframe + per-cluster bounding-box outline. Each cluster's
        // mesh is in cluster-local coordinates (0..CLUSTER_DIM on each
        // axis), so we translate by its world offset.
        for c in &self.clusters {
            let offset = c.id.world_offset();
            let model = Mat4::from_translation(Vec3::new(offset[0], offset[1], offset[2]));
            renderer.draw_mesh(c.mesh, model, MeshDrawOptions::default());
            if self.show_wireframe {
                renderer.draw_mesh(
                    c.mesh,
                    model,
                    MeshDrawOptions {
                        wireframe: true,
                        ..MeshDrawOptions::default()
                    },
                );
            }
            // Per-cluster white bounding-box outline so the 3×3 layout
            // is visible at a glance and seam locations are explicit.
            let min = Vec3::new(offset[0], offset[1], offset[2]);
            let max = min + Vec3::splat(CLUSTER_DIM as f32);
            renderer.draw_bounding_box(min, max, [1.0, 1.0, 1.0, 1.0]);
        }

        // Centroid wireframe — neon-green lines from voxel centers to
        // their owned `+++` corners across every surface-boundary
        // voxel. The lines were pre-translated to world space at
        // startup so this is a single draw call per cluster.
        if self.show_centroid_wireframe {
            for c in &self.clusters {
                renderer.draw_lines(&c.centroid_segments, CENTROID_LINE_COLOR);
            }
        }

        // HUD text.
        renderer.draw_text(
            "voxel cluster — depth-banded water + cumulonimbus + cirrus — WASD move, R/F up/down, right-drag look",
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
                "clusters: {}   verts: {}   tris: {}   algo: {}   seed: 0x{:016X}",
                self.clusters.len(),
                self.total_vertex_count,
                self.total_triangle_count,
                if self.use_surface_contour {
                    "contour_surface"
                } else {
                    "contour_cluster"
                },
                self.seed,
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

        // ---- Checkboxes ----
        if let Some(white) = self.white {
            // Mesh-edge wireframe checkbox.
            draw_checkbox(
                renderer,
                white,
                wireframe_checkbox_rect(),
                self.show_wireframe,
            );
            // Centroid wireframe checkbox.
            draw_checkbox(
                renderer,
                white,
                centroid_checkbox_rect(),
                self.show_centroid_wireframe,
            );
            // Contour-algorithm A/B checkbox.
            draw_checkbox(
                renderer,
                white,
                contour_algo_checkbox_rect(),
                self.use_surface_contour,
            );
        }
        renderer.draw_text(
            "wireframe overlay (click to toggle)",
            Vec2::new(40.0, 170.0),
            14.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "centroid wireframe — {} segments (click to toggle)",
                self.total_segment_count
            ),
            Vec2::new(40.0, 198.0),
            14.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            &format!(
                "surface contour [{}] (off = legacy cell-centroid)",
                if self.use_surface_contour {
                    "ON"
                } else {
                    "OFF"
                },
            ),
            Vec2::new(40.0, 226.0),
            14.0,
            [0.75, 0.85, 0.95, 1.0],
        );
    }
}

/// Render a checkbox at `rect`. Filled neon green when `checked`,
/// dark grey when not. The outer white border is the parent box, the
/// inner 2-pixel-inset fill is the state indicator.
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

/// Stamp deterministic cumulonimbus and cirrus formations into
/// `cluster` based on a hash of `(seed, cx, cz)`. About one in three
/// clusters gets a cumulonimbus at altitude 200–240; about one in
/// four gets a cirrus wisp at altitude 250+. The choice is
/// reproducible — same seed and grid coordinates always yields the
/// same set of clouds.
fn stamp_atmospheric_features(cluster: &mut flicker_voxel::Cluster, seed32: u32, cx: u16, cz: u16) {
    let cumulonimbus_key = mix_u32(seed32, cx as u32, cz as u32, 0xC10D_C10D);
    let cirrus_key = mix_u32(seed32, cx as u32, cz as u32, 0xC1FF_C1FF);

    // ~1/3 of clusters get a cumulonimbus.
    if cumulonimbus_key.is_multiple_of(3) {
        let cloud_seed = cumulonimbus_key;
        // Interior placement, well away from the cluster boundary so
        // the cauliflower spread does not clip out of frame on either
        // side.
        let margin: u32 = 60;
        let inner_cx = margin + (cloud_seed % (CLUSTER_DIM - 2 * margin));
        let inner_cz = margin + ((cloud_seed >> 8) % (CLUSTER_DIM - 2 * margin));
        generators::cumulonimbus_at(
            cluster,
            cloud_seed,
            inner_cx as i32,
            inner_cz as i32,
            /* base_y */ 200,
            /* top_y */ 240,
            /* max_radius */ 25,
        );
    }

    // ~1/4 of clusters get a cirrus wisp.
    if cirrus_key.is_multiple_of(4) {
        let wisp_seed = cirrus_key;
        let start = [
            (wisp_seed % (CLUSTER_DIM - 80)) as i32 + 20,
            252 + ((wisp_seed >> 16) % 4) as i32,
            ((wisp_seed >> 8) % (CLUSTER_DIM - 80)) as i32 + 20,
        ];
        // Long-axis offset roughly along (+X, slight +Z), length ~140.
        let dx = 140 + ((wisp_seed >> 4) % 30) as i32;
        let dz = 30 + ((wisp_seed >> 12) % 50) as i32;
        let end = [
            (start[0] + dx).min(CLUSTER_DIM as i32 - 5),
            start[1] + ((wisp_seed >> 20) % 4) as i32 - 1,
            (start[2] + dz).min(CLUSTER_DIM as i32 - 5),
        ];
        generators::cirrus_wisp_at(cluster, wisp_seed, start, end, /* thickness */ 2);
    }
}

/// Small deterministic mixer for the scene's cluster-level decisions.
/// Not cryptographic — just enough to scatter the cumulonimbus and
/// cirrus picks across different seed/grid combinations.
fn mix_u32(a: u32, b: u32, c: u32, d: u32) -> u32 {
    let mut h = a
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(b.wrapping_mul(0x85EB_CA77))
        .wrapping_add(c.wrapping_mul(0xC2B2_AE3D))
        .wrapping_add(d.wrapping_mul(0x27D4_EB2F));
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    h
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
