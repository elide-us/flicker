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
//! Camera controls (rebindable via `Bindings`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * Escape: open the pause menu (Resume / Quit).
//!
//! Debug toggles are driven by a scripted HUD (`scripts/hud.lua`,
//! loaded at startup via `flicker-script`) — six clickable
//! checkboxes the Lua side owns, replacing the old `1`/`2`/`\` key
//! handling:
//!   * Wireframe overlay on top of the solid mesh.
//!   * Corner-vector arrows — for every stored voxel whose
//!     `CornerVector` differs from the default, draw a line from the
//!     voxel's grid coord to the decoded corner tip. Visualizes where
//!     the contour's QEF placed each active cell's dual vertex.
//!   * Navmesh wireframe — the LOD2 walkable surface drawn magenta as
//!     floor-to-floor links between walkable-adjacent columns.
//!   * Surface walk — switch to surface-walk locomotion: WASD walks in the
//!     XZ plane under gravity with a ground-clamp against the nav surface
//!     (consumed by `walk_step`/`ground_height_at`). Fly mode (the default)
//!     is free 6-DOF and generates no nav.
//!   * Camera-driven LOD — each cluster's LOD follows its distance from
//!     the camera (smoothed to the mesher's ±1 adjacency invariant),
//!     re-meshing on a swap.
//!   * LOD billboards — a digit per cluster, on the navmesh surface at
//!     the cluster centre, showing that cluster's current LOD.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use flicker::app::{run, Action, Bindings, ControlConfig, InputState, Key};
use flicker::render::{
    Camera, Mat4, MeshDrawOptions, MeshHandle, MeshIndices, MeshVertex, Renderer, TextureHandle,
    Vec2, Vec3,
};
use flicker::scene::{Scene, SceneManager, Transition};
use flicker::script::{HudCommand, ScriptHost};
use flicker_voxel::{
    cluster_center_world, contour, in_nav_rings, BakedCluster, Cluster, ClusterId, ClusterMap,
    ClusterNav, CornerVector, FaceDir, LocalCoord, Lod, Material, NeighborContext,
    Scene as WorldScene, CLUSTER_DIM, NAV_DIM,
};
use flicker_worker::WorkerPool;

mod display;
mod ui;

/// Side length of the cluster field, in clusters. A 3×3 row in XZ
/// gives one fully-interior cluster (all four lateral neighbors
/// present), which is what actually exercises seam tangent stitching
/// on every face simultaneously.
const FIELD_DIM: u16 = 3;

/// The game's run phase. `Booting` covers world generation (physics off, the
/// 3D clipmap not drawn — just the loading widget); `Active` is live play.
#[derive(Copy, Clone, PartialEq, Eq)]
enum GamePhase {
    Booting,
    Active,
}

struct GameScene {
    /// LOD-0 source-of-truth cluster data (see `docs/architecture.md`):
    /// QEF corners + dense state for each cluster at full resolution.
    /// Populated once (bake or contour) and never re-derived at runtime;
    /// edits (a later slice) will mutate this. Keyed by LOD-0 `ClusterId`s.
    ///
    /// Wrapped in `Arc<RwLock<…>>` ahead of the mesh-worker slice: workers
    /// will hold a clone and *read* it to derive meshes off-thread, while
    /// edits take the write lock. Today only `rebuild` touches it.
    source: Arc<RwLock<ClusterMap>>,

    /// Background worker pool: per-cluster derive+mesh jobs run here off the
    /// main thread (see `build_cluster`). `None` until `init` creates it.
    pool: Option<WorkerPool>,
    /// Result channel for completed [`ClusterBuild`]s. Jobs hold a clone of
    /// the sender; `render` drains the receiver and applies fresh results.
    build_tx: Option<Sender<ClusterBuild>>,
    build_rx: Option<Receiver<ClusterBuild>>,
    /// Monotonic field generation, bumped on every LOD change. Jobs carry it
    /// so stale (superseded) results are dropped on arrival — best-effort.
    generation: u64,
    /// Current-generation results collected so far; applied as a set once the
    /// whole field has reported in (see `drain_and_apply`).
    pending: Vec<ClusterBuild>,

    /// A 1×1 white pixel uploaded once at `init`. The sprite shader
    /// multiplies it by a tint, so this is the "solid colored quad"
    /// primitive used to draw the scripted HUD's checkbox rectangles.
    white: Option<TextureHandle>,

    /// One mesh handle per cluster, paired with the cluster's id so
    /// `render` can draw each at its world offset.
    meshes: Vec<(ClusterId, MeshHandle)>,

    /// LOD2 walkable surface per cluster, derived on the load path
    /// alongside the mesh and gated to rings 0–2 (§4.6). Consumed by
    /// `walk_step`/`ground_height_at` in surface-walk mode to ground-clamp
    /// the camera; empty in fly mode. Ready to drop onto the ring
    /// scheduler's worker queue unchanged when that lands.
    navs: Vec<(ClusterId, ClusterNav)>,

    /// First-person camera state.
    position: Vec3,
    yaw: f32,
    pitch: f32,
    /// Last cursor position while right-dragging, so we can compute a
    /// per-frame delta. `None` when right is not held.
    last_look_cursor: Option<Vec2>,

    bindings: Bindings,
    config: ControlConfig,

    /// The scripted HUD. Owns the three debug-toggle checkboxes; the
    /// fields below are refreshed from it each frame. `None` only if
    /// the script failed to load (the example still runs without it).
    script: Option<ScriptHost>,

    /// Wireframe-overlay second pass on top of the solid mesh. Mirrors
    /// the script's `"wireframe"` checkbox, refreshed each `update`.
    wireframe_on: bool,
    /// Draw corner-vector arrows (precomputed in `init`). Mirrors the
    /// script's `"corner_arrows"` checkbox.
    corner_arrows_on: bool,
    /// Draw the LOD2 navmesh as a magenta wireframe. Mirrors the
    /// script's `"navmesh"` checkbox, refreshed each `update`.
    navmesh_on: bool,
    /// Cached line segments: from each stored voxel's world grid coord
    /// to its decoded `CornerVector` tip, across all clusters in the
    /// field.
    corner_arrows: Vec<(Vec3, Vec3)>,
    /// Cached navmesh wireframe: one world-space segment per pair of
    /// walkable-linked adjacent columns, across all clusters. Rebuilt in
    /// [`Self::rebuild`] alongside the per-cluster navs (`floor_at` +
    /// `linked`); drawn each frame when `navmesh_on`.
    navmesh_segments: Vec<(Vec3, Vec3)>,

    /// Mirrors the script's `"camera_lod"` checkbox: when on, camera
    /// distance drives each cluster's LOD (see `target_lod_for_cluster`).
    /// When off, every cluster renders at LOD 0.
    camera_lod_on: bool,
    /// Mirrors the script's `"lod_billboards"` checkbox: draw a per-cluster
    /// LOD-digit billboard on the navmesh surface at the cluster centre.
    lod_billboards_on: bool,
    /// Locomotion mode, mirroring the script's `"surface_walk"` checkbox.
    /// `false` = fly mode: free 6-DOF, no nav generated. `true` =
    /// surface-walk mode, which generates the LOD2 nav surface around the
    /// player and walks on it (`walk_step`): WASD in the XZ plane with
    /// gravity + a ground-clamp. See `docs/architecture.md` "Mesh &
    /// navigation generation".
    locomotion_walk: bool,
    /// Currently-applied per-cluster LOD for the 3×3 field, already smoothed
    /// to the mesher's ±1 cross-LOD adjacency invariant. `rebuild` meshes to
    /// this and the billboards display it; recomputed each `update`.
    lod_field: [[u8; FIELD_DIM as usize]; FIELD_DIM as usize],
    /// Digit-glyph atlas (digits 0–7) for the LOD billboards, uploaded once
    /// in `init`.
    digit_atlas: Option<TextureHandle>,

    /// CPU-side world-space triangle data for ray-casting against the
    /// rendered mesh. One entry per cluster: `(id, world vertex
    /// positions, u32 indices)`. Populated alongside [`Self::meshes`]
    /// in [`Self::rebuild`] — the uploaded `MeshHandle` alone isn't
    /// enough for picking. Brute-force ray–triangle is fine at 9
    /// clusters / a few hundred thousand triangles.
    pick_meshes: Vec<(ClusterId, Vec<Vec3>, Vec<u32>)>,

    /// Most recent pick: `(owning cluster, dual-cell center in
    /// cluster-local grid coords)`. `None` until the first hit; sticky
    /// thereafter.
    selection: Option<(ClusterId, [i32; 3])>,

    /// Vertical velocity (voxels/s) for surface-walk gravity. Integrated
    /// each frame while `locomotion_walk` is on and zeroed by the
    /// ground-clamp on contact. Unused in fly mode.
    vy: f32,
    /// Whether the walking camera is currently resting on the surface (vs
    /// airborne/falling). Drives the HUD readout; the clamp sets it.
    grounded: bool,
    /// Set on a fly→walk toggle: the camera snaps down onto the surface
    /// beneath it on the first frame the nav under it is available (nav is
    /// generated asynchronously, so the snap waits for it).
    walk_needs_snap: bool,

    /// Previous-frame Escape key level, for press-edge detection (only the
    /// mouse exposes a ready-made edge flag). A press pushes the pause overlay.
    escape_prev: bool,
    /// Gothic UI theme: drawn as the loading widget while `Booting`, and handed
    /// to each `PauseScene` we push (so pausing never re-uploads). `None` until
    /// `enter`.
    ui_theme: Option<ui::Theme>,

    /// Boot gate: `Booting` (world cooking — physics off, clipmap not drawn,
    /// loading widget up) until the nav range is meshed + nav-ready, then
    /// `Active` (live play).
    phase: GamePhase,
    /// Number of clusters in the local nav range; the boot gate waits for this
    /// many nav surfaces (plus a fully meshed field) before going `Active`.
    nav_ready_target: usize,
}

impl Default for GameScene {
    fn default() -> Self {
        // Camera gets its real pose in `init`. The placeholders here
        // just satisfy the Default bound; nothing renders before init.
        Self {
            source: Arc::new(RwLock::new(ClusterMap::new())),
            pool: None,
            build_tx: None,
            build_rx: None,
            generation: 0,
            pending: Vec::new(),
            white: None,
            meshes: Vec::new(),
            navs: Vec::new(),
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            last_look_cursor: None,
            bindings: Bindings::wasd(),
            config: ControlConfig::default(),
            script: None,
            wireframe_on: false,
            corner_arrows_on: false,
            navmesh_on: false,
            corner_arrows: Vec::new(),
            navmesh_segments: Vec::new(),
            camera_lod_on: false,
            lod_billboards_on: false,
            locomotion_walk: false,
            lod_field: [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize],
            digit_atlas: None,
            pick_meshes: Vec::new(),
            selection: None,
            vy: 0.0,
            grounded: false,
            walk_needs_snap: false,
            escape_prev: false,
            ui_theme: None,
            phase: GamePhase::Booting,
            nav_ready_target: 0,
        }
    }
}

/// Path to the HUD script, resolved against this crate's source dir so the
/// example finds it regardless of the working directory `cargo run` uses.
const HUD_SCRIPT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/hud.lua");

impl GameScene {
    /// Build the game scene, loading the HUD script best-effort (it still runs
    /// without it). Other state takes its placeholder values from [`Default`];
    /// the world + camera come up in [`Scene::enter`].
    fn new() -> Self {
        let script = match ScriptHost::from_file(HUD_SCRIPT_PATH) {
            Ok(s) => {
                tracing::info!("loaded HUD script from {HUD_SCRIPT_PATH}");
                Some(s)
            }
            Err(e) => {
                tracing::error!("HUD script load failed (continuing without it): {e}");
                None
            }
        };
        Self {
            script,
            ..Self::default()
        }
    }

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

/// Field-of-view used by [`GameScene::render`]; mirrored here so the
/// picking ray uses the exact same vertical FOV as the projection.
const PICK_FOV_Y_RADIANS: f32 = 60.0_f32 * std::f32::consts::PI / 180.0;

/// Bounding rect of the scripted HUD's checkbox panel, in HUD pixels
/// (origin top-left). Mirrors `scripts/hud.lua`'s `ORIGIN_X`/`ORIGIN_Y`
/// /`ROW_H`/`BOX` plus the row of label text on the right. Generous on
/// purpose: a click anywhere inside this rect is treated as the HUD's,
/// not a world pick. Keep in sync with the Lua side if the layout
/// moves.
const HUD_PANEL_X0: f32 = 12.0;
const HUD_PANEL_Y0: f32 = 152.0;
const HUD_PANEL_X1: f32 = 280.0;
// Six checkbox rows: the Lua panel's last box bottom is
// `ORIGIN_Y + 5*ROW_H + BOX = 180 + 130 + 18 = 328`; pad to ~10px.
const HUD_PANEL_Y1: f32 = 338.0;

impl GameScene {
    /// `true` when the cursor sits on the scripted HUD's checkbox panel
    /// — the script already consumes the click there (toggling a
    /// checkbox), so we must not also fire a world pick on the same
    /// press edge.
    fn cursor_on_hud(cursor: Vec2) -> bool {
        cursor.x >= HUD_PANEL_X0
            && cursor.x <= HUD_PANEL_X1
            && cursor.y >= HUD_PANEL_Y0
            && cursor.y <= HUD_PANEL_Y1
    }

    /// Origin + direction of the picking ray for screen-space cursor
    /// `cursor` on a viewport of pixel size `viewport`.
    ///
    /// Camera basis built to **match** the renderer's view matrix
    /// (`glam::Mat4::look_at_rh` in [`flicker_render::Camera::view`]):
    ///   * `r = f.cross(Y)` — same as `look_at_rh`'s internal right
    ///     vector (`forward × up`). For the example's yaw-0/face-+Z
    ///     pose this resolves to `(-1, 0, 0)`, which means world `+X`
    ///     lands on the **left** half of the screen — counter-intuitive
    ///     until you note that the camera looks *into* `+Z`. Build
    ///     the picking ray with this same convention so screen-left
    ///     clicks fire rays toward world `+X` and screen-right clicks
    ///     toward world `-X`. (Implementing `Y.cross(f)` instead
    ///     produces the inverse and visibly mirrors the pick.)
    ///   * `u = r.cross(f)` — recomputed up; for a pitched-down camera
    ///     this leans `+Y` toward `+Z` (top of view tilts forward),
    ///     matching head-tilt intuition.
    ///
    /// NDC x runs left→right with the pixel x; NDC y runs bottom→top
    /// because the mouse origin is top-left.
    fn build_pick_ray(&self, cursor: Vec2, viewport: Vec2) -> (Vec3, Vec3) {
        let f = self.forward();
        let r = f.cross(Vec3::Y).normalize_or_zero();
        let u = r.cross(f).normalize_or_zero();
        let aspect = viewport.x / viewport.y;
        let t = (PICK_FOV_Y_RADIANS * 0.5).tan();
        // +0.5 so the ray passes through the pixel's centre, not its
        // top-left corner — matters at low resolutions, harmless at
        // high.
        let ndc_x = 2.0 * (cursor.x + 0.5) / viewport.x - 1.0;
        let ndc_y = 1.0 - 2.0 * (cursor.y + 0.5) / viewport.y;
        let dir = (f + r * (ndc_x * aspect * t) + u * (ndc_y * t)).normalize_or_zero();
        (self.position, dir)
    }

    /// Ray–triangle intersection (Möller–Trumbore). Returns the
    /// parametric `t` along `(origin, dir)` for the front-face hit, or
    /// `None` if the ray misses, hits the back face within numerical
    /// tolerance, or lands behind the origin. The mesh emits oriented
    /// quads, so back-face culling here matches what the renderer
    /// shows.
    fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
        let edge1 = b - a;
        let edge2 = c - a;
        let h = dir.cross(edge2);
        let det = edge1.dot(h);
        // Back-face / parallel-ray rejection. Positive det = ray
        // hits the front face (CCW winding when viewed from
        // `origin`); negative det = back face.
        if det <= 1e-7 {
            return None;
        }
        let inv_det = 1.0 / det;
        let s = origin - a;
        let bu = inv_det * s.dot(h);
        if !(0.0..=1.0).contains(&bu) {
            return None;
        }
        let q = s.cross(edge1);
        let bv = inv_det * dir.dot(q);
        if bv < 0.0 || bu + bv > 1.0 {
            return None;
        }
        let t = inv_det * edge2.dot(q);
        if t > 1e-4 {
            Some(t)
        } else {
            None
        }
    }

    /// Walk every retained triangle and return the nearest front-face
    /// hit as `(ClusterId, world point)`. Brute force over the field's
    /// ~9 cluster meshes: fine for 9 × a few hundred thousand
    /// triangles at click rate. Add spatial acceleration when the
    /// field grows.
    fn try_pick(&self, cursor: Vec2, viewport: Vec2) -> Option<(ClusterId, Vec3)> {
        let (origin, dir) = self.build_pick_ray(cursor, viewport);
        let mut best: Option<(f32, ClusterId)> = None;
        for (id, verts, indices) in &self.pick_meshes {
            for tri in indices.chunks_exact(3) {
                let a = verts[tri[0] as usize];
                let b = verts[tri[1] as usize];
                let c = verts[tri[2] as usize];
                if let Some(t) = Self::ray_triangle(origin, dir, a, b, c) {
                    if best.map_or(true, |(bt, _)| t < bt) {
                        best = Some((t, *id));
                    }
                }
            }
        }
        best.map(|(t, id)| (id, origin + dir * t))
    }

    /// Snap a world-space hit point to the nearest dual-cell center
    /// (lattice grid point) inside the owning cluster: subtract the
    /// cluster's world offset, round each axis, clamp to `[0,
    /// CLUSTER_DIM]`. The range is `[0, 256]` (inclusive both ends) so
    /// hits exactly on a face plane resolve to the boundary grid
    /// point.
    fn hit_to_local_p(id: ClusterId, hit_world: Vec3) -> [i32; 3] {
        let off = id.world_offset();
        let dim = CLUSTER_DIM as i32;
        let snap = |w: f32, o: f32| (w - o).round().clamp(0.0, dim as f32) as i32;
        [
            snap(hit_world.x, off[0]),
            snap(hit_world.y, off[1]),
            snap(hit_world.z, off[2]),
        ]
    }
}

// ---------- virtual-voxel viz layer (Stage 2 of the inspector) ----------
//
// A "virtual voxel" is the dual cell centred on a grid point `p` — a
// cube whose 8 corners are the dual vertices of the 8 primal cells
// meeting at `p`. Each corner is owned by a different voxel: the one
// occupying that octant around `p`. We *read* each owner's stored
// corner to place its octant; if the owner's storage is empty (no
// override, or out of cluster bounds), the display defaults to the
// owner's cell-centre, which puts the corner at `p ± 0.5` — the clean
// unit lattice cube. The "weird shapes" appear at the surface where
// stored corners pull the cube off-lattice.
//
// `Cluster::get` returns truth: stored override or `base`. Display
// defaults belong here, in the viz layer, not on `Cluster`. (A prior
// bug put LOD-stride filtering inside `get`; it returned `base` for
// real data and cost a brutal debug detour. Never again — two
// near-identical accessors where one lies by design is exactly the
// trap.) Hence the free function `display_corner` and explicit
// `VirtualVoxelCorner` struct that records both translations.

/// Returns the owner voxel's stored corner if `m` lies inside the
/// cluster's `[0, CLUSTER_DIM)³` range, else [`CornerVector::DEFAULT`].
/// The in-range path goes through [`Cluster::get`] — the storage's one
/// source of truth — and reads `.corner()`. The OOB path is the
/// *display* default; v1 does not chase cross-cluster owners.
///
/// Deliberately a free function (not a `Cluster` method) — fabricated
/// defaults on display-time queries must never be confused with the
/// truthful in-storage accessor.
fn display_corner(cluster: &Cluster, m: [i32; 3]) -> CornerVector {
    let dim = CLUSTER_DIM as i32;
    if (0..dim).contains(&m[0]) && (0..dim).contains(&m[1]) && (0..dim).contains(&m[2]) {
        let coord = LocalCoord::new(m[0] as u32, m[1] as u32, m[2] as u32).expect("range checked");
        cluster.get(coord).corner()
    } else {
        CornerVector::DEFAULT
    }
}

/// One corner of a virtual voxel, with both coordinate frames kept
/// explicit. `owner_relative` is the same value the owning voxel
/// stores (the provenance/write-back handle, *if* editing ever
/// arrives); `self_relative` is the same world point expressed from
/// the dual cell's centre `p` — what the renderer and the math reason
/// in. Same point, two frames; storing both keeps the bookkeeping
/// visible while debugging.
#[derive(Copy, Clone, Debug)]
struct VirtualVoxelCorner {
    /// Owning voxel's min-corner in cluster-local grid coords:
    /// `m = p + (bx - 1, by - 1, bz - 1)` per the octant mapping.
    owner_local: [i32; 3],
    /// `V.to_components()`: corner offset from `m` (the owner's own
    /// min corner), range `[-0.5, 1.5]`. The owner's default value of
    /// `(0.5, 0.5, 0.5)` is its cell centre.
    owner_relative: [f32; 3],
    /// `(m - p) + V.to_components()`: same world point expressed from
    /// `p`. For default owners this collapses to `(bx - 0.5, by - 0.5,
    /// bz - 0.5)` → corner at `p ± 0.5`, the clean lattice cube.
    self_relative: [f32; 3],
    /// Absolute world-space corner position (`cluster_origin + m +
    /// V`). The renderer consumes this; picking uses it for hit
    /// outlines.
    world: Vec3,
}

/// A virtual voxel: dual cell centred on a grid point `p` inside the
/// given cluster, with its 8 corners' provenance and translations.
#[derive(Clone, Debug)]
struct VirtualVoxel {
    /// Owning cluster (so callers can format the selection consistently
    /// and so a later edit pass can find the storage).
    cluster: ClusterId,
    /// Cluster-local grid coord of the dual cell's centre.
    center_local: [i32; 3],
    /// Corners indexed by `o ∈ 0..8`, bits `(bx, by, bz) = (o & 1,
    /// (o >> 1) & 1, (o >> 2) & 1)`. Bit `1` = the `+` side of `p` on
    /// that axis; bit `0` = the `-` side. (The `---` corner `o = 0`
    /// borrows the `p - (1, 1, 1)` voxel's `+++` reach — same world
    /// point, two frames.)
    corners: [VirtualVoxelCorner; 8],
}

impl VirtualVoxel {
    /// Build the virtual voxel at lattice point `p` in `cluster` whose
    /// world offset is `cluster_origin`. Eight reads from
    /// `display_corner`, one per octant.
    fn build(cluster_id: ClusterId, cluster: &Cluster, p: [i32; 3]) -> Self {
        let off = cluster_id.world_offset();
        let cluster_origin = Vec3::new(off[0], off[1], off[2]);
        let mut corners = [VirtualVoxelCorner {
            owner_local: [0; 3],
            owner_relative: [0.0; 3],
            self_relative: [0.0; 3],
            world: Vec3::ZERO,
        }; 8];
        for o in 0..8 {
            let bx = (o & 1) as i32;
            let by = ((o >> 1) & 1) as i32;
            let bz = ((o >> 2) & 1) as i32;
            let m = [p[0] + bx - 1, p[1] + by - 1, p[2] + bz - 1];
            let v = display_corner(cluster, m).to_components();
            // self-relative = (m - p) + V = (bx - 1, by - 1, bz - 1) + V.
            // For the default V = (0.5, 0.5, 0.5) this becomes
            // (bx - 0.5, by - 0.5, bz - 0.5) → the corner sits at
            // p ± 0.5 (clean axis-aligned unit cube around p).
            let self_relative = [
                (m[0] - p[0]) as f32 + v[0],
                (m[1] - p[1]) as f32 + v[1],
                (m[2] - p[2]) as f32 + v[2],
            ];
            let world = cluster_origin
                + Vec3::new(m[0] as f32 + v[0], m[1] as f32 + v[1], m[2] as f32 + v[2]);
            corners[o] = VirtualVoxelCorner {
                owner_local: m,
                owner_relative: v,
                self_relative,
                world,
            };
        }
        Self {
            cluster: cluster_id,
            center_local: p,
            corners,
        }
    }
}

/// The 12 cube edges as pairs of octant indices `(o0, o1)`, ordered by
/// the axis the edge runs along (X = bit 0, Y = bit 1, Z = bit 2).
/// Every pair differs in exactly one bit: that's the axis-aligned cube
/// topology. The dual cell's mesh is just `corners[o0].world →
/// corners[o1].world` for each entry.
const CUBE_EDGES: [(usize, usize); 12] = [
    // X-axis edges (toggle bit 0).
    (0b000, 0b001),
    (0b010, 0b011),
    (0b100, 0b101),
    (0b110, 0b111),
    // Y-axis edges (toggle bit 1).
    (0b000, 0b010),
    (0b001, 0b011),
    (0b100, 0b110),
    (0b101, 0b111),
    // Z-axis edges (toggle bit 2).
    (0b000, 0b100),
    (0b001, 0b101),
    (0b010, 0b110),
    (0b011, 0b111),
];

impl GameScene {
    /// Build the [`VirtualVoxel`] for the current selection, if any.
    /// Cheap — eight `cluster.get` reads per call — so callers
    /// recompute every frame instead of caching.
    fn current_virtual_voxel(&self) -> Option<VirtualVoxel> {
        let (id, p) = self.selection?;
        // The render map is gone (meshing is async); read the LOD-0 source for
        // the inspector. (Pick is temporary — see the input-controls work.)
        let source = self.source.read().ok()?;
        let cluster = source.get(ClusterId::new(0, id.x(), 0, id.z()))?;
        Some(VirtualVoxel::build(id, cluster, p))
    }
}

impl GameScene {
    /// Rebuild every cluster's contour and mesh from scratch. Called
    /// once at init and again on every `\` toggle. Cheaper than tracking
    /// dirty bits here — the whole 9-cluster rebuild costs a couple
    /// seconds and a `\` press is a deliberate debug action.
    /// Populate the LOD-0 source of truth once — from the on-disk bake if
    /// present, else by contouring the primitive. Never re-derived at runtime
    /// (see `docs/architecture.md`); edits (a later slice) mutate it directly.
    fn ensure_source(&mut self) {
        let mut source = self.source.write().expect("source lock poisoned");
        if !source.is_empty() {
            return;
        }
        let material = Material::new(1, 1, 0).expect("grey material is in-range");
        let bake_dir = bake_dir_path();
        let lod0_ids: Vec<ClusterId> = (0..FIELD_DIM)
            .flat_map(|x| (0..FIELD_DIM).map(move |z| ClusterId::new(0, x, 0, z)))
            .collect();
        if let Some(loaded) = try_load_bake_field(&bake_dir, &lod0_ids) {
            tracing::info!(
                "loaded {} LOD-0 source clusters from bake at {}",
                loaded.len(),
                bake_dir.display()
            );
            for (id, cluster) in loaded {
                source.insert(id, cluster);
            }
            return;
        }
        for id in &lod0_ids {
            let scene = WorldScene::world_at(id.world_offset());
            source.insert(*id, contour(&scene, material, *id));
        }
    }

    /// Re-mesh the whole field on the worker pool: bump the generation and
    /// submit one `build_cluster` job per cell, each holding a clone of the
    /// source `Arc` and the result sender. Returns immediately — completed
    /// builds are applied later by `drain_and_apply`.
    fn submit_field_jobs(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        // Results collected for the previous generation are now stale.
        self.pending.clear();
        let (Some(pool), Some(tx)) = (self.pool.as_ref(), self.build_tx.as_ref()) else {
            return;
        };
        let lod_field = self.lod_field;
        let camera = [self.position.x, self.position.y, self.position.z];
        // Generate nav while Booting (the readiness gate needs it) as well as in
        // surface-walk mode.
        let walk = self.locomotion_walk || matches!(self.phase, GamePhase::Booting);
        for x in 0..FIELD_DIM {
            for z in 0..FIELD_DIM {
                let source = Arc::clone(&self.source);
                let tx = tx.clone();
                pool.submit(move || {
                    let src = source.read().expect("source lock poisoned");
                    let build = build_cluster(&src, x, z, lod_field, camera, walk, generation);
                    let _ = tx.send(build);
                });
            }
        }
    }

    /// Drain completed builds; once the whole field of the current generation
    /// has reported in, apply it as a set — free the old mesh slots, upload
    /// the new geometry into recycled slots, and rebuild the per-frame draw
    /// data (mesh handles, pick triangles, nav, navmesh + corner-arrow
    /// segments). Stale (superseded-generation) results are dropped.
    fn drain_and_apply(&mut self, renderer: &mut Renderer) {
        if let Some(rx) = self.build_rx.as_ref() {
            while let Ok(build) = rx.try_recv() {
                if build.generation == self.generation {
                    self.pending.push(build);
                }
            }
        }
        let field = (FIELD_DIM as usize) * (FIELD_DIM as usize);
        if self.pending.len() < field {
            return;
        }

        let builds = std::mem::take(&mut self.pending);
        // Free the previous field's mesh slots so the renderer recycles them.
        for (_, handle) in self.meshes.drain(..) {
            renderer.free_mesh(handle);
        }
        self.pick_meshes.clear();
        self.navs.clear();
        self.navmesh_segments.clear();
        self.corner_arrows.clear();
        // The selection was anchored to the old triangles; drop it.
        self.selection = None;

        for b in builds {
            let handle = renderer.upload_mesh(&b.vertices, MeshIndices::U32(&b.indices));
            self.meshes.push((b.id, handle));
            self.pick_meshes.push((b.id, b.pick_positions, b.indices));
            if let Some(nav) = b.nav {
                self.navs.push((b.id, nav));
            }
            self.navmesh_segments.extend(b.navmesh_segments);
            self.corner_arrows.extend(b.arrows);
        }
    }
}

/// Voxel-space lift applied to navmesh wireframe segments so the magenta
/// grid hovers just clear of the grey surface mesh instead of z-fighting
/// it. 1 voxel = 6 inches; tune up if it reads as buried.
const NAVMESH_VIZ_LIFT: f32 = 1.0;

/// World-space position of nav column `(x, z)` at floor index `floor` in
/// the cluster whose local origin is `origin`: the LOD2 sample position
/// horizontally (`origin + sample * stride`), the floor height
/// vertically (`origin_y + floor * stride`), lifted clear of the mesh.
///
/// `x`/`z` may be `NAV_DIM` (64) — the sample one step past a `+X`/`+Z`
/// boundary, which lands exactly on the neighbour's near edge in *this*
/// cluster's own frame. That is how a seam segment's far endpoint is
/// placed without ever consulting the neighbour's origin.
fn nav_column_point(origin: Vec3, x: u8, z: u8, floor: u8) -> Vec3 {
    // LOD2 sample stride in voxels: CLUSTER_DIM / NAV_DIM = 256 / 64 = 4.
    let stride = CLUSTER_DIM as f32 / NAV_DIM as f32;
    origin
        + Vec3::new(
            x as f32 * stride,
            floor as f32 * stride + NAVMESH_VIZ_LIFT,
            z as f32 * stride,
        )
}

/// Append this cluster's complete navmesh wireframe to `out`: one
/// segment per pair of walkable-linked 4-adjacent columns, interior
/// **and** across cluster boundaries. Only each column's `+X` and `+Z`
/// neighbour is visited, so every link is emitted exactly once — the
/// same low-side-owns convention the mesher uses for seam quads.
///
/// Boundary links are read through `neighbors`, this cluster's own
/// references to its neighbours' state fields, via
/// [`ClusterNav::linked_across`] / [`ClusterNav::floor_across`] — exactly
/// how the mesher reads neighbour voxels while meshing. The far endpoint
/// is placed at the sample one past the boundary (`NAV_DIM`) in *this*
/// cluster's frame, which lands on the neighbour's near edge. So the seam
/// is generated correctly in this single pass, never stitched together
/// from independently-built per-cluster navs.
fn append_navmesh_segments(
    nav: &ClusterNav,
    id: ClusterId,
    cluster: &Cluster,
    neighbors: &NeighborContext<'_>,
    out: &mut Vec<(Vec3, Vec3)>,
) {
    let off = id.world_offset();
    let origin = Vec3::new(off[0], off[1], off[2]);
    let dim = NAV_DIM as u8; // 64
    let last = dim - 1; // 63
    for x in 0..dim {
        for z in 0..dim {
            let Some(f) = nav.floor_at(x, z) else {
                continue;
            };
            let here = nav_column_point(origin, x, z, f);

            // Interior +X / +Z links. `linked` is true only when both
            // columns have a floor, so the neighbour lookup is infallible.
            if x + 1 < dim && nav.linked((x, z), (x + 1, z)) {
                let fx = nav.floor_at(x + 1, z).expect("linked implies a floor");
                out.push((here, nav_column_point(origin, x + 1, z, fx)));
            }
            if z + 1 < dim && nav.linked((x, z), (x, z + 1)) {
                let fz = nav.floor_at(x, z + 1).expect("linked implies a floor");
                out.push((here, nav_column_point(origin, x, z + 1, fz)));
            }

            // Seam +X / +Z links at the cluster's far edge: read the
            // neighbour column through our references and place its
            // endpoint at the virtual sample `dim` in our own frame.
            if x == last && nav.linked_across(cluster, neighbors, (x, z), FaceDir::PosX) {
                let nf = nav
                    .floor_across(cluster, neighbors, (x, z), FaceDir::PosX)
                    .expect("linked_across implies a neighbour floor");
                out.push((here, nav_column_point(origin, dim, z, nf)));
            }
            if z == last && nav.linked_across(cluster, neighbors, (x, z), FaceDir::PosZ) {
                let nf = nav
                    .floor_across(cluster, neighbors, (x, z), FaceDir::PosZ)
                    .expect("linked_across implies a neighbour floor");
                out.push((here, nav_column_point(origin, x, dim, nf)));
            }
        }
    }
}

// ===== Async per-cluster build (runs on the worker pool) =====

/// Everything `render` needs to display one cluster, produced off the main
/// thread by a worker from the LOD-0 source. Self-contained so a job can
/// build it with only a read lock on the source — no `self`, no renderer.
struct ClusterBuild {
    id: ClusterId,
    /// Generation tag for best-effort application: a result is applied only
    /// if it still matches the field's current generation (else a newer LOD
    /// request superseded it and this one is dropped).
    generation: u64,
    vertices: Vec<MeshVertex>,
    indices: Vec<u32>,
    /// World-space triangle positions for the (temporary) CPU ray-pick.
    pick_positions: Vec<Vec3>,
    /// This cluster's orange corner-vector arrow segments.
    arrows: Vec<(Vec3, Vec3)>,
    /// This cluster's magenta navmesh wireframe segments.
    navmesh_segments: Vec<(Vec3, Vec3)>,
    /// LOD2 walkable surface, present iff the cluster is in the nav rings.
    nav: Option<ClusterNav>,
}

/// Derive cluster `(x, z)` at its render LOD from the LOD-0 `source`, mesh it
/// against its (also-derived) neighbours, and bundle everything `render`
/// needs. **Pure** — no `self`, no renderer, no GPU — so a worker thread runs
/// it with only a read lock on `source`. This is the work that used to run
/// synchronously in `render`; moving it here is what removes the swap hitch.
fn build_cluster(
    source: &ClusterMap,
    x: u16,
    z: u16,
    lod_field: [[u8; FIELD_DIM as usize]; FIELD_DIM as usize],
    camera: [f32; 3],
    walk: bool,
    generation: u64,
) -> ClusterBuild {
    let lod_for = |xx: u16, zz: u16| lod_field[xx as usize][zz as usize];
    let derive = |xx: u16, zz: u16| -> (Cluster, Lod) {
        let lod = Lod::new(lod_for(xx, zz)).expect("valid lod");
        let src = source
            .get(ClusterId::new(0, xx, 0, zz))
            .expect("LOD-0 source populated");
        (flicker_voxel::derive_lod(src, lod), lod)
    };

    let (self_c, self_lod) = derive(x, z);
    let neg_x = if x > 0 { Some(derive(x - 1, z)) } else { None };
    let pos_x = if x + 1 < FIELD_DIM {
        Some(derive(x + 1, z))
    } else {
        None
    };
    let neg_z = if z > 0 { Some(derive(x, z - 1)) } else { None };
    let pos_z = if z + 1 < FIELD_DIM {
        Some(derive(x, z + 1))
    } else {
        None
    };
    let neighbors = NeighborContext {
        neg_x: neg_x.as_ref().map(|(c, l)| (c, *l)),
        pos_x: pos_x.as_ref().map(|(c, l)| (c, *l)),
        neg_z: neg_z.as_ref().map(|(c, l)| (c, *l)),
        pos_z: pos_z.as_ref().map(|(c, l)| (c, *l)),
        ..NeighborContext::none()
    };

    let id = ClusterId::new(self_lod.level(), x, 0, z);
    let off = id.world_offset();
    let origin = Vec3::new(off[0], off[1], off[2]);
    let cm = flicker_voxel::mesh(&self_c, &neighbors, self_lod);

    // Nav (LOD2 walkable surface) for clusters in rings 0–2 — but only in
    // surface-walk mode. In fly mode (the default and only locomotion today)
    // no nav is generated and the engine produces no collisions; nav exists
    // solely for walking/collision, which fly mode does not use. State is
    // LOD-independent (derive copies it verbatim), so it matches the source.
    let mut navmesh_segments = Vec::new();
    let nav = if walk && in_nav_rings(camera, cluster_center_world(id)) {
        let nav = ClusterNav::compute_nav(&self_c, &neighbors);
        append_navmesh_segments(&nav, id, &self_c, &neighbors, &mut navmesh_segments);
        Some(nav)
    } else {
        None
    };

    let vertices: Vec<MeshVertex> = cm
        .vertices
        .iter()
        .map(|v| MeshVertex {
            position: v.position,
            normal: v.normal,
            material: v.material,
        })
        .collect();
    let pick_positions: Vec<Vec3> = cm
        .vertices
        .iter()
        .map(|v| origin + Vec3::new(v.position[0], v.position[1], v.position[2]))
        .collect();

    let mut arrows = Vec::new();
    for (coord, voxel) in self_c.overrides() {
        if voxel.corner() == CornerVector::DEFAULT {
            continue;
        }
        let base = origin + Vec3::new(coord.x() as f32, coord.y() as f32, coord.z() as f32);
        let [dx, dy, dz] = voxel.corner().to_components();
        arrows.push((base, base + Vec3::new(dx, dy, dz)));
    }

    ClusterBuild {
        id,
        generation,
        vertices,
        indices: cm.indices,
        pick_positions,
        arrows,
        navmesh_segments,
        nav,
    }
}

// ===== Camera-driven LOD policy =====

/// Base distance (voxel units) for the per-cluster LOD policy: a cluster
/// `2^n` of these from the camera gets LOD `n` (clamped 0–7). Tuned small
/// (~half a cluster edge) so the 9-cluster field shows visible swaps as the
/// camera moves — the field is too small to exercise a wide LOD range
/// otherwise. A bigger field (more clusters) is the follow-up that exercises
/// the policy properly.
const LOD_BASE_DISTANCE: f32 = 128.0;

/// Per-cluster target LOD from the camera's world-space distance to the
/// cluster centre. Lower = finer. `clamp(floor(log2(dist / base)), 0, 7)`,
/// reusing `flicker_voxel::cluster_center_world` for the centre.
fn target_lod_for_cluster(camera: Vec3, id: ClusterId) -> u8 {
    let c = cluster_center_world(id);
    let distance = (camera - Vec3::new(c[0], c[1], c[2])).length().max(1.0);
    let raw = (distance / LOD_BASE_DISTANCE).log2().floor() as i32;
    // Cap the demo at LOD 7. The 3×3 field never gets far enough to need the
    // LOD-8 single-vector level, and `derive_lod`'s footprint expansion at
    // stride 256 would rewrite the whole cluster for a single cell — wasteful
    // until that expansion is replaced by a snap-on-read in the mesher. Lift
    // to `Lod::MAX.level()` then.
    raw.clamp(0, 7) as u8
}

/// Relax a 3×3 per-cluster LOD field so no 4-adjacent pair differs by more
/// than one level — the mesher's locked cross-LOD adjacency invariant
/// (`flicker_voxel::mesh` panics otherwise). Iterates to a fixed point,
/// raising (coarsening) the finer side of any over-steep pair. Values only
/// increase and are bounded by the field's max, so it always converges.
fn smooth_lod_field(field: &mut [[u8; FIELD_DIM as usize]; FIELD_DIM as usize]) {
    let dim = FIELD_DIM as usize;
    loop {
        let mut changed = false;
        for x in 0..dim {
            for z in 0..dim {
                for (nx, nz) in [(x + 1, z), (x, z + 1)] {
                    if nx >= dim || nz >= dim {
                        continue;
                    }
                    let here = field[x][z];
                    let there = field[nx][nz];
                    if here.abs_diff(there) > 1 {
                        // Raise the finer (lower-LOD) side to one below the
                        // coarser side, closing the gap to exactly 1.
                        if here < there {
                            field[x][z] = there - 1;
                        } else {
                            field[nx][nz] = here - 1;
                        }
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

// ===== LOD-digit billboard atlas (world-space, on the navmesh surface) =====

/// World-space edge length (voxels) of the LOD-digit billboards.
const BILLBOARD_SIZE: f32 = 32.0;

// Tiny 5×7 bitmap font for digits 0–7, baked into a texture atlas.
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const DIGITS: [[u8; GLYPH_H]; 8] = [
    // 0
    [
        0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
    ],
    // 1
    [
        0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
    ],
    // 2
    [
        0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
    ],
    // 3
    [
        0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
    ],
    // 4
    [
        0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
    ],
    // 5
    [
        0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
    ],
    // 6
    [
        0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
    ],
    // 7
    [
        0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
    ],
];

const CELL_W: usize = 32;
const CELL_H: usize = 40;
const ATLAS_W: usize = CELL_W * 8;
const ATLAS_H: usize = CELL_H;

/// Build the RGBA8 digit atlas: 8 cells (digits 0–7) side by side, each a
/// scaled-up white glyph on transparent black.
fn build_digit_atlas() -> Vec<u8> {
    const SCALE: usize = 4;
    let glyph_px_w = GLYPH_W * SCALE;
    let glyph_px_h = GLYPH_H * SCALE;
    let margin_x = (CELL_W - glyph_px_w) / 2;
    let margin_y = (CELL_H - glyph_px_h) / 2;

    let mut pixels = vec![0u8; ATLAS_W * ATLAS_H * 4];
    for (digit_idx, rows) in DIGITS.iter().enumerate() {
        let cell_x0 = digit_idx * CELL_W + margin_x;
        let cell_y0 = margin_y;
        for (row_idx, &row_bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_W {
                let bit = (row_bits >> (GLYPH_W - 1 - col)) & 1;
                if bit == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let px = cell_x0 + col * SCALE + dx;
                        let py = cell_y0 + row_idx * SCALE + dy;
                        let i = (py * ATLAS_W + px) * 4;
                        pixels[i] = 0xff;
                        pixels[i + 1] = 0xff;
                        pixels[i + 2] = 0xff;
                        pixels[i + 3] = 0xff;
                    }
                }
            }
        }
    }
    pixels
}

/// UV sub-rect of the atlas for digit `d` (0–7).
fn digit_uv_rect(d: u8) -> (Vec2, Vec2) {
    let cell_w_uv = 1.0 / 8.0;
    let u0 = d as f32 * cell_w_uv;
    let u1 = u0 + cell_w_uv;
    (Vec2::new(u0, 0.0), Vec2::new(u1, 1.0))
}

// ===== Walk locomotion (surface-walk mode) =====

/// Camera eye height above the walkable surface, in voxels. 12 voxels ×
/// 0.5 ft/voxel = 6 ft, a standing eye line. The ground-clamp parks the
/// camera here above the nav floor beneath it.
const EYE_HEIGHT: f32 = 12.0;

/// Downward acceleration while walking, in voxels/s². 64 voxels × 0.5 ft =
/// 32 ft/s², Earth gravity. Integrated into `vy` each frame; the
/// ground-clamp zeroes it on contact.
const WALK_GRAVITY: f32 = 64.0;

/// Largest surface rise (voxels) a single horizontal step may climb before
/// it is blocked — the nav's walkable-slope gate expressed as a height
/// delta. The nav links LOD2 columns whose floor indices differ by ≤ 2
/// cells, and one cell is `CLUSTER_DIM / NAV_DIM` = 4 voxels, so 2 cells =
/// 8 voxels. A height-delta stand-in for `ClusterNav::linked`, used because
/// the applied navs (`self.navs`) don't retain the `NeighborContext` that
/// `linked_across` would need to consult across a cluster seam. Stepping
/// *down* any distance is allowed (you walk off the ledge and fall).
const MAX_STEP_UP: f32 = 8.0;

/// Ground speed while walking, in voxels/s. 24 voxels × 0.5 ft = 12 ft/s, a
/// brisk jog — deliberately ~5× slower than the fly `move_speed` (120) so
/// the surface reads at a human pace. Tune to taste.
const WALK_SPEED: f32 = 24.0;

impl GameScene {
    /// World-space surface height (camera-frame Y, voxels) of the walkable
    /// nav under world position `(x, z)`, or `None` when no applied nav
    /// covers that column — outside the field, or a column with no floor.
    /// Reads `self.navs` (the LOD2 surfaces applied by `drain_and_apply`),
    /// populated only in surface-walk mode for clusters in the nav rings,
    /// so this returns `None` in fly mode.
    fn ground_height_at(&self, x: f32, z: f32) -> Option<f32> {
        let dim = CLUSTER_DIM as f32;
        let cxf = (x / dim).floor();
        let czf = (z / dim).floor();
        if cxf < 0.0 || czf < 0.0 {
            return None;
        }
        let (cx, cz) = (cxf as u16, czf as u16);
        let (id, nav) = self
            .navs
            .iter()
            .find(|(id, _)| id.x() == cx && id.z() == cz)?;
        // LOD2 cell size in voxels: CLUSTER_DIM / NAV_DIM = 256 / 64 = 4.
        let stride = dim / NAV_DIM as f32;
        let last = (NAV_DIM - 1) as f32;
        let nx = ((x - cxf * dim) / stride).floor().clamp(0.0, last) as u8;
        let nz = ((z - czf * dim) / stride).floor().clamp(0.0, last) as u8;
        let floor = nav.floor_at(nx, nz)?;
        // Cluster y-origin (0 in this field) + floor height in voxels.
        Some(id.world_offset()[1] + floor as f32 * stride)
    }

    /// One frame of surface-walk locomotion: WASD in the XZ plane, gravity
    /// integrated on `vy`, and a ground-clamp that parks the eye
    /// `EYE_HEIGHT` above the nav floor under the camera. A horizontal step
    /// that would climb more than `MAX_STEP_UP` (a wall/cliff) or leave the
    /// meshed nav is blocked; stepping down a ledge is allowed (you fall).
    /// On the first frame after fly→walk it snaps the camera onto the
    /// surface below it, once the (asynchronously generated) nav arrives. If
    /// the camera was parked off the field (it spawns looking in from
    /// outside), the snap recenters it over the field first. While the snap
    /// is pending, gravity and movement are frozen so the camera can't fall
    /// into the void before there's a surface to land on.
    fn walk_step(&mut self, dt_s: f32, input: &InputState) {
        if self.walk_needs_snap {
            if let Some(g) = self.ground_height_at(self.position.x, self.position.z) {
                self.position.y = g + EYE_HEIGHT;
                self.vy = 0.0;
                self.grounded = true;
                self.walk_needs_snap = false;
            } else if !self.navs.is_empty() {
                // Nav has arrived but the camera isn't over it (spawned
                // outside the field). Recenter over the field centre so the
                // next frame's snap lands on the surface.
                let center = FIELD_DIM as f32 * 0.5 * CLUSTER_DIM as f32;
                self.position.x = center;
                self.position.z = center;
            }
            // Either the nav hasn't arrived yet, or we just recentered: skip
            // gravity/movement this frame and try the snap again next frame.
            if self.walk_needs_snap {
                return;
            }
        }

        // Horizontal intent, flattened to the XZ plane (R/F are inert while
        // walking).
        let mut horizontal = Vec3::ZERO;
        if input.action_active(&self.bindings, Action::MoveForward) {
            horizontal += self.move_forward();
        }
        if input.action_active(&self.bindings, Action::MoveBackward) {
            horizontal -= self.move_forward();
        }
        if input.action_active(&self.bindings, Action::StrafeRight) {
            horizontal += self.move_right();
        }
        if input.action_active(&self.bindings, Action::StrafeLeft) {
            horizontal -= self.move_right();
        }
        let step = horizontal.normalize_or_zero() * WALK_SPEED * dt_s;
        let new_x = self.position.x + step.x;
        let new_z = self.position.z + step.z;

        // Slope/edge gate: block a step that would climb steeper than the
        // nav's walkable slope, or walk off the meshed nav.
        let cur_ground = self.ground_height_at(self.position.x, self.position.z);
        let dst_ground = self.ground_height_at(new_x, new_z);
        let allow = match (cur_ground, dst_ground) {
            (Some(c), Some(d)) => d - c <= MAX_STEP_UP,
            (None, _) => true,        // airborne / off-grid: don't constrain XZ
            (Some(_), None) => false, // would step off the nav edge: block
        };
        if allow {
            self.position.x = new_x;
            self.position.z = new_z;
        }

        // Vertical: integrate gravity, then clamp to the surface under the
        // resolved XZ. At/under it → grounded (snap up, zero vy); above it →
        // airborne, keep falling.
        self.vy -= WALK_GRAVITY * dt_s;
        self.position.y += self.vy * dt_s;
        if let Some(g) = self.ground_height_at(self.position.x, self.position.z) {
            let target = g + EYE_HEIGHT;
            if self.position.y <= target {
                self.position.y = target;
                self.vy = 0.0;
                self.grounded = true;
            } else {
                self.grounded = false;
            }
        }
    }
}

impl GameScene {
    /// Fraction (0..=1) of the spawn field meshed so far, for the loading bar.
    /// `pending` fills as workers report; it snaps to 1.0 once the full set is
    /// applied into `meshes`.
    fn boot_progress(&self) -> f32 {
        let field = (FIELD_DIM as usize) * (FIELD_DIM as usize);
        if self.meshes.len() == field {
            1.0
        } else {
            (self.pending.len() as f32 / field as f32).clamp(0.0, 1.0)
        }
    }
}

impl Scene for GameScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // Frame the whole field from outside its -Z face, angled down. Done
        // before generation so the nav-ring gate (and the boot readiness
        // target) use the real camera pose.
        let field_extent = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        let center_x = field_extent * 0.5;
        self.position = Vec3::new(center_x, field_extent * 1.1, -field_extent * 0.5);
        self.yaw = 0.0; // face +Z, into the field.
        self.pitch = -0.55; // look down at it.

        // How many clusters fall in the local nav range — the boot gate waits
        // for all of them to be meshed *and* to have a nav surface (§ user's
        // loading spec) before going Active.
        let cam = [self.position.x, self.position.y, self.position.z];
        self.nav_ready_target = (0..FIELD_DIM)
            .flat_map(|x| (0..FIELD_DIM).map(move |z| (x, z)))
            .filter(|&(x, z)| in_nav_rings(cam, cluster_center_world(ClusterId::new(0, x, 0, z))))
            .count();

        // Stand up the worker pool + result channel, populate the LOD-0 source
        // once, and submit the initial field build (with nav, since we're
        // Booting). Meshes/nav appear over the next frames as jobs complete
        // (drained in `render`); the loading widget shows until the nav range
        // is ready.
        let (tx, rx) = mpsc::channel::<ClusterBuild>();
        self.build_tx = Some(tx);
        self.build_rx = Some(rx);
        self.pool = Some(WorkerPool::with_default_size());
        self.ensure_source();
        self.submit_field_jobs();

        // 1×1 white pixel — tinted to build solid colored HUD quads.
        // Retained sprite-UI capability; no active widgets yet.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));

        // Digit-glyph atlas for the LOD billboards (digits 0–7).
        self.digit_atlas =
            Some(renderer.load_texture(&build_digit_atlas(), ATLAS_W as u32, ATLAS_H as u32));

        // Gothic UI theme — drawn as the loading widget while Booting, and
        // handed to each PauseScene we push (so pausing never re-uploads).
        self.ui_theme = Some(ui::Theme::build(renderer));
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let dt_s = dt.as_secs_f32();

        // Booting: the world is still cooking — ignore input and just poll for
        // readiness. Go Active once the whole field is meshed and every
        // nav-range cluster has a nav surface (then physics + the clipmap turn
        // on); until then `render` shows the loading widget, not the 3D view.
        if matches!(self.phase, GamePhase::Booting) {
            let field = (FIELD_DIM as usize) * (FIELD_DIM as usize);
            if self.meshes.len() == field && self.navs.len() >= self.nav_ready_target {
                self.phase = GamePhase::Active;
            }
            return Transition::None;
        }

        // Escape edge (we track the level ourselves — only the mouse exposes a
        // ready-made press flag) pushes the pause overlay. The scene manager
        // then freezes us, so no gameplay runs until it pops.
        let esc_down = input.key_down(Key::Escape);
        let esc_pressed = esc_down && !self.escape_prev;
        self.escape_prev = esc_down;
        if esc_pressed {
            let theme = self.ui_theme.expect("pause theme built in enter");
            return Transition::Push(Box::new(PauseScene::new(theme)));
        }

        // Debug toggles now live in the HUD script: feed it the mouse +
        // click edge, and mirror the checkbox states it reports back.
        // (`as_ref().map(..)` releases the `self.script` borrow before we
        // write the other fields below.)
        if let Some(result) = self.script.as_ref().map(|s| s.update(input)) {
            match result {
                Ok(toggles) => {
                    self.wireframe_on = toggles.is_on("wireframe");
                    self.corner_arrows_on = toggles.is_on("corner_arrows");
                    self.navmesh_on = toggles.is_on("navmesh");
                    self.camera_lod_on = toggles.is_on("camera_lod");
                    self.lod_billboards_on = toggles.is_on("lod_billboards");

                    // Locomotion mode: surface-walk generates the nav surface;
                    // fly mode generates none (and no collision). A change
                    // re-meshes the field so nav appears/disappears with it.
                    let walk = toggles.is_on("surface_walk");
                    if walk != self.locomotion_walk {
                        self.locomotion_walk = walk;
                        // Entering walk: re-mesh to generate nav, then snap the
                        // camera onto the surface once that nav arrives.
                        if walk {
                            self.walk_needs_snap = true;
                            self.vy = 0.0;
                        }
                        self.submit_field_jobs();
                    }

                    // Desired per-cluster LOD field: the camera-driven
                    // distance policy (smoothed to the mesher's ±1 adjacency
                    // invariant) when enabled, else all clusters at LOD 0. A
                    // change triggers a re-derive + re-mesh of the changed
                    // clusters in `render` — cheap (render-time stride, no
                    // re-contour); the worker pool will move it off-thread.
                    let mut desired = [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize];
                    if self.camera_lod_on {
                        for x in 0..FIELD_DIM {
                            for z in 0..FIELD_DIM {
                                desired[x as usize][z as usize] = target_lod_for_cluster(
                                    self.position,
                                    ClusterId::new(0, x, 0, z),
                                );
                            }
                        }
                        smooth_lod_field(&mut desired);
                    }
                    if desired != self.lod_field {
                        self.lod_field = desired;
                        self.submit_field_jobs();
                    }
                }
                Err(e) => tracing::error!("HUD script update failed: {e}"),
            }
        }

        // Left-click → world pick (inspector). The HUD script consumes
        // the press edge for its own checkbox toggling above, but it
        // does so without telling us — so we re-check the press edge
        // here and gate it on "cursor outside the HUD panel" to avoid
        // double-firing on a checkbox click. Right-drag is for look;
        // left-click is for pick. No conflict.
        if input.mouse_left_pressed && !Self::cursor_on_hud(input.mouse_position) {
            if let Some((id, hit_world)) = self.try_pick(input.mouse_position, renderer.size()) {
                let p = Self::hit_to_local_p(id, hit_world);
                tracing::info!(
                    "pick: cluster ({}, {}, {}, lod {}) → world ({:.2}, {:.2}, {:.2}) → p ({}, {}, {})",
                    id.x(), id.y(), id.z(), id.lod(),
                    hit_world.x, hit_world.y, hit_world.z,
                    p[0], p[1], p[2],
                );
                self.selection = Some((id, p));
            }
        }

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

        // Movement: fly (free 6-DOF) or walk (XZ + gravity/ground-clamp),
        // per the surface-walk locomotion mode.
        if self.locomotion_walk {
            self.walk_step(dt_s, input);
        } else {
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

        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        // Apply any completed mesh builds (uploads happen here — `render` owns
        // `&mut Renderer`). Runs while Booting too, so the world cooks under the
        // loading widget.
        self.drain_and_apply(renderer);

        // Booting: draw the loading widget instead of the 3D clipmap.
        if matches!(self.phase, GamePhase::Booting) {
            if let Some(theme) = self.ui_theme {
                let screen = renderer.size();
                theme.draw_loading(renderer, screen, self.boot_progress());
            }
            return;
        }

        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
        });

        // Draw each cluster's extent as a white wireframe box. The extent is
        // LOD-independent, so iterate the grid positions directly.
        for x in 0..FIELD_DIM {
            for z in 0..FIELD_DIM {
                let offset = ClusterId::new(0, x, 0, z).world_offset();
                let min = Vec3::new(offset[0], offset[1], offset[2]);
                let max = min + Vec3::splat(CLUSTER_DIM as f32);
                renderer.draw_bounding_box(min, max, [1.0, 1.0, 1.0, 1.0]);
            }
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

        // Navmesh wireframe: the LOD2 walkable surface as floor-to-floor
        // links between walkable-adjacent columns. Magenta so it reads
        // against both the grey mesh and the orange corner arrows.
        if self.navmesh_on && !self.navmesh_segments.is_empty() {
            renderer.draw_lines(&self.navmesh_segments, [1.0, 0.0, 1.0, 1.0]);
        }

        // LOD billboards: a digit per cluster, sitting on the navmesh surface
        // at the cluster centre, showing that cluster's current LOD. World-
        // space and depth-tested, so terrain in front occludes them.
        if self.lod_billboards_on {
            if let Some(atlas) = self.digit_atlas {
                let half_col = (NAV_DIM / 2) as u8; // centre column of the 64² nav grid
                let stride = CLUSTER_DIM as f32 / NAV_DIM as f32; // 4 voxels / LOD2 cell
                let half_edge = CLUSTER_DIM as f32 * 0.5; // cluster-centre offset
                for (id, nav) in &self.navs {
                    let off = id.world_offset();
                    // Surface height from the navmesh centre column; fall back
                    // to the cluster's volume centre if that column has no floor.
                    let surface_y = match nav.floor_at(half_col, half_col) {
                        Some(f) => off[1] + f as f32 * stride,
                        None => off[1] + half_edge,
                    };
                    // Lift the centre by half the quad height so the
                    // billboard's bottom edge rests on the surface rather than
                    // its midline (which buries the lower half in the mesh).
                    let pos = Vec3::new(
                        off[0] + half_edge,
                        surface_y + BILLBOARD_SIZE * 0.5,
                        off[2] + half_edge,
                    );
                    let lod = self.lod_field[id.x() as usize][id.z() as usize];
                    let (uv_min, uv_max) = digit_uv_rect(lod);
                    renderer.draw_billboard(
                        atlas,
                        pos,
                        Vec2::splat(BILLBOARD_SIZE),
                        uv_min,
                        uv_max,
                        [1.0, 0.95, 0.4, 1.0],
                    );
                }
            }
        }

        // HUD text.
        let controls = if self.locomotion_walk {
            "walk — WASD on surface, gravity, right-drag look"
        } else {
            "fly — WASD move, R/F up/down, right-drag look"
        };
        renderer.draw_text(
            &format!("voxel cluster — {FIELD_DIM}×{FIELD_DIM} field — {controls}"),
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
                self.meshes.len(),
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
        // Diagnostics the checkboxes don't convey on their own.
        renderer.draw_text(
            &format!(
                "corner arrows stored: {}   nav clusters (rings 0–2): {}",
                self.corner_arrows.len(),
                self.navs.len(),
            ),
            Vec2::new(16.0, 104.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        renderer.draw_text(
            "press Escape to quit",
            Vec2::new(16.0, 124.0),
            16.0,
            [0.75, 0.85, 0.95, 1.0],
        );
        // Current pick — the inspector's selection state. `None` until
        // the first left-click lands on a meshed face.
        let pick_line = match self.selection {
            Some((id, p)) => format!(
                "pick: ({}, {}, {}, lod {}) p = ({}, {}, {})",
                id.x(),
                id.y(),
                id.z(),
                id.lod(),
                p[0],
                p[1],
                p[2]
            ),
            None => "pick: (none — left-click a face)".to_string(),
        };
        renderer.draw_text(
            &pick_line,
            Vec2::new(16.0, 144.0),
            16.0,
            [0.95, 0.85, 0.60, 1.0],
        );
        // Walk readout (surface-walk mode only): grounded/airborne, the nav
        // surface height under the camera, and vertical velocity.
        if self.locomotion_walk {
            let ground = self
                .ground_height_at(self.position.x, self.position.z)
                .map_or_else(|| "—".to_string(), |g| format!("{g:.0}"));
            renderer.draw_text(
                &format!(
                    "walk: {}   ground y: {}   vy: {:+.1}",
                    if self.grounded {
                        "grounded"
                    } else {
                        "airborne"
                    },
                    ground,
                    self.vy,
                ),
                Vec2::new(16.0, 164.0),
                16.0,
                [0.6, 0.95, 0.7, 1.0],
            );
        }

        // Virtual-voxel inspector: 12-edge wireframe of the dual cell
        // at the selected lattice point, plus a per-corner readout of
        // both translation frames (owner-relative `V` as the owner
        // voxel stores it, and self-relative — same world point
        // expressed from `p`). Inspect-only; both frames are kept so
        // the bookkeeping is visible while debugging.
        if let Some(vv) = self.current_virtual_voxel() {
            let mut segments: Vec<(Vec3, Vec3)> = Vec::with_capacity(12);
            for &(o0, o1) in &CUBE_EDGES {
                segments.push((vv.corners[o0].world, vv.corners[o1].world));
            }
            // Darker than the bright-white cluster bounding box so the
            // dual cell reads as a distinct overlay rather than a
            // sub-box of the cluster.
            renderer.draw_lines(&segments, [0.7, 0.7, 0.75, 1.0]);

            // Per-corner readout. Anchored to the right side of the
            // screen so it doesn't fight the left-column diagnostics
            // or the scripted HUD's checkbox panel. The leading
            // (-/+ -/+ -/+) tag is the octant bit pattern decoded
            // back into per-axis sign, matching the brief's table.
            let surface_w = renderer.size().x;
            let panel_w = 620.0;
            let panel_x = (surface_w - panel_w - 16.0).max(16.0);
            renderer.draw_text(
                &format!(
                    "virt voxel  cluster ({}, {}, {}, lod {})  p = ({}, {}, {})",
                    vv.cluster.x(),
                    vv.cluster.y(),
                    vv.cluster.z(),
                    vv.cluster.lod(),
                    vv.center_local[0],
                    vv.center_local[1],
                    vv.center_local[2]
                ),
                Vec2::new(panel_x, 16.0),
                16.0,
                [0.95, 0.85, 0.60, 1.0],
            );
            for o in 0..8 {
                let c = &vv.corners[o];
                let bx = (o & 1) as i32;
                let by = ((o >> 1) & 1) as i32;
                let bz = ((o >> 2) & 1) as i32;
                let tag = |b: i32| if b == 1 { '+' } else { '-' };
                let line = format!(
                    "o={} ({}{}{})  m=({:>3},{:>3},{:>3})  V=({:+.3},{:+.3},{:+.3})  self=({:+.3},{:+.3},{:+.3})",
                    o,
                    tag(bx), tag(by), tag(bz),
                    c.owner_local[0], c.owner_local[1], c.owner_local[2],
                    c.owner_relative[0], c.owner_relative[1], c.owner_relative[2],
                    c.self_relative[0], c.self_relative[1], c.self_relative[2],
                );
                renderer.draw_text(
                    &line,
                    Vec2::new(panel_x, 40.0 + (o as f32) * 18.0),
                    13.0,
                    [0.82, 0.86, 0.92, 1.0],
                );
            }
        }

        // The scripted HUD: the Lua side returns plain draw commands,
        // which we turn into sprite (rect) and text calls. Rects are
        // drawn with the 1×1 white texture tinted by the command color.
        if let (Some(script), Some(white)) = (self.script.as_ref(), self.white) {
            match script.draw() {
                Ok(commands) => {
                    for command in commands {
                        match command {
                            HudCommand::Rect { x, y, w, h, color } => {
                                renderer.draw_sprite(
                                    white,
                                    Vec2::new(x, y),
                                    Vec2::new(w, h),
                                    color,
                                );
                            }
                            HudCommand::Text {
                                x,
                                y,
                                text,
                                size,
                                color,
                            } => {
                                renderer.draw_text(&text, Vec2::new(x, y), size, color);
                            }
                        }
                    }
                }
                Err(e) => tracing::error!("HUD script draw failed: {e}"),
            }
        }
    }
}

// ===== Front-end scenes (logo, menu, pause overlay) =====

/// How long the logo splash shows before auto-advancing to the menu.
const LOGO_DURATION: Duration = Duration::from_millis(2200);

/// Logo splash: a large wordmark over the gothic backdrop. Auto-advances to
/// the menu after [`LOGO_DURATION`], or immediately on click / Space / Escape.
struct LogoScene {
    theme: Option<ui::Theme>,
    elapsed: Duration,
}

impl LogoScene {
    fn new() -> Self {
        Self {
            theme: None,
            elapsed: Duration::ZERO,
        }
    }
}

impl Scene for LogoScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.theme = Some(ui::Theme::build(renderer));
        // Apply the persisted (or default) display setting now the window
        // exists — so a saved fullscreen/resolution choice takes effect at
        // launch.
        display::current().apply(renderer);
    }

    fn update(&mut self, dt: Duration, input: &InputState, _renderer: &Renderer) -> Transition {
        self.elapsed += dt;
        let skip =
            input.mouse_left_pressed || input.key_down(Key::Space) || input.key_down(Key::Escape);
        if self.elapsed >= LOGO_DURATION || skip {
            return Transition::Replace(Box::new(MenuScene::new()));
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(theme) = self.theme else {
            return;
        };
        let screen = renderer.size();
        theme.backdrop(renderer, screen);
        theme.wordmark(renderer, screen, "FLICKER");
    }
}

// ===== settings panel (display mode + resolution dropdowns, top-right) =====

/// Inset (px) of the settings panel from the top-right corner. Top-left is
/// reserved for gameplay bars (health/mana/stamina); top-right is system UI.
const SETTINGS_INSET: f32 = 24.0;

/// Seconds the confirm overlay waits before auto-reverting a display change.
const CONFIRM_SECS: f32 = 15.0;

/// A selection made in the settings dropdowns.
enum DisplayChange {
    Mode(display::DisplayMode),
    Res(display::Resolution),
}

/// Apply `change` to the window immediately and record it as current. Returns
/// `Some(previous)` when the change should be confirmed-or-reverted (any
/// resolution change, or switching to exclusive fullscreen); `None` when it is
/// safe to apply outright (windowed / borderless toggles).
fn apply_display_change(
    change: DisplayChange,
    renderer: &Renderer,
) -> Option<display::DisplaySetting> {
    let prev = display::current();
    let (next, confirm) = match change {
        DisplayChange::Mode(m) => (
            display::DisplaySetting {
                mode: m,
                res: prev.res,
            },
            matches!(m, display::DisplayMode::ExclusiveFullscreen),
        ),
        DisplayChange::Res(r) => (
            display::DisplaySetting {
                mode: prev.mode,
                res: r,
            },
            true,
        ),
    };
    next.apply(renderer);
    display::set_current(next);
    confirm.then_some(prev)
}

/// Two stacked dropdowns (mode + resolution) anchored top-right. Shown on the
/// menu and pause overlay; hidden during active gameplay.
struct SettingsPanel {
    mode_dd: ui::Dropdown,
    res_dd: ui::Dropdown,
    width: f32,
    res_options: Vec<display::Resolution>,
    res_labels: Vec<String>,
    mode_labels: Vec<String>,
    last_cursor: Vec2,
}

impl SettingsPanel {
    fn new(renderer: &mut Renderer) -> Self {
        let monitor = renderer.monitor_size();
        let res_options = display::resolution_options(monitor);
        let res_labels: Vec<String> = res_options
            .iter()
            .map(|&r| display::resolution_label(r, monitor))
            .collect();
        let mode_labels: Vec<String> = display::DisplayMode::ALL
            .iter()
            .map(|m| m.label().to_string())
            .collect();
        // Width = widest label across both dropdowns + room for the text inset
        // and the caret, measured with the real font.
        let widest = |labels: &[String], renderer: &mut Renderer| {
            labels
                .iter()
                .map(|l| renderer.measure_text(l, ui::DD_LABEL_SIZE).x)
                .fold(0.0_f32, f32::max)
        };
        let width = widest(&res_labels, renderer).max(widest(&mode_labels, renderer)) + 46.0;
        Self {
            mode_dd: ui::Dropdown::new(),
            res_dd: ui::Dropdown::new(),
            width,
            res_options,
            res_labels,
            mode_labels,
            last_cursor: Vec2::ZERO,
        }
    }

    /// Top-left anchors of the two stacked dropdowns at the current screen size
    /// (resolution sits below the mode dropdown, accounting for its open rows).
    fn anchors(&self, screen: Vec2) -> (Vec2, Vec2) {
        let x = screen.x - SETTINGS_INSET - self.width;
        let mode_anchor = Vec2::new(x, SETTINGS_INSET);
        let res_y = mode_anchor.y + self.mode_dd.height(self.mode_labels.len()) + 8.0;
        (mode_anchor, Vec2::new(x, res_y))
    }

    /// Process a click; return a requested display change, if any.
    fn update(&mut self, input: &InputState, renderer: &Renderer) -> Option<DisplayChange> {
        self.last_cursor = input.mouse_position;
        if !input.mouse_left_pressed {
            return None;
        }
        let (mode_anchor, res_anchor) = self.anchors(renderer.size());
        let cursor = input.mouse_position;
        if let Some(i) = self
            .mode_dd
            .click(mode_anchor, self.width, self.mode_labels.len(), cursor)
        {
            return Some(DisplayChange::Mode(display::DisplayMode::ALL[i]));
        }
        if let Some(i) = self
            .res_dd
            .click(res_anchor, self.width, self.res_options.len(), cursor)
        {
            return Some(DisplayChange::Res(self.res_options[i]));
        }
        None
    }

    fn draw(&self, theme: &ui::Theme, renderer: &mut Renderer) {
        let (mode_anchor, res_anchor) = self.anchors(renderer.size());
        let current = display::current();
        let mode_sel = display::DisplayMode::ALL
            .iter()
            .position(|&m| m == current.mode)
            .unwrap_or(0);
        let res_sel = self
            .res_options
            .iter()
            .position(|&r| r == current.res)
            .unwrap_or(0);
        self.res_dd.draw(
            theme,
            renderer,
            (res_anchor, self.width),
            &self.res_labels,
            res_sel,
            self.last_cursor,
        );
        self.mode_dd.draw(
            theme,
            renderer,
            (mode_anchor, self.width),
            &self.mode_labels,
            mode_sel,
            self.last_cursor,
        );
    }
}

/// Confirm-or-revert overlay shown after a resolution / exclusive-fullscreen
/// change: the change is already applied, and this waits up to [`CONFIRM_SECS`]
/// for the player to Keep it — auto-reverting to `previous` on Revert or
/// timeout. Pushed as an overlay (same mechanism as the pause menu), so it
/// works over the menu or the pause screen.
struct ConfirmDisplayScene {
    theme: ui::Theme,
    previous: display::DisplaySetting,
    remaining: f32,
    hover: Option<ui::ModalButton>,
}

impl ConfirmDisplayScene {
    fn new(theme: ui::Theme, previous: display::DisplaySetting) -> Self {
        Self {
            theme,
            previous,
            remaining: CONFIRM_SECS,
            hover: None,
        }
    }

    fn revert(&self, renderer: &Renderer) {
        self.previous.apply(renderer);
        display::set_current(self.previous);
    }
}

impl Scene for ConfirmDisplayScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        self.remaining -= dt.as_secs_f32();
        if self.remaining <= 0.0 {
            self.revert(renderer);
            return Transition::Pop;
        }
        let layout = ui::modal_layout(renderer.size());
        self.hover = layout.hover(input.mouse_position);
        if input.mouse_left_pressed {
            match self.hover {
                Some(ui::ModalButton::Top) => return Transition::Pop, // Keep
                Some(ui::ModalButton::Bottom) => {
                    self.revert(renderer);
                    return Transition::Pop;
                }
                None => {}
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let screen = renderer.size();
        let layout = ui::modal_layout(screen);
        // A light 25% dim (not the heavy modal scrim) so the new resolution
        // stays visible behind the dialog, which still blocks all interaction
        // beneath it (it's the top scene; the manager freezes the rest).
        self.theme.dim(renderer, screen, 0.25);
        let note = format!("Reverting in {}s", self.remaining.ceil().max(0.0) as i32);
        self.theme.draw_panel(
            renderer,
            &layout,
            "KEEP DISPLAY?",
            Some(&note),
            ("KEEP", "REVERT"),
            self.hover,
        );
    }
}

/// Main menu: the gothic panel with START / QUIT over an opaque backdrop.
/// START replaces this scene with the game; QUIT exits.
struct MenuScene {
    theme: Option<ui::Theme>,
    hover: Option<ui::ModalButton>,
    settings: Option<SettingsPanel>,
}

impl MenuScene {
    fn new() -> Self {
        Self {
            theme: None,
            hover: None,
            settings: None,
        }
    }
}

impl Scene for MenuScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.theme = Some(ui::Theme::build(renderer));
        self.settings = Some(SettingsPanel::new(renderer));
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        // Settings dropdowns (top-right). A confirmable change pushes the
        // confirm overlay; safe changes apply instantly.
        if let Some(panel) = self.settings.as_mut() {
            if let Some(change) = panel.update(input, renderer) {
                if let Some(prev) = apply_display_change(change, renderer) {
                    let theme = self.theme.expect("theme built in enter");
                    return Transition::Push(Box::new(ConfirmDisplayScene::new(theme, prev)));
                }
                return Transition::None;
            }
        }
        let layout = ui::modal_layout(renderer.size());
        self.hover = layout.hover(input.mouse_position);
        if input.mouse_left_pressed {
            match self.hover {
                Some(ui::ModalButton::Top) => {
                    return Transition::Replace(Box::new(GameScene::new()))
                }
                Some(ui::ModalButton::Bottom) => return Transition::Quit,
                None => {}
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let Some(theme) = self.theme else { return };
        let screen = renderer.size();
        let layout = ui::modal_layout(screen);
        theme.backdrop(renderer, screen);
        theme.draw_panel(
            renderer,
            &layout,
            "FLICKER",
            None,
            ("START", "QUIT"),
            self.hover,
        );
        if let Some(panel) = self.settings.as_ref() {
            panel.draw(&theme, renderer);
        }
    }
}

/// Pause overlay pushed over the frozen game. Resume (or Escape) pops back to
/// the game; Quit exits. Reuses the game's already-uploaded [`ui::Theme`].
struct PauseScene {
    theme: ui::Theme,
    hover: Option<ui::ModalButton>,
    escape_prev: bool,
    settings: Option<SettingsPanel>,
}

impl PauseScene {
    fn new(theme: ui::Theme) -> Self {
        // Escape is held at the instant the game pushes us; start `escape_prev`
        // true so the opening press doesn't immediately pop us back.
        Self {
            theme,
            hover: None,
            escape_prev: true,
            settings: None,
        }
    }
}

impl Scene for PauseScene {
    fn is_overlay(&self) -> bool {
        true
    }

    fn enter(&mut self, renderer: &mut Renderer) {
        self.settings = Some(SettingsPanel::new(renderer));
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
        let esc_down = input.key_down(Key::Escape);
        let esc_pressed = esc_down && !self.escape_prev;
        self.escape_prev = esc_down;
        if esc_pressed {
            return Transition::Pop; // resume
        }
        // Settings dropdowns (top-right) — resolution is allowed on the pause
        // screen. A confirmable change pushes the confirm overlay.
        if let Some(panel) = self.settings.as_mut() {
            if let Some(change) = panel.update(input, renderer) {
                if let Some(prev) = apply_display_change(change, renderer) {
                    return Transition::Push(Box::new(ConfirmDisplayScene::new(self.theme, prev)));
                }
                return Transition::None;
            }
        }
        let layout = ui::modal_layout(renderer.size());
        self.hover = layout.hover(input.mouse_position);
        if input.mouse_left_pressed {
            match self.hover {
                Some(ui::ModalButton::Top) => return Transition::Pop, // resume
                Some(ui::ModalButton::Bottom) => return Transition::Quit,
                None => {}
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        let screen = renderer.size();
        let layout = ui::modal_layout(screen);
        self.theme.scrim(renderer, screen);
        self.theme.draw_panel(
            renderer,
            &layout,
            "PAUSED",
            None,
            ("RESUME", "QUIT"),
            self.hover,
        );
        if let Some(panel) = self.settings.as_ref() {
            panel.draw(&self.theme, renderer);
        }
    }
}

/// Directory the example reads bake files from on startup and writes
/// bake files to in `--bake` mode. Resolved against this crate's
/// source dir so `cargo run` finds it from any working directory.
fn bake_dir_path() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/bake"))
}

/// Filename for a freshly-written cluster bake. The on-disk file
/// stores LOD-0 data only (LOD is a render concern) and is gzip-
/// compressed compact JSON, so the canonical extension is
/// `.json.gz`. The cluster's spatial address is the rest of the name
/// — `lod` is omitted because it's always `0` here.
fn bake_filename(x: u16, y: u16, z: u16) -> String {
    format!("cluster_{x}_{y}_{z}.json.gz")
}

/// Legacy filename for uncompressed cluster bakes — what `--bake`
/// wrote before the gzip wiring landed. Used as a fallback by the
/// loader so existing hand-decompressed or pre-gzip bakes still
/// load without a forced re-bake.
fn bake_filename_legacy(x: u16, y: u16, z: u16) -> String {
    format!("cluster_{x}_{y}_{z}.json")
}

/// Try to load every cluster in `ids` from `dir`. Returns `Some(vec)`
/// only if **all** loads succeed; partial loads fall back to
/// contour-from-primitive (no point starting up with a half-baked
/// field). Reads are best-effort: any error path logs at warn level
/// and yields `None`. The compressed name (`.json.gz`) is tried
/// first; if it isn't present, the legacy uncompressed name
/// (`.json`) is tried as a fallback.
fn try_load_bake_field(
    dir: &std::path::Path,
    ids: &[ClusterId],
) -> Option<Vec<(ClusterId, Cluster)>> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let compressed_path = dir.join(bake_filename(id.x(), id.y(), id.z()));
        let legacy_path = dir.join(bake_filename_legacy(id.x(), id.y(), id.z()));
        let (path, bytes) = if let Ok(bytes) = std::fs::read(&compressed_path) {
            (compressed_path, bytes)
        } else if let Ok(bytes) = std::fs::read(&legacy_path) {
            (legacy_path, bytes)
        } else {
            return None; // neither file present → fall back to contour
        };
        let baked = match BakedCluster::from_bytes(&bytes) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("bake load failed for {}: {e}", path.display());
                return None;
            }
        };
        // The bake's on-disk id should match what we asked for. If it
        // doesn't, the file was hand-edited or copied incorrectly —
        // refuse to trust it.
        if baked.id.bits() != id.bits() {
            tracing::warn!(
                "bake at {} carries id {:?}, expected {:?}; skipping bake load",
                path.display(),
                baked.id,
                id,
            );
            return None;
        }
        out.push((baked.id, baked.cluster));
    }
    Some(out)
}

/// Bake every cluster in the 3×3 field to disk at LOD 0, then exit.
/// Triggered by `--bake` on the command line; the demo's renderer
/// never spins up in this mode. Files are written
/// `bake/cluster_{x}_{y}_{z}.json.gz` (compact JSON, gzip-
/// compressed). To inspect a file, `gunzip -c cluster_*.json.gz |
/// jq .` — round-trips through `BakedCluster::from_bytes` either
/// way.
fn run_bake_mode() -> Result<()> {
    let dir = bake_dir_path();
    std::fs::create_dir_all(&dir)?;
    let material = Material::new(1, 1, 0).expect("grey material is in-range");
    let mut written = 0_usize;
    let mut total_bytes = 0_u64;
    for x in 0..FIELD_DIM {
        for z in 0..FIELD_DIM {
            let id = ClusterId::new(0, x, 0, z);
            let scene = WorldScene::world_at(id.world_offset());
            let cluster = contour(&scene, material, id);
            let baked = BakedCluster::from_cluster(id, cluster);
            // Compact JSON, gzipped — the dense state field's 4 MB of
            // packed bytes and the long runs of identical material
            // bytes both compress 5–10× under default gzip. A
            // typical 3×3 demo cluster lands around 10 MB on disk.
            let bytes = baked
                .to_disk_bytes()
                .map_err(|e| anyhow::anyhow!("serialize cluster ({x}, 0, {z}): {e}"))?;
            let path = dir.join(bake_filename(id.x(), id.y(), id.z()));
            std::fs::write(&path, &bytes)?;
            tracing::info!("wrote {} ({} bytes)", path.display(), bytes.len());
            written += 1;
            total_bytes += bytes.len() as u64;
        }
    }
    tracing::info!(
        "baked {written} clusters ({} MB total) to {}",
        total_bytes / (1024 * 1024),
        dir.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "voxel_cluster=info,flicker_app=info,flicker_render=warn".into()
            }),
        )
        .init();

    // Hand-parse argv — at most one flag, no need for a CLI crate.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--bake" => return run_bake_mode(),
            "--help" | "-h" => {
                println!("voxel-cluster — flicker voxel demo");
                println!("Usage:");
                println!("  voxel-cluster           run the demo (loads bake/ if present, else contours)");
                println!("  voxel-cluster --bake    contour the 3×3 field and write bake/cluster_*.json, then exit");
                return Ok(());
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    // Restore persisted display settings (if any) before the window opens; the
    // logo scene applies them once the renderer exists.
    display::load_from_disk();

    // Start on the logo splash; the scene manager drives logo → menu → game →
    // pause.
    run(SceneManager::new(Box::new(LogoScene::new())))?;
    Ok(())
}
