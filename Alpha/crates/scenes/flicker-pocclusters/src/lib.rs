//! flicker-pocclusters: the first Alpha client, focused on CSG editing and cluster
//! manipulation. Extracted from the `voxel-cluster` POC — same 3×3 cluster
//! field, contour/mesh pipeline, and LOD wiring — but with the celestial
//! day/night model dropped in favour of a single fixed studio light for the
//! whole world (see `world_lighting`), so the voxel geometry stays legible
//! while carving.
//!
//! Each cluster contours its own region against the shared global primitive;
//! meshing closes the four internal seams (and the interior cluster's all
//! four faces) via the low-side-owns convention in `flicker_voxel::mesh`.
//!
//! Pipeline: 3×3 `ClusterId`s → `contour` per cluster → `ClusterMap`
//! → per-cluster `NeighborContext` → `mesh` → upload one mesh handle
//! per cluster, drawn at its `world_offset()`. The cluster boundary is
//! drawn as a white wireframe box; two debug toggles let the user
//! inspect the meshes interactively (see controls below).
//!
//! Camera controls (rebindable via the `InputMap`):
//!   * WASD: move forward/back/strafe in the camera's facing.
//!   * R / F: rise / descend (world Y up / down).
//!   * Right-drag: free-look yaw + pitch.
//!   * Escape: open the pause menu (Resume / Quit).
//!
//! Debug toggles are driven by a DECLARATIVE component-tree HUD
//! (`Alpha/content/sensorium/scripts/shared/hud_pocclusters.lua`): the Lua declares the panel via
//! `M.tree()` (checkboxes + the move-speed / sensitivity sliders) and the
//! flicker-widgets Rust walker (`run_ui`) owns layout, hit-test, and draw. Six
//! clickable checkboxes replace the old `1`/`2`/`\` key handling:
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
//! A CSG-cluster POC PACKAGE (library only): the scene runs inside the unified
//! `prism-alpha` launcher (`cargo run -p prism-alpha`), which lists it in the
//! scene picker. This crate exposes a `scene()` factory and no longer builds a
//! standalone binary.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use flicker::net::chat::{ChatClient, ChatCommand, ChatEvent};
use flicker::render::{
    Camera, CompositeTarget, FrameGraph, LightRig, Mat4, MeshDrawOptions, MeshHandle, MeshIndices,
    MeshVertex, PassKind, RenderTargetHandle, Renderer, StageDef, StageInputs, TextureHandle, Vec2,
    Vec3,
};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, ScriptHost, UiNode, ValueMap};
use flicker::ui::{
    chat_panel, render_hud, run_ui, strings, ChatLineKind, ChatLineView, ChatView, RosterEntry,
    SceneDef, Section, Sections, UiInput, UiIntents, UiState, WalkerHandler,
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

mod celestial;
use celestial::CelestialState;

mod route;
use route::{CommandHandler, GameplayBase, RootHandler};

mod scatter;

/// Pack a linear RGB colour into the mesh shader's direct-RGB `material` word: the top bit marks
/// "literal RGB", the low three bytes are r,g,b (mirrors `shaders/mesh.wgsl` `material_color`).
/// Palette-independent flat colour for a static prop.
fn pack_rgb(linear: &[f32]) -> u32 {
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    let (r, g, b) = match linear {
        [r, g, b, ..] => (*r, *g, *b),
        _ => (0.12, 0.42, 0.15), // fallback green if the prop carried no colour
    };
    0x8000_0000 | (byte(b) << 16) | (byte(g) << 8) | byte(r)
}

#[cfg(test)]
mod grass_integration {
    //! Headless proof of the grass data path: the REAL promoted `GrassField` set + its prop meshes
    //! feed the scatter, and every instance lands above the authored waterline. Skips (does not
    //! fail) when the grass assets are not promoted in this tree, so it never blocks an unrelated
    //! checkout.
    use super::*;

    #[test]
    fn grass_set_scatters_above_the_waterline() {
        let props = flicker_content::roots().package().join("props/environment");
        let Ok(set) = flicker_content::PropSet::load(&props.join("GrassField/GrassField.set.json"))
        else {
            eprintln!("skip: GrassField not promoted in this tree");
            return;
        };
        let mut weights = Vec::new();
        for v in &set.variants {
            let mesh = flicker_skeletal::format::load_mesh(
                &props.join(&v.prop).join(format!("{}.json", v.prop)),
            )
            .expect("promoted grass variant loads");
            assert!(!mesh.vertices.is_empty(), "{} has geometry", v.prop);
            assert_eq!(
                mesh.materials.first().map(|m| m.color.len()),
                Some(3),
                "{} carries a flat colour (the POC bake)",
                v.prop
            );
            weights.push(v.weight);
        }

        let span = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        let params = scatter::ScatterParams {
            area_min: [0.0, 0.0],
            area_max: [span, span],
            spacing: 10.0,
            jitter: 0.7,
            sea_level: 120.0,
            shore_margin: 2.0,
            scale_min: 0.8,
            scale_max: 1.3,
            seed: 1,
        };
        let placements = scatter::scatter(&weights, &params, |x, z| {
            flicker_primitive::heightmap::island_height(x, z)
        });
        assert!(
            placements.len() > 200,
            "the island is populated ({} instances)",
            placements.len()
        );
        for pl in &placements {
            assert!(
                pl.pos[2] > params.sea_level + params.shore_margin,
                "instance above the waterline (y={})",
                pl.pos[2]
            );
        }
    }
}

/// Side length of the cluster field, in clusters. A 3×3 row in XZ
/// gives one fully-interior cluster (all four lateral neighbors
/// present), which is what actually exercises seam tangent stitching
/// on every face simultaneously.
const FIELD_DIM: u16 = 3;

/// The ROOT surface's stage source — `stages.pocclusters_world` in
/// `pocclusters.scene.json`. A nested `surface` node names its source in the tree, but
/// the walker skips the ROOT node, so the scene names its own root stage here: the one
/// spelling of the recipe that draws the world and the ground fog over it.
const WORLD_STAGE: &str = "pocclusters_world";

/// The SUN-SHADOW producer stage — `stages.pocclusters_sun_shadow` in
/// `pocclusters.scene.json`. Renders the cluster casters from the sun's view into a depth
/// map the ROOT surface's `shadow_map` consumer samples. Its `extent`/`bias`/`rate` are
/// authored knobs read out of the compiled stage — never spelled in Rust.
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

/// An in-flight drag of the floating chat window (scene-owned, since the walker
/// has no window move/resize — its geometry is static). `Move` remembers the
/// grab offset from the window's top-left so the window tracks the cursor.
#[derive(Copy, Clone, PartialEq)]
enum ChatDrag {
    None,
    Move { grab: Vec2 },
    Resize,
}

// Chat-window hit regions + minimum size (device px). The grip is the top strip
// that drags the window; the corner box resizes it. These are hit rects, so they
// only need to roughly cover the drawn title bar / `◢` handle.
const CHAT_GRIP_H: f32 = 34.0;
const CHAT_CORNER: f32 = 22.0;
const CHAT_MIN_W: f32 = 420.0;
const CHAT_MIN_H: f32 = 180.0;

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

    /// POC grass scatter (kept out of `meshes` so the LOD re-mesh never frees it): one uploaded
    /// mesh handle per GrassField variant, plus the precomputed per-instance `(variant index,
    /// model matrix)`. Empty when the promoted grass assets are absent.
    grass_meshes: Vec<MeshHandle>,
    grass_instances: Vec<(usize, Mat4)>,

    /// First-person camera state.
    position: Vec3,
    yaw: f32,
    pitch: f32,

    /// The from-Home sky (sun/moon/planets/constellations), driven by the Celestial
    /// Cycle panel. The body layout is the shared `flicker_orrery` roster.
    celestial: CelestialState,
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
    /// Soft white disc texture for the planet billboards, uploaded once in `enter`.
    planet_disc: Option<TextureHandle>,
    /// Star glow sprite (core + halo + glint) for the constellation stars.
    star_tex: Option<TextureHandle>,

    /// Mouse-look tuning (sensitivity + invert) from the shell settings panel +
    /// the scene-owned `move_speed` (the HUD slider). The action MAPS live with
    /// the pump now — the scene resolves nothing itself.
    controls: AbstractControls,
    /// Pad tuning handed to the pause overlay; the pump owns the live config.
    gamepad_config: GamepadConfig,
    /// The pair script (`pocclusters.lua`) — derives the HUD display strings
    /// from the raw Model each frame (five-line split). `None` if it failed to
    /// load; the HUD then shows raw-less readouts.
    script: Option<ScriptHost>,
    /// Chat owns the keyboard — the scene-owned context truth the runner reads
    /// through [`Scene::input_context`] (TextEntry while set, World otherwise).
    /// Set by the command handler's one-way hand-off, cleared on submit/cancel.
    chat_focused: bool,

    /// The in-scene HUD as a DECLARATIVE component tree, parsed ONCE from
    /// `hud_pocclusters.lua`'s `tree()` at construction (the walker redraws this
    /// cached tree every frame with fresh Model bindings). `None` if the script
    /// failed to load — the scene still runs without a HUD.
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
    /// Set at construction (walk is the default) and on a fly→walk toggle:
    /// the camera snaps down onto the surface beneath it on the first frame
    /// the nav under it is available (nav is generated asynchronously, so the
    /// snap waits for it).
    walk_needs_snap: bool,

    /// The exclusive-TextEntry chat handler (owns the T hand-off + the key
    /// guard). Events + the route scratch arrive from the PUMP via `SceneInput`
    /// — the private resolver/tick rig deleted with the migration (P6).
    command: CommandHandler,

    // ── In-world chat (clay-chat client; DesignSync ChatPanel over clay-chat v0.1) ──
    /// The clay-chat client (background socket thread). `None` until `enter` connects;
    /// dropped in `exit` to disconnect. Inbound events drained each frame like `build_rx`.
    chat: Option<ChatClient>,
    /// Retained walker state for the chat pass (keyboard focus), separate from the HUD's.
    chat_ui_state: UiState,
    /// This frame's chat draw commands — a SECOND `run_ui` pass over the floating
    /// panel, blitted after the HUD in `render` (so it layers on top).
    chat_commands: Vec<HudCommand>,
    /// The floating window's rect `(x, y, w, h)` in device px — scene-owned so it can
    /// move/resize (walker geometry is static). Seeded bottom-centre in `enter`.
    chat_rect: (f32, f32, f32, f32),
    /// In-flight title-drag (move) / corner-drag (resize), or `None`.
    chat_drag: ChatDrag,
    /// The local nick (updated from the server's `NickAck` / `Renamed`).
    chat_nick: String,
    /// Joined channels (wire form, e.g. `"#general"`) — one tab each.
    chat_active: String,
    chat_channels: Vec<String>,
    /// Per-channel scrollback + roster, built from decoded events.
    chat_logs: HashMap<String, Vec<ChatLineView>>,
    chat_rosters: HashMap<String, Vec<RosterEntry>>,
    /// The input field's current text (mirrors the walker `chat_input` bind).
    chat_input: String,
    /// The active log's scroll offset (`f32::MAX` = follow newest).
    chat_scroll: f32,
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
            chat_focused: false,
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
            // Walk is the DEFAULT locomotion for the Prism Test Room (Aaron
            // 2026-08-22): boot walkable with the navmesh shown, spawned at the
            // field centre (`position` above) and snapped onto the surface on
            // the first Active frame (`walk_needs_snap`).
            locomotion_walk: true,
            lod_field: [[0u8; FIELD_DIM as usize]; FIELD_DIM as usize],
            digit_atlas: None,
            planet_disc: None,
            star_tex: None,
            celestial: CelestialState::default(),
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
            command: CommandHandler::default(),
            chat: None,
            chat_ui_state: UiState::new(),
            chat_commands: Vec::new(),
            chat_rect: (0.0, 0.0, 0.0, 0.0),
            chat_drag: ChatDrag::None,
            chat_nick: String::new(),
            chat_active: String::new(),
            chat_channels: Vec::new(),
            chat_logs: HashMap::new(),
            chat_rosters: HashMap::new(),
            chat_input: String::new(),
            chat_scroll: f32::MAX,
            ui_theme: None,
            phase: GamePhase::Booting,
            nav_ready_target: 0,
        }
    }
}

/// The pair script (`content/sensorium/scripts/pocclusters.lua`) — embedded at
/// compile time like every migrated scene's; `derive()` turns the raw Model
/// into the HUD display strings.
const POCCLUSTERS_SCRIPT: &str =
    include_str!("../../../../content/sensorium/scripts/pocclusters.lua");

impl GameScene {
    /// Build the game scene off the manifest's def (the five-line split): the
    /// authored HUD tree + the folded styles come from `pocclusters.scene.json`,
    /// the pair script derives the display strings. Other state takes its
    /// placeholder values from [`Default`]; the world + camera come up in
    /// [`Scene::enter`].
    fn new(def: &SceneDef) -> Self {
        let ui_styles = flicker::ui::load_shared_styles(def.styles.as_ref());
        let ui_tree = def.tree.clone();
        if ui_tree.is_none() {
            tracing::error!("pocclusters scene file has no `tree` — no HUD");
        }
        // The screen's declarative bindings (S9), read off the authored root once —
        // cached exactly like the tree they were collected from.
        let ui_intents = ui_tree.as_ref().map(UiIntents::of).unwrap_or_default();
        let script = match ScriptHost::new(POCCLUSTERS_SCRIPT, "pocclusters.lua") {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("pocclusters.lua failed to load — raw HUD values only: {e}");
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
            // The Celestial Cycle's Fog control, over its own default: one control dims
            // the distance fog and the ground slab together.
            .set("fog_density", self.celestial.fog / celestial::DEFAULT_FOG)
            .set("fog_time", self.fog_time)
            // GOLDEN HOUR: how far the tonemap resolve lerps toward the recipe's authored
            // golden tint, computed from the SAME sun the cycle lights the world with — 0 at
            // noon and at night, peaking as the sun sits on the horizon. The recipe binds its
            // `grade_strength` to this, so the grade follows the sky with no second clock.
            .set("grade_warmth", self.celestial.grade_warmth());
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
            // Fall back to the SAME island the bakes were contoured from
            // (`HeightField::island`), so a missing bake reproduces the island
            // terrain rather than the old wave-field-plus-gallery world.
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
    /// Post the current input line. A leading `/` is a client command
    /// (`/join /part /leave /nick /names`); anything else — including `/me`, which
    /// round-trips as a plain message and renders as an emote by the `/me ` convention —
    /// is a `MSG` to the active channel. Clears the input.
    fn send_chat_input(&mut self) {
        let text = std::mem::take(&mut self.chat_input).trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(client) = self.chat.as_ref() else {
            return;
        };
        if let Some(rest) = text.strip_prefix('/') {
            let mut it = rest.splitn(2, ' ');
            let verb = it.next().unwrap_or("").to_ascii_lowercase();
            let arg = it.next().unwrap_or("").trim().to_string();
            match verb.as_str() {
                "join" | "j" => {
                    if !arg.is_empty() {
                        client.send(ChatCommand::Join(arg));
                    }
                }
                "part" | "leave" => {
                    let channel = if arg.is_empty() {
                        self.chat_active.clone()
                    } else {
                        arg
                    };
                    client.send(ChatCommand::Part(channel));
                }
                "nick" => {
                    if !arg.is_empty() {
                        client.send(ChatCommand::Nick(arg));
                    }
                }
                "names" => client.send(ChatCommand::Names(self.chat_active.clone())),
                _ => client.send(ChatCommand::Msg {
                    channel: self.chat_active.clone(),
                    text,
                }),
            }
        } else {
            client.send(ChatCommand::Msg {
                channel: self.chat_active.clone(),
                text,
            });
        }
    }

    /// Append a line to a channel's scrollback (ring-capped), auto-following the
    /// active channel to newest.
    fn push_chat_line(&mut self, channel: &str, kind: ChatLineKind, text: String) {
        const CAP: usize = 300;
        let log = self.chat_logs.entry(channel.to_string()).or_default();
        log.push(ChatLineView { kind, text });
        if log.len() > CAP {
            let drop = log.len() - CAP;
            log.drain(0..drop);
        }
        if channel == self.chat_active {
            self.chat_scroll = f32::MAX;
        }
    }

    fn roster_add(&mut self, channel: &str, nick: &str, my_nick: &str) {
        let roster = self.chat_rosters.entry(channel.to_string()).or_default();
        if !roster.iter().any(|m| m.label == nick) {
            roster.push(RosterEntry {
                label: nick.to_string(),
                op: false,
                you: nick == my_nick,
            });
        }
    }

    fn roster_remove(&mut self, channel: &str, nick: &str) {
        if let Some(roster) = self.chat_rosters.get_mut(channel) {
            roster.retain(|m| m.label != nick);
        }
    }

    /// Fold one decoded [`ChatEvent`] into the per-channel logs + rosters (+ our own
    /// nick / active channel on the events that change them).
    fn apply_event(&mut self, ev: ChatEvent, my_nick: &str) {
        match ev {
            ChatEvent::Connected => {
                let active = self.chat_active.clone();
                self.push_chat_line(
                    &active,
                    ChatLineKind::Joined,
                    format!("· {}", strings::resolve("$pc_chat_connected")),
                );
            }
            ChatEvent::Disconnected(reason) => {
                let active = self.chat_active.clone();
                let msg = match reason {
                    Some(r) => format!("· {} — {r}", strings::resolve("$pc_chat_disconnected")),
                    None => format!("· {}", strings::resolve("$pc_chat_disconnected")),
                };
                self.push_chat_line(&active, ChatLineKind::Left, msg);
            }
            ChatEvent::Chat {
                channel,
                from,
                text,
            } => {
                let (kind, line) = if let Some(rest) = text.strip_prefix("/me ") {
                    (ChatLineKind::Emote, format!("✦ {from} {rest}"))
                } else if from == my_nick {
                    (ChatLineKind::You, format!("{from}   {text}"))
                } else {
                    (ChatLineKind::Say, format!("{from}   {text}"))
                };
                self.push_chat_line(&channel, kind, line);
            }
            ChatEvent::Joined { nick, channel } => {
                self.roster_add(&channel, &nick, my_nick);
                self.push_chat_line(
                    &channel,
                    ChatLineKind::Joined,
                    format!("◈ {nick} {}", strings::resolve("$pc_chat_joined")),
                );
                if nick == my_nick {
                    // Our own JOIN success: ensure the tab exists (+ seed the roster for
                    // a channel we joined mid-session) and switch to it.
                    if !self.chat_channels.iter().any(|c| c == &channel) {
                        self.chat_channels.push(channel.clone());
                        if let Some(c) = self.chat.as_ref() {
                            c.send(ChatCommand::Names(channel.clone()));
                        }
                    }
                    self.chat_active = channel;
                    self.chat_scroll = f32::MAX;
                }
            }
            ChatEvent::Parted { nick, channel } => {
                self.roster_remove(&channel, &nick);
                self.push_chat_line(
                    &channel,
                    ChatLineKind::Left,
                    format!("◌ {nick} {}", strings::resolve("$pc_chat_left")),
                );
                if nick == my_nick {
                    self.chat_channels.retain(|c| c != &channel);
                    self.chat_logs.remove(&channel);
                    self.chat_rosters.remove(&channel);
                    if self.chat_active == channel {
                        self.chat_active = self.chat_channels.first().cloned().unwrap_or_default();
                        self.chat_scroll = f32::MAX;
                    }
                }
            }
            ChatEvent::Renamed { old, new } => {
                if old == self.chat_nick {
                    self.chat_nick = new.clone();
                }
                let you = new == self.chat_nick;
                let mut touched: Vec<String> = Vec::new();
                for (channel, roster) in self.chat_rosters.iter_mut() {
                    let mut hit = false;
                    for member in roster.iter_mut() {
                        if member.label == old {
                            member.label = new.clone();
                            member.you = you;
                            hit = true;
                        }
                    }
                    if hit {
                        touched.push(channel.clone());
                    }
                }
                for channel in touched {
                    self.push_chat_line(
                        &channel,
                        ChatLineKind::Renamed,
                        format!("ᛥ {old} {} {new}", strings::resolve("$pc_chat_is_now")),
                    );
                }
            }
            ChatEvent::Names { channel, names } => {
                let roster = names
                    .into_iter()
                    .map(|label| RosterEntry {
                        you: label == my_nick,
                        op: false,
                        label,
                    })
                    .collect();
                self.chat_rosters.insert(channel, roster);
            }
            ChatEvent::NickAck(nick) => {
                self.chat_nick = nick.clone();
                let active = self.chat_active.clone();
                self.push_chat_line(
                    &active,
                    ChatLineKind::Joined,
                    format!("· {} '{nick}'", strings::resolve("$pc_chat_you_are_now")),
                );
            }
            ChatEvent::Notice(text) => {
                let active = self.chat_active.clone();
                self.push_chat_line(&active, ChatLineKind::Left, format!("· {text}"));
            }
            ChatEvent::Error(text) => {
                let active = self.chat_active.clone();
                self.push_chat_line(&active, ChatLineKind::Op, format!("⚠ {text}"));
            }
            ChatEvent::Channels(_) | ChatEvent::Pong(_) => {}
        }
    }

    fn hud_model(&self) -> ValueMap {
        // The ENGINE publishes RAW runtime variables + RESOLVED WORD tokens
        // (localization stays stringtable-resolved engine-side); the PAIR SCRIPT
        // (pocclusters.lua) derives the display strings — the five-line split.
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
            .with("w_corners", strings::resolve("$pc_corners").as_ref())
            // Celestial Cycle: seven two-way NUMBER binds (celestial math units) +
            // their pre-formatted readouts + three view toggles. Each readout is
            // ONE formatted value (clock / phase / month names), so `celestial`
            // keeps the formatting.
            .with("cc_sun", self.celestial.time_of_day)
            .with(
                "cc_sun_val",
                celestial::fmt_clock(self.celestial.time_of_day),
            )
            .with("cc_moon", self.celestial.moon_phase)
            .with(
                "cc_moon_val",
                celestial::fmt_moon(self.celestial.moon_phase),
            )
            .with("cc_year", self.celestial.year_month)
            .with(
                "cc_year_val",
                celestial::fmt_month(self.celestial.year_month),
            )
            .with("cc_speed", self.celestial.sim_speed)
            .with(
                "cc_speed_val",
                celestial::fmt_speed(self.celestial.sim_speed),
            )
            .with("cc_fog", self.celestial.fog)
            .with("cc_fog_val", celestial::fmt_fog(self.celestial.fog))
            .with("cc_lat", self.celestial.latitude)
            .with("cc_lat_val", celestial::fmt_lat(self.celestial.latitude))
            .with("cc_epoch", self.celestial.epoch)
            .with("cc_epoch_val", celestial::fmt_epoch(self.celestial.epoch))
            .with("constellations", self.celestial.show_constellations)
            .with("planets", self.celestial.show_planets)
            .with("celestial_paths", self.celestial.show_paths);

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
                tracing::error!("pocclusters: publishing raw vars failed: {e}");
            }
            match script.derive() {
                Ok(Some(derived)) => {
                    for (k, v) in derived.entries() {
                        m.set(k.clone(), v.clone());
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::error!("pocclusters.lua derive() failed: {e}"),
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
        self.position = Vec3::new(center_x, CLUSTER_DIM as f32 * 0.75, center_x);
        self.yaw = 0.0; // face +Z, across the field.
        self.pitch = -0.55; // angled slightly down.

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

        // POC grass scatter: load the promoted GrassField variation set + its prop meshes ONCE,
        // upload each, then scatter instances across the island ABOVE the waterline. Fail-soft — a
        // missing set (e.g. a tree without the props promoted) just renders the world without grass.
        {
            const CM_TO_VOXEL: f32 = 1.0 / 15.24; // 1 voxel = 6 in = 15.24 cm (Y-up voxel world)
            const SEA_LEVEL: f32 = 120.0; // authored flood height in voxels (pocclusters.scene.json)
            let props = flicker_content::roots().package().join("props/environment");
            if let Ok(set) =
                flicker_content::PropSet::load(&props.join("GrassField/GrassField.set.json"))
            {
                let mut weights = Vec::new();
                for v in &set.variants {
                    let mesh_path = props.join(&v.prop).join(format!("{}.json", v.prop));
                    match flicker_skeletal::format::load_mesh(&mesh_path) {
                        Ok(mesh) => {
                            // Flat colour → packed direct-RGB per vertex (palette-independent). The
                            // colour is the FBX base colour the prop POC baked into the material.
                            let material = pack_rgb(
                                mesh.materials
                                    .first()
                                    .map(|m| m.color.as_slice())
                                    .unwrap_or(&[]),
                            );
                            let verts: Vec<MeshVertex> = mesh
                                .vertices
                                .iter()
                                .map(|vx| MeshVertex {
                                    position: vx.p,
                                    normal: vx.n,
                                    material,
                                })
                                .collect();
                            let handle =
                                renderer.upload_mesh(&verts, MeshIndices::U32(&mesh.indices));
                            self.grass_meshes.push(handle);
                            weights.push(v.weight);
                        }
                        Err(e) => tracing::warn!("grass variant {} failed to load: {e}", v.prop),
                    }
                }
                if !self.grass_meshes.is_empty() {
                    // The island spans X,Z in [0, FIELD_DIM*CLUSTER_DIM) voxels.
                    let span = FIELD_DIM as f32 * CLUSTER_DIM as f32;
                    let params = scatter::ScatterParams {
                        area_min: [0.0, 0.0],
                        area_max: [span, span],
                        spacing: 10.0,
                        jitter: 0.7,
                        sea_level: SEA_LEVEL,
                        shore_margin: 2.0,
                        scale_min: 0.8,
                        scale_max: 1.3,
                        seed: 1,
                    };
                    let placements = scatter::scatter(&weights, &params, |x, z| {
                        flicker_primitive::heightmap::island_height(x, z)
                    });
                    self.grass_instances = placements
                        .iter()
                        .map(|pl| {
                            // scatter pos = [X, Z, height]; the world is Y-up, so Y = height.
                            let world = Vec3::new(pl.pos[0], pl.pos[2], pl.pos[1]);
                            let model = Mat4::from_translation(world)
                                * Mat4::from_rotation_y(pl.yaw)
                                * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2) // Z-up → Y-up
                                * Mat4::from_scale(Vec3::splat(CM_TO_VOXEL * pl.scale));
                            (pl.variant, model)
                        })
                        .collect();
                    tracing::info!(
                        "grass: {} variants, {} instances above y={SEA_LEVEL}",
                        self.grass_meshes.len(),
                        self.grass_instances.len()
                    );
                }
            }
        }

        // 1×1 white pixel — tinted to build solid colored HUD quads.
        // Retained sprite-UI capability; no active widgets yet.
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));

        // Digit-glyph atlas for the LOD billboards (digits 0–7).
        self.digit_atlas =
            Some(renderer.load_texture(&build_digit_atlas(), ATLAS_W as u32, ATLAS_H as u32));

        // Soft white disc for the planet billboards riding the sky dome.
        let (disc, dw, dh) = celestial::build_disc_texture();
        self.planet_disc = Some(renderer.load_texture(&disc, dw, dh));

        // Star glow sprite (core + halo + glint) for the constellation stars.
        let (glow, gw, gh) = celestial::build_star_glow_texture();
        self.star_tex = Some(renderer.load_texture(&glow, gw, gh));

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

        // Connect the in-world chat to the clay-chat server (background socket
        // thread; inbound events are drained each Active frame). Optimistically show
        // #general until the server's own join echo confirms it, and seed the roster
        // with a NAMES. If the server is down the client just reports Disconnected.
        let nick = default_chat_nick();
        self.chat_nick = nick.clone();
        self.chat_active = "#general".to_string();
        self.chat_channels = vec!["#general".to_string()];
        let client = ChatClient::connect(nick);
        client.send(ChatCommand::Join("#general".to_string()));
        client.send(ChatCommand::Names("#general".to_string()));
        self.chat = Some(client);

        // Float the window bottom-centre, ~3/5 of the screen wide (wide, not docked).
        let screen = renderer.size();
        let w = (screen.x * 0.6).clamp(CHAT_MIN_W, (screen.x - 40.0).max(CHAT_MIN_W));
        let h = (screen.y * 0.42).clamp(CHAT_MIN_H, (screen.y - 40.0).max(CHAT_MIN_H));
        let x = ((screen.x - w) * 0.5).max(0.0);
        let y = (screen.y - h - 24.0).max(0.0);
        self.chat_rect = (x, y, w, h);
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

        // Advance the celestial clock (sun/moon/planets/epoch) by the panel's Speed;
        // paused when Speed is 0. Drives the day/night sky in `render`.
        self.celestial.update(dt);

        // ── The input seam (spec §5/§9): ONE resolve + ONE dispatch replaces the old
        // `chat_focus` / T-Esc-click hand-off + `menu_prev` edge + `hud_hit`/`chat_hit` +
        // `active()==World` gate ladder. ──
        //
        // The active CONTEXT is the durable "chat owns the keyboard" truth: `TextEntry`
        // sits on the stack exactly while the chat line is focused, so gameplay
        // movement/look/pick suppress automatically (its map is empty). That is what the
        // deleted `chat_focus` bool used to track.
        let screen = renderer.size();
        let focused = self.chat_focused;

        // Chat window move/resize (scene-owned window management, NOT input arbitration):
        // update the rect and report whether a left-press landed in the panel this frame
        // (a click-to-enter). The focus/context change itself is the command handler's
        // job, just below.
        let mut click_focus = false;
        {
            let (mut cx, mut cy, mut cw, mut ch) = self.chat_rect;
            let m = input.mouse_position;
            if input.mouse_left_pressed {
                if in_rect(
                    m,
                    cx + cw - CHAT_CORNER,
                    cy + ch - CHAT_CORNER,
                    CHAT_CORNER,
                    CHAT_CORNER,
                ) {
                    self.chat_drag = ChatDrag::Resize;
                    click_focus = true;
                } else if in_rect(m, cx, cy, cw, CHAT_GRIP_H) {
                    self.chat_drag = ChatDrag::Move {
                        grab: Vec2::new(m.x - cx, m.y - cy),
                    };
                    click_focus = true;
                } else if in_rect(m, cx, cy, cw, ch) {
                    click_focus = true;
                }
            }
            if input.mouse_left {
                match self.chat_drag {
                    ChatDrag::Move { grab } => {
                        cx = (m.x - grab.x).clamp(0.0, (screen.x - cw).max(0.0));
                        cy = (m.y - grab.y).clamp(0.0, (screen.y - ch).max(0.0));
                    }
                    ChatDrag::Resize => {
                        cw = (m.x - cx).clamp(CHAT_MIN_W, (screen.x - cx).max(CHAT_MIN_W));
                        ch = (m.y - cy).clamp(CHAT_MIN_H, (screen.y - cy).max(CHAT_MIN_H));
                    }
                    ChatDrag::None => {}
                }
            } else {
                self.chat_drag = ChatDrag::None;
            }
            self.chat_rect = (cx, cy, cw, ch);
        }

        // Exclusive TextEntry keyboard owner: the T hand-off / Esc-cancel / Enter-submit
        // state machine + the trigger-key guard (4B15929B), promoted out of the scene into
        // `CommandHandler`. Runs before the chat pass so `guard` gates this frame's
        // typed(); its TextEntry-context + focus intents go into the reused RouteCtx and
        // are reconciled after dispatch.
        let text = self
            .command
            .drive(input, focused, click_focus, signals.route);
        if text.entered {
            self.chat_focused = true;
        }
        let guard = self.command.guard();

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
                typed: String::new(),
                backspace: false,
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

            // Celestial Cycle controls (two-way): the three view toggles + the seven
            // sliders. The walker echoes the model value when not dragging, so these
            // are idempotent against the clock that auto-advanced earlier this frame;
            // a drag scrubs it.
            self.celestial.show_constellations = results.is_on("constellations");
            self.celestial.show_planets = results.is_on("planets");
            self.celestial.show_paths = results.is_on("celestial_paths");
            if let Some(v) = results.number("cc_sun") {
                self.celestial.time_of_day = v as f32;
            }
            if let Some(v) = results.number("cc_moon") {
                self.celestial.moon_phase = v as f32;
            }
            if let Some(v) = results.number("cc_year") {
                self.celestial.year_month = v as f32;
            }
            if let Some(v) = results.number("cc_speed") {
                self.celestial.sim_speed = v as f32;
            }
            if let Some(v) = results.number("cc_fog") {
                self.celestial.fog = v as f32;
            }
            if let Some(v) = results.number("cc_lat") {
                self.celestial.latitude = v as f32;
            }
            if let Some(v) = results.number("cc_epoch") {
                self.celestial.epoch = v as f32;
            }

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

        // ── Chat panel: drain the socket into the per-channel logs/rosters, then
        // build + run the floating window as a SECOND walker pass (its own UiState)
        // so it can move/resize each frame; its commands layer over the HUD in
        // `render`. Reports `chat_hit` (pointer over the panel → the walker consumes
        // the click) and this frame's send/join/part button edges (acted on after
        // dispatch). ──
        let (chat_hit, chat_send, chat_join, chat_part) = {
            let my_nick = self.chat_nick.clone();
            while let Some(ev) = self.chat.as_ref().and_then(|c| c.try_recv()) {
                self.apply_event(ev, &my_nick);
            }

            // Re-assert the chat field's focus each frame (run_ui clears focus on any
            // clicked frame) from the context-derived truth (replaces `chat_focus`).
            if focused {
                self.chat_ui_state.request_focus("chat_input");
            } else {
                self.chat_ui_state.clear_focus();
            }

            let (cx, cy, cw, ch) = self.chat_rect;
            let empty_lines: Vec<ChatLineView> = Vec::new();
            let empty_roster: Vec<RosterEntry> = Vec::new();
            let lines = self
                .chat_logs
                .get(&self.chat_active)
                .unwrap_or(&empty_lines);
            let roster = self
                .chat_rosters
                .get(&self.chat_active)
                .unwrap_or(&empty_roster);
            let mut tree = chat_panel(
                cx,
                cy,
                cw,
                ch,
                &ChatView {
                    style: "pocclusters.chat",
                    active: &self.chat_active,
                    channels: &self.chat_channels,
                    lines,
                    roster,
                    nick: &self.chat_nick,
                    you_label: &strings::resolve("$pc_chat_you"),
                },
            );
            // The floating window is a DECLARED surface of this screen: its root
            // rides the `chat` gate (always on today), so hiding it is a helper
            // call — not a bespoke code path — once something wants to (S9).
            tree.visible_bind = Some("chat".into());

            let mut cmodel = ValueMap::new();
            self.surfaces.publish(&mut cmodel);
            // The tab strip selects by INDEX (an index is a number, everywhere), so the
            // scene publishes the active channel's position in `chat_channels`.
            let active_idx = self
                .chat_channels
                .iter()
                .position(|c| c == &self.chat_active)
                .unwrap_or(0);
            cmodel.set("chat_tab", active_idx as f64);
            cmodel.set("chat_scroll", self.chat_scroll as f64);
            cmodel.set("chat_input", self.chat_input.as_str());

            let cin = UiInput {
                mouse: input.mouse_position,
                // Suppress the walker click while a title/corner drag is in flight, so
                // a drag never also toggles a tab or button under the cursor.
                clicked: input.mouse_left_pressed && matches!(self.chat_drag, ChatDrag::None),
                down: input.mouse_left,
                right_down: input.mouse_right,
                screen,
                // Route typed text / backspace to the field only while it owns the
                // keyboard (TextEntry) and the trigger-key guard is clear (4B15929B) —
                // the promoted `chat_focus && !chat_key_guard`.
                typed: if focused && !guard {
                    input.typed().to_string()
                } else {
                    String::new()
                },
                backspace: focused && !guard && input.backspace(),
                wheel: input.mouse_wheel_delta,
                exclusive: false,
                motion: Default::default(),
            };
            let cframe = run_ui(
                &tree,
                &cmodel,
                &self.ui_styles,
                &cin,
                &mut self.chat_ui_state,
            );
            let chat_hit = cframe.results.is_on("hud_hit");
            self.chat_commands = cframe.commands;

            if let Some(t) = cframe.results.text("chat_input") {
                self.chat_input = t.to_string();
            }
            if let Some(s) = cframe.results.number("chat_scroll") {
                self.chat_scroll = s as f32;
            }
            if let Some(sel) = cframe.results.number("chat_tab") {
                if let Some(channel) = self.chat_channels.get(sel as usize) {
                    if channel != &self.chat_active {
                        self.chat_active = channel.clone();
                        self.chat_scroll = f32::MAX;
                    }
                }
            }

            (
                chat_hit,
                cframe.results.is_on("chat_send"),
                cframe.results.is_on("chat_join"),
                cframe.results.is_on("chat_part"),
            )
        };

        // ── Dispatch the PUMP's resolved events through the 4-handler chain
        // (root → command → walker → gameplay). The runner resolved this frame
        // over the scene's declared context (`input_context()`), so while chat
        // owns the keyboard the TextEntry map resolves nothing at all. ──
        let mut root = RootHandler;
        let mut gameplay = GameplayBase::default();
        // The walker layer wraps the CHAT walker's retained focus (it owns the
        // `chat_input` field) + this frame's pointer-consume = HUD `hud_hit` OR chat
        // `chat_hit` (the old two-gate fall-through, folded into the one walker
        // layer) + the screen's DECLARED intents (S9: `on_menu = "pause_open"`).
        self.fired_sigs.clear(); // last frame's mirror rode the HUD walk above — done
        let mut walker = WalkerHandler::hud(&mut self.chat_ui_state, hud_hit || chat_hit)
            .with_intents(&self.ui_intents);
        {
            let mut chain: [&mut dyn InputHandler; 4] =
                [&mut root, &mut self.command, &mut walker, &mut gameplay];
            Router::dispatch(signals.events, &mut chain, signals.route);
        }

        // Any exit leaves TextEntry (Enter/send = submit, Esc = cancel): pop the context +
        // clear focus through the router queue. The panel's send button folds into submit.
        let submit = text.submit || chat_send;
        if submit || text.cancel {
            signals.route.pop_context();
            signals.route.clear_focus();
            self.chat_focused = false;
        }

        // Surface context wiring (S9): any declared-surface flip since last frame
        // becomes Push/PopContext on the same queue (no pocclusters surface
        // carries a context today — the seam is standard, the call a live no-op).
        // The RUNNER applies the queued requests to the pump after `update`; the
        // chat field's walker focus is re-asserted per-frame from `chat_focused`.
        self.surfaces.apply_section_contexts(signals.route);
        // The screen's fired intents (S9), drained once per frame: acted on below
        // and queued for the one-frame `sig_<name>` Model mirror.
        self.fired_sigs = walker.take_fired();

        // Chat side effects (post the line / join / leave) — identical to the old inline
        // handling, now driven by the command handler's submit + the panel buttons.
        if submit {
            self.send_chat_input();
        }
        if chat_join {
            let channel = self.chat_input.trim().to_string();
            if !channel.is_empty() {
                if let Some(c) = self.chat.as_ref() {
                    c.send(ChatCommand::Join(channel));
                }
                self.chat_input.clear();
            }
        }
        if chat_part {
            if let Some(c) = self.chat.as_ref() {
                c.send(ChatCommand::Part(self.chat_active.clone()));
            }
        }

        // The screen DECLARED `on_menu = "pause_open"` (S9): the walker layer
        // consumed the Menu press and fired the name; the scene maps it onto its
        // pause push — the root's hardcoded Menu arm is gone. Under TextEntry the
        // guard is doubly structural: the empty map resolves no Menu at all, and
        // the command layer's capture sits above the walker anyway.
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
        if !self.chat_focused {
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
        self.chat_focused.then_some(InputContext::TextEntry)
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
        // `run_ui`), then the floating chat window over it — the screen surface's final
        // 2D, as one overlay run after the composite. Rects + text only (no engine
        // textures), so `white` — the 1×1 fill pixel — is the entire texture table.
        if let Some(white) = me.white {
            let hud_commands = &me.hud_commands;
            let chat_commands = &me.chat_commands;
            fg.overlay(move |r| {
                render_hud(r, hud_commands, white, &[]);
                render_hud(r, chat_commands, white, &[]);
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
        // Drop the client → its background socket thread winds down (see `ChatClient`).
        self.chat = None;
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

        // The from-Home sky COMPOSED OVER the stage's own rig, never replacing it: the
        // frame graph has already applied `stages.pocclusters_world`'s `hearth` preset
        // — DRIVEN, so the fire's flicker gain for this frame is baked into its
        // intensity — and `over` overwrites only the two directional slots (sun/moon +
        // eclipse) plus ambient / sky palette / fog / star rotation, leaving the room's
        // hearth standing. The recipe's `sky` pass renders the gradient, sun/moon discs,
        // Milky-Way "galactic cloud" and the eclipse corona from this same `LightRig`.
        renderer.set_scene(self.celestial.over(renderer.scene_lighting()));
        // The seven worlds on the ecliptic (geocentric, from the shared roster), the
        // ecliptic track, and the night constellations (the Chalice + placeholders).
        self.celestial
            .draw(renderer, self.position, self.planet_disc, self.star_tex);

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
        sun_rig.lights[slot].direction = self.celestial.sun_dir();
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

/// Point-in-rect test in device px (top-left origin).
fn in_rect(p: Vec2, x: f32, y: f32, w: f32, h: f32) -> bool {
    p.x >= x && p.x < x + w && p.y >= y && p.y < y + h
}

/// The chat nick for this client — the OS user name (the client codec sanitizes it),
/// else a default. There is no auth/registration at this stage; the web side owns
/// identity later, so this is just a friendly label for the raw-protocol test.
fn default_chat_nick() -> String {
    std::env::var("USER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "prism".to_string())
}

/// Package subdirectory holding the LOD-0 cluster bakes the scene loads. The
/// island set (`bakes_island/`, written by `flicker-voxel`'s `bake_island`
/// bin) — NOT the old wave-field `bakes/`, which stays on disk untouched. The
/// live-contour fallback in [`GameScene::ensure_source`] builds the same
/// island, so a missing bake reproduces the same terrain.
const ISLAND_BAKES: &str = "bakes_island";

/// Directory the scene reads baked clusters from on startup (contour-from-primitive
/// is the fallback when a bake is absent). Resolved through the content-roots
/// service so it works from any working directory; the bakes live in the shared
/// content tree.
fn bake_dir_path() -> std::path::PathBuf {
    flicker_core::roots::roots().package().join(ISLAND_BAKES)
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

#[cfg(test)]
mod script_smoke {
    //! Load the *real* `pocclusters.scene.json` + the pair script and walk a
    //! frame, so a broken tree, a Lua syntax/runtime error, or a raw English
    //! string fails the build instead of only surfacing in the running app.
    use super::*;

    /// The shipped scene file, read by the gates exactly as the manifest reads it.
    const POCCLUSTERS_SCENE: &str =
        include_str!("../../../../content/sensorium/scenes/pocclusters.scene.json");

    fn load_strings() {
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");
    }

    fn scene_def() -> SceneDef {
        SceneDef::parse("pocclusters", POCCLUSTERS_SCENE).expect("pocclusters.scene.json loads")
    }

    /// The loader is repointed at the ISLAND bake set (`bakes_island/`), not
    /// the old wave-field `bakes/`.
    #[test]
    fn the_loader_reads_the_island_bake_set() {
        assert_eq!(ISLAND_BAKES, "bakes_island");
        assert!(
            bake_dir_path().ends_with(ISLAND_BAKES),
            "bake_dir_path points at the island set: {:?}",
            bake_dir_path()
        );
    }

    /// All nine island bakes load through the real gz-at-rest seam
    /// ([`try_load_bake_field`]) and each carries a contoured surface bulk-
    /// filled with Gravel — proof the `bake_island` output is what the scene
    /// picks up. Resolved compile-time-relative (like the other gates) so the
    /// test doesn't lean on process-global roots state.
    #[test]
    fn the_nine_island_bakes_load_and_contour_nonempty() {
        let dir = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/package/bakes_island"
        ));
        let ids: Vec<ClusterId> = (0..FIELD_DIM)
            .flat_map(|x| (0..FIELD_DIM).map(move |z| ClusterId::new(0, x, 0, z)))
            .collect();
        let loaded = try_load_bake_field(&dir, &ids).expect(
            "all nine island bakes load — run `cargo run -p flicker-voxel --bin bake_island`",
        );
        assert_eq!(loaded.len(), 9, "one bake per cell of the 3×3 field");
        let gravel = Material::new(23, 23, 0).raw();
        for (id, cluster) in &loaded {
            assert!(
                cluster.override_count() > 0,
                "island cluster {id:?} contoured to a non-empty surface"
            );
            assert_eq!(
                cluster.default_material().raw(),
                gravel,
                "island clusters are bulk-filled with Gravel"
            );
        }
    }

    /// THE PAIR-SCRIPT REGRESSION GATE: build the bench exactly as the resolver
    /// does (real def, real pocclusters.lua) and run the REAL hud_model path —
    /// the raw variables must come back as the DERIVED display strings the tree
    /// binds. A derive() that throws leaves numbers (or nothing) under the keys.
    #[test]
    fn the_pair_script_derives_the_display_strings() {
        load_strings();
        let def = scene_def();
        assert_eq!(def.behaviour, "pocclusters");
        let scene = GameScene::new(&def);
        assert!(
            scene.script.is_some(),
            "pocclusters.lua loads (the pair script)"
        );
        assert!(
            scene.ui_tree.is_some(),
            "the scene file carries the HUD tree"
        );
        let m = scene.hud_model();
        for key in [
            "title_line",
            "cam_val",
            "grid_val",
            "move_val",
            "diag_val",
            "speed_val",
        ] {
            assert!(
                m.text(key).is_some(),
                "derive() must yield display TEXT for '{key}' — got {:?}",
                m.number(key).map(|n| format!("Number({n})"))
            );
        }
        let title = m.text("title_line").unwrap();
        assert!(
            title.contains("3×3"),
            "the title composes the field size: {title:?}"
        );
        assert!(
            m.text("grid_val").unwrap().contains("256³"),
            "the grid line composes the cluster dimension"
        );
        assert_eq!(
            m.text("pick_val"),
            Some(""),
            "no pick yet → the pick readout is empty (its row is gated)"
        );
        assert!(
            m.text("walk_val").is_some_and(|s| !s.is_empty()),
            "walk is the default → a derived walk readout is present: {:?}",
            m.text("walk_val")
        );
    }

    /// Walk the REAL tree with the REAL derived model (+ a pick fixture so the
    /// gated inspector panel draws too) and gate the authored data: known kinds
    /// only, no raw display literals in the tree, no raw display copy published
    /// from Rust into the Model.
    #[test]
    fn hud_tree_walks_with_model() {
        load_strings();
        let def = scene_def();
        let tree = def.tree.clone().expect("scene defines a tree");
        let styles = flicker::ui::load_shared_styles(def.styles.as_ref());

        // Vocabulary gate: an unknown kind renders NOTHING, so a name left
        // behind by a rename would be invisible until someone opened the window.
        assert!(
            flicker::ui::unknown_kinds(&tree).is_empty(),
            "pocclusters.scene.json names unknown kinds: {:?}",
            flicker::ui::unknown_kinds(&tree)
        );
        // The strings gate (S10): every display literal is a `$token`.
        assert!(
            flicker::ui::raw_display_literals(&tree).is_empty(),
            "pocclusters.scene.json ships raw display literals: {:?}",
            flicker::ui::raw_display_literals(&tree)
        );
        // The MODEL-CHANNEL strings gate (S10's blind side): every `.set`/`.with`
        // display value in this crate is a resolved `$token`, a data shape, or
        // carries an explicit `strings-gate-exempt` reason.
        let flags = strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(
            flags.is_empty(),
            "raw display copy published into the Model: {flags:?}"
        );

        // The real derived model, plus a pick fixture so the `has_pick`-gated
        // inspector panel draws (a real pick needs a meshed world; the fixture
        // exercises the authored panel with the same key shapes derive() emits).
        let scene = GameScene::new(&def);
        let mut m = scene.hud_model();
        m.set("has_pick", true);
        m.set("no_pick", false);
        let r = |t: &str| strings::resolve(t).into_owned();
        m.set("insp_title", format!("{} (10, 20, 30)", r("$pc_cell")));
        m.set(
            "insp_sub",
            format!(
                "{} (1, 0, 2) {} 0 · 8 {}",
                r("$pc_cluster"),
                r("$pc_lod"),
                r("$pc_corners")
            ),
        );
        for i in 0..8 {
            m.set(format!("insp_c{i}_name"), format!("c{i} +--"));
            for ax in ["lx", "ly", "lz", "wx", "wy", "wz"] {
                m.set(format!("insp_c{i}_{ax}"), "0.50");
            }
        }

        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            right_down: false,
            screen: Vec2::new(1920.0, 1080.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &m, &styles, &snap, &mut UiState::new());
        assert!(
            !frame.commands.is_empty(),
            "the HUD draws its panels + controls"
        );
        let has_text = |needle: &str| {
            frame
                .commands
                .iter()
                .any(|c| matches!(c, HudCommand::Text { text, .. } if text.contains(needle)))
        };
        assert!(has_text("Cluster field"), "the title line renders");
        assert!(
            has_text("Wireframe overlay"),
            "the authored checkbox labels render"
        );
        assert!(has_text("Celestial Cycle"), "the celestial panel renders");
        assert!(has_text("Corner"), "the inspector table header renders");
        assert!(
            has_text("Cell (10, 20, 30)"),
            "the inspector panel renders while has_pick is set"
        );
    }
}

// ── Ground fog — the slab the root stage's recipe lays over the field ─────

/// The lowest walkable floor over every nav surface of the field — the level the ground
/// fog lies at. `None` until a nav has arrived. Costs one sweep per field rebuild, never
/// per frame.
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

/// The ROOT stage's own gates — everything the shipped `stages.pocclusters_world`
/// promises and this scene has to keep: the fog slab's binds, and the room's authored
/// light rig surviving the Celestial Cycle that composes over it.
#[cfg(test)]
mod stage_tests {
    use super::*;

    use flicker::render::{LightKind, LightRig, PassKind, WaveKind};

    /// **GATE — the ROOT stage authors the ground fog, and every number it does NOT
    /// author is one this scene publishes.** The whole slab now lives in
    /// `pocclusters.scene.json`'s `stages.pocclusters_world` recipe, so this compiles the
    /// REAL shipped file through the REAL loader path and proves: the recipe is clean, the
    /// DERIVED order is sky-then-content-then-fog (a fog running first would sample a
    /// depth nothing had written, and the sky is a PASS, not a `draw_sky()` line beside
    /// the recipe), every `*_bind` it names is a key `stage_inputs` actually publishes
    /// (the one way an authored name resolves to nothing and ships the fog frozen), and
    /// the slab those binds resolve to is still the one the field was tuned for.
    #[test]
    fn the_root_stage_authors_the_ground_fog_and_publishes_its_binds() {
        let def = SceneDef::parse(
            "pocclusters",
            include_str!("../../../../content/sensorium/scenes/pocclusters.scene.json"),
        )
        .expect("pocclusters.scene.json parses");
        // The styles root the runtime builds for THIS scene: the shared theme and its
        // satellites, with the scene's own `stages` section folded in.
        let styles = flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        );
        let (stage, problems) = flicker::ui::compile_stage(&styles, WORLD_STAGE)
            .unwrap_or_else(|| panic!("stages.{WORLD_STAGE} is authored"));
        assert!(
            problems.is_empty(),
            "stages.{WORLD_STAGE}:\n  {}",
            problems.join("\n  ")
        );

        // The recipe, in the order the frame graph runs it.
        let (order, cyclic) = stage.pass_order();
        assert!(!cyclic, "the recipe has a read/write cycle");
        let kinds: Vec<&str> = order
            .iter()
            .map(|&i| stage.recipe()[i].kind.kind())
            .collect();
        assert_eq!(
            kinds,
            [
                "sky",
                "shadow_map",
                "scene",
                "water_surface",
                "ground_fog",
                "bloom",
                "tonemap_grade"
            ],
            "the sky is behind the world, the sun-shadow consumer binds before the lit \
             `scene` samples it, the WATER floods over the scene it reads the depth of \
             (after `scene`, before the fog that melts its far edge into the horizon), the \
             fog reads the depth the world wrote, then BLOOM adds the glow of the bright HDR \
             highlights (the sun glint, the sun disc) back into the working colour before the \
             tonemap resolves it last"
        );
        assert!(
            stage.camera.is_none(),
            "the Celestial Cycle owns the framing — the stage authors none"
        );

        // Every input key the recipe binds, whatever pass binds it — including the
        // `tonemap_grade`, whose grade strength now RIDES the golden-hour curve.
        let bound: Vec<&str> = stage
            .recipe()
            .iter()
            .flat_map(|pass| match &pass.kind {
                PassKind::GroundFog(f) => {
                    f.binds.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>()
                }
                PassKind::VolumetricDisk(v) => v.binds.iter().map(|(_, k)| k.as_str()).collect(),
                PassKind::TonemapGrade(t) => t.binds.iter().map(|(_, k)| k.as_str()).collect(),
                _ => Vec::new(),
            })
            .collect();
        // Published from a scene whose clock has actually ADVANCED, so the typed clock
        // channel is proven to carry this scene's own accumulator rather than a zero.
        let mut scene = GameScene::new(&def);
        scene.fog_time = 12.5;
        let inputs = scene.stage_inputs();
        assert_eq!(
            inputs.clock_seconds(),
            12.5,
            "the stage clock the light drivers run on is the scene's OWN `fog_time` — \
             typed simulation output, never a wall clock"
        );
        assert_eq!(
            inputs.get("fog_time"),
            Some(inputs.clock_seconds()),
            "ONE scene clock: the fog's drift and the hearth's flicker read the same \
             accumulator, so they can never drift apart"
        );
        let published: Vec<&str> = inputs.keys().collect();
        assert_eq!(
            bound,
            ["fog_floor", "fog_density", "fog_time", "grade_warmth"],
            "the fog binds its floor, its density and its clock; the tonemap binds the \
             golden-hour warmth its grade strength rides"
        );
        for key in &bound {
            assert!(
                published.contains(key),
                "stages.{WORLD_STAGE} binds `{key}`, which the scene never publishes \
                 (it publishes {published:?})"
            );
        }

        // GOLDEN HOUR, on the REAL channel: the shipped recipe's tonemap authors a golden
        // TINT and NO strength (a number there would be dead data — the compiler says so),
        // and the strength it resolves to at this frame's sun IS the warmth the Celestial
        // Cycle published. A rename on either side, or an authored strength shadowing the
        // bind, fails here rather than shipping a frame that grades the same all day.
        let PassKind::TonemapGrade(tonemap) = &stage.recipe()[order[6]].kind else {
            unreachable!(
                "pass 6 (of sky, shadow, scene, water, fog, bloom, tonemap) is the tonemap"
            )
        };
        // The tint is AARON'S ART, retuned in-window — so this gate must NOT pin its three
        // numbers. Pinning them made the scene file's own promise ("tuned in-window, never in
        // Rust") a lie: every art pass on the grade broke a Rust test, which is precisely the
        // pressure that pushes art back into Rust. What is actually load-bearing is that the
        // tint travelled the REAL channel — that the parser read this line at all rather than
        // leaving `TonemapGradePass::default()`'s neutral `Vec3::ZERO` standing, which would
        // resolve to a lerp toward BLACK and quietly darken the golden hour instead of warming
        // it. So: non-default, and a plausible WARM tint (red the strongest channel), with the
        // exact values left free.
        assert_ne!(
            tonemap.grade,
            flicker::render::TonemapGradePass::default().grade,
            "the tonemap's `grade` is still the neutral default — the authored tint never \
             reached the runtime (a typo'd key, or the field dropped from the scene file)"
        );
        assert!(
            tonemap.grade.x > tonemap.grade.y && tonemap.grade.y > tonemap.grade.z,
            "the authored grade must still be a WARM tint (R > G > B) — the golden hour is what \
             the bound strength ramps toward; the exact numbers are art, tuned in the scene \
             file, and are deliberately NOT pinned here. Got {:?}",
            tonemap.grade
        );
        assert_eq!(
            tonemap.binds,
            vec![(
                flicker::render::TonemapSlot::GradeStrength,
                "grade_warmth".to_string()
            )],
            "the strength is BOUND (and the exposure is left neutral)"
        );
        let (tint, strength, exposure) = tonemap.resolve(&inputs);
        assert_eq!(tint, tonemap.grade);
        assert_eq!(exposure, 1.0, "unauthored exposure stays neutral");
        assert_eq!(
            strength,
            scene.celestial.grade_warmth(),
            "the resolved grade strength IS the cycle's published golden-hour warmth"
        );

        // The slab those binds resolve to: two units below the floor the nav sweep
        // found, twelve above it, at the Celestial Cycle's own default fog, in the
        // horizon colour of whatever the sky is doing, localized to the cluster field.
        let PassKind::GroundFog(fog) = &stage.recipe()[order[4]].kind else {
            unreachable!("pass 4 (of sky, shadow_map, scene, water, fog, tonemap) is the fog")
        };
        let resolved = fog.resolve(&inputs, Vec3::new(0.1, 0.2, 0.3));
        assert_eq!((resolved.bottom, resolved.top), (-2.0, 12.0));
        assert_eq!(resolved.density, 1.0, "the Fog control at its default");
        assert_eq!(resolved.color, Vec3::new(0.1, 0.2, 0.3), "the live horizon");
        let field = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        assert_eq!(
            (resolved.bounds_min, resolved.bounds_max),
            (Vec2::ZERO, Vec2::splat(field)),
            "the fog is localized to the cluster field"
        );
        assert!(resolved.edge_fade < field / 2.0);
        assert!(nav_floor_min(&[]).is_none(), "no nav, no floor");
    }

    /// **GATE — the authored sun-shadow knobs reach the runtime, on the REAL channel.**
    /// `shadow_knobs` is exactly what `render` reads every frame to drive the shadow, so this
    /// compiles the SHIPPED `pocclusters.scene.json` through the REAL loader and proves the
    /// values the code reads ARE the ones authored — the CONSUMER's `light` + `bias` (the world
    /// recipe's `shadow_map` line) and the PRODUCER's `extent` (`pocclusters_sun_shadow`), each
    /// from its ONE authority. A bias/light authored on the wrong stage, or drifted from these
    /// numbers, fails here — no silent second source (rule AEEF2A68), and the gate covers the
    /// channel the drift travels rather than a Rust re-derivation (rule 8634C200).
    #[test]
    fn the_authored_shadow_knobs_reach_the_runtime() {
        let def = SceneDef::parse(
            "pocclusters",
            include_str!("../../../../content/sensorium/scenes/pocclusters.scene.json"),
        )
        .expect("pocclusters.scene.json parses");
        let styles = flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        );
        let (world, wp) = flicker::ui::compile_stage(&styles, WORLD_STAGE)
            .unwrap_or_else(|| panic!("stages.{WORLD_STAGE} is authored"));
        assert!(
            wp.is_empty(),
            "stages.{WORLD_STAGE}:\n  {}",
            wp.join("\n  ")
        );
        let (producer, pp) = flicker::ui::compile_stage(&styles, SHADOW_STAGE)
            .unwrap_or_else(|| panic!("stages.{SHADOW_STAGE} is authored"));
        assert!(
            pp.is_empty(),
            "stages.{SHADOW_STAGE}:\n  {}",
            pp.join("\n  ")
        );

        // The two roles the two-authority read relies on: the producer stage renders the depth
        // (a producer `shadow_map`, `from: none`); the world recipe samples it (a consumer
        // `shadow_map` naming the producer surface).
        assert!(
            producer
                .recipe()
                .iter()
                .any(|p| matches!(&p.kind, PassKind::ShadowMap(s) if s.from.is_none())),
            "the producer stage authors a producer (from: none) shadow_map pass"
        );
        assert!(
            world.recipe().iter().any(|p| {
                matches!(&p.kind, PassKind::ShadowMap(s) if s.from.as_deref() == Some(SHADOW_STAGE))
            }),
            "the world recipe's consumer shadow_map names the producer surface"
        );

        // THE channel: exactly what `render` feeds the shadow this frame — light + bias from
        // the consumer line, extent from the producer stage, one authored value each. (Bias
        // rides an f64→f32 cast in the parser, so it is checked to tolerance, not bit-equal.)
        let (light, bias, extent) =
            shadow_knobs(&world, &producer).expect("both shadow roles are authored");
        assert_eq!(
            light, 0,
            "the runtime reads the CONSUMER's authored light 0"
        );
        assert!(
            (bias - 0.0015).abs() < 1e-6,
            "the runtime reads the CONSUMER's authored bias 0.0015, got {bias}"
        );
        assert_eq!(
            extent, 640.0,
            "the runtime reads the PRODUCER's authored extent 640"
        );
    }

    /// **GATE — the water floods the island at sea level 120 with animated waves.** Compiles
    /// the SHIPPED `pocclusters.scene.json` through the REAL loader and proves the water demo is
    /// wired end to end: the world recipe derives the `water_surface` AFTER the lit `scene` (it
    /// reads that depth) and BEFORE `ground_fog` (the fog melts its far edge) and the tonemap
    /// (it writes the `hdr` the tonemap reads); the water's `sea_level` is 120 and it carries a
    /// real wave roster — three RADIAL sources ringing the island plus two DIRECTIONAL
    /// open-ocean swells, each with a positive amplitude and a computed wavenumber. The two
    /// directional entries are what keeps the horizon band moving instead of reading as a dead
    /// glass mirror, so their presence (and their LONG wavelengths, which is what stops the far
    /// field aliasing) is asserted, not just the count. The OLD reflection mirror is GONE — the
    /// animated water is real geometry with a sky-slot specular, not a reflection RT — so the
    /// scene authors no `pocclusters_reflect`.
    #[test]
    fn the_water_floods_the_island_with_animated_waves() {
        let def = SceneDef::parse(
            "pocclusters",
            include_str!("../../../../content/sensorium/scenes/pocclusters.scene.json"),
        )
        .expect("pocclusters.scene.json parses");
        let styles = flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        );
        let (world, wp) = flicker::ui::compile_stage(&styles, WORLD_STAGE)
            .unwrap_or_else(|| panic!("stages.{WORLD_STAGE} is authored"));
        assert!(
            wp.is_empty(),
            "stages.{WORLD_STAGE}:\n  {}",
            wp.join("\n  ")
        );

        // The DERIVED order (the only ordering the recipe carries): water after the scene it
        // reads the depth of, before the fog and the tonemap.
        let (order, cyclic) = world.pass_order();
        assert!(!cyclic, "the world recipe has a read/write cycle");
        let kinds: Vec<&str> = order
            .iter()
            .map(|&i| world.recipe()[i].kind.kind())
            .collect();
        let pos = |k: &str| kinds.iter().position(|&x| x == k);
        let (scene_i, water_i, fog_i, tone_i) = (
            pos("scene").expect("a scene pass"),
            pos("water_surface").expect("stages.pocclusters_world declares a water_surface pass"),
            pos("ground_fog").expect("a ground_fog pass"),
            pos("tonemap_grade").expect("a tonemap_grade pass"),
        );
        assert!(
            scene_i < water_i && water_i < fog_i && water_i < tone_i,
            "water derives after `scene` and before `ground_fog`/`tonemap_grade`, got {kinds:?}"
        );

        // The water pass carries its authored knobs on the real channel: reads depth, writes
        // hdr, sea_level 120, and a real wave roster.
        let water_pass = world
            .recipe()
            .iter()
            .find(|p| matches!(p.kind, PassKind::WaterSurface(_)))
            .expect("the world recipe declares a water_surface pass");
        assert!(
            water_pass.reads.iter().any(|r| r == "depth")
                && water_pass.writes.iter().any(|w| w == "hdr"),
            "the water reads the scene depth and writes the hdr the tonemap resolves"
        );
        let PassKind::WaterSurface(w) = &water_pass.kind else {
            unreachable!("matched WaterSurface above")
        };
        assert!(
            (w.sea_level - 120.0).abs() < 1e-6,
            "the flood height is the authored sea level 120, got {}",
            w.sea_level
        );
        assert_eq!(
            w.waves.len(),
            5,
            "five wave sources: three radial around the island + two directional swells"
        );
        for (i, s) in w.waves.iter().enumerate() {
            assert!(s.amplitude > 0.0, "wave source {i} has a real amplitude");
            assert!(
                s.k > 0.0,
                "wave source {i} has a real wavenumber (2π/wavelength)"
            );
        }
        let radial = w
            .waves
            .iter()
            .filter(|s| matches!(s.kind, WaveKind::Radial { .. }))
            .count();
        let directional: Vec<_> = w
            .waves
            .iter()
            .filter(|s| matches!(s.kind, WaveKind::Directional { .. }))
            .collect();
        assert_eq!(radial, 3, "three RADIAL sources ring the island");
        assert_eq!(
            directional.len(),
            2,
            "two DIRECTIONAL sources carry the open-ocean swell — without them the horizon \
             band is a dead glass mirror again"
        );
        for s in &directional {
            let WaveKind::Directional { dir } = s.kind else {
                unreachable!("filtered on the kind above")
            };
            assert!(
                (dir.length() - 1.0).abs() < 1e-5,
                "an ambient swell's direction is normalized at parse: {dir:?}"
            );
            // λ = 2π/k. LONG is load-bearing: a short far-field wavelength lands under a pixel
            // out at the horizon and shimmers instead of swelling.
            let wavelength = std::f32::consts::TAU / s.k;
            assert!(
                wavelength > 100.0,
                "the open-ocean swell is authored LONG (λ > 100) so the far field does not \
                 alias; got {wavelength}"
            );
        }

        // ENVIRONMENT-LIT: the sea is no longer a flat authored ramp — it mirrors the LIVE sky
        // by Fresnel, and `env_strength` is the one dial on that. It reaches the runtime on the
        // real channel (`resolve`, exactly what the frame graph hands the renderer), so an
        // authored 0 that would silently switch the reflection back off fails here.
        let mut sim = GameScene::new(&def);
        sim.fog_time = 12.5;
        let resolved = w.resolve(&sim.stage_inputs());
        assert!(
            resolved.env_strength > 0.0 && resolved.env_strength <= 1.0,
            "the water mirrors the sky (env_strength in 0..1, and not off): {}",
            resolved.env_strength
        );

        // The OLD reflection mirror is GONE: the scene authors no `pocclusters_reflect` stage.
        assert!(
            !def.stages()
                .into_iter()
                .any(|s| s.contains_key("pocclusters_reflect")),
            "the reflection mirror stage is deleted — the water is real geometry now"
        );
    }

    /// **GATE — the room's own fire survives the Celestial Cycle.** The Prism Test Room
    /// is the first fireplace-class stage: the rig is AUTHORED (`stages.lighting.hearth`)
    /// and the cycle COMPOSES over it instead of replacing it. The regression that costs
    /// nothing to make and reports nothing at runtime is `over` going back to handing out
    /// a fresh rig — the fire would simply stop existing. So build the rig through the
    /// REAL path, in the order a frame runs it: compile the SHIPPED
    /// `pocclusters_world` stage → `driven(t)` (what `FrameGraph::surface` does with the
    /// clock this scene publishes) → `celestial.over(...)` (what `draw_world` does) —
    /// and prove the hearth is still there, still flickering, at the same count, while
    /// slots 0/1 are the cycle's and nobody else's.
    #[test]
    fn the_hearth_survives_the_celestial_composition() {
        let def = SceneDef::parse(
            "pocclusters",
            include_str!("../../../../content/sensorium/scenes/pocclusters.scene.json"),
        )
        .expect("pocclusters.scene.json parses");
        let styles = flicker::ui::load_styles_for(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../content/sensorium/resources/ui_theme.json"
            ),
            def.styles.as_ref(),
        );
        let (stage, problems) = flicker::ui::compile_stage(&styles, WORLD_STAGE)
            .unwrap_or_else(|| panic!("stages.{WORLD_STAGE} is authored"));
        assert!(problems.is_empty(), "{}", problems.join("\n  "));

        // What the shipped preset compiles to: the two slots the cycle owns, then the
        // fire. The ORDER is the contract — `over` writes slots 0 and 1 by INDEX.
        let authored = stage.lighting;
        assert_eq!(
            authored.count, 3,
            "`hearth` is the sun slot, the moon slot, and the fire"
        );
        for (i, body) in [(0usize, "sun"), (1, "moon")] {
            assert_eq!(
                authored.lights[i].kind,
                LightKind::Dir,
                "slot {i} is the {body} — a directional slot the cycle overwrites"
            );
            assert_eq!(
                authored.lights[i].color,
                Vec3::ZERO,
                "slot {i} (the {body}) is RESERVED for the cycle, so the preset authors \
                 it black — a lit one would be a light nobody authored for this room"
            );
        }

        let hearth = authored.lights[2];
        assert_eq!(hearth.kind, LightKind::Point, "the fire is a point light");
        assert!(
            hearth.radius > 0.0,
            "the hearth has real falloff — the one thing that makes `intensity` mean \
             anything"
        );
        assert!(
            hearth.intensity > 1.0,
            "an authored falloff radius costs the colour-carries-the-magnitude \
             convention: a fire's intensity is in the tens, not {}",
            hearth.intensity
        );
        // It stands ON the field's floor. The world heightmap is the truth about that
        // surface (the field is generated from it), so ask it rather than guessing.
        let field = FIELD_DIM as f32 * CLUSTER_DIM as f32;
        assert!(
            (0.0..field).contains(&hearth.position.x) && (0.0..field).contains(&hearth.position.z),
            "the hearth at {:?} is outside the {field}-unit cluster field",
            hearth.position
        );
        let ground = flicker_voxel::heightmap::world_height_seeded(
            hearth.position.x,
            hearth.position.z,
            flicker_voxel::heightmap::DEFAULT_SEED,
        );
        assert!(
            hearth.position.y > ground && hearth.position.y < ground + 8.0,
            "the hearth sits at y={} over terrain at y={ground} — a fire belongs just \
             above the walkable floor, neither buried in it nor hanging over it",
            hearth.position.y
        );

        // Step one: the frame graph drives the rig against the scene's clock.
        let t = 3.25_f32;
        let driver = hearth.driver.expect("the hearth flickers");
        let gain = driver.gain(t);
        assert!(
            gain > 0.0 && gain < 1.0,
            "a `flicker` driver only ever DIMS a fire (gain {gain} at t={t})"
        );
        let driven = authored.driven(t);
        assert_eq!(
            driven.lights[2].intensity,
            hearth.intensity * gain,
            "the fire's intensity for this frame is its authored one times its gain"
        );

        // Step two: the cycle composes over it.
        let cycle = CelestialState::default();
        let composed = cycle.over(driven);
        assert_eq!(
            composed.count, driven.count,
            "composition never changes the light COUNT — every authored light stands"
        );
        assert_eq!(
            composed.lights[2], driven.lights[2],
            "the hearth — position, colour, falloff, driver, driven intensity — is \
             untouched by the cycle"
        );

        // Slots 0/1 are the CYCLE's, whatever the stage authored: the same cycle
        // composed over a bare default rig must produce the identical two lights.
        let reference = cycle.over(LightRig::default());
        assert_eq!(
            composed.lights[0], reference.lights[0],
            "slot 0 is the cycle's sun, not the preset's"
        );
        assert_eq!(
            composed.lights[1], reference.lights[1],
            "slot 1 is the cycle's moon, not the preset's"
        );
        assert_eq!(
            composed.sky_sun(),
            composed.lights[0],
            "the sky pass reads the cycle's sun back out of slot 0"
        );
        assert_eq!(
            composed.ambient, reference.ambient,
            "the cycle owns ambient"
        );
        assert_eq!(
            composed.fog_color, reference.fog_color,
            "and the fog colour"
        );
        assert_eq!(
            composed.fog_density, reference.fog_density,
            "and the fog density"
        );
        assert_ne!(
            composed.lights[0].color, authored.lights[0].color,
            "the cycle's morning sun actually replaced the reserved black slot"
        );
    }
}
