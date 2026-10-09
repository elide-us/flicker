//! flicker-csgtest: the CSG Test scene — the proving ground where the new
//! contouring engine (QEF mesher + corner-vector stretch) was verified against
//! the baked wave field, and the buildout home for the VOXEL EDITING TOOLS.
//! Deliberately forked from `flicker-pocclusters` (same 3×3 cluster field,
//! contour/mesh pipeline, and LOD wiring — lineage, not migration debt): a
//! stripped one-light stage (no celestial model; see `world_lighting`) so the
//! voxel geometry stays legible while carving.
//!
//! The world is the SEEDED WAVE FIELD, loaded from the `bakes/` package set
//! (written by `flicker-voxel`'s `bake_island -- wave`); the fly-cam spawn
//! frames the whole field. Each cluster contours its own region; meshing
//! closes the internal seams via the low-side-owns convention in
//! `flicker_voxel::mesh`.
//!
//! Pipeline: 3×3 `ClusterId`s → per-cluster derive + `mesh` jobs on the
//! worker pool (against a `NeighborContext` over the LOD-0 `ClusterMap`
//! source) → upload one mesh handle per cluster, drawn at its
//! `world_offset()`. Debug toggles let the user inspect the meshes
//! interactively (see controls below).
//!
//! NEXT HERE: the voxel editing tools — the generic 3D-manipulation gadget
//! (Translate / Rotate / Scale / Flip) grown out of the Clayworks joint gizmo
//! (`flicker_mechanics::gizmo`), driven by the controller-first Aim → Locked →
//! Modify selection model, with per-surface enable/disable of each mode
//! declared from the Lua side like every other walker behaviour control. The
//! same gadget serves the voxel-Template construction flow (move/rotate a
//! template before stamping). Design of record lives in MCP memory.
//!
//! Camera controls (rebindable via the `InputMap`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * Escape: open the pause menu (Resume / Quit).
//!
//! Debug toggles are driven by a DECLARATIVE component-tree HUD authored in
//! `csgtest.scene.json` (`tree` + folded `styles`): the flicker-widgets Rust
//! walker (`run_ui`) owns layout, hit-test, and draw; the pair script
//! (`csgtest.lua`) derives the display strings. Six clickable checkboxes:
//!   * Wireframe overlay on top of the solid mesh.
//!   * Corner-vector arrows — for every stored voxel whose
//!     `CornerVector` differs from the default, draw a line from the
//!     voxel's grid coord to the decoded corner tip. Visualizes where
//!     the contour's QEF placed each active cell's dual vertex.
//!   * Navmesh wireframe — the LOD2 walkable surface drawn magenta as
//!     floor-to-floor links between walkable-adjacent columns.
//!   * Surface walk — the DEFAULT locomotion: WASD walks in the XZ plane
//!     under gravity with a ground-clamp against the nav surface (consumed
//!     by `walk_step`/`ground_height_at`). Toggling it off gives fly mode —
//!     free 6-DOF, generating no nav.
//!   * Camera-driven LOD — each cluster's LOD follows its distance from
//!     the camera (smoothed to the mesher's ±1 adjacency invariant),
//!     re-meshing on a swap.
//!   * LOD billboards — a digit per cluster, on the navmesh surface at
//!     the cluster centre, showing that cluster's current LOD.
//!
//! Library only: the scene runs inside the unified `prism-alpha` launcher
//! (`cargo run -p prism-alpha`), which lists it in the scene picker. This
//! crate exposes a `scene()` factory and builds no standalone binary.

use std::cell::Cell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use flicker::render::{
    Camera, CompositeTarget, FrameGraph, LightRig, Mat4, MeshDrawOptions, MeshHandle, MeshIndices,
    MeshVertex, PassKind, RenderTargetHandle, Renderer, StageDef, StageInputs, TextureHandle, Vec2,
    Vec3,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, strings, SceneDef, Section, Sections, UiInput, UiIntents, UiState,
    WalkerHandler,
};
use flicker_input_core::ActionSignal;
use flicker_input_core::{AbstractControls, GamepadConfig, InputContext, InputState};
use flicker_input_router::{InputHandler, Router};
use flicker_shell::{PauseScene, Theme};
use flicker_voxel::{
    cluster_center_world, contour, in_nav_rings, BakedCluster, Cluster, ClusterId, ClusterMap,
    ClusterNav, CornerVector, FaceDir, HeightField, LocalCoord, Lod, Material, NeighborContext,
    CLUSTER_DIM, NAV_DIM,
};
use flicker_worker::WorkerPool;

mod route;
use route::{GameplayBase, RootHandler};

/// Side length of the cluster field, in clusters. A 3×3 row in XZ
/// gives one fully-interior cluster (all four lateral neighbors
/// present), which is what actually exercises seam tangent stitching
/// on every face simultaneously.
const FIELD_DIM: u16 = 3;

/// The ROOT surface's stage source — `stages.csgtest_world` in
/// `csgtest.scene.json`. A nested `surface` node names its source in the tree, but
/// the walker skips the ROOT node, so the scene names its own root stage here: the one
/// spelling of the recipe that draws the world and the ground fog over it.
const WORLD_STAGE: &str = "csgtest_world";

/// The SUN-SHADOW producer stage. Still the POCCLUSTERS stage name — and
/// `csgtest.scene.json` authors no `stages.pocclusters_sun_shadow` (its only stage is
/// `csgtest_world`), so `enter`'s `stage_def` lookup misses and the stage compiles to
/// `StageDef::default()`: the consumer binds the disabled default and the field draws
/// UNSHADOWED. Deliberate for now (the Cargo.toml scope says "no shadow"); authoring a
/// `csgtest_sun_shadow` stage in the scene file is all it takes to turn shadows on.
const SHADOW_STAGE: &str = "pocclusters_sun_shadow";

/// Side of the (square) sun-shadow depth map, in texels. A shadow map is square, so the
/// producer camera and the consumer uniform share one aspect-1 matrix.
///
/// This is the ONE shadow knob still spelled in Rust rather than the scene file (bias/light
/// live on the consumer `shadow_map` line, extent + rate on the producer stage). The size is
/// fixed at `create_render_target` time in `enter`, BEFORE any surface exists, and the
/// attachment schema is rect×`scale` (a fraction of the surface's seated rect), so it cannot
/// express an absolute texel count; `StageDef` carries no resolution field either. Moving it
/// into data would need a new authored field on the stage schema (a `flicker-render`
/// `StageDef` + parser change), which is out of scope for this slice — a flagged Aaron call,
/// not a defect.
const SHADOW_SIZE: u32 = 2048;

/// The game's run phase. `Booting` covers world generation (physics off, the
/// 3D clipmap not drawn — just the loading widget); `Active` is live play.
#[derive(Copy, Clone, PartialEq, Eq)]
enum GamePhase {
    Booting,
    Active,
}

struct GameScene {
    /// LOD-0 source-of-truth cluster data:
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

    /// POC grass scatter (kept out of `meshes` so the LOD re-mesh never frees it): one uploaded
    /// mesh handle per GrassField variant, plus the precomputed per-instance `(variant index,
    /// model matrix)`. Empty when the promoted grass assets are absent.
    grass_meshes: Vec<MeshHandle>,
    grass_instances: Vec<(usize, Mat4)>,

    /// First-person camera state.
    position: Vec3,
    yaw: f32,
    pitch: f32,

    /// The ROOT surface's authored stage ([`WORLD_STAGE`] in the scene file): the recipe
    /// that runs the world's own drawing and the ground fog over it. Compiled ONCE in
    /// `enter`; the numbers the simulation owns reach its binds through
    /// [`GameScene::stage_inputs`], so nothing re-reads JSON per frame.
    world_stage: StageDef,
    /// The SUN-SHADOW producer stage ([`SHADOW_STAGE`]), compiled ONCE in `enter`. Its
    /// `extent`/`bias` are read out of the compiled recipe — the authored art knobs.
    shadow_stage: StageDef,
    /// The offscreen depth target the sun shadow renders into (created in `enter`), sampled
    /// by the ROOT surface's `shadow_map` consumer. `None` before `enter` runs.
    shadow_target: Option<RenderTargetHandle>,
    /// The light-view-projection the depth CURRENTLY in [`Self::shadow_target`] was rendered
    /// with — captured inside the producer surface's draw closure, which the frame graph runs
    /// ONLY when the throttled shadow clock fires (`rate {hz:20}`). The consumer samples with
    /// this captured matrix, so a throttled frame reads the stale depth with the matrix it was
    /// actually drawn with (never a fresh matrix over a stale depth), and the matrix is rebuilt
    /// only when the depth is. `Cell` because both draw closures hold `&self`.
    shadow_light_vp: Cell<Mat4>,
    /// The lowest walkable floor over the meshed field — where the fog slab lies,
    /// published to the recipe as `fog_floor`. Recomputed when the nav surfaces arrive,
    /// never per frame; `None` until they do.
    fog_floor: Option<f32>,
    /// The fog's drift clock, seconds — real time, independent of the celestial speed.
    /// Published to the recipe as `fog_time`.
    fog_time: f32,

    /// Mouse-look tuning (sensitivity + invert) from the shell settings panel +
    /// the scene-owned `move_speed` (the HUD slider). The action MAPS live with
    /// the pump now — the scene resolves nothing itself.
    controls: AbstractControls,
    /// Pad tuning handed to the pause overlay; the pump owns the live config.
    gamepad_config: GamepadConfig,
    /// The pair script (`csgtest.lua`) — derives the HUD display strings
    /// from the raw Model each frame (five-line split). `None` if it failed to
    /// load; the HUD then shows raw-less readouts.
    script: Option<ScriptHost>,

    /// The in-scene HUD as a DECLARATIVE component tree, taken ONCE from
    /// `csgtest.scene.json`'s authored `tree` at construction (the walker
    /// redraws this cached tree every frame with fresh Model bindings). `None`
    /// if the scene file has none — the scene still runs without a HUD.
    ui_tree: Option<UiNode>,
    /// The screen's declarative signal bindings (S9), collected from the cached
    /// tree's ROOT `on_<signal>` props at the same build point (`on_menu =
    /// "pause_open"`). The walker layer consumes a declared signal; `update`
    /// maps the fired name onto its transition.
    ui_intents: UiIntents,
    /// Intent names fired last frame — republished ONCE into the next HUD Model
    /// as the transient `sig_<name>` mirror (S9), then dropped.
    fired_sigs: Vec<String>,
    /// Retained walker interaction state (slider drag capture) across frames.
    ui_state: UiState,
    /// The screen's declared surface set (S8): the inspector panel + the "none"
    /// pick row (both derived from the selection each frame) and the floating
    /// chat window (always on today — declared so S9 can toggle/diff it). Owns
    /// the `visible_bind` gates both walker passes read; published into each
    /// pass's Model. (`walk` is NOT here: it is the surface-walk checkbox's
    /// two-way control bind that some rows also gate on.)
    surfaces: Sections,
    /// The Prism-token-resolved `ui_theme.json` the walker resolves node
    /// `style` paths against (colours/sizes; the palette stays single-sourced).
    ui_styles: serde_json::Value,
    /// This frame's HUD draw commands — the walker builds them in `update`,
    /// `render` blits them (one walk per frame; no per-frame Lua).
    hud_commands: Vec<HudCommand>,

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
    /// gravity + a ground-clamp.
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
    /// Set at construction (walk is the default) and on a fly→walk toggle:
    /// the camera snaps down onto the surface beneath it on the first frame
    /// the nav under it is available (nav is generated asynchronously, so the
    /// snap waits for it).
    walk_needs_snap: bool,

    /// Gothic UI theme: drawn as the loading widget while `Booting`, and handed
    /// to each `PauseScene` we push (so pausing never re-uploads). `None` until
    /// `enter`.
    ui_theme: Option<Theme>,

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
            grass_meshes: Vec::new(),
            grass_instances: Vec::new(),
            // Field CENTER in X/Z (same center expression as the walk recenter), a
            // fly-safe height up the cluster's vertical extent so the camera starts
            // INSIDE the map rather than at the world origin corner (spec task D).
            position: Vec3::new(
                FIELD_DIM as f32 * 0.5 * CLUSTER_DIM as f32,
                CLUSTER_DIM as f32 * 0.75,
                FIELD_DIM as f32 * 0.5 * CLUSTER_DIM as f32,
            ),
            yaw: 0.0,
            pitch: 0.0,
            controls: AbstractControls::default(),
            gamepad_config: GamepadConfig::default(),
            script: None,
            ui_tree: None,
            ui_intents: UiIntents::default(),
            fired_sigs: Vec::new(),
            ui_state: UiState::new(),
            // The Screen declaration (S8). `inspector`/`pick_none` publish under the
            // pre-existing tree keys (`has_pick`/`no_pick`); `chat` gates the floating
            // window's root and starts on (nothing hides it yet — S9 data).
            surfaces: Sections::new(vec![
                Section::new("inspector").key("has_pick"),
                Section::new("pick_none").key("no_pick").on(),
                Section::new("chat").on(),
            ]),
            ui_styles: serde_json::Value::Null,
            hud_commands: Vec::new(),
            wireframe_on: false,
            corner_arrows_on: false,
            navmesh_on: true,
            corner_arrows: Vec::new(),
            navmesh_segments: Vec::new(),
            camera_lod_on: false,
            lod_billboards_on: false,
            // Fly is the DEFAULT locomotion for the CSG Test Scene: boot in free
            // 6-DOF flight above the field (spawn set in `enter`) so the whole
            // wave-field mesh is in view with no ground-clamp. Toggle Surface Walk
            // on to snap onto the nav surface.
            locomotion_walk: false,
            lod_field: [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize],
            digit_atlas: None,
            world_stage: StageDef::default(),
            shadow_stage: StageDef::default(),
            shadow_target: None,
            shadow_light_vp: Cell::new(Mat4::IDENTITY),
            fog_floor: None,
            fog_time: 0.0,
            pick_meshes: Vec::new(),
            selection: None,
            vy: 0.0,
            grounded: false,
            walk_needs_snap: true,
            ui_theme: None,
            phase: GamePhase::Booting,
            nav_ready_target: 0,
        }
    }
}

/// The pair script (`content/sensorium/scripts/csgtest.lua`) — embedded at
/// compile time like every migrated scene's; `derive()` turns the raw Model
/// into the HUD display strings.
const CSGTEST_SCRIPT: &str = include_str!("../../../../content/sensorium/scripts/csgtest.lua");

impl GameScene {
    /// Build the game scene off the manifest's def (the five-line split): the
    /// authored HUD tree + the folded styles come from `csgtest.scene.json`,
    /// the pair script derives the display strings. Other state takes its
    /// placeholder values from [`Default`]; the world + camera come up in
    /// [`Scene::enter`].
    fn new(def: &SceneDef) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let ui_tree = def.tree.clone();
        if ui_tree.is_none() {
            tracing::error!("csgtest scene file has no `tree` — no HUD");
        }
        // The screen's declarative bindings (S9), read off the authored root once —
        // cached exactly like the tree they were collected from.
        let ui_intents = ui_tree.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(CSGTEST_SCRIPT, "csgtest.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("csgtest.lua failed to load — raw HUD values only: {e}");
                None
            }
        };
        Self {
            ui_tree,
            ui_intents,
            ui_styles,
            script,
            ..Self::default()
        }
    }

    /// This frame's published numbers for the root stage's recipe — the ONE channel
    /// between the simulation and the authored `ground_fog` / `tonemap_grade` passes. Every
    /// key here is a `*_bind` the recipe names (and vice versa: the scene's own gate proves
    /// it), so a bind can never resolve to nothing.
    ///
    /// The stage CLOCK rides the same channel as a typed field rather than a key: it is
    /// simulation output the frame graph reads directly (`stage.lighting.driven(t)`),
    /// not a name a recipe binds. It is the SAME `fog_time` accumulator the fog's
    /// `time_bind` already publishes — one scene clock, so the hearth's flicker and the
    /// fog's drift never drift apart, and both are deterministic rather than wall-clock.
    fn stage_inputs(&self) -> StageInputs {
        let mut inputs = StageInputs::default();
        inputs
            .clock(self.fog_time)
            // The lowest walkable floor the slab lies on — swept once per nav rebuild,
            // never per frame; before any nav has arrived the slab sits at world zero.
            .set("fog_floor", self.fog_floor.unwrap_or(0.0))
            .set("fog_time", self.fog_time);
        inputs
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

impl GameScene {
    /// Origin + direction of the picking ray for screen-space cursor
    /// `cursor` on a viewport of pixel size `viewport`.
    ///
    /// Camera basis built to **match** the renderer's view matrix
    /// (`glam::Mat4::look_at_rh` in [`flicker_render::Camera::view`]):
    ///   * `r = f.cross(Y)` — same as `look_at_rh`'s internal right
    ///     vector (`forward × up`). For the scene's yaw-0/face-+Z
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
            for tri in indices.as_chunks::<3>().0 {
                let a = verts[tri[0] as usize];
                let b = verts[tri[1] as usize];
                let c = verts[tri[2] as usize];
                if let Some(t) = Self::ray_triangle(origin, dir, a, b, c) {
                    if best.is_none_or(|(bt, _)| t < bt) {
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

/// One corner of a virtual voxel. A corner is a neighbour voxel's stored vector `V`
/// (owned by the voxel whose min-corner is `m = p + (bx-1, by-1, bz-1)`), kept in the
/// two frames the inspector + renderer reason in: `self_relative` (that vector expressed
/// from THIS cell's centre `p` — its local frame) and `world` (its absolute position).
#[derive(Copy, Clone, Debug)]
struct VirtualVoxelCorner {
    /// `(m - p) + V.to_components()`: the corner expressed from this cell's centre `p`
    /// — this voxel's local frame. For default owners this collapses to
    /// `(bx - 0.5, by - 0.5, bz - 0.5)`, the clean lattice cube. The inspector panel's
    /// `local` columns.
    self_relative: [f32; 3],
    /// Absolute world-space corner position (`cluster_origin + m + V`). The renderer's
    /// wireframe overlay consumes this; the inspector panel's `world` columns.
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
            self_relative: [0.0; 3],
            world: Vec3::ZERO,
        }; 8];
        for (o, corner) in corners.iter_mut().enumerate() {
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
            *corner = VirtualVoxelCorner {
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
        // Id 23 = Gravel in the material catalog (materials.json): a neutral
        // mid-value stone (~0.5, faintly warm) chosen so the fixed studio light
        // reads Lambertian gradients cleanly — the closest catalog match to the
        // retired demo palette's matte STONE. (History: was demo index 1,
        // DEEP_WATER navy, which biased the scene blue and swallowed dim light.)
        let material = Material::new(23, 23, 0);
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
            // Fallback when no bake is present: contour the ISLAND dome live.
            // NOTE this does NOT reproduce the wave-field bakes this scene
            // normally loads (`bakes/` = `bake_island -- wave`) — it's the
            // island terrain instead. Kept as-is from the fork; acceptable for
            // a dev fallback, but don't mistake it for the proven wave field.
            let island = HeightField::island(id.world_offset());
            source.insert(*id, contour(&island, material, *id));
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
        // The nav surfaces just changed — re-anchor the fog slab on the lowest floor.
        self.fog_floor = nav_floor_min(&self.navs);
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
    // The FULL in-plane neighborhood — the four faces AND the four
    // diagonals, so the seam quads at a 4-cluster corner junction
    // resolve instead of dropping (the old corner holes). The derived
    // neighbors are owned here; the context borrows them.
    let ring: Vec<(i32, i32, (Cluster, Lod))> = (-1i32..=1)
        .flat_map(|dx| (-1i32..=1).map(move |dz| (dx, dz)))
        .filter(|&(dx, dz)| dx != 0 || dz != 0)
        .filter_map(|(dx, dz)| {
            let (nx, nz) = (x as i32 + dx, z as i32 + dz);
            ((0..FIELD_DIM as i32).contains(&nx) && (0..FIELD_DIM as i32).contains(&nz))
                .then(|| (dx, dz, derive(nx as u16, nz as u16)))
        })
        .collect();
    let mut neighbors = NeighborContext::none();
    for (dx, dz, (c, l)) in &ring {
        neighbors.set(*dx, 0, *dz, c, *l);
    }

    let id = ClusterId::new(self_lod.level(), x, 0, z);
    let off = id.world_offset();
    let origin = Vec3::new(off[0], off[1], off[2]);
    let cm = flicker_voxel::mesh(&self_c, &neighbors, self_lod);

    // Nav (LOD2 walkable surface) for clusters in rings 0–2 — but only in
    // surface-walk mode (the default). In fly mode no nav is generated and the
    // engine produces no collisions; nav exists solely for walking/collision,
    // which fly mode does not use. State is LOD-independent (derive copies it
    // verbatim), so it matches the source.
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
    fn walk_step(&mut self, dt_s: f32, wish: Vec2) {
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
        // walking). `wish` is the pump-resolved (strafe, forward) pair — one
        // axis path for keyboard, d-pad, and stick alike (spec §9 / input-P3).
        let horizontal = self.move_forward() * wish.y + self.move_right() * wish.x;
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

    /// The engine values published to the HUD each frame (the walker reads them by
    /// name via `bind` / `text_bind`). Three kinds: the six toggle BOOLS (two-way
    /// with the checkboxes), the two slider NUMBERS (move speed / sensitivity), and
    /// the stat lines PRE-FORMATTED to the exact strings the text nodes display — the
    /// component walker has no printf, so the formatting lives here. The deep
    /// virtual-voxel inspector is *not* here; it stays Rust (`render`).
    fn hud_model(&self) -> ValueMap {
        // The ENGINE publishes RAW runtime variables + RESOLVED WORD tokens
        // (localization stays stringtable-resolved engine-side); the PAIR SCRIPT
        // (csgtest.lua) derives the display strings — the five-line split.
        // Pre-formatted values remain only where a readout IS one formatted
        // value (the celestial fmt_* clock/phase/month strings, the sablework-
        // sanctioned shape).
        let walk_mode = self.locomotion_walk;
        let mode_tag = strings::resolve(if walk_mode {
            "$pc_walk_mode"
        } else {
            "$pc_fly_mode"
        })
        .into_owned();

        let mut raw = ValueMap::new()
            // Debug-overlay toggles (BOOL, two-way).
            .with("wireframe", self.wireframe_on)
            .with("arrows", self.corner_arrows_on)
            .with("navmesh", self.navmesh_on)
            .with("camera_lod", self.camera_lod_on)
            .with("lod_billboards", self.lod_billboards_on)
            .with("walk", walk_mode)
            // Move-speed slider (NUMBER, two-way).
            .with("move_speed", self.controls.move_speed)
            // Camera + grid + diagnostics raw values.
            .with("cam_x", f64::from(self.position.x))
            .with("cam_y", f64::from(self.position.y))
            .with("cam_z", f64::from(self.position.z))
            .with("yaw_deg", f64::from(self.yaw.to_degrees()))
            .with("pitch_deg", f64::from(self.pitch.to_degrees()))
            .with("mesh_count", self.meshes.len() as f64)
            .with("cluster_dim", f64::from(CLUSTER_DIM))
            .with("field_dim", f64::from(FIELD_DIM))
            .with("arrow_count", self.corner_arrows.len() as f64)
            .with("nav_count", self.navs.len() as f64)
            .with("vy", f64::from(self.vy))
            // Resolved WORDS the pair script composes with (never raw English).
            .with("mode_tag", mode_tag.as_str())
            .with(
                "w_cluster_field",
                strings::resolve("$pc_cluster_field").as_ref(),
            )
            .with("w_yaw", strings::resolve("$pc_yaw").as_ref())
            .with("w_pitch", strings::resolve("$pc_pitch").as_ref())
            .with("w_clusters", strings::resolve("$pc_clusters").as_ref())
            .with("w_voxels", strings::resolve("$pc_voxels").as_ref())
            .with(
                "w_mode",
                strings::resolve(if walk_mode {
                    "$pc_mode_walk"
                } else {
                    "$pc_mode_fly"
                })
                .as_ref(),
            )
            .with("w_arrows", strings::resolve("$pc_arrows").as_ref())
            .with("w_nav", strings::resolve("$pc_nav").as_ref())
            .with("w_cluster", strings::resolve("$pc_cluster").as_ref())
            .with("w_lod", strings::resolve("$pc_lod").as_ref())
            .with("w_voxel", strings::resolve("$pc_voxel").as_ref())
            .with(
                "w_state",
                strings::resolve(if self.grounded {
                    "$pc_grounded"
                } else {
                    "$pc_airborne"
                })
                .as_ref(),
            )
            .with("w_ground_y", strings::resolve("$pc_ground_y").as_ref())
            .with("w_cell", strings::resolve("$pc_cell").as_ref())
            .with("w_corners", strings::resolve("$pc_corners").as_ref());

        // Walk readout raws (their row is gated on `walk`).
        if walk_mode {
            let ground = match self.ground_height_at(self.position.x, self.position.z) {
                Some(g) => format!("{g:.0}"),
                None => "\u{2014}".to_string(),
            };
            raw.set("ground_y_s", ground);
        }

        // The declared surfaces' gates (`has_pick` / `no_pick` / `chat`) — one publish.
        self.surfaces.publish(&mut raw);

        // Selection raws: the picked face + the inspected cell's 8 corners (each a
        // neighbour voxel's stored vector in THIS voxel's local frame plus its
        // absolute world position). Published only while a face is selected — the
        // inspector panel is gated on `has_pick`.
        if let Some((id, p)) = self.selection {
            raw.set("pick_cx", f64::from(id.x()));
            raw.set("pick_cy", f64::from(id.y()));
            raw.set("pick_cz", f64::from(id.z()));
            raw.set("pick_lod", f64::from(id.lod()));
            raw.set("pick_vx", f64::from(p[0]));
            raw.set("pick_vy", f64::from(p[1]));
            raw.set("pick_vz", f64::from(p[2]));
        }
        if let Some(vv) = self.current_virtual_voxel() {
            raw.set("insp_cx", f64::from(vv.center_local[0]));
            raw.set("insp_cy", f64::from(vv.center_local[1]));
            raw.set("insp_cz", f64::from(vv.center_local[2]));
            raw.set("insp_kx", f64::from(vv.cluster.x()));
            raw.set("insp_ky", f64::from(vv.cluster.y()));
            raw.set("insp_kz", f64::from(vv.cluster.z()));
            raw.set("insp_lod", f64::from(vv.cluster.lod()));
            for (i, c) in vv.corners.iter().enumerate() {
                raw.set(format!("c{i}_lx"), f64::from(c.self_relative[0]));
                raw.set(format!("c{i}_ly"), f64::from(c.self_relative[1]));
                raw.set(format!("c{i}_lz"), f64::from(c.self_relative[2]));
                raw.set(format!("c{i}_wx"), f64::from(c.world.x));
                raw.set(format!("c{i}_wy"), f64::from(c.world.y));
                raw.set(format!("c{i}_wz"), f64::from(c.world.z));
            }
        }

        // The pair script derives the display strings over the raw publish.
        let mut m = raw.clone();
        if let Some(script) = &self.script {
            if let Err(e) = script.set_model(&raw) {
                tracing::error!("csgtest: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("csgtest.lua derive() failed: {e}"),
            }
        }

        // The transient `sig_<name>` mirror (S9): intent names fired last frame
        // ride exactly this ONE publish for scripts to observe (`update` clears
        // them right after the walk).
        UiIntents::mirror_into(&mut m, &self.fired_sigs);

        m
    }
}

/// Build the CSG-cluster editor as a boxed [`Scene`] for the `prism-alpha` launcher
/// (and any other shell host). The one public entry point — the scene owns its world
/// generation, HUD, and pause plumbing internally, exactly as the shell expects.
pub fn scene(def: &SceneDef) -> Box<dyn Scene> {
    Box::new(GameScene::new(def))
}

impl Scene for GameScene {
    fn enter(&mut self, renderer: &mut Renderer) {
        // The mesh pass boots with an all-magenta (loud-wrong) palette; give it
        // the material catalog's colours so the field draws real materials. An
        // unreadable catalog stays magenta — visibly wrong, never silently so.
        let data_dir = flicker_content::roots().data();
        match flicker_materials::Tables::from_source(&flicker_materials::JsonTableSource::new(
            &data_dir,
        )) {
            Ok(t) => renderer.set_material_palette(&t.render_palette()),
            Err(e) => tracing::warn!(
                "material catalog unreadable at {} — mesh palette stays loud magenta: {e}",
                data_dir.display()
            ),
        }

        // Start INSIDE the field, near its centre, angled slightly down. Done
        // before generation so the nav-ring gate (and the boot readiness
        // target) use the real camera pose.
        let field_extent = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        let center_x = field_extent * 0.5;
        // Fly-cam spawn: above the SOUTH (−Z) cluster, looking NORTH (+Z) and angled down
        // so the whole 3×3 wave field is in view (south cluster centre ≈ (384, 128, 128)).
        self.position = Vec3::new(center_x, CLUSTER_DIM as f32 * 1.1, CLUSTER_DIM as f32 * 0.4);
        self.yaw = 0.0; // face +Z = north, across the field.
        self.pitch = -0.5; // angled down for a field overview.

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

        // The ROOT surface's recipe, compiled ONCE from the scene file's own `stages`
        // section (`stage_def` warns every authoring problem it found). An unauthored or
        // unreadable source still shows the world — the default recipe is exactly the
        // content pass — but loses the fog the stage was carrying.
        self.world_stage =
            flicker::ui::stage_def(&self.ui_styles, WORLD_STAGE).unwrap_or_else(|| {
                tracing::error!(
                    "stages.{WORLD_STAGE} did not compile — the field draws with no ground fog"
                );
                StageDef::default()
            });

        // The SUN-SHADOW producer: its stage (rate/extent/bias) compiled once, and its
        // offscreen square depth target allocated once. A failure to compile just leaves the
        // world unshadowed (the consumer binds the disabled default) — never a crash.
        self.shadow_stage =
            flicker::ui::stage_def(&self.ui_styles, SHADOW_STAGE).unwrap_or_default();
        self.shadow_target = Some(renderer.create_render_target(SHADOW_SIZE, SHADOW_SIZE));

        // Gothic UI theme — drawn as the loading widget while Booting, and
        // handed to each PauseScene we push (so pausing never re-uploads).
        self.ui_theme = Some(Theme::build(renderer));

        // Seed the mouse LOOK settings from the shell's settings panel (sensitivity +
        // invert) so the panel's values apply from the first frame; `move_speed` stays
        // a scene control. Live changes arrive via `take_pending_input` in `update`.
        let look = flicker_shell::input_controls();
        self.controls.mouse_sensitivity = look.mouse_sensitivity;
        self.controls.invert_mouse_pitch = look.invert_mouse_pitch;
        self.controls.invert_mouse_yaw = look.invert_mouse_yaw;
    }

    fn update(
        &mut self,
        dt: Duration,
        input: &InputState,
        signals: &mut SceneInput,
        renderer: &Renderer,
    ) -> Transition {
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
        self.fog_time += dt_s;

        // Pick up input settings changes made in the pause→settings overlay.
        // Live input-settings changes from pause→settings: apply the mouse LOOK
        // settings the panel owns (sensitivity + invert) + any keybind change.
        // `move_speed` is a scene control (the HUD slider), so it is NOT overwritten
        // — a wholesale `self.controls = controls` would reset it to the panel default.
        if let Some((_map, look, _gp)) = flicker_shell::take_pending_input() {
            // The PUMP owns the live action maps (rebinds apply there); the scene
            // takes only the mouse-LOOK half it still consumes directly.
            self.controls.mouse_sensitivity = look.mouse_sensitivity;
            self.controls.invert_mouse_pitch = look.invert_mouse_pitch;
            self.controls.invert_mouse_yaw = look.invert_mouse_yaw;
        }

        // The in-scene HUD is a DECLARATIVE component tree walked by the Rust
        // component walker (`run_ui`): build the Model, walk the cached tree → this
        // frame's draw commands (stashed for `render`) + interaction results. The
        // toggle states + slider values come back two-way, so the engine stays the
        // single source of truth. `hud_hit` = cursor over any UI region (or a slider
        // drag), which gates the world pick below so a checkbox/slider click doesn't
        // also fire a face pick behind the panel.
        let screen = renderer.size();
        let mut hud_hit = false;
        // Surface states derived from scene state, synced once — the Screen
        // declaration is then published into BOTH walker passes' Models.
        let has_pick = self.selection.is_some();
        self.surfaces.set("inspector", has_pick);
        self.surfaces.set("pick_none", !has_pick);
        if self.ui_tree.is_some() {
            let model = self.hud_model();
            let snap = UiInput {
                mouse: input.mouse_position,
                clicked: input.mouse_left_pressed,
                down: input.mouse_left,
                right_down: input.mouse_right,
                screen,
                wheel: input.mouse_wheel_delta,
                exclusive: false,
                motion: Default::default(),
            };
            let frame = {
                // Disjoint field borrows: `ui_tree` / `ui_styles` read, `ui_state`
                // mutated.
                let tree = self.ui_tree.as_ref().unwrap();
                run_ui(tree, &model, &self.ui_styles, &snap, &mut self.ui_state)
            };
            let results = frame.results;
            self.hud_commands = frame.commands;
            hud_hit = results.is_on("hud_hit");

            self.wireframe_on = results.is_on("wireframe");
            self.corner_arrows_on = results.is_on("arrows");
            self.navmesh_on = results.is_on("navmesh");
            self.camera_lod_on = results.is_on("camera_lod");
            self.lod_billboards_on = results.is_on("lod_billboards");

            // The move-speed + sensitivity sliders report their values back (the
            // walker returns the current value unchanged when not dragging, so this
            // is idempotent).
            if let Some(v) = results.number("move_speed") {
                self.controls.move_speed = v as f32;
            }
            // Mouse sensitivity is owned by the shell settings panel now (not a HUD
            // control) — it arrives via `input_controls()` (enter) + `take_pending_input`.

            // Locomotion mode is now a `walk` checkbox (was the old dropdown).
            // Surface-walk generates the nav surface; fly mode generates none. A
            // change re-meshes the field so nav appears/disappears with it.
            let walk = results.is_on("walk");
            if walk != self.locomotion_walk {
                self.locomotion_walk = walk;
                // Entering walk: re-mesh to generate nav, then snap the camera onto
                // the surface once that nav arrives.
                if walk {
                    self.walk_needs_snap = true;
                    self.vy = 0.0;
                }
                self.submit_field_jobs();
            }

            // Desired per-cluster LOD field: the camera-driven distance policy
            // (smoothed to the mesher's ±1 adjacency invariant) when enabled, else
            // all clusters at LOD 0. A change triggers a re-derive + re-mesh of the
            // changed clusters in `render` — cheap (render-time stride, no re-contour).
            let mut desired = [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize];
            if self.camera_lod_on {
                for x in 0..FIELD_DIM {
                    for z in 0..FIELD_DIM {
                        desired[x as usize][z as usize] =
                            target_lod_for_cluster(self.position, ClusterId::new(0, x, 0, z));
                    }
                }
                smooth_lod_field(&mut desired);
            }
            if desired != self.lod_field {
                self.lod_field = desired;
                self.submit_field_jobs();
            }
        }

        // ── Dispatch the PUMP's resolved events through the 3-handler chain
        // (root → walker → gameplay). The runner resolved this frame over the
        // scene's declared context (`input_context()`). ──
        let mut root = RootHandler;
        let mut gameplay = GameplayBase::default();
        // The walker layer wraps this frame's pointer-consume (HUD `hud_hit`)
        // + the screen's DECLARED intents (S9: `on_menu = "pause_open"`).
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, hud_hit).with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 3] = [&mut root, &mut walker, &mut gameplay];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }

        // Surface context wiring (S9): any declared-surface flip since last frame
        // becomes Push/PopContext on the same queue (no csgtest surface
        // carries a context today — the seam is standard, the call a live no-op).
        // The RUNNER applies the queued requests to the pump after `update`; the
        // chat field's walker focus is re-asserted per-frame from `chat_focused`.
        self.surfaces.apply_section_contexts(signals.route);
        // The screen's fired intents (S9), drained once per frame: acted on below
        // and queued for the one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto its
        // pause push — the root's hardcoded Menu arm is gone.
        if self.fired_sigs.iter().any(|n| n == "pause_open") {
            let theme = self.ui_theme.expect("pause theme built in enter");
            let pause_map = flicker_shell::input_profile()
                .context_map("World")
                .cloned()
                .unwrap_or_else(flicker_input_core::InputMap::wasd_and_mouse);
            return Transition::Push(Box::new(PauseScene::new(
                theme,
                &pause_map,
                &self.controls,
                &self.gamepad_config,
            )));
        }

        // Gameplay base (Pass-only): the world-pick runs only when a `PrimaryAction`
        // press bubbled past the walker to the base — the typed form of the old
        // `!hud_hit && !chat_hit && active()==World` gate.
        if gameplay.pick {
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

        // Camera look + movement — continuous queries against the PUMP's
        // active-context bindings (spec §9 / input-P3): `signals.pointer_delta`
        // is the mouse-look channel (the profile gates Look* on MouseMotion
        // right-drag), `signals.axis` unifies held keys and stick deflection
        // into ONE 0..1 path per direction signal. While chat owns the keyboard
        // the TextEntry map binds nothing, so every query reads zero — the gate
        // below is belt-and-braces.
        {
            // Mouse look: per-frame pixel deltas, frame-absolute (no dt).
            let mouse = Vec2::new(
                signals.pointer_delta(ActionSignal::LookRight, input)
                    - signals.pointer_delta(ActionSignal::LookLeft, input),
                signals.pointer_delta(ActionSignal::LookDown, input)
                    - signals.pointer_delta(ActionSignal::LookUp, input),
            );
            if mouse != Vec2::ZERO {
                let (dyaw, dpitch) = self.controls.look_delta_mouse(mouse);
                self.yaw -= dyaw;
                self.pitch = (self.pitch + dpitch).clamp(-1.5, 1.5);
            }
            // Stick look: a deadzone-aware rate, dt-scaled to match the mouse feel.
            let stick = Vec2::new(
                signals.axis(ActionSignal::LookRight, input)
                    - signals.axis(ActionSignal::LookLeft, input),
                signals.axis(ActionSignal::LookUp, input)
                    - signals.axis(ActionSignal::LookDown, input),
            );
            if stick != Vec2::ZERO {
                let (dyaw, dpitch) = self.controls.look_delta_stick(stick);
                self.yaw -= dyaw * dt_s;
                self.pitch = (self.pitch + dpitch * dt_s).clamp(-1.5, 1.5);
            }

            // Movement intent (KBM + pad on the one axis path): x = strafe, y =
            // forward. Fly adds the vertical pair; walk flattens to XZ.
            let wish = Vec2::new(
                signals.axis(ActionSignal::StrafeRight, input)
                    - signals.axis(ActionSignal::StrafeLeft, input),
                signals.axis(ActionSignal::MoveForward, input)
                    - signals.axis(ActionSignal::MoveBackward, input),
            );
            if self.locomotion_walk {
                self.walk_step(dt_s, wish);
            } else {
                let mut motion = self.move_forward() * wish.y + self.move_right() * wish.x;
                motion += Vec3::Y
                    * (signals.axis(ActionSignal::MoveUp, input)
                        - signals.axis(ActionSignal::MoveDown, input));
                if motion.length_squared() > 1.0 {
                    motion = motion.normalize();
                }
                if motion.length_squared() > 0.0 {
                    self.position += motion * self.controls.move_speed * dt_s;
                }
            }
        }

        Transition::None
    }

    fn input_context(&self) -> Option<InputContext> {
        // Chat owns the keyboard → the runner resolves the pump over the (empty)
        // TextEntry map, so no gameplay signal fires and every continuous query
        // reads zero. World otherwise (the default base).
        None
    }

    fn render<'f>(&'f mut self, renderer: &mut Renderer, fg: &mut FrameGraph<'f>) {
        // Apply any completed mesh builds (uploads happen here — `render` owns
        // `&mut Renderer`). Runs while Booting too, so the world cooks under the
        // loading widget.
        self.drain_and_apply(renderer);

        // The ROOT surface — the world, or the loading widget while Booting — declared
        // straight into the swapchain (roots run after every offscreen pass). The world
        // runs its AUTHORED recipe (content, then the ground fog that reads the depth the
        // content wrote); the loading widget is a bare root element, with no stage to run.
        if matches!(self.phase, GamePhase::Booting) {
            if let Some(theme) = self.ui_theme {
                let screen = renderer.size();
                let progress = self.boot_progress();
                fg.root(move |r| theme.draw_loading(r, screen, progress));
            }
            return;
        }

        // A shared reborrow lets the world surface's content closure and the HUD overlay
        // both read `self` for the rest of the frame (both are `&self`); the closures live
        // in the graph until the manager's one `execute`.
        let me = &*self;

        // THE SUN SHADOW. Each knob comes from its ONE authority: the `light` the shadow is
        // cast for and the sampling `bias` from the world recipe's CONSUMER `shadow_map` line
        // (where the room applies its shadow and an artist tunes it); the caster-box `extent`
        // from the PRODUCER stage (which fits the depth render). The producer is an offscreen
        // Target (the frame graph renders every Target before every Root, so no authored edge
        // is needed for a root consumer). `None` (unauthored/uncompiled) leaves the world
        // binding the disabled default — byte-identical to no shadow.
        let shadow = me
            .shadow_target
            .zip(shadow_knobs(&me.world_stage, &me.shadow_stage));
        if let Some((target, (light, _, extent))) = shadow {
            fg.surface(
                CompositeTarget::Target(target),
                &me.shadow_stage,
                me.stage_inputs(),
                me.shadow_stage.rate,
                move |r| {
                    // Capture the light-view-projection AT THE MOMENT the depth renders: this
                    // closure runs ONLY when the per-surface clock fires (the graph skips it on
                    // a throttled frame), so the matrix stored here is always the one the depth
                    // now in the target was drawn with — the consumer then samples with it, and
                    // it is rebuilt only when the depth is. The caster view (the light's)
                    // REPLACES the camera, so the depth is written from the sun's POV; then
                    // draw the terrain casters.
                    let light_vp = me.sun_light_vp(light, extent);
                    me.shadow_light_vp.set(light_vp);
                    r.begin_shadow_view(light_vp);
                    me.draw_casters(r);
                },
            );
        }

        fg.surface(
            CompositeTarget::Screen,
            &me.world_stage,
            me.stage_inputs(),
            me.world_stage.rate,
            move |r| {
                // Bind the sun shadow BEFORE the world's lit `scene` pass encodes (the consumer
                // role of the recipe's `shadow_map` line — the pass itself is the ordering
                // marker; the scene wires the resource, exactly like a composite). Sample with
                // the matrix CAPTURED when the producer last rendered its depth
                // (`shadow_light_vp`), so matrix and depth stay locked across throttled frames;
                // `bias` + `light` are the consumer's own authored knobs.
                if let Some((target, (light, bias, _))) = shadow {
                    r.set_shadow_source(target, me.shadow_light_vp.get(), bias, light);
                }
                me.draw_world(r)
            },
        );

        // The component-walker HUD: blit this frame's draw commands (built in `update` by
        // `run_ui`) — the screen surface's final 2D, as one overlay run after the
        // composite. Rects + text only (no engine textures), so `white` — the 1×1 fill
        // pixel — is the entire texture table.
        if let Some(white) = me.white {
            let hud_commands = &me.hud_commands;
            fg.overlay(move |r| {
                render_hud(r, hud_commands, white, &[]);
            });
        }
    }

    fn exit(&mut self, renderer: &mut Renderer) {
        // Give the offscreen target back — the renderer owns the texture, this scene owns only
        // the handle, so teardown never reclaims it automatically (rule 728E682F); the 2048²
        // shadow slot would leak per re-entry otherwise. This is the only offscreen render
        // target this scene owns (the animated water is real geometry, not a reflection RT).
        if let Some(t) = self.shadow_target.take() {
            renderer.free_render_target(t);
        }
    }
}

impl GameScene {
    /// The world as drawn into its surface — camera, clusters and the debug overlays.
    /// This is the `scene` pass of the root stage's recipe (see `render`): the Celestial
    /// Cycle owns the light and the framing, so this closure sets both, while the sky
    /// BEHIND it and the ground fog OVER it are the recipe's own passes, not lines in
    /// here. The sky still rides the lighting set here — `draw_sky` only raises a flag
    /// the encoder consumes, after every pass of the recipe has been applied.
    fn draw_world(&self, renderer: &mut Renderer) {
        renderer.set_camera(&Camera {
            position: self.position,
            target: self.position + self.forward(),
            up: Vec3::Y,
            fov_y_radians: 60.0_f32.to_radians(),
            near: 0.1,
            far: 10000.0,
            ortho_height: None,
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

        // POC grass: draw only the near-field instances (the "nearby LOD field"). Grass is a
        // close-up detail, and this bounds the per-frame draw count since there is no per-draw cull.
        // Colour is baked into each vertex's packed-RGB material, so no tint is needed.
        const GRASS_VIEW_RADIUS: f32 = 130.0; // voxels (~20 m)
        let eye = self.position;
        for (variant, model) in &self.grass_instances {
            if (model.w_axis.truncate() - eye).length_squared()
                > GRASS_VIEW_RADIUS * GRASS_VIEW_RADIUS
            {
                continue;
            }
            if let Some(handle) = self.grass_meshes.get(*variant) {
                renderer.draw_mesh(*handle, *model, MeshDrawOptions::default());
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

        // Virtual-voxel inspector — the 12-edge wireframe of the selected dual cell,
        // a world-space overlay. The per-corner numeric readout now lives in the
        // walker's inspector PANEL (`hud_model` publishes `insp_c*`, drawn by the HUD),
        // so this draws only the 3D outline. Darker than the bright-white cluster
        // bounding box so the dual cell reads as a distinct overlay, not a sub-box.
        if let Some(vv) = self.current_virtual_voxel() {
            let mut segments: Vec<(Vec3, Vec3)> = Vec::with_capacity(12);
            for &(o0, o1) in &CUBE_EDGES {
                segments.push((vv.corners[o0].world, vv.corners[o1].world));
            }
            renderer.draw_lines(&segments, [0.7, 0.7, 0.75, 1.0]);
        }
    }

    /// The ONE light-view-projection for the sun shadow: an orthographic caster box of
    /// half-size `extent` (world units) fitted around the field centre, from the sun's LIVE
    /// direction, for rig slot `light`. Called inside the producer surface's draw closure, so
    /// it runs only when the shadow depth actually re-renders (see `render`) — never per frame.
    fn sun_light_vp(&self, light: u32, extent: f32) -> Mat4 {
        let half = FIELD_DIM as f32 * 0.5 * CLUSTER_DIM as f32;
        let center = Vec3::new(half, CLUSTER_DIM as f32 * 0.5, half);
        let mut sun_rig = LightRig::default();
        let slot = (light as usize).min(sun_rig.lights.len() - 1);
        sun_rig.lights[slot].direction = Vec3::new(-0.4, -0.85, -0.35).normalize();
        sun_rig.shadow_view_proj(slot, center, extent)
    }

    /// The shadow CASTERS — just the terrain cluster meshes, drawn into the sun-shadow
    /// producer surface's depth from the light's view (the camera is the light's, set by
    /// `begin_shadow_view` before this runs). The SAME cluster-mesh loop `draw_world` uses
    /// for the lit field, minus everything a shadow does not cast: no sky, no wireframe/
    /// overlay gizmos, no bounding boxes, no celestial billboards. The colour is discarded;
    /// only the depth is sampled.
    fn draw_casters(&self, renderer: &mut Renderer) {
        for (id, handle) in &self.meshes {
            let o = id.world_offset();
            let model = Mat4::from_translation(Vec3::new(o[0], o[1], o[2]));
            renderer.draw_mesh(*handle, model, MeshDrawOptions::default());
        }
    }
}

/// The sun-shadow knobs `(light, bias, extent)`, each read from its ONE authority (the art
/// knobs live in DATA, tuned in the scene file, never in Rust): the `light` the shadow is cast
/// for and the sampling `bias` come from the CONSUMER `shadow_map` line in `world` (the world
/// recipe — where the room applies its shadow and darkens that light's term); the caster-box
/// `extent` comes from the PRODUCER `shadow_map` pass in `producer` (which fits the depth
/// render). `None` unless BOTH roles are present, which just leaves the world binding the
/// disabled default. This is the live channel the gate asserts the authored values against.
fn shadow_knobs(world: &StageDef, producer: &StageDef) -> Option<(u32, f32, f32)> {
    let (light, bias) = world.recipe().iter().find_map(|p| match &p.kind {
        PassKind::ShadowMap(s) if s.from.is_some() => Some((s.light, s.bias)),
        _ => None,
    })?;
    let extent = producer.recipe().iter().find_map(|p| match &p.kind {
        PassKind::ShadowMap(s) if s.from.is_none() => Some(s.extent),
        _ => None,
    })?;
    Some((light, bias, extent))
}

// `render_hud` now lives in `flicker-widgets` (the reusable UI surface) and is
// imported above; the call sites below are unchanged.

/// Package subdirectory holding the LOD-0 cluster bakes the scene loads: the
/// seeded WAVE-FIELD set (`bakes/`, written by `flicker-voxel`'s
/// `bake_island -- wave`) — the terrain the new contouring engine was proven
/// on. Pocclusters reads the island set (`bakes_island/`) instead; the two
/// sets never collide. NOTE the live-contour fallback in
/// [`GameScene::ensure_source`] builds the ISLAND, not this wave field.
const WAVE_BAKES: &str = "bakes";

/// Directory the scene reads baked clusters from on startup (contour-from-primitive
/// is the fallback when a bake is absent). Resolved through the content-roots
/// service so it works from any working directory; the bakes live in the shared
/// content tree.
fn bake_dir_path() -> std::path::PathBuf {
    flicker_core::roots::roots().package().join(WAVE_BAKES)
}

/// LOGICAL filename for a cluster bake — how loaders address it. The
/// on-disk file stores LOD-0 data only (LOD is a render concern) and
/// ships gz-at-rest as `<name>.json.gz` (the package-wide convention);
/// the shared seam resolves the logical name to whichever form is
/// present. The cluster's spatial address is the rest of the name —
/// `lod` is omitted because it's always `0` here.
fn bake_filename(x: u16, y: u16, z: u16) -> String {
    format!("cluster_{x}_{y}_{z}.json")
}

/// Try to load every cluster in `ids` from `dir`. Returns `Some(vec)`
/// only if **all** loads succeed; partial loads fall back to
/// contour-from-primitive (no point starting up with a half-baked
/// field). Reads are best-effort: any error path logs at warn level
/// and yields `None`. Resolution is the shared gz-at-rest seam
/// (`flicker_core::compression::read_bytes`): `<name>.json.gz` first,
/// loose `<name>.json` as the dev fallback.
fn try_load_bake_field(
    dir: &std::path::Path,
    ids: &[ClusterId],
) -> Option<Vec<(ClusterId, Cluster)>> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let path = dir.join(bake_filename(id.x(), id.y(), id.z()));
        let Ok(bytes) = flicker_core::compression::read_bytes(&path) else {
            return None; // no bake present → fall back to contour
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

/// The lowest walkable floor over every nav surface of the field. `None` until a nav has
/// arrived. Costs one sweep per field rebuild, never per frame.
fn nav_floor_min(navs: &[(ClusterId, ClusterNav)]) -> Option<f32> {
    let stride = CLUSTER_DIM as f32 / NAV_DIM as f32;
    let mut lowest: Option<f32> = None;
    for (id, nav) in navs {
        let base = id.world_offset()[1];
        for x in 0..NAV_DIM as u8 {
            for z in 0..NAV_DIM as u8 {
                if let Some(floor) = nav.floor_at(x, z) {
                    let h = base + floor as f32 * stride;
                    lowest = Some(lowest.map_or(h, |m| m.min(h)));
                }
            }
        }
    }
    lowest
}
